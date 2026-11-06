//! minipool: a connection pool in about a hundred lines (Chapter 13).
//!
//! Not for production (use deadpool-postgres or bb8), but every decision a pool makes is
//! here, in plain sight:
//!
//! * **How many connections at most?** A [`Semaphore`] with `max_size` permits. A caller
//!   holds a permit for as long as it holds a connection, so there are never more than
//!   `max_size` connections in use, and every connection in use or idle was created under a
//!   permit.
//! * **What if all are busy?** [`Pool::get`] waits for a permit, but only for `wait_timeout`:
//!   an overloaded service should fail fast rather than queue requests without bound.
//! * **Which idle connection next?** The most recently used one (a stack). That keeps a few
//!   connections busy and warm, and lets the others sit idle.
//! * **Is it still alive?** `is_closed()` is free and catches a connection whose task has
//!   seen it die; `check_connection()` costs a round trip and catches the rest
//!   ([`Options::verify`]). deadpool calls these `RecyclingMethod::Fast` and `Verified`.
//! * **What goes back?** Whatever the borrower leaves behind. A dropped `Transaction` has
//!   already queued its `ROLLBACK`, but a `SET` or a `LISTEN` stays with the session.

use std::ops::{Deref, DerefMut};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_postgres::{Client, Config, NoTls};

#[derive(Debug, Clone)]
pub struct Options {
    pub max_size: usize,
    pub wait_timeout: Duration,
    /// Make a round trip on every checkout to be sure the connection works.
    pub verify: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            max_size: 8,
            wait_timeout: Duration::from_secs(5),
            verify: false,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PoolError {
    #[error("no connection became free within the wait timeout")]
    Timeout,
    #[error("could not connect: {0}")]
    Connect(#[from] tokio_postgres::Error),
}

/// A pool of connections to one database. Cheap to clone: clones share the pool.
#[derive(Clone)]
pub struct Pool {
    inner: Arc<Inner>,
}

struct Inner {
    config: Config,
    options: Options,
    permits: Arc<Semaphore>,
    // A std mutex, not tokio's: it is never held across an `.await`.
    idle: Mutex<Vec<Client>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Status {
    pub max_size: usize,
    pub in_use: usize,
    pub idle: usize,
}

impl Pool {
    pub fn new(config: Config, options: Options) -> Pool {
        let permits = Arc::new(Semaphore::new(options.max_size));
        Pool {
            inner: Arc::new(Inner {
                config,
                options,
                permits,
                idle: Mutex::new(Vec::new()),
            }),
        }
    }

    /// A connection: an idle one if a healthy one is left, a new one otherwise.
    pub async fn get(&self) -> Result<Conn, PoolError> {
        let permit = tokio::time::timeout(
            self.inner.options.wait_timeout,
            Arc::clone(&self.inner.permits).acquire_owned(),
        )
        .await
        .map_err(|_| PoolError::Timeout)?
        .expect("the semaphore is never closed");

        loop {
            let candidate = self.inner.idle.lock().expect("not poisoned").pop();
            let Some(client) = candidate else { break };
            if client.is_closed() {
                continue; // its connection task has ended: drop it, try the next
            }
            if self.inner.options.verify && client.check_connection().await.is_err() {
                continue;
            }
            return Ok(self.lend(client, permit));
        }

        let (client, connection) = self.inner.config.connect(NoTls).await?;
        tokio::spawn(async move {
            let _ = connection.await; // ends when the client is dropped or the server goes away
        });
        Ok(self.lend(client, permit))
    }

    fn lend(&self, client: Client, permit: OwnedSemaphorePermit) -> Conn {
        Conn {
            client: Some(client),
            pool: Arc::clone(&self.inner),
            _permit: permit,
        }
    }

    pub fn status(&self) -> Status {
        Status {
            max_size: self.inner.options.max_size,
            in_use: self.inner.options.max_size - self.inner.permits.available_permits(),
            idle: self.inner.idle.lock().expect("not poisoned").len(),
        }
    }
}

/// A borrowed connection. It derefs to `Client` (mutably too, for `transaction()`), and goes
/// back to the pool when dropped.
pub struct Conn {
    client: Option<Client>,
    pool: Arc<Inner>,
    // Declared last, so it is dropped last: the client is back on the idle stack before the
    // permit lets the next caller in. The other way round, that caller could find the stack
    // empty and open a connection too many.
    _permit: OwnedSemaphorePermit,
}

impl Conn {
    /// Keep the connection: it leaves the pool for good, and its place is freed.
    pub fn detach(mut self) -> Client {
        self.client
            .take()
            .expect("present until dropped or detached")
    }
}

impl Deref for Conn {
    type Target = Client;
    fn deref(&self) -> &Client {
        self.client
            .as_ref()
            .expect("present until dropped or detached")
    }
}

impl DerefMut for Conn {
    fn deref_mut(&mut self) -> &mut Client {
        self.client
            .as_mut()
            .expect("present until dropped or detached")
    }
}

impl Drop for Conn {
    fn drop(&mut self) {
        if let Some(client) = self.client.take()
            && !client.is_closed()
        {
            self.pool.idle.lock().expect("not poisoned").push(client);
        }
    }
}
