# Chapter 5 — Checking a statement once, not every row

> **Building an ORM on tokio-postgres** · Previous: [Chapter 4 — Attributes, errors and generics](04-attributes.md) · Next: [Chapter 6 — Tables: the model derive](06-tables.md)

Every expansion since Chapter 0 has had the same two methods. `resolve` takes a list of columns
and returns a tuple of positions; `from_row_at` takes a row and that tuple, and builds the
struct. Chapter 2's tuple impls have them too, with `type Columns = ()`. This chapter explains
that shape. Chapter 1 measured what it replaces: a lookup by name for every field of every row,
about three times the cost of reading by position, repeated although its answer cannot change,
because every row of a statement has the same columns in the same order.

So shelf splits the work in two. What depends only on the statement is done once, when the
statement is prepared: find each field's column, and check that its type can be read into the
field. What depends on the row is done per row: decode the values. The first half has a second
payoff, which matters more than the speed. A struct that does not fit its query is reported
before the query runs, with the struct's name, the field's, the column's and both types, even
when the query would return no rows, and even when it is an `INSERT` that would have written
one.

**In this chapter you will learn:**

- why every row of a statement shares one column list, and what that list holds before any row
  exists
- how `resolve` finds each field's column by its exact name and checks its type, once per
  statement
- which mistakes are found before the query runs, and which can only be found in a row
- what the derive generates for this, and how `from_row` keeps a single row simple
- what it is worth, measured on 100,000 rows, and what `resolve` itself costs per call
- how sqlx finds its columns, for comparison

---

## 5.1 The answer belongs to the statement

Chapter 1's numbers, briefly: on 100,000 rows of twelve columns, a struct read by name took
about three times as long as the same struct read by position in a release build, and 1.6
times as long in a debug build ([§1.2](01-mapping-by-hand.md#12-by-name-or-by-position)). The
decoding was the same in both. The difference was the searching: twelve walks along the column
list per row, 78 name comparisons, each walk finding the position it found on the row before.

It has to find the same position, because a row has no column names of its own. In
tokio-postgres 0.7.18 a `Row` holds the `Statement` it came from, and `Row::columns()` returns
that statement's list. This program checks it with pointer equality: not two equal lists, but
the same list.

```rust
#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = shelf_playground::connect().await?;

    let statement = client
        .prepare(
            "SELECT 'sku-' || g AS sku, g * 100 AS price_cents FROM generate_series(1, 3) AS g",
        )
        .await?;
    let rows = client.query(&statement, &[]).await?;
    // Not three equal lists: the statement's one list, three times.
    for row in &rows {
        assert!(std::ptr::eq(row.columns(), statement.columns()));
    }

    let names: Vec<&str> = statement.columns().iter().map(|c| c.name()).collect();
    println!("{} rows, one column list: {names:?}", rows.len());

    // SQL text is prepared too (main book §4.1), and its rows share that statement's list.
    let series = client
        .query("SELECT g FROM generate_series(1, 5) AS g", &[])
        .await?;
    assert!(
        series
            .iter()
            .all(|row| std::ptr::eq(row.columns(), series[0].columns()))
    );
    Ok(())
}
```

```text
3 rows, one column list: ["sku", "price_cents"]
```

So the work of reading a row splits cleanly:

| Work | Depends on | How often shelf does it |
|---|---|---|
| Find each field's column | the statement | once per statement |
| Check that the column's type can be read into the field | the statement | once per statement |
| Decode each value, and notice a NULL | the row | once per row |

Reading by position moved the first line of that table out of the loop in Chapter 1, at the
price of positions written into the code, which break silently when someone changes the
`SELECT`. Finding them once per statement keeps the names and drops the cost.

## 5.2 A statement knows its columns

`Client::prepare` sends the SQL to the server, which parses it and describes it, and returns a
`Statement` ([main book §4.2](../../book/04-prepared-statements.md#42-prepare-one-parse-many-executions)).
Its `columns()` are known at that point, before anything has run, so a query that will return
no rows still has them:

```rust
use tokio_postgres::types::Type;

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = shelf_playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE products (
                 id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
                 sku text NOT NULL,
                 price_cents bigint NOT NULL
             );",
        )
        .await?;

    let statement = client
        .prepare(
            "SELECT id, sku, price_cents, price_cents * 2 AS doubled FROM products WHERE false",
        )
        .await?;
    let rows = client.query(&statement, &[]).await?;

    println!(
        "{} rows, {} columns:",
        rows.len(),
        statement.columns().len()
    );
    for column in statement.columns() {
        let name = column.name();
        let sql_type = column.type_().to_string();
        let source = match column.column_id() {
            Some(n) => format!("column {n} of a table"),
            None => "an expression".to_string(),
        };
        println!("  {name:<12} {sql_type:<5} {source}");
    }
    assert!(rows.is_empty());
    assert_eq!(statement.columns()[3].name(), "doubled");
    assert_eq!(statement.columns()[3].type_(), &Type::INT8);
    Ok(())
}
```

```text
0 rows, 4 columns:
  id           int8  column 1 of a table
  sku          text  column 2 of a table
  price_cents  int8  column 3 of a table
  doubled      int8  an expression
```

> **Protocol lens:** the columns come from the *RowDescription* message, the server's answer to
> the *Describe* that `prepare` sends (main book
> [§4.1](../../book/04-prepared-statements.md#41-what-query-sends)). For each column it
> carries a name; the OID of the table and the column's number in it, when the column comes
> straight from a table, and zero otherwise; the type's OID, size and modifier; and a format
> code. tokio-postgres keeps the name, the table OID and column number (`None` for zero), the
> type modifier, and the type, turning its OID into a `Type`. For a type it does not know, such
> as an enum or a domain, it asks the catalog, once per connection. What a row description does
> not carry is whether a column can be NULL: Chapter 13 goes to the catalog for that.

Finding a field's column in that list, and checking its type, is everything `resolve` does.
Chapter 1's `FromRow` had one method, `from_row`. To do the statement's work once, the trait
needs somewhere to keep what that work found, and a way to read a row with it: an associated
type, `Columns`, and two methods. This stage program writes the two-step trait by hand, a
`Product` that implements it, and a `fetch_all` that prepares the statement, resolves once, and
then reads every row:

```rust
use tokio_postgres::types::{FromSqlOwned, ToSql};
use tokio_postgres::{Client, Column, Row};

type Error = Box<dyn std::error::Error + Send + Sync>;

/// Chapter 1's trait, in two steps.
trait FromRow: Sized {
    /// Where this type's values are in a result.
    type Columns;
    /// Once per statement: find each field's column, and check its type.
    fn resolve(columns: &[Column]) -> Result<Self::Columns, Error>;
    /// Once per row: read the values at the positions `resolve` found.
    fn from_row_at(row: &Row, columns: &Self::Columns) -> Result<Self, Error>;
}

/// The position of the column `name`, checked to be readable as `T`.
fn column<T: FromSqlOwned>(columns: &[Column], name: &str) -> Result<usize, Error> {
    let index = columns
        .iter()
        .position(|c| c.name() == name) // the exact name, nothing else
        .ok_or_else(|| format!("the result has no column {name:?}"))?;
    let sql_type = columns[index].type_();
    if !T::accepts(sql_type) {
        let rust_type = std::any::type_name::<T>();
        return Err(format!(
            "column {name:?} has type {sql_type}, which cannot be read into {rust_type}"
        )
        .into());
    }
    Ok(index)
}

#[derive(Debug)]
struct Product {
    sku: String,
    price_cents: i64,
}

impl FromRow for Product {
    type Columns = (usize, usize);

    fn resolve(columns: &[Column]) -> Result<Self::Columns, Error> {
        Ok((
            column::<String>(columns, "sku")?,
            column::<i64>(columns, "price_cents")?,
        ))
    }

    fn from_row_at(row: &Row, columns: &Self::Columns) -> Result<Self, Error> {
        Ok(Product {
            sku: row.try_get(columns.0)?,
            price_cents: row.try_get(columns.1)?,
        })
    }
}

/// Every row of `sql`, read into `T`: prepare, resolve once, then read each row.
async fn fetch_all<T: FromRow>(
    client: &Client,
    sql: &str,
    params: &[&(dyn ToSql + Sync)],
) -> Result<Vec<T>, Error> {
    let statement = client.prepare(sql).await?;
    let columns = T::resolve(statement.columns())?; // before the query runs
    let rows = client.query(&statement, params).await?;
    rows.iter()
        .map(|row| T::from_row_at(row, &columns))
        .collect()
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let client = shelf_playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE products (sku text PRIMARY KEY, price_cents bigint NOT NULL);
             INSERT INTO products VALUES ('MUG-01', 1299), ('TEE-02', 2499);",
        )
        .await?;

    // The columns in another order than the fields: the positions take care of it.
    let products: Vec<Product> = fetch_all(
        &client,
        "SELECT price_cents, sku FROM products ORDER BY sku",
        &[],
    )
    .await?;
    assert_eq!(products[0].sku, "MUG-01");
    assert_eq!(products[1].price_cents, 2499);

    // Two queries that do not fit: both are refused before they run, with no row to look at.
    let mistakes = [
        (
            "SELECT sku FROM products WHERE false",
            r#"the result has no column "price_cents""#,
        ),
        (
            "SELECT sku, price_cents::int AS price_cents FROM products WHERE false",
            r#"column "price_cents" has type int4, which cannot be read into i64"#,
        ),
    ];
    for (sql, expected) in mistakes {
        let error = fetch_all::<Product>(&client, sql, &[]).await.unwrap_err();
        println!("{error}");
        assert_eq!(error.to_string(), expected);
    }
    Ok(())
}
```

```text
the result has no column "price_cents"
column "price_cents" has type int4, which cannot be read into i64
```

`column` compares names with `==`, so it finds a column by its exact name only. That is a
deliberate difference from `Row::try_get(&str)`, whose second walk ignores ASCII case
([§1.2](01-mapping-by-hand.md#12-by-name-or-by-position)): by the time PostgreSQL describes a
statement it has already folded unquoted names to lower case, so the name in the description
is the name, and a column that matches only when case is ignored is a different column.

> **Rust refresher:** an *associated type* is a type each impl of a trait chooses, as each
> `Iterator` chooses its `Item`. Here `Product` chooses `(usize, usize)`, and a tuple impl
> can choose `()`. Generic code names it through its type parameter, `T::Columns`, without
> knowing what it is: `fetch_all` keeps the `T::Columns` that `resolve` returns and hands it
> back to `from_row_at`, and never looks inside.

## 5.3 Types checked once

The second half of `column` is one call: `T::accepts(column.type_())`. Every `FromSql` type has
`accepts`, which says whether it can decode a given PostgreSQL type. In postgres-types 0.2.14,
`i64` accepts `int8` and nothing else; `String` accepts `text`, `varchar`, `bpchar`, `name` and
`unknown`, and the extension types `citext`, `ltree`, `lquery` and `ltxtquery`; `Option<T>`
accepts what `T` accepts. The answer depends only on the column's type, so like the position
it belongs to the statement. tokio-postgres's `try_get` asks it on every row anyway (§1.2);
shelf asks once, in `resolve`, where a mismatch can still be reported with the names of the
struct and the field. This is shelf's version of the stage program's `column`:

~~~rust
// orm/shelf/src/row.rs (excerpt)
/// The position of the column `name` in `columns`, checked to be readable as `T`.
///
/// Generated code calls this from `resolve`; it is public for that reason only.
#[doc(hidden)]
pub fn column<T: FromColumn>(
    columns: &[Column],
    model: &'static str,
    field: &'static str,
    name: &str,
) -> Result<usize, Error> {
    let mapping = |problem| Error::Mapping {
        model,
        field,
        problem,
    };
    // Exact names only. `Row::try_get(&str)` would also try again ignoring ASCII case, but
    // PostgreSQL itself folds unquoted names to lower case, so the exact name is the rule.
    let Some(index) = columns.iter().position(|c| c.name() == name) else {
        return Err(mapping(Mapping::MissingColumn {
            column: name.to_owned(),
        }));
    };
    check::<T>(&columns[index], index).map_err(mapping)
}

// …

fn check<T: FromColumn>(column: &Column, index: usize) -> Result<usize, Mapping> {
    if T::accepts(column.type_()) {
        Ok(index)
    } else {
        Err(Mapping::WrongType {
            column: column.name().to_owned(),
            sql_type: column.type_().clone(),
            rust_type: short_type_name(std::any::type_name::<T>()),
        })
    }
}
~~~

[orm/shelf/src/row.rs](../shelf/src/row.rs)

Two things differ from the stage. The error is a value a program can match on,
`Error::Mapping { model, field, problem }`, with `Mapping::MissingColumn` or
`Mapping::WrongType` as the problem. And the Rust type is shortened by `short_type_name`, so
that a field of type `Option<String>` is reported as `Option<String>`, not as `type_name`'s
`core::option::Option<alloc::string::String>`. (`check` is shared with `column_at`, which the
tuple impls call with a fixed position.)

Because the check needs only the statement, it happens before the query runs, and a query that
does not fit its struct never runs at all. This program shows it twice, through the real
derive: a `SELECT` that would return no rows, and an `INSERT … RETURNING` whose result lacks a
column the struct reads:

```rust
use shelf::{FromRow, Mapping};

#[derive(Debug, FromRow)]
#[allow(dead_code)] // the program only shows the errors
struct Product {
    sku: String,
    price_cents: i64,
    note: Option<String>,
}

#[tokio::main]
async fn main() -> Result<(), shelf::Error> {
    let client = shelf_playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE products (
                 sku text PRIMARY KEY,
                 price_cents bigint NOT NULL,
                 note integer -- a mistake: notes are text
             );
             INSERT INTO products VALUES ('MUG-01', 1299, NULL);",
        )
        .await?;

    // No row, and still an error: the type is the statement's, not a row's.
    let error = shelf::fetch_all::<Product>(
        &client,
        "SELECT sku, price_cents, note FROM products WHERE false",
        &[],
    )
    .await
    .unwrap_err();
    println!("{error}");
    assert!(matches!(
        error,
        shelf::Error::Mapping {
            field: "note",
            problem: Mapping::WrongType { .. },
            ..
        }
    ));

    // An INSERT whose RETURNING has no `note`: prepared, checked, and never run.
    let error = shelf::fetch_one::<Product>(
        &client,
        "INSERT INTO products (sku, price_cents) VALUES ('TEE-02', 2499) RETURNING sku, price_cents",
        &[],
    )
    .await
    .unwrap_err();
    println!("{error}");
    let count: i64 = client
        .query_one("SELECT count(*) FROM products", &[])
        .await?
        .get(0);
    assert_eq!(count, 1, "the INSERT never ran");
    Ok(())
}
```

```text
Product.note: column "note" has type int4, which cannot be read into Option<String>
Product.note: the result has no column "note"
```

> **SQL lens:** preparing a statement parses and analyses it, which is how the server knows the
> columns of an `INSERT … RETURNING` that has not run. Nothing is executed until a *Bind* and
> an *Execute* follow, so an `INSERT` that is only prepared writes nothing. shelf's check sits
> between the two exchanges, which is why the count above is still 1.

Two problems are left for the rows, because a statement cannot reveal them:

- **A NULL in a field that is not an `Option`** (`Mapping::Null`). A column's type says nothing
  about whether it holds NULL, so this is found only when one arrives
  ([§1.3](01-mapping-by-hand.md#13-three-ways-to-fail) showed the message). Chapter 13's
  `verify` narrows it down, from the catalog, for a table's own columns.
- **A value that cannot be decoded or converted** (`Mapping::Decode`): a `try_from` that
  refuses it, or a `jsonb` document that does not fit the field's type.

Both still name the struct, the field and the column, because the per-row helper that reads a
value is given the same names as `column` (In shelf, below). And tokio-postgres's own
`accepts` check still runs inside `try_get(index)` on every row: the measurement behind Chapter
1's timings found that skipping it, through a raw-bytes wrapper, saves nothing measurable
([orm/PLAN.md §12](../PLAN.md#12-findings-from-building-the-software-material-for-the-chapters),
finding 8), and it turns a misuse of `from_row_at` into an error instead of garbage
(Exercise 3).

## 5.4 What the derive generates

The derive writes the stage program's impl. You have seen its output since §0.7, and each
piece has a counterpart in the stage:

| The stage program (§5.2) | `#[derive(FromRow)]` |
|---|---|
| `type Columns = (usize, usize);` | `type Columns = (usize, usize);`, one element per field |
| `column::<String>(columns, "sku")?` | `::shelf::__private::column::<String>(columns, "Product", "sku", "sku")?` |
| `row.try_get(columns.0)?` | `::shelf::__private::get::<String>(row, columns.0, "Product", "sku")?` |

The extra arguments are the error's context, written as literals at compile time: the struct,
the field and the column, which differ when the field is renamed (Chapter 4).

`Columns` is a tuple, not an array, because its elements are not all `usize`: a skipped field
contributes `()`, and a flattened one the nested struct's own `Columns`, whole (Chapter 4,
§4.3). A resolved `Columns` is therefore a small tree of positions, and for a concrete struct
you can print it. This program resolves a `Product` with a flattened `Price` and a skipped
field against a statement whose columns come in another order:

```rust
use shelf::FromRow;

#[derive(Debug, PartialEq, FromRow)]
struct Price {
    #[shelf(rename = "price_cents")]
    cents: i64,
    currency: String,
}

#[derive(Debug, PartialEq, FromRow)]
struct Product {
    sku: String,
    #[shelf(flatten)]
    price: Price,
    #[shelf(skip)]
    selected: bool,
}

#[tokio::main]
async fn main() -> Result<(), shelf::Error> {
    let client = shelf_playground::connect().await?;
    let statement = client
        .prepare("SELECT 'EUR' AS currency, 1299::int8 AS price_cents, 'MUG-01' AS sku")
        .await?;

    let columns = Product::resolve(statement.columns())?;
    println!("{columns:?}");
    assert_eq!(columns, (2, (1, 0), ()));

    let row = client.query_one(&statement, &[]).await?;
    let product = Product::from_row_at(&row, &columns)?;
    assert_eq!(
        product,
        Product {
            sku: "MUG-01".into(),
            price: Price {
                cents: 1299,
                currency: "EUR".into()
            },
            selected: false,
        }
    );
    Ok(())
}
```

```text
(2, (1, 0), ())
```

`sku` is at position 2. `Price`'s own `resolve`, searching the same list, found `price_cents`
at 1 and `currency` at 0. `selected` had nothing to find. `from_row_at` then walks the same
tree: `columns.0` for `sku`, and `&columns.1` handed to `Price::from_row_at` with the same row.

This is the trait in shelf:

~~~rust
// orm/shelf/src/row.rs (excerpt)
pub trait FromRow: Sized {
    /// Where this type's values are in a result, as found by `resolve`. For a derived struct,
    /// a tuple with one element per field.
    type Columns: Send + Sync + 'static;

    /// Finds and checks this type's columns in `columns`: a statement's, or a row's.
    fn resolve(columns: &[Column]) -> Result<Self::Columns, Error>;

    /// Reads `row`, whose columns `columns` was resolved from.
    fn from_row_at(row: &Row, columns: &Self::Columns) -> Result<Self, Error>;

    /// Reads a single row: `resolve`, then `from_row_at`. For many rows of one statement,
    /// resolve once instead ([`from_rows`] does).
    fn from_row(row: &Row) -> Result<Self, Error> {
        Self::from_row_at(row, &Self::resolve(row.columns())?)
    }
}
~~~

[orm/shelf/src/row.rs](../shelf/src/row.rs)

Three things in it go beyond the stage:

- **`from_row` stays**, as a provided method. It resolves against the row's own columns, which
  are its statement's (§5.1), and reads the row. For one row, that is the same work as the two
  steps, in one call. In a loop over many rows it repeats `resolve` for each, which §5.5
  measures.
- **`Columns` is `Send`.** shelf's generic code holds a `T::Columns` across an `.await`:
  `fetch_all` resolves, then awaits the query, then reads the rows. Its future, and the
  `+ Send` futures of the model methods built on it (Chapter 6's `find` and `all`, for one),
  can be `Send` for an unknown `T` only if the trait promises it; removing `Send` from the
  bound stops shelf compiling. `Sync` and `'static` are not needed by any code in shelf
  today. They cost nothing, since a tuple of positions has both, and they promise generic code
  that a resolved `Columns` can be shared between tasks and kept as long as its statement,
  which is what a cache of them would need (§5.6).
- **Tuples choose `()`**, as Chapter 2 showed: their positions are fixed, so their `resolve`
  only checks each column's type, with `column_at`.

For rows you already have, `from_rows` resolves against the first row and reads them all:

~~~rust
// orm/shelf/src/row.rs (excerpt)
/// Reads every row of one statement into `T`, resolving the columns once.
pub fn from_rows<T: FromRow>(rows: &[Row]) -> Result<Vec<T>, Error> {
    let Some(first) = rows.first() else {
        return Ok(Vec::new());
    };
    let columns = T::resolve(first.columns())?;
    rows.iter()
        .map(|row| T::from_row_at(row, &columns))
        .collect()
}
~~~

[orm/shelf/src/row.rs](../shelf/src/row.rs)

> **Pitfall:** `from_rows` gets rows, not a statement, so on an empty result it has no columns
> to check, and returns an empty `Vec` whatever the struct. A test against an empty table
> passes, and the first row in production fails. When you have the SQL, call `fetch_all`,
> which checks the struct against the statement whether or not any row comes back.

## 5.5 Measured

The shelf crate has a benchmark for this, `bench_mapping`. It reads 100,000 rows with the
twelve column types of Chapter 1's program, again in the reverse of the struct's order, four
ways, and times only the mapping: by name and by index written by hand, and through shelf,
resolved once (`from_rows`) and resolved again for every row (`from_row` in a loop). It runs in
release mode and prints the median of five rounds as a table:

```bash
cargo run -p shelf --release --example bench_mapping
```

One run printed:

| Mapping, 100000 rows × 12 columns | Median of 5 | × |
|---|---:|---:|
| by name, by hand (`try_get("c01")`) | 40.6 ms | 3.0 |
| by index, by hand (`try_get(i)`) | 13.5 ms | 1.0 |
| shelf, resolved once (`from_rows`) | 14.1 ms | 1.1 |
| shelf, resolved per row (`from_row`) | 41.3 ms | 3.1 |

Three runs on Apple Silicon (rustc 1.99.1, PostgreSQL 18.6 on localhost) gave 40.6–41.7 ms
by name, 13.2–13.5 ms by index, 14.1–14.4 ms resolved once and 41.3–42.7 ms resolved per row
([orm/PLAN.md §12](../PLAN.md#12-findings-from-building-the-software-material-for-the-chapters),
findings 13 and 58). Resolved once, shelf costs 4–8 per cent more than the hand-written index
code, which in exchange you would have to keep in step with every `SELECT`. Resolved for every
row, it costs what lookups by name cost, because that is what it does: `resolve` walks the
column list once per field. That is why `fetch_all` and `from_rows` resolve once, and why
`from_row` is for a single row.

The book's examples run as debug builds, where the gap is smaller but still clear. This
program measures the two shelf ways on the same rows, and then the cost of one `resolve` on its
own, against the cheapest round trip there is:

```rust
use std::hint::black_box;
use std::time::{Duration, Instant};

use shelf::FromRow;

#[derive(FromRow)]
#[allow(dead_code)] // only the mapping is measured
struct Wide {
    c01: i64,
    c02: String,
    c03: i32,
    c04: Option<String>,
    c05: i64,
    c06: bool,
    c07: String,
    c08: i64,
    c09: Option<i32>,
    c10: String,
    c11: i64,
    c12: f64,
}

const SQL: &str = "
    SELECT i::float8 AS c12, i AS c11, 'name ' || i AS c10,
           CASE WHEN i % 3 = 0 THEN NULL ELSE i::int END AS c09,
           i AS c08, 'tag ' || i AS c07, i % 2 = 0 AS c06, i AS c05,
           CASE WHEN i % 5 = 0 THEN NULL ELSE 'note' END AS c04,
           i::int AS c03, 'sku-' || i AS c02, i AS c01
    FROM generate_series(1, 100000::bigint) AS i";

#[tokio::main]
async fn main() -> Result<(), shelf::Error> {
    let client = shelf_playground::connect().await?;
    let rows = client.query(SQL, &[]).await?;

    // The same rows, read twice: resolving once, and resolving for every row.
    let mut once = Duration::MAX;
    let mut per_row = Duration::MAX;
    for _ in 0..3 {
        let start = Instant::now();
        let a: Vec<Wide> = shelf::from_rows(&rows)?;
        once = once.min(start.elapsed());

        let start = Instant::now();
        let b = rows
            .iter()
            .map(Wide::from_row)
            .collect::<Result<Vec<_>, _>>()?;
        per_row = per_row.min(start.elapsed());
        assert_eq!(a.len(), b.len());
    }
    println!(
        "{} rows: resolved once {once:.1?}, per row {per_row:.1?} ({:.1}×)",
        rows.len(),
        per_row.as_secs_f64() / once.as_secs_f64()
    );
    assert!(once < per_row);

    // One resolve on its own, against one round trip of a prepared `SELECT 1`.
    let statement = client.prepare(SQL).await?;
    let start = Instant::now();
    for _ in 0..10_000 {
        black_box(Wide::resolve(black_box(statement.columns()))?);
    }
    let resolve = start.elapsed() / 10_000;
    let ping = client.prepare("SELECT 1").await?;
    let start = Instant::now();
    for _ in 0..100 {
        client.query(&ping, &[]).await?;
    }
    let round_trip = start.elapsed() / 100;
    println!("one resolve {resolve:.1?}, one round trip {round_trip:.1?}");
    assert!(resolve * 10 < round_trip);
    Ok(())
}
```

```text
100000 rows: resolved once 134.5ms, per row 226.7ms (1.7×)
one resolve 874.0ns, one round trip 116.2µs
```

Three runs gave 1.7× each time (134.5–135.6 ms resolved once, 225.6–227.2 ms per row): smaller
than release's 3×, as Chapter 1's debug ratio was, and with a wide margin for the assertion.
One `resolve` of the twelve fields took 815–874 ns, and one round trip 77–116 µs; the
assertion asks only for a factor of ten.

## 5.6 Where shelf resolves

`fetch_all` is the stage program's helper, with shelf's executor (Chapter 7) in place of a
`Client`:

~~~rust
// orm/shelf/src/fetch.rs (excerpt)
/// Every row of `sql`, read into `T`.
pub async fn fetch_all<T: FromRow>(
    db: &impl Executor,
    sql: &str,
    params: &[&(dyn ToSql + Sync)],
) -> Result<Vec<T>, Error> {
    let statement = db.prepare(sql).await?;
    let columns = T::resolve(statement.columns())?;
    let rows = db.query(&statement, params).await?;
    rows.iter()
        .map(|row| T::from_row_at(row, &columns))
        .collect()
}
~~~

[orm/shelf/src/fetch.rs](../shelf/src/fetch.rs)

`fetch_one` and `fetch_opt` differ only in the call that runs the query. The `Statement` comes
from the executor's `prepare`. On a plain `Client` or `Transaction`, that is the first of the
two round trips `query` would make with SQL text anyway
([main book §4.1](../../book/04-prepared-statements.md#41-what-query-sends)), so the check
costs no round trip of its own. On a deadpool connection it is `prepare_cached`, which after
the first call returns the statement from the connection's cache without asking the server
([main book §13.4](../../book/13-pooling.md#134-object-prepare_cached-transaction-and-objecttake);
Chapter 7 measures the strategies).

Everything in shelf that reads rows goes through one of these, or resolves the same way itself.
The model methods (`find`, `all`, `insert`) and the query builder call `fetch_*`. The `update`
of a versioned model prepares and resolves by itself, because its result carries two more
columns that say whether the row was written (Chapter 9). `fetch_stream` resolves before the
first row and keeps the `Columns` in the stream for as long as rows arrive (Chapter 12). And
`verify` goes a step further: it calls the same `column` for every field against the table
itself, at startup, rather than against one query (Chapter 13).

### Once per call, not once per connection

`resolve` runs on every call, even when deadpool's cache hands back a statement that was
resolved a minute ago. shelf could keep the `Columns` next to the cached statement, in a map
per connection keyed by the statement and the struct's type, and skip the work. §5.5 says what
that would save: one `resolve`, under a microsecond even in a debug build. In a release build,
one `resolve` of the twelve-column struct took 262–268 ns, and one round trip of a prepared
`SELECT 1` to a server on the same machine 46–47 µs (PLAN.md §12, finding 59): resolving is
about half a per cent of the cheapest call there is, and less over a real network. A cache
would cost a map lookup per call (hashing the SQL or the statement's name, and the type), a
place to keep the map per connection, and a rule for when a statement goes stale
([main book §4.6](../../book/04-prepared-statements.md#46-caching-statements-and-when-the-cache-goes-stale)).
shelf does not have one. Chapter 7's exercise builds a statement cache for a plain `Client`, and
a `Columns` cache would sit next to it.

### Side by side — *reading 100,000 rows*

The task: read 100,000 rows of four columns, one of them nullable, into a struct.

~~~rust
// sqlx (illustration)
#[derive(sqlx::FromRow)]
struct Product {
    id: i64,
    sku: String,
    price_cents: i64,
    note: Option<String>,
}

// The derive reads each field with row.try_get("sku"): a hash-map lookup per field per row.
let products: Vec<Product> = sqlx::query_as(SQL).fetch_all(&pool).await?;
~~~

By hand, finding the positions once from the prepared statement, and with shelf, in one program
that checks that both read the same values:

```rust
use shelf::FromRow;
use tokio_postgres::Statement;

#[derive(Debug, PartialEq, FromRow)]
struct Product {
    id: i64,
    sku: String,
    price_cents: i64,
    note: Option<String>,
}

const SQL: &str = "
    SELECT i AS id, 'sku-' || i AS sku, i * 100 AS price_cents,
           CASE WHEN i % 10 = 0 THEN 'clearance' END AS note
    FROM generate_series(1, 100000::bigint) AS i";

/// By hand: a column's position in the statement's result.
fn position(statement: &Statement, name: &str) -> usize {
    statement
        .columns()
        .iter()
        .position(|c| c.name() == name)
        .expect("the column is in the result")
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = shelf_playground::connect().await?;

    // By hand: prepare, find the positions once, then read every row by position.
    let statement = client.prepare(SQL).await?;
    let [id, sku, price_cents, note] =
        ["id", "sku", "price_cents", "note"].map(|name| position(&statement, name));
    let by_hand = client
        .query(&statement, &[])
        .await?
        .iter()
        .map(|row| {
            Ok(Product {
                id: row.try_get(id)?,
                sku: row.try_get(sku)?,
                price_cents: row.try_get(price_cents)?,
                note: row.try_get(note)?,
            })
        })
        .collect::<Result<Vec<_>, tokio_postgres::Error>>()?;

    // shelf: the same steps, in one call.
    let with_shelf: Vec<Product> = shelf::fetch_all(&client, SQL, &[]).await?;

    assert_eq!(by_hand.len(), 100_000);
    assert_eq!(by_hand, with_shelf);
    assert_eq!(with_shelf[9].note.as_deref(), Some("clearance"));
    Ok(())
}
```

| | by hand | shelf | sqlx |
|---|---|---|---|
| **Same** | Each field's column is found by its exact name in the statement's column list, and NULL is `Option` | The same | The same |
| **Different** | You find the positions once, in each function that reads; nothing checks the types before the first row, and a missing column is whatever you make of it (a panic here) | The derive finds the positions and checks every type once per statement, before the query runs, even when no row comes back; errors name the struct, the field, the column and both types | Each field is looked up on every row, in a `HashMap` of names that the statement carries (sqlx-postgres 0.9.0's `PgRow`); a type is checked per value and not for a NULL, so a type mismatch in an empty result, or in a column that holds only NULLs, is never reported; a name that appears twice finds the last column |

The last cell is worth a sentence. sqlx builds its map by inserting each column's name in
order, so for a result with two columns called `id` the second insert wins; tokio-postgres's
`position`, and so shelf's `column`, finds the first. Exercise 2 is about that.

---

## In shelf

The per-row half is `get`, which every field's read in `from_row_at` calls, and the function
that turns tokio-postgres's error into shelf's:

~~~rust
// orm/shelf/src/row.rs (excerpt)
/// Reads the value at `index` (found by [`column`]) as `T`.
#[doc(hidden)]
pub fn get<T: FromColumn>(
    row: &Row,
    index: usize,
    model: &'static str,
    field: &'static str,
) -> Result<T, Error> {
    row.try_get(index)
        .map_err(|e| decode_error(row, index, model, field, e))
}

// …

fn decode_error(
    row: &Row,
    index: usize,
    model: &'static str,
    field: &'static str,
    error: tokio_postgres::Error,
) -> Error {
    let column = column_name(row, index);
    let message = error.to_string();
    let problem = match error.into_source() {
        Some(source) if source.is::<WasNull>() => Mapping::Null { column },
        Some(source) => Mapping::Decode { column, source },
        // No cause: the index is not in the row, which `resolve` rules out unless the row
        // comes from another statement.
        None => Mapping::Decode {
            column,
            source: message.into(),
        },
    };
    Error::Mapping {
        model,
        field,
        problem,
    }
}
~~~

[orm/shelf/src/row.rs](../shelf/src/row.rs)

`try_get(index)` does tokio-postgres's per-row work: the bounds check, `accepts`, and the
decoding. Its error carries the cause one level down, in `source()` (§1.3), and `decode_error`
sorts it: a `WasNull` cause is `Mapping::Null`, any other cause is `Mapping::Decode`. An error
with no cause at all means the position does not exist in the row, which `resolve` rules out
for every row of the statement it resolved: it can only happen when the row comes from another
statement with fewer columns. (A row of another statement whose column at that position has
the wrong type fails `accepts`, which is a cause: Exercise 3.) The model and field names travel
as `&'static str`, so the context costs nothing until something fails.

## When not to resolve once

- **One row.** `Product::from_row(&row)` resolves and reads in one call. For a single row it is
  the same work as the two steps, and nothing is gained by keeping the `Columns`.
- **Rows of different statements.** A `Columns` belongs to the statement it was resolved from.
  Rows of two queries, even queries that look alike, are each read with their own, or with
  `from_row`.
- **When you need only a value or two.** As in Chapter 1, `query_one_scalar` or `row.get` in
  place needs no struct and no `resolve`.

## Summary

- In tokio-postgres 0.7.18 a `Row` holds its `Statement`, and every row of a statement returns
  the same column list. Where a field's column is, and whether its type can be read into the
  field, depend only on the statement.
- `Client::prepare` gives a `Statement` whose `columns()` (name, type, table OID and column
  number) are known before anything runs, from the server's *RowDescription*.
- `FromRow` has an associated type, `Columns`, and two steps: `resolve` finds each field's
  column by its exact name and checks `accepts` once per statement; `from_row_at` reads a row by
  position. `from_row` does both, for one row.
- A missing column or a wrong type is reported before the query runs, even when it would
  return no rows; an `INSERT … RETURNING` that does not fit its struct is never executed. A
  NULL in a non-`Option` field and a value that fails to decode can only be found in a row.
- `Columns` is a tuple, one element per field: `usize`, `()` for `skip`, the nested `Columns`
  for `flatten`. It is `Send` because shelf's futures hold it across an `.await`.
- Resolving once costs 4–8 per cent over hand-written index code; resolving per row costs what
  lookups by name cost (about 3× in release, 1.7× in a debug build). One `resolve` costs well
  under a microsecond, so shelf resolves on every call and caches nothing.
- tokio-postgres still checks `accepts` on every row. It costs nothing measurable, and it turns
  a row read with another statement's positions into an error instead of garbage.
- `from_rows` on an empty slice checks nothing; `fetch_all` checks against the statement.

## Exercises

1. **One statement, two executions.** Prepare a statement with a parameter, resolve a struct
   against it once, and read the rows of two executions with different parameters.
   *Hint:* `Columns` belongs to the statement, not to the execution; `Product::resolve` and
   `Product::from_row_at` are public.
2. **Two columns, one name.** A join such as
   `SELECT o.id, l.id, l.sku FROM orders o JOIN lines l ON l.order_id = o.id` returns two
   columns named `id`. Which one does shelf read into a struct with an `id` field, and which
   would sqlx read? Write a `resolve_unique::<T>` that refuses such a result before calling
   `T::resolve`. Then decide whether shelf should refuse it.
   *Hint:* compare each column's name with the names before it. §5.6's side by side says what
   each library does.
3. **Positions from another statement.** Resolve a `Product { sku: String, price_cents: i64 }`
   against `SELECT sku, price_cents …`, then call `from_row_at` on a row of
   `SELECT price_cents, sku …`. What happens? What would happen if tokio-postgres did not call
   `accepts` on every row?
   *Hint:* the positions point at the other column. `String`'s `FromSql` reads any bytes that
   are valid UTF-8, and an `int8` is eight bytes.

### Solutions

**1.** Resolve once, after `prepare`, and keep the `Columns` for as long as the statement:

```rust
use shelf::FromRow;

#[derive(Debug, FromRow)]
struct Product {
    sku: String,
    price_cents: i64,
}

#[tokio::main]
async fn main() -> Result<(), shelf::Error> {
    let client = shelf_playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE products (sku text PRIMARY KEY, price_cents bigint NOT NULL);
             INSERT INTO products VALUES ('MUG-01', 1299), ('TEE-02', 2499), ('VASE-03', 3999);",
        )
        .await?;

    let statement = client
        .prepare("SELECT sku, price_cents FROM products WHERE price_cents < $1 ORDER BY sku")
        .await?;
    let columns = Product::resolve(statement.columns())?; // once, for every execution

    for (limit, expected) in [(2000_i64, vec!["MUG-01"]), (3000, vec!["MUG-01", "TEE-02"])] {
        let rows = client.query(&statement, &[&limit]).await?;
        let products = rows
            .iter()
            .map(|row| Product::from_row_at(row, &columns))
            .collect::<Result<Vec<_>, _>>()?;
        let skus: Vec<&str> = products.iter().map(|p| p.sku.as_str()).collect();
        assert_eq!(skus, expected);
        assert!(products.iter().all(|p| p.price_cents < limit));
    }
    Ok(())
}
```

This is what a repository function that runs one query many times can do by hand. shelf's
`fetch_*` take SQL text rather than a `Statement`, so each call prepares (or asks the cache) and
resolves again, which §5.6 found not worth avoiding.

**2.** shelf reads the first `id`, the order's, as `Row::try_get("id")` would; sqlx would read
the last, the line's. Neither says anything. A check that refuses the result:

```rust
use shelf::FromRow;
use tokio_postgres::Column;

#[derive(Debug, FromRow)]
struct Line {
    id: i64,
    sku: String,
}

/// `T`'s columns, refusing a result in which two columns have the same name.
fn resolve_unique<T: FromRow>(columns: &[Column]) -> Result<T::Columns, String> {
    for (i, column) in columns.iter().enumerate() {
        if columns[..i].iter().any(|c| c.name() == column.name()) {
            return Err(format!(
                "the result has two columns named {:?}; rename one with AS",
                column.name()
            ));
        }
    }
    T::resolve(columns).map_err(|e| e.to_string())
}

#[tokio::main]
async fn main() -> Result<(), shelf::Error> {
    let client = shelf_playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE orders (id bigint PRIMARY KEY);
             CREATE TEMP TABLE lines (
                 id bigint PRIMARY KEY,
                 order_id bigint NOT NULL,
                 sku text NOT NULL
             );
             INSERT INTO orders VALUES (7);
             INSERT INTO lines VALUES (70, 7, 'MUG-01');",
        )
        .await?;
    let join = "SELECT o.id, l.id, l.sku FROM orders o JOIN lines l ON l.order_id = o.id";

    // shelf reads the first column called `id`: the order's.
    let line: Line = shelf::fetch_one(&client, join, &[]).await?;
    assert_eq!((line.id, line.sku.as_str()), (7, "MUG-01"));

    let statement = client.prepare(join).await?;
    let error = resolve_unique::<Line>(statement.columns()).unwrap_err();
    println!("{error}");
    assert!(error.starts_with(r#"the result has two columns named "id""#));

    // With an alias, the line's own id is the only `id`.
    let line: Line = shelf::fetch_one(
        &client,
        "SELECT o.id AS order_id, l.id, l.sku FROM orders o JOIN lines l ON l.order_id = o.id",
        &[],
    )
    .await?;
    assert_eq!(line.id, 70);
    Ok(())
}
```

```text
the result has two columns named "id"; rename one with AS
```

shelf does not refuse such a result. Two columns of one name are legal SQL, and common in a
join, and the struct may read neither of them; refusing every such result would refuse queries
that work. The precise check refuses only a name the struct reads, and belongs in `column`,
which knows the name: after `position` finds the first match, a second search of the rest of
the list finds an ambiguous one, at the cost of walking the whole list for every field. shelf
leaves it out (PLAN.md §10); the alias is the fix either way.

**3.** The positions say `sku` is column 0, which in the other statement is `price_cents`, an
`int8`. tokio-postgres's per-row `accepts` refuses to read it as a `String`, and shelf reports
it as a decoding problem of the field `sku`, naming the column it actually found:

```rust
use shelf::{FromRow, Mapping};
use tokio_postgres::types::{FromSql, Type};

#[derive(Debug, FromRow)]
#[allow(dead_code)] // the program only shows the error
struct Product {
    sku: String,
    price_cents: i64,
}

#[tokio::main]
async fn main() -> Result<(), shelf::Error> {
    let client = shelf_playground::connect().await?;
    let first = client
        .prepare("SELECT 'MUG-01' AS sku, 1299::int8 AS price_cents")
        .await?;
    let columns = Product::resolve(first.columns())?;
    assert_eq!(columns, (0, 1));

    // A row of another statement, whose columns are the other way round.
    let row = client
        .query_one("SELECT 1299::int8 AS price_cents, 'MUG-01' AS sku", &[])
        .await?;
    let error = Product::from_row_at(&row, &columns).unwrap_err();
    println!("{error}");
    assert!(matches!(
        error,
        shelf::Error::Mapping {
            field: "sku",
            problem: Mapping::Decode { .. },
            ..
        }
    ));

    // Without the per-row check, `String` would decode the int8's eight bytes as text.
    let text = <String as FromSql>::from_sql(&Type::TEXT, &1299_i64.to_be_bytes())
        .expect("eight bytes that happen to be valid UTF-8");
    assert_eq!(text, "\0\0\0\0\0\0\u{5}\u{13}");
    Ok(())
}
```

```text
Product.sku: column "price_cents": cannot convert between the Rust type `alloc::string::String` and the Postgres type `int8`
```

Without the per-row check, the read would succeed: the eight bytes of 1299 as a big-endian
`int8` are all valid UTF-8, so `sku` would hold six NUL characters and two control characters,
and nothing would report it. That is the case for keeping tokio-postgres's check on every row,
although `resolve` makes it redundant for correct code: it costs nothing measurable, and it
turns a misuse into an error.

---

Previous: [Chapter 4 — Attributes, errors and generics](04-attributes.md) · Next: [Chapter 6 — Tables: the model derive](06-tables.md)
