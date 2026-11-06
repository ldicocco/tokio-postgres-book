//! Cancelling a query whose answer nobody is waiting for any more (Chapter 12, Chapter 16).

use std::ops::Deref;

use deadpool_postgres::Object;

use super::MakeTls;

/// Owns a pooled connection while a slow query runs on it. If the guard is dropped before
/// [`CancelOnDrop::finish`] (the request timed out, or the client went away and axum dropped
/// the handler's future), it asks the server to cancel the query.
///
/// Dropping a future does not stop a query: the server keeps working, and the connection
/// keeps waiting for the answer. Only a cancel request, sent on a separate connection with
/// the `CancelToken`, stops it.
///
/// The connection is then *discarded*, not returned to the pool. The cancel request is
/// asynchronous, so if the query finished at about the same moment, the request could land
/// on whatever the next borrower runs on that connection.
pub struct CancelOnDrop {
    conn: Option<Object>,
    tls: MakeTls,
}

impl CancelOnDrop {
    pub fn new(conn: Object, tls: MakeTls) -> Self {
        CancelOnDrop {
            conn: Some(conn),
            tls,
        }
    }

    /// The query finished: disarm the guard and hand the connection back.
    pub fn finish(mut self) -> Object {
        self.conn
            .take()
            .expect("the connection is only taken here and in drop")
    }
}

impl Deref for CancelOnDrop {
    type Target = Object;
    fn deref(&self) -> &Object {
        self.conn
            .as_ref()
            .expect("the connection is only taken here and in drop")
    }
}

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        let Some(conn) = self.conn.take() else { return };
        let token = conn.cancel_token();
        let tls = self.tls.clone();
        // `drop` cannot await, so the cancel request goes out from a task of its own.
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        runtime.spawn(async move {
            match token.cancel_query(tls).await {
                Ok(()) => tracing::info!("cancelled a query nobody was waiting for"),
                Err(error) => tracing::warn!(%error, "could not cancel an abandoned query"),
            }
            // Detach from the pool and close.
            drop(Object::take(conn));
        });
    }
}
