//! `lagproxy`: put a slow network between a client and PostgreSQL.
//!
//!     cargo run -p lagproxy -- --upstream localhost:5432 --delay-ms 5
//!     DATABASE_URL=postgres://postgres:…@127.0.0.1:6543/tpbook cargo run --example …

use std::time::Duration;

use clap::Parser;

/// A TCP proxy that delays every byte by a fixed time in each direction: a round trip
/// through it costs twice the delay.
#[derive(Debug, Parser)]
#[command(version)]
struct Args {
    /// Where to listen (port 0: any free port; the address is printed on start).
    #[arg(long, default_value = "127.0.0.1:6543")]
    listen: String,
    /// The server to forward to, as host:port.
    #[arg(long, default_value = "localhost:5432")]
    upstream: String,
    /// One-way delay in milliseconds.
    #[arg(long, default_value_t = 5)]
    delay_ms: u64,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let proxy = lagproxy::Proxy::start(
        &args.listen,
        &args.upstream,
        Duration::from_millis(args.delay_ms),
    )
    .await?;
    // The first line of output is the address: tools/book.py reads it.
    println!(
        "listening on {} → {} (+{} ms each way)",
        proxy.addr(),
        args.upstream,
        args.delay_ms
    );
    tokio::signal::ctrl_c().await?;
    Ok(())
}
