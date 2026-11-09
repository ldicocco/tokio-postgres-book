//! What N+1 costs: 100 orders with 3 lines each, their lines loaded five ways.
//!
//!     cargo run -p lagproxy -- --delay-ms 5            # :6543 → :5432, +5 ms each way
//!     DATABASE_URL=postgres://…@localhost:6543/tpbook \
//!         cargo run -p shelf --release --example bench_relations
//!
//! It reads `DATABASE_URL` (or `.env`): point it at lagproxy to see round trips cost what they
//! cost over a network, or at the server to see them nearly free. The tables live in a schema
//! of their own, dropped at the end, so that a pool's connections see them too. Each strategy
//! runs three times, and the median is printed as a Markdown table (Chapter 11 shows it).

use std::time::{Duration, Instant};

use futures_util::future::try_join_all;
use shelf::{FromRow, Table};
use tokio_postgres::{Config, NoTls};

const ORDERS: i64 = 100;

#[derive(Debug, FromRow, Table)]
#[shelf(table = "orders", has_many(OrderLine, key = order_id, as = lines))]
struct Order {
    #[shelf(primary_key)]
    id: i64,
    customer: String,
}

#[derive(Debug, FromRow, Table)]
#[shelf(table = "order_lines")]
#[allow(dead_code)] // only the loading is measured
struct OrderLine {
    #[shelf(primary_key)]
    id: i64,
    order_id: i64,
    sku: String,
    quantity: i32,
}

fn median(mut times: Vec<Duration>) -> f64 {
    times.sort();
    times[times.len() / 2].as_secs_f64() * 1e3
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _ = dotenvy::dotenv();
    let url = std::env::var("DATABASE_URL").expect("set DATABASE_URL (or put it in .env)");
    let schema = format!("shelf_bench_{}", std::process::id());
    let mut config: Config = url.parse()?;
    let (setup, connection) = config.connect(NoTls).await?;
    tokio::spawn(connection);
    setup
        .batch_execute(&format!(
            "CREATE SCHEMA {schema};
             CREATE TABLE {schema}.orders (id bigint PRIMARY KEY, customer text NOT NULL);
             CREATE TABLE {schema}.order_lines (
                 id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
                 order_id bigint NOT NULL REFERENCES {schema}.orders,
                 sku text NOT NULL,
                 quantity integer NOT NULL);
             CREATE INDEX ON {schema}.order_lines (order_id);
             INSERT INTO {schema}.orders SELECT g, 'customer ' || g FROM generate_series(1, {ORDERS}) g;
             INSERT INTO {schema}.order_lines (order_id, sku, quantity)
                 SELECT o, 'SKU-' || l, l FROM generate_series(1, {ORDERS}) o, generate_series(1, 3) l;
             ANALYZE {schema}.orders, {schema}.order_lines;"
        ))
        .await?;
    config.options(format!("-c search_path={schema}"));
    let (client, connection) = config.connect(NoTls).await?;
    tokio::spawn(connection);
    let pool =
        deadpool_postgres::Pool::builder(deadpool_postgres::Manager::new(config.clone(), NoTls))
            .max_size(1)
            .build()?;
    let pooled = pool.get().await?;

    let orders = Order::all(&client).await?;
    let mut results: Vec<(&str, &str, Vec<Duration>)> = vec![
        ("N+1: `order.lines(&db)` in a loop", "`Client`", Vec::new()),
        ("N+1: `order.lines(&db)` in a loop", "pooled", Vec::new()),
        ("N+1, pipelined: `try_join_all`", "`Client`", Vec::new()),
        (
            "Batched: `Order::load_lines(&db, &orders)`",
            "`Client`",
            Vec::new(),
        ),
        (
            "Batched: `Order::load_lines(&db, &orders)`",
            "pooled",
            Vec::new(),
        ),
    ];
    // A round trip, for scale: a statement the pooled connection has cached.
    let mut round_trip = Vec::new();
    for _ in 0..3 {
        let mut loaded = Vec::new();
        let t = Instant::now();
        for order in &orders {
            loaded.push(order.lines(&client).await?);
        }
        results[0].2.push(t.elapsed());
        let t = Instant::now();
        for order in &orders {
            loaded.push(order.lines(&pooled).await?);
        }
        results[1].2.push(t.elapsed());
        let t = Instant::now();
        loaded.extend(try_join_all(orders.iter().map(|order| order.lines(&client))).await?);
        results[2].2.push(t.elapsed());
        let t = Instant::now();
        loaded.extend(Order::load_lines(&client, &orders).await?);
        results[3].2.push(t.elapsed());
        let t = Instant::now();
        loaded.extend(Order::load_lines(&pooled, &orders).await?);
        results[4].2.push(t.elapsed());
        assert!(loaded.iter().all(|lines| lines.len() == 3));
        assert_eq!(loaded.len(), 4 * orders.len() + orders.len());
        let t = Instant::now();
        Order::find(&pooled, &1).await?;
        round_trip.push(t.elapsed());
    }

    println!(
        "| Loading the lines of {} orders | Executor | Median of 3 | × one round trip |",
        orders.len()
    );
    println!("|---|---|---:|---:|");
    let unit = median(round_trip);
    for (name, executor, times) in results {
        let ms = median(times);
        println!("| {name} | {executor} | {ms:.1} ms | {:.0} |", ms / unit);
    }
    println!("\n(one round trip, a cached `find`: {unit:.2} ms)");

    drop(pooled);
    setup
        .batch_execute(&format!("DROP SCHEMA {schema} CASCADE"))
        .await?;
    Ok(())
}
