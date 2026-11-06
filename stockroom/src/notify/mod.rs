//! Events from the database to the rest of the process (Chapter 11).
//!
//! One task ([`listener::run`]) holds a dedicated connection that `LISTEN`s, and publishes
//! what arrives on the [`Hub`]: order events to every subscriber (the SSE streams), and
//! "the outbox has work" to the worker.

pub mod listener;

use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tokio::sync::{Notify, broadcast};
use uuid::Uuid;

use crate::domain::OrderStatus;

/// The payload of a notification on `order_events`, as the trigger in
/// `migrations/0002_order_events.sql` builds it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrderEvent {
    pub order_id: Uuid,
    pub store_id: Uuid,
    pub status: OrderStatus,
    pub previous: Option<OrderStatus>,
    pub version: i32,
}

#[derive(Debug, Clone)]
pub enum HubEvent {
    Order(OrderEvent),
    /// Events may have been missed (the listener reconnected, or a subscriber lagged):
    /// re-read the current state instead of trusting the stream.
    Resync,
}

/// What the server reported about itself when the listener connected: `ParameterStatus`
/// messages, read with `Connection::parameter`.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerInfo {
    pub server_version: Option<String>,
    pub server_encoding: Option<String>,
    pub time_zone: Option<String>,
    pub date_style: Option<String>,
    pub in_hot_standby: Option<String>,
    pub application_name: Option<String>,
}

#[derive(Clone)]
pub struct Hub {
    inner: Arc<Inner>,
}

struct Inner {
    events: broadcast::Sender<HubEvent>,
    outbox: Notify,
    server: Mutex<Option<ServerInfo>>,
}

impl Hub {
    /// `capacity`: how far a subscriber may fall behind before it starts missing events.
    pub fn new(capacity: usize) -> Hub {
        let (events, _) = broadcast::channel(capacity);
        Hub {
            inner: Arc::new(Inner {
                events,
                outbox: Notify::new(),
                server: Mutex::new(None),
            }),
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<HubEvent> {
        self.inner.events.subscribe()
    }

    pub fn publish(&self, event: HubEvent) {
        // An error only means that nobody is subscribed right now.
        let _ = self.inner.events.send(event);
    }

    /// Wake the outbox worker. A wake-up sent while the worker is busy is kept, not lost.
    pub fn wake_outbox(&self) {
        self.inner.outbox.notify_one();
    }

    pub async fn outbox_woken(&self) {
        self.inner.outbox.notified().await;
    }

    /// `None` until the listener has connected once.
    pub fn server_info(&self) -> Option<ServerInfo> {
        self.inner
            .server
            .lock()
            .expect("never poisoned: no code panics while holding it")
            .clone()
    }

    fn set_server_info(&self, info: ServerInfo) {
        *self
            .inner
            .server
            .lock()
            .expect("never poisoned: no code panics while holding it") = Some(info);
    }
}
