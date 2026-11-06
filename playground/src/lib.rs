//! Helpers shared by the book's runnable examples.
//!
//! Chapter 1 builds `connect` step by step and prints it in full; later chapters add helpers
//! here and print each one where it is introduced. The regions marked `// region: <name>` are
//! the exact text the chapters print — `tools/book.py check` compares the two.

// region: connect
use tokio_postgres::{Client, Error, NoTls};

/// Connects to the database named by the `DATABASE_URL` environment variable and spawns the
/// connection task, returning the client.
///
/// # Panics
///
/// If `DATABASE_URL` is not set: examples are programs, not libraries, and a clear panic is
/// the most useful thing they can do about a missing setting.
pub async fn connect() -> Result<Client, Error> {
    let url = std::env::var("DATABASE_URL")
        .expect("set DATABASE_URL, e.g. postgres://postgres:secret@localhost:5432/tpbook");
    let (client, connection) = tokio_postgres::connect(&url, NoTls).await?;
    tokio::spawn(async move {
        if let Err(e) = connection.await {
            eprintln!("connection error: {e}");
        }
    });
    Ok(client)
}
// endregion: connect
