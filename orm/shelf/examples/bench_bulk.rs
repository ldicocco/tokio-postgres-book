//! Bulk writes through shelf: pgload's rows (main book Chapter 10) loaded five ways.
//!
//!     cargo run -p shelf --release --example bench_bulk -- --rows 20000 [--keyed]
//!     cargo run -p lagproxy -- --listen 127.0.0.1:6544 --delay-ms 1
//!     DATABASE_URL=postgres://…@localhost:6544/tpbook \
//!         cargo run -p shelf --release --example bench_bulk -- --rows 2000
//!
//! The rows and the table are pgload's (`id`, `sku`, a `name` with commas and quotes,
//! `price_cents`, `created_at`, in a temporary table without constraints), so that the table
//! it prints compares with pgload's. `--keyed` loads into a regular table with a primary key
//! and `NOT NULL` columns instead: what WAL and an index add. Each strategy runs three times on
//! one pooled connection (deadpool's statement cache), each run in a transaction that is rolled
//! back, and the median is printed as a Markdown table. Every strategy must store the same rows:
//! a count and an md5 fingerprint of the table, read inside the transaction, are compared.

use std::time::{Duration, Instant};

use futures_util::{StreamExt, TryStreamExt, stream};
use jiff::{SignedDuration, Timestamp};
use shelf::{FromRow, Insert, Table};
use tokio_postgres::{Config, NoTls};

#[derive(Debug, FromRow, Table)]
#[shelf(table = "load_target")]
#[allow(dead_code)] // only the writing is measured
struct Product {
    #[shelf(primary_key)]
    id: i64,
    sku: String,
    name: String,
    price_cents: i64,
    created_at: Timestamp,
}

#[derive(Insert)]
#[shelf(model = Product)]
struct NewProduct {
    id: i64,
    sku: String,
    name: String,
    price_cents: i64,
    created_at: Timestamp,
}

/// pgload's rows, with the prices and times spread by a fixed rule instead of a seeded RNG.
fn generate(n: usize) -> Vec<NewProduct> {
    let start = Timestamp::from_second(1_767_225_600).expect("a valid timestamp"); // 2026-01-01
    (0..n)
        .map(|i| NewProduct {
            id: i as i64 + 1,
            sku: format!("SKU{:06}", i + 1),
            name: format!(
                "Product {}, \"{}\" edition",
                i + 1,
                ["blue", "green", "red"][i % 3]
            ),
            price_cents: 99 + (i as i64 * 7_919) % 99_901,
            created_at: start
                .checked_add(SignedDuration::from_secs(
                    (i as i64 * 104_729) % (86_400 * 300),
                ))
                .expect("in range"),
        })
        .collect()
}

#[derive(Clone, Copy)]
enum Strategy {
    Loop,
    Pipelined,
    ManyBatches,
    ManyOnce,
    Copy,
}

impl Strategy {
    const ALL: [Strategy; 5] = [
        Strategy::Loop,
        Strategy::Pipelined,
        Strategy::ManyBatches,
        Strategy::ManyOnce,
        Strategy::Copy,
    ];

    fn name(self) -> &'static str {
        match self {
            Strategy::Loop => "`insert`, one at a time",
            Strategy::Pipelined => "`insert`, pipelined (64 in flight)",
            Strategy::ManyBatches => "`insert_many`, batches of 1,000",
            Strategy::ManyOnce => "`insert_many`, one call",
            Strategy::Copy => "`copy_in` (binary COPY)",
        }
    }
}

const FINGERPRINT: &str = "SELECT count(*), md5(string_agg(
    concat_ws('|', id, sku, name, price_cents, created_at), ',' ORDER BY id))
    FROM load_target";

fn median(mut times: Vec<Duration>) -> Duration {
    times.sort();
    times[times.len() / 2]
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _ = dotenvy::dotenv();
    let url = std::env::var("DATABASE_URL").expect("set DATABASE_URL (or put it in .env)");
    let args: Vec<String> = std::env::args().collect();
    let rows: usize = match args.iter().position(|a| a == "--rows") {
        Some(i) => args.get(i + 1).expect("--rows N").parse()?,
        None => 20_000,
    };
    let keyed = args.iter().any(|a| a == "--keyed");
    let schema = format!("shelf_bench_{}", std::process::id());
    let mut config: Config = url.parse()?;
    let (setup, connection) = config.connect(NoTls).await?;
    tokio::spawn(connection);
    setup
        .batch_execute(&format!("CREATE SCHEMA {schema}"))
        .await?;
    config.options(format!("-c search_path={schema}"));
    let pool = deadpool_postgres::Pool::builder(deadpool_postgres::Manager::new(config, NoTls))
        .max_size(1)
        .build()?;
    let mut db = pool.get().await?;
    db.batch_execute(if keyed {
        "CREATE TABLE load_target (
             id bigint PRIMARY KEY, sku text NOT NULL, name text NOT NULL,
             price_cents bigint NOT NULL, created_at timestamptz NOT NULL)"
    } else {
        // pgload's: no WAL, no index, no constraints (on the pool's one connection).
        "CREATE TEMP TABLE load_target (
             id bigint, sku text, name text, price_cents bigint, created_at timestamptz)"
    })
    .await?;

    let batch = generate(rows);
    let mut fingerprints = Vec::new();
    let mut results = Vec::new();
    for strategy in Strategy::ALL {
        let mut times = Vec::new();
        for _ in 0..3 {
            let tx = db.transaction().await?;
            let t = Instant::now();
            match strategy {
                Strategy::Loop => {
                    for row in &batch {
                        row.insert(&tx).await?;
                    }
                }
                Strategy::Pipelined => {
                    let tx = &tx;
                    stream::iter(&batch)
                        .map(|row| row.insert(tx))
                        .buffered(64)
                        .try_for_each(|_| async { Ok(()) })
                        .await?;
                }
                Strategy::ManyBatches => {
                    for chunk in batch.chunks(1_000) {
                        NewProduct::insert_many(&tx, chunk).await?;
                    }
                }
                Strategy::ManyOnce => {
                    NewProduct::insert_many(&tx, &batch).await?;
                }
                Strategy::Copy => {
                    NewProduct::copy_in(&tx, &batch).await?;
                }
            }
            times.push(t.elapsed());
            let row = tx.query_one(FINGERPRINT, &[]).await?;
            fingerprints.push((row.get::<_, i64>(0), row.get::<_, String>(1)));
            tx.rollback().await?;
        }
        results.push((strategy.name(), median(times)));
    }
    assert_eq!(fingerprints[0].0, rows as i64);
    assert!(
        fingerprints.iter().all(|f| *f == fingerprints[0]),
        "every strategy stores the same rows"
    );

    let fastest = results.iter().map(|(_, t)| *t).min().expect("five results");
    let table = if keyed {
        "a keyed table"
    } else {
        "pgload's table"
    };
    println!("| {rows} rows, {table} | Median of 3 | × the fastest |");
    println!("|---|---:|---:|");
    for (name, time) in results {
        println!(
            "| {name} | {:.3} s | {:.1} |",
            time.as_secs_f64(),
            time.as_secs_f64() / fastest.as_secs_f64()
        );
    }

    drop(db);
    setup
        .batch_execute(&format!("DROP SCHEMA {schema} CASCADE"))
        .await?;
    Ok(())
}
