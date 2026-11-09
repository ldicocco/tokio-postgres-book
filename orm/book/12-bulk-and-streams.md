# Chapter 12 — Bulk writes and streams

> **Building an ORM on tokio-postgres** · Previous: [Chapter 11 — Relations without N+1](11-relations.md) · Next: [Chapter 13 — Trusting the mapping: schema checks and tests](13-trusting-the-mapping.md)

Inserting twenty thousand rows one `insert` at a time is the N+1 problem again, in the other
direction: twenty thousand round trips. The main book measured it with pgload: one `INSERT` per
row was about a hundred times slower than binary COPY on localhost, and `unnest` batches came
within a few milliseconds of COPY (main book [§10.5](../../book/10-copy.md#105-what-copy-buys-the-pgload-table)).
A model layer that offers only `insert` pushes every bulk load into the slow path.

shelf adds two bulk writes, both generated from the `New…` struct you already have: `insert_many`,
one `INSERT` with an array per column, and `copy_in`, binary COPY for the largest loads. Then the
chapter turns to the other large direction, reading, where a `Vec` of a million models is a
million models held at once, and gives queries a stream of models that resolves its columns once
and lets the server wait for a slow reader. Writing the chapter found a bug in `insert_many`;
§12.1 shows it and its fix.

**In this chapter you will learn:**

- how `insert_many` sends a batch as one array per column through `unnest`
- where the arrays' types come from, and why a domain needs its own
- why an array column cannot go through `unnest`, and a composite needs `unnest` in the right place
- how `copy_in` writes binary COPY with types it learnt from the server
- what each bulk strategy costs, measured beside pgload
- how a stream of models reads one row at a time, and what it holds while it does
- how a COPY rejected as it starts can kill a connection in 0.7.18, and how shelf avoids it

---

## 12.1 `insert_many`

A batch of rows is a table, and SQL can turn arrays into a table: `unnest` of an array returns
one row per element. Send one array per column, and one statement inserts the whole batch:

~~~sql
INSERT INTO products (sku, name, price_cents)
SELECT unnest($1::text[]), unnest($2::text[]), unnest($3::int8[])
RETURNING …
~~~

Three parameters, whatever the batch's size: one statement to prepare, one round trip, and no
limit on the number of rows. `#[derive(Insert)]` writes what this needs from each struct: the list
of columns, and an `arrays` method that turns a slice of rows into one array per column, each a
`Vec<&T>` of references into the rows, so nothing is copied:

~~~rust
// orm/shelf/src/bulk.rs (excerpt)
/// The column `field` reads from every row, as one array parameter: `Vec<&T>` is sent as
/// `T`'s array type. Generated code calls this once per column of an `Insert` struct.
#[doc(hidden)]
pub fn array<'r, R: Sync, T: ToSql + Sync + 'r>(
    rows: &'r [R],
    field: fn(&'r R) -> &'r T,
) -> Array<'r> {
    Box::new(rows.iter().map(field).collect::<Vec<&T>>())
}
~~~

[orm/shelf/src/bulk.rs](../shelf/src/bulk.rs)

The derive calls it once per field: `array(rows, |row| &row.sku)`, `array(rows, |row|
&row.name)`, and so on. `Vec<&T>` implements `ToSql` whenever `&T` does, and is sent as `T`'s
array type.

Here it is at work on a thousand products, with a domain column, through a pool of one so that
the statements the connection cached can be read back from the server:

```rust
use postgres_types::{FromSql, ToSql};
use shelf::{FromRow, Insert, Table};

#[derive(Debug, Clone, PartialEq, ToSql, FromSql)]
#[postgres(name = "sku")]
struct Sku(String);

#[derive(Debug, FromRow, Table)]
#[shelf(table = "products", schema = "shelf_ch12_many")]
struct Product {
    #[shelf(primary_key, generated)]
    id: i64,
    sku: Sku,
    name: String,
    price_cents: i64,
}

#[derive(Insert)]
#[shelf(model = Product)]
struct NewProduct {
    sku: Sku,
    name: String,
    price_cents: i64,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let pool = shelf_playground::pool(1);
    let client = pool.get().await?;
    client
        .batch_execute(
            "DROP SCHEMA IF EXISTS shelf_ch12_many CASCADE;
             CREATE SCHEMA shelf_ch12_many;
             CREATE DOMAIN shelf_ch12_many.sku AS text CHECK (VALUE ~ '^[A-Z]+-[0-9]+$');
             CREATE TABLE shelf_ch12_many.products (
                 id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
                 sku shelf_ch12_many.sku NOT NULL UNIQUE,
                 name text NOT NULL,
                 price_cents bigint NOT NULL
             );",
        )
        .await?;

    let batch: Vec<NewProduct> = (1..=1000)
        .map(|i| NewProduct {
            sku: Sku(format!("SKU-{i:04}")),
            name: format!("Product {i}"),
            price_cents: 100 * i,
        })
        .collect();
    let products = NewProduct::insert_many(&client, &batch).await?;
    println!(
        "{} rows; the first {:?}, the last {:?}",
        products.len(),
        products[0].sku,
        products[999].sku
    );

    let rows = client
        .query_typed(
            "SELECT statement FROM pg_prepared_statements WHERE statement LIKE '%unnest%'",
            &[],
        )
        .await?;
    let statement: String = rows[0].get(0);
    println!("{statement}");

    assert_eq!(products.len(), 1000);
    assert!(products.iter().zip(&batch).all(|(p, n)| p.sku == n.sku)); // the slice's order
    assert!(statement.contains(r#"unnest($1::"shelf_ch12_many"."sku"[])"#));

    client
        .batch_execute("DROP SCHEMA shelf_ch12_many CASCADE")
        .await?;
    Ok(())
}
```

```text
1000 rows; the first Sku("SKU-0001"), the last Sku("SKU-1000")
INSERT INTO "shelf_ch12_many"."products" ("sku", "name", "price_cents") SELECT unnest($1::"shelf_ch12_many"."sku"[]), unnest($2::"pg_catalog"."text"[]), unnest($3::"pg_catalog"."int8"[]) RETURNING "id", "sku", "name", "price_cents"
```

One statement wrote a thousand rows and returned them as models, with their generated ids. Each
array carries a cast, and the cast is not a guess: the `sku` array is cast to the domain,
`"shelf_ch12_many"."sku"[]`, the other two to their built-in types, every name schema-qualified
and quoted.

> **SQL lens:** `RETURNING` gives the rows back in the order PostgreSQL inserted them, and an
> `INSERT … SELECT` inserts in the order the `SELECT` produces, which for `unnest` is the arrays'
> order. In practice the models come back in the slice's order, which the program asserts. SQL
> itself does not promise it, so code that must pair each input with its row should match them
> on a unique column (a SKU) rather than by position.

### Where the casts come from

The casts are required. A bare `unnest($1)` gives PostgreSQL nothing to infer the parameter's
type from, since `unnest` takes any array. So `insert_many` needs each column's type, and the
question is where to get it. The first plan was to prepare `SELECT "sku", … FROM products LIMIT
0` and read the result columns' types. This program shows why that was wrong, beside what shelf
does instead, which is to prepare the struct's own single-row `INSERT` and read its parameters:

```rust
use shelf::{FromRow, Insert, Table};

#[derive(Debug, FromRow, Table)]
#[shelf(table = "products", schema = "shelf_ch12_types")]
#[allow(dead_code)] // the program only prepares
struct Product {
    #[shelf(primary_key, generated)]
    id: i64,
    sku: String,
    name: String,
}

#[derive(Insert)]
#[shelf(model = Product)]
#[allow(dead_code)]
struct NewProduct {
    sku: String,
    name: String,
}

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = shelf_playground::connect().await?;
    client
        .batch_execute(
            "DROP SCHEMA IF EXISTS shelf_ch12_types CASCADE;
             CREATE SCHEMA shelf_ch12_types;
             CREATE DOMAIN shelf_ch12_types.sku AS text CHECK (VALUE ~ '^[A-Z]+-[0-9]+$');
             CREATE TABLE shelf_ch12_types.products (
                 id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
                 sku shelf_ch12_types.sku NOT NULL,
                 name text NOT NULL
             );",
        )
        .await?;

    let select = client
        .prepare("SELECT sku, name FROM shelf_ch12_types.products LIMIT 0")
        .await?;
    let insert = client.prepare(NewProduct::INSERT).await?;
    for (column, ty) in select.columns().iter().map(|c| (c.name(), c.type_())) {
        println!(
            "SELECT  {column:<5} {}.{} {:?}",
            ty.schema(),
            ty.name(),
            ty.kind()
        );
    }
    for (column, ty) in NewProduct::COLUMNS.iter().zip(insert.params()) {
        println!(
            "INSERT  {column:<5} {}.{} {:?}",
            ty.schema(),
            ty.name(),
            ty.kind()
        );
    }

    assert_eq!(select.columns()[0].type_().name(), "text");
    assert_eq!(insert.params()[0].name(), "sku");

    client
        .batch_execute("DROP SCHEMA shelf_ch12_types CASCADE")
        .await?;
    Ok(())
}
```

```text
SELECT  sku   pg_catalog.text Simple
SELECT  name  pg_catalog.text Simple
INSERT  "sku" shelf_ch12_types.sku Domain(Text)
INSERT  "name" pg_catalog.text Simple
```

A result column of a domain type is described as its base type (the main book's verified facts,
[§5.5](../../book/05-custom-types.md#55-domains)), so the `SELECT` says `text`. The `INSERT`'s
parameter that targets the domain column is described as the domain itself. The difference
decides what a batch accepts. With `text[]` casts, a `Vec<&Sku>` of domain newtypes would be
refused on the client, since postgres-types' derive accepts only the domain's own type; with the
domain's array cast, it is accepted, and a `Vec<&String>` is refused, exactly as for one `insert`
(Chapter 8, §8.2). Taking the types from the `INSERT` makes `insert_many` accept what `insert`
accepts, value for value. The domain's `CHECK` still runs on every row on the server.

Preparing the `INSERT` costs a round trip on a plain `Client`, and nothing after the first time
on a pooled connection, where the statement is cached. `insert_many` prepares it but never runs
it.

### What `unnest` cannot carry

**An array column.** A batch's `tags` column would be an array of arrays, `Vec<&Vec<String>>`,
and that is where `unnest` stops: tokio-postgres 0.7.18 cannot send a `Vec<Vec<String>>` as a
`text[]` parameter ("error serializing parameter 0"), and PostgreSQL arrays are rectangular, so
`unnest` of a two-dimensional array flattens it into single elements (orm/PLAN.md §12 #37). shelf
does not pretend: `insert_many` refuses a struct with an array column before anything is sent,
and says what to use instead.

```rust
use shelf::{FromRow, Insert, Table};

#[derive(Debug, FromRow, Table)]
#[shelf(table = "products")]
struct Product {
    #[shelf(primary_key, generated)]
    id: i64,
    sku: String,
    tags: Vec<String>,
}

#[derive(Insert)]
#[shelf(model = Product)]
struct NewProduct {
    sku: String,
    tags: Vec<String>,
}

#[tokio::main]
async fn main() -> Result<(), shelf::Error> {
    let client = shelf_playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE products (
                 id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
                 sku text NOT NULL, tags text[] NOT NULL
             );",
        )
        .await?;
    let batch = vec![
        NewProduct {
            sku: "TEE-01".into(),
            tags: vec!["sale".into(), "cotton".into()],
        },
        NewProduct {
            sku: "MUG-01".into(),
            tags: vec![],
        },
    ];

    let error = NewProduct::insert_many(&client, &batch).await.unwrap_err();
    println!("{error}");
    assert!(matches!(error, shelf::Error::Unsupported { .. }));

    let loaded = NewProduct::copy_in(&client, &batch).await?;
    let tee = Product::query()
        .filter(Product::SKU.eq("TEE-01"))
        .first(&client)
        .await?
        .expect("loaded");
    println!("copy_in loaded {loaded}; TEE-01's tags: {:?}", tee.tags);
    assert_eq!(loaded, 2);
    assert_eq!(tee.tags, ["sale", "cotton"]);
    Ok(())
}
```

```text
Product: insert_many cannot send the array column "tags" through unnest, which would flatten an array of arrays; use copy_in, or insert in a loop
copy_in loaded 2; TEE-01's tags: ["sale", "cotton"]
```

Binary COPY sends each row's values one by one, so an array column is just a value (§12.2).

**A composite column, in the wrong place.** Until this chapter was written, shelf's statement
read `SELECT * FROM unnest($1::…[], $2::…[])`, the form pgload uses. A probe for the chapter
tried it with a composite column, an address, and PostgreSQL refused:

```text
ERROR:  INSERT has more expressions than target columns
LINE 1: ...r[]) AS INSERT INTO stores (code, address) SELECT * FROM unn...
                                                             ^
```

The main book had met the cause (its PLAN.md, §12 #4): `unnest` in `FROM` expands each composite
value into its fields, so `(code, address)` became three columns, `code`, `street`, `city`. In the
select list, `unnest` keeps each value whole. shelf now writes `SELECT unnest($1::…[]),
unnest($2::…[]), …`, a test pins it (`a_composite_column_goes_through_insert_many_whole` in
`tests/bulk.rs`), and the benchmark of §12.3 was run again with the new form. Two facts make the
select-list form equivalent for shelf: since PostgreSQL 10, set-returning functions in a select
list run in step, one row from each per output row; and arrays of unequal length are padded with
NULLs in both forms, which never matters here, since every array is built from the same slice.

## 12.2 Binary COPY from the derive

`copy_in` is the fastest way in. The derive writes the statement at compile time, beside
`INSERT`, from the same columns:

~~~text
COPY "products" ("sku", "name", "price_cents") FROM STDIN (FORMAT binary)
~~~

Binary COPY sends every value in its binary wire format, so the writer must know each column's
type, and it trusts what it is told: the main book showed that an `int4` declared for a `real`
column loads silently wrong data, since both are four bytes (main book
[§10.3](../../book/10-copy.md#103-binary-copy)). stockroom's seeder names its types by hand. shelf
does not name any: it hands `BinaryCopyInWriter` the parameter types of the prepared `INSERT`, the
same types `insert_many` casts to, which are the columns' own, from the server. Each row's values
are its `params()`, the very values `insert` sends:

~~~rust
// orm/shelf/src/bulk.rs (excerpt)
pub(crate) async fn copy_in<I: Insert>(db: &impl Executor, rows: &[I]) -> Result<u64, Error> {
    // …
    // Preparing a COPY checks nothing, and a COPY rejected as it starts leaves the connection
    // unusable in 0.7.18 (issue #789). Preparing the INSERT checks the table and columns.
    let insert = db.prepare(I::INSERT).await?;
    let copy = db.prepare(I::COPY).await?;
    let sink = db.copy_in(&copy).await?;
    // Binary COPY trusts these types: they are the columns' own, from the server.
    let mut writer = pin!(BinaryCopyInWriter::new(sink, insert.params()));
    for row in rows {
        writer.as_mut().write(&row.params()).await?;
    }
    Ok(writer.finish().await?)
}
~~~

[orm/shelf/src/bulk.rs](../shelf/src/bulk.rs)

`BinaryCopyInWriter` must be pinned, and `pin!` pins it on the stack of this function. That is
safe here because the writer never escapes: every return from the function, `?` included, drops
it, and a dropped writer ends the COPY. Forgetting `finish()` would abort it, which is why it is
the function's last expression.

> **Pitfall:** `drop(writer)` on a value pinned with `pin!` drops the `Pin<&mut _>`, not the
> writer, which lives to the end of its block with its COPY open; the next query on the
> connection then waits behind it (main book PLAN.md §12 #34). shelf's writer lives in a
> function body for that reason. In code of your own, put a pinned writer or stream in a block
> that ends where it should, or pin it in a `Box`.

What can go wrong once the COPY runs leaves the connection usable. A value the client cannot
encode fails at its `write`; a value the server refuses, such as a SKU that breaks the domain's
check, fails at `finish()`, with the line, and nothing is loaded:

```rust
use postgres_types::{FromSql, ToSql};
use shelf::{FromRow, Insert, Table};

#[derive(Debug, Clone, ToSql, FromSql)]
#[postgres(name = "sku")]
struct Sku(String);

#[derive(Debug, FromRow, Table)]
#[shelf(table = "products", schema = "shelf_ch12_copy")]
#[allow(dead_code)] // only counted
struct Product {
    #[shelf(primary_key, generated)]
    id: i64,
    sku: Sku,
    price_cents: i64,
}

#[derive(Insert)]
#[shelf(model = Product)]
struct NewProduct {
    sku: Sku,
    price_cents: i64,
}

#[tokio::main]
async fn main() -> Result<(), shelf::Error> {
    let client = shelf_playground::connect().await?;
    client
        .batch_execute(
            "DROP SCHEMA IF EXISTS shelf_ch12_copy CASCADE;
             CREATE SCHEMA shelf_ch12_copy;
             CREATE DOMAIN shelf_ch12_copy.sku AS text CHECK (VALUE ~ '^[A-Z]+-[0-9]+$');
             CREATE TABLE shelf_ch12_copy.products (
                 id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
                 sku shelf_ch12_copy.sku NOT NULL, price_cents bigint NOT NULL
             );",
        )
        .await?;
    let mut batch: Vec<NewProduct> = (1..=10_000)
        .map(|i| NewProduct {
            sku: Sku(format!("SKU-{i}")),
            price_cents: i,
        })
        .collect();

    // Line 5000 breaks the domain's check: the whole COPY fails, at `finish`.
    batch[4_999].sku = Sku("not a sku".into());
    let shelf::Error::Db(error) = NewProduct::copy_in(&client, &batch).await.unwrap_err() else {
        panic!("a database error");
    };
    let db = error.as_db_error().expect("a server error");
    println!(
        "{}\n  where: {}",
        db.message(),
        db.where_().unwrap_or_default()
    );
    assert_eq!(Product::count(&client).await?, 0); // nothing loaded, and the connection works

    batch[4_999].sku = Sku("SKU-5000".into());
    let loaded = NewProduct::copy_in(&client, &batch).await?;
    println!("loaded {loaded}");
    assert_eq!(loaded, 10_000);

    client
        .batch_execute("DROP SCHEMA shelf_ch12_copy CASCADE")
        .await?;
    Ok(())
}
```

```text
value for domain shelf_ch12_copy.sku violates check constraint "sku_check"
  where: COPY products, line 5000, column sku
loaded 10000
```

`shelf::Error::Db` wraps the driver's `tokio_postgres::Error`, and its `as_db_error` reaches the
server's message, its `where_` and its SQLSTATE (Chapter 7, §7.5). `copy_in` returns a count, not models: COPY has no `RETURNING`.
When you need the generated keys, use `insert_many`.

## 12.3 Measured

How much each strategy buys was measured with `bench_bulk`, a benchmark in shelf's repository
written from pgload's code: pgload's rows (an id, a SKU, a name with commas and quotes, a price
and a timestamp) into its temporary table without constraints, on one pooled connection, each run
in a transaction that is rolled back, every strategy checked to store the same rows by an md5 of
the table:

~~~bash
cargo run -p shelf --release --example bench_bulk -- --rows 20000 [--keyed]
cargo run -p pgload --release -- --rows 20000
~~~

| 20,000 rows | shelf (`bench_bulk`) | pgload |
|---|---|---|
| one `insert` / `INSERT` at a time | 0.767–0.782 s | 0.733–0.761 s |
| pipelined, 64 in flight | 0.146–0.151 s | 0.120–0.121 s |
| `insert_many` in batches of 1,000 / `unnest` batches | 0.015 s | 0.013–0.014 s |
| `insert_many`, one call | 0.014 s | — |
| CSV COPY | — | 0.013 s |
| `copy_in` / binary COPY | 0.006 s | 0.007–0.008 s |

(shelf: median of three per strategy, three runs; pgload: two runs; release builds, rustc
1.99.1, Apple Silicon, PostgreSQL 18.6 on localhost, after the fix of §12.1.)

shelf's `copy_in` matches pgload's binary COPY, and `insert_many` costs about what pgload's
hand-written `unnest` does, a little more because it returns and maps every row, which pgload's
does not. The loop is a hundred times slower than COPY on localhost, where a round trip costs
under 50 µs; at any real latency, the gap is the round trips.

Two more variants show where the time goes when the conditions change: `--keyed` loads into a
regular table with a primary key and `NOT NULL` columns, and the second runs 2,000 rows through
lagproxy at 1 ms each way:

| | 20,000 rows, a keyed table | 2,000 rows, 1 ms each way |
|---|---|---|
| one `insert` at a time | 0.822–0.834 s | 9.480–9.513 s |
| pipelined, 64 in flight | 0.180–0.202 s | 0.156–0.157 s |
| `insert_many` in batches of 1,000 | 0.030–0.058 s | 0.013–0.018 s |
| `insert_many`, one call | 0.026–0.038 s | 0.008–0.009 s |
| `copy_in` | 0.017–0.028 s | 0.012 s |

(Three runs each, as above.) The index and the WAL add to every strategy, and make the batched
ones noisy, since a large write waits for WAL in bursts. Through the proxy, the loop paid 2,000
round trips, 4.7 ms each; every batched form paid one or two. `copy_in` is slower than
`insert_many` there because a COPY takes two round trips even with its statements cached, one to
start and one to finish, where a cached `insert_many` takes one. pgload's own numbers at 1 ms are
higher for the batched forms because its timer also covers five round trips of setup and commit,
which `bench_bulk`'s does not (orm/PLAN.md §12 #45): a benchmark says what it times.

> **Pitfall:** outside a transaction, every statement commits, and every commit waits for its
> WAL to reach the disk. 20,000 pipelined inserts into a table with a primary key took 0.91–0.98 s
> when each one committed, and 0.13–0.14 s between one `BEGIN` and `COMMIT` (orm/PLAN.md §12
> #46, `synchronous_commit = on`). A loop or a pipeline of `insert`s belongs in a transaction,
> and a benchmark must say whether it used one. `insert_many` and `copy_in` are one statement,
> so one commit, either way.

A debug build changes the ratios but not the order. This program times three strategies on
10,000 rows, each in a transaction that is rolled back, as the book's checker runs it:

```rust
use std::time::Instant;

use shelf::{FromRow, Insert, Table};

#[derive(Debug, FromRow, Table)]
#[shelf(table = "products")]
#[allow(dead_code)] // only written
struct Product {
    #[shelf(primary_key)]
    id: i64,
    sku: String,
    name: String,
    price_cents: i64,
}

#[derive(Insert)]
#[shelf(model = Product)]
struct NewProduct {
    id: i64,
    sku: String,
    name: String,
    price_cents: i64,
}

#[tokio::main]
async fn main() -> Result<(), shelf::Error> {
    let mut client = shelf_playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE products (
                 id bigint, sku text, name text, price_cents bigint
             );",
        )
        .await?;
    let batch: Vec<NewProduct> = (1..=10_000)
        .map(|i| NewProduct {
            id: i,
            sku: format!("SKU-{i:05}"),
            name: format!("Product {i}, \"the {i}th\""),
            price_cents: 99 * i,
        })
        .collect();

    let tx = client.transaction().await?;
    let started = Instant::now();
    for row in &batch {
        row.insert(&tx).await?;
    }
    let one_by_one = started.elapsed();
    tx.rollback().await?;

    let tx = client.transaction().await?;
    let started = Instant::now();
    NewProduct::insert_many(&tx, &batch).await?;
    let many = started.elapsed();
    tx.rollback().await?;

    let tx = client.transaction().await?;
    let started = Instant::now();
    NewProduct::copy_in(&tx, &batch).await?;
    let copy = started.elapsed();
    assert_eq!(Product::count(&tx).await?, 10_000);
    tx.rollback().await?;

    println!("insert, one at a time  {one_by_one:>9.1?}");
    println!("insert_many            {many:>9.1?}");
    println!("copy_in                {copy:>9.1?}");

    assert!(many * 10 < one_by_one, "one statement against ten thousand");
    assert!(
        copy * 10 < one_by_one,
        "one COPY against ten thousand statements"
    );
    Ok(())
}
```

```text
insert, one at a time       1.2s
insert_many               23.3ms
copy_in                    9.7ms
```

Three runs of the debug build gave 1.2 s for the loop each time, 23.2–23.7 ms for `insert_many`
and 9.1–9.7 ms for `copy_in`: the loop 50 times slower than `insert_many` and 125 times slower
than COPY, about the release build's ratio for COPY. The program asserts a factor of ten, which
leaves room for a slower machine and still fails if a batch ever turned back into a loop.

## 12.4 Streams of models

`all()` collects every row into a `Vec`, which is right for a page and wrong for an export: a
million models held at once, and nothing written until the last one arrives. tokio-postgres reads
rows as they come off the socket through `query_raw`, which returns a `RowStream` (main book
[§9.2](../../book/09-streaming.md#92-query_raw-and-rowstream)). shelf wraps it in a stream of
models, `ModelStream<'a, T>`, from `fetch_stream::<T>(&db, sql, params)` or a query's `stream`:

```rust
use futures_util::TryStreamExt;
use shelf::{FromRow, Table};

#[derive(Debug, FromRow, Table)]
#[shelf(table = "products")]
struct Product {
    #[shelf(primary_key)]
    id: i64,
    sku: String,
    price_cents: i64,
}

#[tokio::main]
async fn main() -> Result<(), shelf::Error> {
    let client = shelf_playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE products (id bigint PRIMARY KEY, sku text NOT NULL,
                                         price_cents bigint NOT NULL);
             INSERT INTO products SELECT g, 'SKU-' || g, g FROM generate_series(1, 100000) g;",
        )
        .await?;

    let query = Product::query().filter(Product::PRICE_CENTS.ge(1_000));
    let mut products = query.stream(&client).await?;
    let (mut count, mut total, mut last_id) = (0_u64, 0_i64, 0_i64);
    // One model at a time: each is read, added up and dropped before the next arrives.
    while let Some(product) = products.try_next().await? {
        count += 1;
        total += product.price_cents;
        last_id = product.id;
        assert!(product.sku.starts_with("SKU-"));
    }
    println!("{count} products, {total} cents in all, the last id {last_id}");
    assert_eq!(count, 99_001);
    assert_eq!(total, (1_000..=100_000).sum::<i64>());
    Ok(())
}
```

```text
99001 products, 4999550500 cents in all, the last id 100000
```

The stream resolved `Product`'s columns once, from the prepared statement, before the first row
(Chapter 5), so a mapping mistake fails at `stream(&client)`, before the query runs; each row is
then read by position as it arrives. A row that fails to decode is an `Err` in its place in the
stream, and the rows before it have already been handled.

> **Rust refresher:** a `Stream` is polled through `Pin<&mut Self>`, because a future or stream
> may hold references into itself once polled. `try_next()` needs `Self: Unpin`, a promise that
> moving the value is harmless. tokio-postgres's `RowStream` is not `Unpin`, which is why the
> main book's examples write `pin!(rows)` before reading them. `ModelStream` holds its
> `RowStream` in a `Pin<Box<RowStream>>`: the box stays put wherever the `ModelStream` moves, so
> shelf declares `impl Unpin for ModelStream`, and the loop above needs no `pin!`. It also means
> that `drop(products)` really drops the stream, which `drop` on a `pin!`-pinned one does not
> (§12.2's pitfall).

### What the stream holds

A stream reads rows from a connection, and while it does, that connection serves no one else.
In tokio-postgres 0.7.18 each request's response channel holds one message, so an unread stream
stops the connection's reader, and every later request on the connection waits behind it (main
book [§9.3](../../book/09-streaming.md#93-backpressure-and-the-stalled-connection)). That is
backpressure, and it is what keeps a slow reader from filling memory; it is also a trap for code
that uses the connection for something else mid-stream:

```rust
use std::time::Duration;

use futures_util::TryStreamExt;
use shelf::FromRow;

#[derive(Debug, FromRow)]
struct Number {
    n: i64,
}

#[tokio::main]
async fn main() -> Result<(), shelf::Error> {
    let client = shelf_playground::connect().await?;
    // About 10 MB of rows: more than the socket buffers hold, so the server waits.
    let sql = "SELECT g AS n, repeat('x', 100) AS label FROM generate_series(1, 100000::bigint) g";
    let mut numbers = shelf::fetch_stream::<Number>(&client, sql, &[]).await?;
    assert_eq!(numbers.try_next().await?.map(|n| n.n), Some(1));

    // Another request on the same connection waits behind the unread rows.
    let waiting = tokio::time::timeout(
        Duration::from_millis(300),
        client.query_one("SELECT 1", &[]),
    )
    .await;
    println!("SELECT 1 while the stream is unread: {waiting:?}");
    assert!(waiting.is_err(), "it should wait for the stream");

    // Dropped, the stream's remaining rows are read and discarded, and the connection serves.
    drop(numbers);
    let one: i32 = client.query_one("SELECT 1", &[]).await?.get(0);
    println!("SELECT 1 after the drop: {one}");
    assert_eq!(one, 1);
    Ok(())
}
```

```text
SELECT 1 while the stream is unread: Err(Elapsed(()))
SELECT 1 after the drop: 1
```

The `SELECT 1` did not fail; it waited, and would have waited forever. Dropping the stream does
not stop the query either: the connection reads the rest of the rows and throws them away, which
for a large result costs as long as reading them (main book PLAN.md §12 #35).

With a pool, the danger is worse: a pooled connection returned to the pool with a stream still
running would stall whoever borrowed it next. `RowStream` does not borrow the client, so
tokio-postgres cannot prevent that (main book PLAN.md §12 #13). `ModelStream<'a, T>` does: its
`'a` is a borrow of the executor, so the connection cannot be given back while the stream lives.
A UI test pins it:

~~~rust
// orm/shelf/tests/ui/stream.rs (excerpt)
async fn first_number(pool: deadpool_postgres::Pool) -> i64 {
    let client = pool.get().await.unwrap();
    let mut rows = shelf::fetch_stream::<Number>(&client, "SELECT 1::bigint AS n", &[])
        .await
        .unwrap();
    drop(client); // back to the pool, with the stream unfinished
    let first = futures_util::TryStreamExt::try_next(&mut rows).await.unwrap();
    first.unwrap().n
}
~~~

[orm/shelf/tests/ui/stream.rs](../shelf/tests/ui/stream.rs)

```text
error[E0505]: cannot move out of `client` because it is borrowed
  --> tests/ui/stream.rs:14:10
   |
10 |     let client = pool.get().await.unwrap();
   |         ------ binding `client` declared here
11 |     let mut rows = shelf::fetch_stream::<Number>(&client, "SELECT 1::bigint AS n", &[])
   |                                                  ------- borrow of `client` occurs here
...
14 |     drop(client); // back to the pool, with the stream unfinished
   |          ^^^^^^ move out of `client` occurs here
15 |     let first = futures_util::TryStreamExt::try_next(&mut rows).await.unwrap();
   |                                                      --------- borrow later used here
```

The borrow has a cost: a `ModelStream` cannot be returned from the function that owns the
connection, which is exactly what an HTTP handler streaming a response body wants to do. The body
must then own the connection itself, and hold the stream inside; Chapter 14 builds that, for the
capstone's `/products/stream`.

## 12.5 A COPY rejected as it starts

The main book found the sharpest edge in tokio-postgres 0.7.18's COPY: when the server rejects a
`COPY … FROM STDIN` *as it starts*, because the table or a column does not exist, the error comes
back from `copy_in`, and the connection is gone: the next request fails with "unexpected message
from server" (main book
[§10.1](../../book/10-copy.md#a-copy-that-fails-to-start-loses-the-connection),
[rust-postgres issue #789](https://github.com/rust-postgres/rust-postgres/issues/789)).

A model layer is exactly where this would bite. A struct whose column was renamed in a migration
writes a `COPY` naming the old column, and the first bulk load after the deploy would kill a
pooled connection. Preparing the `COPY` first does not help, because PostgreSQL checks nothing
about a `COPY` when it is prepared. Preparing the `INSERT` of the same columns does help: it
checks the table and every column, and fails cleanly. `copy_in` prepares it anyway, for the
types, so the guard costs nothing:

```rust
use shelf::{FromRow, Insert, Table};

#[derive(Debug, FromRow, Table)]
#[shelf(table = "products")]
#[allow(dead_code)] // only counted
struct Product {
    #[shelf(primary_key, generated)]
    id: i64,
    sku: String,
}

#[derive(Insert)]
#[shelf(model = Product)]
struct NewProduct {
    sku: String,
    note: String, // a column the table no longer has
}

#[tokio::main]
async fn main() -> Result<(), shelf::Error> {
    let client = shelf_playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE products (
                 id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
                 sku text NOT NULL, note text
             );
             ALTER TABLE products DROP COLUMN note;",
        )
        .await?;

    // Preparing the COPY checks nothing: not even the table.
    let prepared = client.prepare("COPY no_such_table FROM STDIN").await;
    println!(
        "prepare a COPY of a missing table: {:?}",
        prepared.map(|_| "ok")
    );

    // shelf prepares the INSERT first, which names the missing column.
    let batch = [NewProduct {
        sku: "TEE-01".into(),
        note: "new".into(),
    }];
    let shelf::Error::Db(error) = NewProduct::copy_in(&client, &batch).await.unwrap_err() else {
        panic!("a database error");
    };
    let refused = error.as_db_error().expect("a server error").message();
    println!("copy_in: {refused}");

    // The connection survived.
    println!("then: {} products", Product::count(&client).await?);
    assert_eq!(Product::count(&client).await?, 0);
    Ok(())
}
```

```text
prepare a COPY of a missing table: Ok("ok")
copy_in: column "note" of relation "products" does not exist
then: 0 products
```

The same struct sent through tokio-postgres's `copy_in` directly would have lost the connection.
The guard covers what can be known before the COPY starts, which is what #789 is about; errors
*during* the copy, like §12.2's domain check, never needed it.

### Side by side — *loading ten thousand rows*

The task: insert ten thousand products in one round trip, and read their generated ids back.

~~~rust
// sqlx (illustration)
let mut builder: QueryBuilder<Postgres> =
    QueryBuilder::new("INSERT INTO products (sku, name, price_cents) ");
builder.push_values(&batch, |mut row, product| {
    row.push_bind(&product.sku)
        .push_bind(&product.name)
        .push_bind(product.price_cents);
});
builder.push(" RETURNING id");
// VALUES ($1, $2, $3), ($4, $5, $6), …: 30,000 parameters for 10,000 rows.
let ids: Vec<i64> = builder
    .build_query_scalar()
    .persistent(false)
    .fetch_all(&pool)
    .await?;
~~~

By hand, with the three arrays written out as pgload does, and with shelf's `insert_many`, in one
program that checks both store the same rows:

```rust
use shelf::{FromRow, Insert, Table};
use tokio_postgres::Client;

#[derive(Debug, FromRow, Table)]
#[shelf(table = "products")]
struct Product {
    #[shelf(primary_key, generated)]
    id: i64,
    sku: String,
    name: String,
    price_cents: i64,
}

#[derive(Insert)]
#[shelf(model = Product)]
struct NewProduct {
    sku: String,
    name: String,
    price_cents: i64,
}

/// By hand: one array per column, built and cast by you.
async fn insert_by_hand(
    client: &Client,
    batch: &[NewProduct],
) -> Result<Vec<i64>, tokio_postgres::Error> {
    let skus: Vec<&str> = batch.iter().map(|p| p.sku.as_str()).collect();
    let names: Vec<&str> = batch.iter().map(|p| p.name.as_str()).collect();
    let prices: Vec<i64> = batch.iter().map(|p| p.price_cents).collect();
    let rows = client
        .query(
            "INSERT INTO products (sku, name, price_cents)
             SELECT unnest($1::text[]), unnest($2::text[]), unnest($3::int8[])
             RETURNING id",
            &[&skus, &names, &prices],
        )
        .await?;
    Ok(rows.iter().map(|row| row.get(0)).collect())
}

#[tokio::main]
async fn main() -> Result<(), shelf::Error> {
    let client = shelf_playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE products (
                 id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
                 sku text NOT NULL, name text NOT NULL, price_cents bigint NOT NULL
             );",
        )
        .await?;
    let batch: Vec<NewProduct> = (1..=10_000)
        .map(|i| NewProduct {
            sku: format!("SKU-{i:05}"),
            name: format!("Product {i}"),
            price_cents: 99 * i,
        })
        .collect();

    let by_hand = insert_by_hand(&client, &batch).await?;
    let with_shelf = NewProduct::insert_many(&client, &batch).await?;

    assert_eq!(by_hand.len(), 10_000);
    assert_eq!(with_shelf.len(), 10_000);
    assert_eq!(with_shelf[0].id, by_hand[9_999] + 1); // the next ids, in order
    let same = with_shelf
        .iter()
        .zip(&batch)
        .all(|(p, n)| p.sku == n.sku && p.name == n.name && p.price_cents == n.price_cents);
    assert!(same);
    assert_eq!(Product::count(&client).await?, 20_000);
    Ok(())
}
```

| | by hand | shelf | sqlx |
|---|---|---|---|
| **Same** | One statement and one round trip for the whole batch, the values as parameters, the generated ids returned | The same | The same |
| **Different** | Three parameters, one array per column; you build the arrays, write the casts (`text[]` for a domain column would refuse a domain newtype) and keep both in step with the table | Three parameters, built by the derive from the struct, cast to the columns' own types learnt from the prepared `INSERT`; an array column is refused with advice; rows come back as models | One parameter per value, `VALUES ($1, $2, $3), …`: a statement holds at most 65,535 parameters, so a batch of 10,000 rows of seven columns must be split, and each batch size is a new statement, hence `persistent(false)` (its own docs' advice) |

The parameter limit is the protocol's: *Bind* counts its parameters in 16 bits. A probe for this
chapter sent 65,535 parameters through tokio-postgres without trouble; at 65,536, PostgreSQL 18.6
prepared the statement, and tokio-postgres 0.7.18 failed to read its description ("error parsing
response from server", caused by "invalid message length: parameters is not drained"), after which
the connection still worked. With one array per column, `insert_many` sends as many parameters as
the struct has columns, for any number of rows.

---

## In shelf

`insert_many` builds its statement at run time, from the derive's column list and the types the
prepared `INSERT` reports; the table and column names are identifiers the derive quoted, and the
type names are quoted by the same rule:

~~~rust
// orm/shelf/src/bulk.rs (excerpt)
pub(crate) async fn insert_many<I: Insert>(
    db: &impl Executor,
    rows: &[I],
) -> Result<Vec<I::Model>, Error> {
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let insert = db.prepare(I::INSERT).await?;
    let model = <I::Model as Table>::MODEL;
    let (table, returning) = (<I::Model as Table>::TABLE, <I::Model as Table>::COLUMNS);
    // … a struct with no columns: one row of defaults per value, through generate_series
    let mut casts = Vec::with_capacity(I::COLUMNS.len());
    for (i, (ty, column)) in insert.params().iter().zip(I::COLUMNS).enumerate() {
        if is_array(ty) {
            return Err(Error::Unsupported {
                model,
                // … the message printed in §12.1
            });
        }
        casts.push(format!(
            "unnest(${}::{}.{}[])",
            i + 1,
            quote_ident(ty.schema()),
            quote_ident(ty.name())
        ));
    }
    let sql = format!(
        "INSERT INTO {table} ({}) SELECT {} RETURNING {returning}",
        I::COLUMNS.join(", "),
        casts.join(", ")
    );
    let arrays = I::arrays(rows);
    let params: Vec<&(dyn ToSql + Sync)> = arrays.iter().map(|a| &**a as _).collect();
    fetch_all(db, &sql, &params).await
}
~~~

[orm/shelf/src/bulk.rs](../shelf/src/bulk.rs)

`is_array` looks through domains (`Kind::Domain(base)`), so a domain over an array type is
refused too. The statement depends only on the struct, so on a pooled connection it is prepared
once and cached, like any other.

## In stockroom

`stockroom seed` loads products with binary COPY by hand, naming each column's type in COPY's
column order:

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
~~~

[stockroom/src/cli/seed.rs](../../stockroom/src/cli/seed.rs)

Five types written by hand, in the order of a column list written by hand, beside a third list,
the values of each `write`. The three must agree, and only a run finds out if they do not. With a
`NewProduct` struct and `#[derive(Insert)]`, the columns, the types and the values all come from
one place: the struct's fields, and the server's description of its `INSERT`.

## When not to use shelf's bulk writes

- **A client's file.** A CSV upload belongs in a staging table through text COPY, where errors
  come back with their line, and then in the real table through SQL (main book §10.4, and
  stockroom's `/products/import`).
- **A load that must merge.** `insert_many` inserts; an upsert of a batch is `copy_in` into a
  staging table and one `INSERT … ON CONFLICT` or `MERGE` (Exercise 3).
- **A few rows.** For three rows, `insert` three times in a transaction, or pipelined, is
  clearer, and the difference is a couple of round trips.

## Summary

- `insert_many` sends a batch as one array per column, `INSERT … SELECT unnest($1::…[]), …
  RETURNING …`: one statement and one round trip for any number of rows, the rows back as models,
  in practice in the slice's order.
- Each array is cast to its column's type as PostgreSQL types the struct's own `INSERT`, prepared:
  a domain as the domain, so a batch accepts what `insert` accepts. A `SELECT … LIMIT 0` would
  describe a domain column as its base type.
- An array column cannot go through `unnest`: `Error::Unsupported`, before anything is sent. A
  composite column needs `unnest` in the select list, where it stays whole; `SELECT * FROM
  unnest(…)` expands it, the bug this chapter found and fixed.
- `copy_in` writes binary COPY with the `INSERT`'s parameter types, the columns' own, and each
  row's `params()`. Errors during the copy leave the connection usable.
- On localhost, 20,000 rows took 0.77 s one by one, 0.15 s pipelined, 0.014 s with `insert_many`
  and 0.006 s with `copy_in`, close to pgload's hand-written versions (its pipelined loop: 0.12 s).
- `ModelStream` reads one model at a time, its columns resolved once; it is `Unpin`, so it needs
  no `pin!`. While it lives, its connection serves no one else, and the borrow keeps a pooled
  connection from going back mid-stream.
- A COPY rejected as it starts kills the connection in 0.7.18 (#789), and preparing a `COPY`
  checks nothing; shelf prepares the `INSERT` of the same columns first, which fails cleanly.

## Exercises

1. **Batches.** Write `insert_in_batches(&db, &rows, size)` that writes a large slice with one
   `insert_many` per `size` rows, all or nothing.
   *Hint:* `rows.chunks(size)`, inside one transaction.
2. **Copy a table through a stream.** Copy every product into an archive table, a thousand at a
   time, without holding them all at once.
   *Hint:* the stream holds its connection, so the writes need another one. `try_chunks` from
   `futures_util` turns a stream of models into a stream of `Vec`s.
3. **COPY, then upsert.** Load a price list that may update existing products: `copy_in` into a
   staging table, then one statement that inserts the new SKUs and updates the others.
   *Hint:* a model and an `Insert` struct for the staging table, then
   `INSERT … SELECT … ON CONFLICT (sku) DO UPDATE` (main book
   [§10.6](../../book/10-copy.md#106-copy-into-a-temp-table-then-upsert)).

### Solutions

**1.** One transaction, one `insert_many` per chunk:

```rust
use shelf::{FromRow, Insert, Table};

#[derive(Debug, FromRow, Table)]
#[shelf(table = "products")]
struct Product {
    #[shelf(primary_key, generated)]
    id: i64,
    sku: String,
}

#[derive(Insert)]
#[shelf(model = Product)]
struct NewProduct {
    sku: String,
}

/// Every row, `size` at a time, all or nothing.
async fn insert_in_batches(
    client: &mut tokio_postgres::Client,
    rows: &[NewProduct],
    size: usize,
) -> Result<Vec<Product>, shelf::Error> {
    let tx = client.transaction().await?;
    let mut products = Vec::with_capacity(rows.len());
    for chunk in rows.chunks(size) {
        products.extend(NewProduct::insert_many(&tx, chunk).await?);
    }
    tx.commit().await?;
    Ok(products)
}

#[tokio::main]
async fn main() -> Result<(), shelf::Error> {
    let mut client = shelf_playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE products (
                 id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
                 sku text NOT NULL UNIQUE
             );",
        )
        .await?;
    let rows: Vec<NewProduct> = (1..=2_500)
        .map(|i| NewProduct {
            sku: format!("SKU-{i}"),
        })
        .collect();
    let products = insert_in_batches(&mut client, &rows, 1_000).await?;
    assert_eq!(products.len(), 2_500);

    // A duplicate in the last batch: the first two batches are rolled back with it.
    let mut again: Vec<NewProduct> = (2_501..=5_000)
        .map(|i| NewProduct {
            sku: format!("SKU-{i}"),
        })
        .collect();
    again[2_499].sku = "SKU-1".into();
    assert!(insert_in_batches(&mut client, &again, 1_000).await.is_err());
    assert_eq!(Product::count(&client).await?, 2_500);
    Ok(())
}
```

If the transaction is dropped by the `?`, it rolls back (main book Chapter 8), which is what makes
the batches all or nothing. Batches keep each statement's arrays, and the server's memory for one
statement, bounded.

**2.** One connection streams, the other writes; `try_chunks` groups the models:

```rust
use futures_util::TryStreamExt;
use shelf::{FromRow, Insert, Table};

#[derive(Debug, FromRow, Table)]
#[shelf(table = "products", schema = "shelf_ch12_archive")]
struct Product {
    #[shelf(primary_key)]
    id: i64,
    sku: String,
}

#[derive(Debug, FromRow, Table)]
#[shelf(table = "archive", schema = "shelf_ch12_archive")]
struct Archived {
    #[shelf(primary_key)]
    id: i64,
    sku: String,
}

#[derive(Insert)]
#[shelf(model = Archived)]
struct NewArchived<'a> {
    id: i64,
    sku: &'a str,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let reader = shelf_playground::connect().await?;
    let writer = shelf_playground::connect().await?;
    reader
        .batch_execute(
            "DROP SCHEMA IF EXISTS shelf_ch12_archive CASCADE;
             CREATE SCHEMA shelf_ch12_archive;
             CREATE TABLE shelf_ch12_archive.products (id bigint PRIMARY KEY, sku text NOT NULL);
             CREATE TABLE shelf_ch12_archive.archive (id bigint PRIMARY KEY, sku text NOT NULL);
             INSERT INTO shelf_ch12_archive.products SELECT g, 'SKU-' || g
                 FROM generate_series(1, 25000) g;",
        )
        .await?;

    let mut batches = Product::query().stream(&reader).await?.try_chunks(1_000);
    let mut written = 0;
    while let Some(batch) = batches.try_next().await? {
        let rows: Vec<NewArchived> = batch
            .iter()
            .map(|p| NewArchived {
                id: p.id,
                sku: &p.sku,
            })
            .collect();
        written += NewArchived::copy_in(&writer, &rows).await?;
    }
    assert_eq!(written, 25_000);
    assert_eq!(Archived::count(&writer).await?, 25_000);

    reader
        .batch_execute("DROP SCHEMA shelf_ch12_archive CASCADE")
        .await?;
    Ok(())
}
```

Writing on `reader` instead would wait forever behind the stream's unread rows (§12.4). The
tables live in a named schema because a temporary table belongs to the session that created it,
and this program has two. `try_chunks` yields a `TryChunksError` carrying the items so far when
the stream fails, which `?` turns into an `anyhow::Error`. `INSERT … SELECT` in SQL would do the
same copy in one statement without a round trip per batch; the stream is for when the batches go
somewhere SQL cannot reach.

**3.** A staging model that cannot reject anything, COPY into it, and one upsert:

```rust
use shelf::{FromRow, Insert, Table};

#[derive(Debug, FromRow, Table)]
#[shelf(table = "products")]
struct Product {
    #[shelf(primary_key, generated)]
    id: i64,
    sku: String,
    price_cents: i64,
}

/// The staging table: no key of its own beyond a row number, no constraints.
#[derive(Debug, FromRow, Table)]
#[shelf(table = "price_import")]
#[allow(dead_code)] // only written to
struct PriceRow {
    #[shelf(primary_key, generated)]
    line: i64,
    sku: String,
    price_cents: i64,
}

#[derive(Insert)]
#[shelf(model = PriceRow)]
struct NewPriceRow {
    sku: String,
    price_cents: i64,
}

#[tokio::main]
async fn main() -> Result<(), shelf::Error> {
    let mut client = shelf_playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE products (
                 id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
                 sku text NOT NULL UNIQUE, price_cents bigint NOT NULL
             );
             INSERT INTO products (sku, price_cents) VALUES ('TEE-01', 2499), ('MUG-01', 1299);",
        )
        .await?;
    let prices = vec![
        NewPriceRow {
            sku: "TEE-01".into(),
            price_cents: 1999,
        },
        NewPriceRow {
            sku: "CAP-01".into(),
            price_cents: 1899,
        },
    ];

    let tx = client.transaction().await?;
    tx.batch_execute(
        "CREATE TEMP TABLE price_import (
             line bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
             sku text, price_cents bigint
         ) ON COMMIT DROP",
    )
    .await?;
    NewPriceRow::copy_in(&tx, &prices).await?;
    let changed = tx
        .execute(
            "INSERT INTO products (sku, price_cents)
             SELECT sku, price_cents FROM price_import ORDER BY line
             ON CONFLICT (sku) DO UPDATE SET price_cents = excluded.price_cents",
            &[],
        )
        .await?;
    tx.commit().await?;

    let products = Product::query()
        .order_by(Product::SKU.asc())
        .all(&client)
        .await?;
    let listed: Vec<(&str, i64)> = products
        .iter()
        .map(|p| (p.sku.as_str(), p.price_cents))
        .collect();
    assert_eq!(changed, 2);
    assert_eq!(
        listed,
        [("CAP-01", 1899), ("MUG-01", 1299), ("TEE-01", 1999)]
    );
    Ok(())
}
```

The staging table is created in the transaction, so the COPY cannot fail to start, and `ON COMMIT
DROP` removes it with the transaction. The upsert is SQL because it is one statement over a set,
which is what SQL is for; shelf's part is the struct that knows the staging table's columns and
types. A price list from a client would also be validated in the staging table first (main book
§10.4).

---

Previous: [Chapter 11 — Relations without N+1](11-relations.md) · Next: [Chapter 13 — Trusting the mapping: schema checks and tests](13-trusting-the-mapping.md)
