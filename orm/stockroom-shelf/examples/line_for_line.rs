//! What each ported repository function costs, stockroom's against shelf's: the statements it
//! prepares, the statements it runs, and its round trips (Chapter 14.3).
//!
//!     cargo run -p stockroom-shelf --release --example line_for_line [-- --delay-ms 5]
//!
//! The pool's one connection goes through two proxies, in-process: a counter of the protocol
//! messages the client sends (Parse is a statement prepared, Execute a statement run), then
//! lagproxy, which delays each direction by `--delay-ms`. A call's time divided by one round
//! trip's (a cached `SELECT 1`) is its round trips.
//!
//! Each function runs once to warm the connection's type cache (stockroom's types are looked
//! up on first use), then once after its statement cache is cleared (what it prepares: its
//! statements), then five times warm (the median). The data lives in a database cloned from
//! stockroom's test template, through its harness, and is dropped at the end.

#[path = "../../../stockroom/tests/common/mod.rs"]
mod harness;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use deadpool_postgres::{Object, Pool};
use serde_json::json;
use stockroom::domain::{Address, Cents, Customer, Quantity, Sku};
use stockroom::repo as hand;
use stockroom::repo::orders::{NewLine, NewOrder};
use stockroom_shelf::repo as shelf;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_postgres::IsolationLevel;
use tokio_postgres::config::Host;

/// The messages clients sent through [`count_proxy`] since the last [`Counts::take`].
#[derive(Default)]
struct Counts {
    parse: AtomicUsize,
    execute: AtomicUsize,
}

impl Counts {
    /// (Parse, Execute) since the last call.
    fn take(&self) -> (usize, usize) {
        (
            self.parse.swap(0, Ordering::SeqCst),
            self.execute.swap(0, Ordering::SeqCst),
        )
    }
}

/// Forwards every connection to `upstream`, counting the Parse and Execute messages the client
/// sends. After the startup packet (a length, no type byte), each message is a type byte and a
/// length that counts itself.
async fn count_proxy(upstream: String, counts: Arc<Counts>) -> std::io::Result<String> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    tokio::spawn(async move {
        while let Ok((client, _)) = listener.accept().await {
            let Ok(server) = TcpStream::connect(&upstream).await else {
                continue;
            };
            let counts = Arc::clone(&counts);
            tokio::spawn(async move {
                let (mut client_read, mut client_write) = client.into_split();
                let (mut server_read, mut server_write) = server.into_split();
                tokio::spawn(async move {
                    let _ = tokio::io::copy(&mut server_read, &mut client_write).await;
                });
                let (mut pending, mut started) = (Vec::new(), false);
                let mut chunk = vec![0; 64 * 1024];
                while let Ok(n) = client_read.read(&mut chunk).await {
                    if n == 0 || server_write.write_all(&chunk[..n]).await.is_err() {
                        break;
                    }
                    pending.extend_from_slice(&chunk[..n]);
                    loop {
                        let at = usize::from(started); // the type byte, once started
                        if pending.len() < at + 4 {
                            break;
                        }
                        let length = u32::from_be_bytes(pending[at..at + 4].try_into().unwrap());
                        let total = at + length as usize;
                        if pending.len() < total {
                            break;
                        }
                        let counter = match (started, pending[0]) {
                            (true, b'P') => Some(&counts.parse),
                            (true, b'E') => Some(&counts.execute),
                            _ => None,
                        };
                        if let Some(counter) = counter {
                            counter.fetch_add(1, Ordering::SeqCst);
                        }
                        started = true;
                        pending.drain(..total);
                    }
                }
            });
        }
    });
    Ok(addr.to_string())
}

fn median(mut times: Vec<Duration>) -> Duration {
    times.sort();
    times[times.len() / 2]
}

/// What one function cost on one side.
struct Cost {
    function: &'static str,
    side: &'static str,
    /// Parse messages of a call on an empty statement cache.
    cold_prepares: usize,
    /// Parse and Execute messages of a warm call.
    prepares: usize,
    runs: usize,
    time: Duration,
}

struct Bench {
    pool: Pool,
    counts: Arc<Counts>,
    costs: Vec<Cost>,
}

impl Bench {
    /// Runs `call` to warm up, once on an empty statement cache, then five times, and records it.
    async fn measure(
        &mut self,
        function: &'static str,
        side: &'static str,
        mut call: impl AsyncFnMut(&mut Object, usize),
    ) {
        let mut db = self.pool.get().await.expect("a connection");
        call(&mut db, 0).await;
        db.statement_cache.clear();
        self.counts.take();
        call(&mut db, 1).await;
        let (cold_prepares, _) = self.counts.take();
        let (mut times, mut prepares, mut runs) = (Vec::new(), 0, 0);
        for i in 2..7 {
            let start = Instant::now();
            call(&mut db, i).await;
            times.push(start.elapsed());
            (prepares, runs) = self.counts.take();
        }
        self.costs.push(Cost {
            function,
            side,
            cold_prepares,
            prepares,
            runs,
            time: median(times),
        });
    }
}

fn address() -> Address {
    Address {
        street: "Main 1".into(),
        city: "Berlin".into(),
        postcode: "10115".into(),
        country: "DE".into(),
    }
}

fn order(skus: &[&str]) -> NewOrder {
    NewOrder {
        store: "mitte".into(),
        customer: Customer {
            name: "Ada".into(),
            email: "ada@example.com".into(),
        },
        lines: skus
            .iter()
            .map(|sku| NewLine {
                sku: Sku::parse(sku).unwrap(),
                quantity: Quantity::new(1).unwrap(),
            })
            .collect(),
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let delay_ms: u64 = match std::env::args().skip_while(|a| a != "--delay-ms").nth(1) {
        Some(ms) => ms.parse()?,
        None => 5,
    };
    let db = harness::TestDb::new().await;
    db.client()
        .await
        .batch_execute(
            "INSERT INTO stores (code, name, address)
             SELECT code, initcap(code), ROW('Main 1', 'Berlin', '10115', 'DE')::address
             FROM unnest(ARRAY['mitte', 'spandau', 'wedding']) AS code;
             INSERT INTO products (sku, name, price_cents, tags)
             SELECT 'ITEM-' || lpad(i::text, 3, '0'), 'Item ' || i, 100 * i,
                    CASE WHEN i % 2 = 0 THEN ARRAY['sale'] ELSE ARRAY[]::text[] END
             FROM generate_series(1, 50) i;
             INSERT INTO stock (store_id, product_id, on_hand)
             SELECT s.id, p.id, 1000000 FROM stores s CROSS JOIN products p;",
        )
        .await?;

    let Host::Tcp(host) = &db.config.get_hosts()[0] else {
        panic!("ADMIN_DATABASE_URL must name a TCP host");
    };
    let upstream = format!("{host}:{}", db.config.get_ports()[0]);
    let lag =
        lagproxy::Proxy::start("127.0.0.1:0", upstream, Duration::from_millis(delay_ms)).await?;
    let counts = Arc::new(Counts::default());
    let counted = count_proxy(lag.addr().to_string(), Arc::clone(&counts)).await?;
    let (host, port) = counted.rsplit_once(':').unwrap();
    // A new `Config`: `host` and `port` add to a config's lists (several hosts are allowed), so
    // setting them on a copy of the test's would leave its own host first.
    let mut config = tokio_postgres::Config::new();
    config.host(host).port(port.parse()?);
    config.dbname(db.config.get_dbname().unwrap());
    config.user(db.config.get_user().unwrap());
    config.password(db.config.get_password().unwrap());
    config.options(db.config.get_options().unwrap_or_default());
    let pool = stockroom::db::pool::create(config, harness::tls(), 1)?;

    // One round trip: a cached `SELECT 1`, five times.
    let probe = pool.get().await?;
    let one = probe.prepare_cached("SELECT 1").await?;
    let mut times = Vec::new();
    for _ in 0..5 {
        let start = Instant::now();
        probe.query_one(&one, &[]).await?;
        times.push(start.elapsed());
    }
    let round_trip = median(times);
    drop(probe);

    let mut bench = Bench {
        pool,
        counts,
        costs: Vec::new(),
    };
    let item = Sku::parse("ITEM-001")?;
    let cursor = Sku::parse("ITEM-010")?;

    bench
        .measure("stores::list", "stockroom", async |db, _| {
            hand::stores::list(&*db).await.unwrap();
        })
        .await;
    bench
        .measure("stores::list", "shelf", async |db, _| {
            shelf::stores::list(&*db).await.unwrap();
        })
        .await;

    bench
        .measure("stores::create", "stockroom", async |db, i| {
            let new = hand::stores::NewStore {
                code: format!("hand-{i}"),
                name: "Store".into(),
                address: address(),
            };
            hand::stores::create(&*db, &new).await.unwrap();
        })
        .await;
    bench
        .measure("stores::create", "shelf", async |db, i| {
            let new = shelf::stores::NewStore {
                code: format!("shelf-{i}"),
                name: "Store".into(),
                address: address(),
            };
            shelf::stores::create(&*db, &new).await.unwrap();
        })
        .await;

    bench
        .measure("products::page, first", "stockroom", async |db, _| {
            hand::products::page(&*db, None, None, 21).await.unwrap();
        })
        .await;
    bench
        .measure("products::page, first", "shelf", async |db, _| {
            shelf::products::page(&*db, None, None, 21).await.unwrap();
        })
        .await;
    bench
        .measure(
            "products::page, cursor and tag",
            "stockroom",
            async |db, _| {
                let page = hand::products::page(&*db, Some(&cursor), Some("sale"), 21);
                page.await.unwrap();
            },
        )
        .await;
    bench
        .measure("products::page, cursor and tag", "shelf", async |db, _| {
            let page = shelf::products::page(&*db, Some(&cursor), Some("sale"), 21);
            page.await.unwrap();
        })
        .await;

    bench
        .measure("products::by_sku", "stockroom", async |db, _| {
            hand::products::by_sku(&*db, &item).await.unwrap();
        })
        .await;
    bench
        .measure("products::by_sku", "shelf", async |db, _| {
            shelf::products::by_sku(&*db, &item).await.unwrap();
        })
        .await;

    bench
        .measure("products::create", "stockroom", async |db, i| {
            let new = hand::products::NewProduct {
                sku: Sku::parse(&format!("HAND-{i}")).unwrap(),
                name: "New".into(),
                price_cents: Cents(100),
                tags: vec!["new".into()],
                attributes: json!({}),
            };
            hand::products::create(&*db, &new).await.unwrap();
        })
        .await;
    bench
        .measure("products::create", "shelf", async |db, i| {
            let new = shelf::products::NewProduct {
                sku: Sku::parse(&format!("SHELF-{i}")).unwrap(),
                name: "New".into(),
                price_cents: Cents(100),
                tags: vec!["new".into()],
                attributes: json!({}),
            };
            shelf::products::create(&*db, &new).await.unwrap();
        })
        .await;

    // Each side patches a product of its own, so that the versions stay apart.
    let (hand_sku, shelf_sku) = (Sku::parse("ITEM-002")?, Sku::parse("ITEM-003")?);
    let mut version = 1;
    bench
        .measure("products::update, If-Match", "stockroom", async |db, i| {
            let patch = hand::products::ProductPatch {
                price_cents: Some(Cents(1000 + i as i64)),
                ..Default::default()
            };
            let updated = hand::products::update(&*db, &hand_sku, Some(version), &patch);
            version = updated.await.unwrap().version;
        })
        .await;
    let mut version = 1;
    bench
        .measure("products::update, If-Match", "shelf", async |db, i| {
            let patch = shelf::products::ProductPatch {
                price_cents: Some(Cents(1000 + i as i64)),
                ..Default::default()
            };
            let updated = shelf::products::update(&*db, &shelf_sku, Some(version), patch);
            version = updated.await.unwrap().version;
        })
        .await;
    bench
        .measure(
            "products::update, no If-Match",
            "stockroom",
            async |db, i| {
                let patch = hand::products::ProductPatch {
                    name: Some(format!("Item {i}")),
                    ..Default::default()
                };
                let updated = hand::products::update(&*db, &hand_sku, None, &patch);
                updated.await.unwrap();
            },
        )
        .await;
    bench
        .measure("products::update, no If-Match", "shelf", async |db, i| {
            let patch = shelf::products::ProductPatch {
                name: Some(format!("Item {i}")),
                ..Default::default()
            };
            let updated = shelf::products::update(&*db, &shelf_sku, None, patch);
            updated.await.unwrap();
        })
        .await;

    // In a transaction, as the route runs it: `BEGIN` and `COMMIT` are in the counts.
    let skus = ["ITEM-004", "ITEM-005", "ITEM-006"];
    bench
        .measure("orders::place, 3 lines", "stockroom", async |db, _| {
            let order = order(&skus);
            stockroom::db::in_transaction(db, IsolationLevel::ReadCommitted, async move |tx| {
                hand::orders::place(tx, &order).await
            })
            .await
            .unwrap();
        })
        .await;
    bench
        .measure("orders::place, 3 lines", "shelf", async |db, _| {
            let order = order(&skus);
            stockroom::db::in_transaction(db, IsolationLevel::ReadCommitted, async move |tx| {
                shelf::orders::place(tx, &order).await
            })
            .await
            .unwrap();
        })
        .await;

    let placed = {
        let mut db = bench.pool.get().await?;
        let order = order(&skus);
        stockroom::db::in_transaction(&mut db, IsolationLevel::ReadCommitted, async move |tx| {
            hand::orders::place(tx, &order).await
        })
        .await
        .map_err(|e| e.to_string())?
    };
    bench
        .measure("orders::get, 3 lines", "stockroom", async |db, _| {
            hand::orders::get(&*db, placed.id).await.unwrap();
        })
        .await;
    bench
        .measure("orders::get, 3 lines", "shelf", async |db, _| {
            shelf::orders::get(&*db, placed.id).await.unwrap();
        })
        .await;

    println!(
        "One round trip (a cached `SELECT 1`, {delay_ms} ms each way): {:.1} ms\n",
        round_trip.as_secs_f64() * 1e3
    );
    println!("| Function | Side | Prepared, cold | Prepared, warm | Run | Round trips | ms |");
    println!("|---|---|---:|---:|---:|---:|---:|");
    for cost in &bench.costs {
        let trips = cost.time.as_secs_f64() / round_trip.as_secs_f64();
        println!(
            "| {} | {} | {} | {} | {} | {:.1} | {:.1} |",
            cost.function,
            cost.side,
            cost.cold_prepares,
            cost.prepares,
            cost.runs,
            trips,
            cost.time.as_secs_f64() * 1e3
        );
    }
    Ok(())
}
