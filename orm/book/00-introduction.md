# Chapter 0 — Introduction and setup

> **Building an ORM on tokio-postgres** · Next: [Chapter 1 — Mapping rows by hand](01-mapping-by-hand.md)

> **Sketch, 7 November 2026.** A plan of this chapter, not its text; it will be replaced by
> the chapter. Part I, first chapter. Builds on the whole of the main book, and on its
> [Chapter 0](../../book/00-introduction.md) (the setup) and Appendix F (the crates built on
> tokio-postgres). §0.7 is already written, and its two programs run in `book.py --book orm
> check`. shelf: `#[derive(FromRow)]` and `fetch_all` (built, S1a); the API of §0.3 (mostly
> proposed). Crates: tokio-postgres 0.7.18, syn 3.0.9, prettyplease 0.3.0. Decisions: PLAN.md
> §10 (the name, three crates, the scope). Plan: [orm/PLAN.md](../PLAN.md) §1, §3, §4, §7.

[Async PostgreSQL in Rust](../../book/00-introduction.md) taught tokio-postgres as a driver:
you wrote every SQL statement yourself, and turned every row into a struct with a
`TryFrom<&Row>` that named each column again ([Chapter 3](../../book/03-rows-and-types.md)).
This book builds the layer that writes that code for you. **shelf** is a small ORM: derive
macros that map rows to structs, a model layer that writes the everyday SQL, and a typed query
builder. You build it one chapter at a time, and each chapter's code is in this repository,
compiled and tested.

The goal is not to replace diesel or SeaORM. It is to know what an ORM does underneath, by
building one on a driver you already understand: what a derive macro receives and returns, how
it reports a mistake at the right line, what SQL a model method sends and how many round trips
it costs, and where hand-written SQL is still the better tool.

**In this chapter you will learn:**

- what an ORM does for you, and the jobs shelf deliberately leaves to SQL
- where shelf sits among Rust's database libraries, and what each of them decides differently
- why shelf is three crates, and what each one holds
- how this book follows the build: stage programs, excerpts, and expansions printed by running
  the code generator
- how to set up the playground and run a chapter's examples
- what `#[derive(FromRow)]` writes, by running it

---

## 0.1 What an ORM does, and what shelf will not do

Plan:
- Three jobs: mapping rows to structs and back; writing the everyday SQL (find, insert,
  update, delete) from a struct; following relations between tables without one query per
  row.
- What shelf leaves out on purpose (PLAN.md §4.6), each with its reason: migrations (main book
  [Chapter 14](../../book/14-data-layer.md)), checking queries against a live database at
  compile time, lazy loading and change tracking, several databases, a blocking API, and
  Postgres enums and composites (postgres-types' derives already map them).
- The thread through the book: every shelf method says what SQL it sends and how many round
  trips it costs, and every chapter ends with when not to use it.

## 0.2 Where shelf sits

Plan:
- One table, five columns at most: diesel and diesel-async (a schema DSL, compile-time
  checked), SeaORM (entities and `ActiveModel`, on sqlx), toasty (from the Tokio project,
  young), sqlx's `FromRow` (mapping only), postgres-from-row (a `FromRow` derive for
  tokio-postgres), Cornucopia (SQL first, Rust generated). The main book's
  [Appendix](../../book/19-appendix.md) F has the research; versions and dates from there,
  rechecked when the chapter is written (check).
- What shelf decides differently: runtime-checked mapping with one check per statement, SQL
  that a reader can predict, tokio-postgres underneath with nothing hidden.

## 0.3 The finished API in ten lines

Plan:
- A model with `#[derive(FromRow, Table)]`, a `New…` struct with `#[derive(Insert)]`, then
  insert, find, a filtered query and a batched relation load. Written as a runnable block once
  phase S1c exists; until then PLAN.md §4.4 shows its target shape.

## 0.4 Three crates, and why

Draft prose:

A derive macro is a function that the compiler calls with the tokens of your struct and that
returns new tokens. Rust requires that function to live in a crate of its own kind, a
*proc-macro crate*, which can export macros and nothing else. That is inconvenient: code that
generates code is code you want to test and to show, and a proc-macro crate cannot be called by
a test in another crate or by an example in this book.

So shelf is three crates. `shelf-codegen` is an ordinary library that holds everything the
derives generate, as functions from a parsed struct to tokens. `shelf-macros` is the
proc-macro crate, and is three lines per derive: parse, call `shelf-codegen`, return the result
or the error. `shelf` is what you depend on: the traits and functions your code calls, and the
derives, re-exported.

## 0.5 How this book follows the build

Plan:
- The problem: each chapter shows shelf at one stage, but the repository holds the finished
  crate (PLAN.md §3).
- Stage programs: each new mechanism first as a runnable single-file program, including the
  macro code, because syn and quote run in any program.
- Excerpts of the finished crate, kept true by `book.py excerpts`; what later chapters add is
  elided with `// …`.
- Nothing generated is typed by hand: expansions come from running shelf-codegen, compiler
  errors from the UI tests' `.stderr` files.
- The callouts: the main book's four, plus **Macro lens**.

## 0.6 Setup

Plan:
- The main book's setup ([Chapter 0](../../book/00-introduction.md)): PostgreSQL 18, `.env`
  with `ADMIN_DATABASE_URL`.
- The shelf playground, `orm/playground`: what it depends on, its `pretty` helper, and how a
  chapter's examples are generated and run (`python3 tools/book.py --book orm check
  --chapter N`).
- shelf's own tests (`cargo test -p shelf -p shelf-codegen`) and the database `shelf_test`.

## 0.7 The first derive, end to end

`#[derive(FromRow)]` reads each named field from the column of the same name. This program
derives it for a struct and reads two rows into it with `shelf::fetch_all`:

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

`use shelf::FromRow` brings in two things with one name, the trait and the derive macro, because
traits and macros live in different namespaces (serde's `Serialize` works the same way).

What did the derive write? A proc-macro crate can export only macros, so shelf keeps the code
generation in an ordinary library, `shelf-codegen`, and this program can call it on a struct
and print the result:

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

It prints the `impl` the compiler sees in place of `#[derive(FromRow)]`. `resolve` finds and
checks each field's column once per statement; `from_row_at` then reads each row by position
(Chapter 5 explains why):

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

## The programs

- Both programs in §0.7 exist and run (`run`): the first derives `FromRow` for a struct and
  reads two rows with `fetch_all`; the second runs `shelf_codegen::from_row::expand` on a
  struct and prints the expansion, which is pasted below it as captured.
- §0.3's program (`run`) waits for phase S1c.

## Claims

- Docs: the crates' versions and dates in §0.2, from the main book's Appendix F (recheck).
- Opinion: an ORM is worth building once, to know what the one you use does for you.

## Summary

Plan: one bullet per section.

## Exercises

Plan:
1. Add a field to §0.7's `Product` that the table does not have, and read the error. *Hint:*
   it names the struct, the field and the column, before the query runs.
2. Rename a column with `AS` in the query and keep the program working without touching the
   struct; then the other way round. *Hint:* Chapter 4's `rename`.

> **Open questions.** (1) Whether §0.2's comparison table belongs here or in the appendix's
> phrasebook, with a short version here. (2) The ten-line taste needs S1b and S1c; until then,
> §0.3 can show only the mapping. (3) Whether to explain the absolute paths of the expansion
> (`::shelf`, `::core`) here or leave them to Chapter 3, which covers hygiene.

---

Next: [Chapter 1 — Mapping rows by hand](01-mapping-by-hand.md)
