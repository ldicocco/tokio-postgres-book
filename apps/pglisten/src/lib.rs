//! pglisten: listen for notifications, and survive losing the connection (Chapters 11, 12).
//!
//! [`run`] holds one connection that `LISTEN`s on the given channels, drives it with
//! `poll_message` so that notifications and notices are not lost, and reports everything as
//! an [`Event`]. When the connection fails it reconnects with exponential backoff; after each
//! (re)connect it can run a catch-up query, because whatever was notified while nobody was
//! listening is gone for good. The order matters: `LISTEN` first, then catch up, so nothing
//! falls into the gap between the two.

use std::time::{Duration, Instant};

use futures_util::{StreamExt, stream};
use tokio::sync::mpsc;
use tokio_postgres::{AsyncMessage, Config, NoTls, SimpleQueryMessage};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// Listening (again). `server_version` is what the server reported when it accepted the
    /// connection (`Connection::parameter`).
    Listening {
        server_version: Option<String>,
    },
    /// Rows of the catch-up query, as text.
    CaughtUp {
        rows: Vec<Vec<Option<String>>>,
    },
    Notification {
        channel: String,
        payload: String,
        process_id: i32,
    },
    Notice {
        severity: String,
        message: String,
    },
    /// The connection failed; the next attempt comes after `retry_in`.
    Lost {
        error: String,
        retry_in: Duration,
    },
}

#[derive(Debug, Clone, Default)]
pub struct Options {
    pub channels: Vec<String>,
    /// Run after every successful `LISTEN`, to fetch what may have been missed.
    pub catch_up: Option<String>,
    /// Ask the server for a round trip this often (`check_connection`), so that a dead
    /// connection is noticed even when no notification is due.
    pub ping: Option<Duration>,
}

/// `name` as an SQL identifier: in double quotes, with any double quote doubled.
///
/// A channel name is an identifier, and identifiers cannot be parameters (`LISTEN $1` is a
/// syntax error), so this is the one place where text from outside goes into SQL. Quoting
/// also keeps its case: unquoted, `LISTEN Orders` would listen on `orders`, while
/// `pg_notify('Orders', …)` notifies `Orders`.
pub fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// Listens until `shutdown`, reporting every event to `on_event`.
pub async fn run(
    config: Config,
    options: Options,
    mut on_event: impl FnMut(Event),
    shutdown: CancellationToken,
) {
    let mut delay = Duration::from_millis(100);
    loop {
        let started = Instant::now();
        let error = match session(&config, &options, &mut on_event, &shutdown).await {
            Ok(()) => return, // shut down
            Err(error) => error,
        };
        if started.elapsed() > Duration::from_secs(60) {
            delay = Duration::from_millis(100); // a new failure, not the same one again
        }
        on_event(Event::Lost {
            error: error.to_string(),
            retry_in: delay,
        });
        tokio::select! {
            () = tokio::time::sleep(delay) => {}
            () = shutdown.cancelled() => return,
        }
        delay = (delay * 2).min(Duration::from_secs(30));
    }
}

/// One connection's life: `Ok` at shutdown, `Err` when the connection failed.
async fn session(
    config: &Config,
    options: &Options,
    on_event: &mut impl FnMut(Event),
    shutdown: &CancellationToken,
) -> anyhow::Result<()> {
    let (client, mut connection) = config.connect(NoTls).await?;
    let server_version = connection.parameter("server_version").map(str::to_owned);

    // The connection is driven here, not merely awaited, so that what the server sends
    // unprompted (notifications, notices) comes out instead of being dropped.
    let (forward, mut messages) = mpsc::channel(1024);
    let driver = tokio::spawn(async move {
        let mut incoming = stream::poll_fn(move |cx| connection.poll_message(cx));
        while let Some(message) = incoming.next().await {
            if forward.send(message?).await.is_err() {
                break;
            }
        }
        Ok::<_, tokio_postgres::Error>(())
    });

    let listen: Vec<String> = options
        .channels
        .iter()
        .map(|c| format!("LISTEN {}", quote_ident(c)))
        .collect();
    client.batch_execute(&listen.join("; ")).await?;
    on_event(Event::Listening { server_version });

    if let Some(sql) = &options.catch_up {
        let rows = client
            .simple_query(sql)
            .await?
            .into_iter()
            .filter_map(|message| match message {
                SimpleQueryMessage::Row(row) => Some(
                    (0..row.len())
                        .map(|i| row.get(i).map(str::to_owned))
                        .collect(),
                ),
                _ => None,
            })
            .collect();
        on_event(Event::CaughtUp { rows });
    }

    let mut ping = tokio::time::interval(options.ping.unwrap_or(Duration::from_secs(3600)));
    ping.tick().await; // the first tick is immediate
    loop {
        tokio::select! {
            () = shutdown.cancelled() => {
                driver.abort();
                return Ok(());
            }
            _ = ping.tick(), if options.ping.is_some() => client.check_connection().await?,
            message = messages.recv() => match message {
                Some(AsyncMessage::Notification(n)) => on_event(Event::Notification {
                    channel: n.channel().to_owned(),
                    payload: n.payload().to_owned(),
                    process_id: n.process_id(),
                }),
                Some(AsyncMessage::Notice(notice)) => on_event(Event::Notice {
                    severity: notice.severity().to_owned(),
                    message: notice.message().to_owned(),
                }),
                Some(_) => {} // non-exhaustive
                None => {
                    // The driver stopped: the connection is gone, and its result says why.
                    return Err(match driver.await {
                        Ok(Err(error)) => error.into(),
                        _ => anyhow::anyhow!("the connection closed"),
                    });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn quote_ident_quotes_and_doubles_quotes() {
        assert_eq!(super::quote_ident("orders"), "\"orders\"");
        assert_eq!(super::quote_ident("Say \"hi\""), "\"Say \"\"hi\"\"\"");
    }
}
