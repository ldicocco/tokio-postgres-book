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

// region: slice_iter
use tokio_postgres::types::ToSql;

/// Adapts a parameter slice, the kind `query` takes, to the iterator that the `*_raw`
/// methods take: `client.query_raw(&statement, slice_iter(&[&a, &b]))`.
pub fn slice_iter<'a>(
    params: &'a [&'a (dyn ToSql + Sync)],
) -> impl ExactSizeIterator<Item = &'a dyn ToSql> + Send + Sync + 'a {
    params.iter().map(|p| *p as _)
}
// endregion: slice_iter

// region: connect_with_messages
use tokio::sync::mpsc;
use tokio_postgres::AsyncMessage;

/// Like `connect`, but drives the connection with `poll_message`, and forwards what the
/// server sends unprompted (notifications and notices) to the returned receiver instead of
/// dropping it. The channel is unbounded, which suits examples: nothing is lost, and memory is
/// the limit if nobody reads.
pub async fn connect_with_messages()
-> Result<(Client, mpsc::UnboundedReceiver<AsyncMessage>), Error> {
    let url = std::env::var("DATABASE_URL")
        .expect("set DATABASE_URL, e.g. postgres://postgres:secret@localhost:5432/tpbook");
    let (client, mut connection) = tokio_postgres::connect(&url, NoTls).await?;
    let (forward, messages) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        loop {
            match std::future::poll_fn(|cx| connection.poll_message(cx)).await {
                // A closed receiver only means nobody is interested any more; the connection
                // must still be driven for the client's queries.
                Some(Ok(message)) => {
                    let _ = forward.send(message);
                }
                Some(Err(e)) => {
                    eprintln!("connection error: {e}");
                    break;
                }
                None => break,
            }
        }
    });
    Ok((client, messages))
}
// endregion: connect_with_messages

// region: pool
use deadpool_postgres::{Manager, ManagerConfig, Pool, RecyclingMethod, Runtime};

/// A deadpool pool of at most `max_size` connections to `DATABASE_URL` (Chapter 13). Part
/// III's examples mostly use a pool of one, so that every request sees the same session, and
/// with it the example's temporary tables.
pub fn pool(max_size: usize) -> Pool {
    let config: tokio_postgres::Config = std::env::var("DATABASE_URL")
        .expect("set DATABASE_URL, e.g. postgres://postgres:secret@localhost:5432/tpbook")
        .parse()
        .expect("DATABASE_URL is a valid connection string");
    let manager = Manager::from_config(
        config,
        NoTls,
        ManagerConfig {
            recycling_method: RecyclingMethod::Fast,
        },
    );
    Pool::builder(manager)
        .max_size(max_size)
        .runtime(Runtime::Tokio1)
        .wait_timeout(Some(std::time::Duration::from_secs(5)))
        .build()
        .expect("a valid pool configuration")
}
// endregion: pool

// region: call
use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

/// Sends one request through `app`, in memory, without binding a port, and returns the
/// response's status, headers and body: parsed as JSON, as a JSON string if it is plain text,
/// or `Null` when it is empty.
pub async fn call(
    app: &axum::Router,
    request: Request<Body>,
) -> (StatusCode, HeaderMap, serde_json::Value) {
    let response = app
        .clone()
        .oneshot(request)
        .await
        .expect("a Router's error type is Infallible");
    let (parts, body) = response.into_parts();
    let bytes = body
        .collect()
        .await
        .expect("an in-memory body cannot fail")
        .to_bytes();
    let json = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|_| {
            serde_json::Value::String(String::from_utf8_lossy(&bytes).into_owned())
        })
    };
    (parts.status, parts.headers, json)
}
// endregion: call
