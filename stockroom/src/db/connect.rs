//! A dedicated connection, outside the pool, whose server notices end up in the log.

use futures_util::{StreamExt, stream};
use tokio_postgres::{AsyncMessage, Client, Config};

use super::MakeTls;

/// Connects, and drives the connection on a task of its own.
///
/// Awaiting a `Connection` (what the pool does) runs it but drops the notices the server
/// sends, such as "relation already exists, skipping". Polling it with `poll_message`
/// instead hands each one over (Chapter 11). Used by `migrate`, `seed` and `export`.
pub async fn connect(config: &Config, tls: &MakeTls) -> Result<Client, tokio_postgres::Error> {
    let (client, mut connection) = config.connect(tls.clone()).await?;
    tokio::spawn(async move {
        // `poll_message` must not be called again after it returns `None` or an error;
        // the stream ends on `None`, and the loop ends on an error.
        let mut messages = stream::poll_fn(move |cx| connection.poll_message(cx));
        while let Some(message) = messages.next().await {
            match message {
                Ok(AsyncMessage::Notice(notice)) => {
                    tracing::info!(
                        severity = notice.severity(),
                        code = notice.code().code(),
                        "{}",
                        notice.message()
                    )
                }
                Ok(AsyncMessage::Notification(n)) => {
                    tracing::debug!(
                        channel = n.channel(),
                        "notification on a connection that does not listen"
                    )
                }
                Ok(_) => {} // `AsyncMessage` is non-exhaustive
                Err(error) => {
                    tracing::error!(%error, "database connection failed");
                    break;
                }
            }
        }
    });
    Ok(client)
}
