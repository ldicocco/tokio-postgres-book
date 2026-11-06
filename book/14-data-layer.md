# Chapter 14 — Schema, migrations and the data layer

> **Async PostgreSQL in Rust** · Previous: [Chapter 13 — Connection pooling](13-pooling.md) ·
> Next: [Chapter 15 — axum I: a database-backed JSON API](15-axum-api.md)

A service that talks to a database makes two promises about it. The schema it expects must
exist, in exactly the shape it expects, on every database it runs against: the developer's,
the tests', and production's. And its SQL must be somewhere a reader can find it, check it,
and change it without hunting through HTTP handlers.

This chapter covers both. The first half is migrations: how stockroom carries its schema in its
binary and applies it safely, even when several instances start at once, and what the
ready-made alternative, refinery, does. The second half is the data layer: repository functions
that work on any connection or transaction, rows turned into types in one place, and domain
types that keep invalid values out of both the program and the database. It ends with a tool
that generates that layer from SQL.

---

## 14.1 Embedded migrations

A migration is a script that moves the schema from one version to the next. The usual design,
and stockroom's, has four parts:

- **Numbered SQL files** (`migrations/0001_init.sql`, `0002_order_events.sql`), never edited
  once applied: a change is a new file.
- **Compiled into the binary** with `include_str!`, so that a build carries the schema it
  expects, and a deployment cannot pair the wrong files with the wrong code.
- **A table that records what was applied** (`schema_migrations`), with each migration's
  version and a checksum of its text.
- **A runner** that applies, in order, the migrations not yet recorded, and refuses to run if an
  applied migration's text has changed since.

~~~rust
// stockroom/src/db/migrate.rs (excerpt)
pub const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "init",
        sql: include_str!("../../migrations/0001_init.sql"),
    },
    Migration {
        version: 2,
        name: "order_events",
        sql: include_str!("../../migrations/0002_order_events.sql"),
    },
];
~~~

[stockroom/src/db/migrate.rs](../stockroom/src/db/migrate.rs)

The runner fits in a page. This one is stockroom's, reduced to its essentials, and run the way
a deployment runs it: two instances start at the same moment, and both try to migrate. (It works
in a schema of its own, which it drops at the end.)

```rust
use std::collections::HashMap;
use tokio_postgres::Client;

type Error = Box<dyn std::error::Error>;

struct Migration {
    version: i32,
    name: &'static str,
    sql: &'static str,
}

const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "products",
        sql: "CREATE TABLE product (sku text PRIMARY KEY, name text NOT NULL);",
    },
    Migration {
        version: 2,
        name: "prices",
        sql: "ALTER TABLE product ADD COLUMN price_cents bigint NOT NULL DEFAULT 0;",
    },
];

/// FNV-1a, 64 bits, as hex: enough to notice an edited migration.
fn checksum(sql: &str) -> String {
    let hash = sql.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    });
    format!("{hash:016x}")
}

/// Applies the migrations not yet recorded, each in its own transaction, under an advisory
/// lock so that concurrent runners take turns. Returns the versions it applied.
async fn migrate(client: &mut Client, migrations: &[Migration]) -> Result<Vec<i32>, Error> {
    // One lock per schema, so that this example cannot collide with anything else.
    let lock = "SELECT pg_advisory_lock(hashtext(current_schema()))";
    client.execute(lock, &[]).await?;
    let result = apply(client, migrations).await;
    client
        .execute("SELECT pg_advisory_unlock(hashtext(current_schema()))", &[])
        .await?;
    result
}

async fn apply(client: &mut Client, migrations: &[Migration]) -> Result<Vec<i32>, Error> {
    client
        .batch_execute(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                 version    integer     PRIMARY KEY,
                 name       text        NOT NULL,
                 checksum   text        NOT NULL,
                 applied_at timestamptz NOT NULL DEFAULT now()
             )",
        )
        .await?;
    let applied: HashMap<i32, String> = client
        .query("SELECT version, checksum FROM schema_migrations", &[])
        .await?
        .iter()
        .map(|row| (row.get(0), row.get(1)))
        .collect();
    for m in migrations {
        if let Some(recorded) = applied.get(&m.version)
            && *recorded != checksum(m.sql)
        {
            return Err(format!(
                "migration {} ({}) was edited after it was applied",
                m.version, m.name
            )
            .into());
        }
    }

    let mut done = Vec::new();
    for m in migrations
        .iter()
        .filter(|m| !applied.contains_key(&m.version))
    {
        let tx = client.transaction().await?;
        tx.batch_execute(m.sql).await?;
        tx.execute(
            "INSERT INTO schema_migrations (version, name, checksum) VALUES ($1, $2, $3)",
            &[&m.version, &m.name, &checksum(m.sql)],
        )
        .await?;
        tx.commit().await?;
        done.push(m.version);
    }
    Ok(done)
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    // A schema of our own, so that the example leaves nothing behind. Its name is made of
    // digits generated here, never of input, which is what makes formatting it into SQL safe.
    let schema = format!("ch14_{}", std::process::id());
    let mut a = playground::connect().await?;
    let mut b = playground::connect().await?;
    a.batch_execute(&format!("CREATE SCHEMA {schema}")).await?;
    for client in [&a, &b] {
        client
            .batch_execute(&format!("SET search_path = {schema}"))
            .await?;
    }

    // Two instances start at once: one migrates; the other waits for the lock, then finds
    // nothing left to do.
    let (first, second) = tokio::join!(migrate(&mut a, MIGRATIONS), migrate(&mut b, MIGRATIONS));
    let mut applied = [first?, second?];
    applied.sort();
    assert_eq!(applied, [vec![], vec![1, 2]]);

    // Running again is a no-op...
    assert!(migrate(&mut a, MIGRATIONS).await?.is_empty());

    // ...and an applied migration whose text changed is refused.
    let edited = [
        Migration {
            sql: MIGRATIONS[0].sql,
            ..MIGRATIONS[0]
        },
        Migration {
            sql: "ALTER TABLE product ADD COLUMN price_cents numeric;",
            ..MIGRATIONS[1]
        },
    ];
    let err = migrate(&mut a, &edited).await.unwrap_err();
    println!("{err}");

    a.batch_execute(&format!("DROP SCHEMA {schema} CASCADE"))
        .await?;
    Ok(())
}
```

```text
migration 2 (prices) was edited after it was applied
```

The checksum is not about security; it catches the honest mistake of fixing a migration in
place after it has run somewhere, which leaves databases that ran the old text and databases
that ran the new one with different schemas and the same version number.

## 14.2 Running migrations safely

> **SQL lens:** in PostgreSQL, DDL is transactional. `CREATE TABLE`, `ALTER TABLE`,
> `CREATE FUNCTION` and nearly all their kin take part in the surrounding transaction and roll
> back with it. So a migration that fails halfway leaves *nothing* behind: not half a table, not
> a column without its index. That is why the runner gives each migration its own transaction
> and records it in the same one. The exceptions are the commands that cannot run in a
> transaction block at all, chiefly `CREATE INDEX CONCURRENTLY` (and `DROP INDEX
> CONCURRENTLY`, `VACUUM`, `CREATE DATABASE`).

```rust
use tokio_postgres::error::SqlState;

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let mut client = playground::connect().await?;

    // A migration that fails halfway: its first two statements are undone with it.
    let tx = client.transaction().await?;
    let err = tx
        .batch_execute(
            "CREATE TEMP TABLE supplier (id int PRIMARY KEY);
             ALTER TABLE supplier ADD COLUMN name text NOT NULL;
             CREATE INDEX ON supplier (nmae);",
        )
        .await
        .unwrap_err();
    println!("{}", err.as_db_error().expect("a server error").message());
    drop(tx);
    let exists: bool = client
        .query_one("SELECT to_regclass('pg_temp.supplier') IS NOT NULL", &[])
        .await?
        .get(0);
    assert!(!exists, "nothing of the failed migration is left");

    // The exception: CREATE INDEX CONCURRENTLY refuses to run in a transaction block...
    let schema = format!("ch14_{}", std::process::id());
    client
        .batch_execute(&format!(
            "CREATE SCHEMA {schema}; CREATE TABLE {schema}.supplier (id int, name text)"
        ))
        .await?;
    let tx = client.transaction().await?;
    let err = tx
        .batch_execute(&format!(
            "CREATE INDEX CONCURRENTLY ON {schema}.supplier (name)"
        ))
        .await
        .unwrap_err();
    println!("{}", err.as_db_error().expect("a server error").message());
    assert_eq!(err.code(), Some(&SqlState::ACTIVE_SQL_TRANSACTION)); // 25001
    drop(tx);

    // ...including the implicit one around several statements in one batch_execute.
    let err = client
        .batch_execute(&format!(
            "SELECT 1; CREATE INDEX CONCURRENTLY ON {schema}.supplier (name)"
        ))
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some(&SqlState::ACTIVE_SQL_TRANSACTION));

    // Alone, it works.
    client
        .batch_execute(&format!(
            "CREATE INDEX CONCURRENTLY ON {schema}.supplier (name)"
        ))
        .await?;
    client
        .batch_execute(&format!("DROP SCHEMA {schema} CASCADE"))
        .await?;
    Ok(())
}
```

```text
column "nmae" does not exist
CREATE INDEX CONCURRENTLY cannot run inside a transaction block
```

(The example uses a real table for the second half because PostgreSQL builds indexes on
temporary tables non-concurrently anyway.) A runner that supports such migrations must run them
outside its usual transaction, one statement per `batch_execute`, and accept that a failure in
the middle leaves an invalid index behind to drop and rebuild. Many teams keep them out of
automatic migrations and run them by hand.

**Several instances, one migration.** When three replicas of a service start together and each
migrates on startup, two of them must wait. A session-level advisory lock does that without a
table to lock: `pg_advisory_lock(key)` blocks until no other session holds `key`, and the lock
is held across the transactions that follow until `pg_advisory_unlock`, or until the session
ends, which matters if the instance crashes mid-migration. The second instance then reads
`schema_migrations`, finds everything applied, and moves on. The `tokio::join!` in §14.1 is
that scenario in miniature. stockroom's key is a constant everyone agrees on:

~~~rust
// stockroom/src/db/migrate.rs (excerpt)
/// The advisory lock all instances agree on: "STOCKROO" in ASCII.
const LOCK_KEY: i64 = 0x5354_4f43_4b52_4f4f;
~~~

Three more details from stockroom's runner are worth copying:

- **Lift the guard rails for the migration session.** stockroom's sessions carry a 5-second
  `statement_timeout` and a 2-second `lock_timeout` (Chapter 12); a migration may legitimately
  take longer and wait for locks, so the runner runs `SET statement_timeout = 0; SET
  lock_timeout = 0` on its own connection first.
- **Clear the type cache afterwards.** tokio-postgres caches what it learned about custom types
  (Chapter 5); after a migration changes one, `client.clear_type_cache()` makes the next query
  look it up afresh.
- **Log the notices.** `CREATE TABLE IF NOT EXISTS` reports "relation … already exists, skipping"
  as a notice. stockroom's `db::connect` drives its connection with `poll_message` (Chapter 11)
  and logs every notice, so a migration's server-side messages end up in the service's log.

Migrate either at startup, under the lock (simple, and what stockroom's `serve` does unless
started with `--migrate false`), or in a
separate deployment step before the new version starts (`stockroom migrate`), which keeps a
failed migration from taking down every starting instance. Either way, write migrations that
the *previous* version of the code can live with, because during a rolling deployment both run
at once: add a column before using it, stop using it before dropping it.

## 14.3 refinery, the ready-made alternative

[refinery](https://docs.rs/refinery/0.10.0/refinery/) (0.10.0, checked 2026-11-06) is the most
used migration crate for tokio-postgres. Its migrations are files named `V1__init.sql`,
`V2__order_events.sql` (or `U…` for non-contiguous versions), embedded at compile time by a
macro, and recorded in a `refinery_schema_history` table with their checksums:

~~~rust
// refinery (illustration): refinery = { version = "0.10", features = ["tokio-postgres"] }
mod embedded {
    refinery::embed_migrations!("migrations");
}

let report = embedded::migrations::runner()
    .set_abort_divergent(true) // the default: refuse edited migrations
    .run_async(&mut client)    // a &mut tokio_postgres::Client
    .await?;
for migration in report.applied_migrations() {
    tracing::info!(%migration, "applied");
}
~~~

It does what §14.1's runner does, with more options: `set_grouped(true)` runs all pending
migrations in one transaction, `set_target` stops at a version, and migrations can also be Rust
functions that return SQL. Through deadpool, `run_async` wants the `tokio_postgres::Client`
underneath the `Object` (`object.deref_mut().deref_mut()`, as its README shows).

What it does not do is lock. Its tokio-postgres driver opens a transaction per migration and
nothing else (`refinery_core/src/drivers/tokio_postgres.rs`), so with several instances
starting together, take an advisory lock around `run_async` yourself, exactly as §14.1 does, or
migrate in a separate deployment step. This book writes its own runner because it is short and
teaches both of those lessons; refinery is a sound choice when you would rather not own one.

## 14.4 The repository layer

stockroom keeps every SQL statement in `repo/`, one module per aggregate (`stores`, `products`,
`stock`, `orders`, `reports`, `outbox`), as plain async functions. Handlers never contain SQL;
they call repository functions. Three conventions make that work.

**Functions take any client.** A function that only reads or writes should not care whether it
runs on a pooled connection or inside a transaction someone else opened. tokio-postgres's
`GenericClient` (Chapter 8) expresses "a `Client` or a `Transaction`"; deadpool-postgres has a
`GenericClient` of its own, implemented by `Object` and deadpool's `Transaction`, which adds
`prepare_cached`. Both are sealed and `#[async_trait]` traits: you can write functions generic
over them, `&impl GenericClient`, but not implement them for your own types, nor use them as
`dyn GenericClient`.

**Statements are prepared once per connection.** `prepare_cached` (Chapter 13) means each
function's SQL is parsed and planned once per pooled connection, and then costs no extra round
trip.

**Rows become types in one place.** Each type implements `TryFrom<&Row>`, reading columns by
name, so a function's last line converts, and a column added to the `SELECT` but not to the
type is noticed in one place.

```rust
use deadpool_postgres::{GenericClient, Manager, Pool, Runtime};
use tokio_postgres::{Config, Error, NoTls, Row};

#[derive(Debug, PartialEq)]
struct Product {
    sku: String,
    price_cents: i64,
}

impl TryFrom<&Row> for Product {
    type Error = Error;

    fn try_from(row: &Row) -> Result<Self, Self::Error> {
        Ok(Product {
            sku: row.try_get("sku")?,
            price_cents: row.try_get("price_cents")?,
        })
    }
}

// The repository: SQL in one place, for any client.

async fn insert(db: &impl GenericClient, sku: &str, price_cents: i64) -> Result<Product, Error> {
    let stmt = db
        .prepare_cached(
            "INSERT INTO product (sku, price_cents) VALUES ($1, $2)
             RETURNING sku, price_cents",
        )
        .await?;
    Product::try_from(&db.query_one(&stmt, &[&sku, &price_cents]).await?)
}

async fn by_sku(db: &impl GenericClient, sku: &str) -> Result<Option<Product>, Error> {
    let stmt = db
        .prepare_cached("SELECT sku, price_cents FROM product WHERE sku = $1")
        .await?;
    db.query_opt(&stmt, &[&sku])
        .await?
        .as_ref()
        .map(Product::try_from)
        .transpose()
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config: Config = std::env::var("DATABASE_URL")?.parse()?;
    // One connection, so that the temporary table is visible to every borrower.
    let pool = Pool::builder(Manager::new(config, NoTls))
        .max_size(1)
        .runtime(Runtime::Tokio1)
        .build()?;
    let mut conn = pool.get().await?;
    conn.batch_execute("CREATE TEMP TABLE product (sku text PRIMARY KEY, price_cents bigint)")
        .await?;

    // On the pooled connection...
    insert(&conn, "CHAIR-01", 4999).await?;

    // ...and inside a transaction, with the same functions.
    let tx = conn.transaction().await?;
    insert(&tx, "TABLE-01", 12900).await?;
    assert!(by_sku(&tx, "TABLE-01").await?.is_some());
    drop(tx); // rolled back

    assert_eq!(
        by_sku(&conn, "CHAIR-01").await?,
        Some(Product {
            sku: "CHAIR-01".into(),
            price_cents: 4999
        })
    );
    assert_eq!(by_sku(&conn, "TABLE-01").await?, None);
    Ok(())
}
```

stockroom's functions have the same shape, with `AppError` as the error type and a
`tracing` span that names the function but never the parameter values:

~~~rust
// stockroom/src/repo/stores.rs (excerpt)
#[tracing::instrument(level = "debug", skip_all)]
pub async fn list(db: &impl GenericClient) -> Result<Vec<Store>, AppError> {
    let stmt = db
        .prepare_cached("SELECT id, code, name, address, created_at FROM stores ORDER BY code")
        .await?;
    from_rows(&db.query(&stmt, &[]).await?)
}

#[tracing::instrument(level = "debug", skip_all, fields(code))]
pub async fn id_by_code(db: &impl GenericClient, code: &str) -> Result<Uuid, AppError> {
    let stmt = db
        .prepare_cached("SELECT id FROM stores WHERE code = $1")
        .await?;
    match db.query_opt(&stmt, &[&code]).await? {
        Some(row) => Ok(row.try_get(0)?),
        None => Err(AppError::NotFound("store")),
    }
}
~~~

[stockroom/src/repo/stores.rs](../stockroom/src/repo/stores.rs) ·
[stockroom/src/repo/mod.rs](../stockroom/src/repo/mod.rs)

`from_rows`, in `repo/mod.rs`, is the one generic helper: `rows.iter().map(T::try_from)`,
collected, for any `T` that implements `TryFrom<&Row>`.

Functions that only make sense inside a transaction say so in their signature: `orders::place`
takes `&Transaction<'_>`, because placing an order is several statements that must commit
together, and the compiler then refuses to call it on a bare connection. Chapter 8's
`in_transaction` retry helper is how handlers call those.

> **Pitfall:** a repository function that takes `&impl GenericClient` and runs two statements
> that must succeed together is a bug waiting for a caller who passes a pooled connection. Make
> it take a `&Transaction`, or open one itself.

## 14.5 Domain types at the boundary

A value that has been checked should not need checking again. stockroom parses input into
domain types at the edge, in the HTTP layer, and from then on functions take `Sku`, not `&str`,
and `Cents`, not `i64`. The database enforces the same rules once more, as a constraint, for
everything that does not go through this code:

| Type | Rust | Database | Conversion (Chapter 5) |
|---|---|---|---|
| `Sku` | `Sku::parse` checks the format | the domain `sku`, with the `CHECK` constraint `sku_format` | hand-written `ToSql`/`FromSql` that accept the domain and its base type |
| `Cents` | an `i64` newtype | `bigint` with `CHECK (price_cents > 0)` | `#[postgres(transparent)]` |
| `Quantity` | positive `i32` | the domain `positive_quantity` | derived, for a domain |
| `OrderStatus` | an enum | the enum type `order_status` | derived, with `allow_mismatch` |
| `Address` | a struct | the composite type `address` | derived |

~~~rust
// stockroom/src/domain/sku.rs (excerpt)
/// 3–12 upper-case letters or digits, optionally followed by a dash and 1–6 more: `CHAIR-01`.
/// The same rule as the `sku_format` constraint on the domain.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Sku(String);
~~~

[stockroom/src/domain/sku.rs](../stockroom/src/domain/sku.rs)

`#[serde(try_from = "String")]` makes JSON deserialization go through `Sku::parse`, so a request
body with a malformed SKU is rejected by the extractor with a 422 before any handler runs
(Chapter 15). And the private field means the only ways to get a `Sku` are parsing and reading
one from the database.

Why repeat the rule in the database? Because not every write goes through this code: a psql
session, a script, the next service. The constraint's name, `sku_format`, is also part of the
API: `AppError` maps a violation of it to a 422 with a message for the client (Chapter 15),
which is why constraint names in stockroom are chosen with care and listed in
`error::constraint_message`.

## 14.6 Generating the data layer: Cornucopia

Hand-written repository functions are a few lines each, and stockroom has a few dozen. A larger
code base may prefer to generate them. [Cornucopia](https://cornucopia-rs.github.io/cornucopia/)
(1.0.1, checked 2026-11-06) generates Rust from annotated SQL files:

```sql
-- queries/products.sql

--! product_by_sku
SELECT id, sku, name, price_cents FROM products WHERE sku = :sku;

--! insert_product
INSERT INTO products (sku, name, price_cents) VALUES (:sku, :name, :price_cents);
```

It prepares every query against a real database, which it either connects to (`cornucopia
live <url>`), creates for the occasion on an existing server (`cornucopia fresh`), or starts in a
container from your schema files (`cornucopia schema`). From the parameter and column types the
server reports (the same `Statement::params()` and `columns()` of Chapter 4), it generates a
crate with one typed function per query and one struct per row shape, over tokio-postgres
(asynchronous by default, `--sync` for the `postgres` crate):

~~~rust
// Cornucopia (illustration): the generated crate in use
let product = queries::products::product_by_sku()
    .bind(&client, &"CHAIR-01")
    .one()
    .await?;
~~~

The trade is the one sqlx's macros make, done ahead of time: every query is checked against a
real schema before the program is built, at the cost of a generation step, and of a database at
generation time. Version 1.0 merged the Clorinde fork back into Cornucopia; projects that used
Clorinde should follow Cornucopia's migration guide.

---

## In stockroom

- `migrations/0001_init.sql` and `0002_order_events.sql`: the schema, with named constraints.
- `db/migrate.rs`: §14.1's runner with the details of §14.2. `stockroom migrate` runs it, and
  so does `serve` at startup by default; `tests/migrations.rs` checks that a migrated database
  has nothing left to do, that an edited migration is refused, and that a failed migration
  leaves no trace.
- `repo/`: one module per aggregate, functions over deadpool's `GenericClient`, statements
  through `prepare_cached`, rows through `TryFrom<&Row>`.
- `domain/`: the types of §14.5, each with its tests in `tests/types.rs`.

## Summary

- Embed numbered migrations with `include_str!`, record them with a checksum, and refuse edited
  ones. Never edit an applied migration: add a new one.
- DDL is transactional in PostgreSQL: one transaction per migration, recorded in the same
  transaction, means a failure leaves nothing behind. `CREATE INDEX CONCURRENTLY` is the
  exception and must run alone, outside any transaction, even an implicit one.
- A session-level `pg_advisory_lock` lets several instances migrate at startup safely. Lift
  statement and lock timeouts for the migration session, clear the type cache after it, and log
  its notices.
- refinery (0.10) embeds `V1__name.sql` files and runs them with `run_async`; it does not lock,
  so add the advisory lock yourself.
- Repository functions take `&impl GenericClient` (deadpool's has `prepare_cached`), keep all
  SQL in one module per aggregate, and convert rows with `TryFrom<&Row>`. Functions that need a
  transaction take `&Transaction`.
- Parse input into domain types at the edge, and repeat the rules as named database
  constraints.
- Cornucopia generates typed query functions from annotated SQL, checked against a real
  database at generation time.

## Exercises

1. **A concurrent index.** Extend §14.1's runner so that a migration can be marked
   `transactional: false`, and runs such migrations statement by statement outside a
   transaction. What should the runner record if the second of three statements fails?
   *Hint:* the `schema_migrations` row is the last thing to write, and only on success.
2. **Down migrations.** stockroom has none. Argue for or against adding them, considering a
   migration that drops a column.
   *Hint:* what would the down migration put back?
3. **One more repository function.** Add `by_tag(db, tag, limit)` in §14.4's style that
   returns the SKUs of the products carrying `tag`, and call it both on a pooled connection and
   inside a transaction.
   *Hint:* write the condition as `tags @> ARRAY[$1]`, which a GIN index on `tags` can serve;
   `$1 = ANY(tags)` means the same but cannot use one.
4. **Where does the check fail?** Insert a product with the SKU `chair` (lower case) three ways:
   through `Sku::parse`, with a `&str` and `$1::text`, and from psql. Where is each one stopped,
   and with what error?
   *Hint:* Chapter 5's finding about parameters that target a domain column.

### Solutions

**1.** The runner cannot make such a migration atomic, so it should make it *restartable*:
write each statement so that it can run again (`CREATE INDEX CONCURRENTLY IF NOT EXISTS`, `DROP
INDEX CONCURRENTLY IF EXISTS`), run them in order, and insert the `schema_migrations` row only
after the last one succeeds. If the second statement fails, nothing is recorded; the next run
starts the migration from the top, and the first statement does nothing the second time. A
failed `CREATE INDEX CONCURRENTLY` leaves an *invalid* index behind, which `IF NOT EXISTS` would
then skip, so the migration should drop invalid indexes first (`pg_index.indisvalid` tells).

**2.** Against, for most services. A down migration for "drop column" would have to recreate the
column, but not its data, so it does not undo the change; it only makes the schema look as if it
had. Rolling back a deployment is safer when migrations are additive and the previous version of
the code can run on the new schema (§14.2), so that no schema change needs undoing. Down
migrations are useful in development, to iterate on a migration that has not been shared.

**3.**

```rust
use deadpool_postgres::{GenericClient, Manager, Pool, Runtime};
use tokio_postgres::{Config, Error, NoTls};

async fn by_tag(db: &impl GenericClient, tag: &str, limit: i64) -> Result<Vec<String>, Error> {
    let stmt = db
        .prepare_cached("SELECT sku FROM product WHERE tags @> ARRAY[$1] ORDER BY sku LIMIT $2")
        .await?;
    Ok(db
        .query(&stmt, &[&tag, &limit])
        .await?
        .iter()
        .map(|row| row.get(0))
        .collect())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config: Config = std::env::var("DATABASE_URL")?.parse()?;
    let pool = Pool::builder(Manager::new(config, NoTls))
        .max_size(1)
        .runtime(Runtime::Tokio1)
        .build()?;
    let mut conn = pool.get().await?;
    conn.batch_execute(
        "CREATE TEMP TABLE product (sku text PRIMARY KEY, tags text[] NOT NULL);
         INSERT INTO product VALUES
             ('CHAIR-01', '{seating,oak}'), ('STOOL-01', '{seating}'), ('TABLE-01', '{oak}');",
    )
    .await?;

    assert_eq!(
        by_tag(&conn, "seating", 10).await?,
        ["CHAIR-01", "STOOL-01"]
    );
    let tx = conn.transaction().await?;
    assert_eq!(by_tag(&tx, "oak", 1).await?, ["CHAIR-01"]);
    tx.commit().await?;
    Ok(())
}
```

**4.** `Sku::parse("chair")` fails in Rust, before any SQL: the request never reaches the
database. A `&str` passed for a parameter that targets the `sku` column fails on the *client*,
because tokio-postgres describes that parameter as the domain and `&str` does not accept it
(Chapter 5); with `$1::text`, the cast moves the check to the server, where the domain's
`sku_format` constraint fails with 23514 (`check_violation`). From psql, the same constraint
fails with the same error. The constraint is the only check that every path meets.

---

Previous: [Chapter 13 — Connection pooling](13-pooling.md) ·
Next: [Chapter 15 — axum I: a database-backed JSON API](15-axum-api.md)
