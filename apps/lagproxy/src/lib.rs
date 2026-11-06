//! lagproxy: a TCP proxy that holds every byte back for a fixed time, in both directions.
//!
//! On localhost a round trip to PostgreSQL takes a few dozen microseconds, so the cost of
//! round trips, the subject of half the book, cannot be seen. Put this proxy in between with
//! a delay of a few milliseconds and the network behaves like one between two data centres:
//! every round trip costs twice the delay, and pipelining, batching and pool sizing all start
//! to show in the numbers (Chapters 4, 7, 10 and 13).
//!
//! It adds *latency*, not a bandwidth limit: chunks are timestamped as they arrive and sent
//! on, in order, once their time has come, so many can be in flight at once.
//!
//! The delay is a lower bound. Tokio's timer rounds a sleep up to the next millisecond, and
//! the operating system adds slack of its own: on the Mac the book was written on, a 1 ms
//! sleep took 2.3 ms, and a round trip through the proxy cost `2 × delay` plus 3 to 6 ms. That
//! is why the book's examples compare timings with each other, never with a fixed number.
//!
//! ```no_run
//! # async fn demo() -> std::io::Result<()> {
//! use std::time::Duration;
//! let proxy = lagproxy::Proxy::start("127.0.0.1:0", "localhost:5432", Duration::from_millis(5)).await?;
//! // Connect to `proxy.addr()` instead of the server: each round trip now takes 10 ms longer.
//! # Ok(()) }
//! ```

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream, ToSocketAddrs};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::Instant;

/// A running proxy. It stops accepting connections when dropped.
pub struct Proxy {
    addr: SocketAddr,
    accept: JoinHandle<()>,
}

impl Proxy {
    /// Listens on `listen` (port 0: any free port) and forwards every connection to
    /// `upstream`, delaying each direction by `delay`.
    pub async fn start(
        listen: impl ToSocketAddrs,
        upstream: impl Into<String>,
        delay: Duration,
    ) -> io::Result<Proxy> {
        let listener = TcpListener::bind(listen).await?;
        let addr = listener.local_addr()?;
        let upstream: Arc<str> = upstream.into().into();
        let accept = tokio::spawn(async move {
            loop {
                let Ok((client, _)) = listener.accept().await else {
                    continue;
                };
                let upstream = Arc::clone(&upstream);
                tokio::spawn(async move {
                    if let Ok(server) = TcpStream::connect(&*upstream).await {
                        forward(client, server, delay).await;
                    }
                });
            }
        });
        Ok(Proxy { addr, accept })
    }

    /// Where to connect instead of the upstream server.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }
}

impl Drop for Proxy {
    fn drop(&mut self) {
        self.accept.abort();
    }
}

async fn forward(client: TcpStream, server: TcpStream, delay: Duration) {
    // The proxy's own buffering must not add delay of its own.
    let _ = client.set_nodelay(true);
    let _ = server.set_nodelay(true);
    let (client_read, client_write) = client.into_split();
    let (server_read, server_write) = server.into_split();
    tokio::join!(
        pipe(client_read, server_write, delay),
        pipe(server_read, client_write, delay),
    );
}

/// Copies `from` to `to`, holding every chunk back for `delay`.
async fn pipe(mut from: OwnedReadHalf, mut to: OwnedWriteHalf, delay: Duration) {
    let (queue, mut queued) = mpsc::unbounded_channel::<(Instant, Vec<u8>)>();
    let read = async move {
        let mut buffer = vec![0u8; 64 * 1024];
        loop {
            match from.read(&mut buffer).await {
                Ok(0) | Err(_) => break, // end of stream: dropping `queue` ends the writer
                Ok(n) => {
                    if queue
                        .send((Instant::now() + delay, buffer[..n].to_vec()))
                        .is_err()
                    {
                        break;
                    }
                }
            }
        }
    };
    let write = async move {
        while let Some((due, chunk)) = queued.recv().await {
            tokio::time::sleep_until(due).await;
            if to.write_all(&chunk).await.is_err() {
                break;
            }
        }
        // Pass the end of the stream on, once everything before it has been delivered.
        let _ = to.shutdown().await;
    };
    tokio::join!(read, write);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An echo server, to test the proxy without a database.
    async fn echo_server() -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let (mut r, mut w) = socket.split();
                    let _ = tokio::io::copy(&mut r, &mut w).await;
                });
            }
        });
        addr
    }

    #[tokio::test]
    async fn a_round_trip_costs_twice_the_delay() {
        let echo = echo_server().await;
        let proxy = Proxy::start("127.0.0.1:0", echo.to_string(), Duration::from_millis(25))
            .await
            .unwrap();
        let mut socket = TcpStream::connect(proxy.addr()).await.unwrap();
        let started = Instant::now();
        socket.write_all(b"ping").await.unwrap();
        let mut reply = [0u8; 4];
        socket.read_exact(&mut reply).await.unwrap();
        assert_eq!(&reply, b"ping");
        assert!(
            started.elapsed() >= Duration::from_millis(50),
            "{:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn it_adds_latency_not_a_bandwidth_limit() {
        let echo = echo_server().await;
        let proxy = Proxy::start("127.0.0.1:0", echo.to_string(), Duration::from_millis(25))
            .await
            .unwrap();
        let mut socket = TcpStream::connect(proxy.addr()).await.unwrap();
        let started = Instant::now();
        // Ten messages sent back to back are all in flight at once: one round trip in all,
        // not ten.
        for i in 0..10u8 {
            socket.write_all(&[i; 100]).await.unwrap();
        }
        let mut reply = vec![0u8; 1000];
        socket.read_exact(&mut reply).await.unwrap();
        assert_eq!(reply[999], 9);
        let elapsed = started.elapsed();
        assert!(
            elapsed >= Duration::from_millis(50) && elapsed < Duration::from_millis(250),
            "{elapsed:?}"
        );
    }
}
