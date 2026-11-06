# Chapter 13 — Connection pooling

> **Async PostgreSQL in Rust** · Previous: [Chapter 12 — Production connections: timeouts, cancellation, health and TLS](12-production-connections.md) ·
> Next: [Chapter 14 — Schema, migrations and the data layer](14-data-layer.md)

Part II showed how much one connection can do: it pipelines, streams, copies and listens. A
web service still needs more than one, because a request that runs a transaction needs a
connection to itself, a slow report must not hold up everyone else, and a connection that dies
should take one request down with it, not all of them. Opening a connection per request is the
wrong answer: a TCP handshake, perhaps TLS, authentication and a new server process add up to
several round trips, and milliseconds, before the first query.

A *pool* keeps a set of open connections and lends them out. This chapter builds one from
scratch to show every decision a pool makes, then turns to the one stockroom uses,
deadpool-postgres, and measures when a pool helps and when one shared client does better.

---

## 13.1 Why one `Client` is not enough

Chapter 7 shared one `Client` between tasks, and for independent queries that is hard to beat.
Three things break it:

- **Transactions.** `Client::transaction` takes `&mut self` (Chapter 8), so a client shared
  through an `Arc` cannot start one. That is the borrow checker enforcing a real rule: a
  transaction is a state of the *session*, and anything else sent on the session joins it.
- **Head-of-line blocking.** The server runs one statement at a time per session. A slow query,
  or a slowly read stream, holds up every request behind it (Chapters 7 and 9).
- **Failure isolation.** One connection is one point of failure. If it dies, every task using
  it fails at once, and a cancel request meant for one task's query can stop another's
  (Chapter 12).

A pool answers all three: each borrower gets a session of its own, for as long as it needs it.
It does not answer them for free, and §13.5 shows the price.

## 13.2 A pool from scratch: minipool

**minipool** (`apps/minipool`) is a working pool in about a hundred lines, written to make each
decision visible. Its state is a semaphore and a stack:

~~~rust
// apps/minipool/src/lib.rs (excerpt)
struct Inner {
    config: Config,
    options: Options,
    permits: Arc<Semaphore>,
    // A std mutex, not tokio's: it is never held across an `.await`.
    idle: Mutex<Vec<Client>>,
}
~~~

**How many connections at most?** A `tokio::sync::Semaphore` with `max_size` permits. A borrower
holds a permit for as long as it holds a connection, so there are never more than `max_size`
in use. **What if all are busy?** Wait for a permit, but not forever: an overloaded service
should fail fast instead of queueing requests without bound. **Which idle connection next?**
The most recently used, which keeps a few connections warm. **Is it still alive?** Check
before lending:

~~~rust
// apps/minipool/src/lib.rs (excerpt)
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
~~~

The borrowed connection is a guard, `Conn`, which derefs to `Client` (mutably too, so that
`transaction()` works) and puts the client back when it is dropped:

~~~rust
// apps/minipool/src/lib.rs (excerpt)
pub struct Conn {
    client: Option<Client>,
    pool: Arc<Inner>,
    // Released after `Drop::drop` has put the client back on the idle stack, so the next
    // caller the permit lets in finds it there instead of opening a connection too many.
    _permit: OwnedSemaphorePermit,
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
~~~

[apps/minipool/src/lib.rs](../apps/minipool/src/lib.rs)

> **Rust refresher:** a struct's own `Drop::drop` runs first, and only then are its fields
> dropped, in declaration order. So `Conn::drop` has pushed the client back before `_permit`
> releases the semaphore, whatever the order of the fields. The order matters only between
> fields: had the client been returned by a field's own `Drop` instead, that field would have to
> be declared before `_permit`, or a waiter let in by the permit could find the stack still
> empty and open a connection beyond `max_size`.

Two more lessons hide in this code. What goes back to the pool is *whatever the borrower left
behind*: a dropped `Transaction` has already queued its `ROLLBACK`, but a `SET`, a temporary
table or a `LISTEN` stays with the session. And `is_closed()` only knows what the connection
task has seen, which §13.3 puts to the test.

## 13.3 deadpool-postgres

[deadpool-postgres](https://docs.rs/deadpool-postgres/0.14.2/deadpool_postgres/) (0.14.2, on
deadpool 0.13) is the same design, finished: a `Manager` that knows how to create and recycle
tokio-postgres connections, and a generic `Pool` with timeouts, hooks and metrics. The `Manager`
is built from a `tokio_postgres::Config`, a TLS connector, and a `ManagerConfig` that holds the one
decision deadpool leaves to you, the
[`RecyclingMethod`](https://docs.rs/deadpool-postgres/0.14.2/deadpool_postgres/enum.RecyclingMethod.html):

| Method | Runs when a connection is reused | Catches |
|---|---|---|
| `Fast` (the default) | `is_closed()` only | connections whose task has ended |
| `Verified` | `is_closed()` and an empty query: one round trip | connections that died without the task noticing yet |
| `Clean` | `is_closed()` and `CLOSE ALL; SET SESSION AUTHORIZATION DEFAULT; RESET ALL; UNLISTEN *; SELECT pg_advisory_unlock_all(); DISCARD TEMP; DISCARD SEQUENCES;` | the above, plus session state left behind |
| `Custom(sql)` | `is_closed()` and your SQL | what your SQL checks |

`Clean` is `DISCARD ALL` without `DEALLOCATE ALL` and `DISCARD PLANS`, so that the statement
cache of §13.4 stays valid. Here is what each one does with state a borrower leaves behind, in a
pool of one connection so that the second borrower is sure to get the first one's session:

```rust
use deadpool_postgres::{Manager, ManagerConfig, Pool, RecyclingMethod, Runtime};
use tokio_postgres::{Config, NoTls};

fn pool_of_one(recycling_method: RecyclingMethod) -> Pool {
    let config: Config = std::env::var("DATABASE_URL")
        .expect("DATABASE_URL")
        .parse()
        .expect("a valid connection string");
    let manager = Manager::from_config(config, NoTls, ManagerConfig { recycling_method });
    Pool::builder(manager)
        .max_size(1)
        .runtime(Runtime::Tokio1)
        .build()
        .expect("a valid pool configuration")
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    for method in [
        RecyclingMethod::Fast,
        RecyclingMethod::Verified,
        RecyclingMethod::Clean,
    ] {
        let pool = pool_of_one(method.clone());
        {
            let first = pool.get().await?;
            first
                .batch_execute(
                    "SET application_name = 'left behind';
                     CREATE TEMP TABLE scratch (x int);
                     LISTEN left_behind;",
                )
                .await?;
        } // back to the pool
        let second = pool.get().await?;
        let row = second
            .query_one(
                "SELECT current_setting('application_name'),
                        to_regclass('pg_temp.scratch') IS NOT NULL,
                        (SELECT count(*) FROM pg_listening_channels())",
                &[],
            )
            .await?;
        let (name, temp, listening): (String, bool, i64) = (row.get(0), row.get(1), row.get(2));
        println!(
            "{method:?}: application_name {name:?}, temp table {temp}, listening on {listening}"
        );
        assert_eq!(temp, method != RecyclingMethod::Clean);
    }
    Ok(())
}
```

```text
Fast: application_name "left behind", temp table true, listening on 1
Verified: application_name "left behind", temp table true, listening on 1
Clean: application_name "", temp table false, listening on 0
```

So `Fast`, like `Verified`, is right only for code that leaves nothing behind, which is a
property of your code, not of the pool. stockroom makes that promise (§ In stockroom); a service
that cannot make it wants `Clean`.

The difference between `Fast` and `Verified` shows when a connection dies while it is idle in
the pool. `Fast` can hand it out before its connection task has read the server's goodbye:

```rust
use deadpool_postgres::{Manager, ManagerConfig, Pool, RecyclingMethod, Runtime};
use tokio_postgres::{Config, NoTls};

fn pool_of_one(recycling_method: RecyclingMethod) -> Pool {
    let config: Config = std::env::var("DATABASE_URL")
        .expect("DATABASE_URL")
        .parse()
        .expect("a valid connection string");
    let manager = Manager::from_config(config, NoTls, ManagerConfig { recycling_method });
    Pool::builder(manager)
        .max_size(1)
        .runtime(Runtime::Tokio1)
        .build()
        .expect("a valid pool configuration")
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let admin = playground::connect().await?;
    for method in [RecyclingMethod::Fast, RecyclingMethod::Verified] {
        let pool = pool_of_one(method.clone());
        let pid: i32 = {
            let conn = pool.get().await?;
            conn.query_one("SELECT pg_backend_pid()", &[]).await?.get(0)
        };
        // The idle pooled connection is killed; its task may not have noticed yet.
        admin
            .execute("SELECT pg_terminate_backend($1)", &[&pid])
            .await?;
        let conn = pool.get().await?;
        let first_query = conn.query_one("SELECT pg_backend_pid()", &[]).await;
        println!(
            "{method:?}: first query {}",
            match &first_query {
                Ok(_) => "succeeded".to_owned(),
                Err(e) => format!("failed: {e}"),
            }
        );
        if method == RecyclingMethod::Verified {
            let new_pid: i32 = first_query?.get(0);
            assert_ne!(new_pid, pid); // a fresh connection
        }
    }
    Ok(())
}
```

```text
Fast: first query failed: db error
Verified: first query succeeded
```

(The `Fast` line depends on a race, which `Fast` lost on every run here: `pg_terminate_backend`
returns before the backend's goodbye has reached the client.) `Verified` pays one round trip on
every checkout for that guarantee. On localhost that is noise; on a network with a 2 ms round
trip it is a 2 ms tax on every request. Many services choose `Fast` and handle the rare failure
with a retry, or the HTTP 503 it becomes; services whose database can fail over tend to choose
`Verified`.

The `Pool` itself is configured with a builder:

~~~rust
// stockroom/src/db/pool.rs (excerpt)
pub fn create(config: Config, tls: MakeTls, max_size: usize) -> Result<Pool, BuildError> {
    let manager = Manager::from_config(
        config,
        tls,
        ManagerConfig {
            recycling_method: RecyclingMethod::Fast,
        },
    );
    Pool::builder(manager)
        .max_size(max_size)
        .runtime(Runtime::Tokio1)
        // Waiting longer than this for a free connection means the service is overloaded:
        // better to answer 503 than to queue requests without bound.
        .wait_timeout(Some(Duration::from_secs(5)))
        .create_timeout(Some(Duration::from_secs(5)))
        .recycle_timeout(Some(Duration::from_secs(5)))
        .build()
}
~~~

[stockroom/src/db/pool.rs](../stockroom/src/db/pool.rs)

The three timeouts bound the three ways `get()` can wait: for a free slot, for a new connection
to open, and for the recycling check. They need a runtime to run their timers, hence
`.runtime(Runtime::Tokio1)`; without one, `build()` refuses a configuration with timeouts
(`BuildError::NoRuntimeSpecified`). Without any timeouts, `get()` waits as long as it takes, which under
overload means forever. deadpool also has `post_create`, `pre_recycle` and `post_recycle` hooks,
for a `SET` that every new session should run, say.

## 13.4 `Object`, `prepare_cached`, `transaction` and `Object::take`

`pool.get()` returns an
[`Object`](https://docs.rs/deadpool-postgres/0.14.2/deadpool_postgres/type.Client.html)
(deadpool-postgres also calls it `Client`), which goes back to the pool when dropped. It derefs
to deadpool's `ClientWrapper`, which derefs to `tokio_postgres::Client`, so every method of the
earlier chapters works on it, and `&object` coerces to `&tokio_postgres::Client`.
deadpool-postgres adds three things:

- **`prepare_cached(sql)`**: a per-connection statement cache. The first call prepares (one
  round trip); later calls with the same SQL on the same connection return the cached
  `Statement` without asking the server.
- **`transaction()` and `build_transaction()`**, returning deadpool's `Transaction`, which also
  has `prepare_cached`.
- **A `GenericClient` trait** of its own, implemented by `Object` and deadpool's `Transaction`,
  with `prepare_cached` in it. Repository functions written against it work in and out of
  transactions (Chapter 14).

And `Object::take(object)` detaches the connection from the pool for good: the pool forgets it
and may open another in its place.

```rust
use deadpool_postgres::{Manager, Object, Pool, Runtime};
use std::time::{Duration, Instant};
use tokio_postgres::{Config, NoTls};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config: Config = std::env::var("DATABASE_URL")?.parse()?;
    let pool = Pool::builder(Manager::new(config, NoTls))
        .max_size(2)
        .runtime(Runtime::Tokio1)
        .wait_timeout(Some(Duration::from_millis(100)))
        .build()?;

    let a = pool.get().await?;
    let mut b = pool.get().await?;
    println!("{:?}", pool.status());
    // Both are out: the third waits 100 ms, then gives up.
    let err = pool.get().await.unwrap_err();
    println!("{err}");

    // The statement cache: the first call asks the server, the second does not.
    let started = Instant::now();
    let statement = b.prepare_cached("SELECT $1::int + 1").await?;
    let first = started.elapsed();
    let started = Instant::now();
    b.prepare_cached("SELECT $1::int + 1").await?;
    let second = started.elapsed();
    println!("prepare_cached: {first:?}, then {second:?}");
    assert!(second < first);
    assert_eq!(b.statement_cache.size(), 1);

    // A deadpool transaction, which has prepare_cached too.
    let tx = b.transaction().await?;
    let cached = tx.prepare_cached("SELECT $1::int + 1").await?;
    assert_eq!(cached.columns().len(), statement.columns().len());
    let two: i32 = tx.query_one(&cached, &[&1i32]).await?.get(0);
    tx.commit().await?;
    assert_eq!(two, 2);

    // Detach `a` for good: the pool shrinks, and `a` lives on as a plain client.
    let detached = Object::take(a);
    drop(b);
    println!("{:?}", pool.status());
    detached.query_one("SELECT 1", &[]).await?;
    Ok(())
}
```

```text
Status { max_size: 2, size: 2, available: 0, waiting: 0 }
Timeout occurred while waiting for a slot to become available
prepare_cached: 1.251833ms, then 3.458µs
Status { max_size: 2, size: 1, available: 1, waiting: 0 }
```

`Object::take` is for connections that must not go back: one whose session state you cannot
undo, one with a cancel request possibly in flight (Chapter 12's `CancelOnDrop`), or one you
want to keep for a long-lived job.

> **Pitfall:** `prepare_cached` caches per connection, by SQL text. A statement prepared before
> a migration changes a table's columns can fail afterwards with "cached plan must not change
> result type" (0A000) on every connection that cached it. After schema changes at run time,
> clear the caches (`pool.manager().statement_caches.clear()`) or restart the service, which is
> what most deployments do anyway.

## 13.5 How big? Measured

PostgreSQL limits connections with `max_connections` (100 by default, of which
`superuser_reserved_connections`, 3 by default, are kept for superusers), and each connection
is a server process with its own memory. Every instance of every service that connects takes
from that budget: four instances with pools of 25 already use it all. So the question is never
"how many could help?", but "how few are enough?".

`minipool`'s binary measures the trade-off. 64 tasks each run 50 units of work through one
shared client, through minipool, and through deadpool, with pools of 8. A unit is either one
`query_typed` (one round trip) or a transaction around it (`BEGIN`, query, `COMMIT`: three round
trips that cannot be pipelined with anyone else's). Release build, PG 18.6 on localhost, Apple
Silicon:

```bash
cargo run -p minipool --release -- --tasks 64 --pool-size 8
cargo run -p minipool --release -- --tasks 64 --pool-size 8 --delay-ms 2
```

```text
64 tasks × 50 iterations = 3200 operations, pools of 8, no added one-way delay

| Setup | Work | Time | Operations per second |
|---|---|---:|---:|
| one shared client | query | 0.050 s | 63403 |
| minipool, 8 connections | query | 0.041 s | 78587 |
| deadpool, 8 connections | query | 0.041 s | 77514 |
| minipool, 8 connections | transaction | 0.107 s | 29786 |
| deadpool, 8 connections | transaction | 0.107 s | 30006 |

64 tasks × 50 iterations = 3200 operations, pools of 8, 2 ms one-way delay

| Setup | Work | Time | Operations per second |
|---|---|---:|---:|
| one shared client | query | 0.383 s | 8350 |
| minipool, 8 connections | query | 3.084 s | 1038 |
| deadpool, 8 connections | query | 3.328 s | 962 |
| minipool, 8 connections | transaction | 9.577 s | 334 |
| deadpool, 8 connections | transaction | 9.093 s | 352 |
```

What the numbers say:

- **On localhost, a pool is a little faster for independent queries**: eight server processes
  work in parallel, while one connection's statements run one at a time.
- **Across a network, one pipelined client is eight times faster for independent queries.** A
  pool of 8 has at most 8 requests in flight, each waiting a full round trip; the shared client
  has all 64 in flight on one connection. This is Chapter 7's lesson with a number on it.
- **Transactions are where a pool is required**, and their cost is round trips: three per
  transaction, with the connection held for all of them. 352 transactions a second on eight
  connections is 44 per connection, about 23 ms each: three round trips of about 8 ms through
  lagproxy at a 2 ms delay. Only more connections, or fewer round trips, raise that ceiling.
- **minipool and deadpool are equal.** The cost is in the round trips, not the pool.

So size a pool for the transactions in flight at once, and no larger: roughly the rate at
which requests need a session to themselves, times how long each holds it (three round trips,
here), which is Little's law. Throughput beyond that comes from fewer round trips per transaction (pipelining inside it,
Chapter 7; doing the work in one statement), not from more connections.

## 13.6 bb8, and PgBouncer

**bb8** is the other common pool. Its PostgreSQL adapter, `bb8-postgres` (0.9.0, on bb8 0.9.1,
checked 2026-11-06), wraps the same `tokio_postgres::Client`. The design differs in emphasis:
bb8 runs a background "reaper" that closes idle connections after `idle_timeout` and old ones
after `max_lifetime`, keeps `min_idle` connections open, and checks connections on checkout with
`test_on_check_out` (on by default), much like `Verified`. bb8-postgres has no statement cache;
with it you prepare statements yourself or use `query_typed`. For a service like stockroom the
two are interchangeable, and this book uses deadpool for its statement cache and simpler API.

~~~rust
// bb8 (illustration)
let manager = bb8_postgres::PostgresConnectionManager::new(config, NoTls);
let pool = bb8::Pool::builder()
    .max_size(8)
    .connection_timeout(Duration::from_secs(5))
    .idle_timeout(Some(Duration::from_secs(600)))
    .build(manager)
    .await?;
let conn = pool.get().await?; // a PooledConnection that derefs to tokio_postgres::Client
~~~

**PgBouncer** is a pool outside the process: a proxy that many service instances connect to,
multiplexing them onto a few server connections. In *transaction* pooling mode, the most useful
one, a client's consecutive transactions may run on different server connections, so session
state does not survive between transactions. That rules out everything §13.7 lists, and it
used to rule out named prepared statements too. PgBouncer 1.21 (2023) added support for
protocol-level named prepared statements, and since 1.24 it is on by default
(`max_prepared_statements = 200`), so tokio-postgres's `prepare` and deadpool's
`prepare_cached` work through current versions. Behind an older PgBouncer, or one configured
without it, use `query_typed` (Chapter 4): it sends the unnamed statement with its parameter
types in one round trip, and never depends on the server remembering anything.

## 13.7 What not to do with a pooled connection

A pooled connection is borrowed, and the next borrower inherits its session. So:

- **No `LISTEN`.** deadpool drives each connection by simply awaiting it, so notifications are
  dropped, and `Clean` unlistens anyway. Listen on a dedicated connection (Chapter 11).
- **No streams that outlive their `Object`.** A `RowStream` or `CopyOutStream` does not borrow
  the connection; if the `Object` returns to the pool while one is unread, the next borrower's
  queries queue behind it. Move the `Object` into the stream, and cancel and discard it if the
  stream is dropped early (Chapter 9's "In stockroom").
- **No session `SET`.** Use `SET LOCAL` inside a transaction, which ends with it, or startup
  options on the pool's `Config` (Chapter 12), which every connection gets.
- **No session-level advisory locks or temporary tables that outlive a transaction.** Use
  `pg_advisory_xact_lock` and `ON COMMIT DROP`.
- **No long transactions.** A borrowed connection held for seconds is one connection fewer for
  everyone else (§13.5), and its snapshot holds back `VACUUM` (Chapter 9).

### Side by side — *a pool*

The task: a pool of at most 8 connections that gives up after 5 seconds of waiting, and a
transaction on a connection from it.

~~~rust
// sqlx (illustration)
let pool = sqlx::postgres::PgPoolOptions::new()
    .max_connections(8)
    .acquire_timeout(Duration::from_secs(5))
    .connect(&url)
    .await?;
let mut tx = pool.begin().await?;
sqlx::query("INSERT INTO audit (note) VALUES ($1)").bind("pooled").execute(&mut *tx).await?;
tx.commit().await?;
~~~

```rust
use deadpool_postgres::{Manager, ManagerConfig, Pool, RecyclingMethod, Runtime};
use std::time::Duration;
use tokio_postgres::{Config, NoTls};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config: Config = std::env::var("DATABASE_URL")?.parse()?;
    let manager = Manager::from_config(
        config,
        NoTls,
        ManagerConfig {
            recycling_method: RecyclingMethod::Fast,
        },
    );
    let pool = Pool::builder(manager)
        .max_size(8)
        .runtime(Runtime::Tokio1)
        .wait_timeout(Some(Duration::from_secs(5)))
        .build()?;

    let mut conn = pool.get().await?;
    let tx = conn.transaction().await?;
    tx.batch_execute("CREATE TEMP TABLE audit (note text) ON COMMIT DROP")
        .await?;
    let insert = tx
        .prepare_cached("INSERT INTO audit (note) VALUES ($1)")
        .await?;
    assert_eq!(tx.execute(&insert, &[&"pooled"]).await?, 1);
    tx.commit().await?;
    Ok(())
}
```

| | sqlx | tokio-postgres + deadpool |
|---|---|---|
| **Same** | A bounded pool with a wait timeout; a transaction borrows one connection | A bounded pool with a wait timeout; a transaction borrows one connection |
| **Different** | The pool is part of the driver, and checks each connection before lending it by default (`test_before_acquire`) | The pool is a separate crate over `tokio_postgres::Client`; the check on reuse (`RecyclingMethod`) is your choice, and `prepare_cached` caches statements on request |

---

## In stockroom

`db::pool::create` (§13.3) is the whole pool: `Fast` recycling, three 5-second timeouts. The
comment on it is the contract that makes `Fast` safe:

~~~rust
// stockroom/src/db/pool.rs (excerpt)
/// `RecyclingMethod::Fast` hands a returned connection to the next caller after checking only
/// that it is not closed. That is right for this service because no handler leaves session
/// state behind: no `SET` (only `SET LOCAL`, which ends with its transaction), no `LISTEN`
/// (the listener has its own connection), and no temporary table that outlives a
/// transaction. A service that cannot promise that wants `RecyclingMethod::Clean`.
~~~

Handlers get a connection through an extractor, so a handler's signature says that it uses the
database, and the connection returns to the pool when the handler's future ends:

~~~rust
// stockroom/src/http/mod.rs (excerpt)
pub struct Db(pub Object);

impl FromRequestParts<AppState> for Db {
    type Rejection = AppError;

    async fn from_request_parts(
        _parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        Ok(Db(state.pool.get().await?))
    }
}
~~~

[stockroom/src/http/mod.rs](../stockroom/src/http/mod.rs)

A pool that cannot lend a connection in time is an overloaded service, and `AppError` says so
with a 503, not a 500:

~~~rust
// stockroom/src/error.rs (excerpt)
impl From<PoolError> for AppError {
    fn from(error: PoolError) -> Self {
        match error {
            PoolError::Backend(e) => e.into(),
            PoolError::Timeout(_) => {
                AppError::Unavailable("no database connection became free in time".into())
            }
            other => AppError::Unavailable(other.to_string()),
        }
    }
}
~~~

[stockroom/src/error.rs](../stockroom/src/error.rs)

Chapter 15 builds the rest of the HTTP layer on these pieces. `GET /readyz` reports the pool's
`status()` (Chapter 16), and the size is a flag, `--pool-size`, because it belongs to the
deployment, not the code.

## Summary

- One shared `Client` cannot run transactions (`&mut self`), suffers head-of-line blocking, and
  fails as a whole. A pool gives each borrower its own session.
- A pool is a semaphore (how many), a stack of idle clients (which one), a health check (is it
  alive) and a guard that returns the client on drop, in the right order.
- deadpool-postgres: `Manager::from_config(config, tls, ManagerConfig { recycling_method })`,
  then `Pool::builder(...).max_size(n).runtime(Runtime::Tokio1)` with wait, create and recycle
  timeouts. `Fast` checks `is_closed()` only, `Verified` adds a round trip, `Clean` also resets
  session state.
- `Object` derefs to `tokio_postgres::Client`; it adds `prepare_cached`, its own `transaction`,
  and a `GenericClient` with `prepare_cached`. `Object::take` detaches a connection.
- Measured: over a 2 ms delay, one pipelined client ran independent queries 8× faster than a
  pool of 8; transactions need a pool, and their throughput is bounded by round trips per
  transaction. Size pools for concurrent transactions, within `max_connections`.
- Never leave session state on a pooled connection: no `LISTEN`, no unread streams, no session
  `SET`, no session locks or temp tables. PgBouncer's transaction mode needs the same discipline;
  `query_typed` works behind any pooler.

## Exercises

1. **Leak detection.** Write a `Custom` recycling query that makes recycling *fail* if the
   returned connection has its `application_name` changed, so that such a connection is
   discarded instead of reused, and test it with a pool of one.
   *Hint:* recycling fails when the query errors; `DO $$ BEGIN IF … THEN RAISE EXCEPTION …; END
   IF; END $$` raises one, and `current_setting('application_name')` reads the setting.
2. **Measure `Verified`.** Time 1,000 checkouts through lagproxy with a 1 ms delay, from
   §13.3's `pool_of_one` built once with `Fast` and once with `Verified`. Predict the difference
   first.
   *Hint:* one round trip per checkout.
3. **Too small.** stockroom's pool has 5 seconds of `wait_timeout`. With `--pool-size 2`, run
   ten concurrent `GET /reports/sales?from=2026-01-01&to=2026-12-31&delayMs=3000` requests. What do the clients see, and in
   what order?
   *Hint:* each request holds its connection for three seconds.
4. **minipool, verified.** minipool's `Options::verify` calls `check_connection()` on every
   idle connection it reuses. Repeat §13.3's terminate experiment against minipool with `verify` on and off.
   *Hint:* minipool is a library; add it to the playground with a path dependency.

### Solutions

**1.** deadpool runs a `Custom` query with `simple_query` when it reuses a connection; an
error fails the recycling, and the pool drops that connection and opens another:

```rust
use deadpool_postgres::{Manager, ManagerConfig, Pool, RecyclingMethod, Runtime};
use tokio_postgres::{Config, NoTls};

const CHECK: &str = "DO $$
BEGIN
    IF current_setting('application_name') <> '' THEN
        RAISE EXCEPTION 'session state left behind';
    END IF;
END $$";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config: Config = std::env::var("DATABASE_URL")?.parse()?;
    let manager = Manager::from_config(
        config,
        NoTls,
        ManagerConfig {
            recycling_method: RecyclingMethod::Custom(CHECK.to_owned()),
        },
    );
    let pool = Pool::builder(manager)
        .max_size(1)
        .runtime(Runtime::Tokio1)
        .build()?;

    let leaky: i32 = {
        let conn = pool.get().await?;
        conn.batch_execute("SET application_name = 'left behind'")
            .await?;
        conn.query_one("SELECT pg_backend_pid()", &[]).await?.get(0)
    };
    let conn = pool.get().await?;
    let row = conn
        .query_one(
            "SELECT pg_backend_pid(), current_setting('application_name')",
            &[],
        )
        .await?;
    let (pid, name): (i32, String) = (row.get(0), row.get(1));
    assert_ne!(pid, leaky, "the leaky connection was discarded");
    assert_eq!(name, "");
    Ok(())
}
```

**2.** Each `Verified` checkout costs one more round trip: twice the delay plus lagproxy's
timer overhead, roughly 5 to 8 ms at a 1 ms delay on the machine of Chapter 4. So 1,000
checkouts take several seconds longer than with `Fast`, whose checkout makes no round trip at
all.

**3.** Two requests get a connection at once and finish after about three seconds; two of the
waiting requests then get the freed connections. The remaining six are still waiting when
`wait_timeout` expires at the five-second mark, and receive 503 ("no database connection became
free in time") without ever running their query. The second pair finishes last, after about six
seconds.

**4.** With `verify` off, minipool behaves like `Fast`: the killed connection is lent, and the
first query fails. With `verify` on, `check_connection()` fails on the dead connection, minipool
drops it and opens a new one, and the first query succeeds.

---

Previous: [Chapter 12 — Production connections: timeouts, cancellation, health and TLS](12-production-connections.md) ·
Next: [Chapter 14 — Schema, migrations and the data layer](14-data-layer.md)
