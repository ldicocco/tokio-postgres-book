# Chapter 7 — One connection, many tasks: pipelining

> **Async PostgreSQL in Rust** · Previous: [Chapter 6 — Errors](06-errors.md) ·
> Next: [Chapter 8 — Transactions and concurrency control](08-transactions.md)

Chapter 4 counted round trips and concluded that a chain of queries, each waiting for the one
before, costs one round trip per link. This chapter breaks the chain. When queries do not
depend on each other, a client can send all of them and then collect the answers: the server
still runs them one at a time, but nobody waits for the network in between. That is
*pipelining*, and tokio-postgres does it for you, without any special API, whenever several
queries on the same client are in flight at once.

That one feature explains most of tokio-postgres's design: why `Client` methods take `&self`,
why the connection is a separate task, and why a single client can serve a whole program.
It also has limits, and the last part of the chapter is about them: a slow query holds up
everything behind it, and a transaction cannot be pipelined with anyone else's work.

---

## 7.1 Lazy futures, and the order requests are sent in

A tokio-postgres method returns a future, and like every Rust future it does nothing until it
is polled. Only then does it put its request on the connection's queue. The crate's
documentation states the consequence: requests are sent in the order their futures are
*first polled*, not the order in which they were created.

```rust
#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = playground::connect().await?;
    client.batch_execute("CREATE TEMP SEQUENCE ticket").await?;
    let next = client.prepare("SELECT nextval('ticket')").await?;

    // Created in the order a, b...
    let a = client.query_one(&next, &[]);
    let b = client.query_one(&next, &[]);
    // ...but join! polls b first, so b's request goes out first.
    let (b, a) = tokio::join!(b, a);
    let (a, b): (i64, i64) = (a?.get(0), b?.get(0));
    assert_eq!((a, b), (2, 1));
    Ok(())
}
```

There is a subtlety in that example: it uses a prepared `Statement`. A query given as SQL text is
*two* requests, prepare and then execute (Chapter 4), and the second one goes out when the
prepare's answer has come back and the future is polled again, which depends on scheduling.
Run the example above with text instead of `&next`, and the order of the two `nextval` calls
varies from run to run. So when the order matters, use a prepared statement or `query_typed`:
one request each, sent in the order you poll them.

## 7.2 `join!`: N queries, about one round trip

Awaiting queries one after another sends each request only after the previous answer arrived.
Polling them together with `join!` (or `try_join!`, which stops at the first error) puts all
the requests on the wire at once:

```text
 one after another                        joined
   client ──q1──▶ server                    client ──q1 q2 q3──▶ server
   client ◀──r1── server                    client ◀──r1 r2 r3── server
   client ──q2──▶ server
   client ◀──r2── server                    1 round trip (plus the server's work)
   client ──q3──▶ server
   client ◀──r3── server
   3 round trips
```

On localhost both take microseconds. Through lagproxy, which adds 5 ms each way (Chapter 4
shows how to set it up), the difference is the whole point:

<!-- check: needs=lagproxy -->
```rust
use std::time::Instant;

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = playground::connect().await?; // through lagproxy
    let count = client.prepare("SELECT count(*) FROM pg_class").await?;
    let size = client
        .prepare("SELECT pg_database_size(current_database())")
        .await?;
    let version = client.prepare("SHOW server_version").await?;
    let now = client.prepare("SELECT now()").await?;

    let started = Instant::now();
    client.query_one(&count, &[]).await?;
    client.query_one(&size, &[]).await?;
    client.query_one(&version, &[]).await?;
    client.query_one(&now, &[]).await?;
    let one_by_one = started.elapsed();

    let started = Instant::now();
    let (c, s, v, n) = tokio::try_join!(
        client.query_one(&count, &[]),
        client.query_one(&size, &[]),
        client.query_one(&version, &[]),
        client.query_one(&now, &[]),
    )?;
    let joined = started.elapsed();

    println!("one by one: {one_by_one:?}, joined: {joined:?}");
    assert!(c.get::<_, i64>(0) > 0 && s.get::<_, i64>(0) > 0);
    assert!(!v.get::<_, String>(0).is_empty());
    let _: jiff::Timestamp = n.get(0);
    assert!(
        joined * 2 < one_by_one,
        "about one round trip instead of four"
    );
    Ok(())
}
```

```text
one by one: 59.7ms, joined: 16.2ms
```

Four round trips became one. The server did exactly the same work in both cases, in the same
order; what disappeared is the waiting on the network in between.

> **Protocol lens:** each tokio-postgres request ends with a *Sync* message, which marks the
> end of a unit of work for the server. Requests from different futures are just written to
> the socket back to back, and the server reads and answers them in order. Nothing else is
> needed: the protocol has always allowed this. libpq gained an explicit "pipeline mode" API
> only in PostgreSQL 14, and psql only in 18 (see the side by side below).

## 7.3 One client, many tasks

`join!` pipelines within one task. The same happens across tasks, because a `Client` can be
shared: its methods take `&self`, and it is `Send` and `Sync`. Wrap it in an `Arc`, give each
task a clone, and their requests meet on the one connection:

<!-- check: needs=lagproxy -->
```rust
use std::sync::Arc;
use std::time::{Duration, Instant};

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = Arc::new(playground::connect().await?); // through lagproxy
    let delay: u64 = std::env::var("LAGPROXY_DELAY_MS").map_or(5, |v| v.parse().unwrap());
    let statement = client.prepare("SELECT $1::int8 * $1").await?;

    // Twenty "requests", each in its own task, each needing one query.
    let started = Instant::now();
    let mut tasks = Vec::new();
    for i in 0..20_i64 {
        let (client, statement) = (Arc::clone(&client), statement.clone());
        tasks.push(tokio::spawn(async move {
            let row = client.query_one(&statement, &[&i]).await?;
            Ok::<i64, tokio_postgres::Error>(row.get(0))
        }));
    }
    let mut squares = Vec::new();
    for task in tasks {
        squares.push(task.await.expect("no panic")?);
    }
    let elapsed = started.elapsed();

    assert_eq!(squares[7], 49);
    // Twenty one-round-trip queries; they all overlapped.
    let one_round_trip = Duration::from_millis(2 * delay);
    println!("20 tasks in {elapsed:?}");
    assert!(elapsed < one_round_trip * 5);
    Ok(())
}
```

It printed "20 tasks in 12.6ms": about one round trip through the proxy, for twenty
queries. That is a service in miniature: many concurrent requests, each with a little
database work, sharing one connection, none of them waiting for the others' round trips. Chapter 13 measures
it against a connection pool over the same slow network. The shared client won by almost an
order of magnitude for independent queries, because a pool of eight allows eight requests in
flight, and one pipelined connection allows them all.

> **Rust refresher:** `Arc<Client>` is all the sharing takes, with no `Mutex`, because nothing
> needs `&mut Client`. The connection task is the only owner of the socket, and the client
> only talks to it through a channel. Shared state behind message passing, not behind a lock:
> the same *actor* pattern you would use to share any resource between tasks.

## 7.4 Under the hood: a queue, in order

The `Client` sends each request to the connection task through an unbounded channel, along
with the sending end of a small channel for its responses. The connection writes requests to
the socket in the order they arrived, and routes the server's responses, which come back in
the same order, to their senders. Two properties follow.

**Order is guaranteed.** Requests are executed in the order they were sent, on one session. So
a write followed by a read, sent without waiting, sees the write.

**There is no overtaking.** The server runs one statement at a time per session. A slow query
holds up every request queued behind it on that connection, however fast those are. This is
*head-of-line blocking*:

```rust
use std::sync::Arc;
use std::time::{Duration, Instant};

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let shared = Arc::new(playground::connect().await?);
    let other = playground::connect().await?;

    // Something slow starts on the shared connection...
    let slow = {
        let shared = Arc::clone(&shared);
        tokio::spawn(async move { shared.execute("SELECT pg_sleep(0.5)", &[]).await })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;

    // ...and a trivial query behind it waits for it to finish.
    let started = Instant::now();
    shared.query_one("SELECT 1", &[]).await?;
    let behind_slow = started.elapsed();

    // The same query on another connection does not.
    let started = Instant::now();
    other.query_one("SELECT 1", &[]).await?;
    let elsewhere = started.elapsed();

    slow.await.expect("no panic")?;
    println!("behind the slow query: {behind_slow:?}; on another connection: {elsewhere:?}");
    assert!(behind_slow > Duration::from_millis(300));
    assert!(elsewhere < Duration::from_millis(100));
    Ok(())
}
```

```text
behind the slow query: 451.3ms; on another connection: 1.3ms
```

A program that shares one client should therefore keep slow work (reports, exports, anything
that scans a lot) off it. Give that work a connection of its own, or put it behind a pool
(Chapter 13), and use `statement_timeout` (Chapter 12) to bound what can hold up the queue.

> **Pitfall:** a large result consumed slowly is also slow work. The connection delivers a
> query's rows into a small buffer and stops reading the socket when that buffer is full, so a
> `RowStream` that you read slowly holds up everything behind it. Chapter 9 measures this.

## 7.5 Many queries, a few at a time

`join!` takes a fixed number of futures. For a list, use `futures_util`'s combinators:
`try_join_all` runs them all at once, and `buffer_unordered(n)` (on a stream of futures) runs at
most `n` at a time, which keeps a huge list from queueing thousands of requests:

```rust
use futures_util::{StreamExt, TryStreamExt, future, stream};

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE prices AS
             SELECT 'SKU' || i AS sku, i * 100 AS price_cents FROM generate_series(1, 500) AS i",
        )
        .await?;
    let price = client
        .prepare("SELECT price_cents FROM prices WHERE sku = $1")
        .await?;
    let skus: Vec<String> = (1..=500).map(|i| format!("SKU{i}")).collect();

    // All at once: one future per SKU, all pipelined.
    let rows = future::try_join_all(skus.iter().take(10).map(|sku| {
        let price = &price;
        let client = &client;
        // The parameter slice must live as long as the future: build it inside the async
        // block. (`client.query_one(price, &[sku])` as the closure's result would borrow a
        // temporary slice that dies when the closure returns.)
        async move { client.query_one(price, &[sku]).await }
    }))
    .await?;
    assert_eq!(rows.len(), 10);

    // At most 16 in flight at a time, in any order, summed as they come.
    let total: i32 = stream::iter(&skus)
        .map(|sku| {
            let (price, client) = (&price, &client);
            async move {
                let row = client.query_one(price, &[sku]).await?;
                Ok::<i32, tokio_postgres::Error>(row.get(0))
            }
        })
        .buffer_unordered(16)
        .try_fold(0, |sum, cents| async move { Ok(sum + cents) })
        .await?;
    assert_eq!(total, (1..=500).sum::<i32>() * 100);
    Ok(())
}
```

`buffer_unordered` yields results as they complete; `buffered(n)` keeps them in input order.
On one connection the server answers in order anyway, so the difference is small here, but it
matters as soon as the futures do more than one query.

> **Pitfall:** the comment in that example is the error you will meet first when pipelining a
> list. A future returned by `query_one` borrows its parameter slice, so `|sku|
> client.query_one(&price, &[sku])` does not compile: the slice `&[sku]` is a temporary inside
> the closure. Put the call in an `async move` block (as above), or bind the slice to a variable
> that outlives the futures. stockroom's code has both, with a comment each time.

## 7.6 When not to pipeline

Pipelining is for independent work. Three situations need care.

**A query that needs the previous one's answer** cannot be sent before that answer arrives.
That sounds obvious, but it is worth turning around: look for queries that *do not* depend
on each other, because they are often written one after another out of habit. Better still,
let the database do the dependent steps in one statement (a join, a subquery, `RETURNING`, a
`WITH` clause).

**An error in one query does not stop the others.** Each request is its own unit of work, so
the server runs every request it received, and with `try_join!` the futures you stop waiting
for still did their work:

```rust
use tokio_postgres::types::Type;

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = playground::connect().await?;
    client
        .batch_execute("CREATE TEMP TABLE log (n int)")
        .await?;

    // Three requests, all sent before any answer: the middle one fails.
    let result = tokio::try_join!(
        client.execute_typed("INSERT INTO log VALUES ($1)", &[(&1, Type::INT4)]),
        client.execute_typed("SELECT 1 / $1", &[(&0, Type::INT4)]),
        client.execute_typed("INSERT INTO log VALUES ($1)", &[(&3, Type::INT4)]),
    );
    assert!(result.is_err(), "try_join! reports the division by zero...");

    let logged: Vec<i32> = client
        .query_scalar("SELECT n FROM log ORDER BY n", &[])
        .await?;
    assert_eq!(logged, [1, 3], "...but both inserts happened");
    Ok(())
}
```

When several changes must succeed or fail together, that is a transaction's job, not a
pipeline's.

**A transaction on a shared client** is the dangerous case. A transaction is a state of the
*session*: between `BEGIN` and `COMMIT`, every statement on the connection is part of it,
whoever sent it. Start one with plain SQL on a client that other tasks share, and their
statements join your transaction:

```rust
use std::sync::Arc;
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let shared = Arc::new(playground::connect().await?);
    shared
        .batch_execute("CREATE TEMP TABLE audit (event text)")
        .await?;

    // Task A opens a transaction with plain SQL, writes, and later rolls back.
    let task_a = {
        let shared = Arc::clone(&shared);
        tokio::spawn(async move {
            shared.batch_execute("BEGIN").await?;
            shared
                .execute("INSERT INTO audit VALUES ('a: tentative')", &[])
                .await?;
            tokio::time::sleep(Duration::from_millis(100)).await;
            shared.batch_execute("ROLLBACK").await
        })
    };
    tokio::time::sleep(Duration::from_millis(30)).await;

    // Meanwhile, task B writes something that has nothing to do with A's transaction.
    shared
        .execute("INSERT INTO audit VALUES ('b: must survive')", &[])
        .await?;
    task_a.await.expect("no panic")?;

    // B's row ran inside A's transaction, and A's rollback took it away.
    let left: i64 = shared
        .query_one("SELECT count(*) FROM audit", &[])
        .await?
        .get(0);
    assert_eq!(left, 0);
    Ok(())
}
```

This is exactly why `Client::transaction` takes `&mut self`. A `Transaction` borrows the client
mutably, so while it exists, nothing else can use that client, and the borrow checker turns this
bug into a compile error. A client behind an `Arc` cannot give out `&mut`, so it cannot start a
transaction at all. Transactions need a connection of their own for their duration: the next
chapter's subject, and the main reason for connection pools.

### Side by side — concurrent queries

The task: run two independent queries without waiting for the first answer before sending the
second.

```sql
-- psql 18 has a pipeline mode of its own: \startpipeline … \endpipeline.
tpbook=# \startpipeline
tpbook=# SELECT 1 AS a \bind \sendpipeline
tpbook=# SELECT 2 AS b \bind \sendpipeline
tpbook=# \endpipeline
 a
---
 1
(1 row)

 b
---
 2
(1 row)
```

~~~rust
// sqlx (illustration)
// A PgConnection runs one query at a time (its methods take &mut self), so this does not
// compile on one connection. Concurrency comes from a pool: two queries, two connections.
let (a, b) = tokio::try_join!(
    sqlx::query_scalar::<_, i32>("SELECT 1").fetch_one(&pool),
    sqlx::query_scalar::<_, i32>("SELECT 2").fetch_one(&pool),
)?;
~~~

```rust
#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = playground::connect().await?;
    // One connection; both requests are written before either answer is read.
    let (a, b) = tokio::try_join!(
        client.query_one("SELECT 1", &[]),
        client.query_one("SELECT 2", &[]),
    )?;
    assert_eq!((a.get::<_, i32>(0), b.get::<_, i32>(0)), (1, 2));
    Ok(())
}
```

| | psql | sqlx | tokio-postgres |
|---|---|---|---|
| **Same** | Requests sent back to back on one session, answered in order | Concurrent queries finish without waiting for each other | Requests sent back to back on one session, answered in order |
| **Different** | Explicit mode, statement by statement, in psql 18 | No pipelining on one connection: concurrency costs one connection per query in flight | Automatic: any futures in flight at once on the same client, from one task or many |

---

## In stockroom

stockroom pipelines in three places, each for a different reason. The dashboard is the
textbook case, five independent aggregates in one `try_join!`:

~~~rust
// stockroom/src/repo/reports.rs (excerpt)
pub async fn dashboard(client: &Client) -> Result<Dashboard, AppError> {
    let (products, stores, open_orders, today, low) = tokio::try_join!(
        client.query_one_scalar::<i64, _>("SELECT count(*) FROM products", &[]),
        client.query_one_scalar::<i64, _>("SELECT count(*) FROM stores", &[]),
        client.query_one_scalar::<i64, _>(
            "SELECT count(*) FROM orders WHERE status IN ('pending', 'paid', 'ready')",
            &[]
        ),
        // … today's orders and revenue, and the ten lowest stock levels …
    )?;
~~~

Reading an order is two queries, the order and its lines, that both need only the id. The
parameter slice is bound to a name because both futures borrow it:

~~~rust
// stockroom/src/repo/orders.rs (excerpt)
// Two queries, one round trip: pipelined. (The parameter slice is bound to a name
// because both futures borrow it until they finish.)
let params: &[&(dyn ToSql + Sync)] = &[&id];
let (head, lines) = tokio::try_join!(db.query_opt(&head, params), db.query(&lines, params))?;
~~~

Placing an order pipelines inside a transaction, which is safe because the transaction owns its
connection. Every line's reservation is a guarded `UPDATE`, and they all go out at once:

~~~rust
// stockroom/src/repo/orders.rs (excerpt)
// Every reservation at once: pipelined, sent in this order, run one by one by the server.
let reserve = tx
    .prepare_cached(
        "UPDATE stock SET reserved = reserved + $3
         WHERE store_id = $1 AND product_id = $2 AND available >= $3",
    )
    .await?;
let (reserve, store_id) = (&reserve, &store_id);
let reserved = future::try_join_all(lines.iter().map(|l| async move {
    // The parameter slice must live as long as the future that uses it: inside the
    // `async` block, not as a temporary in the closure.
    tx.execute(reserve, &[store_id, &l.product_id, &l.quantity])
        .await
}))
.await?;
~~~

"Sent in this order" is a promise with consequences. The lines are sorted by product id so that
every order locks stock rows in the same order and two orders cannot deadlock (Chapter 8). The
promise holds because `reserve` is a prepared statement, so each reservation is a single
request, and `try_join_all` polls its futures in list order. With SQL text, §7.1 showed, the
order would vary.

## Summary

- Futures are lazy: a request is sent when its future is first polled. Single-request calls (a
  `Statement`, `query_typed`) are sent in that order; a SQL-text query is two requests, and its
  execution order can vary.
- Futures in flight at the same time on the same client are pipelined: N independent queries
  cost about one round trip instead of N. `join!`, `try_join!`, `try_join_all` and
  `buffer_unordered` all do it.
- `Client` is `Send + Sync` with `&self` methods: share it with `Arc`, and tasks pipeline
  their requests on one connection.
- The server runs one statement at a time per session: a slow query, or a slowly consumed
  result, blocks everything queued behind it.
- A query future borrows its parameter slice: build it inside an `async move` block or bind it
  to a name.
- Every request is executed, even if you stop waiting for it; errors do not cancel neighbours.
- Never start a transaction with SQL on a shared client: other tasks' statements join it.
  `transaction()` takes `&mut self` to make that impossible.

## Exercises

1. **Measure the dashboard.** Through lagproxy, time stockroom's five dashboard queries one
   after another, then with `try_join!` as in the excerpt. Predict both from Chapter 4's
   numbers first.
   *Hint:* the queries are SQL text, so count two round trips each.
2. **Order or not.** Repeat §7.1's example with SQL text instead of the prepared statement, in a
   loop of 20. How often is the order reversed? Explain the result in terms of requests.
   *Hint:* count the requests that each `query_one` with SQL text sends, and when its second one
   leaves.
3. **A bulk lookup.** Given 10,000 SKUs, fetch their prices with at most 32 queries in flight,
   and compare the time with one `= ANY($1)` query for all of them. Which is faster, and when
   would you still choose the first?
   *Hint:* `stream::iter(skus).map(…).buffer_unordered(32)` (§7.5) for the first; `sku =
   ANY($1)` with a `&[String]` for the second.
4. **Fix the shared transaction.** Rewrite §7.6's last example so that task A's rollback
   cannot touch task B's row, while B still uses the shared client.
   *Hint:* A needs a connection of its own.

### Solutions

**1.** Ten round trips one after another (five prepares, five executes), so about 150 ms at a
5 ms delay. Joined, the five prepares go out together and the five executes go out as their
prepares complete: about two round trips, 30 ms. Preparing the five statements once
(`prepare_cached`, as stockroom does elsewhere) would make it one round trip.

**3.** One `= ANY($1)` query is far faster: one statement, one plan, one result, instead of
10,000 executions. Pipelining hides network latency, not the cost of 10,000 statements on the
server. The first approach is still the right one when each item needs a different query, or
the work per item is more than a lookup.

**4.**

```rust
use std::sync::Arc;
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let shared = Arc::new(playground::connect().await?);
    // Temporary tables belong to one session, so the table must be a real one here, visible
    // to both connections. A schema of its own keeps it apart, and goes away at the end.
    shared
        .batch_execute(
            "DROP SCHEMA IF EXISTS ch07 CASCADE; CREATE SCHEMA ch07;
             CREATE TABLE ch07.audit (event text);",
        )
        .await?;

    // Task A: a connection of its own, and a real transaction on it.
    let task_a = tokio::spawn(async move {
        let mut own = playground::connect().await?;
        let tx = own.transaction().await?;
        tx.execute("INSERT INTO ch07.audit VALUES ('a: tentative')", &[])
            .await?;
        tokio::time::sleep(Duration::from_millis(100)).await;
        tx.rollback().await
    });
    tokio::time::sleep(Duration::from_millis(30)).await;

    shared
        .execute("INSERT INTO ch07.audit VALUES ('b: must survive')", &[])
        .await?;
    task_a.await.expect("no panic")?;

    let left: Vec<String> = shared
        .query_scalar("SELECT event FROM ch07.audit", &[])
        .await?;
    assert_eq!(left, ["b: must survive"]);
    shared.batch_execute("DROP SCHEMA ch07 CASCADE").await?;
    Ok(())
}
```

---

Previous: [Chapter 6 — Errors](06-errors.md) ·
Next: [Chapter 8 — Transactions and concurrency control](08-transactions.md)
