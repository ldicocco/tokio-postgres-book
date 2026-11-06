# Chapter 9 — Streaming rows and portals

> **Async PostgreSQL in Rust** · Previous: [Chapter 8 — Transactions and concurrency control](08-transactions.md) ·
> Next: [Chapter 10 — Bulk data: COPY](10-copy.md)

Every query so far has returned a `Vec<Row>`: the whole result, in memory, before your code sees
the first row. That is the right default for the results most programs handle, a page of
products or one order and its lines. It is the wrong one for an export of the whole catalog, a
report over a year of stock movements, or an HTTP response that should start before the last
row is read.

This chapter is about the other ways to read a result. A `RowStream` hands you rows as they
arrive. A *portal* goes further: the server produces only the rows you ask for, a batch at a
time. SQL has its own version of portals, cursors, and HTTP has its own answer, keyset
pagination. Each comes with a cost, and the most important one is easy to miss: a stream that
you read slowly holds up its whole connection.

---

## 9.1 Why a `Vec<Row>` is not always enough

`query` is a convenience. In 0.7.18 it is literally `query_raw(…)` followed by
`try_collect()` ([`client.rs`](https://docs.rs/tokio-postgres/0.7.18/src/tokio_postgres/client.rs.html)):
it reads the stream this chapter is about into a vector and hands you the vector. Two costs
follow, and both grow with the result:

- **Memory.** Every row is held at once. Chapter 3's `Row::raw_size_bytes` measures it.
- **Latency.** Nothing reaches your code until the last row has arrived.

```rust
#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = playground::connect().await?;
    let rows = client
        .query(
            "SELECT i, repeat('x', 100) AS filler FROM generate_series(1, 200000) i",
            &[],
        )
        .await?;
    let bytes: usize = rows.iter().map(|row| row.raw_size_bytes()).sum();
    println!(
        "{} rows, {} MB of row data, all in memory at once",
        rows.len(),
        bytes / 1_000_000
    );
    assert_eq!(rows.len(), 200_000);
    assert!(bytes > 20_000_000);
    Ok(())
}
```

```text
200000 rows, 22 MB of row data, all in memory at once
```

Twenty-two megabytes is affordable once. It is not affordable per request when a hundred
clients ask for the export at the same time, and it grows with the table. The fix is to
handle each row and let it go before the next one arrives.

## 9.2 `query_raw` and `RowStream`

`query_raw` takes the same statement as `query` and returns a
[`RowStream`](https://docs.rs/tokio-postgres/0.7.18/tokio_postgres/struct.RowStream.html), a
`Stream` of `Result<Row, Error>`. Its parameters are different:

```text
pub async fn query_raw<T, P, I>(&self, statement: &T, params: I) -> Result<RowStream, Error>
where
    T: ?Sized + ToStatement,
    P: BorrowToSql,
    I: IntoIterator<Item = P>,
    I::IntoIter: ExactSizeIterator,
```

Instead of a slice of `&(dyn ToSql + Sync)`, it takes any iterator of values that can lend a
`&dyn ToSql` ([`BorrowToSql`](https://docs.rs/postgres-types/0.2.14/postgres_types/trait.BorrowToSql.html)),
as long as the iterator knows its length: the parameter count is checked against the
statement before anything is sent. The `_raw` family shares that signature: `query_raw`,
`execute_raw`, and, on a `Transaction`, `bind_raw` (§9.4). `query_typed_raw` takes
`(value, Type)` pairs instead.

To pass the usual mixed list of parameters, you need a small adapter from a slice to that
iterator. tokio-postgres has one internally but does not export it, so add it to the
playground's `src/lib.rs`:

<!-- mirror: playground/src/lib.rs#slice_iter -->
~~~rust
use tokio_postgres::types::ToSql;

/// Adapts a parameter slice, the kind `query` takes, to the iterator that the `*_raw`
/// methods take: `client.query_raw(&statement, slice_iter(&[&a, &b]))`.
pub fn slice_iter<'a>(
    params: &'a [&'a (dyn ToSql + Sync)],
) -> impl ExactSizeIterator<Item = &'a dyn ToSql> + Send + Sync + 'a {
    params.iter().map(|p| *p as _)
}
~~~

Reading a stream then looks like this. `try_next` comes from `futures_util::TryStreamExt`:

```rust
use futures_util::TryStreamExt;
use std::pin::pin;

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = playground::connect().await?;
    let statement = client
        .prepare("SELECT n, $2::text || n FROM generate_series(1, $1::int) n")
        .await?;
    let rows = client
        .query_raw(&statement, playground::slice_iter(&[&5i32, &"item "]))
        .await?;
    let mut rows = pin!(rows);

    // The number of rows is in the message that ends the result, so it is not known yet.
    assert_eq!(rows.rows_affected(), None);

    let mut labels = Vec::new();
    while let Some(row) = rows.try_next().await? {
        labels.push(row.get::<_, String>(1)); // handle the row; then it is dropped
    }
    assert_eq!(labels, ["item 1", "item 2", "item 3", "item 4", "item 5"]);
    assert_eq!(rows.rows_affected(), Some(5));
    Ok(())
}
```

`rows_affected()` reads the count from the *CommandComplete* message that ends every result, so
it is `None` until the stream has been read to the end. For an `INSERT … RETURNING` whose
output you stream, it gives the count without your counting the rows.

When the parameters all have one type, or are decided at run time, the adapter is not needed:

```rust
use futures_util::TryStreamExt;
use tokio_postgres::types::{ToSql, Type};

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = playground::connect().await?;

    // All of one type: any collection of values, by value or by reference.
    let words: Vec<String> = vec!["stock".into(), "room".into()];
    let rows: Vec<_> = client
        .query_raw("SELECT $1 || $2", words)
        .await?
        .try_collect()
        .await?;
    assert_eq!(rows[0].get::<_, String>(0), "stockroom");

    client
        .batch_execute("CREATE TEMP TABLE word (w text)")
        .await?;
    let inserted = client
        .execute_raw("INSERT INTO word VALUES ($1), ($2)", ["stock", "room"])
        .await?;
    assert_eq!(inserted, 2);

    // Decided at run time: boxed trait objects. Only the placeholder *numbers* are formatted
    // into the SQL; the values travel as parameters.
    let (min_price, tag): (Option<i64>, Option<&str>) = (Some(500), Some("a"));
    let mut sql = String::from(
        "SELECT count(*) FROM (VALUES (100::bigint, 'a'), (900, 'b'), (1200, 'a')) AS p(price, tag)
         WHERE true",
    );
    let mut params: Vec<Box<dyn ToSql + Sync + Send>> = Vec::new();
    if let Some(min) = min_price {
        params.push(Box::new(min));
        sql += &format!(" AND price >= ${}", params.len());
    }
    if let Some(tag) = tag {
        params.push(Box::new(tag.to_owned()));
        sql += &format!(" AND tag = ${}", params.len());
    }
    let rows: Vec<_> = client.query_raw(&sql, params).await?.try_collect().await?;
    assert_eq!(rows[0].get::<_, i64>(0), 1);

    // query_typed_raw: each value with its type, no prepare round trip (Chapter 4).
    let rows: Vec<_> = client
        .query_typed_raw(
            "SELECT $1 + 1, $2",
            [
                (&41i32 as &dyn ToSql, Type::INT4),
                (&"answer" as &dyn ToSql, Type::TEXT),
            ],
        )
        .await?
        .try_collect()
        .await?;
    assert_eq!(rows[0].get::<_, i32>(0), 42);
    Ok(())
}
```

> **Rust refresher:** `try_next` and `next` need a stream that is `Unpin`, and `RowStream` is
> declared `!Unpin`, so it must be *pinned* first: promised never to move again. There are two
> ways. `std::pin::pin!(rows)` pins it where it is, on the stack, and gives you a
> `Pin<&mut RowStream>` that you can poll but not move or return. `Box::pin(rows)` moves it to
> the heap and gives you a `Pin<Box<RowStream>>`, which you can return from a function, store in
> a struct or send to a task, at the cost of one allocation. Combinators that take the stream by
> value, like `try_collect` above or `try_fold`, pin it themselves.

> **Pitfall:** `drop(rows)` on a stream pinned with `pin!` drops the `Pin<&mut _>`, a reference,
> and the stream itself lives on until the end of the enclosing block. It compiles without a
> warning, and §9.3 shows what an unread stream does to its connection: the next query on the
> client never finishes. To end a stream early, let it go out of scope (put it in a block), or
> pin it with `Box::pin`, which owns it, so that `drop` really drops it.

## 9.3 Backpressure, and the stalled connection

A stream does not make the server wait for you. The server runs the query and sends rows as
fast as the socket accepts them. What makes it wait is that tokio-postgres stops *reading* the
socket when you stop reading the stream.

> **Protocol lens:** each request's responses go to the caller through a channel created with a
> capacity of one (`mpsc::channel(1)` in `client.rs`). When that channel is full, the connection
> task parks the message it is holding and stops reading the socket. The operating system's
> receive buffer fills, TCP tells the server to stop sending, and the server's backend blocks
> in a write, which `pg_stat_activity` shows as the wait event `ClientWrite`. Memory stays flat
> all the way down. The price is that one socket serves the whole connection: nothing else
> queued on it can be answered until the stream moves again.

Here is that state, produced on purpose: read one row of a large result, then try another
query on the same client:

```rust
use futures_util::TryStreamExt;
use std::pin::pin;
use std::time::Duration;
use tokio::time::timeout;

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = playground::connect().await?;
    let other = playground::connect().await?;
    let pid: i32 = client
        .query_one("SELECT pg_backend_pid()", &[])
        .await?
        .get(0);
    let wait = Duration::from_millis(500);
    {
        let rows = client
            .query_raw(
                "SELECT i, repeat('x', 100) FROM generate_series(1, 200000) i",
                playground::slice_iter(&[]),
            )
            .await?;
        let mut rows = pin!(rows);
        let first = rows.try_next().await?.expect("a first row");
        assert_eq!(first.get::<_, i32>(0), 1);

        // The stream is not being read. A query on the same client waits behind it...
        let same = timeout(wait, client.query_one("SELECT 1", &[])).await;
        // ...while the same query on another connection does not.
        let elsewhere = timeout(wait, other.query_one("SELECT 1", &[])).await;
        // And the server is stuck writing to the client.
        let event: Option<String> = other
            .query_one(
                "SELECT wait_event FROM pg_stat_activity WHERE pid = $1",
                &[&pid],
            )
            .await?
            .get(0);
        println!(
            "same connection timed out: {}; other connection timed out: {}; server waits on {:?}",
            same.is_err(),
            elsewhere.is_err(),
            event
        );
        assert!(same.is_err());
        assert!(elsewhere.is_ok());
        assert_eq!(event.as_deref(), Some("ClientWrite"));
    } // the stream is dropped here

    // With the stream gone, the connection discards the rest of its rows and the queue moves.
    client.query_one("SELECT 1", &[]).await?;
    Ok(())
}
```

```text
same connection timed out: true; other connection timed out: false; server waits on Some("ClientWrite")
```

Three consequences shape the rest of this chapter and Part III:

- **A `RowStream` does not borrow the `Client`.** The compiler lets you hold it for as long as
  you like, pass it to another task, or return it from a function, and the connection stalls
  for all that time. A slow consumer is a slow query, as Chapter 7 put it, as far as everyone
  else on the connection is concerned.
- **Stream on a connection of its own.** Never stream on a client that other tasks share. In
  a service, the stream should own the pooled connection it reads from, and keep it until the
  last row (stockroom's `/products/stream`, at the end of this chapter).
- **Dropping a stream does not stop the query.** The connection reads and throws away every
  remaining row, and the server computes all of them. §9.4 measures what that costs and shows
  the alternative. Chapter 12 shows how to cancel a query outright.

## 9.4 Portals: the server produces only what you ask for

When you run a query through the extended protocol, the server keeps its execution state in a
*portal*. tokio-postgres normally uses the unnamed portal and asks for every row at once. Inside
a transaction, you can make a named portal yourself with
[`Transaction::bind`](https://docs.rs/tokio-postgres/0.7.18/tokio_postgres/struct.Transaction.html#method.bind)
and then ask for rows with `query_portal(&portal, max_rows)`. Each call is one round trip and
returns at most `max_rows` rows; between calls, the query is suspended on the server, with its
place kept:

```rust
#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let mut client = playground::connect().await?;
    let tx = client.transaction().await?;
    let statement = tx.prepare("SELECT generate_series(1, 10)").await?;
    let portal = tx.bind(&statement, &[]).await?;

    let mut batches = Vec::new();
    loop {
        let rows = tx.query_portal(&portal, 4).await?;
        batches.push(rows.len());
        if rows.len() < 4 {
            break; // a short batch is the last one
        }
    }
    assert_eq!(batches, [4, 4, 2]);
    // An exhausted portal returns nothing.
    assert!(tx.query_portal(&portal, 4).await?.is_empty());
    tx.commit().await?;
    Ok(())
}
```

If the total is a multiple of the batch size, the last full batch is followed by an empty one,
which the same `<` test handles. `bind_raw` and `query_portal_raw` are the iterator and stream
versions. Two details of the stream version, and the portal's lifetime, are worth seeing once:

```rust
use futures_util::TryStreamExt;
use std::pin::pin;
use tokio_postgres::error::SqlState;

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let mut client = playground::connect().await?;
    let tx = client.transaction().await?;
    let statement = tx.prepare("SELECT generate_series(1, $1::int)").await?;
    let portal = tx.bind_raw(&statement, [7i32]).await?;

    // A batch of 3 as a stream. The portal is suspended, not finished: no count.
    let batch = tx.query_portal_raw(&portal, 3).await?;
    let mut batch = pin!(batch);
    let mut n = 0;
    while batch.try_next().await?.is_some() {
        n += 1;
    }
    assert_eq!((n, batch.rows_affected()), (3, None));

    // max_rows = 0 means "all that is left". The portal completes, and the count it reports
    // covers this last batch only.
    let rest = tx.query_portal_raw(&portal, 0).await?;
    let mut rest = pin!(rest);
    let mut m = 0;
    while rest.try_next().await?.is_some() {
        m += 1;
    }
    assert_eq!((m, rest.rows_affected()), (4, Some(4)));
    tx.commit().await?;

    // A portal lives and dies with its transaction.
    let tx = client.transaction().await?;
    let err = tx.query_portal(&portal, 1).await.unwrap_err();
    assert_eq!(err.code(), Some(&SqlState::INVALID_CURSOR_NAME)); // 34000
    println!("{}", err.as_db_error().expect("a server error").message());
    Ok(())
}
```

```text
portal "p0" does not exist
```

The compiler does not tie a `Portal` to its transaction, so using one after the commit is a
runtime error from the server. Dropping a `Portal` sends a *Close* for it, and closing a portal
that no longer exists is not an error, so the drop itself is harmless.

### Stopping early

The difference between a portal and a stream shows when you stop before the end. A stream
asked the server for everything, so stopping early still costs the whole result (§9.3). A
portal asked for ten rows, so ten rows is all the server made:

```rust
use futures_util::TryStreamExt;
use std::pin::pin;
use std::time::Instant;

const MANY: &str = "SELECT generate_series(1, 3000000)";

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let mut client = playground::connect().await?;

    // A stream: read one row, drop it, and wait until the connection is free again.
    let started = Instant::now();
    {
        let rows = client.query_raw(MANY, playground::slice_iter(&[])).await?;
        let mut rows = pin!(rows);
        rows.try_next().await?;
    }
    client.query_one("SELECT 1", &[]).await?;
    let stream_cost = started.elapsed();

    // A portal: ask for ten rows, then end the transaction.
    let started = Instant::now();
    let tx = client.transaction().await?;
    let portal = tx.bind(MANY, &[]).await?;
    assert_eq!(tx.query_portal(&portal, 10).await?.len(), 10);
    tx.commit().await?;
    client.query_one("SELECT 1", &[]).await?;
    let portal_cost = started.elapsed();

    println!("stopping after the first rows: stream {stream_cost:?}, portal {portal_cost:?}");
    assert!(portal_cost < stream_cost / 10);
    Ok(())
}
```

```text
stopping after the first rows: stream 181.19775ms, portal 554.125µs
```

(Measured on localhost against PG 18.6, in a debug build. The stream's cost is the server
producing three million rows that nobody reads.)

> **SQL lens:** a portal stops early only if the plan can. `generate_series` in the select list
> produces one row at a time, as the example needs. In `FROM`, as
> `SELECT i FROM generate_series(1, 3000000) i`, it is a function scan that computes the whole
> series before returning the first row: the same ten-row portal took 111 ms. An `ORDER BY`
> that no index provides, a `GROUP BY` or a `DISTINCT` likewise read all their input first.
> Check the plan with `EXPLAIN` before relying on a portal to save work.

A portal keeps its transaction open, and with it a snapshot, for as long as you page through
it. On a busy database that has a cost of its own: the rows that other transactions delete or
update cannot be vacuumed away while your snapshot might still need them. Keep portal
transactions as short as the job allows, and read-only. stockroom's export, at the end of this
chapter, uses the isolation level made for this job.

## 9.5 The SQL alternative: `DECLARE` and `FETCH`

A portal made with `bind` is the protocol's version of a SQL *cursor*. The server treats them
as the same kind of object, and the
[`pg_cursors`](https://www.postgresql.org/docs/18/view-pg-cursors.html) view lists both:

```rust
use tokio_postgres::error::SqlState;

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let mut client = playground::connect().await?;

    // Outside a transaction, a plain cursor would end with its own statement, so the server
    // refuses to create one.
    let err = client
        .batch_execute("DECLARE numbers CURSOR FOR SELECT 1")
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some(&SqlState::NO_ACTIVE_SQL_TRANSACTION)); // 25P01

    let tx = client.transaction().await?;
    tx.batch_execute("DECLARE numbers CURSOR FOR SELECT generate_series(1, 10)")
        .await?;
    let first = tx.query("FETCH 4 FROM numbers", &[]).await?;
    let rest = tx.query("FETCH ALL FROM numbers", &[]).await?;
    assert_eq!((first.len(), rest.len()), (4, 6));

    // A portal made with bind() sits beside the cursor.
    let statement = tx.prepare("SELECT 42").await?;
    let _portal = tx.bind(&statement, &[]).await?;
    let names: Vec<String> = tx
        .query(
            "SELECT name FROM pg_cursors WHERE name <> '' ORDER BY name",
            &[],
        )
        .await?
        .iter()
        .map(|row| row.get(0))
        .collect();
    println!("{names:?}");
    assert_eq!(names.len(), 2);
    assert!(names.contains(&"numbers".to_owned()));
    tx.commit().await?;

    // WITH HOLD: the cursor outlives the transaction that created it.
    client
        .batch_execute("DECLARE kept CURSOR WITH HOLD FOR SELECT generate_series(1, 10)")
        .await?;
    assert_eq!(client.query("FETCH 4 FROM kept", &[]).await?.len(), 4);
    client.batch_execute("CLOSE kept").await?;
    Ok(())
}
```

```text
["numbers", "p0"]
```

(The unnamed portal, `''`, is the one running the `pg_cursors` query itself. tokio-postgres
names its portals `p0`, `p1` and so on, from a counter shared by the whole program.)

So when would you write `DECLARE` instead of calling `bind`? When you need what only the SQL
form offers:

- **`WITH HOLD`**, to keep reading after the transaction commits. The
  [documentation](https://www.postgresql.org/docs/18/sql-declare.html) is frank about the
  price: "the rows represented by a held cursor are copied into a temporary file or memory area
  so that they remain available for subsequent transactions." The whole remaining result is
  computed at the commit.
- **`SCROLL`**, to move backwards or jump (`FETCH PRIOR`, `FETCH ABSOLUTE 100`, `MOVE`).
- **A cursor driven from SQL**, by a PL/pgSQL function or by another tool that only speaks SQL.

Otherwise prefer `bind`: the parameters are typed and sent separately, no names to invent, and
the portal is closed for you when it is dropped. Each `FETCH` is also a statement in its own
right, so it pays the prepare round trip of Chapter 4 unless you prepare it once.

## 9.6 Keyset pagination: the HTTP answer

Neither a stream nor a portal survives the end of an HTTP request: the next page arrives as a
new request, often on another connection from the pool, in another transaction. The page has
to be described in the query itself. `LIMIT … OFFSET …` is the obvious way, and it gets slower
the deeper you go, because, in the words of the
[documentation](https://www.postgresql.org/docs/18/queries-limit.html), "the rows skipped by
an `OFFSET` clause still have to be computed inside the server".

*Keyset pagination* (also called the seek method) remembers the last key of the page instead,
and asks for the rows after it. With an index on the key, every page costs the same. `EXPLAIN
ANALYZE` makes the difference countable:

```rust
use tokio_postgres::{Client, Error};

/// How many rows the scan under the `LIMIT` produced, from `EXPLAIN (ANALYZE, FORMAT JSON)`.
async fn rows_scanned(client: &Client, explain: &str) -> Result<f64, Error> {
    let plan: serde_json::Value = client.query_one(explain, &[]).await?.get(0);
    Ok(plan[0]["Plan"]["Plans"][0]["Actual Rows"]
        .as_f64()
        .expect("a row count"))
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let client = playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE item (id int PRIMARY KEY, name text);
             INSERT INTO item SELECT i, 'item ' || i FROM generate_series(1, 100000) i;
             ANALYZE item;",
        )
        .await?;

    // Page 1,981, fifty rows a page, both ways.
    let offset = rows_scanned(
        &client,
        "EXPLAIN (ANALYZE, FORMAT JSON, COSTS OFF, TIMING OFF)
         SELECT * FROM item ORDER BY id LIMIT 50 OFFSET 99000",
    )
    .await?;
    let keyset = rows_scanned(
        &client,
        "EXPLAIN (ANALYZE, FORMAT JSON, COSTS OFF, TIMING OFF)
         SELECT * FROM item WHERE id > 99000 ORDER BY id LIMIT 50",
    )
    .await?;
    println!("OFFSET read {offset} rows from the index; keyset read {keyset}");
    assert_eq!((offset, keyset), (99_050.0, 50.0));

    // Walking the whole table: each page starts after the last id of the one before.
    let page = client
        .prepare("SELECT id FROM item WHERE id > $1 ORDER BY id LIMIT $2")
        .await?;
    let (mut after, mut pages) = (0i32, 0);
    loop {
        let rows = client.query(&page, &[&after, &1000i64]).await?;
        let Some(last) = rows.last() else { break };
        after = last.get(0);
        pages += 1;
    }
    assert_eq!((pages, after), (100, 100_000));
    Ok(())
}
```

```text
OFFSET read 99050 rows from the index; keyset read 50
```

(PostgreSQL 18 reports actual row counts with decimals, `99050.0` in the JSON, which is why
the helper reads an `f64`.)

Three rules make keyset pagination correct, not just fast:

- **The key must be unique.** If the sort column has duplicates (a price, a date), add a
  tie-breaker and compare row values: `WHERE (price, id) > ($1, $2) ORDER BY price, id`, with an
  index on `(price, id)`. Exercise 3 builds it.
- **The client gets an opaque cursor, not the key.** Encode the last key (base64 of JSON, for
  instance) so that clients cannot start building URLs from your column values, and so that
  you can change the key later.
- **Ask for one row more than the page.** If it comes back, there is a next page; drop it and
  return a cursor. If not, this was the last page, and no empty request is needed to find out.

Unlike a portal, keyset pages are separate queries with separate snapshots: a row inserted
between two requests appears if it sorts after the cursor. For an HTTP API, that is usually
what you want.

## 9.7 `simple_query_raw`

The simple protocol (Chapter 2) has a streaming form too.
[`simple_query_raw`](https://docs.rs/tokio-postgres/0.7.18/tokio_postgres/struct.Client.html#method.simple_query_raw)
returns a `SimpleQueryStream` of the same `SimpleQueryMessage`s that `simple_query` collects
into a vector, for any number of statements:

```rust
use futures_util::TryStreamExt;
use std::pin::pin;
use tokio_postgres::SimpleQueryMessage;

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = playground::connect().await?;
    let messages = client
        .simple_query_raw("SELECT generate_series(1, 3) AS n; SELECT 'done' AS status")
        .await?;
    let mut messages = pin!(messages);
    let mut seen = Vec::new();
    while let Some(message) = messages.try_next().await? {
        seen.push(match message {
            SimpleQueryMessage::RowDescription(columns) => format!("columns {}", columns[0].name()),
            SimpleQueryMessage::Row(row) => format!("row {}", row.get(0).unwrap_or("NULL")),
            SimpleQueryMessage::CommandComplete(n) => format!("complete {n}"),
            _ => "something new".to_owned(), // the enum is #[non_exhaustive]
        });
    }
    assert_eq!(
        seen,
        [
            "columns n",
            "row 1",
            "row 2",
            "row 3",
            "complete 3",
            "columns status",
            "row done",
            "complete 1",
        ]
    );
    Ok(())
}
```

It is the tool for programs that run SQL they did not write, the way psql does. pgq's `--stream`
mode is built on it, and prints each row as soon as it arrives:

~~~rust
// apps/pgq/src/lib.rs (excerpt)
pub async fn stream(client: &Client, sql: &str, mut print: impl FnMut(&str)) -> Result<u64, Error> {
    let mut messages = pin!(client.simple_query_raw(sql).await?);
    let mut rows = 0;
    while let Some(message) = messages.try_next().await? {
        match message {
            SimpleQueryMessage::RowDescription(columns) => {
                let names: Vec<&str> = columns.iter().map(|c| c.name()).collect();
                print(&names.join("\t"));
            }
            SimpleQueryMessage::Row(row) => {
                let cells: Vec<&str> = (0..row.len()).map(|i| row.get(i).unwrap_or("")).collect();
                print(&cells.join("\t"));
                rows += 1;
            }
            _ => {}
        }
    }
    Ok(rows)
}
~~~

[apps/pgq/src/lib.rs](../apps/pgq/src/lib.rs)

Everything in §9.3 applies unchanged: the same channel, the same stall.

### Side by side — *iterating over a million rows*

The task: add up a million values without holding them all in memory.

```sql
-- psql prints the rows in groups of FETCH_COUNT as they arrive, instead of collecting the
-- whole result first. (psql 18 declares no cursor for this: pg_cursors stays empty.)
tpbook=# \set FETCH_COUNT 1000
tpbook=# \o /dev/null
tpbook=# SELECT generate_series(1, 1000000) AS n;
tpbook=# \o
-- psql cannot add them up itself; the server can, and sends one row:
tpbook=# SELECT sum(n) FROM generate_series(1, 1000000) n;
     sum
--------------
 500000500000
(1 row)
```

~~~rust
// sqlx (illustration)
use futures_util::TryStreamExt;
let mut rows = sqlx::query_scalar::<_, i32>("SELECT generate_series(1, 1000000)").fetch(&pool);
let mut sum = 0i64;
while let Some(n) = rows.try_next().await? {
    sum += i64::from(n);
}
~~~

```rust
use futures_util::TryStreamExt;

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = playground::connect().await?;
    let rows = client
        .query_raw(
            "SELECT generate_series(1, 1000000)",
            playground::slice_iter(&[]),
        )
        .await?;
    // try_fold takes the stream by value and pins it itself.
    let sum = rows
        .try_fold(0i64, |sum, row| async move {
            Ok(sum + i64::from(row.get::<_, i32>(0)))
        })
        .await?;
    assert_eq!(sum, 500_000_500_000);
    Ok(())
}
```

| | psql | sqlx | tokio-postgres |
|---|---|---|---|
| **Same** | Rows are handled in groups as they arrive; memory stays flat | `fetch` returns a stream of rows | `query_raw` returns a stream of rows |
| **Different** | A display setting, `FETCH_COUNT`, rather than an API | The stream borrows its executor (`&pool` here), and a pool lends it a connection of its own | The stream borrows nothing, and stalls its connection until it is read or dropped |

The psql line deserves its comment: the best way to add up a million values is to let the
server do it and send one row. Stream when the client really needs every row.

---

## In stockroom

`GET /products/stream` sends the whole catalog as newline-delimited JSON, one line per row,
with memory use that does not grow with the catalog. The handler takes a connection from the
pool, starts the query, and hands the stream to the response body:

~~~rust
// stockroom/src/http/products.rs (excerpt)
pub async fn stream(Db(db): Db) -> Result<Response, AppError> {
    let stmt = db.prepare_cached(products::ALL_IN_SKU_ORDER).await?;
    let rows = db.query_raw(&stmt, db::slice_iter(&[])).await?;
    let body = Body::from_stream(ndjson(db, rows));
    Ok(([(header::CONTENT_TYPE, "application/x-ndjson")], body).into_response())
}

/// The body stream owns the pooled connection, so the connection stays checked out until the
/// last row is sent. `RowStream` does not borrow the client: had `db` gone back to the pool
/// when the handler returned, the next borrower's queries would queue behind these rows.
fn ndjson(db: Object, rows: RowStream) -> impl Stream<Item = Result<Bytes, AppError>> + Send {
    async_stream::try_stream! {
        let _checked_out = db;
        let mut rows = pin!(rows);
        while let Some(row) = rows.try_next().await? {
            let mut line = serde_json::to_vec(&Product::try_from(&row)?)?;
            line.push(b'\n');
            yield Bytes::from(line);
        }
    }
}
~~~

[stockroom/src/http/products.rs](../stockroom/src/http/products.rs)

The line `let _checked_out = db;` is the whole of §9.3's advice in one statement. The handler
returns as soon as the query has started, long before the last row is sent. If `db`, the pooled
connection, were dropped with the handler, it would go back to the pool with a stream still
running on it, and the next request to borrow it would wait behind the rest of the catalog.
Moving it into the body's stream keeps it checked out until the last row is written, or until
the client hangs up and axum drops the body. The underscore name keeps the value alive; a bare
`_` would drop it on the spot. And the backpressure works end to end: a slow HTTP client slows
the body, which slows the stream, which leaves the server waiting in `ClientWrite`, all without
buffering.

`stockroom export products` reads the same catalog through a portal instead, in batches of
`--batch` rows (500 by default), inside the transaction mode made for long reads:

~~~rust
// stockroom/src/cli/export.rs (excerpt)
/// A portal lives in a transaction. `SERIALIZABLE, READ ONLY, DEFERRABLE` is the one made
/// for exports: it waits, if needed, for a moment when a consistent snapshot is guaranteed,
/// and then can never fail with a serialization error, nor slow anyone else down.
async fn products(
    client: &mut Client,
    batch: i32,
    out: &mut (impl AsyncWrite + Unpin),
) -> anyhow::Result<u64> {
    let tx = client
        .build_transaction()
        .isolation_level(IsolationLevel::Serializable)
        .read_only(true)
        .deferrable(true)
        .start()
        .await?;
    let statement = tx.prepare(ALL_IN_SKU_ORDER).await?;
    let portal = tx.bind(&statement, &[]).await?;
    let mut total = 0;
    loop {
        let rows = tx.query_portal(&portal, batch).await?;
        for row in &rows {
            let mut line = serde_json::to_vec(&Product::try_from(row)?)?;
            line.push(b'\n');
            out.write_all(&line).await?;
        }
        total += rows.len() as u64;
        if rows.len() < batch as usize {
            break; // a short batch is the last one
        }
    }
    tx.commit().await?;
    Ok(total)
}
~~~

[stockroom/src/cli/export.rs](../stockroom/src/cli/export.rs)

Why a portal here and a stream in the HTTP handler? The export writes to a file, so one round
trip per 500 rows is noise, and the batches give it natural points to flush. The HTTP handler
wants the first byte out as soon as possible and lets TCP set the pace. Both hold one
connection for the whole job; the portal also holds one snapshot, which is what makes the
export consistent.

The paged listing, `GET /products?cursor=…&limit=…`, uses §9.6's keyset pagination, with
the one-extra-row trick and an opaque cursor:

~~~rust
// stockroom/src/repo/products.rs (excerpt)
let stmt = db
    .prepare_cached(
        "SELECT id, sku, name, price_cents, tags, attributes, version, created_at, updated_at
         FROM products
         WHERE sku > coalesce($1, '') AND ($2::text IS NULL OR tags @> ARRAY[$2])
         ORDER BY sku
         LIMIT $3",
    )
    .await?;
~~~

~~~rust
// stockroom/src/http/products.rs (excerpt)
// One more than asked for: if it comes back, there is a next page.
let mut items = products::page(&db, after.as_ref(), query.tag.as_deref(), limit + 1).await?;
let next_cursor = if items.len() as i64 > limit {
    items.truncate(limit as usize);
    items.last().map(|last| encode_cursor(&last.sku))
} else {
    None
};
~~~

[stockroom/src/repo/products.rs](../stockroom/src/repo/products.rs) ·
[stockroom/src/http/products.rs](../stockroom/src/http/products.rs)

`sku` is unique, so it needs no tie-breaker.

## Summary

- `query` collects a `RowStream` into a `Vec`. For large results, read the stream itself with
  `query_raw`: rows arrive one by one, and memory stays flat.
- The `_raw` methods take an `ExactSizeIterator` of `BorrowToSql` values. Use `slice_iter` for
  mixed parameters, a plain collection for parameters of one type, and
  `Vec<Box<dyn ToSql + Sync + Send>>` for parameters decided at run time.
- `RowStream` is `!Unpin`: pin it with `pin!` (on the stack, cannot be dropped early) or
  `Box::pin` (owned, movable). `rows_affected()` is `None` until the stream ends.
- An unread stream stalls its whole connection: the response channel is bounded at one, the
  connection stops reading, and the server waits in `ClientWrite`. Stream only on a connection
  that nothing else uses, and keep that connection with the stream.
- Dropping a stream does not stop the query; the server still produces every row. A portal
  (`bind`, `query_portal`) produces only the rows you ask for, a batch per round trip, inside a
  transaction, if the plan can produce rows lazily.
- `DECLARE … CURSOR` is the SQL form of a portal. Use it for `WITH HOLD`, `SCROLL`, or SQL-only
  callers; otherwise `bind` is simpler.
- Across HTTP requests, use keyset pagination: a unique key, an opaque cursor, one extra row to
  detect the last page. Its cost does not grow with the page number; `OFFSET`'s does.
- `simple_query_raw` streams the simple protocol's messages, statement after statement.

## Exercises

1. **Return a stream.** Write `async fn numbers(client: &Client, n: i32) -> Result<impl
   Stream<Item = Result<Row, Error>>, Error>` that streams `1..=n`, and use it from `main` with
   `try_next`.
   *Hint:* `RowStream` borrows nothing, so it can leave the function; `Box::pin` it so that the
   caller can call `try_next` without pinning.
2. **Batches from a cursor.** Rewrite §9.4's batch loop with `DECLARE` and `FETCH` instead of
   `bind` and `query_portal`. What does each batch cost in round trips, and how would you make
   it cost one?
   *Hint:* `FETCH 4 FROM numbers` is a statement; Chapter 4 counted what a statement given as
   text costs.
3. **A tie-breaker.** Create a temp table of 1,000 products with only ten distinct prices,
   then page through it in price order, 64 rows a page, with keyset pagination. Check that
   every product is seen exactly once.
   *Hint:* `WHERE (price, id) > ($1, $2) ORDER BY price, id`.
4. **The cost of a consistent export.** stockroom's export keeps one transaction open for the
   whole catalog. What does a long-lived snapshot cost the rest of the database? How would
   exporting with keyset pages change that, and what would the export lose?
   *Hint:* think of `VACUUM`, and of a product renamed halfway through the export.

### Solutions

**1.**

```rust
use futures_util::{Stream, TryStreamExt};
use tokio_postgres::{Client, Error, Row};

async fn numbers(
    client: &Client,
    n: i32,
) -> Result<impl Stream<Item = Result<Row, Error>> + Send + 'static, Error> {
    let rows = client
        .query_raw("SELECT generate_series(1, $1::int)", [n])
        .await?;
    Ok(Box::pin(rows))
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let client = playground::connect().await?;
    let mut rows = numbers(&client, 3).await?;
    let mut seen = Vec::new();
    while let Some(row) = rows.try_next().await? {
        seen.push(row.get::<_, i32>(0));
    }
    assert_eq!(seen, [1, 2, 3]);
    Ok(())
}
```

The `'static` in the return type is the point: the stream outlives the borrow of `client`. It
also means nothing stops the caller from holding it while using `client` for something else,
with §9.3's result.

**2.** As text, each `FETCH` is a prepare and an execute, two round trips per batch. Prepare it
once and each batch is one round trip, the same as `query_portal`:

```rust
#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let mut client = playground::connect().await?;
    let tx = client.transaction().await?;
    tx.batch_execute("DECLARE numbers CURSOR FOR SELECT generate_series(1, 10)")
        .await?;
    let fetch = tx.prepare("FETCH 4 FROM numbers").await?;
    let mut batches = Vec::new();
    loop {
        let rows = tx.query(&fetch, &[]).await?;
        batches.push(rows.len());
        if rows.len() < 4 {
            break;
        }
    }
    assert_eq!(batches, [4, 4, 2]);
    tx.commit().await?;
    Ok(())
}
```

On the wire, the two differ only in who names the portal and how the batch size travels:
`query_portal` sends an *Execute* with a row limit to a portal made by *Bind*; `FETCH` runs
a statement whose result is the next rows of a portal made by `DECLARE`.

**3.**

```rust
use std::collections::HashSet;

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE product (id int PRIMARY KEY, price int NOT NULL);
             INSERT INTO product SELECT i, (i % 10) * 100 FROM generate_series(1, 1000) i;
             CREATE INDEX ON product (price, id);",
        )
        .await?;
    let first = client
        .prepare("SELECT price, id FROM product ORDER BY price, id LIMIT $1")
        .await?;
    let next = client
        .prepare(
            "SELECT price, id FROM product WHERE (price, id) > ($1, $2)
             ORDER BY price, id LIMIT $3",
        )
        .await?;

    let mut seen = HashSet::new();
    let mut last: Option<(i32, i32)> = None;
    loop {
        let rows = match last {
            None => client.query(&first, &[&64i64]).await?,
            Some((price, id)) => client.query(&next, &[&price, &id, &64i64]).await?,
        };
        let Some(end) = rows.last() else { break };
        last = Some((end.get(0), end.get(1)));
        for row in &rows {
            assert!(seen.insert(row.get::<_, i32>(1)), "a product seen twice");
        }
    }
    assert_eq!(seen.len(), 1000);
    Ok(())
}
```

With `WHERE price > $1` alone, every page would skip the rest of the products sharing the last
price on the page before.

**4.** While the export's snapshot is open, `VACUUM` cannot remove any row version that the
snapshot might still see, in any table of the database: dead rows from every update and
delete accumulate until the export commits, and tables and indexes grow. With keyset pages,
each page is a short transaction and nothing is held between pages. What the export loses is
consistency: each page shows the catalog at a different moment, so a product whose SKU changes
halfway through can appear twice or not at all, and the file no longer describes one moment
in time. For a backup-like export, keep the snapshot; for a feed that is refreshed anyway,
page.

---

Previous: [Chapter 8 — Transactions and concurrency control](08-transactions.md) ·
Next: [Chapter 10 — Bulk data: COPY](10-copy.md)
