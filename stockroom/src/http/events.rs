//! `GET /orders/events`: server-sent events from `LISTEN order_events` (Chapter 16).

use std::convert::Infallible;

use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use futures_util::{Stream, StreamExt};
use serde::Deserialize;
use tokio::sync::broadcast::error::RecvError;

use super::{AppQuery, AppState};
use crate::notify::HubEvent;
use crate::{AppError, repo};

#[derive(Debug, Deserialize)]
pub struct EventsQuery {
    /// Only this store's orders.
    store: Option<String>,
}

/// One event per order change: `event: order` with the change as JSON. An `event: resync`
/// means events may have been missed, so the client should reload what it shows.
///
/// The stream holds no database connection: it reads the hub, which the single listener
/// connection feeds. (The connection used to look up the store goes back to the pool at
/// once.) It ends at shutdown, or graceful shutdown would wait for it forever.
pub async fn orders(
    State(state): State<AppState>,
    AppQuery(query): AppQuery<EventsQuery>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, AppError> {
    let store_id = match &query.store {
        Some(code) => Some(repo::stores::id_by_code(&state.pool.get().await?, code).await?),
        None => None,
    };
    let mut events = state.hub.subscribe();
    let stream = async_stream::stream! {
        loop {
            match events.recv().await {
                Ok(HubEvent::Order(event)) => {
                    if store_id.is_some_and(|id| id != event.store_id) {
                        continue;
                    }
                    let id = format!("{}:{}", event.order_id, event.version);
                    match Event::default().event("order").id(id).json_data(&event) {
                        Ok(sse) => yield Ok(sse),
                        Err(error) => tracing::warn!(%error, "could not encode an order event"),
                    }
                }
                Ok(HubEvent::Resync) | Err(RecvError::Lagged(_)) => {
                    yield Ok(Event::default().event("resync").data("reload"));
                }
                Err(RecvError::Closed) => break,
            }
        }
    };
    let stream = stream.take_until(state.shutdown.clone().cancelled_owned());
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}
