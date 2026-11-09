# Chapter 1 — Mapping rows by hand

> **Building an ORM on tokio-postgres** · Previous: [Chapter 0 — Introduction and setup](00-introduction.md) · Next: [Chapter 2 — Declarative macros](02-declarative-macros.md)

You have written this function before. Every struct that stockroom reads from the database has
a `TryFrom<&Row>` that names each field and reads each column: `id: row.try_get("id")?`,
`code: row.try_get("code")?`, once per field, for every struct
([main book §3.5](../../book/03-rows-and-types.md#35-rows-into-structs)). The SQL above it
names the same columns again. Rename a column and you change three places, and nothing checks
any of them against the others until a row arrives.

An ORM's first job is to write that function for you. Before writing the macro that does it,
this chapter writes it by hand once more, carefully: as a trait of shelf's own, with the three
ways it can fail, nested structs, and helpers that run a query and map its rows. It also
measures what reading a column by name actually costs, because that number decides the shape of
the derive in Part I.

**In this chapter you will learn:**

- why shelf has a `FromRow` trait of its own instead of using `TryFrom<&Row>`
- what `Row::try_get("sku")` does in tokio-postgres 0.7.18, and what it costs per row
- the three ways a mapping fails, and what each error says and does not say
- how to read a nested struct from the same row
- `fetch_all`, `fetch_one` and `fetch_opt`: a query and its mapping in one call, on a client or
  a transaction
- which parts of the hand-written code a macro should take over

---

## 1.1 A trait of shelf's own

The mapping needs a name. stockroom uses the standard library's `TryFrom<&Row>`; shelf defines
a trait instead, with one method for now:

```rust
use tokio_postgres::Row;

/// A value built from one result row.
trait FromRow: Sized {
    fn from_row(row: &Row) -> Result<Self, tokio_postgres::Error>;
}

#[derive(Debug, PartialEq)]
struct Product {
    id: i64,
    sku: String,
    name: String,
    price_cents: i64,
    tags: Option<Vec<String>>,
}

impl FromRow for Product {
    fn from_row(row: &Row) -> Result<Self, tokio_postgres::Error> {
        Ok(Product {
            id: row.try_get("id")?,
            sku: row.try_get("sku")?,
            name: row.try_get("name")?,
            price_cents: row.try_get("price_cents")?,
            tags: row.try_get("tags")?,
        })
    }
}

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = shelf_playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE products (
                 id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
                 sku text NOT NULL UNIQUE,
                 name text NOT NULL,
                 price_cents bigint NOT NULL,
                 tags text[]
             );
             INSERT INTO products (sku, name, price_cents, tags)
             VALUES ('MUG-01', 'Mug', 1299, '{kitchen,sale}'),
                    ('TEE-02', 'T-shirt', 2499, NULL);",
        )
        .await?;

    let rows = client
        .query(
            "SELECT id, sku, name, price_cents, tags FROM products ORDER BY id",
            &[],
        )
        .await?;
    let products = rows
        .iter()
        .map(Product::from_row)
        .collect::<Result<Vec<_>, _>>()?;

    assert_eq!(products.len(), 2);
    assert_eq!(
        products[0].tags,
        Some(vec!["kitchen".into(), "sale".into()])
    );
    assert_eq!(
        products[1],
        Product {
            id: products[1].id,
            sku: "TEE-02".into(),
            name: "T-shirt".into(),
            price_cents: 2499,
            tags: None,
        }
    );
    Ok(())
}
```

The body is the same as stockroom's `try_from`. What a trait of shelf's own buys is room to
grow, and three things `TryFrom<&Row>` cannot give:

- **Associated items.** Chapter 5 adds an associated type and two methods to `FromRow`, so
  that a statement's columns are found once instead of on every row. `TryFrom` is the standard
  library's trait, and nobody can add to it.
- **A short bound.** A generic helper over `TryFrom<&Row>` must say "for a row borrowed for any
  lifetime", as stockroom's `from_rows` does: `for<'r> T: TryFrom<&'r Row, Error =
  tokio_postgres::Error>`. Over `FromRow` it says `T: FromRow`.
- **Impls for types that are not yours.** Reading `SELECT sku, price_cents` into a tuple,
  `(String, i64)`, needs an impl for tuples. Rust's *orphan rule* allows an impl only if the
  trait or the type belongs to your crate, and for `TryFrom<&Row> for (A, B)`, neither does.
  This fails to compile in any crate but tokio-postgres itself:

~~~rust
use tokio_postgres::Row;
use tokio_postgres::types::FromSqlOwned;

impl<A: FromSqlOwned, B: FromSqlOwned> TryFrom<&Row> for (A, B) {
    type Error = tokio_postgres::Error;
    fn try_from(row: &Row) -> Result<Self, Self::Error> {
        Ok((row.try_get(0)?, row.try_get(1)?))
    }
}
~~~

```text
error[E0117]: only traits defined in the current crate can be implemented for arbitrary types
 --> src/bin/orphan.rs:4:1
  |
4 | impl<A: FromSqlOwned, B: FromSqlOwned> TryFrom<&Row> for (A, B) {
  | ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^-------------^^^^^------
  |                                        |                 |
  |                                        |                 this is not defined in the current crate because tuples are always foreign
  |                                        `tokio_postgres::Row` is not defined in the current crate
  |
  = note: impl doesn't have any local type before any uncovered type parameters
  = note: for more information see https://doc.rust-lang.org/reference/items/implementations.html#orphan-rules
  = note: define and implement a trait or new type instead
```

With `FromRow` defined in shelf, `impl<A, B> FromRow for (A, B)` is allowed, because the trait
is local. Chapter 2 writes those impls for twelve tuple sizes at once.

> **Rust refresher:** `FromSqlOwned` is tokio-postgres's name for "a type that can be read from
> a column without borrowing from the row", a shorthand for `for<'a> FromSql<'a>`
> ([main book §3.4](../../book/03-rows-and-types.md#34-borrowing-from-a-row)). `String` is
> one; `&str` is not, because it points into the row's buffer. A mapping trait whose method
> returns `Self` with no lifetime, like `FromRow`, can only build owned values, and shelf keeps
> it that way: Chapter 4 refuses a struct with a lifetime parameter, with an error that says why.

## 1.2 By name or by position

`row.try_get("sku")` looks like a map lookup, but a `Row` has no map. In 0.7.18 it walks the
row's columns and compares each name with `"sku"`. If none matches exactly, it walks them again,
ignoring ASCII case
([`row.rs`](https://docs.rs/tokio-postgres/0.7.18/src/tokio_postgres/row.rs.html)):

~~~rust
// tokio-postgres 0.7.18: src/row.rs, abridged
impl RowIndex for str {
    fn __idx<T>(&self, columns: &[T]) -> Option<usize>
    where
        T: AsName,
    {
        if let Some(idx) = columns.iter().position(|d| d.as_name() == self) {
            return Some(idx);
        };

        // FIXME ASCII-only case insensitivity isn't really the right thing to
        // do. Postgres itself uses a dubious wrapper around tolower and JDBC
        // uses the US locale.
        columns
            .iter()
            .position(|d| d.as_name().eq_ignore_ascii_case(self))
    }
}
~~~

Then, whether you asked by name or by position, `try_get` checks that the column's type can be
read into the Rust type you asked for, and only then decodes the bytes:

~~~rust
// tokio-postgres 0.7.18: src/row.rs, abridged
fn get_inner<'a, I, T>(&'a self, idx: &I) -> Result<T, Error>
where
    I: RowIndex + fmt::Display,
    T: FromSql<'a>,
{
    let idx = match idx.__idx(self.columns()) {
        Some(idx) => idx,
        None => return Err(Error::column(idx.to_string())),
    };

    let ty = self.columns()[idx].type_();
    if !T::accepts(ty) {
        return Err(Error::from_sql(
            Box::new(WrongType::new::<T>(ty.clone())),
            idx,
        ));
    }

    FromSql::from_sql_nullable(ty, self.col_buffer(idx)).map_err(|e| Error::from_sql(e, idx))
}
~~~

So a struct of twelve fields read by name does twelve searches per row. A search that finds
its column at position *p* compares *p* + 1 names, so the twelve together compare
1 + 2 + … + 12 = 78 names on every row, whatever order the columns come in. A missing column
costs two full walks, and the second walk means `"SKU"` finds a column named `sku`.

What does that cost? This program reads 100,000 rows of twelve columns (four `bigint`, four
`text`, two `integer`, a `boolean` and a `double precision`, two of them sometimes NULL) into
the same struct twice: by name, and by position. It fetches the rows once and times only the
mapping, best of three rounds:

```rust
use std::time::{Duration, Instant};
use tokio_postgres::Row;

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

fn by_name(r: &Row) -> Result<Wide, tokio_postgres::Error> {
    Ok(Wide {
        c01: r.try_get("c01")?,
        c02: r.try_get("c02")?,
        c03: r.try_get("c03")?,
        c04: r.try_get("c04")?,
        c05: r.try_get("c05")?,
        c06: r.try_get("c06")?,
        c07: r.try_get("c07")?,
        c08: r.try_get("c08")?,
        c09: r.try_get("c09")?,
        c10: r.try_get("c10")?,
        c11: r.try_get("c11")?,
        c12: r.try_get("c12")?,
    })
}

// The SELECT lists the columns in reverse, so c01 is at position 11.
fn by_position(r: &Row) -> Result<Wide, tokio_postgres::Error> {
    Ok(Wide {
        c01: r.try_get(11)?,
        c02: r.try_get(10)?,
        c03: r.try_get(9)?,
        c04: r.try_get(8)?,
        c05: r.try_get(7)?,
        c06: r.try_get(6)?,
        c07: r.try_get(5)?,
        c08: r.try_get(4)?,
        c09: r.try_get(3)?,
        c10: r.try_get(2)?,
        c11: r.try_get(1)?,
        c12: r.try_get(0)?,
    })
}

fn best_of_three(
    rows: &[Row],
    map: fn(&Row) -> Result<Wide, tokio_postgres::Error>,
) -> Result<Duration, tokio_postgres::Error> {
    let mut best = Duration::MAX;
    for _ in 0..3 {
        let start = Instant::now();
        let mapped: Vec<Wide> = rows.iter().map(map).collect::<Result<_, _>>()?;
        best = best.min(start.elapsed());
        assert_eq!(mapped.len(), rows.len());
    }
    Ok(best)
}

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = shelf_playground::connect().await?;
    let rows = client
        .query(
            "SELECT i::float8 AS c12, i AS c11, 'name ' || i AS c10,
                    CASE WHEN i % 3 = 0 THEN NULL ELSE i::int END AS c09,
                    i AS c08, 'tag ' || i AS c07, i % 2 = 0 AS c06, i AS c05,
                    CASE WHEN i % 5 = 0 THEN NULL ELSE 'note' END AS c04,
                    i::int AS c03, 'sku-' || i AS c02, i AS c01
             FROM generate_series(1, 100000::bigint) AS i",
            &[],
        )
        .await?;

    let name = best_of_three(&rows, by_name)?;
    let position = best_of_three(&rows, by_position)?;
    println!(
        "{} rows: by name {name:.1?}, by position {position:.1?} ({:.1}×)",
        rows.len(),
        name.as_secs_f64() / position.as_secs_f64()
    );
    assert!(position < name);
    Ok(())
}
```

```text
100000 rows: by name 197.8ms, by position 123.2ms (1.6×)
```

That run was a debug build, which is how `book.py check` builds the examples (rustc 1.99.1,
Apple Silicon, PostgreSQL 18.6 on localhost); three runs gave 1.5× to 1.7×. In a release build the same comparison is 38 ms by
name against 12.5 ms by position, about **3×**
([orm/PLAN.md §12](../PLAN.md#12-findings-from-building-the-software-material-for-the-chapters),
findings 8 and 13). The lookups, not the decoding, are most of the work, and they repeat on
every row although the answer cannot change: every row of a statement has the same columns, in
the same order.

Reading by position is not the fix, though. Positions break silently when someone adds a column
to the `SELECT`, which is why the main book recommends names. The fix is to look each name up
**once per statement** and then read every row by position. That needs a trait with somewhere
to keep the positions, which is the first thing `FromRow` will grow (Chapter 5).

> **Pitfall:** the second, case-insensitive walk means a misspelt name can still find a column:
> `row.try_get("SKU")` reads a column named `sku`. PostgreSQL itself folds unquoted identifiers
> to lower case, and a quoted `"SKU"` is a different column from `sku`, so a mapping that
> matches names ignoring case can read the wrong one when both exist. shelf matches names
> exactly.

## 1.3 Three ways to fail

A row can fail to fit a struct in three ways: a column the struct needs is missing, a column
has a type the field cannot hold, or a column is NULL and the field is not an `Option`. This
program provokes each with a query that gets it wrong, and prints the error's `Display` and its
`Debug`:

```rust
use tokio_postgres::Row;

trait FromRow: Sized {
    fn from_row(row: &Row) -> Result<Self, tokio_postgres::Error>;
}

#[derive(Debug)]
#[allow(dead_code)] // the program only shows the errors
struct Product {
    sku: String,
    name: String,
    price_cents: i64,
}

impl FromRow for Product {
    fn from_row(row: &Row) -> Result<Self, tokio_postgres::Error> {
        Ok(Product {
            sku: row.try_get("sku")?,
            name: row.try_get("name")?,
            price_cents: row.try_get("price_cents")?,
        })
    }
}

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = shelf_playground::connect().await?;
    let mistakes = [
        // No `name` column.
        (
            "SELECT 'MUG-01' AS sku, 1299::int8 AS price_cents",
            "invalid column `name`",
        ),
        // `1299` is an int4, and the field is an i64.
        (
            "SELECT 'MUG-01' AS sku, 'Mug' AS name, 1299 AS price_cents",
            "error deserializing column 2",
        ),
        // A NULL name, and the field is a String.
        (
            "SELECT 'MUG-01' AS sku, NULL::text AS name, 1299::int8 AS price_cents",
            "error deserializing column 1",
        ),
    ];
    for (sql, expected) in mistakes {
        let row = client.query_one(sql, &[]).await?;
        let error = Product::from_row(&row).unwrap_err();
        println!("{error}\n    {error:?}");
        assert_eq!(error.to_string(), expected);
    }
    Ok(())
}
```

```text
invalid column `name`
    Error { kind: Column("name"), cause: None }
error deserializing column 2
    Error { kind: FromSql(2), cause: Some(WrongType { postgres: Int4, rust: "i64" }) }
error deserializing column 1
    Error { kind: FromSql(1), cause: Some(WasNull) }
```

tokio-postgres reports what it knows: a column, by name or position, and, one level down in
`source()`, the cause. What it cannot report is what you were doing: which struct, and which
field. In a service with forty row types, "error deserializing column 2" in a log is the start
of a search. Your `from_row` knows the struct and the field, but adding them by hand means a
`map_err` on every line (Exercise 1).

This is what shelf reports for the same three queries, with the same struct and
`#[derive(shelf::FromRow)]`:

```rust
#[derive(Debug, shelf::FromRow)]
#[allow(dead_code)] // the program only shows the errors
struct Product {
    sku: String,
    name: String,
    price_cents: i64,
}

#[tokio::main]
async fn main() -> Result<(), shelf::Error> {
    let client = shelf_playground::connect().await?;
    let mistakes = [
        (
            "SELECT 'MUG-01' AS sku, 1299::int8 AS price_cents",
            r#"Product.name: the result has no column "name""#,
        ),
        (
            "SELECT 'MUG-01' AS sku, 'Mug' AS name, 1299 AS price_cents",
            r#"Product.price_cents: column "price_cents" has type int4, which cannot be read into i64"#,
        ),
        (
            "SELECT 'MUG-01' AS sku, NULL::text AS name, 1299::int8 AS price_cents",
            r#"Product.name: column "name" is NULL; read it into an Option"#,
        ),
    ];
    for (sql, expected) in mistakes {
        let error = shelf::fetch_one::<Product>(&client, sql, &[])
            .await
            .unwrap_err();
        println!("{error}");
        assert_eq!(error.to_string(), expected);
    }
    Ok(())
}
```

```text
Product.name: the result has no column "name"
Product.price_cents: column "price_cents" has type int4, which cannot be read into i64
Product.name: column "name" is NULL; read it into an Option
```

Each message names the struct, the field, the column and, for a type mismatch, both types. The
first two are not even row errors in shelf: they are found when the statement is prepared,
before the query runs (Chapter 5). The third can only be found in a row, because a column's
type says nothing about whether it holds NULL.

> **SQL lens:** why is `1299` an `int4` and `'Mug'` a `text`? An integer literal that fits in
> 32 bits is typed `integer`; a quoted literal in a `SELECT` list, with nothing else to go on,
> is resolved as `text`. The server decides each column's type when it prepares the statement,
> and the row description carries it to the client. That is the type `accepts` checks: the
> Rust side never guesses, and never widens an `int4` into an `i64` for you.

## 1.4 Nested structs

Some structs are better split. A price is an amount and a currency, and it has behaviour of its
own (formatting, arithmetic), so it deserves a type. But in the table it is two columns of the
`products` row, not a table of its own. Reading it by hand means calling the inner struct's
`from_row` from the outer one, on the same row:

```rust
use tokio_postgres::Row;

trait FromRow: Sized {
    fn from_row(row: &Row) -> Result<Self, tokio_postgres::Error>;
}

#[derive(Debug, PartialEq)]
struct Price {
    cents: i64,
    currency: String,
}

impl FromRow for Price {
    fn from_row(row: &Row) -> Result<Self, tokio_postgres::Error> {
        Ok(Price {
            cents: row.try_get("price_cents")?,
            currency: row.try_get("currency")?,
        })
    }
}

#[derive(Debug, PartialEq)]
struct Product {
    sku: String,
    price: Price,
}

impl FromRow for Product {
    fn from_row(row: &Row) -> Result<Self, tokio_postgres::Error> {
        Ok(Product {
            sku: row.try_get("sku")?,
            price: Price::from_row(row)?, // the same row, read by the inner struct
        })
    }
}

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = shelf_playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE products (
                 sku text PRIMARY KEY,
                 price_cents bigint NOT NULL,
                 currency text NOT NULL
             );
             INSERT INTO products VALUES ('MUG-01', 1299, 'EUR');",
        )
        .await?;

    let row = client
        .query_one("SELECT sku, price_cents, currency FROM products", &[])
        .await?;
    let product = Product::from_row(&row)?;
    assert_eq!(
        product,
        Product {
            sku: "MUG-01".into(),
            price: Price {
                cents: 1299,
                currency: "EUR".into()
            },
        }
    );
    Ok(())
}
```

It works, and it shows the catch: the inner struct and the outer one take their names from the
same row. `Price` reads `price_cents` and `currency`, so it fits this query and no other. A
product with two prices, a list price and a sale price, cannot use `Price` twice: both would
read the same two columns. You would alias them in SQL and give `Price` a second impl, or pass a
prefix in at run time and build each name with `format!`, one allocation per column per row.

Chapter 4's `#[shelf(flatten)]` replaces the hand-written call with one attribute: it reads a
nested `FromRow` struct from the same row. The column names stay the inner struct's business,
which you set with its own `#[shelf(rename = "…")]` attributes, so the catch remains (§4.3).

## 1.5 Fetch helpers

Every read so far has had the same three steps: run the query, map each row, collect the
results into a `Result<Vec<_>, _>`. A helper does all three, and two siblings cover the other
shapes a result can have, exactly one row and at most one row:

```rust
use tokio_postgres::types::ToSql;
use tokio_postgres::{GenericClient, Row};

trait FromRow: Sized {
    fn from_row(row: &Row) -> Result<Self, tokio_postgres::Error>;
}

/// Every row of `sql`, read into `T`.
async fn fetch_all<T: FromRow>(
    db: &impl GenericClient,
    sql: &str,
    params: &[&(dyn ToSql + Sync)],
) -> Result<Vec<T>, tokio_postgres::Error> {
    let rows = db.query(sql, params).await?;
    rows.iter().map(T::from_row).collect()
}

/// The one row of `sql`; an error if there are none, or several.
async fn fetch_one<T: FromRow>(
    db: &impl GenericClient,
    sql: &str,
    params: &[&(dyn ToSql + Sync)],
) -> Result<T, tokio_postgres::Error> {
    let row = db.query_one(sql, params).await?;
    T::from_row(&row)
}

/// The row of `sql`, if there is one; an error if there are several.
async fn fetch_opt<T: FromRow>(
    db: &impl GenericClient,
    sql: &str,
    params: &[&(dyn ToSql + Sync)],
) -> Result<Option<T>, tokio_postgres::Error> {
    let row = db.query_opt(sql, params).await?;
    row.as_ref().map(T::from_row).transpose()
}

#[derive(Debug, PartialEq)]
struct Product {
    sku: String,
    price_cents: i64,
}

impl FromRow for Product {
    fn from_row(row: &Row) -> Result<Self, tokio_postgres::Error> {
        Ok(Product {
            sku: row.try_get("sku")?,
            price_cents: row.try_get("price_cents")?,
        })
    }
}

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let mut client = shelf_playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE products (sku text PRIMARY KEY, price_cents bigint NOT NULL);
             INSERT INTO products VALUES ('MUG-01', 1299), ('TEE-02', 2499);",
        )
        .await?;

    // On the client.
    let cheap: Vec<Product> = fetch_all(
        &client,
        "SELECT sku, price_cents FROM products WHERE price_cents < $1 ORDER BY sku",
        &[&2000_i64],
    )
    .await?;
    assert_eq!(cheap.len(), 1);
    let one: Product = fetch_one(
        &client,
        "SELECT sku, price_cents FROM products WHERE sku = $1",
        &[&"TEE-02"],
    )
    .await?;
    assert_eq!(one.price_cents, 2499);
    let none: Option<Product> = fetch_opt(
        &client,
        "SELECT sku, price_cents FROM products WHERE sku = $1",
        &[&"VASE-03"],
    )
    .await?;
    assert_eq!(none, None);

    // The same helpers inside a transaction: it sees its own insert, and nothing outside does.
    let tx = client.transaction().await?;
    tx.execute("INSERT INTO products VALUES ('VASE-03', 3999)", &[])
        .await?;
    let inside: Vec<Product> = fetch_all(&tx, "SELECT sku, price_cents FROM products", &[]).await?;
    assert_eq!(inside.len(), 3);
    tx.rollback().await?;
    let after: Vec<Product> =
        fetch_all(&client, "SELECT sku, price_cents FROM products", &[]).await?;
    assert_eq!(after.len(), 2);
    Ok(())
}
```

The helpers take tokio-postgres's `GenericClient`, which `Client` and `Transaction` both
implement ([main book Chapter 8](../../book/08-transactions.md)), so one repository function
can run inside or outside a transaction. A deadpool connection derefs, in two steps, to a
`Client`, so `&**object` passes too ([main book Chapter 13](../../book/13-pooling.md)). Chapter 7 replaces
`GenericClient` with a trait of shelf's own, because tokio-postgres's is sealed and has no
statement cache.

shelf's own `fetch_all` takes the same arguments, and does one thing more: it prepares the
statement first and checks the struct against the statement's columns before it runs the query.
That is why the first two mistakes in §1.3 were found before any row was read. It costs no extra
round trip, because `query` with a SQL string prepares the statement anyway
([main book §4.1](../../book/04-prepared-statements.md#41-what-query-sends)). Chapter 5 shows
how.

## 1.6 Counting the boilerplate

Look at what you typed for a struct of five fields, and where each name went:

| Where the name is written | Written by | Checked against the table |
|---|---|---|
| The struct's field (`price_cents: i64`) | you | never: the compiler knows nothing of the table |
| The `SELECT` list (`SELECT …, price_cents, …`) | you | by the server, when the statement is prepared |
| The `try_get` call (`row.try_get("price_cents")`) | you | when a row is read, against the `SELECT` list |

Each field is named three times, and the second and third are strings that nothing compares
with the first. The struct's name is in none of them, which is why §1.3's errors could not name
it. And every mapping has the same shape: one line per field, the field's name turned into a
string. That is the part a macro can take over:

- **The `from_row` body**: every line follows from the field's name and type.
- **The error context**: the struct's and the field's names are known when the code is
  generated, so every error can carry them at no cost.
- **The column positions**: the code can find them once per statement and keep them (Chapter 5).

What a macro cannot take over yet is the SQL. A derive that only sees the struct does not know
the table; Chapter 6's `#[derive(Table)]` is told it, and writes the column list too.

### Side by side — *a row into a struct*

The task: read products, with a nullable array column, into a struct.

~~~rust
// sqlx (illustration)
#[derive(sqlx::FromRow)]
struct Product {
    sku: String,
    price_cents: i64,
    tags: Option<Vec<String>>,
}

let products: Vec<Product> =
    sqlx::query_as("SELECT sku, price_cents, tags FROM products ORDER BY sku")
        .fetch_all(&pool)
        .await?;
~~~

By hand and with shelf, in one program that checks that both read the same values:

```rust
use tokio_postgres::Row;

trait FromRow: Sized {
    fn from_row(row: &Row) -> Result<Self, tokio_postgres::Error>;
}

// By hand.
#[derive(Debug, PartialEq)]
struct Product {
    sku: String,
    price_cents: i64,
    tags: Option<Vec<String>>,
}

impl FromRow for Product {
    fn from_row(row: &Row) -> Result<Self, tokio_postgres::Error> {
        Ok(Product {
            sku: row.try_get("sku")?,
            price_cents: row.try_get("price_cents")?,
            tags: row.try_get("tags")?,
        })
    }
}

// shelf: the same fields, and one line instead of the impl.
#[derive(Debug, PartialEq, shelf::FromRow)]
struct ShelfProduct {
    sku: String,
    price_cents: i64,
    tags: Option<Vec<String>>,
}

const SQL: &str = "SELECT sku, price_cents, tags FROM products ORDER BY sku";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = shelf_playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE products (sku text PRIMARY KEY, price_cents bigint, tags text[]);
             INSERT INTO products VALUES ('MUG-01', 1299, '{kitchen}'), ('TEE-02', 2499, NULL);",
        )
        .await?;

    let by_hand = client
        .query(SQL, &[])
        .await?
        .iter()
        .map(Product::from_row)
        .collect::<Result<Vec<_>, _>>()?;
    let with_shelf: Vec<ShelfProduct> = shelf::fetch_all(&client, SQL, &[]).await?;

    assert_eq!(by_hand.len(), with_shelf.len());
    for (a, b) in by_hand.iter().zip(&with_shelf) {
        assert_eq!(
            (&a.sku, a.price_cents, &a.tags),
            (&b.sku, b.price_cents, &b.tags)
        );
    }
    assert_eq!(with_shelf[1].tags, None);
    Ok(())
}
```

| | by hand | shelf | sqlx |
|---|---|---|---|
| **Same** | Fields are matched to columns by name; NULL is `Option`; types are checked at run time | The same | The same |
| **Different** | One line per field, in an impl you write and can change. Errors name a column, not the struct or field. Every name is looked up on every row | One line, `#[derive(FromRow)]`. The struct is checked against the statement once, before the first row; errors name the struct and field. Owned values only | The derive is generic over sqlx's databases (`R: Row`). Each name is looked up on every row, in a hash map the statement carries. `query_as!` can check the query at compile time |

---

## In stockroom

stockroom is the hand-written version at full size. Its `Store` has five fields, and the file
names each of them four times just to read it: in the struct, in two `SELECT` lists (one in
`list`, one in `create`'s `RETURNING`), and in `try_from`:

~~~rust
// stockroom/src/repo/stores.rs (excerpt)
pub struct Store {
    pub id: Uuid,
    pub code: String,
    pub name: String,
    pub address: Address,
    pub created_at: Timestamp,
}

impl TryFrom<&Row> for Store {
    type Error = tokio_postgres::Error;

    fn try_from(row: &Row) -> Result<Self, Self::Error> {
        Ok(Store {
            id: row.try_get("id")?,
            code: row.try_get("code")?,
            name: row.try_get("name")?,
            address: row.try_get("address")?, // the composite, through the derive
            created_at: row.try_get("created_at")?,
        })
    }
}

// …
pub async fn list(db: &impl GenericClient) -> Result<Vec<Store>, AppError> {
    let stmt = db
        .prepare_cached("SELECT id, code, name, address, created_at FROM stores ORDER BY code")
        .await?;
    from_rows(&db.query(&stmt, &[]).await?)
}
~~~

[stockroom/src/repo/stores.rs](../../stockroom/src/repo/stores.rs)

`address` is a PostgreSQL composite type, which `Address` maps through postgres-types' derive
([main book Chapter 5](../../book/05-custom-types.md)). That is one column holding a whole
struct, read by `FromSql`, and the opposite of §1.4's nesting, where one struct spans several
columns. shelf keeps both: `FromSql` types such as `Address` are ordinary fields, and
`flatten` handles the other kind.

Chapter 14 rewrites part of stockroom on shelf and counts the lines, saved and added.

## When not to use a mapping trait

- **One value.** `query_one_scalar::<i64>("SELECT count(*) …")` reads it without a struct
  ([main book §2.5](../../book/02-queries.md#25-scalars-query_scalar-query_one_scalar-query_opt_scalar)).
- **A one-off query read in place.** In a migration check or a test, `row.get("total")` is
  clearer than a struct that exists for one line.
- **Hot loops over borrowed data.** Mapping into owned structs allocates a `String` per text
  column per row. When you only need to look at the values, borrow them from the rows with
  `row.get::<_, &str>(…)` and keep the rows alive
  ([main book §3.4](../../book/03-rows-and-types.md#34-borrowing-from-a-row)).

## Summary

- shelf maps rows with a trait of its own, `FromRow`, rather than `TryFrom<&Row>`: it can grow
  associated items, gives generic code a short bound, and can be implemented for tuples, which
  the orphan rule forbids for `TryFrom<&Row>`.
- In 0.7.18, `try_get("name")` walks the columns comparing names (and walks them again, ignoring
  ASCII case, if none matches), then checks `accepts`, then decodes. Twelve fields by name
  compare 78 names per row in any order.
- By name costs about 3× by position in a release build, 1.6× in a debug build, on 100,000 rows
  of 12 columns. Positions break when the `SELECT` changes; the answer is to find the positions
  once per statement.
- A mapping fails on a missing column, a wrong type, or a NULL in a non-`Option` field.
  tokio-postgres's errors name the column but not the struct or field; shelf's name all of them.
- A nested struct can be read from the same row by calling its `from_row`, but its column names
  are then fixed.
- `fetch_all`, `fetch_one` and `fetch_opt` run a query and map it, on anything that implements
  `GenericClient`.
- Each field is named three times by hand, twice as strings nothing checks. A macro can write
  the mapping and its error context; the SQL needs a derive that knows the table.

## Exercises

1. **Errors with context.** Change the trait so that `from_row` returns an error naming the
   struct and the field, as well as tokio-postgres's error and its cause, and implement it for a
   `Product` with three fields.
   *Hint:* a small helper, `get(row, "Product", "price_cents")`, keeps each line short. Count how
   often the struct's name now appears in the impl.
2. **Does the order matter?** Predict how §1.2's timing changes if the `SELECT` lists the
   columns in the struct's order instead of in reverse. Then measure it.
   *Hint:* in each order, at which position does each field's search stop?
3. **A tuple.** Implement `FromRow` for `(A, B)`, reading by position, and read
   `SELECT sku, price_cents FROM products` into a `Vec<(String, i64)>`.
   *Hint:* each element needs `FromSqlOwned`; §1.1 showed why this impl needs your own trait.

### Solutions

**1.** The error type carries the two names as `&'static str`, so building it costs nothing:

```rust
use std::fmt;
use tokio_postgres::Row;
use tokio_postgres::types::FromSqlOwned;

#[derive(Debug)]
struct MappingError {
    model: &'static str,
    field: &'static str,
    source: tokio_postgres::Error,
}

impl fmt::Display for MappingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}: {}", self.model, self.field, self.source)?;
        // The useful part of a tokio-postgres error is often its cause.
        if let Some(cause) = std::error::Error::source(&self.source) {
            write!(f, ": {cause}")?;
        }
        Ok(())
    }
}

impl std::error::Error for MappingError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

trait FromRow: Sized {
    fn from_row(row: &Row) -> Result<Self, MappingError>;
}

fn get<T: FromSqlOwned>(
    row: &Row,
    model: &'static str,
    field: &'static str,
) -> Result<T, MappingError> {
    row.try_get(field).map_err(|source| MappingError {
        model,
        field,
        source,
    })
}

#[derive(Debug)]
#[allow(dead_code)] // the program only shows the error
struct Product {
    sku: String,
    name: String,
    price_cents: i64,
}

impl FromRow for Product {
    fn from_row(row: &Row) -> Result<Self, MappingError> {
        Ok(Product {
            sku: get(row, "Product", "sku")?,
            name: get(row, "Product", "name")?,
            price_cents: get(row, "Product", "price_cents")?,
        })
    }
}

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = shelf_playground::connect().await?;
    let row = client
        .query_one(
            "SELECT 'MUG-01' AS sku, 'Mug' AS name, 1299 AS price_cents",
            &[],
        )
        .await?;
    let error = Product::from_row(&row).unwrap_err();
    println!("{error}");
    assert_eq!(
        error.to_string(),
        "Product.price_cents: error deserializing column 2: \
         cannot convert between the Rust type `i64` and the Postgres type `int4`"
    );
    Ok(())
}
```

```text
Product.price_cents: error deserializing column 2: cannot convert between the Rust type `i64` and the Postgres type `int4`
```

The struct's name is now written once per field, and each field's name is written twice: as
the field and as the string. Both are exactly what a macro knows when it generates the impl.

**2.** Nothing measurable changes. A search that finds its column at position *p* compares
*p* + 1 names, and every column is looked up once per row, so the total is 1 + 2 + … + 12 = 78
comparisons per row for *any* order of the `SELECT` list. Measured in a debug build, three
rounds each: 202.7–206.9 ms in reverse order and 205.2–206.6 ms in the struct's order, against
124–130 ms by position in both. What does grow the cost is the number of columns: twice as many
columns means nearly four times as many comparisons per row (300 against 78).

**3.** The impl is allowed because `FromRow` is defined in this program:

```rust
use tokio_postgres::Row;
use tokio_postgres::types::FromSqlOwned;

trait FromRow: Sized {
    fn from_row(row: &Row) -> Result<Self, tokio_postgres::Error>;
}

impl<A: FromSqlOwned, B: FromSqlOwned> FromRow for (A, B) {
    fn from_row(row: &Row) -> Result<Self, tokio_postgres::Error> {
        Ok((row.try_get(0)?, row.try_get(1)?))
    }
}

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = shelf_playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE products (sku text PRIMARY KEY, price_cents bigint NOT NULL);
             INSERT INTO products VALUES ('MUG-01', 1299), ('TEE-02', 2499);",
        )
        .await?;

    let pairs = client
        .query("SELECT sku, price_cents FROM products ORDER BY sku", &[])
        .await?
        .iter()
        .map(<(String, i64)>::from_row)
        .collect::<Result<Vec<_>, _>>()?;
    assert_eq!(pairs, [("MUG-01".into(), 1299), ("TEE-02".into(), 2499)]);
    Ok(())
}
```

Writing the impl for `(A, B, C)`, then `(A, B, C, D)`, and so on to twelve, is the kind of
repetition the next chapter's macros remove.

---

Previous: [Chapter 0 — Introduction and setup](00-introduction.md) · Next: [Chapter 2 — Declarative macros](02-declarative-macros.md)
