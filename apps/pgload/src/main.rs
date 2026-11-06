//! `pgload`: time five ways of loading the same rows.
//!
//!     cargo run -p pgload --release -- --rows 20000
//!     cargo run -p pgload --release -- --rows 5000 --delay-ms 1     # a 2 ms round trip

use std::time::{Duration, Instant};

use clap::Parser;
use pgload::Strategy;
use tokio::net::TcpStream;
use tokio_postgres::{Config, NoTls};

/// Load the same generated rows with each strategy and print a timing table.
#[derive(Debug, Parser)]
#[command(version)]
struct Args {
    /// Connection string; also read from DATABASE_URL (and a .env file).
    #[arg(long, env = "DATABASE_URL", hide_env_values = true)]
    database_url: String,
    /// How many rows to load.
    #[arg(long, default_value_t = 10_000)]
    rows: usize,
    /// Which strategies to run (default: all of them).
    #[arg(long, value_enum, value_delimiter = ',')]
    strategies: Vec<Strategy>,
    /// Add this much one-way network delay (through an in-process lagproxy).
    #[arg(long, default_value_t = 0)]
    delay_ms: u64,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();
    let args = Args::parse();
    let config: Config = args.database_url.parse()?;

    // With a delay, connect through a proxy that adds it. `connect_raw` takes a stream we
    // opened ourselves and only uses the configuration's user, password, database, options
    // and application name. (Changing the configuration would not do: `Config::host` and
    // `Config::port` *add* a host to the list instead of replacing it.)
    let (mut client, _proxy) = if args.delay_ms > 0 {
        let upstream = upstream_of(&config)?;
        let proxy = lagproxy::Proxy::start(
            "127.0.0.1:0",
            upstream,
            Duration::from_millis(args.delay_ms),
        )
        .await?;
        let stream = TcpStream::connect(proxy.addr()).await?;
        // `connect` would set this itself; with `connect_raw` the stream is used as given,
        // and Nagle's algorithm would hold small messages back waiting for ACKs.
        stream.set_nodelay(true)?;
        let (client, connection) = config.connect_raw(stream, NoTls).await?;
        tokio::spawn(connection);
        (client, Some(proxy))
    } else {
        let (client, connection) = config.connect(NoTls).await?;
        tokio::spawn(connection);
        (client, None)
    };

    let rows = pgload::generate(args.rows, 42);
    let strategies = if args.strategies.is_empty() {
        Strategy::ALL.to_vec()
    } else {
        args.strategies
    };
    println!(
        "{} rows, {} one-way delay\n",
        args.rows,
        if args.delay_ms > 0 {
            format!("{} ms", args.delay_ms)
        } else {
            "no added".into()
        }
    );

    let mut results = Vec::new();
    for strategy in strategies {
        let started = Instant::now();
        let loaded = pgload::load(&mut client, strategy, &rows).await?;
        let elapsed = started.elapsed();
        anyhow::ensure!(
            loaded == args.rows as u64,
            "{}: loaded {loaded} rows",
            strategy.name()
        );
        results.push((strategy, elapsed));
    }

    let fastest = results.iter().map(|(_, t)| *t).min().unwrap_or_default();
    println!("| Strategy | Time | Rows per second | Relative |");
    println!("|---|---:|---:|---:|");
    for (strategy, elapsed) in &results {
        let seconds = elapsed.as_secs_f64();
        println!(
            "| {} | {:.3} s | {:.0} | {:.1}× |",
            strategy.name(),
            seconds,
            args.rows as f64 / seconds,
            seconds / fastest.as_secs_f64()
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
    let port = config.get_ports().first().copied().unwrap_or(5432);
    Ok(format!("{host}:{port}"))
}
