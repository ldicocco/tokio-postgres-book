//! `pgq`: a tiny psql.
//!
//!     cargo run -p pgq -- -c "SELECT now(); SELECT 1 AS a, NULL AS b"
//!     cargo run -p pgq -- --describe -c "SELECT * FROM pg_class WHERE relname = \$1"
//!     echo "SELECT generate_series(1, 5)" | cargo run -p pgq -- --stream
//!     cargo run -p pgq                       # interactive: end each statement with ;

use std::io::{BufRead, IsTerminal, Read, Write};
use std::time::Instant;

use clap::Parser;
use futures_util::{StreamExt, stream};
use tokio_postgres::{AsyncMessage, Client, NoTls};

/// A tiny psql: runs SQL through the simple query protocol and prints tables.
#[derive(Debug, Parser)]
#[command(version)]
struct Args {
    /// Connection string; also read from DATABASE_URL (and a .env file).
    #[arg(long, env = "DATABASE_URL", hide_env_values = true)]
    database_url: String,
    /// SQL to run (repeatable). Without it, SQL is read from standard input.
    #[arg(short = 'c', long = "command")]
    commands: Vec<String>,
    /// Describe each statement (parameter and column types) instead of running it.
    #[arg(long)]
    describe: bool,
    /// Print rows as they arrive (tab-separated) instead of as a table.
    #[arg(long)]
    stream: bool,
    /// Print how long each command took.
    #[arg(long)]
    timing: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();
    let args = Args::parse();
    let client = connect(&args.database_url).await?;

    if !args.commands.is_empty() {
        for sql in &args.commands {
            execute(&client, &args, sql).await;
        }
    } else if std::io::stdin().is_terminal() {
        interactive(&client, &args).await?;
    } else {
        let mut sql = String::new();
        std::io::stdin().read_to_string(&mut sql)?;
        execute(&client, &args, &sql).await;
    }
    Ok(())
}

/// Connects, and drives the connection on a task that prints the server's notices to
/// standard error, as psql does ("NOTICE: table … does not exist, skipping").
async fn connect(url: &str) -> anyhow::Result<Client> {
    let (client, mut connection) = tokio_postgres::connect(url, NoTls).await?;
    tokio::spawn(async move {
        let mut messages = stream::poll_fn(move |cx| connection.poll_message(cx));
        while let Some(message) = messages.next().await {
            match message {
                Ok(AsyncMessage::Notice(notice)) => {
                    eprintln!("{}: {}", notice.severity(), notice.message())
                }
                Ok(AsyncMessage::Notification(n)) => {
                    eprintln!("notification on {}: {}", n.channel(), n.payload())
                }
                Ok(_) => {}
                Err(error) => {
                    eprintln!("connection lost: {error}");
                    break;
                }
            }
        }
    });
    Ok(client)
}

async fn execute(client: &Client, args: &Args, sql: &str) {
    let started = Instant::now();
    let result = if args.describe {
        pgq::describe(client, sql)
            .await
            .map(|text| print!("{text}"))
    } else if args.stream {
        let mut stdout = std::io::stdout().lock();
        pgq::stream(client, sql, |line| {
            let _ = writeln!(stdout, "{line}");
        })
        .await
        .map(|rows| eprintln!("({rows} rows)"))
    } else {
        pgq::run(client, sql).await.map(|text| print!("{text}"))
    };
    if let Err(error) = result {
        // A database error says more than its one-line Display: show the useful parts.
        match error.as_db_error() {
            Some(db) => {
                eprintln!("{}: {}", db.severity(), db.message());
                if let Some(detail) = db.detail() {
                    eprintln!("DETAIL: {detail}");
                }
                if let Some(hint) = db.hint() {
                    eprintln!("HINT: {hint}");
                }
                eprintln!("SQLSTATE: {}", db.code().code());
            }
            None => eprintln!("error: {error}"),
        }
    }
    if args.timing {
        eprintln!("Time: {:.3} ms", started.elapsed().as_secs_f64() * 1000.0);
    }
}

/// Reads statements line by line; a line ending in `;` runs what has been typed so far.
async fn interactive(client: &Client, args: &Args) -> anyhow::Result<()> {
    let mut buffer = String::new();
    let stdin = std::io::stdin();
    loop {
        print!("{}", if buffer.is_empty() { "pgq> " } else { "pgq-> " });
        std::io::stdout().flush()?;
        let mut line = String::new();
        if stdin.lock().read_line(&mut line)? == 0 {
            break; // end of input
        }
        buffer.push_str(&line);
        if line.trim_end().ends_with(';') {
            execute(client, args, &buffer).await;
            buffer.clear();
        }
    }
    if !buffer.trim().is_empty() {
        execute(client, args, &buffer).await;
    }
    Ok(())
}
