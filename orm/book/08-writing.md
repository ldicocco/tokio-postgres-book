# Chapter 8 — Writing rows: insert, update, delete, upsert

> **Building an ORM on tokio-postgres** · Previous: [Chapter 7 — The executor and shelf's errors](07-executor.md) · Next: [Chapter 9 — Partial updates and optimistic locking](09-patch-and-versions.md)

Reading is half of a model layer. The other half is writing, and the first question is what type
you write. A `Product` read from the database has an `id` and a `created_at`, but a product you
are about to create has neither: the database will assign them. Making those fields `Option`s
would spread `unwrap()`s through every reader of the model. shelf does what stockroom already
does by hand, and gives creation its own struct, `NewProduct`, with only the columns you supply.

`#[derive(Insert)]` on that struct writes the `INSERT`, and `RETURNING` hands back the full
model, defaults filled in, in the same round trip. Updates and deletes go through the model
itself, by its key, and an upsert is one more clause. Each method's SQL is printed, and each one
is run until it fails, so that you know what its failures look like.

**In this chapter you will learn:**

- why creation gets its own struct, and how `#[derive(Insert)]` builds a statement from another
  type's constants
- how `RETURNING` gives back the whole model, with the database's defaults, in one round trip
- `update`, `delete` and `delete_by_key`, and what zero rows affected means
- upserts with `ON CONFLICT`, why shelf has no `DO NOTHING` method, and when `MERGE` is the
  better statement
- what constraint violations from shelf's writes look like, and where they become HTTP statuses

---

## 8.1 `#[derive(Insert)]`

A `New…` struct holds the columns a new row supplies, and names the model the row becomes:

~~~rust
#[derive(Insert)]
#[shelf(model = Product)]
struct NewProduct<'a> {
    sku: &'a str,
    price_cents: i64,
}
~~~

`model = Product` gives the derive two things: the table the row goes into, and the columns
`RETURNING` reads back, so that `insert` returns a `Product`. Both are the model's constants,
`Product::TABLE` and `Product::COLUMNS` from Chapter 6, and here §6.5's problem arrives: the
`Insert` derive sees the tokens `Product`, not the model's fields, so it cannot write the table
or the column list into a literal. It writes what it knows, its own columns and their
placeholders, and joins the model's constants in at compile time with §6.5's `const fn`s. This
program prints the `INSERT` the derive generates, by running shelf-codegen, and then the
statement the compiler made of it:

```rust
use shelf::{FromRow, Insert, Table};
use syn::{DeriveInput, parse_quote};

#[derive(Debug, FromRow, Table)]
#[shelf(table = "products")]
#[allow(dead_code)] // the program only prints the SQL
struct Product {
    #[shelf(primary_key, generated)]
    id: i64,
    sku: String,
    price_cents: i64,
}

#[derive(Insert)]
#[shelf(model = Product)]
#[allow(dead_code)]
struct NewProduct<'a> {
    sku: &'a str,
    price_cents: i64,
}

fn main() {
    // What the derive writes, by running it...
    let input: DeriveInput = parse_quote! {
        #[shelf(model = Product)]
        struct NewProduct<'a> {
            sku: &'a str,
            price_cents: i64,
        }
    };
    let code = shelf_playground::pretty(shelf_codegen::insert::expand(&input).unwrap());
    let start = code.find("    const INSERT").expect("an INSERT constant");
    let end = start + code[start..].find("    };").expect("its end") + "    };".len();
    println!("{}", &code[start..end]);

    // ...and the statement the compiler joined from it.
    println!("{}", NewProduct::INSERT);
    assert_eq!(
        NewProduct::INSERT,
        r#"INSERT INTO "products" ("sku", "price_cents") VALUES ($1, $2) RETURNING "id", "sku", "price_cents""#
    );
}
```

```text
    const INSERT: &'static ::core::primitive::str = {
        const PARTS: &[&::core::primitive::str] = &[
            "INSERT INTO ",
            <Product as ::shelf::Table>::TABLE,
            " (\"sku\", \"price_cents\") VALUES ($1, $2) RETURNING ",
            <Product as ::shelf::Table>::COLUMNS,
        ];
        const BYTES: [::core::primitive::u8; ::shelf::__private::total_len(PARTS)] = ::shelf::__private::join(
            PARTS,
        );
        ::shelf::__private::utf8(&BYTES)
    };
INSERT INTO "products" ("sku", "price_cents") VALUES ($1, $2) RETURNING "id", "sku", "price_cents"
```

The block is §6.5's, generated: the derive's own text (`" (\"sku\", \"price_cents\") VALUES
($1, $2) RETURNING "`) between the model's two constants, joined into one `&'static str`. The
`id` is in the `RETURNING` list and not in the column list, because the model reads it and the
`New…` struct does not supply it. The values are `$1` and `$2`, and `params()` lists the fields
in the same order.

The struct may borrow. `params()` returns references to its fields, `&self.sku` and
`&self.price_cents`, and `insert` takes `&self`, so a `NewProduct<'a>` built from a request's
`&str`s is written without copying them. What it may not have is a type parameter: the
statement is a constant built from the model's constants, and §6.5 showed that such a constant
cannot be written in a generic impl. A lifetime is not a problem, because nothing in the
statement mentions it.

`insert` itself is a provided method of the `Insert` trait, two lines on top of Chapter 5's
`fetch_one`:

~~~rust
// orm/shelf/src/write.rs (excerpt)
pub trait Insert: Sync {
    /// The model the row becomes: its table is where the row goes, and its columns what
    /// `RETURNING` reads back.
    type Model: Table;

    /// `INSERT INTO … (…) VALUES ($1, …) RETURNING …`, the model's columns.
    const INSERT: &'static str;

    /// The columns it writes, quoted, in field order.
    const COLUMNS: &'static [&'static str];

    // … COPY and arrays (Chapter 12)

    /// The fields, as `INSERT`'s parameters, in order.
    fn params(&self) -> Vec<&(dyn ToSql + Sync)>;

    /// Inserts the row and returns it as the model, with every default filled in: one round
    /// trip.
    fn insert(
        &self,
        db: &impl Executor,
    ) -> impl Future<Output = Result<Self::Model, Error>> + Send {
        async move { fetch_one(db, Self::INSERT, &self.params()).await }
    }

    // … insert_many and copy_in (Chapter 12)
}
~~~

[orm/shelf/src/write.rs](../shelf/src/write.rs)

Because it goes through `fetch_one`, the `INSERT` is prepared first, and `Product` is resolved
against its `RETURNING` columns before anything is inserted (Chapter 5): a model that does not
fit its table fails without writing a row. "One round trip" is the cost on a deadpool connection
with the statement cached; on a plain `Client`, the prepare adds one more (Chapter 7).

### What `Insert` refuses

`Insert` is the only shelf derive on its struct, so it reports every mistake in `#[shelf(…)]`
itself, unknown keys included (Chapter 4, §4.2). Its own refusals are pinned by a UI test:

~~~rust
// orm/shelf/tests/ui/insert.rs (excerpt)
#[derive(Insert)]
struct NoModel {
    sku: String,
}

#[derive(Insert)]
#[shelf(model = Product, table = "products")]
struct Misplaced {
    #[shelf(primary_key)]
    id: i64,
    #[shelf(skip, conflict)]
    note: String,
}

#[derive(FromRow)]
struct NotATable {
    sku: String,
}

#[derive(Insert)]
#[shelf(model = NotATable)]
struct NewThing {
    sku: String,
}
~~~

[orm/shelf/tests/ui/insert.rs](../shelf/tests/ui/insert.rs)

```text
error: Insert needs the model the row becomes: #[shelf(model = Product)]
 --> tests/ui/insert.rs:6:8
  |
6 | struct NoModel {
  |        ^^^^^^^

error: `table` belongs on the model: an Insert struct writes to its model's table
  --> tests/ui/insert.rs:19:26
   |
19 | #[shelf(model = Product, table = "products")]
   |                          ^^^^^

error: `primary_key` has no meaning on an Insert struct, which writes each field to its column; it belongs on the model
  --> tests/ui/insert.rs:21:13
   |
21 |     #[shelf(primary_key)]
   |             ^^^^^^^^^^^

error: `conflict` cannot apply to a field marked `skip`, which is not written
  --> tests/ui/insert.rs:23:19
   |
23 |     #[shelf(skip, conflict)]
   |                   ^^^^^^^^

error[E0277]: `NotATable` is not a shelf model
  --> tests/ui/insert.rs:33:17
   |
33 | #[shelf(model = NotATable)]
   |                 ^^^^^^^^^ not a `Table`
   |
help: the trait `Table` is not implemented for `NotATable`
  --> tests/ui/insert.rs:28:1
   |
28 | struct NotATable {
   | ^^^^^^^^^^^^^^^^
   = note: derive it: `#[derive(FromRow, Table)]`, with `#[shelf(table = "…")]` on the struct and `#[shelf(primary_key)]` on its key
help: the trait `Table` is implemented for `Product`
  --> tests/ui/insert.rs:10:19
   |
10 | #[derive(FromRow, Table)]
   |                   ^^^^^
note: required by a bound in `shelf::Insert::Model`
  --> src/write.rs
   |
   |     type Model: Table;
   |                 ^^^^^ required by this bound in `Insert::Model`
   = note: this error originates in the derive macro `Table` (in Nightly builds, run with -Z macro-backtrace for more info)
```

The first four come from the derive. The last one cannot: whether `NotATable` is a model is a
fact about another item, which the derive never sees.

> **Macro lens:** the derive emits `type Model = NotATable;`, and the trait declares
> `type Model: Table;`, so the compiler checks the bound when it type-checks the impl, long after
> the macro has returned. The error lands on `NotATable` inside the attribute because the derive
> builds that line from the attribute's own tokens, which carry their spans into the output. The
> wording is shelf's: `Table` has a `#[diagnostic::on_unimplemented]` with a message, a label and
> a note, as Chapter 4 gave `FromColumn`. (The build reports it twice, the second time for the
> `<NotATable as Table>::TABLE` the joined statement uses; the UI test's `.stderr` has both.)

## 8.2 Database defaults

The columns a `New…` struct leaves out are filled by their defaults. Three kinds are common, and
the model marks each one `generated`:

| Column | Default | What fills it |
|---|---|---|
| `id uuid DEFAULT uuidv7()` | a version-7 UUID, which sorts by creation time | `uuidv7()`, PG 18+ ([main book §3.9](../../book/03-rows-and-types.md#39-exact-numbers-uuids)) |
| `id bigint GENERATED ALWAYS AS IDENTITY` | the next value of the column's sequence | the identity |
| `created_at timestamptz DEFAULT now()` | the transaction's start time | `now()` |

`generated` tells Chapter 6's `update` not to write the column; the `Insert` struct leaves it
out simply by not having the field. `RETURNING` reads the values back with the rest of the row:

```rust
use jiff::Timestamp;
use shelf::{FromRow, Insert, Table};
use uuid::Uuid;

#[derive(Debug, FromRow, Table)]
#[shelf(table = "products")]
struct Product {
    #[shelf(primary_key, generated)]
    id: Uuid,
    sku: String,
    price_cents: i64,
    #[shelf(generated)]
    created_at: Timestamp,
}

#[derive(Insert)]
#[shelf(model = Product)]
struct NewProduct<'a> {
    sku: &'a str,
    price_cents: i64,
}

#[tokio::main]
async fn main() -> Result<(), shelf::Error> {
    let client = shelf_playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE products (
                 id uuid PRIMARY KEY DEFAULT uuidv7(),
                 sku text NOT NULL UNIQUE,
                 price_cents bigint NOT NULL,
                 created_at timestamptz NOT NULL DEFAULT now()
             );",
        )
        .await?;

    let before = Timestamp::now();
    let mug = NewProduct {
        sku: "MUG-01",
        price_cents: 1299,
    }
    .insert(&client)
    .await?;
    let tee = NewProduct {
        sku: "TEE-02",
        price_cents: 2499,
    }
    .insert(&client)
    .await?;

    // The database's values, read back by the same statement.
    assert_eq!(mug.id.get_version_num(), 7);
    assert!(tee.id > mug.id, "version-7 UUIDs sort by creation time");
    assert!(mug.created_at >= before - jiff::SignedDuration::from_secs(5));
    assert_eq!(Product::find(&client, &mug.id).await?.sku, "MUG-01");
    println!(
        "{} {} {}",
        mug.sku,
        mug.price_cents,
        mug.id.get_version_num()
    );
    Ok(())
}
```

```text
MUG-01 1299 7
```

The `Insert` derive does not know which of the model's columns are generated (that is in the
model's attributes, another item again). So a `New…` struct that names one is not refused at
compile time; the database refuses it, if the column says so. An identity declared `GENERATED
ALWAYS` does:

```rust
use shelf::{FromRow, Insert, Table};
use tokio_postgres::error::SqlState;

#[derive(Debug, FromRow, Table)]
#[shelf(table = "products")]
#[allow(dead_code)] // the program only shows the error
struct Product {
    #[shelf(primary_key, generated)]
    id: i64,
    sku: String,
}

/// A mistake: `id` belongs to the database.
#[derive(Insert)]
#[shelf(model = Product)]
struct NewProduct {
    id: i64,
    sku: String,
}

#[tokio::main]
async fn main() -> Result<(), shelf::Error> {
    let client = shelf_playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE products (
                 id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
                 sku text NOT NULL
             );",
        )
        .await?;

    let error = NewProduct {
        id: 42,
        sku: "MUG-01".into(),
    }
    .insert(&client)
    .await
    .unwrap_err();
    let shelf::Error::Db(error) = error else {
        panic!("a database error");
    };
    let server = error.as_db_error().expect("from the server");
    println!("{}", server.message());
    println!("{}", server.detail().unwrap_or_default());
    println!("{}", server.hint().unwrap_or_default());
    assert_eq!(server.code(), &SqlState::GENERATED_ALWAYS);

    // The server refuses the statement as it parses it: preparing alone fails.
    let error = client.prepare(NewProduct::INSERT).await.unwrap_err();
    assert_eq!(error.code(), Some(&SqlState::GENERATED_ALWAYS));
    Ok(())
}
```

```text
cannot insert a non-DEFAULT value into column "id"
Column "id" is an identity column defined as GENERATED ALWAYS.
Use OVERRIDING SYSTEM VALUE to override.
```

The server refuses the statement as it parses it, so the error comes from the *Parse* that
`prepare` sends, and nothing runs. A column
declared `GENERATED BY DEFAULT AS IDENTITY`, or with a plain `DEFAULT`, would accept the value
and skip the default, which is usually a bug that nothing reports; a model's `generated` columns
deserve `GENERATED ALWAYS` where PostgreSQL offers it.

> **Pitfall:** a parameter that goes into a domain column is typed as the domain, in an `INSERT`'s
> `VALUES` and in an `UPDATE`'s `SET` alike, and a `String` refuses it on the client before
> anything is sent ([main book §5.5](../../book/05-custom-types.md#55-domains)). A `New…`
> struct for a table with a `sku` domain needs a field type that accepts the domain: a derived
> domain newtype, or a type with a hand-written `accepts`, as stockroom's `Sku` has. The model
> can read the column into a `String`, because a domain result column is described as its base
> type. This program shows both halves.

```rust
use postgres_types::{FromSql, ToSql};
use shelf::{FromRow, Insert, Table};

/// The `sku` domain, as a derived newtype.
#[derive(Debug, Clone, ToSql, FromSql)]
#[postgres(name = "sku")]
struct Sku(String);

#[derive(Debug, FromRow, Table)]
#[shelf(table = "products", schema = "shelf_ch08_domain")]
#[allow(dead_code)]
struct Product {
    #[shelf(primary_key, generated)]
    id: i64,
    sku: String, // read back as text: a domain result column is its base type
}

#[derive(Insert)]
#[shelf(model = Product)]
struct NewProduct {
    sku: String,
}

#[derive(Insert)]
#[shelf(model = Product)]
struct NewProductSku {
    sku: Sku,
}

#[tokio::main]
async fn main() -> Result<(), shelf::Error> {
    let client = shelf_playground::connect().await?;
    client
        .batch_execute(
            "DROP SCHEMA IF EXISTS shelf_ch08_domain CASCADE;
             CREATE SCHEMA shelf_ch08_domain;
             CREATE DOMAIN shelf_ch08_domain.sku AS text
                 CHECK (VALUE ~ '^[A-Z0-9]{3,12}(-[A-Z0-9]{1,6})?$');
             CREATE TABLE shelf_ch08_domain.products (
                 id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
                 sku shelf_ch08_domain.sku NOT NULL
             );",
        )
        .await?;

    // A String for a domain parameter: refused by the client, nothing inserted.
    let error = NewProduct {
        sku: "MUG-01".into(),
    }
    .insert(&client)
    .await
    .unwrap_err();
    println!("{error}");
    let cause = std::error::Error::source(&error).expect("the conversion's error");
    println!("  caused by: {cause}");
    assert!(matches!(&error, shelf::Error::Db(e) if e.as_db_error().is_none()));

    // The domain's own type is accepted, and the model reads the value as text.
    let mug = NewProductSku {
        sku: Sku("MUG-01".into()),
    }
    .insert(&client)
    .await?;
    assert_eq!(mug.sku, "MUG-01");

    client
        .batch_execute("DROP SCHEMA shelf_ch08_domain CASCADE")
        .await?;
    Ok(())
}
```

```text
error serializing parameter 0
  caused by: cannot convert between the Rust type `alloc::string::String` and the Postgres type `shelf_ch08_domain.sku`
```

## 8.3 Update and delete

Updates and deletes need a row that exists, so they go through the model, by its key. Chapter
6's derive writes two more statements for them:

| Method | Statement | Zero rows means |
|---|---|---|
| `product.update(&db)` | `UPDATE` every column that is neither key nor `generated`, `WHERE` the key, `RETURNING` the model's columns | `Error::NotFound` |
| `product.delete(&db)` | `DELETE … WHERE` the key | `Error::NotFound` |
| `Product::delete_by_key(&db, &key)` | the same, with a key you have | `Error::NotFound` |

`update` writes the value you hold over the row and returns the row as it now is, which matters
when the database changes something itself: a trigger, a `version` that goes up by one, an
`updated_at` (Chapter 9). Zero rows affected has one meaning for all three: no row has the key,
because someone deleted it, or because it never existed. shelf reports that as `NotFound`
rather than as success, since a write that wrote nothing is not what the caller asked for:

```rust
use shelf::{FromRow, Insert, Table};

#[derive(Debug, Clone, FromRow, Table)]
#[shelf(table = "products")]
struct Product {
    #[shelf(primary_key, generated)]
    id: i64,
    sku: String,
    price_cents: i64,
}

#[derive(Insert)]
#[shelf(model = Product)]
struct NewProduct<'a> {
    sku: &'a str,
    price_cents: i64,
}

/// Where a row's current version lives in the table, as text.
async fn ctid(client: &tokio_postgres::Client, id: i64) -> Result<String, shelf::Error> {
    let row = client
        .query_one("SELECT ctid::text FROM products WHERE id = $1", &[&id])
        .await?;
    Ok(row.try_get(0)?)
}

#[tokio::main]
async fn main() -> Result<(), shelf::Error> {
    let client = shelf_playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE products (
                 id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
                 sku text NOT NULL UNIQUE,
                 price_cents bigint NOT NULL
             );",
        )
        .await?;
    println!("{}", Product::UPDATE);
    println!("{}", Product::DELETE);

    let mut mug = NewProduct {
        sku: "MUG-01",
        price_cents: 1299,
    }
    .insert(&client)
    .await?;
    mug.price_cents = 999;
    let updated = mug.update(&client).await?;
    assert_eq!(Product::find(&client, &mug.id).await?.price_cents, 999);

    // An update that changes nothing still writes a new version of the row.
    let before = ctid(&client, mug.id).await?;
    updated.update(&client).await?;
    let after = ctid(&client, mug.id).await?;
    println!("ctid {before} -> {after}");
    assert_ne!(before, after);

    // Gone: every write by its key is NotFound.
    mug.delete(&client).await?;
    for error in [
        mug.update(&client).await.unwrap_err(),
        mug.delete(&client).await.unwrap_err(),
        Product::delete_by_key(&client, &mug.id).await.unwrap_err(),
    ] {
        assert!(matches!(error, shelf::Error::NotFound { model: "Product" }));
    }
    Ok(())
}
```

```text
UPDATE "products" SET "sku" = $1, "price_cents" = $2 WHERE "id" = $3 RETURNING "id", "sku", "price_cents"
DELETE FROM "products" WHERE "id" = $1
ctid (0,2) -> (0,3)
```

> **SQL lens:** PostgreSQL never changes a row in place. An `UPDATE` writes a new version of the
> row and marks the old one dead, to be cleaned up by `VACUUM`, whatever the new version holds;
> `ctid`, the row version's physical address, moved above although no value changed
> ([Routine Vacuuming](https://www.postgresql.org/docs/18/routine-vacuuming.html)). So writing
> every column costs what writing one costs, as far as the table is concerned: the whole row is
> rewritten either way. Indexes are another matter: a *HOT* update, which needs no new index
> entries, is possible only when no indexed column changes value and the new version fits on
> the same page ([Heap-Only Tuples](https://www.postgresql.org/docs/18/storage-hot.html)). The real cost of
> writing every column is elsewhere: it overwrites what someone else changed since you read the
> row (Exercise 2), which is Chapter 9's subject.

`update`'s statement for a model whose every column is key or generated would have nothing to
`SET`. The derive then uses `FIND` as its `UPDATE`, so `update` returns the row as it is, or
`NotFound`, and never sends invalid SQL.

## 8.4 Upsert

An upsert inserts a row, or updates the one already there when the new row collides with a
unique key. PostgreSQL's statement for it is `INSERT … ON CONFLICT (…) DO UPDATE SET …`, where
`excluded` names the row that could not be inserted
([main book §2.6](../../book/02-queries.md#26-returning-changing-and-reading-in-one-statement)).
In shelf, the fields marked `#[shelf(conflict)]` name the key, and the derive then also
implements `Upsert`, with one more statement:

```rust
use shelf::{FromRow, Insert, Table, Upsert};
use tokio_postgres::error::SqlState;

#[derive(Debug, FromRow, Table)]
#[shelf(table = "products")]
struct Product {
    #[shelf(primary_key, generated)]
    id: i64,
    sku: String,
    price_cents: i64,
}

/// A price list's line: insert the SKU, or reprice it.
#[derive(Insert)]
#[shelf(model = Product)]
struct PriceLine<'a> {
    #[shelf(conflict)]
    sku: &'a str,
    price_cents: i64,
}

/// A mistake: no unique key is on `price_cents` alone.
#[derive(Insert)]
#[shelf(model = Product)]
struct ByPrice<'a> {
    sku: &'a str,
    #[shelf(conflict)]
    price_cents: i64,
}

#[tokio::main]
async fn main() -> Result<(), shelf::Error> {
    let client = shelf_playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE products (
                 id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
                 sku text NOT NULL CONSTRAINT products_sku_key UNIQUE,
                 price_cents bigint NOT NULL
             );",
        )
        .await?;
    println!("{}", PriceLine::UPSERT);

    let first = PriceLine {
        sku: "MUG-01",
        price_cents: 1299,
    }
    .upsert(&client)
    .await?;
    let again = PriceLine {
        sku: "MUG-01",
        price_cents: 999,
    }
    .upsert(&client)
    .await?;
    assert_eq!(
        (again.id, again.price_cents),
        (first.id, 999),
        "the same row, repriced"
    );
    assert_eq!(Product::count(&client).await?, 1);

    // The upsert that updated still drew an identity value: the next row skips it.
    let tee = PriceLine {
        sku: "TEE-02",
        price_cents: 2499,
    }
    .upsert(&client)
    .await?;
    println!("ids: {}, {}", first.id, tee.id);
    assert_eq!(tee.id, first.id + 2);

    // A conflict target that matches no unique index: preparing it succeeds...
    client.prepare(ByPrice::UPSERT).await?;
    // ...and running it fails, when PostgreSQL plans it.
    let error = ByPrice {
        sku: "VASE-03",
        price_cents: 3999,
    }
    .upsert(&client)
    .await
    .unwrap_err();
    let shelf::Error::Db(error) = &error else {
        panic!("a database error: {error}");
    };
    println!(
        "{}",
        error.as_db_error().expect("from the server").message()
    );
    assert_eq!(error.code(), Some(&SqlState::INVALID_COLUMN_REFERENCE));
    Ok(())
}
```

```text
INSERT INTO "products" ("sku", "price_cents") VALUES ($1, $2) ON CONFLICT ("sku") DO UPDATE SET "price_cents" = excluded."price_cents" RETURNING "id", "sku", "price_cents"
ids: 1, 3
there is no unique or exclusion constraint matching the ON CONFLICT specification
```

`SET` names every column of the struct except the conflict key, each set to its `excluded`
value. The target must match a unique index or constraint, column for column, and `ByPrice`
shows what happens when it does not: SQLSTATE 42P10, but only when the statement runs.
PostgreSQL looks for the index that arbitrates the conflict when it *plans* the statement, and
a *Parse* only parses and analyses it, so preparing the `UPSERT` succeeds and Chapter 5's check
cannot catch this one. (The `GENERATED ALWAYS` mistake of §8.2 is caught earlier, while the
statement is parsed and rewritten, which is why `prepare` failed there.) And the gap in the ids is the
price of upserting into a table with an identity or a sequence: the row's defaults are computed
before PostgreSQL knows whether it will insert, and a sequence value, once drawn, is never given
back.

### Why no `DO NOTHING`

The other form, `ON CONFLICT DO NOTHING`, skips the row on a conflict. With `RETURNING`, it then
returns no row at all:

```text
 id | sku | price_cents 
----+-----+-------------
(0 rows)

INSERT 0 0
```

(That is psql 18 running `INSERT … VALUES ('MUG-01', 999) ON CONFLICT DO NOTHING RETURNING id,
sku, price_cents` when `MUG-01` already exists.) A method for it would return
`Option<Product>`, and `None` would say only that some row was in the way, not which one, or
what it holds; a caller that needs the existing row must read it with a second query, and
between the two another transaction can change or delete it. shelf's derive does something else
when the `Insert` struct has no column besides its conflict key: it writes `DO UPDATE SET "sku"
= excluded."sku"`, an update that changes nothing, so that the existing row is still returned,
in the same statement. (It is still an update: it takes the row's lock and writes a new
version.) A `DO NOTHING` method is easy to add on top of the public constants, which Exercise 1
does; shelf leaves it out because what it returns is rarely what the caller wanted.

### `MERGE`

`MERGE` (PG 15+) is the SQL standard's statement for the same job, with more to say: it can
update, insert or delete depending on conditions, and since PostgreSQL 17 `RETURNING
merge_action()` reports which happened to each row. The main book uses it to apply a price list
loaded with COPY ([main book §10.6](../../book/10-copy.md#106-copy-into-a-temp-table-then-upsert)).
For one row from a request, `ON CONFLICT` is the better statement, and the PostgreSQL
documentation says why: when `MERGE` runs concurrently with other changes the usual isolation
rules apply, so two `MERGE`s that both decide "not matched" can both insert and one fails with a
unique violation, while `INSERT … ON CONFLICT` "offers the ability to run an `UPDATE` if a
concurrent `INSERT` occurs" ([MERGE](https://www.postgresql.org/docs/18/sql-merge.html)).
Exercise 3 writes the upsert as a `MERGE`.

## 8.5 Constraint errors

A write that breaks a constraint fails on the server, after the statement has been prepared and
the struct checked, and shelf passes the error on as `Error::Db`. Everything an application needs
to report it is inside: the SQLSTATE says which kind of rule, and the constraint's name says
which rule ([main book §6.3](../../book/06-errors.md#63-sqlstate-the-codes-worth-matching)):

```rust
use shelf::{FromRow, Insert, Table};

#[derive(Debug, FromRow, Table)]
#[shelf(table = "products")]
#[allow(dead_code)]
struct Product {
    #[shelf(primary_key, generated)]
    id: i64,
    sku: String,
    price_cents: i64,
    store_id: Option<i64>,
}

#[derive(Insert)]
#[shelf(model = Product)]
struct NewProduct<'a> {
    sku: &'a str,
    price_cents: i64,
    store_id: Option<i64>,
}

#[tokio::main]
async fn main() -> Result<(), shelf::Error> {
    let client = shelf_playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE stores (id bigint PRIMARY KEY);
             CREATE TEMP TABLE products (
                 id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
                 sku text NOT NULL CONSTRAINT products_sku_key UNIQUE,
                 price_cents bigint NOT NULL CONSTRAINT products_price_positive CHECK (price_cents > 0),
                 store_id bigint CONSTRAINT products_store_fkey REFERENCES stores
             );",
        )
        .await?;
    let new = |sku, price_cents, store_id| NewProduct {
        sku,
        price_cents,
        store_id,
    };
    new("MUG-01", 1299, None).insert(&client).await?;

    for row in [
        new("MUG-01", 999, None),      // a second MUG-01
        new("TEE-02", 0, None),        // a price that is not positive
        new("VASE-03", 3999, Some(7)), // a store that does not exist
    ] {
        let error = row.insert(&client).await.unwrap_err();
        let shelf::Error::Db(error) = &error else {
            panic!("a database error: {error}");
        };
        let server = error.as_db_error().expect("from the server");
        println!(
            "{} {:<24} {}",
            server.code().code(),
            server.constraint().unwrap_or("-"),
            server.message()
        );
    }
    assert_eq!(Product::count(&client).await?, 1);
    Ok(())
}
```

```text
23505 products_sku_key         duplicate key value violates unique constraint "products_sku_key"
23514 products_price_positive  new row for relation "products" violates check constraint "products_price_positive"
23503 products_store_fkey      insert or update on table "products" violates foreign key constraint "products_store_fkey"
```

Turning these into statuses is the application's job, keyed by constraint name, as Chapter 7's
`AppError` did: 23505 on `products_sku_key` is a 409, 23514 and 23503 are 422s, each with a
message the API's client can act on. A constraint renamed in a migration silently stops matching
that table, which is why stockroom treats constraint names as a contract (its
`error::constraint_message`), and why naming constraints yourself, as these do, beats the names
PostgreSQL makes up.

One more consequence follows from the error being the server's. Inside a transaction, a failed
statement aborts the transaction: every statement after it fails with 25P02 until a rollback
([main book §6.6](../../book/06-errors.md#66-an-error-inside-a-transaction)). A `NotFound` or a
`Stale` from shelf is different: shelf decides it from the rows that came back, nothing failed on
the server, and the transaction can go on.

### Side by side — *inserting a row and reading it back*

The task: create a store from a request and return it, with the id and timestamp the database
assigned.

~~~rust
// SeaORM (illustration)
let store = store::ActiveModel {
    code: Set("ber-01".to_owned()),
    name: Set("Berlin Mitte".to_owned()),
    ..Default::default() // id and created_at: NotSet, so the database's defaults
};
// INSERT INTO "stores" ("code", "name") VALUES ($1, $2) RETURNING "id", "code", "name", "created_at"
let store: store::Model = store.insert(&db).await?;
~~~

By hand, as stockroom's `stores::create` writes it, and with shelf, in one program that checks
both return the same kind of row:

```rust
use jiff::Timestamp;
use shelf::{FromRow, Insert, Table};
use uuid::Uuid;

#[derive(Debug, FromRow, Table)]
#[shelf(table = "stores")]
struct Store {
    #[shelf(primary_key, generated)]
    id: Uuid,
    code: String,
    name: String,
    #[shelf(generated)]
    created_at: Timestamp,
}

#[derive(Insert)]
#[shelf(model = Store)]
struct NewStore<'a> {
    code: &'a str,
    name: &'a str,
}

/// By hand: the statement and its column list written out, the row read with FromRow.
async fn create_by_hand(
    db: &tokio_postgres::Client,
    new: &NewStore<'_>,
) -> Result<Store, shelf::Error> {
    let statement = db
        .prepare(
            "INSERT INTO stores (code, name) VALUES ($1, $2)
             RETURNING id, code, name, created_at",
        )
        .await?;
    let row = db.query_one(&statement, &[&new.code, &new.name]).await?;
    Store::from_row(&row)
}

#[tokio::main]
async fn main() -> Result<(), shelf::Error> {
    let client = shelf_playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE stores (
                 id uuid PRIMARY KEY DEFAULT uuidv7(),
                 code text NOT NULL UNIQUE,
                 name text NOT NULL,
                 created_at timestamptz NOT NULL DEFAULT now()
             );",
        )
        .await?;

    let by_hand = create_by_hand(
        &client,
        &NewStore {
            code: "ber-01",
            name: "Berlin Mitte",
        },
    )
    .await?;
    let with_shelf = NewStore {
        code: "ham-01",
        name: "Hamburg Altona",
    }
    .insert(&client)
    .await?;

    for store in [&by_hand, &with_shelf] {
        assert_eq!(store.id.get_version_num(), 7);
        assert!(store.created_at <= Timestamp::now());
    }
    assert_eq!(Store::find(&client, &by_hand.id).await?.code, "ber-01");
    assert_eq!(
        Store::find(&client, &with_shelf.id).await?.name,
        "Hamburg Altona"
    );
    Ok(())
}
```

| | by hand | shelf | SeaORM |
|---|---|---|---|
| **Same** | A struct of the columns supplied; one `INSERT … RETURNING` that writes them and reads the whole row back, defaults included | The same | The same |
| **Different** | You write the statement, its column list and its `RETURNING` list, and keep all three in step with two structs | A `New…` struct with `#[derive(Insert)]` names its model; the statement is a constant joined at compile time from both, and the model is checked against its `RETURNING` columns before the row is inserted | The same `ActiveModel` type serves insert and update: each field is `Set`, `NotSet` or `Unchanged`, and only `Set` fields are written; `insert` runs the model's `before_save` and `after_save` hooks around the statement, which sea-query builds at run time |

---

## In shelf

The upsert's statement is built beside the `INSERT`, from the same pieces, when any field is
marked `conflict`:

~~~rust
// orm/shelf-codegen/src/insert.rs (excerpt)
    let conflict: Vec<String> = columns
        .iter()
        .filter(|c| c.conflict)
        .map(|c| c.name.clone())
        .collect();
    if !conflict.is_empty() {
        // Every other column takes the new value. With none, the conflict key is set to itself,
        // so that the existing row is still returned (`DO NOTHING` would return no row).
        let mut updated: Vec<&String> = names.iter().filter(|n| !conflict.contains(n)).collect();
        if updated.is_empty() {
            updated = conflict.iter().take(1).collect();
        }
        let set: Vec<String> = updated
            .iter()
            .map(|n| format!("{0} = excluded.{0}", quote_ident(n)))
            .collect();
        let on_conflict = format!(
            " ON CONFLICT ({}) DO UPDATE SET {} RETURNING ",
            column_list(&conflict),
            set.join(", ")
        );
        let middle = format!("{values}{on_conflict}");
        let upsert = joined(&[quote!("INSERT INTO "), table, quote!(#middle), returning]);
        out.extend(quote! {
            #[automatically_derived]
            impl #impl_generics ::shelf::Upsert for #name #ty_generics #where_clause {
                const UPSERT: &'static ::core::primitive::str = #upsert;
            }
        });
    }
~~~

[orm/shelf-codegen/src/insert.rs](../shelf-codegen/src/insert.rs)

`joined` is the block §8.1 printed, wrapped in a function of the derive. `Upsert` is a separate
trait, a subtrait of `Insert`, so that a struct without a conflict key has no `upsert` method to
call by mistake: the compiler, not a run-time error, says it cannot upsert.

`update` is a provided method of `Table`. It puts the key's parameters after the values it sets,
in the order of the `$n` in `UPDATE`, and hands the statement to `written`, which for a model
without a version is `fetch_opt` with `NotFound` for no row:

~~~rust
// orm/shelf/src/table.rs (excerpt)
    fn update(&self, db: &impl Executor) -> impl Future<Output = Result<Self, Error>> + Send {
        async move {
            let key = self.key();
            let mut params = self.update_params();
            params.extend(Self::key_params(&key));
            written(db, Self::UPDATE, &params).await
        }
    }
~~~

[orm/shelf/src/table.rs](../shelf/src/table.rs)

The rest of `written`, for a model with a `version`, is Chapter 9's.

## When not to use shelf's writes

- **A guarded update.** stockroom's reservation, `UPDATE stock SET reserved = reserved + $3
  WHERE store_id = $1 AND product_id = $2 AND available >= $3`, is correct because of its
  `WHERE`: the condition and
  the change happen in one statement, under the row's lock. Read, check in Rust, then `update`
  would let two orders sell the last chair. Write it by hand (main book
  [§8.9](../../book/08-transactions.md#89-selling-the-last-chair-four-ways)).
- **Counters and expressions.** `SET stock = stock - 1` computes from the row's current value;
  `update` writes the value you hold, which may be stale.
- **Bulk loads.** One `insert` per row costs a round trip per row; Chapter 12's `insert_many`
  and `copy_in` cost one per batch.
- **Several outcomes per row.** Inserting, updating and deleting from one source by conditions
  is what `MERGE` is for.

## Summary

- Creation gets its own struct. `#[derive(Insert)]` with `#[shelf(model = Product)]` writes
  `INSERT INTO … (its columns) VALUES ($1, …) RETURNING (the model's columns)`, joining the
  model's `TABLE` and `COLUMNS` in at compile time. The struct may borrow; it may not have type
  parameters.
- `insert` returns the model with every default filled in, in one round trip on a cached
  connection, and checks the model against `RETURNING` before inserting.
- `generated` marks the columns the database fills (`uuidv7()`, PG 18+; identities; `now()`).
  The `Insert` derive cannot see them: a `New…` struct that names one is refused by the database,
  if the column is `GENERATED ALWAYS`.
- A domain column's parameter is typed as the domain, so a `String` is refused on the client;
  write it with a type that accepts the domain.
- `update` writes every column that is neither key nor generated and returns the row; `delete`
  and `delete_by_key` delete by key. Zero rows is `Error::NotFound` for all three.
- `#[shelf(conflict)]` fields make an `Upsert`: `ON CONFLICT (…) DO UPDATE SET` every other
  column, returning the row whether it was inserted or updated. An upsert that updates still
  draws an identity value.
- shelf has no `DO NOTHING` method: on a conflict it returns no row, and the caller is left with
  a second query and a race. `MERGE … RETURNING merge_action()` (PG 17+) suits batches with
  several outcomes; `ON CONFLICT` suits one row under concurrency.
- A constraint violation is `Error::Db` with the SQLSTATE and the constraint's name; mapping it to
  a status is the application's job, by name.

## Exercises

1. **`insert_if_absent`.** Write `insert_if_absent(&db, &row) -> Result<Option<T::Model>, _>`
   for any `T: Insert`, with `ON CONFLICT DO NOTHING`. What does it return for a row that
   collides?
   *Hint:* `Insert::COLUMNS`, `Table::TABLE` and `Table::COLUMNS` are public; `fetch_opt`
   reads zero or one row.
2. **Two tabs.** Read the same product twice, as two admins' browser tabs would. Change the price
   in one copy and the name in the other, and `update` both. What is in the table?
   *Hint:* `update` writes every column it is given.
3. **Upsert with `MERGE`.** Write the price-list upsert of §8.4 as a `MERGE` that returns the row
   and `merge_action()`, and read both into one struct.
   *Hint:* `USING (VALUES ($1::text, $2::bigint)) AS new (sku, price_cents)`; the casts give the
   parameters their types. A `FromRow` struct can `flatten` a model.

### Solutions

**1.** The statement is assembled at run time from constants, which are quoted identifiers, never
values:

```rust
use shelf::{Executor, FromRow, Insert, Table};

#[derive(Debug, FromRow, Table)]
#[shelf(table = "products")]
struct Product {
    #[shelf(primary_key, generated)]
    id: i64,
    sku: String,
    price_cents: i64,
}

#[derive(Insert)]
#[shelf(model = Product)]
struct NewProduct<'a> {
    sku: &'a str,
    price_cents: i64,
}

/// Inserts `row` unless it collides with a unique key; then nothing is written, and nothing
/// comes back.
async fn insert_if_absent<T: Insert>(
    db: &impl Executor,
    row: &T,
) -> Result<Option<T::Model>, shelf::Error> {
    let placeholders: Vec<String> = (1..=T::COLUMNS.len()).map(|n| format!("${n}")).collect();
    let sql = format!(
        "INSERT INTO {} ({}) VALUES ({}) ON CONFLICT DO NOTHING RETURNING {}",
        <T::Model as Table>::TABLE,
        T::COLUMNS.join(", "),
        placeholders.join(", "),
        <T::Model as Table>::COLUMNS
    );
    shelf::fetch_opt(db, &sql, &row.params()).await
}

#[tokio::main]
async fn main() -> Result<(), shelf::Error> {
    let client = shelf_playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE products (
                 id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
                 sku text NOT NULL UNIQUE,
                 price_cents bigint NOT NULL
             );",
        )
        .await?;
    let mug = |price_cents| NewProduct {
        sku: "MUG-01",
        price_cents,
    };

    let first = insert_if_absent(&client, &mug(1299)).await?;
    assert_eq!(first.map(|p| p.price_cents), Some(1299));
    // The same SKU again: no error, no row, and the table unchanged.
    assert!(insert_if_absent(&client, &mug(999)).await?.is_none());
    assert_eq!(Product::find(&client, &1).await?.price_cents, 1299);
    Ok(())
}
```

`None` means "a row was in the way", without saying which row or which unique constraint: a
table with two unique keys gives the same `None` for both. To return the existing row, read it
after the `None`, by the key you meant, and accept that it may have changed in between; or use
`upsert`, whose `RETURNING` reads it under the row's lock.

**2.** The second `update` writes back the price its copy still holds, so the first change is
lost:

```rust
use shelf::{FromRow, Table};

#[derive(Debug, Clone, FromRow, Table)]
#[shelf(table = "products")]
struct Product {
    #[shelf(primary_key)]
    id: i64,
    name: String,
    price_cents: i64,
}

#[tokio::main]
async fn main() -> Result<(), shelf::Error> {
    let client = shelf_playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE products (
                 id bigint PRIMARY KEY, name text NOT NULL, price_cents bigint NOT NULL
             );
             INSERT INTO products VALUES (1, 'Mug', 1299);",
        )
        .await?;

    let mut tab_a = Product::find(&client, &1).await?;
    let mut tab_b = Product::find(&client, &1).await?;
    tab_a.price_cents = 999;
    tab_a.update(&client).await?;
    tab_b.name = "Blue mug".into();
    tab_b.update(&client).await?;

    let now = Product::find(&client, &1).await?;
    assert_eq!(now.name, "Blue mug");
    assert_eq!(now.price_cents, 1299, "tab A's price is lost");
    Ok(())
}
```

Nothing failed, and nothing reported the loss. Two things would have prevented it, and Chapter
9 builds both: writing only the columns that changed (a patch), and refusing a write made against
an old version of the row (a version column).

**3.** A `FromRow` struct reads `merge_action()` and the model, flattened, from one row:

```rust
use shelf::{FromRow, Table};

#[derive(Debug, FromRow, Table)]
#[shelf(table = "products")]
struct Product {
    #[shelf(primary_key, generated)]
    id: i64,
    sku: String,
    price_cents: i64,
}

/// One row of the MERGE's RETURNING: what it did, and the row it did it to.
#[derive(Debug, FromRow)]
struct Merged {
    action: String,
    #[shelf(flatten)]
    product: Product,
}

const MERGE: &str = "MERGE INTO products AS p
    USING (VALUES ($1::text, $2::bigint)) AS new (sku, price_cents) ON p.sku = new.sku
    WHEN MATCHED THEN UPDATE SET price_cents = new.price_cents
    WHEN NOT MATCHED THEN INSERT (sku, price_cents) VALUES (new.sku, new.price_cents)
    RETURNING merge_action() AS action, p.id, p.sku, p.price_cents";

#[tokio::main]
async fn main() -> Result<(), shelf::Error> {
    let client = shelf_playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE products (
                 id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
                 sku text NOT NULL UNIQUE,
                 price_cents bigint NOT NULL
             );",
        )
        .await?;

    let first: Merged = shelf::fetch_one(&client, MERGE, &[&"MUG-01", &1299_i64]).await?;
    let again: Merged = shelf::fetch_one(&client, MERGE, &[&"MUG-01", &999_i64]).await?;
    println!("{} {:?}", first.action, first.product);
    println!("{} {:?}", again.action, again.product);
    assert_eq!(
        (first.action.as_str(), again.action.as_str()),
        ("INSERT", "UPDATE")
    );
    assert_eq!(again.product.id, first.product.id);
    Ok(())
}
```

```text
INSERT Product { id: 1, sku: "MUG-01", price_cents: 1299 }
UPDATE Product { id: 1, sku: "MUG-01", price_cents: 999 }
```

`MERGE` says what it did, which `ON CONFLICT` does not, and it can do more (a `WHEN MATCHED AND
… THEN DELETE`, or no action at all for an unchanged price). It needs casts where `ON CONFLICT`
needed none, because a `VALUES` list in `USING` has no column to take its types from. And under
concurrency it is the weaker statement: two sessions that both find `MUG-01` missing both take
the `NOT MATCHED` branch, and the second insert fails with 23505, where `ON CONFLICT` would have
turned it into an update.

---

Previous: [Chapter 7 — The executor and shelf's errors](07-executor.md) · Next: [Chapter 9 — Partial updates and optimistic locking](09-patch-and-versions.md)
