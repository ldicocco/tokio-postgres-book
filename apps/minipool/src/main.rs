//! `minipool`: how many queries per second, with one shared client, minipool and deadpool?
//!
//!     cargo run -p minipool --release -- --tasks 64 --pool-size 8
//!     cargo run -p minipool --release -- --tasks 64 --pool-size 8 --delay-ms 2

use std::sync::Arc;
use std::time::{Duration, Instant};

use clap::Parser;
use futures_util::future::try_join_all;
use tokio_postgres::types::Type;
use tokio_postgres::{Client, Config, NoTls};

/// Run the same work through a single shared client, minipool and deadpool, and compare.
#[derive(Debug, Parser)]
#[command(version)]
struct Args {
    /// Connection string; also read from DATABASE_URL (and a .env file).
    #[arg(long, env = "DATABASE_URL", hide_env_values = true)]
    database_url: String,
    /// Concurrent tasks.
    #[arg(long, default_value_t = 64)]
    tasks: usize,
    /// Units of work per task.
    #[arg(long, default_value_t = 50)]
    iterations: usize,
    /// Connections per pool.
    #[arg(long, default_value_t = 8)]
    pool_size: usize,
    /// Add this much one-way network delay (through an in-process lagproxy).
    #[arg(long, default_value_t = 0)]
    delay_ms: u64,
}

/// One unit of work: one query, one round trip (`query_typed` needs no prepare).
async fn query(client: &Client, i: i64) -> Result<(), tokio_postgres::Error> {
    let row = client
        .query_typed_one("SELECT $1::int8 + 1", &[(&i, Type::INT8)])
        .await?;
    assert_eq!(row.get::<_, i64>(0), i + 1);
    Ok(())
}

/// One unit of work in a transaction: BEGIN, a query, COMMIT. Needs `&mut Client`.
async fn transaction(client: &mut Client, i: i64) -> Result<(), tokio_postgres::Error> {
    let tx = client.transaction().await?;
    tx.query_typed_one("SELECT $1::int8 + 1", &[(&i, Type::INT8)])
        .await?;
    tx.commit().await
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();
    let args = Args::parse();
    let mut config: Config = args.database_url.parse()?;
    let _proxy = if args.delay_ms > 0 {
        let proxy = lagproxy::Proxy::start(
            "127.0.0.1:0",
            upstream_of(&config)?,
            Duration::from_millis(args.delay_ms),
        )
        .await?;
        config = through(&config, proxy.addr().port());
        Some(proxy)
    } else {
        None
    };
    let ops = args.tasks * args.iterations;
    println!(
        "{} tasks × {} iterations = {ops} operations, pools of {}, {} one-way delay\n",
        args.tasks,
        args.iterations,
        args.pool_size,
        if args.delay_ms > 0 {
            format!("{} ms", args.delay_ms)
        } else {
            "no added".into()
        }
    );
    println!("| Setup | Work | Time | Operations per second |");
    println!("|---|---|---:|---:|");
    let report = |setup: &str, work: &str, elapsed: Duration| {
        println!(
            "| {setup} | {work} | {:.3} s | {:.0} |",
            elapsed.as_secs_f64(),
            ops as f64 / elapsed.as_secs_f64()
        );
    };

    // One client, shared by every task: queries pipeline on its one connection. It cannot
    // run the transaction workload at all: `transaction()` needs `&mut Client`.
    let (client, connection) = config.connect(NoTls).await?;
    tokio::spawn(connection);
    let shared = Arc::new(client);
    let started = Instant::now();
    try_join_all((0..args.tasks).map(|t| {
        let shared = Arc::clone(&shared);
        tokio::spawn(async move {
            for i in 0..args.iterations {
                query(&shared, (t * args.iterations + i) as i64).await?;
            }
            Ok::<_, tokio_postgres::Error>(())
        })
    }))
    .await?
    .into_iter()
    .collect::<Result<(), _>>()?;
    report("one shared client", "query", started.elapsed());

    let mini = minipool::Pool::new(
        config.clone(),
        minipool::Options {
            max_size: args.pool_size,
            ..Default::default()
        },
    );
    let deadpool =
        deadpool_postgres::Pool::builder(deadpool_postgres::Manager::new(config.clone(), NoTls))
            .max_size(args.pool_size)
            .build()?;

    for work in ["query", "transaction"] {
        let started = Instant::now();
        try_join_all((0..args.tasks).map(|t| {
            let pool = mini.clone();
            tokio::spawn(async move {
                for i in 0..args.iterations {
                    let mut conn = pool.get().await?;
                    let n = (t * args.iterations + i) as i64;
                    if work == "query" {
                        query(&conn, n).await?
                    } else {
                        transaction(&mut conn, n).await?
                    }
                }
                Ok::<_, anyhow::Error>(())
            })
        }))
        .await?
        .into_iter()
        .collect::<Result<(), _>>()?;
        report(
            &format!("minipool, {} connections", args.pool_size),
            work,
            started.elapsed(),
        );

        let started = Instant::now();
        try_join_all((0..args.tasks).map(|t| {
            let pool = deadpool.clone();
            tokio::spawn(async move {
                for i in 0..args.iterations {
                    let mut conn = pool.get().await?;
                    let n = (t * args.iterations + i) as i64;
                    if work == "query" {
                        query(&conn, n).await?
                    } else {
                        transaction(&mut conn, n).await?
                    }
                }
                Ok::<_, anyhow::Error>(())
            })
        }))
        .await?
        .into_iter()
        .collect::<Result<(), _>>()?;
        report(
            &format!("deadpool, {} connections", args.pool_size),
            work,
            started.elapsed(),
        );
    }
    Ok(())
}

/// `host:port` of the first host in the configuration.
fn upstream_of(config: &Config) -> anyhow::Result<String> {
    let host = match config.get_hosts().first() {
        Some(tokio_postgres::config::Host::Tcp(host)) => host.clone(),
        _ => anyhow::bail!("--delay-ms needs a TCP host in the connection string"),
    };
    Ok(format!(
        "{host}:{}",
        config.get_ports().first().copied().unwrap_or(5432)
    ))
}

/// The same settings, pointed at 127.0.0.1:`port`. A new `Config` rather than a modified
/// one: `Config::host` and `Config::port` *add* to the list of hosts to try.
fn through(config: &Config, port: u16) -> Config {
    let mut redirected = Config::new();
    redirected.host("127.0.0.1").port(port);
    if let Some(user) = config.get_user() {
        redirected.user(user);
    }
    if let Some(password) = config.get_password() {
        redirected.password(password);
    }
    if let Some(dbname) = config.get_dbname() {
        redirected.dbname(dbname);
    }
    redirected
}
