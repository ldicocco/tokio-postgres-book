# Chapter 0 — Introduction and setup

> **Building an ORM on tokio-postgres** · Next: [Chapter 1 — Mapping rows by hand](01-mapping-by-hand.md)

[Async PostgreSQL in Rust](../../book/00-introduction.md) taught tokio-postgres as a driver: you
wrote every SQL statement yourself, and you turned every row into a struct with a
`TryFrom<&Row>` that named each column again
([main book §3.5](../../book/03-rows-and-types.md#35-rows-into-structs)). This book builds the
layer that writes that code for you. **shelf** is a small ORM on tokio-postgres: derive macros
that map rows to structs, a model layer that writes the everyday SQL, partial updates with
optimistic locking, a typed query builder, relations without N+1 queries, bulk writes, streams
and a schema check. You build it one chapter at a time, and every stage of it is in this
repository, compiled and tested.

The goal is not to replace diesel or SeaORM. It is to know what an ORM does underneath, by
building one on a driver you already understand: what a derive macro receives and returns, how
it reports a mistake at the right line, what SQL a model method sends and how many round trips
it costs, and where hand-written SQL is still the better tool.

## Contents

| Chapter | You will learn |
|---|---|
| **0 · Introduction and setup** (this page) | What an ORM does, where shelf sits, the three crates, how the book follows the build, and the setup |
| [1 · Mapping rows by hand](01-mapping-by-hand.md) | A `FromRow` trait, reading by name or by position and what each costs, NULL, nested structs, a fetch helper |
| [2 · Declarative macros](02-declarative-macros.md) | `macro_rules!`, a `model!` macro, `FromRow` for twelve tuple sizes from one macro, hygiene, and where it stops |
| [3 · The first derive macro](03-first-derive.md) | Proc-macro crates, syn and quote, the three-crate split, hygiene, seeing the expansion, errors that point at your code |
| [4 · Attributes, errors and generics](04-attributes.md) | `#[shelf(…)]`: `rename`, `skip`, `flatten`, `json`, `try_from`; several errors at once, UI tests, generics |
| [5 · Checking a statement once](05-column-indices.md) | Resolving columns and checking their types once per statement instead of on every row, measured |
| [6 · Tables](06-tables.md) | `#[derive(Table)]`, primary keys, SQL written at compile time, `find` and `all` |
| [7 · The executor and shelf's errors](07-executor.md) | One trait over `Client`, `Transaction` and deadpool, statement strategies measured, `shelf::Error` |
| [8 · Writing rows](08-writing.md) | `#[derive(Insert)]`, database defaults, `RETURNING`, updates, deletes and upserts |
| [9 · Partial updates and optimistic locking](09-patch-and-versions.md) | `#[derive(Patch)]` three ways, `Option<Option<T>>`, version columns, `updated_at` |
| [10 · A typed query builder](10-query-builder.md) | Typed columns, expressions, `$n` numbering, keyset pages, and what fails to compile |
| [11 · Relations without N+1](11-relations.md) | `belongs_to` and `has_many`, the N+1 problem measured, `= ANY($1)`, pipelined loads, joins |
| [12 · Bulk writes and streams](12-bulk-and-streams.md) | `insert_many` through `unnest`, binary COPY written by the derive, streams of models |
| [13 · Trusting the mapping](13-trusting-the-mapping.md) | Checking models against the live schema, testing macros, and what compile-time checks add |
| [14 · Capstone](14-capstone.md) | Part of stockroom rewritten on shelf behind the same routes, compared line for line |
| [A · Appendix](15-appendix.md) | An attribute reference, what each derive generates, a phrasebook, a glossary |

The whole book is also a single PDF, [shelf-book.pdf](shelf-book.pdf).

Part I (Chapters 1–5) is the mapping: from a hand-written trait, through `macro_rules!`, to a
derive that checks a whole statement before it reads the first row. Part II (6–9) is the model:
tables, writing rows, partial updates. Part III (10–14) is queries, relations, bulk work, and
the capstone. Read them in order: each chapter grows the crate the previous one left.

---

## 0.1 What an ORM does, and what shelf will not do

An object-relational mapper does three jobs, and the main book did all three by hand:

1. **It maps rows to structs**, and structs back to parameters. That is the `TryFrom<&Row>`
   stockroom has for every type it reads, and the `&[&(dyn ToSql + Sync)]` list it builds for
   every `INSERT`.
2. **It writes the everyday SQL**: `SELECT … WHERE id = $1`, `INSERT … RETURNING`,
   `UPDATE … SET`, `DELETE`. Their shape follows from the struct and the table, and nobody
   enjoys writing them for the twentieth type.
3. **It follows relations** between tables: an order's lines, a line's product. Done naively,
   that is one query per row, the N+1 problem. Done well, it is one query per relation.

shelf does all three, in that order: Part I maps rows, Part II writes the everyday SQL, and
Part III follows relations. It also leaves things out, each for a reason:

| Left out | Why |
|---|---|
| Migrations, and DDL generated from structs | A schema is better written as SQL, in migrations ([main book Chapter 14](../../book/14-data-layer.md)) |
| Checking queries against a live database at compile time | sqlx's `query!` and Cornucopia do it; Chapter 13 says what it adds and what it costs |
| Lazy loading, an identity map, change tracking, a unit of work | Each hides round trips or state; Chapter 11 says why async Rust makes lazy loading a poor fit |
| Other databases, and a blocking API | shelf is an ORM for tokio-postgres; abstracting over drivers would hide what the driver does |
| Postgres enums, composites and domains | postgres-types' derives already map them ([main book Chapter 5](../../book/05-custom-types.md)), and shelf passes them through |
| Hooks such as `before_insert` | A plain function call before the write does the same, and you can see it |

One thread runs through the whole book: **every shelf method says what SQL it sends and how
many round trips it costs**, and every chapter ends by saying when not to use what it built. An
ORM you cannot predict is an ORM you end up fighting. Building one is the surest way never to
be surprised by the one you use.

## 0.2 Where shelf sits

Rust already has libraries that do some of these jobs. They differ less in features than in two
decisions: who writes the SQL, and when a mistake in the mapping is caught.

| Library | Built on | You write | A mapping mistake is caught |
|---|---|---|---|
| diesel, with diesel-async | tokio-postgres (diesel-async's PostgreSQL connection) | Rust: a schema (`table!`, usually generated from the database) and queries in its builder | At compile time, against the `table!` schema |
| SeaORM | sqlx | Entities (the `DeriveEntityModel` derive), `ActiveModel`s for writes, queries in its builder | At run time |
| sqlx | its own driver | SQL; `#[derive(FromRow)]` maps rows | At compile time with `query!` and `query_as!` (against a live database, or query data saved from one); at run time with `FromRow`, on every row |
| postgres-from-row | tokio-postgres | SQL; `#[derive(FromRow)]` maps rows | At run time, on every row |
| Cornucopia | tokio-postgres | SQL in `.sql` files, from which Rust functions are generated | When the code is generated, against a live database |
| **shelf** | tokio-postgres | SQL where it matters; derives write the everyday statements | At run time, once per statement, before the first row is read; and against the table at startup, with `verify` |

A newer entrant, **toasty**, is the Tokio project's ORM: models are structs with
`#[derive(Model)]`, behind one API for SQL databases and DynamoDB, and its PostgreSQL driver
holds a `tokio_postgres::Client`. It is young (0.11, with breaking changes in each release). The
main book's [Appendix F](../../book/19-appendix.md#f-the-crates-built-on-tokio-postgres) has
the versions, dates and details of every crate in this section that is built on tokio-postgres.
The illustrations in this book follow sqlx 0.9, SeaORM 2.0, diesel 2.3 and darling 0.24.

What shelf decides differently, and why it is worth building rather than adopting:

- **The SQL stays visible.** A derive writes statements you can predict, and each chapter
  prints the ones it generates. Anything else is your SQL, mapped by the same derive. There is
  no second query language to learn for the hard cases.
- **The mapping is checked at run time, but once.** shelf compares the struct with the
  statement's column list when the statement is prepared, so a renamed column or a wrong type is
  an error that names the struct, the field, the column and both types, even for a query that
  returns no rows (Chapter 5). Checking at compile time catches more, and costs a database at
  build time; Chapter 13 weighs the two.
- **tokio-postgres is underneath, with nothing hidden.** shelf takes the `Client` and
  `Transaction` you already have (and deadpool's connections, Chapter 7). Pipelining, COPY,
  `LISTEN` and everything else in the main book still work on the same connection.

## 0.3 The finished API in ten lines

By the end of Part III, a model and its everyday SQL look like this. Chapters 6–13 build each
piece, and this program runs them all, against temporary tables of its own. The ten lines that
matter are the calls in `main` after the setup, and each one that talks to the server has a
comment saying what it sends:

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
    tags: Vec<String>,
    #[shelf(version)]
    version: i32,
    #[shelf(generated)]
    created_at: Timestamp,
}

#[derive(Insert)]
#[shelf(model = Product)]
struct NewProduct {
    sku: String,
    price_cents: i64,
    tags: Vec<String>,
}

#[derive(Debug, FromRow, Table)]
#[shelf(table = "orders", has_many(Line, key = order_id, as = lines))]
struct Order {
    #[shelf(primary_key)]
    id: i64,
}

#[derive(Debug, FromRow, Table)]
#[shelf(table = "lines")]
struct Line {
    #[shelf(primary_key, generated)]
    id: i64,
    order_id: i64,
    sku: String,
}

fn new_product(sku: &str, price_cents: i64, tags: &[&str]) -> NewProduct {
    NewProduct {
        sku: sku.into(),
        price_cents,
        tags: tags.iter().map(|t| t.to_string()).collect(),
    }
}

#[tokio::main]
async fn main() -> Result<(), shelf::Error> {
    let db = shelf_playground::connect().await?;
    db.batch_execute(
        "CREATE TEMP TABLE products (
             id uuid PRIMARY KEY DEFAULT uuidv7(),
             sku text NOT NULL UNIQUE,
             price_cents bigint NOT NULL,
             tags text[] NOT NULL DEFAULT '{}',
             version integer NOT NULL DEFAULT 1,
             created_at timestamptz NOT NULL DEFAULT now()
         );
         CREATE TEMP TABLE orders (id bigint PRIMARY KEY);
         CREATE TEMP TABLE lines (
             id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
             order_id bigint NOT NULL REFERENCES orders,
             sku text NOT NULL
         );
         INSERT INTO orders VALUES (1), (2);
         INSERT INTO lines (order_id, sku) VALUES (1, 'MUG-01'), (1, 'TEE-02'), (2, 'MUG-01');",
    )
    .await?;
    let new_mug = new_product("MUG-01", 1299, &["kitchen", "sale"]);
    let batch = [
        new_product("TEE-02", 2499, &["sale"]),
        new_product("VASE-03", 899, &["sale"]),
        new_product("LAMP-04", 4999, &["home"]),
    ];

    let mug = new_mug.insert(&db).await?; // INSERT … RETURNING
    let mug = Product::find(&db, &mug.id).await?; // SELECT … WHERE "id" = $1, or Error::NotFound
    NewProduct::copy_in(&db, &batch).await?; // the whole batch in one binary COPY
    let query = Product::query()
        .filter(
            Product::PRICE_CENTS
                .gt(1_000)
                .and(Product::TAGS.contains("sale")),
        )
        .order_by(Product::SKU.asc())
        .limit(20);
    let page = query.all(&db).await?; // one SELECT, three parameters
    let orders = Order::all(&db).await?; // SELECT … ORDER BY "id"
    let lines = Order::load_lines(&db, &orders).await?; // one query for every order: = ANY($1)
    shelf::verify::<Product>(&db).await?; // the struct against the table's columns

    println!("{}", query.sql());
    let skus: Vec<&str> = page.iter().map(|p| p.sku.as_str()).collect();
    let per_order: Vec<usize> = lines.iter().map(Vec::len).collect();
    println!("page: {skus:?}; lines per order: {per_order:?}");
    assert_eq!((mug.sku.as_str(), mug.version), ("MUG-01", 1));
    assert!(mug.created_at <= Timestamp::now());
    assert_eq!(skus, ["MUG-01", "TEE-02"]);
    assert_eq!(per_order, [2, 1]);
    assert_eq!(lines[0][1].sku, "TEE-02");
    assert!(
        lines
            .iter()
            .flatten()
            .all(|line| line.id > 0 && line.order_id > 0)
    );
    Ok(())
}
```

```text
SELECT "id", "sku", "price_cents", "tags", "version", "created_at" FROM "products" WHERE "price_cents" > $1 AND "tags" @> $2 ORDER BY "sku" LIMIT $3
page: ["MUG-01", "TEE-02"]; lines per order: [2, 1]
```

Each call says in its comment what it sends:

- `insert` sends one `INSERT … RETURNING` with the model's columns, and the database fills in
  `id` (with `uuidv7()`, PG 18+), `version` and `created_at` (Chapter 8).
- `find` sends `SELECT <every column> FROM products WHERE id = $1`, and turns "no row" into an
  error you can match, `Error::NotFound` (Chapter 6).
- `copy_in` writes the whole batch with one binary `COPY`. Its sibling `insert_many` writes a
  batch with one `INSERT … SELECT unnest(…), …`, for structs without array columns such as
  `tags` (Chapter 12).
- The query builder renders one `SELECT` with `$1`, `$2` and `$3` as parameters, printed
  above. Its columns are typed, so comparing `PRICE_CENTS` with a string does not compile
  (Chapter 10).
- `load_lines` reads the lines of every order in the slice with one query, not one per order
  (Chapter 11).
- `verify` compares the struct with the table, so drift is caught at startup instead of at the
  first request that reads the column (Chapter 13).

On a plain `Client`, as here, shelf prepares each statement before it runs it: that is the round
trip `query` makes with SQL text anyway, and the moment shelf checks the struct against the
statement's columns (Chapter 5). On a deadpool connection, the statement comes from the
connection's cache after the first call (Chapter 7).

## 0.4 Three crates, and why

A derive macro is a function that the compiler calls with the tokens of your struct, and that
returns new tokens. Rust requires that function to live in a crate of its own kind, a
*proc-macro crate*, which can export macros and nothing else. That is inconvenient: code that
generates code is code you want to test and to show, and nothing outside the compiler can call
a function in a proc-macro crate, neither a test in another crate nor an example in this book.

So shelf is three crates, and the code that writes code lives in the one that is an ordinary
library:

```text
 your crate ──depends on──▶ shelf ──re-exports the derives of──▶ shelf-macros
                              │                                (proc-macro crate)
                              │                                       │ calls
   traits, fetch helpers,  ◀──┘                                       ▼
   errors: what your code                tests, the book's  ──▶  shelf-codegen
   and the generated code call           examples                (a plain library)
```

| Crate | Kind | What it holds |
|---|---|---|
| [`shelf`](../shelf/src/lib.rs) | library | What you depend on: the traits and functions your code calls, the error type, and the derives, re-exported. A hidden module, `__private`, holds what generated code names, so your crate does not need its own dependency on tokio-postgres |
| [`shelf-macros`](../shelf-macros/src/lib.rs) | proc-macro | A shell: per derive, three lines that parse the input, call shelf-codegen, and return the result or the error |
| [`shelf-codegen`](../shelf-codegen/src/lib.rs) | library | Everything the derives generate, as functions from a parsed struct to tokens. Its own tests, and this book's examples, call them directly |

Two more crates sit beside them: `orm/playground`, which holds this book's examples, and
`orm/stockroom-shelf`, Chapter 14's capstone, which serves part of stockroom's API on shelf.
Chapter 3 builds the split from the inside, and explains the one type that makes it possible,
`proc_macro2::TokenStream`.

## 0.5 How this book follows the build

A build-along book has a problem: each chapter shows the crate at one stage, but the repository
holds the finished crate. If the book quoted only the finished code, Chapter 3 would show
attribute parsing that Chapter 4 explains, and checks that Chapter 5 motivates. This book
handles it in four ways:

1. **Stage programs.** Each new mechanism appears first as a complete, runnable program in one
   file: a hand-written impl, a `macro_rules!` macro, or a derive's logic written as an ordinary
   function from a parsed struct to tokens. A proc macro cannot be defined in a single file, but
   its logic can, because syn and quote work in any program. So even the macro code in this book
   is compiled, run and checked.
2. **The finished crate, in excerpts.** Then the chapter shows how shelf does it, in excerpts
   whose first line names the file. A tool checks every excerpt against the file it quotes, and
   code that later chapters add is elided with `// …`.
3. **Behaviour, through the finished crate.** Claims about what shelf does are runnable programs
   that use `#[derive(shelf::FromRow)]` and friends against a real database.
4. **Nothing generated is typed by hand.** What a derive expands to is printed by running
   shelf-codegen, and compiler errors are pasted from a real build, as the main book pasted psql
   output.

Each chapter then follows the main book's pattern. **Side by side** sections solve one task
three ways: **by hand** with tokio-postgres, as the main book would; with **shelf**; and with
an established crate where it fits the topic (sqlx for row mapping, SeaORM for models and
relations, diesel for the query builder). The first two are runnable programs; the third is an
illustration, not compiled, and its first line names the crate. A table of what is the **same**
and what is **different** follows. **In shelf** sections show the finished crate, an **In
stockroom** section appears where stockroom's hand-written code is the comparison, and a
section on **when not to** use what the chapter built comes before the summary and the
exercises.

Five kinds of boxes appear where they are earned. The first four are the main book's:

> **Protocol lens:** what travels over the socket.

> **SQL lens:** what the server does with it.

> **Rust refresher:** a language feature you may be hazy on, where it first matters:
> `PhantomData`, async methods in traits, `LazyLock`.

> **Pitfall:** a mistake that compiles, passes a quick test, and fails in production.

The fifth is new:

> **Macro lens:** what the compiler hands a macro and what it takes back: tokens, spans,
> hygiene, and the order in which things expand.

Code blocks come in the main book's two kinds. A plain `rust` block is a complete program that
was compiled, linted and run against a fresh database, and its `assert!`s say what it produces.
A block whose first line names a file, a crate or "illustration" is an excerpt or a sketch, and
is not compiled on its own.

## 0.6 Setup

### What you need

The main book's setup, from its [Chapter 0](../../book/00-introduction.md#setting-up):
PostgreSQL 18, a database for the examples named by `DATABASE_URL`, and Rust 1.89 or newer
(the 2024 edition, let chains, and the locked dependencies: uuid 1.29 declares 1.89). This book
was written with rustc 1.99.1.

shelf is not published on crates.io. It lives in this repository, next to the main book's
workspace, and so do its examples. Work from a clone of the repository; every command below
runs from its root.

### The playground

The book's examples run in the crate `orm/playground`, whose package is `shelf-playground`. It
depends on `shelf` and `shelf-codegen` by path, on the macro crates syn, quote, proc-macro2 and
prettyplease, and on what the main book's playground has (tokio, tokio-postgres with its
features, serde, jiff, uuid, deadpool-postgres). Unlike the main book's playground, it takes
every version from the workspace's `Cargo.toml`, so there is nothing to copy.

Its library re-exports the main book's helpers, so an example connects with
`shelf_playground::connect()`, which reads `DATABASE_URL` exactly as the main book's
`connect()` does. It adds one helper of its own, `pretty`, which formats generated code;
Chapter 3 prints it.

Each runnable block in a chapter becomes one file in `orm/playground/examples/`, named after
the chapter and its position: the first program in this chapter, §0.3's, is `shelf_ch00_01`.
To run one:

```bash
python3 tools/book.py --book orm extract                 # write orm/playground/examples/ from the book
export DATABASE_URL=postgres://postgres:secret@localhost:5432/tpbook
cargo run -p shelf-playground --example shelf_ch00_01
```

> **Pitfall:** `orm/playground/examples/` is generated, and `extract` (like `check`) deletes
> every `.rs` file there that the book did not produce. Keep your own programs in
> `orm/playground/src/bin/` instead: `orm/playground/src/bin/mine.rs` runs with
> `cargo run -p shelf-playground --bin mine`.

To check a chapter the way it was checked when it was written, let `book.py` build it, lint it
with clippy, and run every program against a database it creates for the purpose,
`tpbook_shelf_check`. It needs `ADMIN_DATABASE_URL`, a connection with the right to create
databases, which the repository's `.env.example` describes:

```bash
python3 tools/book.py --book orm check --chapter 0       # one chapter's examples
python3 tools/book.py --book orm check                   # the whole book
```

### shelf's own tests

shelf is tested apart from the book: tests that pin down what each derive expands to, tests that
pin down the compiler errors it causes, a hygiene test, and integration tests that give each test
its own schema in a database named `shelf_test`, besides unit tests. Chapter 13 explains the
four kinds.

```bash
cargo test -p shelf -p shelf-codegen                     # needs ADMIN_DATABASE_URL
```

## 0.7 The first derive, end to end

Here is shelf as it stands at the end of Part I. `#[derive(FromRow)]` reads each named field
from the column of the same name. This program derives it for a struct and reads two rows into
it with `shelf::fetch_all`:

```rust
use shelf::FromRow;

#[derive(Debug, FromRow)]
struct Product {
    id: i64,
    sku: String,
    price_cents: i64,
    tags: Option<Vec<String>>,
}

#[tokio::main]
async fn main() -> Result<(), shelf::Error> {
    let client = shelf_playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE products (
                 id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
                 sku text NOT NULL UNIQUE,
                 price_cents bigint NOT NULL,
                 tags text[]
             );
             INSERT INTO products (sku, price_cents, tags)
             VALUES ('MUG-01', 1299, '{kitchen}'), ('TEE-02', 2499, NULL);",
        )
        .await?;

    let products: Vec<Product> = shelf::fetch_all(
        &client,
        "SELECT id, sku, price_cents, tags FROM products ORDER BY id",
        &[],
    )
    .await?;

    println!("{products:#?}");
    assert_eq!(products.len(), 2);
    assert!(products[0].id < products[1].id);
    assert_eq!(products[0].sku, "MUG-01");
    assert_eq!(products[0].price_cents, 1299);
    assert_eq!(products[0].tags, Some(vec!["kitchen".to_string()]));
    assert_eq!(products[1].tags, None);
    Ok(())
}
```

```text
[
    Product {
        id: 1,
        sku: "MUG-01",
        price_cents: 1299,
        tags: Some(
            [
                "kitchen",
            ],
        ),
    },
    Product {
        id: 2,
        sku: "TEE-02",
        price_cents: 2499,
        tags: None,
    },
]
```

`use shelf::FromRow` brings in two things with one name: the trait and the derive macro. That
works because traits and macros live in different namespaces, and it is how serde's `Serialize`
works too. The NULL in `tags` became `None` because the field is an `Option`, exactly as with
`row.try_get` in the main book.

What did the derive write? Because the code generation lives in `shelf-codegen`, an ordinary
library (§0.4), this program can call it on a struct and print the result:

```rust
use syn::{DeriveInput, parse_quote};

fn main() {
    let input: DeriveInput = parse_quote! {
        struct Product {
            id: i64,
            sku: String,
        }
    };
    let generated = shelf_codegen::from_row::expand(&input).expect("a struct with named fields");
    let code = shelf_playground::pretty(generated);
    println!("{code}");
    assert!(code.contains(r#"column::<String>(columns, "Product", "sku", "sku")?"#));
}
```

It prints the `impl` that the compiler sees next to the struct in place of
`#[derive(FromRow)]`:

```text
#[automatically_derived]
impl ::shelf::FromRow for Product {
    type Columns = (usize, usize);
    fn resolve(
        columns: &[::shelf::__private::tokio_postgres::Column],
    ) -> ::core::result::Result<Self::Columns, ::shelf::Error> {
        ::core::result::Result::Ok((
            ::shelf::__private::column::<i64>(columns, "Product", "id", "id")?,
            ::shelf::__private::column::<String>(columns, "Product", "sku", "sku")?,
        ))
    }
    fn from_row_at(
        row: &::shelf::__private::tokio_postgres::Row,
        columns: &Self::Columns,
    ) -> ::core::result::Result<Self, ::shelf::Error> {
        ::core::result::Result::Ok(Self {
            id: ::shelf::__private::get::<i64>(row, columns.0, "Product", "id")?,
            sku: ::shelf::__private::get::<String>(row, columns.1, "Product", "sku")?,
        })
    }
}
```

Three things in it are worth noticing now, and each gets a chapter:

- The field names appear as strings, `"id"` and `"sku"`: the derive turned each field's name
  into the column it reads, the job you did by hand with `row.try_get("sku")`. Chapter 1 writes
  that by hand once more, and Chapters 2 and 3 make a macro write it.
- Every path is absolute: `::core::result::Result`, never `Result`. The `impl` lands in your
  module, where `Result` may mean something else. Chapter 3 calls this hygiene.
- There are two steps. `resolve` finds and checks each field's column once per statement, and
  `from_row_at` reads each row by position. Chapter 5 measures why.

## Summary

- An ORM maps rows to structs, writes the everyday SQL, and follows relations. shelf does all
  three, and leaves out migrations, compile-time query checking, lazy loading, other databases
  and hooks.
- Libraries differ in who writes the SQL and when a mapping mistake is caught. shelf keeps the
  SQL visible and checks the mapping at run time, once per statement, before the first row.
- shelf is three crates: `shelf` (what you use), `shelf-macros` (a thin proc-macro shell) and
  `shelf-codegen` (the code generation, as a plain library that tests and the book call).
- Each mechanism appears first as a stage program, then as excerpts of the finished crate;
  behaviour is shown through the real derives, and expansions and compiler errors are captured,
  never typed.
- The book's examples run in `orm/playground`; `book.py --book orm extract` writes them and
  `check` runs them. Keep your own programs in `orm/playground/src/bin/`.

## Exercises

1. **A missing column.** Add a field `weight_grams: i32` to the first program's `Product`, which
   the table does not have, and run it. What does the error name, and does the query run at all?
   *Hint:* it names the struct, the field and the column. Try it with `WHERE false` added to the
   query, which returns no rows.
2. **A renamed column.** Rename the table's `sku` column to `sku_code`, and keep the program
   working without changing the struct.
   *Hint:* the derive reads the column names the *query* produces, and `AS` renames a column in
   the result. (Chapter 4's `#[shelf(rename = "…")]` is the other way.)
3. **A keyword as a field name.** Add a field `r#type: String` to the struct in the second
   program and print the expansion again. Which column does it read?
   *Hint:* `type` is a keyword, so the field needs the raw-identifier prefix `r#`; the column
   does not.

### Solutions

**1.** The error names all three: the struct, the field, and the column it looked for. It is
reported before any row is read, because `fetch_all` checks the struct against the statement's
column list when it prepares the statement, so it fails the same way when the query returns no
rows. Chapter 5 shows how.

```rust
use shelf::FromRow;

#[derive(Debug, FromRow)]
#[allow(dead_code)] // the program only shows the error
struct Product {
    id: i64,
    sku: String,
    weight_grams: i32,
}

#[tokio::main]
async fn main() -> Result<(), shelf::Error> {
    let client = shelf_playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE products (id bigint PRIMARY KEY, sku text NOT NULL);
             INSERT INTO products VALUES (1, 'MUG-01');",
        )
        .await?;

    for sql in [
        "SELECT id, sku FROM products",
        "SELECT id, sku FROM products WHERE false",
    ] {
        let error = shelf::fetch_all::<Product>(&client, sql, &[])
            .await
            .unwrap_err();
        println!("{error}");
        assert_eq!(
            error.to_string(),
            r#"Product.weight_grams: the result has no column "weight_grams""#
        );
    }
    Ok(())
}
```

```text
Product.weight_grams: the result has no column "weight_grams"
Product.weight_grams: the result has no column "weight_grams"
```

**2.** Rename the column back in the query, `SELECT id, sku_code AS sku, …`. The derive never
sees the table, only the columns of the result:

```rust
use shelf::FromRow;

#[derive(Debug, FromRow)]
struct Product {
    id: i64,
    sku: String,
}

#[tokio::main]
async fn main() -> Result<(), shelf::Error> {
    let client = shelf_playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE products (id bigint PRIMARY KEY, sku_code text NOT NULL);
             INSERT INTO products VALUES (1, 'MUG-01');",
        )
        .await?;

    let products: Vec<Product> = shelf::fetch_all(
        &client,
        "SELECT id, sku_code AS sku FROM products ORDER BY id",
        &[],
    )
    .await?;
    assert_eq!(products[0].id, 1);
    assert_eq!(products[0].sku, "MUG-01");
    Ok(())
}
```

**3.** It reads the column `type`. The `r#` prefix is how Rust spells a keyword used as an
identifier, and the derive strips it before it uses the name as a column (Chapter 3, §3.5):

```rust
use syn::{DeriveInput, parse_quote};

fn main() {
    let input: DeriveInput = parse_quote! {
        struct Product {
            id: i64,
            r#type: String,
        }
    };
    let code = shelf_playground::pretty(shelf_codegen::from_row::expand(&input).unwrap());
    assert!(code.contains(r#"column::<String>(columns, "Product", "type", "type")?"#));
    assert!(code.contains("r#type: ::shelf::__private::get::<String>"));
}
```

---

Next: [Chapter 1 — Mapping rows by hand](01-mapping-by-hand.md)
