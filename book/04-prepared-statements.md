# Chapter 4 — Prepared statements and the extended protocol

> **Async PostgreSQL in Rust** · Previous: [Chapter 3 — Rows and the type mapping](03-rows-and-types.md) ·
> Next: [Chapter 5 — Custom types: ToSql and FromSql](05-custom-types.md)

On localhost a query takes a few dozen microseconds, and it hardly matters how many messages
it needs. Across a network, every round trip between client and server costs a fixed
time (half a millisecond inside a data centre, tens of milliseconds between regions) and the
count of round trips becomes the most important number in your program's performance. This
chapter teaches you to count them.

It starts with what the friendly `client.query("…", &params)` actually sends, which explains
why tokio-postgres's API has *statements* at all. Then come the tools for doing better:
preparing a statement once and running it many times, `query_typed` for one-shot queries in a
single round trip, and caching statements. Along the way you will meet the two errors that
prepared statements are known for.

---

## 4.1 What `query` sends

A `Row` knows the PostgreSQL type of every column, and a parameter is checked against a
PostgreSQL type before it is sent (Chapter 3). Where do those types come from? From the server,
which is the only party that can tell what `$1` means in `WHERE price_cents > $1`. So when you
pass SQL text to `query`, tokio-postgres does two exchanges with the server:

```text
 client                                              server
   │ Parse "s7": SELECT … WHERE price_cents > $1       │
   │ Describe "s7"                                     │   round trip 1:
   │ Sync ────────────────────────────────────────────▶│   what does it take,
   │◀──── ParseComplete, ParameterDescription [int8],  │   what does it return?
   │      RowDescription [sku text], ReadyForQuery     │
   │                                                   │
   │ Bind "s7" with $1 = 3000 (binary int8)            │   round trip 2:
   │ Execute                                           │   run it
   │ Sync ────────────────────────────────────────────▶│
   │◀──── BindComplete, DataRow…, CommandComplete,     │
   │      ReadyForQuery                                │
   │ Close "s7", Sync ────────────────────────────────▶│   (not awaited)
```

The first exchange *prepares* the statement: the server parses it, works out the parameter and
result types, and keeps it under a name (tokio-postgres numbers them `s0`, `s1` and so on). The
second *executes* it with your values, encoded in binary now that their types are known.
Finally, because the statement was only needed for this one call, tokio-postgres tells the
server to forget it, without waiting for an answer.

Two round trips per query is the price of passing SQL text. Most of the time you do not have to
pay it.

> **Protocol lens:** these are the messages of PostgreSQL's *extended query protocol*. Chapter 2
> showed the *simple* one, a single *Query* message carrying SQL text with no parameters, in
> one round trip. The extended protocol splits the same work into steps so that statements
> can be reused, values can travel separately and in binary, and results can be fetched in
> parts (portals, Chapter 9). Every tokio-postgres method except `simple_query` and
> `batch_execute` uses it.

## 4.2 `prepare`: one parse, many executions

`Client::prepare` does the first exchange on its own and gives you a `Statement`. Running a
`Statement` costs one round trip:

```rust
use tokio_postgres::types::Type;

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE products (sku text PRIMARY KEY, price_cents bigint);
             INSERT INTO products VALUES ('CHAIR-01', 4999), ('LAMP-01', 2999);",
        )
        .await?;

    // Parse and describe once...
    let cheaper_than = client
        .prepare("SELECT sku FROM products WHERE price_cents < $1 ORDER BY sku")
        .await?;
    // ...and the statement knows what the server worked out.
    assert_eq!(cheaper_than.params(), [Type::INT8]);
    assert_eq!(cheaper_than.columns()[0].name(), "sku");
    assert_eq!(cheaper_than.columns()[0].type_(), &Type::TEXT);

    // Run it many times: one round trip each.
    for (limit, expected) in [(9999_i64, 2), (3000, 1), (1000, 0)] {
        let rows = client.query(&cheaper_than, &[&limit]).await?;
        assert_eq!(rows.len(), expected);
    }

    // While the Statement lives, the server keeps it, in this session only.
    let prepared: Vec<String> = client
        .query_scalar("SELECT statement FROM pg_prepared_statements", &[])
        .await?;
    assert!(prepared.iter().any(|sql| sql.contains("price_cents < $1")));

    drop(cheaper_than); // sends Close: the server forgets it
    Ok(())
}
```

Every method that runs SQL accepts either text or a statement, through the `ToStatement` trait,
implemented for `str`, `String` and `Statement`: `client.query("…", …)` and `client.query(&stmt,
…)` are the same method. A `Statement` is cheap to clone (it is reference-counted), and the
server-side statement is closed when the last clone is dropped.

A statement belongs to the session that prepared it. Use it on another connection and the
server does not know the name:

```rust
use tokio_postgres::error::SqlState;

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let one = playground::connect().await?;
    let other = playground::connect().await?;

    let statement = one.prepare("SELECT 1").await?;
    let error = other.query(&statement, &[]).await.unwrap_err();
    assert_eq!(error.code(), Some(&SqlState::INVALID_SQL_STATEMENT_NAME)); // 26000
    let message = error.as_db_error().unwrap().message();
    assert!(message.starts_with("prepared statement \"s"), "{message}");
    Ok(())
}
```

That is why a pool needs a statement cache *per connection* (§4.6, Chapter 13), and why a
statement prepared through a pooled connection must not be kept after the connection goes back.

> **SQL lens:** a prepared statement saves the server work too: the parse, and for repeated
> executions possibly the plan. PostgreSQL plans each of the first five executions with the
> actual values (a *custom* plan), then may switch to a *generic* plan, made once without
> values, if it estimates that is not worse. For skewed data, such as a `status` column where
> 'pending' is rare and 'done' is everywhere, a generic plan can be a bad fit for some values.
> The `plan_cache_mode` setting (`force_custom_plan`) overrides the choice when you find such
> a case.

## 4.3 When the server cannot infer a type

The server infers each parameter's type from its context: from the column it is compared with
or assigned to, or from an operator. Usually that works. When there is no context, preparing
fails:

```rust
use tokio_postgres::error::SqlState;
use tokio_postgres::types::Type;

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = playground::connect().await?;

    // What the server infers from context:
    let inferred = client
        .prepare("SELECT $1 + 1, $2 || 'x', $3::int8, $4")
        .await?;
    assert_eq!(
        inferred.params(),
        [Type::INT4, Type::TEXT, Type::INT8, Type::TEXT]
    );

    // No context at all: the server gives up.
    let error = client.prepare("SELECT $1 IS NULL").await.unwrap_err();
    assert_eq!(error.code(), Some(&SqlState::INDETERMINATE_DATATYPE)); // 42P18
    assert_eq!(
        error.as_db_error().unwrap().message(),
        "could not determine data type of parameter $1"
    );

    // Fix 1: a cast in the SQL.
    let with_cast = client.prepare("SELECT $1::int8 IS NULL").await?;
    assert_eq!(with_cast.params(), [Type::INT8]);

    // Fix 2: tell prepare the types (the list may be shorter than the parameters; the
    // server infers the rest).
    let typed = client
        .prepare_typed("SELECT $1 IS NULL", &[Type::INT8])
        .await?;
    let is_null: bool = client.query_one(&typed, &[&None::<i64>]).await?.get(0);
    assert!(is_null);
    Ok(())
}
```

Note the last line of the inference examples: a bare `$4` becomes `text`. So does `SELECT $1`,
which is why `client.query_one("SELECT $1", &[&42_i32])` fails on the client with "cannot
convert between the Rust type `i32` and the Postgres type `text`". The server decided the type,
and the Rust value has to match it, not the other way round.

A cast is usually the better fix: it documents the type where the parameter is, and it works
the same in psql. `prepare_typed` is for SQL you do not control, and for the next section.

## 4.4 `query_typed`: one round trip, no statement

Sometimes you run a query once, or you build it dynamically and it is never the same twice.
Preparing it first is a wasted round trip. `query_typed` (0.7.11+; `query_typed_one`,
`query_typed_opt` and `execute_typed` since 0.7.17) skips the prepare step: you give each
parameter's type along with its value, and tokio-postgres sends parse, bind and execute
together, in one round trip, using an unnamed statement that nobody has to close:

```rust
use tokio_postgres::types::Type;

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE products (sku text PRIMARY KEY, price_cents bigint);
             INSERT INTO products VALUES ('CHAIR-01', 4999), ('LAMP-01', 2999);",
        )
        .await?;

    // Each parameter comes with the PostgreSQL type it is sent as.
    let row = client
        .query_typed_one(
            "SELECT count(*) FROM products WHERE price_cents BETWEEN $1 AND $2",
            &[(&1000_i64, Type::INT8), (&3000_i64, Type::INT8)],
        )
        .await?;
    assert_eq!(row.get::<_, i64>(0), 1);

    // The types you declare must still fit the Rust values; this is checked before sending.
    let error = client
        .query_typed_one("SELECT $1 + 1", &[(&1_i64, Type::INT4)])
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "error serializing parameter 0");

    let changed = client
        .execute_typed(
            "UPDATE products SET price_cents = $1 WHERE sku = $2",
            &[(&3999_i64, Type::INT8), (&"CHAIR-01", Type::TEXT)],
        )
        .await?;
    assert_eq!(changed, 1);
    Ok(())
}
```

What you give up: the server parses and plans the statement on every call, and you are the one
stating the types, so you can get them wrong. What you gain: one round trip instead of two, and
no named statements on the server. That last point matters behind connection poolers that hand
a different server connection to each transaction (PgBouncer in transaction mode, Chapter 13):
a named statement prepared on one server connection does not exist on the next, and unnamed
ones avoid the problem entirely.

| Call | Round trips | Statement on the server |
|---|---|---|
| `query("…", params)` | 2 (prepare, execute) | named, closed after the call |
| `prepare("…")` once, then `query(&stmt, params)` | 1 + 1 per execution | named, kept while the `Statement` lives |
| `query_typed("…", params_with_types)` | 1 | unnamed, replaced by the next one |
| `simple_query("…")` / `batch_execute` | 1 | none (no parameters at all) |

## 4.5 Counting round trips

On localhost the difference between those rows is invisible. To see it, put a slow network in
the way: [lagproxy](../apps/lagproxy/src/lib.rs), from this repository, forwards connections to
PostgreSQL and holds every packet back for a few milliseconds in each direction. Start it, point
`DATABASE_URL` at it, and the same program measures what each call costs:

```bash
cargo run -p lagproxy -- --upstream localhost:5432 --delay-ms 5    # listens on :6543
export DATABASE_URL=postgres://postgres:secret@127.0.0.1:6543/tpbook
export LAGPROXY_DELAY_MS=5
```

<!-- check: needs=lagproxy -->
```rust
use std::time::{Duration, Instant};
use tokio_postgres::types::Type;

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    // Through lagproxy: every round trip costs at least twice the delay.
    let client = playground::connect().await?;
    let delay: u64 = std::env::var("LAGPROXY_DELAY_MS").map_or(5, |v| v.parse().unwrap());
    let delay = Duration::from_millis(delay);
    const N: u32 = 10;

    let started = Instant::now();
    for i in 0..N as i64 {
        client.query_one("SELECT $1::int8 + 1", &[&i]).await?;
    }
    let as_text = started.elapsed() / N;

    let statement = client.prepare("SELECT $1::int8 + 1").await?;
    let started = Instant::now();
    for i in 0..N as i64 {
        client.query_one(&statement, &[&i]).await?;
    }
    let prepared = started.elapsed() / N;

    let started = Instant::now();
    for i in 0..N as i64 {
        client
            .query_typed_one("SELECT $1::int8 + 1", &[(&i, Type::INT8)])
            .await?;
    }
    let typed = started.elapsed() / N;

    println!("per query: text {as_text:?}, prepared {prepared:?}, typed {typed:?}");
    assert!(prepared >= delay * 2, "at least one round trip");
    assert!(as_text > prepared * 3 / 2, "about two round trips");
    assert!(typed < prepared * 3 / 2, "about one round trip");
    Ok(())
}
```

With a 5 ms delay each way, on the machine this book was written on:

```text
per query: text 30.6ms, prepared 15.3ms, typed 15.3ms
```

A round trip through the proxy costs about 15 ms rather than 10, because a timer is never
exactly on time (lagproxy's documentation explains the overshoot). The proportions are what
count: passing SQL text costs two round trips, a prepared statement or `query_typed` one.

> **Pitfall:** "it's fast on my machine" is the most common way round trips hide. A request
> handler that runs ten small queries one after another takes 0.5 ms on localhost and 50 ms in
> production with a 5 ms network. Measure through lagproxy (or any latency injector) before
> you trust a performance number, and count the round trips of your hot paths. Chapter 7 shows
> how to run independent queries *without* waiting for each other.

## 4.6 Caching statements, and when the cache goes stale

Preparing every statement once per connection and reusing it is the way to pay one round trip
per query without stating types by hand. A small cache does it: look the SQL up, prepare on a
miss, keep the `Statement`.

```rust
use std::collections::HashMap;
use std::sync::Mutex;
use tokio_postgres::types::ToSql;
use tokio_postgres::{Client, Error, Row, Statement};

/// A client with a statement cache. One cache per connection: statements are per session.
struct CachingClient {
    client: Client,
    statements: Mutex<HashMap<String, Statement>>,
}

impl CachingClient {
    async fn prepare_cached(&self, sql: &str) -> Result<Statement, Error> {
        if let Some(statement) = self.statements.lock().unwrap().get(sql) {
            return Ok(statement.clone()); // cheap: reference-counted
        }
        // The lock is not held across the await. Two tasks may both miss and prepare the
        // same SQL; one of the statements is then dropped (and closed). Harmless.
        let statement = self.client.prepare(sql).await?;
        self.statements
            .lock()
            .unwrap()
            .insert(sql.to_owned(), statement.clone());
        Ok(statement)
    }

    async fn query(&self, sql: &str, params: &[&(dyn ToSql + Sync)]) -> Result<Vec<Row>, Error> {
        let statement = self.prepare_cached(sql).await?;
        self.client.query(&statement, params).await
    }
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let db = CachingClient {
        client: playground::connect().await?,
        statements: Mutex::new(HashMap::new()),
    };
    for n in 0..5_i64 {
        let rows = db.query("SELECT $1::int8 * 2", &[&n]).await?;
        assert_eq!(rows[0].get::<_, i64>(0), n * 2);
    }
    assert_eq!(db.statements.lock().unwrap().len(), 1, "prepared once");
    Ok(())
}
```

You will rarely write this yourself: deadpool-postgres's pooled connections have exactly this
cache built in, as `prepare_cached` (Chapter 13), and stockroom uses it everywhere. But a cache
of prepared statements has a failure mode you need to know about. A statement's result columns
are fixed when it is prepared. Change the table underneath, and the old statement refuses to
run:

```rust
use tokio_postgres::error::SqlState;

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = playground::connect().await?;
    client
        .batch_execute("CREATE TEMP TABLE t (a int); INSERT INTO t VALUES (1)")
        .await?;
    let everything = client.prepare("SELECT * FROM t").await?;

    // A migration adds a column: `SELECT *` would now return two columns.
    client
        .batch_execute("ALTER TABLE t ADD COLUMN b int")
        .await?;
    let error = client.query(&everything, &[]).await.unwrap_err();
    assert_eq!(error.code(), Some(&SqlState::FEATURE_NOT_SUPPORTED)); // 0A000
    assert_eq!(
        error.as_db_error().unwrap().message(),
        "cached plan must not change result type"
    );

    // The cure is to prepare it again.
    let everything = client.prepare("SELECT * FROM t").await?;
    assert_eq!(client.query(&everything, &[]).await?[0].len(), 2);
    Ok(())
}
```

> **Pitfall:** "cached plan must not change result type" in production means a schema change
> reached a service that had statements prepared against the old schema: typically a migration
> run by one instance while others were serving. A per-connection cache keeps handing out the
> broken statement until the connection is replaced. Three habits prevent it. Never `SELECT *`
> in application code (an added column then changes nothing). Make migrations additive.
> And when you must change result types, clear the statement caches or recycle the pool's
> connections after migrating.

### Side by side — *prepared statements*

The task: prepare a query with one parameter once, and run it with two different values.

```sql
-- psql: SQL-level PREPARE / EXECUTE, which any client can send...
tpbook=# PREPARE cheaper_than(bigint) AS
tpbook-#   SELECT sku FROM products WHERE price_cents < $1 ORDER BY sku;
PREPARE
tpbook=# EXECUTE cheaper_than(3000);
   sku
---------
 LAMP-01
(1 row)

tpbook=# DEALLOCATE cheaper_than;
DEALLOCATE
-- ...or, since psql 18, the protocol-level messages tokio-postgres uses:
tpbook=# SELECT sku FROM products WHERE price_cents < $1 ORDER BY sku \parse cheaper_than
tpbook=# \bind_named cheaper_than 9999 \g
tpbook=# \close_prepared cheaper_than
```

~~~rust
// sqlx (illustration)
// Every query is prepared on first use and cached on its connection, automatically.
let skus: Vec<String> = sqlx::query_scalar("SELECT sku FROM products WHERE price_cents < $1 ORDER BY sku")
    .bind(3000_i64)
    .fetch_all(&pool)
    .await?;
// .persistent(false) opts a query out of the cache.
~~~

```rust
#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE products (sku text PRIMARY KEY, price_cents bigint);
             INSERT INTO products VALUES ('CHAIR-01', 4999), ('LAMP-01', 2999);",
        )
        .await?;
    let cheaper_than = client
        .prepare("SELECT sku FROM products WHERE price_cents < $1 ORDER BY sku")
        .await?;
    let cheap: Vec<String> = client
        .query(&cheaper_than, &[&3000_i64])
        .await?
        .iter()
        .map(|row| row.get(0))
        .collect();
    let all = client.query(&cheaper_than, &[&9999_i64]).await?;
    assert_eq!((cheap, all.len()), (vec!["LAMP-01".to_owned()], 2));
    Ok(())
}
```

| | psql | sqlx | tokio-postgres |
|---|---|---|---|
| **Same** | A named, per-session statement, executed with values | Statements prepared per connection, executed with bound values | A named, per-session statement, executed with values |
| **Different** | `PREPARE` is SQL, parsed by the server like any statement; `\parse` (psql 18) uses the protocol | Caching is automatic and invisible: you never hold a statement | Explicit: you prepare and hold a `Statement` (or use deadpool's `prepare_cached`); text queries pay a second round trip; `query_typed` avoids both |

---

## In stockroom

Every repository function in stockroom prepares through the pool's cache, so each statement is
parsed once per connection and every later call costs one round trip. The catalog's paginated
listing:

~~~rust
// stockroom/src/repo/products.rs (excerpt)
pub async fn page(
    db: &impl GenericClient,
    after: Option<&Sku>,
    tag: Option<&str>,
    limit: i64,
) -> Result<Vec<Product>, AppError> {
    let stmt = db
        .prepare_cached(
            "SELECT id, sku, name, price_cents, tags, attributes, version, created_at, updated_at
             FROM products
             WHERE sku > coalesce($1, '') AND ($2::text IS NULL OR tags @> ARRAY[$2])
             ORDER BY sku
             LIMIT $3",
        )
        .await?;
    from_rows(&db.query(&stmt, &[&after, &tag, &limit]).await?)
}
~~~

Look at the parameters through this chapter's eyes. `$3` gets its type, `bigint`, from `LIMIT`.
`$1` is compared with a `sku` through `coalesce($1, '')`, which makes it `text`. And `$2`
appears twice, so it carries an explicit cast at its first use, `$2::text`, because `$2 IS NULL`
alone would be the "could not determine data type" error from §4.3. The column list is spelled
out rather than `*`, so a migration that adds a column to `products` cannot break the cached
statement.

stockroom also guards against the stale-statement problem at startup: `serve` runs the
migrations *before* it creates the pool ([app.rs](../stockroom/src/app.rs)), so no connection
has prepared anything against the old schema.

The tiny psql in this repository uses `prepare` for something other than speed: its
`--describe` mode prepares the statement and prints what the server worked out, without ever
running it:

~~~rust
// apps/pgq/src/lib.rs (excerpt)
pub async fn describe(client: &Client, sql: &str) -> Result<String, Error> {
    let statement = client.prepare(sql).await?;
    // … renders statement.params() and statement.columns() as two tables …
}
~~~

```text
$ cargo run -p pgq -- --describe -c 'SELECT relname, relpages FROM pg_class WHERE oid = $1'
 parameter | type
-----------+------
 $1        | oid

 column   | type | from table
----------+------+---------------------
 relname  | name | oid 1259, column 2
 relpages | int4 | oid 1259, column 10
```

## Summary

- Passing SQL text to `query`, `execute` and friends costs two round trips: prepare (types
  in, types out) and execute. A temporary named statement is created and closed.
- `prepare` returns a `Statement` with `params()` and `columns()`. Executing it costs one round
  trip. Statements belong to one session: using one elsewhere fails with 26000.
- When a parameter's type cannot be inferred (42P18), add a cast (`$1::int8`) or use
  `prepare_typed`. The server decides parameter types; Rust values must fit them.
- `query_typed`, `query_typed_one`, `query_typed_opt` and `execute_typed` take the types with
  the values and need one round trip, with no named statement: good for one-off and dynamic
  SQL, and behind transaction-mode poolers.
- Cache statements per connection (deadpool's `prepare_cached` does). After a schema change,
  cached statements can fail with "cached plan must not change result type": avoid
  `SELECT *`, migrate additively, and re-prepare after migrating.
- Measure through added latency (lagproxy): round trips only show on a real network.

## Exercises

1. **Count them yourself.** With lagproxy at a 10 ms delay, time a loop of 20 `execute` calls
   that each insert one row into a temporary table, first with SQL text and then with a
   prepared statement. Predict both times before you run it.
   *Hint:* at a 10 ms delay, a round trip through lagproxy measured about 26 ms.
2. **Infer it.** For each of these, predict the type the server gives `$1`, then check with
   `prepare` or `pgq --describe`: `SELECT $1 = 1`, `SELECT length($1)`, `SELECT now() - $1`,
   `SELECT ARRAY[$1, 2]`, `SELECT coalesce($1, 0.5)`.
   *Hint:* the server infers each parameter's type from where it appears; `Statement::params()`
   reports what it chose, and not every expression gives it enough to choose.
3. **A dynamic query in one round trip.** Rewrite Chapter 2's `search` (optional filters) with
   `query_typed`, so that every combination of filters costs one round trip.
   *Hint:* the parameter list becomes `Vec<(&(dyn ToSql + Sync), Type)>`.
4. **Stale.** Using the cache from §4.6, write a test that shows the stale-statement failure
   after `ALTER TABLE`, then make `CachingClient::query` recover by itself: on SQLSTATE 0A000,
   drop the cached statement, prepare it again and retry once.
   *Hint:* the error is `SqlState::FEATURE_NOT_SUPPORTED` (0A000), "cached plan must not change
   result type"; Chapter 13 meets it again with deadpool's cache.

### Solutions

**1.** About 2 × 20 × 26 ms ≈ 1.04 s with text, and 20 × 26 ms ≈ 0.52 s (plus one round trip
for `prepare`) with the statement. Every `execute` waits for its answer before the next one is
sent, so the loop is a chain of round trips. Chapter 7 breaks the chain.

**2.** `$1 = 1` makes it `integer` (from the other operand), and `length($1)` makes it `text`
(the most common `length` takes text). `now() - $1` makes it **`timestamptz`**, not
`interval`: of the `-` operators with a `timestamptz` on the left, the server prefers the one
whose right side has the same type, so `now() - $1` subtracts two timestamps and returns an
interval. If you meant "now minus a duration", write `now() - $1::interval`. `ARRAY[$1, 2]`
gives `integer`, and `coalesce($1, 0.5)` gives `numeric`, the type of the other argument.
When the answer surprises you, cast.

**4.**

```rust
use std::collections::HashMap;
use std::sync::Mutex;
use tokio_postgres::error::SqlState;
use tokio_postgres::types::ToSql;
use tokio_postgres::{Client, Error, Row, Statement};

struct CachingClient {
    client: Client,
    statements: Mutex<HashMap<String, Statement>>,
}

impl CachingClient {
    async fn prepare_cached(&self, sql: &str) -> Result<Statement, Error> {
        if let Some(statement) = self.statements.lock().unwrap().get(sql) {
            return Ok(statement.clone());
        }
        let statement = self.client.prepare(sql).await?;
        self.statements
            .lock()
            .unwrap()
            .insert(sql.to_owned(), statement.clone());
        Ok(statement)
    }

    async fn query(&self, sql: &str, params: &[&(dyn ToSql + Sync)]) -> Result<Vec<Row>, Error> {
        let statement = self.prepare_cached(sql).await?;
        match self.client.query(&statement, params).await {
            Err(e) if e.code() == Some(&SqlState::FEATURE_NOT_SUPPORTED) => {
                // Stale after a schema change: forget it, prepare again, retry once.
                self.statements.lock().unwrap().remove(sql);
                let statement = self.prepare_cached(sql).await?;
                self.client.query(&statement, params).await
            }
            other => other,
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let db = CachingClient {
        client: playground::connect().await?,
        statements: Mutex::new(HashMap::new()),
    };
    db.client
        .batch_execute("CREATE TEMP TABLE t (a int); INSERT INTO t VALUES (1)")
        .await?;
    assert_eq!(db.query("SELECT * FROM t", &[]).await?[0].len(), 1);
    db.client
        .batch_execute("ALTER TABLE t ADD COLUMN b int")
        .await?;
    assert_eq!(
        db.query("SELECT * FROM t", &[]).await?[0].len(),
        2,
        "recovered"
    );
    Ok(())
}
```

0A000 is "feature not supported", a broad code. Matching on it alone is acceptable here,
because the retry is harmless; a stricter version also checks the message.

---

Previous: [Chapter 3 — Rows and the type mapping](03-rows-and-types.md) ·
Next: [Chapter 5 — Custom types: ToSql and FromSql](05-custom-types.md)
