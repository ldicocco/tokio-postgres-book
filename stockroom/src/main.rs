//! `stockroom`: migrate, seed, export, serve.

use clap::{Parser, Subcommand};
use stockroom::cli::export::ExportArgs;
use stockroom::cli::seed::SeedArgs;
use stockroom::config::{DbArgs, ServeArgs};
use stockroom::{app, cli, db};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::format::FmtSpan;

/// Click-and-collect for a small retail chain: the capstone of the tokio-postgres book.
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
    /// Apply pending database migrations.
    Migrate,
    /// Fill an empty database with sample data.
    Seed(SeedArgs),
    /// Write products or stock movements to a file.
    Export(ExportArgs),
    /// Run the HTTP service.
    Serve(ServeArgs),
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("stockroom=info,tower_http=info")),
        )
        // With RUST_LOG=stockroom=debug, every repository call logs its duration when it ends.
        .with_span_events(FmtSpan::CLOSE)
        .init();

    let cli = Cli::parse();
    match cli.command {
        Command::Migrate => {
            let mut client = connect(&cli.db).await?;
            let applied = db::migrate::run(&mut client).await?;
            if applied.is_empty() {
                println!("the schema is up to date");
            } else {
                println!("applied migrations {applied:?}");
            }
        }
        Command::Seed(args) => {
            let mut client = connect(&cli.db).await?;
            let report = cli::seed::run(&mut client, &args).await?;
            println!(
                "seeded {} stores, {} products, {} stock rows, {} movements",
                report.stores, report.products, report.stock_rows, report.movements
            );
        }
        Command::Export(args) => {
            let mut client = connect(&cli.db).await?;
            let rows = cli::export::run(&mut client, &args).await?;
            eprintln!("exported {rows} rows");
        }
        Command::Serve(args) => app::serve(cli.db, args).await?,
    }
    Ok(())
}

async fn connect(args: &DbArgs) -> anyhow::Result<tokio_postgres::Client> {
    let tls = db::tls::make_tls(args)?;
    Ok(db::connect(&args.pg_config()?, &tls).await?)
}
