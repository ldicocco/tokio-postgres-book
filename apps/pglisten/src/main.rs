//! `pglisten`: print notifications as they arrive.
//!
//!     cargo run -p pglisten -- order_events outbox
//!     cargo run -p pglisten -- order_events --catch-up "SELECT id, status FROM orders ORDER BY updated_at DESC LIMIT 5"
//!
//! and, in psql: `NOTIFY order_events, 'hello';` or `SELECT pg_notify('order_events', 'hello');`

use std::time::Duration;

use clap::Parser;
use pglisten::{Event, Options};
use tokio_postgres::Config;
use tokio_util::sync::CancellationToken;

/// Listen on PostgreSQL notification channels and print what arrives. Reconnects when the
/// connection is lost.
#[derive(Debug, Parser)]
#[command(version)]
struct Args {
    /// Connection string; also read from DATABASE_URL (and a .env file).
    #[arg(long, env = "DATABASE_URL", hide_env_values = true)]
    database_url: String,
    /// The channels to listen on (case-sensitive).
    #[arg(required = true)]
    channels: Vec<String>,
    /// A query to run after every (re)connect, to catch up on what was missed.
    #[arg(long)]
    catch_up: Option<String>,
    /// Check the connection with a round trip this often (seconds); 0 = never.
    #[arg(long, default_value_t = 30)]
    ping_secs: u64,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();
    let args = Args::parse();
    let mut config: Config = args.database_url.parse()?;
    if config.get_application_name().is_none() {
        config.application_name("pglisten");
    }
    let options = Options {
        channels: args.channels.clone(),
        catch_up: args.catch_up,
        ping: (args.ping_secs > 0).then(|| Duration::from_secs(args.ping_secs)),
    };

    let shutdown = CancellationToken::new();
    let stop = shutdown.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        stop.cancel();
    });

    let channels = args.channels.join(", ");
    pglisten::run(
        config,
        options,
        |event| {
            let now = jiff::Zoned::now().strftime("%H:%M:%S%.3f");
            match event {
                Event::Listening { server_version } => println!(
                    "{now}  listening on {channels} (PostgreSQL {})",
                    server_version.as_deref().unwrap_or("?")
                ),
                Event::CaughtUp { rows } => {
                    println!("{now}  catch-up: {} rows", rows.len());
                    for row in rows {
                        let cells: Vec<&str> =
                            row.iter().map(|c| c.as_deref().unwrap_or("")).collect();
                        println!("               {}", cells.join(" | "));
                    }
                }
                Event::Notification {
                    channel,
                    payload,
                    process_id,
                } => {
                    println!("{now}  {channel} (from pid {process_id}): {payload}")
                }
                Event::Notice { severity, message } => println!("{now}  {severity}: {message}"),
                Event::Lost { error, retry_in } => {
                    eprintln!("{now}  connection lost: {error}; retrying in {retry_in:?}")
                }
            }
        },
        shutdown,
    )
    .await;
    Ok(())
}
