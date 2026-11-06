//! The `LISTEN` connection: driven by hand, reconnected when it fails (Chapter 11).

use std::time::{Duration, Instant};

use anyhow::anyhow;
use futures_util::{StreamExt, stream};
use tokio::sync::mpsc;
use tokio_postgres::tls::TlsStream;
use tokio_postgres::{AsyncMessage, Config, Connection, Notification, Socket};
use tokio_util::sync::CancellationToken;

use super::{Hub, HubEvent, OrderEvent, ServerInfo};
use crate::db::MakeTls;

/// Listens until `shutdown`, reconnecting with exponential backoff whenever the connection
/// is lost. This connection is not from the pool, and must not be: `LISTEN` belongs to a
/// session, and a pooled connection is someone else's session a moment later.
pub async fn run(config: Config, tls: MakeTls, hub: Hub, shutdown: CancellationToken) {
    let mut delay = Duration::from_millis(100);
    loop {
        let started = Instant::now();
        match listen(&config, &tls, &hub, &shutdown).await {
            Ok(()) => return, // shut down
            Err(error) => {
                tracing::warn!(%error, "the LISTEN connection failed; reconnecting in {delay:?}")
            }
        }
        if started.elapsed() > Duration::from_secs(60) {
            // It had been fine for a while: this is a new failure, not the same one again.
            delay = Duration::from_millis(100);
        }
        tokio::select! {
            () = tokio::time::sleep(delay) => {}
            () = shutdown.cancelled() => return,
        }
        delay = (delay * 2).min(Duration::from_secs(10));
    }
}

async fn listen(
    config: &Config,
    tls: &MakeTls,
    hub: &Hub,
    shutdown: &CancellationToken,
) -> anyhow::Result<()> {
    let (client, mut connection) = config.connect(tls.clone()).await?;
    hub.set_server_info(server_info(&connection));

    // A `Connection` is a future that does all the IO; awaiting it would silently discard
    // notifications. `poll_message` drives it just the same *and* yields what the server
    // sends unprompted. The driver task forwards those through a bounded channel: if this
    // task falls behind, the driver stops reading, and TCP pushes back to the server.
    let (forward, mut messages) = mpsc::channel(1024);
    let driver = tokio::spawn(async move {
        let mut incoming = stream::poll_fn(move |cx| connection.poll_message(cx));
        while let Some(message) = incoming.next().await {
            if forward.send(message?).await.is_err() {
                break; // the listener is gone
            }
        }
        Ok::<_, tokio_postgres::Error>(())
    });

    client
        .batch_execute("LISTEN order_events; LISTEN outbox")
        .await?;
    tracing::info!("listening for order events");
    // Anything sent while nobody was listening is lost for good. Tell subscribers to
    // re-read, and the worker to look for jobs it was not woken for.
    hub.publish(HubEvent::Resync);
    hub.wake_outbox();

    loop {
        tokio::select! {
            () = shutdown.cancelled() => {
                driver.abort();
                return Ok(());
            }
            message = messages.recv() => match message {
                Some(AsyncMessage::Notification(notification)) => dispatch(hub, &notification),
                Some(AsyncMessage::Notice(notice)) => tracing::info!("server notice: {}", notice.message()),
                Some(_) => {} // `AsyncMessage` is non-exhaustive
                None => {
                    // The driver stopped, so the connection is gone; its result says why.
                    return Err(match driver.await {
                        Ok(Err(error)) => error.into(),
                        _ => anyhow!("the connection closed"),
                    });
                }
            }
        }
    }
}

fn dispatch(hub: &Hub, notification: &Notification) {
    match notification.channel() {
        "order_events" => match serde_json::from_str::<OrderEvent>(notification.payload()) {
            Ok(event) => hub.publish(HubEvent::Order(event)),
            Err(error) => {
                tracing::warn!(%error, payload = notification.payload(), "unreadable order event")
            }
        },
        "outbox" => hub.wake_outbox(),
        other => tracing::debug!(channel = other, "notification on an unexpected channel"),
    }
}

fn server_info<T: TlsStream + Unpin>(connection: &Connection<Socket, T>) -> ServerInfo {
    let get = |name: &str| connection.parameter(name).map(str::to_owned);
    ServerInfo {
        server_version: get("server_version"),
        server_encoding: get("server_encoding"),
        time_zone: get("TimeZone"),
        date_style: get("DateStyle"),
        in_hot_standby: get("in_hot_standby"),
        application_name: get("application_name"),
    }
}
