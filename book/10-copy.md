# Chapter 10 — Bulk data: COPY

> **Async PostgreSQL in Rust** · Previous: [Chapter 9 — Streaming rows and portals](09-streaming.md) ·
> Next: [Chapter 11 — LISTEN/NOTIFY and notices](11-listen-notify.md)

Loading twenty thousand rows with one `INSERT` each means twenty thousand statements: twenty
thousand times the server parses parameters, plans (or reuses a plan), executes and reports,
and, unless you pipeline them, twenty thousand round trips. `COPY` is PostgreSQL's answer for
moving many rows: one statement, after which the rows simply stream, in either direction, as
text, CSV or a binary format.

tokio-postgres exposes `COPY` as a `Sink` going in and a `Stream` coming out, plus a pair of
helpers for the binary format. This chapter covers all of them, what happens when a load fails
halfway, and how to turn a bulk load into an upsert. It also measures what `COPY` buys: on the same
machine, binary `COPY` loaded rows about a hundred times faster than one `INSERT` at a time,
and more than two hundred times faster across a 1 ms network delay.

---

## 10.1 `copy_in` and `CopyInSink`

[`Client::copy_in`](https://docs.rs/tokio-postgres/0.7.18/tokio_postgres/struct.Client.html#method.copy_in)
takes a `COPY … FROM STDIN` statement and returns a
[`CopyInSink<T>`](https://docs.rs/tokio-postgres/0.7.18/tokio_postgres/struct.CopyInSink.html),
a `futures` `Sink` that accepts any `T: Buf`, usually `Bytes`. You send it chunks of data with
`SinkExt::send`, then call `finish()`, which ends the COPY and returns the number of rows
loaded. Like `RowStream`, the sink is `!Unpin`, so you pin it first.

The data is one byte stream in the format the statement names. The chunks you send do not have
to end at row boundaries; the protocol says so
([COPY operations](https://www.postgresql.org/docs/18/protocol-flow.html#PROTOCOL-COPY)):
"The message boundaries are not required to have anything to do with row boundaries".

```rust
use bytes::Bytes;
use futures_util::SinkExt;
use std::pin::pin;

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE item (sku text PRIMARY KEY, name text, price_cents bigint)",
        )
        .await?;

    let sink = client
        .copy_in("COPY item (sku, name, price_cents) FROM STDIN (FORMAT csv, HEADER)")
        .await?;
    let mut sink = pin!(sink);
    // One byte stream: the first chunk ends in the middle of a row.
    for chunk in [
        "sku,name,price_cents\nA-1,\"Chair, oak\",4999\nA-2,\"Tab",
        "le \"\"Nordic\"\"\",12900\nA-3,Lamp,2500\n",
    ] {
        sink.send(Bytes::from(chunk)).await?;
    }
    let rows = sink.as_mut().finish().await?;
    assert_eq!(rows, 3);

    let name: String = client
        .query_one("SELECT name FROM item WHERE sku = 'A-2'", &[])
        .await?
        .get(0);
    assert_eq!(name, "Table \"Nordic\"");
    Ok(())
}
```

Chunk size is yours to choose. In 0.7.18 the sink gathers chunks of 4 KiB or less until their
total passes 4 KiB, but `SinkExt::send` flushes after every item, and a flush sends whatever
has been gathered (`copy_in.rs`). So `send` makes one protocol message per chunk, however
small. If you produce many small pieces, one per row for instance, either `feed` them and call
`finish` (or `flush`) at the end, or build larger chunks yourself, as pgload does with 64 KiB of
CSV at a time. The binary writer of §10.3 batches its rows on its own.

The text and CSV formats come with the usual escaping rules, which the next section shows
on the way out. Use a CSV library (pgload and stockroom use the `csv` crate) rather than
`format!` and hope.

### `finish`, or nothing

Until `finish()`, nothing has been committed to the table. That is a feature: if your program
fails halfway through producing the data, the half that was sent is thrown away. Dropping the
sink without calling `finish()` sends a *CopyFail* message, and the server aborts the `COPY`:

```rust
use bytes::Bytes;
use futures_util::SinkExt;
use std::pin::pin;
use tokio_postgres::error::SqlState;

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let mut client = playground::connect().await?;
    client
        .batch_execute("CREATE TEMP TABLE item (id int, name text)")
        .await?;
    {
        let sink = client.copy_in("COPY item FROM STDIN (FORMAT csv)").await?;
        let mut sink = pin!(sink);
        sink.send(Bytes::from_static(b"1,chair\n2,table\n")).await?;
        // No finish(): the sink is dropped at the end of this block.
    }
    let count: i64 = client
        .query_one("SELECT count(*) FROM item", &[])
        .await?
        .get(0);
    assert_eq!(count, 0); // aborted; the connection is fine

    // In a transaction, the aborted COPY is an error like any other: the transaction is over.
    let tx = client.transaction().await?;
    {
        let sink = tx.copy_in("COPY item FROM STDIN (FORMAT csv)").await?;
        let mut sink = pin!(sink);
        sink.send(Bytes::from_static(b"1,chair\n")).await?;
    }
    let err = tx
        .query_one("SELECT count(*) FROM item", &[])
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some(&SqlState::IN_FAILED_SQL_TRANSACTION)); // 25P02
    Ok(())
}
```

> **Pitfall:** a `?` between the sends and `finish()` is a silent abort. The function returns
> the error, the sink is dropped, and the rows you thought you had loaded are gone. That is
> usually what you want, but make sure the error is logged rather than swallowed, since the
> COPY itself reports nothing.

### A COPY that fails to start loses the connection

One failure mode of `copy_in` is not like the others, and it is worth knowing before it finds
you. If the server rejects the `COPY` *as it starts*, because the table or a column does not
exist, or an option is not allowed, then in 0.7.18 the connection does not survive:

```rust
use bytes::Bytes;

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = playground::connect().await?;
    let Err(err) = client
        .copy_in::<_, Bytes>("COPY no_such_table FROM STDIN")
        .await
    else {
        panic!("the table does not exist");
    };
    println!(
        "copy_in: {}",
        err.as_db_error().expect("a server error").message()
    );

    // The next request on this connection fails, and so does every one after it.
    let next = client.query_one("SELECT 1", &[]).await;
    println!(
        "the next query: {}",
        next.as_ref()
            .map(|_| "ok".into())
            .unwrap_or_else(|e| e.to_string())
    );
    assert!(next.is_err());
    Ok(())
}
```

```text
copy_in: relation "no_such_table" does not exist
the next query: unexpected message from server
```

The next query receives a response that belongs to no request, and the connection task ends
with the same error; every request after that fails with "connection closed".

> **Protocol lens:** `copy_in` sends *Bind*, *Execute* and *Sync* together, then waits for the
> server to enter copy-in mode. When the server answers with an error instead, it consumes the
> *Sync* and sends *ReadyForQuery*. Meanwhile `copy_in` returns the error and drops its side of
> the copy, and the connection, as for any abandoned copy, sends *CopyFail* and another *Sync*.
> The server, no longer in copy mode, has nothing to abort, and answers the second *Sync* with a
> second *ReadyForQuery* that nobody asked for. The connection task, which matches responses to
> requests strictly in order, gives up with "unexpected message from server". The design
> problem is tracked as [rust-postgres issue #789](https://github.com/rust-postgres/rust-postgres/issues/789).

Errors *during* the copy (a malformed line, a value of the wrong type, a header that does not
match) and errors from `finish()` leave the connection healthy; §10.4 relies on that. So the
defence is about the start:

- **Copy into a table you just created**, in the same transaction, with the columns you
  name. That is the staging-table pattern of §10.4 and §10.6, and it leaves nothing to reject
  at the start.
- **Treat an error from `copy_in` itself as the loss of the connection.** On a connection of
  your own, reconnect. On a pooled one, do not give it back for the next request to trip over:
  detach it and drop it (deadpool's `Object::take`, Chapter 13).

## 10.2 `copy_out` and `CopyOutStream`

[`copy_out`](https://docs.rs/tokio-postgres/0.7.18/tokio_postgres/struct.Client.html#method.copy_out)
takes a `COPY … TO STDOUT` statement and returns a `CopyOutStream`, a `Stream` of
`Result<Bytes, Error>`. The server sends "CopyData messages (always one per row)", as the
protocol documentation puts it, so each chunk is one row. Any query can be the source, with
`COPY (SELECT …) TO STDOUT`.

The example also shows how CSV and the default text format write the awkward values, which
matters as soon as someone reads the file back:

```rust
use bytes::Bytes;
use futures_util::TryStreamExt;

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE note (id int, body text);
             INSERT INTO note VALUES (1, NULL), (2, ''), (3, 'a,b'), (4, 'say \"hi\"');",
        )
        .await?;

    let chunks: Vec<Bytes> = client
        .copy_out("COPY (SELECT * FROM note ORDER BY id) TO STDOUT (FORMAT csv)")
        .await?
        .try_collect()
        .await?;
    assert_eq!(chunks.len(), 4); // one chunk per row
    let csv = String::from_utf8(chunks.concat()).expect("UTF-8");
    print!("CSV:\n{csv}");
    assert_eq!(csv, "1,\n2,\"\"\n3,\"a,b\"\n4,\"say \"\"hi\"\"\"\n");

    let chunks: Vec<Bytes> = client
        .copy_out("COPY (SELECT * FROM note ORDER BY id) TO STDOUT")
        .await?
        .try_collect()
        .await?;
    print!(
        "text:\n{}",
        String::from_utf8(chunks.concat()).expect("UTF-8")
    );
    Ok(())
}
```

```text
CSV:
1,
2,""
3,"a,b"
4,"say ""hi"""
text:
1	\N
2	
3	a,b
4	say "hi"
```

In CSV, `NULL` is an empty field and the empty string is `""`; in the text format, `NULL` is
`\N` and the empty string is nothing at all. Both read back exactly as they were written, but
only by `COPY` with the same format: a spreadsheet, or another program's CSV reader, may well
turn `""` into an empty field and lose the difference. Decide what "no value" means in your
file and say it in the query (stockroom exports "no tags" as `NULL`, with `nullif`).

Everything Chapter 9 said about `RowStream` holds for `CopyOutStream`: it borrows nothing, and
reading it slowly holds up the connection. Treat a large `COPY … TO STDOUT` as a stream that
owns its connection.

## 10.3 Binary COPY

In the binary format, every value travels in the same binary encoding that query parameters
and results use: no number formatting, no quoting, no parsing. tokio-postgres wraps both
directions in
[`binary_copy`](https://docs.rs/tokio-postgres/0.7.18/tokio_postgres/binary_copy/index.html):

- `BinaryCopyInWriter::new(sink, &[Type])` takes the sink and the type of each column, in the
  statement's column order. Pinned, it accepts one row at a time with
  `write(&[&dyn ToSql + Sync])` (or `write_raw` with an iterator), and `finish()` returns the
  row count.
- `BinaryCopyOutStream::new(stream, &[Type])` turns a `copy_out` stream into a stream of
  `BinaryCopyOutRow`s, read with `get` and `try_get` by column index.

```rust
use futures_util::TryStreamExt;
use jiff::Timestamp;
use std::pin::pin;
use tokio_postgres::binary_copy::{BinaryCopyInWriter, BinaryCopyOutStream};
use tokio_postgres::types::Type;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE reading (sensor text, at timestamptz, value float8, tags text[])",
        )
        .await?;
    let types = [
        Type::TEXT,
        Type::TIMESTAMPTZ,
        Type::FLOAT8,
        Type::TEXT_ARRAY,
    ];

    // In: the column types once, then the rows.
    let sink = client
        .copy_in("COPY reading FROM STDIN (FORMAT binary)")
        .await?;
    let writer = BinaryCopyInWriter::new(sink, &types);
    let mut writer = pin!(writer);
    let tags = vec!["lab".to_owned()];
    for i in 0..1000i32 {
        let at = Timestamp::from_second(1_700_000_000 + i64::from(i))?;
        let value = f64::from(i) / 3.0;
        writer.as_mut().write(&[&"t-1", &at, &value, &tags]).await?;
    }
    assert_eq!(writer.as_mut().finish().await?, 1000);

    // Out: the same types, each row decoded as it arrives.
    let stream = client
        .copy_out("COPY (SELECT * FROM reading ORDER BY at) TO STDOUT (FORMAT binary)")
        .await?;
    let rows: Vec<_> = BinaryCopyOutStream::new(stream, &types)
        .try_collect()
        .await?;
    assert_eq!(rows.len(), 1000);
    let second = &rows[1];
    assert_eq!(second.get::<&str>(0), "t-1");
    assert_eq!(
        second.get::<Timestamp>(1),
        Timestamp::from_second(1_700_000_001)?
    );
    assert_eq!(second.get::<f64>(2), 1.0 / 3.0);
    assert_eq!(second.get::<Vec<String>>(3), ["lab"]);
    Ok(())
}
```

Binary `COPY` is the fastest of the formats (§10.5), and the types are exact on both sides. The
price is that nobody checks your list of types against the table. Three mistakes, three
outcomes:

```rust
use std::pin::pin;
use tokio_postgres::binary_copy::BinaryCopyInWriter;
use tokio_postgres::error::SqlState;
use tokio_postgres::types::Type;

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = playground::connect().await?;
    client
        .batch_execute("CREATE TEMP TABLE counter (n bigint); CREATE TEMP TABLE gauge (x real)")
        .await?;

    // INT4 declared for a bigint column: the client cannot know. The server reads four bytes
    // where it expects eight.
    {
        let sink = client
            .copy_in("COPY counter FROM STDIN (FORMAT binary)")
            .await?;
        let writer = BinaryCopyInWriter::new(sink, &[Type::INT4]);
        let mut writer = pin!(writer);
        writer.as_mut().write(&[&5i32]).await?;
        let err = writer.as_mut().finish().await.unwrap_err();
        let db = err.as_db_error().expect("a server error");
        println!(
            "server: {} ({})",
            db.message(),
            db.where_().unwrap_or_default()
        );
        assert_eq!(db.code(), &SqlState::PROTOCOL_VIOLATION); // 08P01
    }

    // INT8 declared, but an i32 given: the client refuses to encode it. (The block matters:
    // the writer must be dropped, aborting its COPY, before the connection can move on.)
    {
        let sink = client
            .copy_in("COPY counter FROM STDIN (FORMAT binary)")
            .await?;
        let writer = BinaryCopyInWriter::new(sink, &[Type::INT8]);
        let mut writer = pin!(writer);
        let err = writer.as_mut().write(&[&5i32]).await.unwrap_err();
        println!("client: {err}");
        assert!(err.as_db_error().is_none());
    }
    let count: i64 = client
        .query_one("SELECT count(*) FROM counter", &[])
        .await?
        .get(0);
    assert_eq!(count, 0);

    // INT4 declared for a `real` column: four bytes where four are expected. Nobody notices,
    // and the bits of the integer 5 are stored as a floating-point number.
    {
        let sink = client
            .copy_in("COPY gauge FROM STDIN (FORMAT binary)")
            .await?;
        let writer = BinaryCopyInWriter::new(sink, &[Type::INT4]);
        let mut writer = pin!(writer);
        writer.as_mut().write(&[&5i32]).await?;
        assert_eq!(writer.as_mut().finish().await?, 1);
    }
    let stored: f32 = client.query_one("SELECT x FROM gauge", &[]).await?.get(0);
    println!("stored: {stored:e}");
    assert!(stored > 0.0 && stored < 1e-40);
    Ok(())
}
```

```text
server: insufficient data left in message (COPY counter, line 1, column n)
client: error serializing parameter 0
stored: 7e-45
```

The client checks each Rust value against the type you declared, with the same `ToSql::accepts`
that guards query parameters (Chapter 3). Nothing checks the declared type against the column:
the server decodes each field with the column's own receive function, and finds out only when
the bytes do not fit. When they do fit, as the four bytes of an `int4` do in a `real`, the load
succeeds with wrong data. Get the types from the table definition, keep the list next to the
`COPY` statement in column order, and test the load by reading a row back. A domain column,
such as stockroom's `sku`, takes its base type (`TEXT`).

## 10.4 When a load fails halfway

By default `COPY` is all or nothing: one bad line, and nothing is loaded. The error says where,
in `DbError::where_()` (Chapter 6), which is what makes it useful to whoever sent the file.
PostgreSQL 17 added `ON_ERROR ignore`, which skips the lines whose values do not convert to the
column types, and PostgreSQL 18 added `REJECT_LIMIT`, which puts a ceiling on how many may be
skipped:

```rust
use bytes::Bytes;
use futures_util::SinkExt;
use std::pin::pin;
use tokio_postgres::error::SqlState;
use tokio_postgres::{Client, Error};

const CSV: &str = "1,chair\n2,table\nthree,lamp\n4,desk\nfive,shelf\n";

/// Runs one `COPY … FROM STDIN` with `data`; returns the rows loaded.
async fn load(client: &Client, copy: &str, data: &'static str) -> Result<u64, Error> {
    let sink = client.copy_in(copy).await?;
    let mut sink = pin!(sink);
    sink.send(Bytes::from_static(data.as_bytes())).await?;
    sink.as_mut().finish().await
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let client = playground::connect().await?;
    client
        .batch_execute("CREATE TEMP TABLE item (id int, name text)")
        .await?;

    // The default, ON_ERROR stop: nothing is loaded, and the error names the line.
    let err = load(&client, "COPY item FROM STDIN (FORMAT csv)", CSV)
        .await
        .unwrap_err();
    let db = err.as_db_error().expect("a server error");
    println!(
        "{}\n  where: {}",
        db.message(),
        db.where_().unwrap_or_default()
    );
    assert_eq!(db.code(), &SqlState::INVALID_TEXT_REPRESENTATION); // 22P02

    // ON_ERROR ignore (PG 17+): the two bad lines are skipped.
    let copy = "COPY item FROM STDIN (FORMAT csv, ON_ERROR ignore)";
    assert_eq!(load(&client, copy, CSV).await?, 3);

    // REJECT_LIMIT (PG 18+): skipping is fine, up to a point.
    let copy = "COPY item FROM STDIN (FORMAT csv, ON_ERROR ignore, REJECT_LIMIT 1)";
    let err = load(&client, copy, CSV).await.unwrap_err();
    println!("{}", err.as_db_error().expect("a server error").message());

    let count: i64 = client
        .query_one("SELECT count(*) FROM item", &[])
        .await?
        .get(0);
    assert_eq!(count, 3); // only the ON_ERROR ignore load went in
    Ok(())
}
```

```text
invalid input syntax for type integer: "three"
  where: COPY item, line 3, column id: "three"
skipped more than REJECT_LIMIT (1) rows due to data type incompatibility
```

`ON_ERROR ignore` has limits worth knowing before you rely on it:

- It skips **data type errors only**. A line with an extra column is still fatal
  ("extra data after last expected column", 22P04), and so is a constraint violation: a
  duplicate key or a failed `CHECK` aborts the whole `COPY`.
- It is **not allowed with the binary format**, and asking for it fails as the `COPY` starts
  ("only ON_ERROR STOP is allowed in BINARY mode", 42601), which is exactly §10.1's
  connection-losing case.
- It reports the skipped lines as **notices**: a summary ("2 rows were skipped due to data type
  incompatibility"), and one notice per line with `LOG_VERBOSITY verbose`. With
  `playground::connect`, notices go to the `log` crate and nobody sees them (Chapter 6);
  Chapter 11 shows how to receive them.

### The staging table

When the rows come from outside (an upload, a partner's export), the robust pattern loads them
first into a temporary table that cannot reject anything, and then moves them with SQL:

1. `CREATE TEMP TABLE … ON COMMIT DROP`, inside the transaction, with loose types (often
   `text`) and no constraints. The `COPY` cannot fail to start, and fails only on lines that are
   not CSV at all.
2. `COPY` into it.
3. Validate with queries: duplicates, unknown references, values that do not convert
   (`pg_input_is_valid`, PG 16+, answers "would this text convert to that type?" without an
   error). Report everything wrong at once, which a single `COPY` error cannot do.
4. Move the rows into the real table with `INSERT … SELECT`, `INSERT … ON CONFLICT` or
   `MERGE` (§10.6), converting types on the way.

The real table is touched only by step 4, in one statement, in the same transaction: the
upload goes in whole, or not at all.

## 10.5 What COPY buys: the pgload table

`pgload` (in `apps/pgload`) loads the same 20,000 generated rows, with commas and quotes in
them, in five ways, and checks that all five store identical data:

| Strategy | Statements | What travels |
|---|---|---|
| `INSERT`, one at a time | 20,000, each awaited | 20,000 round trips |
| `INSERT`, pipelined | 20,000, in flight together (Chapter 7) | about one round trip per window |
| `INSERT … SELECT FROM unnest(…)` | 20, of 1,000 rows each | one array per column |
| `COPY`, CSV | 1 | the rows, as CSV |
| `COPY`, binary | 1 | the rows, in binary |

The `unnest` strategy deserves a look, because it is the fastest way to load rows without
`COPY`, and so the one to use where `COPY` does not fit (inside a larger statement, or with
`RETURNING`). Each column travels as one array parameter, and `unnest` zips the arrays back
into rows:

~~~rust
// apps/pgload/src/lib.rs (excerpt)
async fn unnest(tx: &Transaction<'_>, rows: &[Row]) -> Result<(), Error> {
    // …
             SELECT * FROM unnest($1::bigint[], $2::text[], $3::text[], $4::bigint[], $5::timestamptz[])",
    // …
        tx.execute(&statement, &[&ids, &skus, &names, &prices, &times])
            .await?;
    // …
}
~~~

[apps/pgload/src/lib.rs](../apps/pgload/src/lib.rs)

Measured with a release build, PG 18.6 on localhost, on Apple Silicon. The second run goes
through an in-process lagproxy with a 1 ms delay each way, with ten times fewer rows (the first
strategy would take minutes otherwise):

```bash
cargo run -p pgload --release -- --rows 20000
cargo run -p pgload --release -- --rows 2000 --delay-ms 1
```

```text
20000 rows, no added one-way delay

| Strategy | Time | Rows per second | Relative |
|---|---:|---:|---:|
| INSERT, one at a time | 0.761 s | 26283 | 103.0× |
| INSERT, pipelined | 0.118 s | 169524 | 16.0× |
| INSERT … unnest, batches | 0.012 s | 1607792 | 1.7× |
| COPY, CSV | 0.011 s | 1773115 | 1.5× |
| COPY, binary | 0.007 s | 2707215 | 1.0× |

2000 rows, 1 ms one-way delay

| Strategy | Time | Rows per second | Relative |
|---|---:|---:|---:|
| INSERT, one at a time | 9.663 s | 207 | 220.1× |
| INSERT, pipelined | 0.200 s | 9978 | 4.6× |
| INSERT … unnest, batches | 0.047 s | 42650 | 1.1× |
| COPY, CSV | 0.052 s | 38655 | 1.2× |
| COPY, binary | 0.044 s | 45547 | 1.0× |
```

What the numbers say:

- **Round trips dominate as soon as there is a network.** One `INSERT` at a time went from
  100× slower than binary `COPY` on localhost to 220× slower with 2 ms per round trip.
  A millisecond or two per round trip is common between a service and its database.
- **Pipelining recovers most of it, not all.** It removes the waiting, but the server still
  runs one statement per row.
- **Statements are the next cost.** `unnest` batches send the same data in one statement per
  1,000 rows and come within a factor of two of `COPY`; across the network, `unnest` and both
  `COPY` formats are close, because each is now a handful of round trips plus the bytes
  themselves.
- **Binary beats CSV** by not formatting and parsing text. The margin is clear on localhost and
  shrinks once the network dominates.

So: for thousands of rows or more, use `COPY`. For tens to hundreds, or when you need
`RETURNING` or `ON CONFLICT` in the same statement, `unnest` is nearly as good and much simpler.
Avoid looping over single `INSERT`s; if you must, pipeline them.

## 10.6 COPY into a temp table, then upsert

`COPY` only inserts. To load a file that both adds new rows and changes existing ones, combine
the staging table with an upsert. `MERGE` (PG 15+) says what to do in each case, and since
PostgreSQL 17 it can return `merge_action()` for each row it touched, which turns into a report
for free:

```rust
use bytes::Bytes;
use futures_util::SinkExt;
use std::pin::pin;
use tokio_postgres::error::SqlState;
use tokio_postgres::{Client, Error};

const MERGE: &str = "MERGE INTO product AS p
     USING price_import AS i ON p.sku = i.sku
     WHEN MATCHED AND p.price_cents <> i.price_cents THEN
         UPDATE SET price_cents = i.price_cents
     WHEN NOT MATCHED THEN
         INSERT VALUES (i.sku, i.price_cents)
     RETURNING merge_action()";

/// Loads a price list into `product`; returns what happened to each changed row.
async fn import(client: &mut Client, csv: &'static [u8]) -> Result<Vec<String>, Error> {
    let tx = client.transaction().await?;
    tx.batch_execute(
        "CREATE TEMP TABLE price_import (sku text, price_cents bigint) ON COMMIT DROP",
    )
    .await?;
    let sink = tx
        .copy_in("COPY price_import FROM STDIN (FORMAT csv)")
        .await?;
    let mut sink = pin!(sink);
    sink.send(Bytes::from_static(csv)).await?;
    sink.as_mut().finish().await?;

    let mut actions: Vec<String> = tx
        .query(MERGE, &[])
        .await?
        .iter()
        .map(|row| row.get(0))
        .collect();
    tx.commit().await?;
    actions.sort();
    Ok(actions)
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let mut client = playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE product (sku text PRIMARY KEY, price_cents bigint NOT NULL);
             INSERT INTO product VALUES ('A-1', 4999), ('A-2', 12900);",
        )
        .await?;

    // A-1 unchanged, A-2 repriced, A-3 new.
    let actions = import(&mut client, b"A-1,4999\nA-2,11900\nA-3,2500\n").await?;
    assert_eq!(actions, ["INSERT", "UPDATE"]);

    // The same new SKU twice: the second INSERT collides with the first.
    let err = import(&mut client, b"A-4,100\nA-4,200\n")
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some(&SqlState::UNIQUE_VIOLATION)); // 23505

    // The same existing SKU twice: one row cannot be changed twice by one statement.
    let err = import(&mut client, b"A-1,100\nA-1,200\n")
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some(&SqlState::CARDINALITY_VIOLATION)); // 21000
    println!("{}", err.as_db_error().expect("a server error").message());

    // Neither failed import changed anything.
    let total: i64 = client
        .query_one("SELECT sum(price_cents)::bigint FROM product", &[])
        .await?
        .get(0);
    assert_eq!(total, 4999 + 11900 + 2500);
    Ok(())
}
```

```text
MERGE command cannot affect row a second time
```

The duplicate cases are the ones that surprise. A key listed twice in the file is a
*cardinality violation* when the key exists, and a *unique violation* when it is new, because
`MERGE` decides "not matched" for both rows before inserting either. `INSERT … ON CONFLICT DO
UPDATE` fails with the cardinality error in both cases ("ON CONFLICT DO UPDATE command cannot
affect row a second time"). The cardinality error does not name the key (the unique
violation's `DETAIL` does: "Key (sku)=(A-4) already exists."). So check the staging table for
duplicates first and report them by name, as stockroom does below, or decide which line wins
(Exercise 4).

### Side by side — *a bulk load from a CSV file*

The task: load a CSV file with a header line into a table.

```sql
-- \copy reads the file on the client and streams it to the server. (Plain COPY … FROM
-- 'items.csv' would ask the *server* to open a file on its own disk.)
tpbook=# CREATE TEMP TABLE item (sku text, name text, price_cents bigint);
CREATE TABLE
tpbook=# \copy item FROM 'items.csv' (FORMAT csv, HEADER)
COPY 3
```

~~~rust
// sqlx (illustration)
let mut conn = pool.acquire().await?;
let mut copy = conn
    .copy_in_raw("COPY item FROM STDIN (FORMAT csv, HEADER)")
    .await?;
copy.read_from(tokio::fs::File::open("items.csv").await?).await?;
let rows = copy.finish().await?;
~~~

```rust
use bytes::BytesMut;
use futures_util::SinkExt;
use std::pin::pin;
use tokio::io::AsyncReadExt;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::temp_dir().join(format!("items-{}.csv", std::process::id()));
    tokio::fs::write(
        &path,
        "sku,name,price_cents\nA-1,\"Chair, oak\",4999\nA-2,\"Table \"\"Nordic\"\"\",12900\nA-3,Lamp,2500\n",
    )
    .await?;

    let client = playground::connect().await?;
    client
        .batch_execute("CREATE TEMP TABLE item (sku text, name text, price_cents bigint)")
        .await?;
    let sink = client
        .copy_in("COPY item FROM STDIN (FORMAT csv, HEADER)")
        .await?;
    let mut sink = pin!(sink);
    let mut file = tokio::fs::File::open(&path).await?;
    let mut buf = BytesMut::new();
    loop {
        buf.reserve(64 * 1024);
        if file.read_buf(&mut buf).await? == 0 {
            break;
        }
        sink.send(buf.split().freeze()).await?;
    }
    let rows = sink.as_mut().finish().await?;
    tokio::fs::remove_file(&path).await?;
    assert_eq!(rows, 3);
    Ok(())
}
```

| | psql | sqlx | tokio-postgres |
|---|---|---|---|
| **Same** | One `COPY … FROM STDIN`; the file streams through the client | One `COPY … FROM STDIN`; data sent in chunks, row count at the end | One `COPY … FROM STDIN`; data sent in chunks, row count at the end |
| **Different** | A meta-command that names a local file | Raw bytes only, from slices or any `AsyncRead`; `finish` or `abort` must be called, or the connection reports an error the next time it is used | A `Sink` of any `Buf`, plus a typed writer for binary rows; dropping the sink aborts the `COPY` |

---

## In stockroom

`POST /products/import` takes a CSV price list as the request body and streams it, chunk by
chunk as it arrives, into a staging table. Then it checks for duplicates and merges:

~~~rust
// stockroom/src/repo/products.rs (excerpt)
pub async fn import_csv<S, E>(tx: &Transaction<'_>, chunks: S) -> Result<ImportReport, AppError>
where
    S: Stream<Item = Result<Bytes, E>>,
    E: Display,
{
    tx.batch_execute(
        "CREATE TEMP TABLE product_import (sku text, name text, price_cents bigint, tags text)
         ON COMMIT DROP",
    )
    .await?;

    // HEADER MATCH (PG 15+): the first line must name exactly these columns.
    let sink = tx
        .copy_in("COPY product_import (sku, name, price_cents, tags) FROM STDIN (FORMAT csv, HEADER MATCH)")
        .await?;
    let mut sink = pin!(sink);
    let mut chunks = pin!(chunks);
    while let Some(chunk) = chunks.next().await {
        let chunk = chunk.map_err(|e| AppError::Invalid {
            field: None,
            message: format!("could not read the upload: {e}"),
        })?;
        sink.send(chunk).await.map_err(input_error)?;
    }
    // Without `finish`, dropping the sink would abort the COPY.
    let rows = sink.as_mut().finish().await.map_err(input_error)?;

    // A SKU listed twice would make MERGE insert it twice (a unique violation) or update it
    // twice (a cardinality violation). Naming the SKU is kinder than either.
    let twice = tx
        .query_opt(
            "SELECT sku FROM product_import GROUP BY sku HAVING count(*) > 1 LIMIT 1",
            &[],
        )
        .await?;
    // … then MERGE … RETURNING merge_action(), counted into an ImportReport
~~~

[stockroom/src/repo/products.rs](../stockroom/src/repo/products.rs)

Most of this chapter is in it. The HTTP body is already a stream of `Bytes`, so the
upload goes from socket to `COPY` without being held in memory, and chunk boundaries do not
matter. The staging table exists before the `COPY` starts, so the `COPY` cannot fail at the
start. `HEADER MATCH` (PG 15+) rejects a file whose header names other columns, with
"column name mismatch in header line field 2: got "title", expected "name"" and its line. And
the errors that a client can fix, class 22, are passed back with their line (`input_error`,
Chapter 6), while everything else stays in the log.

`stockroom seed` fills a development database with binary `COPY`, naming the types in column
order. Note the domain:

~~~rust
// stockroom/src/cli/seed.rs (excerpt)
// Products: binary COPY. Every value is sent in its binary wire format, so the column
// types are named up front, one per column, in COPY's column order. The `sku` column is
// a domain, and in COPY a domain travels as its base type: `TEXT`.
let sink = tx
    .copy_in(
        "COPY products (sku, name, price_cents, tags, attributes) FROM STDIN (FORMAT binary)",
    )
    .await?;
let writer = BinaryCopyInWriter::new(
    sink,
    &[
        Type::TEXT,
        Type::TEXT,
        Type::INT8,
        Type::TEXT_ARRAY,
        Type::JSONB,
    ],
);
let mut writer = pin!(writer);
for i in 1..=args.products {
    // …
    writer
        .as_mut()
        .write(&[&sku, &name, &price, &tags, &attributes])
        .await?;
}
let products = writer.as_mut().finish().await?;
~~~

[stockroom/src/cli/seed.rs](../stockroom/src/cli/seed.rs)

The other direction is `GET /products/export.csv`, which passes `COPY … TO STDOUT (FORMAT csv,
HEADER)` straight to the response body, and `stockroom export movements`, which reads the stock
ledger with `BinaryCopyOutStream` and writes CSV. Both hold their connection for the whole
transfer, for the reasons of Chapter 9.

## Summary

- `copy_in` returns a `CopyInSink`: `send` chunks of bytes (they need not align with rows),
  then `finish()`, which returns the row count. Dropping the sink before `finish()` aborts the
  `COPY`, and aborts the surrounding transaction.
- In 0.7.18, a `COPY … FROM STDIN` that the server rejects as it starts (missing table or
  column, invalid option) leaves the connection unusable (issue #789). Copy into a table you
  just created, and treat an error from `copy_in` itself as a lost connection.
- `copy_out` returns a `CopyOutStream` of `Bytes`, one chunk per row; any query can be the
  source. CSV writes `NULL` as an empty field and `''` as `""`.
- Binary `COPY` (`BinaryCopyInWriter`, `BinaryCopyOutStream`) is the fastest format. Name the
  column types exactly, in order: the client checks values against your types, nothing checks
  your types against the table, and a same-sized wrong type loads wrong data without an error.
- `COPY` is all or nothing, and its error names the line. `ON_ERROR ignore` (PG 17+) skips
  rows whose values do not convert, and `REJECT_LIMIT` (PG 18+) caps how many; neither works
  for structural errors, constraints or binary.
- For data from outside, load a staging table, validate with SQL, then `INSERT … SELECT` or
  `MERGE … RETURNING merge_action()`. Deduplicate first: duplicate keys fail the merge.
- Measured: binary `COPY` was about 100× faster than single `INSERT`s on localhost and 220×
  across a 1 ms delay; `unnest` batches came within 2× of it.

## Exercises

1. **A faithful round trip.** Export a table with `NULL`s, empty strings, commas, quotes and a
   newline in its text values to CSV with `copy_out`, load the result into a second table with
   `copy_in`, and prove with SQL that the two tables are identical.
   *Hint:* `EXCEPT` in both directions, or compare `count(*)` of a `FULL JOIN … IS NOT DISTINCT
   FROM`.
2. **Every bad line at once.** Load a CSV of `sku,price_cents` into a staging table so that the
   `COPY` cannot fail on a bad price, then report the line number of every price that is not a
   valid `bigint`, in one query.
   *Hint:* a staging table of `text` columns, plus `line bigint GENERATED ALWAYS AS IDENTITY`
   left out of the `COPY` column list; then `pg_input_is_valid` (PG 16+).
3. **Who checks what.** In §10.3's second example, why does a wrong *Rust* value fail on the
   client while a wrong *declared* type fails on the server, or not at all? Would declaring
   `Type::TEXT` for a `varchar(10)` column work, and what happens with an 11-character value?
   *Hint:* the binary formats of `text` and `varchar` are the same; the length limit is a
   property of the column.
4. **The last line wins.** Change §10.6's import so that when a SKU appears more than once,
   the line nearest the end of the file wins, instead of failing.
   *Hint:* the identity column of Exercise 2, and `DISTINCT ON (sku) … ORDER BY sku, line DESC`.

### Solutions

**1.**

```rust
use bytes::Bytes;
use futures_util::{SinkExt, TryStreamExt};
use std::pin::pin;

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE source (id int, body text);
             INSERT INTO source VALUES
                 (1, NULL), (2, ''), (3, 'a,b'), (4, 'say \"hi\"'), (5, E'two\\nlines');
             CREATE TEMP TABLE target (LIKE source);",
        )
        .await?;

    let chunks: Vec<Bytes> = client
        .copy_out("COPY source TO STDOUT (FORMAT csv)")
        .await?
        .try_collect()
        .await?;
    let sink = client
        .copy_in("COPY target FROM STDIN (FORMAT csv)")
        .await?;
    let mut sink = pin!(sink);
    for chunk in chunks {
        sink.send(chunk).await?;
    }
    assert_eq!(sink.as_mut().finish().await?, 5);

    let differences: i64 = client
        .query_one(
            "SELECT count(*) FROM (
                 (SELECT * FROM source EXCEPT SELECT * FROM target)
                 UNION ALL
                 (SELECT * FROM target EXCEPT SELECT * FROM source)
             ) d",
            &[],
        )
        .await?
        .get(0);
    assert_eq!(differences, 0);
    Ok(())
}
```

`EXCEPT` compares rows with `IS NOT DISTINCT FROM` semantics, so a `NULL` that came back as
`''` would show up as a difference. It does not: `COPY` reads its own CSV back exactly,
quoted newline included.

**2.**

```rust
use bytes::Bytes;
use futures_util::SinkExt;
use std::pin::pin;

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE price_import (
                 line bigint GENERATED ALWAYS AS IDENTITY,
                 sku text,
                 price_cents text
             )",
        )
        .await?;
    let sink = client
        .copy_in("COPY price_import (sku, price_cents) FROM STDIN (FORMAT csv, HEADER)")
        .await?;
    let mut sink = pin!(sink);
    sink.send(Bytes::from_static(
        b"sku,price_cents\nA-1,4999\nA-2,12.900\nA-3,2500\nA-4,\nA-5,lots\n",
    ))
    .await?;
    sink.as_mut().finish().await?;

    // The header is line 1, so data row n is line n + 1.
    let bad: Vec<i64> = client
        .query(
            "SELECT line + 1 FROM price_import
             WHERE price_cents IS NULL OR NOT pg_input_is_valid(price_cents, 'bigint')
             ORDER BY line",
            &[],
        )
        .await?
        .iter()
        .map(|row| row.get(0))
        .collect();
    assert_eq!(bad, [3, 5, 6]);
    Ok(())
}
```

The identity column numbers the rows in the order `COPY` inserts them, which is the order of
the file. An empty CSV field arrives as `NULL`, hence the `IS NULL` test.

**3.** The client has a Rust value and a declared `Type`, and `ToSql::accepts` decides whether
that value may be encoded as that type: `i32` may not be `INT8`. It never sees the table. The
server has the table and a stream of bytes, and decodes each field with the receive function of
the *column's* type; it notices a wrong declared type only when the bytes do not decode, as
with 4 bytes for a `bigint`, and a `real` takes the 4 bytes of an `int4` without complaint.
`TEXT` for a `varchar(10)` column works, because the two types have the same binary format, and
an 11-character value then fails on the server like any other insert: "value too long for type
character varying(10)" (22001), a constraint of the column, not of the format.

**4.**

```rust
use bytes::Bytes;
use futures_util::SinkExt;
use std::pin::pin;

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let mut client = playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE product (sku text PRIMARY KEY, price_cents bigint NOT NULL);
             INSERT INTO product VALUES ('A-1', 4999);",
        )
        .await?;

    let tx = client.transaction().await?;
    tx.batch_execute(
        "CREATE TEMP TABLE price_import (
             line bigint GENERATED ALWAYS AS IDENTITY, sku text, price_cents bigint
         ) ON COMMIT DROP",
    )
    .await?;
    let sink = tx
        .copy_in("COPY price_import (sku, price_cents) FROM STDIN (FORMAT csv)")
        .await?;
    let mut sink = pin!(sink);
    sink.send(Bytes::from_static(b"A-1,100\nA-9,1\nA-1,200\nA-9,2\n"))
        .await?;
    sink.as_mut().finish().await?;
    tx.execute(
        "MERGE INTO product AS p
         USING (
             SELECT DISTINCT ON (sku) sku, price_cents
             FROM price_import
             ORDER BY sku, line DESC
         ) AS i ON p.sku = i.sku
         WHEN MATCHED THEN UPDATE SET price_cents = i.price_cents
         WHEN NOT MATCHED THEN INSERT VALUES (i.sku, i.price_cents)",
        &[],
    )
    .await?;
    tx.commit().await?;

    let prices: Vec<(String, i64)> = client
        .query("SELECT sku, price_cents FROM product ORDER BY sku", &[])
        .await?
        .iter()
        .map(|row| (row.get(0), row.get(1)))
        .collect();
    assert_eq!(prices, [("A-1".to_owned(), 200), ("A-9".to_owned(), 2)]);
    Ok(())
}
```

---

Previous: [Chapter 9 — Streaming rows and portals](09-streaming.md) ·
Next: [Chapter 11 — LISTEN/NOTIFY and notices](11-listen-notify.md)
