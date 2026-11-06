# Chapter 3 — Rows and the type mapping

> **Async PostgreSQL in Rust** · Previous: [Chapter 2 — Running SQL: execute, query and parameters](02-queries.md) ·
> Next: [Chapter 4 — Prepared statements and the extended protocol](04-prepared-statements.md)

A query hands you `Row`s, and every value in them must become a Rust value. tokio-postgres does
that with one rule: **you say which Rust type you want, and the conversion checks that the
column's PostgreSQL type fits.** There is no guessing and no silent widening, and a mismatch is
an error at run time, so the mapping between the two type systems is something you need to
know.

This chapter covers reading a `Row`, the full table of which Rust types match which PostgreSQL
types (including the ones optional cargo features add), NULL, borrowing values without copying
them, and the types that trip people up: timestamps, JSON, arrays and exact decimals. Chapter 5
does the same for types you define yourself.

---

## 3.1 Reading a `Row`

A `Row` knows its columns, as the server described them, and hands out values by position or
by name:

```rust
#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = playground::connect().await?;
    let row = client
        .query_one(
            "SELECT 'CHAIR-01' AS sku, 4999::int8 AS price_cents, NULL::text AS note",
            &[],
        )
        .await?;

    // What the row knows: the names and types the server described.
    let described: Vec<(&str, &str)> = row
        .columns()
        .iter()
        .map(|c| (c.name(), c.type_().name()))
        .collect();
    assert_eq!(
        described,
        [("sku", "text"), ("price_cents", "int8"), ("note", "text")]
    );
    assert_eq!(row.len(), 3);

    // By position or by name. The Rust type you ask for decides the conversion.
    let sku: &str = row.get(0);
    let price: i64 = row.get("price_cents");
    assert_eq!((sku, price), ("CHAIR-01", 4999));

    // `get` panics on any problem; `try_get` returns it.
    let wrong_type = row.try_get::<_, i32>("price_cents").unwrap_err();
    let no_column = row.try_get::<_, i64>("prise_cents").unwrap_err();
    let was_null = row.try_get::<_, String>("note").unwrap_err();
    assert_eq!(wrong_type.to_string(), "error deserializing column 1");
    assert_eq!(no_column.to_string(), "invalid column `prise_cents`");
    assert_eq!(was_null.to_string(), "error deserializing column 2");

    // NULL becomes None, if you ask for an Option.
    let note: Option<String> = row.get("note");
    assert_eq!(note, None);
    Ok(())
}
```

Three kinds of failure, then, all at run time:

| Failure | `Debug` of the error | Cause |
|---|---|---|
| Wrong type | `Error { kind: FromSql(1), cause: Some(WrongType { postgres: Int8, rust: "i32" }) }` | The Rust type does not accept the column's type |
| No such column | `Error { kind: Column("prise_cents"), cause: None }` | A name that is not in the row, or an index past the end |
| NULL into a non-`Option` | `Error { kind: FromSql(2), cause: Some(WasNull) }` | The value was NULL |

Note how little the `Display` text says ("error deserializing column 1"): the useful part is the
cause, in `source()`. Chapter 6 makes that visible.

**`get` or `try_get`?** `get` is convenient where a failure is a bug you want to hear about
loudly: examples, tests, and columns of a query you wrote right above. In code that serves
requests, prefer `try_get` with `?`: a panic in a handler takes down a request, or worse, and
the error has more to say than the panic message.

**By name or by position?** Positions are fast and break silently when someone adds a column
to the `SELECT`. Names survive that, and cost a scan of the column list (a handful of string
comparisons). In application code, use names. If two columns have the same name, which happens
with `SELECT *` over a join, the first one wins. Give them distinct names with `AS`.

## 3.2 The type table

Every Rust type that can be read from a row implements `FromSql`, and every type that can be
sent as a parameter implements `ToSql` (both traits come from the `postgres-types` crate and
are re-exported as `tokio_postgres::types`). Each implementation says which PostgreSQL types it
accepts. Built in, with no features:

| Rust | PostgreSQL |
|---|---|
| `bool` | `boolean` |
| `i8` | `"char"` (the one-byte internal type, not `char(n)`) |
| `i16` | `smallint` (`int2`) |
| `i32` | `integer` (`int4`) |
| `i64` | `bigint` (`int8`) |
| `u32` | `oid` (and only `oid`) |
| `f32` | `real` (`float4`) |
| `f64` | `double precision` (`float8`) |
| `&str`, `String`, `Box<str>`, `Cow<str>` | `text`, `varchar`, `char(n)` (`bpchar`), `name`, `unknown`, and `citext`, `ltree`, `lquery`, `ltxtquery` by name |
| `&[u8]`, `Vec<u8>` | `bytea` |
| `HashMap<String, Option<String>>` | `hstore` |
| `std::time::SystemTime` | `timestamp`, `timestamptz` |
| `std::net::IpAddr` | `inet` |
| `Option<T>` | whatever `T` takes, or NULL |
| `Vec<T>`, `&[T]`, `Box<[T]>` | arrays of whatever `T` takes (one dimension) |
| `[T; N]` (feature `array-impls`) | arrays of exactly `N` elements |

Everything else comes with an optional feature of `tokio-postgres`, so that you only compile
the integrations you use:

| Feature | Rust | PostgreSQL |
|---|---|---|
| `with-jiff-0_2` | `jiff::Timestamp` / `civil::DateTime` / `civil::Date` / `civil::Time` | `timestamptz` / `timestamp` / `date` / `time` |
| `with-chrono-0_4` | `DateTime<Utc>`, `DateTime<Local>`, `DateTime<FixedOffset>` / `NaiveDateTime` / `NaiveDate` / `NaiveTime` | `timestamptz` / `timestamp` / `date` / `time` |
| `with-time-0_3` | `OffsetDateTime` / `PrimitiveDateTime` / `Date` / `Time` | `timestamptz` / `timestamp` / `date` / `time` |
| `with-uuid-1` | `uuid::Uuid` | `uuid` |
| `with-serde_json-1` | `serde_json::Value`, `Json<T>` | `json`, `jsonb` |
| `with-cidr-0_3` | `IpCidr` / `IpInet` | `cidr` / `inet` |
| `with-eui48-1` | `MacAddress` | `macaddr` |
| `with-geo-types-0_7` | `Point<f64>` / `Rect<f64>` / `LineString<f64>` | `point` / `box` / `path` |
| `with-bit-vec-0_8` | `BitVec` | `bit`, `varbit` |

And one from outside: `rust_decimal::Decimal` implements both traits for `numeric` when you
enable `rust_decimal`'s own feature `db-tokio-postgres` (§3.8).

Three gaps are worth knowing before you hit them:

- **No unsigned integers** except `u32` for `oid`. PostgreSQL has no unsigned types, and
  pretending would hide overflows. A `u64` count goes into a `bigint` as an `i64`, with
  `i64::try_from` at the boundary.
- **No `interval`.** None of the date-time features maps it. Convert in SQL
  (`extract(epoch FROM …)` gives seconds as `numeric`), or write the conversion yourself
  (Chapter 5).
- **No `money`**, and you do not want it: store cents in a `bigint`, or use `numeric`.

> **Pitfall:** SQL's own typing decides the column type, and it is not always the one you
> expect. `6 * 7` is `integer`, but `count(*)` is `bigint`, `sum()` of an `integer` column
> is `bigint`, `sum()` of a `bigint` column is **`numeric`**, and `avg()` of any integer is
> `numeric` too. `let total: i64 = row.get("sum")` over a `bigint` column fails. Cast in the
> query (`sum(total_cents)::bigint`) or read a `Decimal`. When in doubt, ask the row:
> `row.columns()[0].type_()`, or `pgq --describe`.

```rust
#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = playground::connect().await?;
    let row = client
        .query_one(
            "SELECT count(*), sum(i), sum(i::int8), avg(i), sum(i::int8)::int8 AS total
             FROM generate_series(1, 4) AS i",
            &[],
        )
        .await?;
    let types: Vec<&str> = row.columns().iter().map(|c| c.type_().name()).collect();
    assert_eq!(types, ["int8", "int8", "numeric", "numeric", "int8"]);
    assert_eq!(row.get::<_, i64>("total"), 10);
    Ok(())
}
```

## 3.3 NULL

NULL is `Option` on both sides. Reading, ask for `Option<T>` wherever a column can be NULL:
outer joins, nullable columns, aggregates over no rows (`max()` of nothing is NULL). Sending,
pass an `Option`. A bare `None` has no type, so name it: `&None::<i64>`.

```rust
#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE notes (id int, note text);
             INSERT INTO notes VALUES (1, NULL), (2, 'fragile');",
        )
        .await?;

    // A NULL parameter.
    client
        .execute("INSERT INTO notes VALUES ($1, $2)", &[&3, &None::<&str>])
        .await?;

    // `= $1` with NULL matches nothing: in SQL, NULL = NULL is not true but NULL.
    let no_note: Option<&str> = None;
    let equal = client
        .query("SELECT id FROM notes WHERE note = $1", &[&no_note])
        .await?;
    assert!(equal.is_empty());

    // IS NOT DISTINCT FROM treats two NULLs as equal: the comparison you meant.
    let same: Vec<i32> = client
        .query(
            "SELECT id FROM notes WHERE note IS NOT DISTINCT FROM $1 ORDER BY id",
            &[&no_note],
        )
        .await?
        .iter()
        .map(|row| row.get("id"))
        .collect();
    assert_eq!(same, [1, 3]);

    // max() over no rows is NULL, even though the column type is integer.
    let max: Option<i32> = client
        .query_one("SELECT max(id) FROM notes WHERE false", &[])
        .await?
        .get(0);
    assert_eq!(max, None);
    Ok(())
}
```

> **SQL lens:** an optional filter ("this note, or any note if none was given") is often
> written `WHERE $1::text IS NULL OR note = $1`. It works, but a prepared statement with that
> shape cannot use an index on `note` well, because the plan must suit both cases. When it
> matters, build two statements, as `search` did in Chapter 2.

## 3.4 Borrowing from a row

`FromSql` has a lifetime: `FromSql<'a>`, where `'a` is the lifetime of the row's data.
Types like `&str` and `&[u8]` take advantage of it. They point into the row's buffer instead of
copying, which makes reading many rows cheaper. The price is that they cannot outlive the
row:

~~~rust
// Does not compile: the row is a temporary, gone at the end of the statement.
let name: &str = client.query_one("SELECT 'chair'", &[]).await?.get(0);
println!("{name}");
// error[E0716]: temporary value dropped while borrowed
~~~

Keep the row (or the `Vec<Row>`) alive for as long as you use what you borrowed, or ask for an
owned type (`String`, `Vec<u8>`) instead:

```rust
#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = playground::connect().await?;
    let rows = client
        .query(
            "SELECT 'Product ' || i AS name FROM generate_series(1, 1000) AS i",
            &[],
        )
        .await?;

    // Borrowed: no allocation per row. `rows` outlives every `&str`.
    let total_length: usize = rows
        .iter()
        .map(|row| row.get::<_, &str>("name").len())
        .sum();
    assert_eq!(
        total_length,
        1000 * "Product ".len() + 9 + 2 * 90 + 3 * 900 + 4
    );

    // Owned: each value has its own allocation, and can go anywhere.
    let names: Vec<String> = rows.iter().map(|row| row.get("name")).collect();
    drop(rows);
    assert_eq!(names[999], "Product 1000");
    Ok(())
}
```

> **Rust refresher:** `FromSql<'a>` with `fn from_sql(ty: &Type, raw: &'a [u8]) -> Result<Self,
> …>` lets `Self` borrow from `raw`, the column's bytes inside the row. A type that never
> borrows implements it for every `'a`, which is what `FromSqlOwned` means: it is a shorthand
> for `for<'a> FromSql<'a>`. APIs that drop the row before handing you the value, such as the
> `query_scalar` family in Chapter 2, require `FromSqlOwned`. That is why
> `query_one_scalar::<&str, _>` does not compile.

## 3.5 Rows into structs

Applications want structs, not rows. tokio-postgres has no derive for this, and the usual
pattern is a `TryFrom<&Row>` implementation per struct, reading columns by name:

```rust
use tokio_postgres::Row;

#[derive(Debug, PartialEq)]
struct Product {
    sku: String,
    name: String,
    price_cents: i64,
    discontinued: Option<jiff::civil::Date>,
}

impl TryFrom<&Row> for Product {
    type Error = tokio_postgres::Error;

    fn try_from(row: &Row) -> Result<Self, Self::Error> {
        Ok(Product {
            sku: row.try_get("sku")?,
            name: row.try_get("name")?,
            price_cents: row.try_get("price_cents")?,
            discontinued: row.try_get("discontinued")?,
        })
    }
}

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE products (
                 sku text PRIMARY KEY, name text NOT NULL, price_cents bigint NOT NULL,
                 discontinued date);
             INSERT INTO products VALUES ('CHAIR-01', 'Garden chair', 4999, NULL),
                                         ('STOOL-01', 'Stool', 1999, '2026-06-30');",
        )
        .await?;

    let rows = client
        .query(
            "SELECT sku, name, price_cents, discontinued FROM products ORDER BY sku",
            &[],
        )
        .await?;
    let products = rows
        .iter()
        .map(Product::try_from)
        .collect::<Result<Vec<_>, _>>()?;

    assert_eq!(products[0].discontinued, None);
    assert_eq!(
        products[1],
        Product {
            sku: "STOOL-01".into(),
            name: "Stool".into(),
            price_cents: 1999,
            discontinued: Some(jiff::civil::date(2026, 6, 30)),
        }
    );
    Ok(())
}
```

It is a few lines more per struct than a derive, and in exchange, the mapping is ordinary code
you can read, step through and change: a column renamed in the query, a computed field, a
nested struct read from the same row. Keep the `SELECT` list and the `TryFrom` together, in the
same module. Chapter 14 shows how stockroom organizes them, and mentions clorinde, which
generates both from `.sql` files.

## 3.6 Dates and times

PostgreSQL has two timestamp types, and they map to two different Rust types:

| PostgreSQL | What it stores | jiff | chrono | time |
|---|---|---|---|---|
| `timestamptz` | an instant: microseconds since 2000-01-01 UTC | `Timestamp` | `DateTime<Utc>` | `OffsetDateTime` |
| `timestamp` | a wall-clock reading, no time zone | `civil::DateTime` | `NaiveDateTime` | `PrimitiveDateTime` |
| `date` | a calendar day | `civil::Date` | `NaiveDate` | `Date` |
| `time` | a time of day | `civil::Time` | `NaiveTime` | `Time` |

This book uses jiff, whose `Timestamp` is exactly what `timestamptz` is: an instant, with no time
zone attached. Despite its name, `timestamptz` does *not* store a time zone. It stores UTC and
converts at the edges. And the edges tokio-postgres uses are binary, so the session's
`TimeZone` setting does not affect what you read:

```rust
use jiff::Timestamp;

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = playground::connect().await?;
    client
        .batch_execute("SET TimeZone = 'America/New_York'")
        .await?;

    let row = client
        .query_one(
            "SELECT '2026-11-06 12:00:00+00'::timestamptz AS at,
                    '2026-11-06 12:00:00+00'::timestamptz::text AS as_text",
            &[],
        )
        .await?;
    let at: Timestamp = row.get("at");
    // Binary: the instant itself, whatever the session's time zone.
    assert_eq!(at.to_string(), "2026-11-06T12:00:00Z");
    // Text: rendered in the session's time zone.
    assert_eq!(row.get::<_, &str>("as_text"), "2026-11-06 07:00:00-05");

    // timestamp (without time zone) needs the civil type; Timestamp refuses it.
    let row = client
        .query_one("SELECT '2026-11-06 12:00'::timestamp", &[])
        .await?;
    assert!(row.try_get::<_, Timestamp>(0).is_err());
    let wall: jiff::civil::DateTime = row.get(0);
    assert_eq!(wall, jiff::civil::date(2026, 11, 6).at(12, 0, 0, 0));
    Ok(())
}
```

The session time zone still matters for anything the *server* computes from a `timestamptz`:
`date_trunc('day', created_at)` and `created_at::date` cut the day at midnight in the session's
time zone. stockroom's sales report says which one it means with `AT TIME ZONE 'UTC'`.

> **Pitfall:** PostgreSQL keeps microseconds; jiff, chrono and `time` keep nanoseconds. A
> value with sub-microsecond digits loses them on the way in, so it does not compare equal on
> the way back. `Timestamp::now()` has nanoseconds on Linux (on macOS the clock happens to
> stop at microseconds, which hides the problem until the tests run in CI). Truncate before
> you store, or compare at microsecond precision.

```rust
use jiff::Timestamp;

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = playground::connect().await?;
    let precise = Timestamp::from_nanosecond(1_700_000_000_123_456_789).expect("in range");
    let stored: Timestamp = client
        .query_one("SELECT $1::timestamptz", &[&precise])
        .await?
        .get(0);
    assert_ne!(stored, precise);
    assert_eq!(stored.to_string(), "2023-11-14T22:13:20.123456Z");
    let truncated = Timestamp::from_microsecond(precise.as_microsecond()).expect("in range");
    assert_eq!(stored, truncated);
    Ok(())
}
```

## 3.7 JSON

With `with-serde_json-1`, `serde_json::Value` reads and writes `json` and `jsonb`. For a column
with a known shape, `Json<T>` does the same through serde with your own type: it wraps any `T:
Serialize` going in and any `T: Deserialize` coming out.

```rust
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio_postgres::types::Json;

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct Customer {
    name: String,
    email: String,
}

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = playground::connect().await?;
    client
        .batch_execute("CREATE TEMP TABLE orders (id int, customer jsonb, extra jsonb)")
        .await?;

    let ada = Customer {
        name: "Ada".into(),
        email: "ada@example.com".into(),
    };
    // A typed struct and a free-form Value, both into jsonb.
    client
        .execute(
            "INSERT INTO orders VALUES (1, $1, $2)",
            &[
                &Json(&ada),
                &json!({ "giftWrap": true, "notes": ["fragile"] }),
            ],
        )
        .await?;

    let row = client
        .query_one("SELECT customer, extra FROM orders WHERE id = 1", &[])
        .await?;
    let Json(customer): Json<Customer> = row.get("customer");
    let extra: Value = row.get("extra");
    assert_eq!(customer, ada);
    assert_eq!(extra["notes"][0], "fragile");

    // Inside the document, in SQL: ->> yields text, -> yields jsonb.
    let row = client
        .query_one(
            "SELECT customer ->> 'name' AS name, extra -> 'giftWrap' AS wrap
             FROM orders WHERE customer ->> 'email' = $1",
            &[&"ada@example.com"],
        )
        .await?;
    assert_eq!(row.get::<_, &str>("name"), "Ada");
    assert_eq!(row.get::<_, Value>("wrap"), json!(true));
    Ok(())
}
```

`json` stores the text as given; `jsonb` stores a parsed form, which is what you want for
anything you query, compare or index. The Rust side works the same with both.

> **Pitfall:** `Json<T>` reads whatever is in the column and deserializes it with serde. If the
> document does not match `T`, for example after someone else's code wrote a slightly
> different shape, reading fails with a conversion error at run time. For columns written by
> several programs, read a `Value` and validate it, or give `T` serde defaults.

## 3.8 Arrays

A one-dimensional PostgreSQL array maps to `Vec<T>` (or a slice going in), for any element
type `T` that maps:

```rust
#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = playground::connect().await?;
    client
        .batch_execute("CREATE TEMP TABLE products (sku text PRIMARY KEY, tags text[])")
        .await?;
    for (sku, tags) in [
        ("CHAIR-01", vec!["garden", "sale"]),
        ("LAMP-01", vec!["lighting"]),
        ("RUG-01", vec![]),
    ] {
        client
            .execute("INSERT INTO products VALUES ($1, $2)", &[&sku, &tags])
            .await?;
    }

    let tags: Vec<String> = client
        .query_one("SELECT tags FROM products WHERE sku = 'CHAIR-01'", &[])
        .await?
        .get(0);
    assert_eq!(tags, ["garden", "sale"]);

    // "Is it one of these?" One array parameter and = ANY, not IN.
    let wanted = ["CHAIR-01", "RUG-01", "VASE-01"];
    let found: Vec<String> = client
        .query_scalar(
            "SELECT sku FROM products WHERE sku = ANY($1) ORDER BY sku",
            &[&&wanted[..]],
        )
        .await?;
    assert_eq!(found, ["CHAIR-01", "RUG-01"]);

    // "Does the array contain this?" @> (contains), indexable with GIN.
    let on_sale: Vec<String> = client
        .query_scalar(
            "SELECT sku FROM products WHERE tags @> ARRAY[$1]",
            &[&"sale"],
        )
        .await?;
    assert_eq!(on_sale, ["CHAIR-01"]);

    // Elements can be NULL: then the element type must be an Option.
    let mixed: Vec<Option<i32>> = client
        .query_one("SELECT ARRAY[1, NULL, 3]", &[])
        .await?
        .get(0);
    assert_eq!(mixed, [Some(1), None, Some(3)]);
    Ok(())
}
```

`= ANY($1)` with one array parameter replaces `IN ($1, $2, …)`. It is one statement whatever
the list's length, so it can be prepared once and reused, and the list cannot grow into
something you have to build with `format!`.

Two limits: an array with NULL elements needs `Vec<Option<T>>` (with `Vec<T>` it fails with
`WasNull`), and arrays of more than one dimension are refused ("array contains too many
dimensions"). Model a grid as a table, or as `jsonb`.

> **Pitfall:** `WHERE sku IN ($1)` does not mean "any of the values in this array". `IN` with
> one item is `sku = $1`, so the server types `$1` as `text`, and the client then refuses to
> send a `Vec` as a `text`: "cannot convert between the Rust type `Vec<&str>` and the Postgres
> type `text`". The `ANY` form is the one you want.

## 3.9 Exact numbers, UUIDs

**`numeric`** is PostgreSQL's arbitrary-precision decimal. `f64` cannot hold it exactly, and
tokio-postgres does not pretend: there is no built-in mapping. `rust_decimal::Decimal` provides
one (with its `db-tokio-postgres` feature, in the playground's `Cargo.toml`):

```rust
use rust_decimal::Decimal;

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = playground::connect().await?;
    let row = client
        .query_one(
            "SELECT 0.1::numeric + 0.2 AS exact, 0.1::float8 + 0.2 AS binary,
                    round(avg(x), 2) AS average
             FROM (VALUES (1999), (2999), (4999)) AS prices(x)",
            &[],
        )
        .await?;
    let exact: Decimal = row.get("exact");
    let binary: f64 = row.get("binary");
    let average: Decimal = row.get("average");
    assert_eq!(exact.to_string(), "0.3");
    assert_eq!(binary.to_string(), "0.30000000000000004");
    assert_eq!(average.to_string(), "3332.33");
    Ok(())
}
```

For money that a program adds up, the simplest exact representation is an integer number of
cents in a `bigint`. That is what stockroom does, with `numeric` only where SQL produces one
(averages).

**`uuid`** maps to `uuid::Uuid` with `with-uuid-1`. PostgreSQL 18 can generate time-ordered
version-7 UUIDs itself, with `uuidv7()`, which makes a good default for primary keys: unique
without coordination, and roughly in insertion order, which keeps B-tree indexes compact.

```rust
use uuid::Uuid;

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = playground::connect().await?;
    let row = client
        .query_one("SELECT uuidv7() AS first, uuidv7() AS second", &[])
        .await?;
    let (first, second): (Uuid, Uuid) = (row.get("first"), row.get("second"));
    assert_eq!(first.get_version_num(), 7);
    assert!(first < second, "time-ordered");

    // And from Rust, generated client-side, the same type goes back in.
    let mine = Uuid::now_v7();
    let echoed: Uuid = client.query_one("SELECT $1::uuid", &[&mine]).await?.get(0);
    assert_eq!(echoed, mine);
    Ok(())
}
```

### Side by side — *a row into a struct*

The task: read a product into a struct, with a nullable column.

```sql
-- psql: rows are printed; \gx shows one column per line, which is how a struct reads.
tpbook=# SELECT sku, name, price_cents, discontinued FROM products WHERE sku = 'STOOL-01' \gx
-[ RECORD 1 ]+-----------
sku          | STOOL-01
name         | Stool
price_cents  | 1999
discontinued | 2026-06-30
```

~~~rust
// sqlx (illustration)
#[derive(sqlx::FromRow)]
struct Product { sku: String, name: String, price_cents: i64, discontinued: Option<chrono::NaiveDate> }

let product: Product = sqlx::query_as("SELECT sku, name, price_cents, discontinued FROM products WHERE sku = $1")
    .bind("STOOL-01")
    .fetch_one(&pool)
    .await?;
~~~

```rust
use tokio_postgres::Row;

#[derive(Debug, PartialEq)]
struct Product {
    sku: String,
    name: String,
    price_cents: i64,
    discontinued: Option<jiff::civil::Date>,
}

impl TryFrom<&Row> for Product {
    type Error = tokio_postgres::Error;
    fn try_from(row: &Row) -> Result<Self, Self::Error> {
        Ok(Product {
            sku: row.try_get("sku")?,
            name: row.try_get("name")?,
            price_cents: row.try_get("price_cents")?,
            discontinued: row.try_get("discontinued")?,
        })
    }
}

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = playground::connect().await?;
    let row = client
        .query_one(
            "SELECT 'STOOL-01' AS sku, 'Stool' AS name, 1999::int8 AS price_cents,
                    '2026-06-30'::date AS discontinued",
            &[],
        )
        .await?;
    let product = Product::try_from(&row)?;
    assert_eq!(product.discontinued, Some(jiff::civil::date(2026, 6, 30)));
    Ok(())
}
```

| | psql | sqlx | tokio-postgres |
|---|---|---|---|
| **Same** | Columns have names and types; NULL is a value of its own | NULL is `Option`; types must match | NULL is `Option`; types must match |
| **Different** | Everything is displayed as text | A derive maps columns to fields by name; `query_as!` checks the mapping at compile time | No derive: a `TryFrom<&Row>` you write, checked at run time. Values can borrow from the row |

---

## In stockroom

Every type stockroom reads from the database has a `TryFrom<&Row>`, next to the queries that
produce its columns. The catalog's `Product` mixes a domain type (`Sku`, Chapter 5), money in
cents, an array, free-form JSON and two timestamps:

~~~rust
// stockroom/src/repo/products.rs (excerpt)
impl TryFrom<&Row> for Product {
    type Error = tokio_postgres::Error;

    fn try_from(row: &Row) -> Result<Self, Self::Error> {
        Ok(Product {
            id: row.try_get("id")?,
            sku: row.try_get("sku")?,
            name: row.try_get("name")?,
            price_cents: row.try_get("price_cents")?,
            tags: row.try_get("tags")?,
            attributes: row.try_get("attributes")?,
            version: row.try_get("version")?,
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
        })
    }
}
~~~

`id` is a `Uuid` (generated by `uuidv7()`), `tags` a `Vec<String>`, `attributes` a
`serde_json::Value`, and the timestamps are jiff `Timestamp`s. One generic helper turns a page
of rows into structs for every such type, converting the error on the way:

~~~rust
// stockroom/src/repo/mod.rs (excerpt)
pub fn from_rows<T>(rows: &[Row]) -> Result<Vec<T>, AppError>
where
    for<'r> T: TryFrom<&'r Row, Error = tokio_postgres::Error>,
{
    rows.iter()
        .map(|row| T::try_from(row).map_err(AppError::from))
        .collect()
}
~~~

The `for<'r>` says "for a row borrowed for any lifetime", which is what lets `T` be built from
each row in turn. An order's customer is structured JSON, read through serde:

~~~rust
// stockroom/src/repo/orders.rs (excerpt)
let Json(customer): Json<Customer> = head.try_get("customer")?;
~~~

## Summary

- `row.get` panics and `row.try_get` returns an error, by index or by name. Prefer names, and
  `try_get` in code that serves requests.
- You choose the Rust type; the conversion checks it against the column's type. Mismatches,
  missing columns and unexpected NULLs are run-time errors whose cause is in `source()`.
- Know the table: `int4` is `i32`, `int8` is `i64`; no unsigned types except `oid`;
  `sum(bigint)` and `avg()` are `numeric`.
- NULL is `Option` both ways; name the type of a bare `None`. `= NULL` matches nothing; use
  `IS NOT DISTINCT FROM`.
- `&str` and `&[u8]` borrow from the row; owned types do not. `FromSqlOwned` means "never
  borrows".
- Map rows to structs with `TryFrom<&Row>`, reading columns by name.
- `timestamptz` is an instant (jiff `Timestamp`), `timestamp` a wall-clock reading (`civil::
  DateTime`). Binary transfer ignores the session time zone, and microseconds are the limit.
- JSON: `serde_json::Value`, or `Json<T>` with serde. Arrays: one dimension, `Vec<Option<T>>`
  for NULL elements, `= ANY($1)` instead of `IN`.
- `numeric` needs `rust_decimal`; `uuid` needs `with-uuid-1`; `interval` has no mapping.

## Exercises

1. **A report row.** Write a struct `StoreSales { store: String, orders: i64, revenue_cents:
   i64 }` and fill it from `SELECT store, count(*) AS orders, sum(total_cents) AS revenue_cents
   FROM orders GROUP BY store` over a temporary `orders (store text, total_cents bigint)`
   table. Run it, read the error, and fix it two different ways.
   *Hint:* §3.2's pitfall.
2. **Zero copies.** Over `SELECT md5(i::text) AS hash FROM generate_series(1, 10000) AS i`,
   count how many hashes start with `"00"` without allocating a `String` per row.
   *Hint:* `&str`, and keep the rows alive.
3. **A left join.** Products and their reviews: `products (sku)` and `reviews (sku, stars
   int)`. Return every product with its average number of stars, or `None` when it has no
   reviews. Which Rust type do you read the average as?
   *Hint:* `avg` over an `int` column returns `numeric`, and a product without reviews gets a
   `NULL` from the left join.
4. **Find by tags.** Write a query that returns the products having *all* of a list of tags,
   and one that returns the products having *any* of them. Both take one array parameter.
   *Hint:* `@>` and `&&`.

### Solutions

**1.** The error is `WrongType { postgres: Numeric, rust: "i64" }`: `sum()` of a `bigint` column
is `numeric`, because a sum of many `bigint`s can overflow `bigint`. Either cast in the query,
`sum(total_cents)::bigint AS revenue_cents` (which turns an overflow into an error on the
server), or read a `rust_decimal::Decimal` and convert it in Rust.

```rust
#[derive(Debug, PartialEq)]
struct StoreSales {
    store: String,
    orders: i64,
    revenue_cents: i64,
}

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE orders (store text, total_cents bigint);
             INSERT INTO orders VALUES ('mitte', 4999), ('mitte', 2999), ('spandau', 999);",
        )
        .await?;
    let rows = client
        .query(
            "SELECT store, count(*) AS orders, sum(total_cents)::bigint AS revenue_cents
             FROM orders GROUP BY store ORDER BY store",
            &[],
        )
        .await?;
    let sales: Vec<StoreSales> = rows
        .iter()
        .map(|row| {
            Ok(StoreSales {
                store: row.try_get("store")?,
                orders: row.try_get("orders")?,
                revenue_cents: row.try_get("revenue_cents")?,
            })
        })
        .collect::<Result<_, tokio_postgres::Error>>()?;
    assert_eq!(
        sales[0],
        StoreSales {
            store: "mitte".into(),
            orders: 2,
            revenue_cents: 7998
        }
    );
    Ok(())
}
```

**3.** `avg(stars)` over an `integer` column is `numeric`, and the left join makes it NULL for a
product without reviews: read an `Option<Decimal>`. If you would rather have a float, cast it,
`avg(stars)::float8`, and read an `Option<f64>`.

**4.** All of them: `WHERE tags @> $1` ("contains"). Any of them: `WHERE tags && $1`
("overlaps"). Both take a `text[]` parameter, a `&[&str]` or `Vec<String>` on the Rust side,
and both can use a GIN index on `tags`.

---

Previous: [Chapter 2 — Running SQL: execute, query and parameters](02-queries.md) ·
Next: [Chapter 4 — Prepared statements and the extended protocol](04-prepared-statements.md)
