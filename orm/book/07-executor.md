# Chapter 7 — The executor and shelf's errors

> **Building an ORM on tokio-postgres** · Previous: [Chapter 6 — Tables: the model derive](06-tables.md) · Next: [Chapter 8 — Writing rows: insert, update, delete, upsert](08-writing.md)

Chapter 1's fetch helpers took tokio-postgres's `GenericClient`, which a `Client` and a
`Transaction` both implement. A service holds neither: it holds a pooled deadpool connection,
which reaches a `Client` only through two dereferences, and loses on the way the one thing a
model layer wants most from it, the connection's statement cache. deadpool has a `GenericClient`
of its own, with that cache, but it covers deadpool's two types and not tokio-postgres's. Both
traits are sealed, so shelf can implement neither for anything, and code generic over one does
not accept the other.

The way out is a trait of shelf's own, with only the calls shelf needs, implemented for the four
types a program actually has. Choosing what goes in it means choosing how each statement reaches
the server, which is a question of round trips the main book taught you to measure. This chapter
writes the trait, measures three ways to send a statement through lagproxy, runs shelf inside
transactions and the main book's retry helper, and gives shelf an error type that tells an
application what it can do about each failure.

**In this chapter you will learn:**

- why neither `GenericClient` will do, and what a trait of shelf's own must contain
- how to write async methods in a trait whose futures are `Send`, and why that matters to
  `tokio::spawn` and axum
- three ways to send a statement (`prepare`, `prepare_cached`, `query_typed`) and their round
  trips, measured
- how shelf calls run inside a transaction, and inside the main book's retry helper
- `shelf::Error`, and how an application maps it to its own errors and HTTP statuses

---

## 7.1 Why not `GenericClient`

Two traits already abstract over "something that can run a statement":

| Trait | Implemented for | `prepare_cached` | Can you implement it? |
|---|---|---|---|
| `tokio_postgres::GenericClient` (0.7.18) | `Client`, `Transaction<'_>` | no | no: sealed |
| `deadpool_postgres::GenericClient` (0.14.2) | `deadpool_postgres::Client` (a pooled `Object`), deadpool's `Transaction<'_>` | yes | no: sealed |

Code written against the first runs inside or outside a transaction
([main book §8.5](../../book/08-transactions.md#85-genericclient-code-that-runs-inside-or-outside-a-transaction)).
A pooled connection derefs, in two steps, to a `Client`, so `&**object` passes too, and every
statement is then prepared on each call as if there were no cache. Code written against the
second uses the cache, and refuses a plain `Client`. Neither can be extended to cover the other,
because both are *sealed*: each has a supertrait that lives in a private module of its crate, so
no other crate can implement it. Trying it on a wrapper of your own:

~~~rust
use tokio_postgres::{Client, GenericClient};

/// A client that logs what it sends.
struct Logged(Client);

impl GenericClient for Logged {}
~~~

```text
error[E0277]: the trait bound `Logged: tokio_postgres::generic_client::private::Sealed` is not satisfied
   --> user/src/bin/sealed.rs:6:24
    |
  6 | impl GenericClient for Logged {}
    |                        ^^^^^^ unsatisfied trait bound
    |
help: the trait `tokio_postgres::generic_client::private::Sealed` is not implemented for `Logged`
   --> user/src/bin/sealed.rs:4:1
    |
  4 | struct Logged(Client);
    | ^^^^^^^^^^^^^
help: the following other types implement trait `tokio_postgres::generic_client::private::Sealed`
   --> /Users/luciano/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tokio-postgres-0.7.18/src/generic_client.rs:116:1
    |
116 | impl private::Sealed for Client {}
    | ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ `Client`
...
246 | impl private::Sealed for Transaction<'_> {}
    | ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ `Transaction<'_>`
note: required by a bound in `GenericClient`
   --> /Users/luciano/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tokio-postgres-0.7.18/src/generic_client.rs:14:26
    |
 14 | pub trait GenericClient: private::Sealed {
    |                          ^^^^^^^^^^^^^^^ required by this bound in `GenericClient`
    = note: `GenericClient` is a "sealed trait", because to implement it you also need to implement `tokio_postgres::generic_client::private::Sealed`, which is not accessible; this is usually done to force you to use one of the provided types that already implement it
    = help: the following types implement the trait:
              tokio_postgres::Client
              tokio_postgres::Transaction<'_>
```

(The same build also lists, as E0046, the seventeen methods the impl is missing, from `execute`
to `client`.) Sealing is a deliberate choice, and a reasonable one: a crate can add methods to a
sealed trait in a minor release without breaking anyone, and tokio-postgres did, adding
`query_typed` in 0.7.11 and `execute_typed`, `query_typed_one` and `query_typed_opt` in 0.7.17.
The price is paid by crates like shelf, which need one trait for all four types.

So shelf defines its own. It needs few methods: something that prepares (from a cache when there
is one), and the four ways to run a prepared statement it uses. Everything else a connection
can do (`batch_execute`, `simple_query`, starting a transaction, `LISTEN`) stays on the
connection, where the program already has it.

## 7.2 The `Executor` trait

The interesting part of the trait is not which methods it has but how they are declared. Here
is a stage version with two of shelf's methods, implemented for a plain `Client` and for a
pooled connection, and called by a generic repository function on tasks of its own:

```rust
use std::future::Future;
use std::sync::Arc;
use tokio_postgres::types::ToSql;
use tokio_postgres::{Error, Row, Statement};

/// The stage executor: two of shelf's methods.
trait Executor: Sync {
    fn prepare(&self, sql: &str) -> impl Future<Output = Result<Statement, Error>> + Send;

    fn query(
        &self,
        statement: &Statement,
        params: &[&(dyn ToSql + Sync)],
    ) -> impl Future<Output = Result<Vec<Row>, Error>> + Send;
}

impl Executor for tokio_postgres::Client {
    fn prepare(&self, sql: &str) -> impl Future<Output = Result<Statement, Error>> + Send {
        tokio_postgres::Client::prepare(self, sql) // every time
    }

    fn query(
        &self,
        statement: &Statement,
        params: &[&(dyn ToSql + Sync)],
    ) -> impl Future<Output = Result<Vec<Row>, Error>> + Send {
        tokio_postgres::Client::query(self, statement, params)
    }
}

impl Executor for deadpool_postgres::Client {
    fn prepare(&self, sql: &str) -> impl Future<Output = Result<Statement, Error>> + Send {
        self.prepare_cached(sql) // from the connection's cache after the first time
    }

    fn query(
        &self,
        statement: &Statement,
        params: &[&(dyn ToSql + Sync)],
    ) -> impl Future<Output = Result<Vec<Row>, Error>> + Send {
        tokio_postgres::Client::query(self, statement, params) // `self` derefs to a Client
    }
}

/// A repository function, generic over the executor.
async fn count(db: &impl Executor) -> Result<i64, Error> {
    let statement = db
        .prepare("SELECT count(*) FROM generate_series(1, 3)")
        .await?;
    db.query(&statement, &[]).await?[0].try_get(0)
}

/// Runs `count` on a task of its own: its future must be `Send` for every `E`.
fn spawn_count<E: Executor + Send + 'static>(
    db: Arc<E>,
) -> tokio::task::JoinHandle<Result<i64, Error>> {
    tokio::spawn(async move { count(&*db).await })
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = Arc::new(shelf_playground::connect().await?);
    assert_eq!(spawn_count(client).await??, 3);

    let pool = shelf_playground::pool(1);
    let object = Arc::new(pool.get().await?);
    for _ in 0..3 {
        assert_eq!(spawn_count(object.clone()).await??, 3);
    }
    assert_eq!(object.statement_cache.size(), 1, "prepared once");
    Ok(())
}
```

`spawn_count` is the case that matters. An axum handler is the same case: axum requires a
handler's future to be `Send`, so that the runtime can move it between threads, and a repository
function generic over its executor is only as `Send` as the futures its executor returns. The
trait says they are, in so many words: `-> impl Future<Output = …> + Send`. Written the natural
way instead, as `async fn prepare(&self, sql: &str) -> Result<Statement, Error>`, the trait makes
no such promise, and the same `spawn_count` fails to compile:

```text
error[E0277]: `impl Future<Output = Result<Statement, tokio_postgres::Error>>` cannot be sent between threads safely
   --> user/src/bin/async_fn.rs:29:5
    |
 29 |     tokio::spawn(async move { count(&*db).await });
    |     ^^^^^^^^^^^^^----------^^^^^^^^^^^^^^^^^^^^^^^
    |     |            |
    |     |            within this `{async block@user/src/bin/async_fn.rs:29:18: 29:28}`
    |     `impl Future<Output = Result<Statement, tokio_postgres::Error>>` cannot be sent between threads safely
    |
    = help: within `{async block@user/src/bin/async_fn.rs:29:18: 29:28}`, the trait `Send` is not implemented for `impl Future<Output = Result<Statement, tokio_postgres::Error>>`
note: required because it's used within this `async` fn body
   --> user/src/bin/async_fn.rs:22:58
    |
 22 |   async fn count(db: &impl Executor) -> Result<i64, Error> {
    |  __________________________________________________________^
 23 | |     let statement = db.prepare("SELECT count(*) FROM products").await?;
 24 | |     Ok(db.query(&statement, &[]).await?[0].get(0))
 25 | | }
    | |_^
note: required because it's used within this `async` block
   --> user/src/bin/async_fn.rs:29:18
    |
 29 |     tokio::spawn(async move { count(&*db).await });
    |                  ^^^^^^^^^^
note: required by a bound in `tokio::spawn`
   --> /Users/luciano/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tokio-1.54.1/src/task/spawn.rs:176:21
    |
174 |     pub fn spawn<F>(future: F) -> JoinHandle<F::Output>
    |            ----- required by a bound in this function
175 |     where
176 |         F: Future + Send + 'static,
    |                     ^^^^ required by this bound in `spawn`
help: `Send` can be made part of the associated future's guarantees for all implementations of `Executor::prepare`
    |
  6 -     async fn prepare(&self, sql: &str) -> Result<Statement, Error>;
  6 +     fn prepare(&self, sql: &str) -> impl std::future::Future<Output = Result<Statement, Error>> + Send;
    |
```

(A second, identical error follows for `query`.) rustc's own help proposes the form shelf uses.

> **Rust refresher:** `async fn` in a trait (stable since Rust 1.75) is shorthand for a method
> returning `impl Future<Output = …>`, and that future's type is chosen by each impl. When the
> caller knows the concrete type (`count(&client)` with a `Client`), the compiler sees the
> impl's future and knows whether it is `Send`. When the caller is generic (`E: Executor`), it
> knows only what the trait declares, and an `async fn` declares nothing about `Send`. Writing
> the return type out, `-> impl Future<Output = …> + Send`, puts the promise in the trait: every
> impl must return a `Send` future, and every generic caller may rely on it. An impl may still
> be written as an `async fn` (Exercise 2's is), or return another `Send` future, as these
> return tokio-postgres's; either way the compiler checks that impl's future against the
> trait's `Send`. Before Rust 1.75 the usual answer was `#[async_trait]`, which boxes every
> future; tokio-postgres's and deadpool's `GenericClient`s still use it.

shelf's trait is the stage's with the rest of its methods. `prepare_uncached` has a default and
exists for Chapter 13's `verify`; `query_raw` and `copy_in` serve Chapter 12's streams and COPY:

~~~rust
// orm/shelf/src/executor.rs (excerpt)
pub trait Executor: Sync {
    /// A prepared statement for `sql`, from the cache when the executor has one.
    fn prepare(&self, sql: &str) -> impl Future<Output = Result<Statement, Error>> + Send;

    /// A prepared statement for `sql`, never from a cache: the server's description now.
    /// [`verify`](crate::verify) needs it, since a cached statement describes a table as it was
    /// when it was prepared. By default, [`prepare`](Executor::prepare).
    fn prepare_uncached(&self, sql: &str) -> impl Future<Output = Result<Statement, Error>> + Send {
        self.prepare(sql)
    }

    /// Every row `statement` returns.
    fn query(
        &self,
        statement: &Statement,
        params: &[&(dyn ToSql + Sync)],
    ) -> impl Future<Output = Result<Vec<Row>, Error>> + Send;

    // … query_one, query_opt, execute: the same shape

    /// Starts `statement` and returns its rows as they arrive, for streams. The `RowStream` does
    /// not borrow the connection, which serves no one else until the stream is finished or
    /// dropped.
    fn query_raw(
        &self,
        statement: &Statement,
        params: &[&(dyn ToSql + Sync)],
    ) -> impl Future<Output = Result<RowStream, Error>> + Send;

    // … copy_in (Chapter 12)
}
~~~

[orm/shelf/src/executor.rs](../shelf/src/executor.rs)

The four impls are one `macro_rules!` macro, Chapter 2's tool for impls that differ in a few
tokens. Each forwards to the target type's own method, except `prepare`, which is the argument
that differs:

~~~rust
// orm/shelf/src/executor.rs (excerpt)
/// Implements [`Executor`] for `$ty` by calling `$target`'s methods (which `$ty` derefs to),
/// with `$prepare` for [`Executor::prepare`].
macro_rules! executor {
    ($ty:ty, $target:ty, $prepare:ident) => {
        impl Executor for $ty {
            fn prepare(&self, sql: &str) -> impl Future<Output = Result<Statement, Error>> + Send {
                self.$prepare(sql)
            }

            fn prepare_uncached(
                &self,
                sql: &str,
            ) -> impl Future<Output = Result<Statement, Error>> + Send {
                <$target>::prepare(self, sql)
            }

            fn query(
                &self,
                statement: &Statement,
                params: &[&(dyn ToSql + Sync)],
            ) -> impl Future<Output = Result<Vec<Row>, Error>> + Send {
                <$target>::query(self, statement, params)
            }

            // …
        }
    };
}

executor!(tokio_postgres::Client, tokio_postgres::Client, prepare);
executor!(
    tokio_postgres::Transaction<'_>,
    tokio_postgres::Transaction<'_>,
    prepare
);
#[cfg(feature = "deadpool")]
executor!(
    deadpool_postgres::Client,
    tokio_postgres::Client,
    prepare_cached
);
#[cfg(feature = "deadpool")]
executor!(
    deadpool_postgres::Transaction<'_>,
    tokio_postgres::Transaction<'_>,
    prepare_cached
);
~~~

[orm/shelf/src/executor.rs](../shelf/src/executor.rs)

The deadpool impls sit behind the default feature `deadpool`, so a program without a pool
does not compile deadpool. `prepare_uncached` names the target type's `prepare` explicitly,
`<$target>::prepare(self, sql)`: on a deadpool connection, `self.prepare(sql)` would find the
trait's own `prepare` first and use the cache.

### `&object`, not `&**object`

A pooled connection is three types deep. `deadpool_postgres::Client` is deadpool's `Object`,
which derefs to deadpool's `ClientWrapper` (where the statement cache lives), which derefs to
tokio-postgres's `Client`. shelf implements `Executor` for the outermost and the innermost:

| You pass | Type | What shelf does |
|---|---|---|
| `&object` | `&deadpool_postgres::Client` | prepares through the connection's cache |
| `&*object` | `&ClientWrapper` | does not compile: no impl |
| `&**object` | `&tokio_postgres::Client` | prepares on every call, as if there were no pool |

The middle one is refused, with the four types that do implement the trait listed in the error:

```text
error[E0277]: the trait bound `ClientWrapper: Executor` is not satisfied
   --> user/src/bin/deref.rs:5:32
    |
  5 |     shelf::fetch_all::<(i64,)>(&*object, "SELECT 1::int8", &[]).await?;
    |     -------------------------- ^^^^^^^^ the trait `Executor` is not implemented for `ClientWrapper`
    |     |
    |     required by a bound introduced by this call
```

The last one compiles and works, which is the trap: nothing reports the lost cache. Chapter 1's
helpers needed `&**object`, because they took tokio-postgres's `GenericClient`; with shelf, pass
the pooled connection itself.

The trait is not sealed. A wrapper that counts or times what passes through it implements it in a
few lines per method (Exercise 1), and Chapter 11 counts round trips with one.

## 7.3 Three ways to send a statement

A statement reaches PostgreSQL in one of three ways, and the difference is round trips
([main book §4.1](../../book/04-prepared-statements.md#41-what-query-sends) to §4.6):

- **Prepare, then run.** *Parse* and *Describe*, then wait for the description; then *Bind* and
  *Execute*. Two round trips on every call. It is also what `client.query(sql_text, …)` costs,
  since tokio-postgres prepares SQL text the same way. The `Statement` is closed when it is
  dropped.
- **Prepare once per connection, from a cache.** deadpool's `prepare_cached` prepares on the first
  call and keeps the `Statement`; every later call on that connection is one round trip. The
  statement stays open on the session for as long as the connection lives.
- **`query_typed`** (tokio-postgres 0.7.11+). *Parse*, *Bind* and *Execute* in one go, with an
  unnamed statement: one round trip, nothing left behind. The price is that the caller must give
  each parameter's PostgreSQL `Type`, and that no `Statement` exists before the query runs.

This program times each through lagproxy, which adds 5 ms each way, and counts the statements
each leaves open on the session. All three use the same connection, so they read the same
temporary table: `&**object` is the plain `Client` inside the pooled one.

<!-- check: needs=lagproxy -->
```rust
use shelf::{FromRow, Table};
use std::time::{Duration, Instant};
use tokio_postgres::types::Type;

#[derive(Debug, FromRow, Table)]
#[shelf(table = "products")]
struct Product {
    #[shelf(primary_key)]
    id: i64,
    sku: String,
}

const CALLS: u32 = 20;

/// The named statements open on this session. `query_typed` sends an unnamed one, which the
/// view does not list, so the count does not count itself.
async fn open_statements(client: &tokio_postgres::Client) -> Result<i64, tokio_postgres::Error> {
    client
        .query_typed_one("SELECT count(*) FROM pg_prepared_statements", &[])
        .await?
        .try_get(0)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Through lagproxy: every round trip costs at least twice the delay.
    let delay: u64 = std::env::var("LAGPROXY_DELAY_MS").map_or(5, |v| v.parse().unwrap());
    let pool = shelf_playground::pool(1);
    let object = pool.get().await?;
    let client: &tokio_postgres::Client = &object; // the same session, without the cache
    client
        .batch_execute(
            "CREATE TEMP TABLE products (id bigint PRIMARY KEY, sku text NOT NULL);
             INSERT INTO products VALUES (1, 'MUG-01');",
        )
        .await?;

    // 1. A plain Client: prepare, then run, on every call.
    let started = Instant::now();
    for _ in 0..CALLS {
        Product::find(client, &1).await?;
    }
    let prepared = started.elapsed() / CALLS;
    let open_after_prepared = open_statements(client).await?;

    // 2. The pooled connection: prepared on the first call, from the cache after.
    let started = Instant::now();
    Product::find(&object, &1).await?;
    let first_cached = started.elapsed();
    let started = Instant::now();
    for _ in 0..CALLS {
        Product::find(&object, &1).await?;
    }
    let cached = started.elapsed() / CALLS;
    let open_after_cached = open_statements(client).await?;

    // 3. query_typed: one round trip, with the parameter's type stated by hand.
    let started = Instant::now();
    for _ in 0..CALLS {
        let row = client
            .query_typed_one(Product::FIND, &[(&1_i64, Type::INT8)])
            .await?;
        Product::from_row(&row)?;
    }
    let typed = started.elapsed() / CALLS;

    println!("prepare, then run   {prepared:>8.1?} a call, {open_after_prepared} left open");
    println!(
        "prepare_cached      {cached:>8.1?} a call (the first {first_cached:.1?}), {open_after_cached} left open"
    );
    println!("query_typed         {typed:>8.1?} a call");

    let round_trip = Duration::from_millis(2 * delay);
    assert!(cached >= round_trip, "at least one round trip");
    assert!(prepared > cached * 3 / 2, "two round trips against one");
    assert!(prepared > typed * 3 / 2, "two round trips against one");
    assert!(first_cached > cached * 3 / 2, "the first call prepares");
    assert_eq!(
        open_after_prepared, 0,
        "each Statement is closed when dropped"
    );
    assert_eq!(open_after_cached, 1, "the cache keeps one");
    Ok(())
}
```

```text
prepare, then run     27.9ms a call, 0 left open
prepare_cached        14.2ms a call (the first 27.2ms), 1 left open
query_typed           13.9ms a call
```

Three runs in a debug build and three in a release build (20 calls each, lagproxy at 5 ms each
way, PostgreSQL 18.6 on the same machine, Apple Silicon) gave the same picture, because the time
is the network's:

| Strategy | Round trips | Per call, debug | Per call, release | Left open on the session |
|---|---|---|---|---|
| prepare, then run (a plain `Client`) | 2 | 27.0–27.9 ms | 27.5–27.6 ms | none |
| `prepare_cached`, first call | 2 | 27.2–28.8 ms | 26.4–28.0 ms | one statement |
| `prepare_cached`, after | 1 | 13.7–14.2 ms | 13.6 ms | one statement |
| `query_typed` | 1 | 13.6–13.9 ms | 13.7–13.9 ms | none |

Prepare-then-run costs two round trips, a cached statement and `query_typed` one each, and a
cached statement's first call costs two. Each `Statement` that `prepare` returns is closed when it
is dropped, at the end of the call, so a plain `Client` leaves nothing behind; the cache keeps one
statement per distinct SQL text, per connection, for the connection's life.

`query_typed` is as fast as the cache without a cache. shelf still does not use it, for two
reasons. It needs each parameter's PostgreSQL `Type`, and a Rust type does not say which: an
`i64` may be bound to an `int8` column, a `String` to `text`, `varchar` or a domain over either,
and only the server knows which the statement needs. And it gives no `Statement` before the
query runs, so Chapter 5's check, which resolves the struct against the statement's columns
before anything executes, would come after the fact: an `INSERT … RETURNING` that does not fit
its struct would already have inserted. So each executor gets the strategy that keeps the
check: a plain `Client` or `Transaction` prepares on every call, which costs nothing that
`query` with SQL text would not; a deadpool connection or transaction uses `prepare_cached`.
For a service, that means one round trip per call after warm-up.

> **Protocol lens:** in tokio-postgres 0.7.18, `prepare` sends *Parse* (with a name from a
> process-wide counter: `s0`, `s1` and so on), *Describe* of the statement and *Sync*, and waits
> for *ParseComplete*, *ParameterDescription* and *RowDescription*: the column list `resolve`
> reads. Running it sends *Bind*, *Execute* and *Sync*. Dropping the last clone of a
> `Statement` queues *Close* and *Sync* for its name on the connection's request channel, so
> they go out before the next request. `query_typed` writes *Parse* with the empty name and the
> types' OIDs, *Bind*, *Describe* of that unnamed statement, *Execute* and *Sync* at once, and
> the server answers all of it in one go; an unnamed statement lasts until the next *Parse*,
> needs no *Close*, and `pg_prepared_statements` does not list it.

## 7.4 Transactions

`Transaction` is an executor too, so every shelf call that takes `&impl Executor` runs inside a
transaction when it is given one. It then sees the transaction's own writes, and nobody else
does until the commit:

```rust
use shelf::{FromRow, Table};

#[derive(Debug, FromRow, Table)]
#[shelf(table = "products", schema = "shelf_ch07_tx")]
struct Product {
    #[shelf(primary_key)]
    id: i64,
    sku: String,
}

#[tokio::main]
async fn main() -> Result<(), shelf::Error> {
    let mut client = shelf_playground::connect().await?;
    let other = shelf_playground::connect().await?;
    // A schema both connections see, dropped at the end.
    client
        .batch_execute(
            "DROP SCHEMA IF EXISTS shelf_ch07_tx CASCADE;
             CREATE SCHEMA shelf_ch07_tx;
             CREATE TABLE shelf_ch07_tx.products (id bigint PRIMARY KEY, sku text NOT NULL);",
        )
        .await?;

    let tx = client.transaction().await?;
    tx.execute(
        "INSERT INTO shelf_ch07_tx.products VALUES (1, 'MUG-01')",
        &[],
    )
    .await?;
    // Inside the transaction, shelf sees the new row; outside, it does not exist yet.
    assert_eq!(Product::find(&tx, &1).await?.sku, "MUG-01");
    assert!(Product::find_opt(&other, &1).await?.is_none());
    tx.commit().await?;
    assert_eq!(Product::count(&other).await?, 1);

    client
        .batch_execute("DROP SCHEMA shelf_ch07_tx CASCADE")
        .await?;
    Ok(())
}
```

The `Executor` has no method that starts a transaction. `Client::transaction` takes `&mut self`
([main book §8.2](../../book/08-transactions.md#82-why-transaction-takes-mut-self)), and an
executor is a shared borrow, which is what lets several shelf calls run on one connection at
once (Exercise 3). So the program starts the transaction on the connection it owns, and hands
`&tx` to everything inside.

### The retry helper

At `SERIALIZABLE`, a transaction can fail because of another one, with SQLSTATE 40001, and the
cure is to run it again from `BEGIN`
([main book §8.6](../../book/08-transactions.md#86-retrying-transactions-that-lose-a-race)). The
main book wrote a helper that does that, taking the transaction's body as an async closure. It
works with shelf calls inside, with one change: the body now returns `shelf::Error`, so the
helper finds the SQLSTATE inside `Error::Db`. This is the main book's two doctors, of whom one
must stay on call, with the reads done by a model:

```rust
use shelf::{FromRow, Table};
use std::time::Duration;
use tokio_postgres::error::SqlState;
use tokio_postgres::{Client, IsolationLevel, Transaction};

#[derive(Debug, FromRow, Table)]
#[shelf(table = "doctors", schema = "shelf_ch07_retry")]
struct Doctor {
    #[shelf(primary_key)]
    name: String,
    on_call: bool,
}

/// Runs `work` in a SERIALIZABLE transaction, again from the start after a serialization
/// failure or a deadlock, up to 10 times: the main book's helper, with shelf's error.
async fn serializable<T>(
    client: &mut Client,
    mut work: impl AsyncFnMut(&Transaction<'_>) -> Result<T, shelf::Error>,
) -> Result<T, shelf::Error> {
    let mut attempt: u32 = 1;
    loop {
        let tx = client
            .build_transaction()
            .isolation_level(IsolationLevel::Serializable)
            .start()
            .await?;
        let outcome = match work(&tx).await {
            Ok(value) => tx
                .commit()
                .await
                .map(|()| value)
                .map_err(shelf::Error::from),
            Err(error) => Err(error), // `tx` is dropped here: rolled back
        };
        match outcome {
            Err(error) if attempt < 10 && retryable(&error) => {
                tokio::time::sleep(Duration::from_millis(5 * u64::from(attempt))).await;
                attempt += 1;
            }
            other => return other,
        }
    }
}

/// A serialization failure or a deadlock: shelf passes the database's errors on as `Db`.
fn retryable(error: &shelf::Error) -> bool {
    let shelf::Error::Db(error) = error else {
        return false;
    };
    matches!(
        error.code(),
        Some(&SqlState::T_R_SERIALIZATION_FAILURE | &SqlState::T_R_DEADLOCK_DETECTED)
    )
}

#[tokio::main]
async fn main() -> Result<(), shelf::Error> {
    let setup = shelf_playground::connect().await?;
    setup
        .batch_execute(
            "DROP SCHEMA IF EXISTS shelf_ch07_retry CASCADE; CREATE SCHEMA shelf_ch07_retry;
             CREATE TABLE shelf_ch07_retry.doctors (name text PRIMARY KEY, on_call bool NOT NULL);
             INSERT INTO shelf_ch07_retry.doctors VALUES ('alice', true), ('bob', true);",
        )
        .await?;

    // Both doctors try to leave at once, each on a task of its own.
    let mut tasks = Vec::new();
    for name in ["alice", "bob"] {
        let name = name.to_owned(); // owned: the closure must own what it uses
        tasks.push(tokio::spawn(async move {
            let mut client = shelf_playground::connect().await?;
            serializable(&mut client, async move |tx| {
                let on_call = Doctor::all(tx).await?.iter().filter(|d| d.on_call).count();
                if on_call < 2 {
                    return Ok(false); // someone must stay
                }
                tokio::time::sleep(Duration::from_millis(20)).await; // a slow decision
                tx.execute(
                    "UPDATE shelf_ch07_retry.doctors SET on_call = false WHERE name = $1",
                    &[&name],
                )
                .await?;
                Ok(true)
            })
            .await
        }));
    }
    let mut left = 0;
    for task in tasks {
        if task.await.expect("no panic")? {
            left += 1;
        }
    }
    assert_eq!(left, 1, "exactly one doctor left");
    let still = Doctor::all(&setup)
        .await?
        .iter()
        .filter(|d| d.on_call)
        .count();
    assert_eq!(still, 1);

    setup
        .batch_execute("DROP SCHEMA shelf_ch07_retry CASCADE")
        .await?;
    Ok(())
}
```

Two things made this compile. `Doctor::all` returns a `Send` future (§7.2), so the body can run
inside `tokio::spawn`. And the closure owns `name`, so each attempt's future borrows from the
closure rather than from the task; a closure that borrowed the task's locals would fail with
"implementation of `Send` is not general enough", as the main book's §8.6 explains. The `?` on
`tx.execute` works because `shelf::Error` has `From<tokio_postgres::Error>`, the `Db` variant.

stockroom's own helper, `db::in_transaction`, takes a closure that returns stockroom's
`AppError`, which classifies SQLSTATEs itself. The capstone calls it unchanged, with a body that
calls shelf and converts shelf's errors (§7.5):

~~~rust
// orm/stockroom-shelf/src/http/orders.rs (excerpt)
pub async fn place(
    Db(mut db): Db,
    AppJson(order): AppJson<NewOrder>,
) -> Result<impl IntoResponse, AppError> {
    // `async move`: the closure owns the request, and each attempt borrows it from there.
    let order = db::in_transaction(&mut db, IsolationLevel::ReadCommitted, async move |tx| {
        orders::place(tx, &order).await
    })
    .await?;
    Ok((StatusCode::CREATED, etag(order.version), Json(order)))
}
~~~

[orm/stockroom-shelf/src/http/orders.rs](../stockroom-shelf/src/http/orders.rs)

There `tx` is deadpool's `Transaction`, which shares its connection's statement cache: a
statement prepared through the connection is found again inside the transaction, and the other
way round.

## 7.5 `shelf::Error`

Everything shelf returns fails with one type. Its variants say what went wrong in terms a caller
can act on:

| Variant | From | What it means | What a caller does |
|---|---|---|---|
| `Mapping { model, field, problem }` | reading (Chapters 4, 5) | the struct does not fit the result | fix the code: a bug, not a request's fault |
| `NotFound { model }` | `find`, writes by key (Chapter 8) | no row has the key | report it: 404 |
| `Stale { model, current_version }` | a versioned update (Chapter 9) | someone changed the row since it was read | report it with the version: 412, or read again |
| `Schema { model, mismatches }` | `verify` (Chapter 13) | the model and its table disagree | refuse to start |
| `Unsupported { model, message }` | the bulk writes (Chapter 12) | a call shelf cannot make for this model | use the other method the message names |
| `Db(tokio_postgres::Error)` | everything else | the database or the connection | look inside: a SQLSTATE and a constraint name |

The enum is `#[non_exhaustive]`, so a `match` on it needs a `_` arm, and shelf can add a variant
in a minor release. Its `Display` is meant for logs, not for API clients; the `Db` variant is
`#[error(transparent)]`, so it displays and sources as the driver's error does, and
`source()` leads to the server's `DbError`
([main book §6.2](../../book/06-errors.md#62-dberror-the-servers-report)).

What shelf does not do is decide what a constraint violation means. A duplicate SKU is a 409 in
one service and a validation message in another, and the knowledge of which constraint guards
what lives in the application, keyed by constraint name, as stockroom's `AppError` does
([main book §6.5](../../book/06-errors.md#65-an-error-type-for-an-application)). An application
error type with a `From<shelf::Error>` impl lets `?` do the conversion:

```rust
use shelf::{FromRow, Table};
use std::error::Error as _;
use tokio_postgres::error::SqlState;

#[derive(Debug, FromRow, Table)]
#[shelf(table = "products")]
struct Product {
    #[shelf(primary_key)]
    id: i64,
    sku: String,
    price_cents: i64,
}

/// What the application's API reports.
#[derive(Debug, thiserror::Error)]
enum AppError {
    #[error("{0} not found")]
    NotFound(&'static str),
    #[error("{0}")]
    Conflict(&'static str),
    #[error("{0}")]
    Invalid(&'static str),
    #[error("changed since it was read; it is now at version {0}")]
    Stale(i64),
    #[error("internal error")]
    Internal(#[source] shelf::Error),
}

impl AppError {
    fn status(&self) -> u16 {
        match self {
            AppError::NotFound(_) => 404,
            AppError::Conflict(_) => 409,
            AppError::Stale(_) => 412,
            AppError::Invalid(_) => 422,
            AppError::Internal(_) => 500,
        }
    }
}

/// Constraint names are a contract between the schema and this function.
fn constraint_message(constraint: &str) -> Option<&'static str> {
    match constraint {
        "products_sku_key" => Some("a product with this SKU already exists"),
        "products_price_cents_check" => Some("a price cannot be negative"),
        _ => None,
    }
}

impl From<shelf::Error> for AppError {
    fn from(error: shelf::Error) -> Self {
        match error {
            shelf::Error::NotFound { model } => AppError::NotFound(model),
            shelf::Error::Stale {
                current_version, ..
            } => AppError::Stale(current_version),
            shelf::Error::Db(db) => {
                let known = db.as_db_error().and_then(|server| {
                    let message = constraint_message(server.constraint()?)?;
                    Some((server.code().clone(), message))
                });
                match known {
                    Some((SqlState::UNIQUE_VIOLATION, message)) => AppError::Conflict(message),
                    Some((SqlState::CHECK_VIOLATION, message)) => AppError::Invalid(message),
                    _ => AppError::Internal(shelf::Error::Db(db)),
                }
            }
            // A mapping or schema error is a bug in this program, not in the request.
            other => AppError::Internal(other),
        }
    }
}

/// A repository function: shelf's errors become the application's at the `?`.
async fn create(
    db: &impl shelf::Executor,
    sku: &str,
    price_cents: i64,
) -> Result<Product, AppError> {
    let sql = "INSERT INTO products (sku, price_cents) VALUES ($1, $2)
               RETURNING id, sku, price_cents";
    Ok(shelf::fetch_one(db, sql, &[&sku, &price_cents]).await?)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = shelf_playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE products (
                 id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
                 sku text NOT NULL UNIQUE,
                 price_cents bigint NOT NULL CHECK (price_cents >= 0)
             );",
        )
        .await?;
    create(&client, "MUG-01", 1299).await?;

    let duplicate = create(&client, "MUG-01", 999).await.unwrap_err();
    let negative = create(&client, "TEE-02", -1).await.unwrap_err();
    let missing = AppError::from(Product::find(&client, &99).await.unwrap_err());
    // A struct that does not fit its query: the client hears 500, the log hears why.
    let mistake = AppError::from(
        shelf::fetch_one::<Product>(&client, "SELECT id, sku FROM products", &[])
            .await
            .unwrap_err(),
    );
    for error in [&duplicate, &negative, &missing, &mistake] {
        println!("{} {error}", error.status());
    }
    assert_eq!(
        [&duplicate, &negative, &missing, &mistake].map(AppError::status),
        [409, 422, 404, 500]
    );

    let mut cause = mistake.source();
    while let Some(error) = cause {
        println!("  caused by: {error}");
        cause = error.source();
    }
    Ok(())
}
```

```text
409 a product with this SKU already exists
422 a price cannot be negative
404 Product not found
500 internal error
  caused by: Product.price_cents: the result has no column "price_cents"
  caused by: the result has no column "price_cents"
```

The 500's message is "internal error" and nothing more, because an API client should not learn
what a struct is called; the chain under it is for the log. The `Db` arm keeps the original
error when it does not recognise the constraint, so the log still has the SQLSTATE.

### In stockroom-shelf: the orphan rule

The capstone cannot write that `From` impl. Its `AppError` is stockroom's, unchanged, and
`shelf::Error` is shelf's: an impl of a foreign trait (`From`) for a foreign type (`AppError`)
in a third crate breaks the orphan rule, and rustc refuses it with E0117 (PLAN.md §12, finding
49). So the capstone converts with a function, at every call, with `map_err`:

~~~rust
// orm/stockroom-shelf/src/error.rs (excerpt)
/// What a client sees when shelf fails: a missing row is 404, a stale version 412, a database
/// error whatever stockroom makes of it (its SQLSTATE and constraint names), and a mapping or
/// schema error, which is a bug, 500.
pub fn app_error(error: shelf::Error) -> AppError {
    match error {
        shelf::Error::NotFound { model } => AppError::NotFound(noun(model)),
        shelf::Error::Stale {
            current_version, ..
        } => match i32::try_from(current_version) {
            Ok(current) => AppError::StaleVersion { current },
            Err(_) => AppError::Internal(format!("version {current_version} is not an i32")),
        },
        shelf::Error::Db(error) => error.into(),
        other => AppError::Internal(other.to_string()),
    }
}
~~~

[orm/stockroom-shelf/src/error.rs](../stockroom-shelf/src/error.rs)

`shelf::Error::Db(error) => error.into()` hands the driver's error to stockroom's own
`From<tokio_postgres::Error>`, which maps SQLSTATEs and constraint names exactly as it does for
stockroom's hand-written code, so a duplicate SKU is the same 409 through either. In a crate that
owns its error type, prefer the `From` impl; in one that borrows it, the function is the price.

### Side by side — *one repository function over a client, a transaction and a pool*

The task: count the products on sale, in a function that runs on whatever the caller holds.

~~~rust
// sqlx (illustration)
async fn on_sale(db: impl PgExecutor<'_>) -> sqlx::Result<i64> {
    sqlx::query_scalar("SELECT count(*) FROM products WHERE on_sale")
        .fetch_one(db)
        .await
}

on_sale(&pool).await?;      // any connection of the pool, per query
on_sale(&mut *conn).await?; // a PoolConnection, dereferenced to the PgConnection
on_sale(&mut *tx).await?;   // a Transaction, dereferenced the same way
// The executor is taken by value: to run two queries on one, take `impl Acquire` instead.
~~~

By hand, generic over tokio-postgres's `GenericClient`, and with shelf, generic over its
`Executor`, in one program that calls each with all four types and checks what reaches
deadpool's statement cache:

```rust
use shelf::Executor;
use tokio_postgres::GenericClient;

const ON_SALE: &str =
    "SELECT count(*) FROM (VALUES (true), (false), (true)) AS products (on_sale) WHERE on_sale";

/// By hand: anything tokio-postgres's GenericClient covers.
async fn on_sale_by_hand(db: &impl GenericClient) -> Result<i64, tokio_postgres::Error> {
    db.query_one(ON_SALE, &[]).await?.try_get(0)
}

/// shelf: anything shelf's Executor covers.
async fn on_sale(db: &impl Executor) -> Result<i64, shelf::Error> {
    let (count,): (i64,) = shelf::fetch_one(db, ON_SALE, &[]).await?;
    Ok(count)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut client = shelf_playground::connect().await?;
    let pool = shelf_playground::pool(1);
    let mut object = pool.get().await?;

    // A plain Client and its Transaction: both functions take them.
    assert_eq!(on_sale_by_hand(&client).await?, 2);
    assert_eq!(on_sale(&client).await?, 2);
    let tx = client.transaction().await?;
    assert_eq!(on_sale_by_hand(&tx).await?, 2);
    assert_eq!(on_sale(&tx).await?, 2);
    tx.commit().await?;

    // A pooled connection: by hand needs the Client inside it, and skips the cache.
    assert_eq!(on_sale_by_hand(&**object).await?, 2);
    assert_eq!(object.statement_cache.size(), 0);
    // shelf takes the connection itself, and its transaction, through the cache.
    assert_eq!(on_sale(&object).await?, 2);
    let tx = object.transaction().await?;
    assert_eq!(on_sale(&tx).await?, 2);
    assert_eq!(on_sale_by_hand(&*tx).await?, 2); // deadpool's Transaction derefs to tokio-postgres's
    tx.commit().await?;
    assert_eq!(
        object.statement_cache.size(),
        1,
        "one statement, prepared once"
    );
    Ok(())
}
```

| | by hand | shelf | sqlx |
|---|---|---|---|
| **Same** | One function for a connection and a transaction; the caller decides which | The same | The same |
| **Different** | tokio-postgres's `GenericClient` covers a `Client` and a `Transaction`; a pooled connection must be passed as `&**object`, which bypasses deadpool's cache (and deadpool's own `GenericClient` refuses a plain `Client`) | One trait for all four types; a pooled connection is passed as `&object` and prepares through its cache; shelf's calls check the struct against the prepared statement before running | The executor is consumed by value: `&Pool` (each query may take a different connection) or `&mut PgConnection` (a pool connection or transaction dereferenced to it); a function that runs two queries on one generic executor takes `Acquire` instead; statements are cached per connection |

---

## In shelf

Reading code that uses the executor, three patterns repeat. A provided method or helper takes
`db: &impl Executor` and returns `impl Future<…> + Send` with an `async move` block inside (the
`Table` methods of Chapter 6); a free function in shelf is a plain `async fn`, because an inherent
or free `async fn`'s future is `Send` whenever what it holds is, which the compiler can see
(`fetch_all`); and every call goes through `prepare` first, then `query`, `query_one`,
`query_opt` or `execute` with the `Statement`:

~~~rust
// orm/shelf/src/fetch.rs (excerpt)
/// The row of `sql`, read into `T`, if there is one; fails if there are several.
pub async fn fetch_opt<T: FromRow>(
    db: &impl Executor,
    sql: &str,
    params: &[&(dyn ToSql + Sync)],
) -> Result<Option<T>, Error> {
    let statement = db.prepare(sql).await?;
    let columns = T::resolve(statement.columns())?;
    let row = db.query_opt(&statement, params).await?;
    row.map(|row| T::from_row_at(&row, &columns)).transpose()
}
~~~

[orm/shelf/src/fetch.rs](../shelf/src/fetch.rs)

The difference between the two forms is where the `Send` promise is made. Inside a trait, the
caller cannot see the impl, so the promise must be written in the signature; outside one,
`fetch_opt`'s future is a concrete type the compiler inspects, and it is `Send` because the
executor's futures are. clippy's `manual_async_fn` lint prefers the `async fn` wherever it is
enough.

## When not to use the executor

- **Statements the trait does not model.** `batch_execute`, `simple_query`, `COPY … TO STDOUT`,
  `LISTEN`, `cancel_token` and `query_typed` are on the connection; use it. The executor is a
  view of the calls shelf makes, not a replacement for the client.
- **Starting a transaction.** That takes `&mut` on the connection (§7.4). Start it there, then
  pass `&tx` to shelf.
- **Many independent reads on one connection.** These work through the executor: shelf's
  futures borrow it shared, so `tokio::try_join!` on two shelf calls pipelines them on one
  connection (Exercise 3), as the main book's
  [Chapter 7](../../book/07-pipelining.md) pipelines raw queries. What the executor cannot do is
  send one batch of many statements in a single write; for that, use the `Client` directly.

## Summary

- tokio-postgres's `GenericClient` covers a `Client` and a `Transaction`; deadpool's covers a
  pooled connection and its transaction and adds `prepare_cached`. Both are sealed, so no third
  crate can implement either, and neither covers all four types.
- shelf's `Executor` is a small trait of its own, `: Sync`, implemented by one `macro_rules!`
  for the four types; a plain `Client` or `Transaction` prepares on every call, a deadpool
  connection or transaction through its cache.
- Its methods return `impl Future<Output = …> + Send`, so a function generic over the executor is
  `Send` and runs in `tokio::spawn` and axum handlers. `async fn` in the trait would not promise
  that, and a generic caller in `tokio::spawn` fails to compile.
- Pass a pooled connection as `&object`. `&*object` does not compile; `&**object` compiles and
  silently skips the cache.
- Through lagproxy: prepare-then-run costs two round trips a call; `prepare_cached` two the first
  time and one after; `query_typed` one, but it needs PostgreSQL types and gives no statement to
  check the struct against before the query runs.
- A shelf call on a `&Transaction` sees the transaction's writes. The main book's retry helper
  runs shelf calls with one change: it finds the SQLSTATE inside `Error::Db`.
- `shelf::Error` separates bugs (`Mapping`, `Schema`), outcomes (`NotFound`, `Stale`) and the
  database (`Db`). Mapping SQLSTATEs and constraint names to statuses stays the application's
  job: a `From<shelf::Error>` impl where the error type is the application's own, a function with
  `map_err` where it is not (the orphan rule).

## Exercises

1. **A counting executor.** Write `Counting<'a, E>`, an executor around another that counts the
   statements prepared and the statements run, and use it to show what `Product::find` sends.
   *Hint:* implement each required method by delegating to the inner executor; count before
   returning its future. `copy_in`'s future yields a `CopyInSink<bytes::Bytes>`.
2. **A statement cache for a plain `Client`.** Write `Cached`, which owns a `Client` and a map
   from SQL text to `Statement`, and implements `Executor` so that `prepare` prepares each text
   once. Check with `pg_prepared_statements` that five `find`s leave one statement.
   *Hint:* the main book's §4.6 wrote the cache; the new part is the `Send` future, which must not
   hold the map's lock across an `.await`.
3. **Two calls at once.** Through lagproxy, time `Product::find` followed by `Product::count` on
   one `Client`, then the same two inside `tokio::try_join!`. How many round trips does each
   take?
   *Hint:* each call is two round trips on a plain `Client`, and the client sends requests in the
   order it receives them, without waiting for answers (main book
   [Chapter 7](../../book/07-pipelining.md)).

### Solutions

**1.** Each method counts, then returns the inner executor's future unchanged:

```rust
use bytes::Bytes;
use shelf::{Executor, FromRow, Table};
use std::future::Future;
use std::sync::atomic::{AtomicU32, Ordering};
use tokio_postgres::types::ToSql;
use tokio_postgres::{CopyInSink, Error, Row, RowStream, Statement};

/// An executor that counts what passes through it on the way to another.
struct Counting<'a, E> {
    inner: &'a E,
    prepared: AtomicU32,
    run: AtomicU32,
}

impl<'a, E: Executor> Counting<'a, E> {
    fn new(inner: &'a E) -> Self {
        Counting {
            inner,
            prepared: AtomicU32::new(0),
            run: AtomicU32::new(0),
        }
    }

    fn counts(&self) -> (u32, u32) {
        (
            self.prepared.load(Ordering::Relaxed),
            self.run.load(Ordering::Relaxed),
        )
    }

    fn ran(&self) {
        self.run.fetch_add(1, Ordering::Relaxed);
    }
}

impl<E: Executor> Executor for Counting<'_, E> {
    fn prepare(&self, sql: &str) -> impl Future<Output = Result<Statement, Error>> + Send {
        self.prepared.fetch_add(1, Ordering::Relaxed);
        self.inner.prepare(sql)
    }

    fn query(
        &self,
        statement: &Statement,
        params: &[&(dyn ToSql + Sync)],
    ) -> impl Future<Output = Result<Vec<Row>, Error>> + Send {
        self.ran();
        self.inner.query(statement, params)
    }

    fn query_one(
        &self,
        statement: &Statement,
        params: &[&(dyn ToSql + Sync)],
    ) -> impl Future<Output = Result<Row, Error>> + Send {
        self.ran();
        self.inner.query_one(statement, params)
    }

    fn query_opt(
        &self,
        statement: &Statement,
        params: &[&(dyn ToSql + Sync)],
    ) -> impl Future<Output = Result<Option<Row>, Error>> + Send {
        self.ran();
        self.inner.query_opt(statement, params)
    }

    fn execute(
        &self,
        statement: &Statement,
        params: &[&(dyn ToSql + Sync)],
    ) -> impl Future<Output = Result<u64, Error>> + Send {
        self.ran();
        self.inner.execute(statement, params)
    }

    fn query_raw(
        &self,
        statement: &Statement,
        params: &[&(dyn ToSql + Sync)],
    ) -> impl Future<Output = Result<RowStream, Error>> + Send {
        self.ran();
        self.inner.query_raw(statement, params)
    }

    fn copy_in(
        &self,
        statement: &Statement,
    ) -> impl Future<Output = Result<CopyInSink<Bytes>, Error>> + Send {
        self.ran();
        self.inner.copy_in(statement)
    }
}

#[derive(Debug, FromRow, Table)]
#[shelf(table = "products")]
struct Product {
    #[shelf(primary_key)]
    id: i64,
    sku: String,
}

#[tokio::main]
async fn main() -> Result<(), shelf::Error> {
    let client = shelf_playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE products (id bigint PRIMARY KEY, sku text NOT NULL);
             INSERT INTO products VALUES (1, 'MUG-01'), (2, 'TEE-02');",
        )
        .await?;

    let counting = Counting::new(&client);
    Product::find(&counting, &1).await?;
    assert_eq!(counting.counts(), (1, 1), "one prepare, one query");
    assert_eq!(Product::all(&counting).await?.len(), 2);
    assert_eq!(Product::count(&counting).await?, 2);
    assert_eq!(counting.counts(), (3, 3));
    Ok(())
}
```

The wrapper counts calls to `prepare`, not round trips: around a deadpool connection it would
count a `prepare` for every call, cached or not. To count what reaches the server, count inside
the connection (the capstone's `line_for_line` counts the client's *Parse* and *Execute*
messages), or time the calls through lagproxy, as §7.3 does.

**2.** The lock is taken twice, each time in a statement of its own, so no guard lives across the
`.await`:

```rust
use bytes::Bytes;
use shelf::{Executor, FromRow, Table};
use std::collections::HashMap;
use std::future::Future;
use std::sync::Mutex;
use tokio_postgres::types::ToSql;
use tokio_postgres::{Client, CopyInSink, Error, Row, RowStream, Statement};

/// A plain `Client` with a statement cache. One per connection: statements are per session.
struct Cached {
    client: Client,
    statements: Mutex<HashMap<String, Statement>>,
}

impl Executor for Cached {
    // An `async fn` in the impl: the compiler checks that its future is `Send`, as the trait
    // requires.
    async fn prepare(&self, sql: &str) -> Result<Statement, Error> {
        let hit = self.statements.lock().unwrap().get(sql).cloned();
        if let Some(statement) = hit {
            return Ok(statement);
        }
        let statement = self.client.prepare(sql).await?;
        self.statements
            .lock()
            .unwrap()
            .insert(sql.to_owned(), statement.clone());
        Ok(statement)
    }

    fn prepare_uncached(&self, sql: &str) -> impl Future<Output = Result<Statement, Error>> + Send {
        self.client.prepare(sql)
    }

    fn query(
        &self,
        statement: &Statement,
        params: &[&(dyn ToSql + Sync)],
    ) -> impl Future<Output = Result<Vec<Row>, Error>> + Send {
        self.client.query(statement, params)
    }

    fn query_one(
        &self,
        statement: &Statement,
        params: &[&(dyn ToSql + Sync)],
    ) -> impl Future<Output = Result<Row, Error>> + Send {
        self.client.query_one(statement, params)
    }

    fn query_opt(
        &self,
        statement: &Statement,
        params: &[&(dyn ToSql + Sync)],
    ) -> impl Future<Output = Result<Option<Row>, Error>> + Send {
        self.client.query_opt(statement, params)
    }

    fn execute(
        &self,
        statement: &Statement,
        params: &[&(dyn ToSql + Sync)],
    ) -> impl Future<Output = Result<u64, Error>> + Send {
        self.client.execute(statement, params)
    }

    fn query_raw(
        &self,
        statement: &Statement,
        params: &[&(dyn ToSql + Sync)],
    ) -> impl Future<Output = Result<RowStream, Error>> + Send {
        self.client.query_raw(statement, params.iter().copied())
    }

    fn copy_in(
        &self,
        statement: &Statement,
    ) -> impl Future<Output = Result<CopyInSink<Bytes>, Error>> + Send {
        self.client.copy_in(statement)
    }
}

#[derive(Debug, FromRow, Table)]
#[shelf(table = "products")]
struct Product {
    #[shelf(primary_key)]
    id: i64,
    sku: String,
}

#[tokio::main]
async fn main() -> Result<(), shelf::Error> {
    let db = Cached {
        client: shelf_playground::connect().await?,
        statements: Mutex::new(HashMap::new()),
    };
    db.client
        .batch_execute(
            "CREATE TEMP TABLE products (id bigint PRIMARY KEY, sku text NOT NULL);
             INSERT INTO products VALUES (1, 'MUG-01');",
        )
        .await?;

    for _ in 0..5 {
        assert_eq!(Product::find(&db, &1).await?.sku, "MUG-01");
    }
    // An unnamed statement (query_typed) does not count itself.
    let open: i64 = db
        .client
        .query_typed_one("SELECT count(*) FROM pg_prepared_statements", &[])
        .await?
        .try_get(0)?;
    assert_eq!(open, 1, "five finds, one statement");
    Ok(())
}
```

Written as `if let Some(statement) = self.statements.lock().unwrap().get(sql) { … }`, the guard
would live to the end of the `if let`, and in a version that awaited inside it the future would
hold a `MutexGuard`, which is not `Send`. A cache like this one could keep each statement's
resolved `Columns` beside it, keyed by the struct's type too, and skip Chapter 5's `resolve`;
§5.6 measured what that would save, under a microsecond per call, and shelf does without.

**3.** Run one after the other, the two calls take four round trips; joined, two:

<!-- check: needs=lagproxy -->
```rust
use shelf::{FromRow, Table};
use std::time::Instant;

#[derive(Debug, FromRow, Table)]
#[shelf(table = "products")]
struct Product {
    #[shelf(primary_key)]
    id: i64,
    sku: String,
}

#[tokio::main]
async fn main() -> Result<(), shelf::Error> {
    let client = shelf_playground::connect().await?; // through lagproxy
    client
        .batch_execute(
            "CREATE TEMP TABLE products (id bigint PRIMARY KEY, sku text NOT NULL);
             INSERT INTO products VALUES (1, 'MUG-01'), (2, 'TEE-02');",
        )
        .await?;

    let started = Instant::now();
    let mug = Product::find(&client, &1).await?;
    let count = Product::count(&client).await?;
    let sequential = started.elapsed();

    let started = Instant::now();
    let (joined_mug, joined_count) =
        tokio::try_join!(Product::find(&client, &1), Product::count(&client))?;
    let joined = started.elapsed();

    assert_eq!((mug.sku, count), (joined_mug.sku, joined_count));
    println!("one after the other {sequential:.1?}, joined {joined:.1?}");
    assert!(joined < sequential * 3 / 4, "about half the round trips");
    Ok(())
}
```

Three runs took 53.5–56.6 ms one after the other and 26.0–27.9 ms joined. Both calls borrow the
client shared, so `try_join!` polls them together: their two *Parse*
requests leave in one round trip, and their two queries in the next. The executor needs nothing
special for this; a plain `Client` accepts requests from any number of futures and keeps their
answers in order.

---

Previous: [Chapter 6 — Tables: the model derive](06-tables.md) · Next: [Chapter 8 — Writing rows: insert, update, delete, upsert](08-writing.md)
