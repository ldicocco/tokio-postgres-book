//! `stockroom-shelf`: stockroom's HTTP API, with stores, products and orders on shelf.
//!
//! Migrating, seeding and exporting are stockroom's own commands (`cargo run -p stockroom --
//! …`); this binary serves, and checks its models against a database.

use clap::{Parser, Subcommand};
use stockroom::config::{DbArgs, ServeArgs};
use stockroom::db;
use stockroom_shelf::{app, repo};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::format::FmtSpan;

/// stockroom on shelf: the capstone of the shelf book.
///
/// Every flag can also come from the environment variable shown in its help, and a `.env`
/// file in the current directory or a parent is read first.
#[derive(Debug, Parser)]
#[command(version)]
struct Cli {
    #[command(flatten)]
    db: DbArgs,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Check every model against the database, and report each mismatch.
    Verify,
    /// Run the HTTP service (after verifying the models).
    Serve(ServeArgs),
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| {
            EnvFilter::new("stockroom_shelf=info,stockroom=info,tower_http=info")
        }))
        .with_span_events(FmtSpan::CLOSE)
        .init();

    let cli = Cli::parse();
    match cli.command {
        Command::Verify => {
            let tls = db::tls::make_tls(&cli.db)?;
            let client = db::connect(&cli.db.pg_config()?, &tls).await?;
            match repo::verify_models(&client).await {
                Ok(()) => println!("every model matches its table"),
                Err(errors) => {
                    for error in &errors {
                        eprintln!("{error}");
                    }
                    std::process::exit(1);
                }
            }
        }
        Command::Serve(args) => app::serve(cli.db, args).await?,
    }
    Ok(())
}
