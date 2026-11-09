# Plan — *Building an ORM on tokio-postgres*: shelf, a build-along course

This is the plan for a child project of [Async PostgreSQL in Rust](../PLAN.md): **shelf**, a
small ORM built on tokio-postgres, and a second book that follows its construction chapter by
chapter. The main purpose is the derive macros that map rows to structs. Around them, shelf
grows what a basic ORM has: a model layer that writes the everyday SQL, partial updates and
optimistic locking, a typed query builder, relations, bulk writes, streams, and a schema check.

This file is the source of truth for the child project, as the root PLAN.md is for the main
book. [orm/CLAUDE.md](CLAUDE.md) holds the working rules that differ from the root
[CLAUDE.md](../CLAUDE.md) or add to it.

Status: **Phases S0 (scaffold) and S1a (the mapping) done on 2026-11-07**, on the branch
`shelf`. `#[derive(FromRow)]` is complete (§4.0): attributes, errors at the right tokens,
generics, tuples, one check per statement, and the `fetch_*` helpers, with expansion, UI and
integration tests and a benchmark. Every chapter has a **sketch** in its place
([SKETCHES.md](SKETCHES.md)). **Chapters 0–4 were written on 2026-11-09**, ahead of S1b at the
user's request (§10): Chapters 1–4 are `current`, and Chapter 0 is a `draft` until its §0.3 can
run (after S1c). The book PDF, `orm/book/shelf-book.pdf`, shows the written chapters and the
sketches. The next phase is S1b, the model (§9, and §13 step by step).

---

## 1. Goal and audience

**Goal.** By the end, readers can write derive macros that are pleasant to use: they parse
attributes, report mistakes at the right line, are hygienic, and are tested. They also know how
an ORM sits on a driver: what SQL each method sends, how many round trips it costs, and when to
drop down to hand-written SQL. The book ends with shelf working, and with part of stockroom's
data layer ported to it.

**Audience.**

| They know | They do not need to know |
|---|---|
| **The main book, or its equivalent:** `Client` and `Connection`, `query` and parameters, `Row`, `ToSql`/`FromSql` and the derives in postgres-types, transactions, deadpool. Each chapter links to the main-book sections it builds on, and does not teach them again. | **Procedural macros**: `TokenStream`, syn, quote, spans, hygiene, helper attributes. The book teaches them from scratch, with a **Macro lens** box where it matters. |
| **Rust, at an average level**, as in the main book. | `PhantomData` and marker types, async methods in traits with `Send` futures, `LazyLock`. Each gets a **Rust refresher** where it first matters. |

---

## 2. Relation to the main book

**What is carried over unchanged:** the root CLAUDE.md's style contract (chapter files, banner,
numbered sections, Summary/Exercises/Solutions, the two code-block kinds, the check
directives, example hygiene, voice), its code conventions, its database rules, the
workspace, and the tooling, which now takes `--book orm`.

**What changes:**

- **The banner** names this book: `> **Building an ORM on tokio-postgres** · Previous: … · Next: …`.
- **Side-by-side sections** compare three ways of doing one task:

  | Column | Role | Compiled? |
  |---|---|---|
  | **By hand** | tokio-postgres as the main book writes it. It shows what shelf saves, and what it costs. | Yes, a runnable ` ```rust ` block |
  | **shelf** | The subject of the book. | Yes, a runnable ` ```rust ` block (it may share one program with the hand-written version and assert that both give the same result) |
  | **An established crate** | Whichever is closest for the topic: sqlx for row mapping, SeaORM for models and relations, diesel for the query builder. Its first line names it: `// SeaORM (illustration)`. | No, a ` ~~~rust ` illustration |

  The closing table is `| | by hand | shelf | <crate> |` with the rows **Same** and **Different**.
- **Callouts:** the four from the main book, plus `> **Macro lens:**`, which covers what the
  compiler hands a macro and what it takes back: tokens, spans, hygiene, and the order of
  expansion.
- **"In shelf" sections** replace "In stockroom". They show trimmed excerpts of the shelf crates
  (`// orm/shelf-codegen/src/from_row.rs (excerpt)`) followed by a link. "In stockroom" still
  appears where stockroom's hand-written code is the comparison.
- **The playground** is `orm/playground` (package `shelf-playground`). Its dependencies
  inherit their versions from the workspace; nothing prints its Cargo.toml for readers to copy.
  Examples are generated as `shelf_chNN_KK.rs`, because both playgrounds build into the same
  `target/debug/examples/`.
- **Links to the main book** are relative (`../../book/03-rows-and-types.md#37-…`), and
  `book.py links` checks them.

---

## 3. How the book follows the build

A build-along book has a problem: each chapter shows the crate at one stage, but the repository
holds the finished crate. The book handles it in five ways:

1. **Stage programs.** Each new mechanism appears first as a runnable single-file program: a
   hand-written impl, a `macro_rules!` macro, or a derive's logic written as an ordinary
   function from `syn::DeriveInput` to a `TokenStream`, run on a struct built with
   `parse_quote!` and printed with `shelf_playground::pretty`. A proc macro cannot be defined
   in a single file, but its logic can: syn, quote and proc-macro2 work in any program. So
   `book.py check` compiles, runs and asserts every stage, including the macro code.
2. **The finished crate, in excerpts.** Then the chapter shows how shelf does it, in excerpts
   that `book.py excerpts` keeps true to the files. Code that later chapters add is elided
   with `// …`.
3. **Behaviour, through the finished crate.** Claims about what shelf does are runnable examples
   that use `#[derive(shelf::…)]` against the check database.
4. **Nothing generated is typed by hand.** Expansions come from running shelf-codegen, and
   compiler errors come from trybuild's `.stderr` files or a real build. Both are captured, as
   the main book captures psql output.
5. **The crates are shaped to grow.** There is one module per derive and per concern
   (`attrs.rs`, `columns.rs`, `sql.rs`, and so on), so a chapter adds a function, a module or a
   `match` arm instead of rewriting what an earlier chapter quoted.

---

## 4. The software: shelf (as built so far, then the target)

Items marked **(built, S1a)** exist and are tested; the rest is the target design, settled when
its phase builds it, and then rewritten "as built", as the root PLAN.md §5 was. §4.0 describes
what S1a built.

### 4.0 As built: the mapping (S1a, 2026-11-07)

| Item | Where | What it is |
|---|---|---|
| `trait FromRow` | `shelf/src/row.rs` | `type Columns: Send + Sync + 'static` (where the values are, found once), `fn resolve(&[Column]) -> Result<Columns, Error>` (find each column by exact name and check `accepts` once per statement), `fn from_row_at(&Row, &Columns) -> Result<Self, Error>` (read by position), and `fn from_row(&Row)` provided (both, for one row) |
| `#[derive(FromRow)]` | `shelf-codegen/src/from_row.rs` | `type Columns` is a tuple with one element per field (`usize`, a flattened type's `Columns`, or `()` for `skip`); `resolve` calls `__private::column::<T>` per field; `from_row_at` calls `__private::get::<T>` (or `get_try_from`, or the nested `from_row_at`) |
| Attributes | `shelf-codegen/src/attrs.rs`, `case.rs` | Struct: `rename_all` (`snake_case`, `camelCase`, `PascalCase`, `SCREAMING_SNAKE_CASE`). Field: `rename`, `skip`, `flatten`, `json`, `try_from = Type` (or `"Type"`). Unknown or repeated keys, values on flags and impossible combinations are errors at the offending tokens, all reported in one build |
| Generics | `shelf-codegen/src/generics.rs` | Lifetime parameters are refused (owned values only). A type parameter gets the bound its fields need (`T: FromColumn`, `T: FromRow`, `Json<T>: FromColumn`, `T: Default`, the `TryFrom` bounds), and only when a field's type mentions it |
| `trait FromColumn` | `shelf/src/row.rs` | `FromSqlOwned` under shelf's name, with `#[diagnostic::on_unimplemented]` for a readable error (§12 #11) |
| Tuples | `shelf/src/tuples.rs` | `(A,)` to 12 elements, by position, from one `macro_rules!` invocation with a line per arity |
| `shelf::Error` | `shelf/src/error.rs` | `Mapping { model, field, problem: Mapping }` with `Mapping::{MissingColumn, WrongType, Null, Decode}`, and `Db(tokio_postgres::Error)`. `#[non_exhaustive]`: S1b adds `NotFound` and `Stale`, S1c `Schema` |
| `from_rows`, `fetch_all`, `fetch_one`, `fetch_opt` | `shelf/src/row.rs`, `fetch.rs` | `fetch_*` take tokio-postgres's `GenericClient`, prepare, resolve against the statement (so a wrong mapping fails before the query runs, with no extra round trip), then query. S1b moves them onto `Executor` |
| `__private` | `shelf/src/lib.rs` | `column`, `column_at`, `get`, `get_try_from`, `tokio_postgres`, `Json`: what generated code names |

### 4.1 Crates

| Crate | Kind | Role |
|---|---|---|
| `orm/shelf` | lib | The runtime: the traits, the executor, errors, the query builder, the relation loaders, bulk writes, streams and `verify`. It re-exports the derives, and `__private` re-exports what generated code needs (tokio-postgres) so that users' crates do not have to depend on it directly. |
| `orm/shelf-macros` | proc-macro | A thin shell: `parse_macro_input!`, then `shelf_codegen::<derive>::expand`, then `into_compile_error` on failure. |
| `orm/shelf-codegen` | lib | Everything the derives generate, as functions `expand(&DeriveInput) -> syn::Result<TokenStream>`. Unit and expansion tests run here. |
| `orm/playground` | lib + generated examples | The book's runnable examples and helpers. |
| `orm/stockroom-shelf` | lib + bin (S2) | The capstone: stockroom's schema, migrations and domain types (`Sku`, `Address`, `OrderStatus`, `Cents`) reused through the `stockroom` crate, a repository layer on shelf, and the same axum routes for stores, products and orders. |

**Why three crates:** a proc-macro crate can export only macros, so code generation that lives
in one cannot be called by unit tests in another crate, by the book's examples, or by anything
else. In a plain library it can. The cost is one more crate, and one crate boundary that the
book explains (Chapter 3).

### 4.2 Derives

| Derive | Goes on | Generates | Ch |
|---|---|---|---|
| `FromRow` **(built, S1a)** | a struct with named fields | `impl FromRow`: `Columns`, `resolve` and `from_row_at` (§4.0) | 3–5 |
| `Table` | a `FromRow` struct | `impl Table`: `TABLE`, `COLUMNS`, the key type and `key()`, the read SQL; later the typed column constants (`Product::SKU`), relation methods and the COPY row writer | 6, 10–12 |
| `Insert` | a `New…` struct | `impl Insert` with `type Model`, the `INSERT … RETURNING` SQL and the parameters in order | 8 |
| `Patch` | a struct of `Option` fields | `impl Patch`: a partial `UPDATE` (the design is chosen in Chapter 9) | 9 |

`FromRow` also covers tuples, `(A,)` up to 12 elements, read by position **(built, S1a)**.
These impls are written with a `macro_rules!` macro (Chapter 2).

### 4.3 Attributes

The container attributes (`#[shelf(…)]` on the struct):

| Attribute | On | Meaning |
|---|---|---|
| `table = "products"`, `schema = "inventory"` | `Table` | The table, quoted at macro time |
| `model = Product` | `Insert`, `Patch` | The model that `RETURNING` maps into, which also gives the table |
| `rename_all = "…"` **(built, S1a)** | `FromRow` | A naming convention for columns: four cases, written by hand |
| `belongs_to(Store, key = store_id)`, `has_many(OrderLine, key = order_id, as = lines)` | `Table` | Relations. Names are explicit: no pluralising guesswork |

The field attributes:

| Attribute | Meaning |
|---|---|
| `rename = "col"` **(built, S1a)** | Read the column `col` |
| `skip` **(built, S1a)** | Do not read; use `Default::default()` |
| `flatten` **(built, S1a)** | A nested `FromRow` read from the same row |
| `json` **(built, S1a)** | Read through `Json<T>` |
| `try_from = SqlType` **(built, S1a)** | Read `SqlType`, then `TryFrom` into the field. The type may also be quoted, as serde writes it |
| `primary_key` | Part of the key; several fields make a composite key (a tuple) |
| `generated` | The database fills it (`uuidv7()`, `now()`, identity): never written by shelf |
| `version` | Optimistic locking: `WHERE … AND version = $n`, `SET version = version + 1` |
| `updated_at` | `SET updated_at = now()` on every update |

### 4.4 The API in use (target, illustration)

~~~rust
// shelf (target design, not built yet)
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
struct NewProduct { sku: String, price_cents: i64, tags: Vec<String> }

let product = NewProduct { … }.insert(&db).await?;          // INSERT … RETURNING
let product = Product::find(&db, &id).await?;                 // Err(Error::NotFound)
let page = Product::query()
    .filter(Product::PRICE_CENTS.gt(1_000).and(Product::TAGS.contains("sale")))
    .order_by(Product::SKU.asc())
    .limit(20)
    .all(&db)
    .await?;
let lines = Order::load_lines(&db, &orders).await?;           // one query: = ANY($1)
NewProduct::insert_many(&db, &batch).await?;                  // unnest; copy_in for binary COPY
shelf::verify::<Product>(&db).await?;                         // the struct against the table
~~~

### 4.5 The executor and errors

- **`Executor`.** Neither `tokio_postgres::GenericClient` nor deadpool's version can be
  implemented outside its crate (both are sealed), and only deadpool's has `prepare_cached`.
  shelf therefore defines a trait of its own, with the calls it needs. Its async methods return
  `impl Future<Output = …> + Send`. It is implemented for `tokio_postgres::{Client,
  Transaction<'_>}` and, behind the default feature `deadpool`, for
  `deadpool_postgres::{Client, Transaction<'_>}`. Which statement strategy each executor uses
  (`prepare`, `prepare_cached` or `query_typed`) is measured through lagproxy in Chapter 7 and
  recorded here.
- **`shelf::Error`**: `Mapping` and `Db` are built (§4.0). To come: `NotFound { model }` and
  `Stale { model }` (S1b), and `Schema(Vec<Mismatch>)` from `verify` (S1c).
  Mapping constraint names to HTTP statuses remains the application's job, as with
  stockroom's `AppError`. Chapter 7 shows how.

### 4.6 Out of scope, on purpose

- Migrations and DDL generated from models. The main book's Chapter 14 covers migrations.
- Checking queries against a live database at compile time (sqlx's `query!`). Chapter 13
  discusses what it adds.
- Lazy loading, an identity map, change tracking and a unit of work, and cascades.
- Other databases, and a blocking API.
- Postgres enums, composites and domains. postgres-types' derives already handle them, and
  shelf passes them through: the capstone uses stockroom's types unchanged.
- Hooks such as `before_insert`. They make a good exercise.

---

## 5. Feature coverage matrix

Each shelf feature, the chapter that builds it, what it rests on in tokio-postgres or
PostgreSQL, and the main-book chapter that taught that.

| shelf feature | Ch | Underneath | Main book |
|---|---|---|---|
| `FromRow` by name | 1, 3 | `Row::try_get(&str)`, `FromSql` | 3 |
| Tuples by position | 2 | `Row::try_get(usize)` | 3 |
| `json`, `try_from`, `flatten`, `rename`, `skip` | 4 | `types::Json<T>`, `FromSql` + `TryFrom` | 3.7, 3.8, 5 |
| One check per statement | 5 | `Statement::columns()`, `Column::type_()`, `FromSql::accepts` | 3, 4 |
| `Table`: SQL consts, quoted identifiers | 6 | `quote_ident`; an explicit column list, never `*` | 2.3, 4.6 |
| `find`, `all`, `count` | 6 | `query_one`, `query_opt`, `query` | 2 |
| `Executor` | 7 | sealed `GenericClient`s, `prepare_cached`, `query_typed` | 4, 8.5, 13 |
| `shelf::Error` | 7 | `Error`, `DbError`, `SqlState` | 6 |
| `Insert`, `RETURNING`, defaults | 8 | `RETURNING`, `DEFAULT uuidv7()` (PG 18) | 2.6 |
| Upsert | 8 | `ON CONFLICT`, `MERGE … RETURNING` (PG 17) | 2.6, 10.6 |
| `Patch` | 9 | statements per shape versus one statement | 4.6 |
| `version`, `updated_at` | 9 | rows affected, `now()` | 8.8, 15.6 |
| Transactions | 7, 9 | `transaction`, `build_transaction`, the retry helper | 8 |
| Query builder | 10 | `$n` parameters, `= ANY($1)`, `@>`, keyset pagination | 2.3, 3.9, 9.6 |
| Relations | 11 | arrays as parameters, pipelining with `try_join!` | 3.9, 7 |
| `insert_many` | 12 | `unnest` of arrays, casts for domains | 10, 5 |
| Binary COPY from the derive | 12 | `BinaryCopyInWriter`, types from a prepared statement | 10 |
| Streams of models | 12 | `query_raw`, `RowStream`, backpressure | 9 |
| `verify` | 13 | `prepare`, `Statement::columns()`, the catalog for nullability | 4, 5 |
| The capstone | 14 | deadpool, axum, `in_transaction` | 13–16 |

---

## 6. Tooling and verification

- **`tools/book.py --book orm …`** runs every command on this book. The settings come from the
  `[book]` table in `orm/book/book.toml`: the playground and package, the example prefix
  `shelf_`, the check database `tpbook_shelf_check`, `orm/README.md` for `index`, and
  `orm/book/shelf-book.pdf`. Chapter previews go to `target/book/shelf/`.
- **Sketches:** a chapter not written yet has its sketch in its place, with the status
  `sketch` (orm/SKETCHES.md); `book.py` requires the card (`> **Sketch, <date>.**`) exactly
  when the status says so, and the registry's `pdf_statuses` puts sketches in the book PDF.
- **Mirrors:** `orm/playground/src/lib.rs` has `// region: pretty`, which Chapter 3 prints.
- **shelf's own tests** (built in S1a; each new derive adds to them):
  - *Unit tests* in shelf-codegen for attribute parsing and error collection, and in shelf for
    helpers (`short_type_name`).
  - *Expansion tests*: `orm/shelf-codegen/tests/expand.rs` runs each derive on the structs in
    `tests/expand/NAME.rs` and compares the pretty-printed result with `NAME.expanded.rs`;
    `SHELF_BLESS=1` rewrites the expected files. Chapter 13 explains the technique.
  - *UI tests* with trybuild: `orm/shelf/tests/ui.rs` and one file per kind of error in
    `tests/ui/`, later also the query builder's type errors. The `.stderr` files depend on
    rustc's wording: regenerate them with `TRYBUILD=overwrite` when the toolchain changes
    (pinned to rustc 1.99.1), and read the diff.
  - *Integration tests* against PostgreSQL in the database `shelf_test`, through
    `orm/shelf/tests/common/mod.rs`: `TestDb::new()` creates the database on first use, gives
    the test its own schema (set with `Config::options("-c search_path=…")`) and drops it at the
    end. This is the main book's §17.2 pattern, simplified because shelf has no migrations.
  - *The hygiene test*, `orm/shelf/tests/derive.rs`: derives next to a `Result` type, an `Ok`
    constant and a `Default` trait of the user's.
  - *Benchmarks* as examples of the shelf crate (`examples/bench_mapping.rs`), run in release
    mode; their tables go into §12 with what they were measured on.
- **Gates:** the root gates (the workspace's cargo test, clippy and fmt already cover the orm
  crates), plus `python3 tools/book.py --book orm check`. The main book's
  `python3 tools/book.py check` must stay green on this branch.

---

## 7. Chapter outline

There are 15 chapters and an appendix, in three parts, at about 600–1,000 lines each. Each
lists its sections, its side-by-side topic, its "In shelf" files, and its exercises. The titles
are registered in [book/book.toml](book/book.toml).

### Part I — Mapping rows: the macros

**Chapter 0 — Introduction and setup**

- 0.1 What an ORM does (it maps rows, writes the everyday SQL and follows relations) and what
  shelf will not do (§4.6).
- 0.2 Where shelf sits: diesel and diesel-async (a schema DSL, compile-time checked), SeaORM
  (entities and `ActiveModel`, on sqlx), toasty (from the Tokio project, young), sqlx's
  `FromRow`, postgres-from-row, and Cornucopia (SQL first, code generated). One table shows what
  each decides. The main book's Appendix F has the research.
- 0.3 The finished API in ten lines.
- 0.4 Three crates, and why (§4.1).
- 0.5 How this book follows the build (§3), and its callouts.
- 0.6 Setup: the main book's setup, the shelf playground, running an example, and seeing an
  expansion.
- 0.7 The first derive, end to end (already in the scaffold).

**Chapter 1 — Mapping rows by hand**

- 1.1 A trait of shelf's own: `FromRow` versus `TryFrom<&Row>`, and what a trait allows later
  (associated items, tuples, generic helpers).
- 1.2 By name or by index: what `Row::try_get("sku")` does in 0.7.18, and what it costs per
  row, measured.
- 1.3 NULL: `Option<T>`, `WasNull`, and a missing column versus a NULL value.
- 1.4 Nested structs: a `Price` inside a `Product`, flattened by hand.
- 1.5 Fetch helpers: `fetch_all::<T>(&client, sql, params)` and its `one`/`opt` siblings.
- 1.6 Counting the boilerplate: each field is named three times (in the struct, the `SELECT`
  list and `try_get`). That repetition is what a macro should take over.
- Side by side: a row into a struct (by hand, a shelf preview, sqlx's `FromRow`).
- In stockroom: the `TryFrom<&Row>` impls in `repo/*.rs`.

**Chapter 2 — Declarative macros**

- 2.1 `macro_rules!`: matchers, the fragment types (`ident`, `ty`, `expr`, `tt`, `vis`,
  `meta`) and repetitions.
- 2.2 A `model!` macro that declares a struct and its `FromRow` impl together.
- 2.3 Tuples: `FromRow` for `(A,)` up to 12 elements from one macro, invoked with a line per
  arity (`(A 0, B 1)`). These are the impls shelf keeps. A recursive version as an exercise.
- 2.4 `$crate`, and hygiene for local variables.
- 2.5 Where declarative macros stop: field attributes need tt-munching, renaming gets awkward,
  error messages are poor, and generics are hard. That is the case for proc macros.
- Side by side: generating impls for many types.
- In shelf: `orm/shelf/src/tuples.rs`.

**Chapter 3 — The first derive macro**

- 3.1 Proc-macro crates: `proc-macro = true`, `proc_macro::TokenStream`, and the compiler as
  the caller. A **Macro lens** on what a derive receives.
- 3.2 The three-crate split, and `proc_macro2` (why the codegen crate cannot use
  `proc_macro`).
- 3.3 Parsing with syn 3: `DeriveInput`, `Data`, `Fields`, `Field`, and syn's features. What
  changed from syn 2, which most material online still shows.
- 3.4 Generating with quote: `#var`, repetition `#(…),*`, `format_ident!`, and spans.
- 3.5 Hygiene: absolute paths, `__private`, raw identifiers (`IdentExt::unraw`), and
  `#[automatically_derived]`. Includes a test where the user's module defines `type Result`.
- 3.6 Seeing the expansion: `pretty()` (printed, a mirror), `cargo expand`, and
  rust-analyzer's "Expand macro recursively".
- 3.7 Errors as values: `syn::Error`, `into_compile_error`, and a span that points at the
  offending token (an enum is refused at its `enum` keyword).
- Side by side: attaching generated behaviour to a type.
- In shelf: `shelf-macros/src/lib.rs`, `shelf-codegen/src/from_row.rs`.

**Chapter 4 — Attributes, errors and generics**

- 4.1 Helper attributes: `attributes(shelf)`, and the error without it (§12 #9).
- 4.2 `Attribute::parse_nested_meta`: key-value pairs, flags and nested lists. Parsing into a
  typed `FieldAttrs`, and rejecting unknown and duplicate keys with spans.
- 4.3 `rename`, `skip`, `flatten`, `json` and `try_from`.
- 4.4 Reporting several errors at once with `syn::Error::combine`, and where errors land
  (spans, §12 #12).
- 4.5 UI tests with trybuild, and keeping `.stderr` files current. What reading them
  changed: `#[diagnostic::on_unimplemented]` on `FromColumn` (§12 #11).
- 4.6 Generics: `split_for_impl`, bounds added only for fields that mention a type parameter,
  and why shelf maps owned values only (borrowing from a row would need `FromRow<'a>`; lifetimes
  are refused with an error).
- 4.7 What darling would save, and why shelf parses by hand.
- Side by side: attribute-driven mapping (by hand, shelf, sqlx's `#[sqlx(rename, flatten, json,
  try_from)]`).
- In shelf: `shelf-codegen/src/attrs.rs`.

**Chapter 5 — Checking a statement once, not every row**

- 5.1 What mapping by name costs on every row (the measurement from §1.2, scaled up).
- 5.2 A statement knows its columns: resolving each field to an index once.
- 5.3 Types checked once: `FromSql::accepts(column.type_())` per column, not per row. A mismatch
  becomes `Error::Mapping`, which names the model, the field, the column and both types, before
  the first row is read.
- 5.4 What the derive generates for this, and how `from_row` stays available for a single row.
- 5.5 Measured: by name versus resolved, on 100,000 rows of 12 columns.
- Side by side: reading 100,000 rows.
- In shelf: `shelf/src/row.rs`, `shelf-codegen/src/from_row.rs`.

### Part II — The model

**Chapter 6 — Tables: the model derive**

- 6.1 What a table adds to a row type: a name, a key and a column list.
- 6.2 `#[derive(Table)]` with `table`, `primary_key` and `generated`; composite keys as tuples.
- 6.3 SQL at compile time: string literals built by the macro, with identifiers quoted there
  (so `order` and `"Mixed"` work), and values never placed in the SQL.
- 6.4 Reading: `find` (`NotFound`), `find_opt`, `all` and `count`. An explicit column list,
  never `*`, because of the "cached plan must not change result type" error (main book §4.6).
- 6.5 Metadata as consts, and where const evaluation stops: a literal cannot include another
  type's const, which is where `LazyLock` comes in. A **Rust refresher**.
- Side by side: fetching by primary key (by hand, shelf, SeaORM's `find_by_id`).
- In shelf: `shelf-codegen/src/table.rs`, `shelf/src/table.rs`.

**Chapter 7 — The executor and shelf's errors**

- 7.1 Why not `GenericClient`: both versions are sealed, and only deadpool's caches statements.
- 7.2 The `Executor` trait: async methods with `Send` futures (a **Rust refresher**), and its
  impls for `Client`, `Transaction` and deadpool's two types.
- 7.3 Statement strategies, measured through lagproxy: `prepare` then execute (two round trips),
  `prepare_cached` (one, after the first), and `query_typed` (one, unnamed, but it needs the
  types).
- 7.4 Transactions: shelf calls inside a `tx`, and the main book's retry helper working
  unchanged.
- 7.5 `shelf::Error`, `source()`, and mapping it into an application's error with constraint
  names.
- Side by side: one repository function over a client, a transaction and a pool.
- In shelf: `shelf/src/executor.rs`, `shelf/src/error.rs`.

**Chapter 8 — Writing rows: insert, update, delete, upsert**

- 8.1 `#[derive(Insert)]` with `model = Product`, and `RETURNING` the model's columns.
- 8.2 Database defaults: `uuidv7()` (PG 18), `now()`, identity columns, and `generated` fields.
- 8.3 `update(&self)` (every column that is neither key nor generated), `delete` and
  `delete_by_key`. Zero rows affected means `NotFound`.
- 8.4 Upsert: `ON CONFLICT (…) DO UPDATE SET … = excluded.…`, what `RETURNING` gives after
  `DO NOTHING`, and `MERGE … RETURNING` (PG 17) as the alternative.
- 8.5 Constraint errors from writes (main book Chapter 6).
- Side by side: inserting a row and reading it back.
- In shelf: `shelf-codegen/src/insert.rs`, `shelf/src/write.rs`.

**Chapter 9 — Partial updates and optimistic locking**

- 9.1 The problem: PATCH bodies, absent versus null, `Option<Option<T>>`, and serde.
- 9.2 Three designs:
  - (a) a dynamic `SET` list, one statement per shape;
  - (b) `coalesce($n, col)`, one statement that can never set NULL;
  - (c) a flag per field, `col = CASE WHEN $f THEN $v ELSE col END`, one statement that can set
    NULL.

  The section measures them: prepared statements in `pg_prepared_statements`, and round trips.
  shelf picks one in S1b.
- 9.3 `#[shelf(version)]`: `WHERE key = $1 AND version = $2` and `version = version + 1`.
  Telling `Stale` from `NotFound` in one round trip (main book §8.8, §15.6).
- 9.4 `#[shelf(updated_at)]` and `now()`, which is the transaction's time (a **SQL lens**).
- 9.5 ETags and `If-Match` in axum, with shelf.
- Side by side: an edit from two admin tabs.
- In shelf: `shelf-codegen/src/patch.rs`.

### Part III — Queries and relations

**Chapter 10 — A typed query builder**

- 10.1 What should be typed (columns and values) and what should stay SQL.
- 10.2 `Column<M, T>` constants generated by `Table` (`Product::PRICE_CENTS`). A **Rust
  refresher** on `PhantomData`.
- 10.3 Expressions: `eq`, `ne`, `lt`, `le`, `gt`, `ge`, `between`, `like`, `is_null`, `any`
  (`= ANY($n)`) and `contains` (`@>`), combined with `and`, `or` and `not`. They form an
  expression tree, which is rendered to SQL with `$n` numbering and a parameter list.
- 10.4 Owned parameters (`Box<dyn ToSql + Sync + Send>`), and why the query owns them.
- 10.5 `order_by`, `limit`, `offset`, keyset `after` (main book §9.6), `count` and `exists`.
- 10.6 What fails to compile: a wrong value type, or a column of another model. Shown with UI
  tests and the captured errors.
- 10.7 The escape hatch: raw SQL through `fetch_all::<T>`, and a raw SQL fragment with its
  parameters inside a filter.
- Side by side: a filtered, sorted page (by hand, shelf, diesel).
- In shelf: `shelf/src/query/`.

**Chapter 11 — Relations without N+1**

- 11.1 `belongs_to` and `has_many`, and the methods they generate: `order.lines(&db)`,
  `Order::load_lines(&db, &orders)` and `line.order(&db)`.
- 11.2 The N+1 problem, measured through lagproxy: 100 orders cost 101 round trips.
- 11.3 Batched loading with `WHERE order_id = ANY($1)` and grouping in a `HashMap`: one round
  trip.
- 11.4 Pipelining independent loads with `try_join!` on one connection (main book Chapter 7).
- 11.5 Joins: duplicated rows, `json_agg` or `array_agg` of composites, and when a join wins.
- 11.6 Why shelf has no lazy loading: async, borrowing, and hidden round trips.
- Side by side: orders with their lines (by hand, shelf, SeaORM's `find_with_related`).
- In shelf: `shelf-codegen/src/relations.rs`, `shelf/src/relations.rs`.

**Chapter 12 — Bulk writes and streams**

- 12.1 `insert_many`: one `INSERT … SELECT * FROM unnest($1::t[], …)` with one array per
  column, built by the derive, and casts for domains (main book §12 #1 and #4).
- 12.2 Binary COPY from the derive: column types learnt from a prepared
  `SELECT … LIMIT 0`, then `BinaryCopyInWriter` (main book Chapter 10: the declared types are
  trusted).
- 12.3 Measured: a loop, pipelined inserts, `unnest` and COPY, compared with pgload's table.
- 12.4 Streams of models: `query().stream(&db)` over `query_raw`, with columns resolved once
  (Chapter 5) and backpressure (main book Chapter 9).
- 12.5 A COPY that the server rejects as it starts kills the connection in 0.7.18
  (rust-postgres #789), and how shelf avoids it.
- Side by side: loading 10,000 rows.
- In shelf: `shelf/src/bulk.rs`, `shelf/src/stream.rs`.

**Chapter 13 — Trusting the mapping: schema checks and tests**

- 13.1 What can drift: a renamed column, a changed type, a nullable column read into `T`.
- 13.2 `shelf::verify::<M>(&db)`: prepare the model's `SELECT`, compare `Statement::columns()`
  with each field's `accepts`, and look up nullability in the catalog (a row description does not
  carry it). Report every mismatch at once.
- 13.3 Where to run it: at startup, and in one test per model.
- 13.4 Testing macros: expansion tests, UI tests and integration tests, and what each one
  catches.
- 13.5 Compile-time checking: sqlx's `query!` and its offline mode, and Cornucopia. What they
  cost and when they are worth it.
- Side by side: catching a renamed column (by hand at the first query, shelf at startup, sqlx
  at compile time).
- In shelf: `shelf/src/verify.rs`, `orm/shelf/tests/`.

**Chapter 14 — Capstone: stockroom on shelf**

- 14.1 The shape of `orm/stockroom-shelf`: stockroom's migrations and domain types reused, and
  stores, products and orders on shelf behind the same routes.
- 14.2 Models for a real schema: a domain (`Sku`), a composite (`Address`), an enum
  (`OrderStatus`), `Json<Customer>`, and a virtual column (read only).
- 14.3 Line for line: stockroom's repository functions against shelf's, counting lines,
  statements and round trips.
- 14.4 Where shelf steps aside: the oversell-proof guarded `UPDATE`, the `MERGE` import, COPY
  export and `LISTEN`. Raw SQL plus `FromRow` is the escape hatch.
- 14.5 What a production ORM would add next.

**Appendix (15)**

- A. An attribute reference.
- B. What each derive generates, printed by running shelf-codegen.
- C. A phrasebook from shelf to diesel, SeaORM and sqlx.
- D. A glossary: token, `TokenStream`, span, hygiene, helper attribute, expansion, and others.
- E. Further reading: *The Little Book of Rust Macros*, syn's documentation, and dtolnay's
  proc-macro workshop.

---

## 8. Versions (verified 2026-11-07)

| Item | Version | Notes |
|---|---|---|
| syn | 3.0.9 | With the `full` feature. Its derive types (`DeriveInput`, `Data`, `Fields`, `Field`) have the same shape as syn 2's, checked in the source |
| quote / proc-macro2 | 1.0.47 / 1.0.107 | |
| prettyplease | 0.3.0 | Built on syn 3. Its output is what the book prints |
| trybuild | 1.0.122 | rust-version 1.88. Added in S1a |
| darling | 0.24.1 | Built on syn 3. Mentioned only (Chapter 4) |
| heck | 0.5.0 | Mentioned only. shelf converts case itself, for the few cases it supports |
| rustc | 1.99.1 | edition 2024, as in the root PLAN.md §8 |
| Everything else | — | As in the root PLAN.md §8 (tokio-postgres 0.7.18, deadpool-postgres 0.14.2, PostgreSQL 18.6) |

---

## 9. Phases

Each phase ends with every gate green, in both books. The software comes before the chapters,
as in the main book, so that every excerpt quotes real code.

| Phase | Deliverable | Done when |
|---|---|---|
| **S0. Scaffold** (done 2026-11-07) | The branch `shelf`; this plan; orm/CLAUDE.md and orm/README.md; the four crates, with a first `FromRow` derive (named fields, by name) and an expansion test; `book.py --book`; a Chapter 0 stub with two runnable examples | Both books' `check` pass, and so do the cargo gates and the tool's tests |
| **S1a. shelf: mapping** (done 2026-11-07) | `FromRow` complete (attributes, combined errors, the generics policy), tuple impls, one check per statement and `Error::Mapping`, `fetch_*` helpers, expansion tests, trybuild UI tests, and the `shelf_test` harness | The gates pass; the measurements for Chapters 1 and 5 are recorded in §12 |
| **Sketches** (done 2026-11-07) | A sketch for each of the 16 chapters, in place, in the format of [SKETCHES.md](SKETCHES.md); the `sketch` status and `pdf_statuses` in `book.py`; the book PDF | `book.py --book orm check` passes; the PDF is fresh |
| **S1b. shelf: the model** | `Table`, `Insert` and `Patch`; `Executor` with the `deadpool` feature; `shelf::Error`; CRUD and upsert; `version` and `updated_at`; the `Patch` design chosen by measurement | The gates pass; the statement-strategy and `Patch` measurements are in §12 |
| **S1c. shelf: queries and relations** | The query builder, relations, `insert_many` and COPY, streams, and `verify` | The gates pass; the N+1 and bulk tables are in §12; the query builder's compile errors have UI tests |
| **S2. stockroom-shelf** | The capstone app and its tests | The gates pass; the ported routes give the same responses as stockroom's |
| **S3a. Part I** (Chapters 0–4 written 2026-11-09, ahead of S1b) | Chapters 0–5 | `book.py --book orm check` passes, including excerpts and links; the chapters are `current` |
| **S3b. Part II** | Chapters 6–9 | The same |
| **S3c. Part III** | Chapters 10–14 and the appendix | The same |
| **S4. Polish and merge** | The book PDF, a read-through, a coverage audit against §5, and merging `shelf` into `main` | `book.py --book orm list` shows every chapter `current` and the PDF fresh |

---

## 10. Decisions taken (recommended defaults, easy to revisit)

| Decision | Choice | Alternatives |
|---|---|---|
| Where it lives | `orm/` in the same repository and workspace, on the branch `shelf` until S4 (chosen 2026-11-07) | Part IV of the main book (rejected); a separate repository |
| Name | **shelf**: `shelf`, `shelf-macros`, `shelf-codegen` (chosen 2026-11-07) | pgmodel, rowmap |
| Scope | The core plus all four extras: `Patch` and optimistic locking, the query builder, relations, and bulk/streams/schema check (chosen 2026-11-07) | the core only |
| Book title | *Building an ORM on tokio-postgres* | — |
| syn | 3, the current major version | syn 2, which most material online still shows |
| Attribute parsing | by hand with `parse_nested_meta` (it teaches the mechanism) | darling, shown as the alternative |
| Where code generation lives | a codegen library plus a thin proc-macro crate | everything in the proc-macro crate |
| The third side-by-side column | per topic: sqlx, SeaORM or diesel | always the same crate |
| Playground versions | inherited from the workspace | spelled out, like the main playground |
| Check database | `tpbook_shelf_check` | sharing `tpbook_check` (the two books' checks would collide) |
| Test database | `shelf_test`, one schema per test | per-test databases from a template (stockroom's way, which shelf does not need without migrations) |
| SQL for model CRUD | literals generated at compile time; `LazyLock` where a literal cannot be built | building SQL at run time on every call |
| The capstone | a port of part of stockroom, reusing the `stockroom` crate | a new domain |
| Chapters not written yet | a sketch in each chapter's place, in the PDF (as *Decisions in the Loop* does; requested 2026-11-07) | sketches in a separate document; planned chapters left out |
| Writing Chapters 0–4 before S1b | done on 2026-11-09, at the user's request: they quote only what S1a built (`FromRow`, the tuples, `fetch_*`, `pretty`, and for Chapter 4 `attrs.rs`, `case.rs`, `generics.rs`, `FromColumn` and the UI tests), and `book.py excerpts` flags any drift when S1b changes those files. S1b's `table` key changes what Chapter 4 says about it (§4.2, "One namespace") and `unknown_attributes.stderr` | waiting for S1b–S2, as §9 orders |
| An unreadable field type is reported twice (Chapter 4, §4.5–4.6) | kept: once per generated use (`column`, `get`), each in `FromColumn`'s words at the field. Bounding concrete field types in the `where` clause reports it once, but at the derive's name and in the driver's words (§12 #24) | one bound site per field |
| Two fields that read the same column | allowed: reading one column twice is odd but legal (a `jsonb` as a typed value and as `serde_json::Value`). The check is Chapter 4's Exercise 2 | refusing it in the derive |
| darling in Chapter 4 | shelf's parser rewritten in darling 0.24.1 in a scratch probe, shown as an illustration with the errors it produced (§12 #25); darling is not a dependency | none |
| Chapter 0's §0.3 (the API in ten lines) | the target design as a `~~~rust` illustration, marked as such; it becomes a runnable program after S1c, and Chapter 0 stays `draft` until then | showing only the mapping |
| Where the comparison with other crates lives | a short table in §0.2 (who writes the SQL, when a mapping mistake is caught), toasty in prose; the appendix's phrasebook maps API names | the full table in the appendix |
| Chapter 1's fetch helpers | the simple form (`query`, then map); shelf's prepare-first version is described and left to Chapter 5 | preparing first already in Chapter 1 |
| Chapter 3's stage generator | new code that writes Chapter 1's impl (`from_row` with `try_get` by name), so the stage reads as Chapter 1 automated; the excerpts of the finished generator elide what Chapters 4 and 5 add | reproducing S0's generator, which was not kept |
| Seeing an expansion | `pretty` first; `cargo expand` recommended too, since it runs on stable (§12 #7) | `pretty` only |

---

## 11. Risks and items to verify

- **Drift between the build-along text and the finished crate.** §3 handles this. Shape the
  crates to grow (§3, point 5) from S1a on.
- **`Send` futures from the `Executor`** when used in generic code inside axum handlers and with
  the retry helper (main book §12 #6). Probe this first in S1b, before the trait is fixed.
- **trybuild's `.stderr` files** change when rustc's wording changes. The toolchain is pinned,
  and they are regenerated deliberately.
- **syn 3 is recent.** Most examples online show syn 2. Resolved for Chapter 3: the
  differences a derive meets are two fields of `Field` (§12 #3, #5).
- **`cargo expand`**: resolved, it runs on stable (§12 #7).
- **`Patch` design (c)** sends a typed value for every field, even unset ones. Verify that a
  `None` of the field's type serializes for every column type, domains included.
- **Query builder ergonomics:** a `&str` for a `String` column, and `i32` literals for `i64`
  columns. Probe the trait design before Chapter 10.
- **Nullability is not in a row description.** `verify` needs the catalog: check that 0.7.18's
  `Column` exposes the table OID and column number.
- **Size.** 15 chapters and an appendix. Write and verify one chapter at a time. Chapter 4 runs
  to about 1,780 lines, most of it programs and captured compiler output, against §7's 600–1,000.
- **`#[shelf(json)]` and SQL NULL** (§12 #26): on an `Option<T>` field, an SQL `NULL` fails with
  `Mapping::Null`, whose advice ("read it into an Option") does not fit. Decide in S1b or S1c:
  a `json(nullable)` key as sqlx has, or a message for this case. Chapter 4 documents the
  current behaviour as a pitfall, and Chapter 1 quotes the message.

---

## 12. Findings from building the software (material for the chapters)

Each finding is hit for real while building shelf, pinned down by a test or a comment in the
code, and tagged with the chapter where it belongs.

| # | Finding | Chapter |
|---|---|---|
| 1 | **Example names must be unique across the workspace's playgrounds:** both build into `target/debug/examples/`, so two `ch00_01` would overwrite each other. The shelf book's examples are `shelf_chNN_KK`. (A tooling finding; no chapter.) | — |
| 2 | **A trait and a derive can share a name** because they live in different namespaces: `use shelf::FromRow` imports both, as serde's `Serialize` does. | 0, 3 |
| 3 | **syn 3's derive types match syn 2's** (`DeriveInput { attrs, vis, ident, generics, data }`, `Data::{Struct, Enum, Union}`, `Fields::Named`), so S0's derive needed no syn-3-specific code. **Except `Field`** (checked in the sources while writing Chapter 3): syn 2's `mutability: FieldMutability` became `modifiers: FieldModifiers`, and a new `default: Option<(Token![=], Expr)>` holds a default field value. Reading `attrs`, `ident` and `ty` is unchanged; building or destructuring a `Field` is not. `IdentExt::unraw` turns `r#type` into the column name `type`. | 3 |
| 4 | **`Row::try_get("name")` searches the columns twice in the worst case** (0.7.18, `row.rs`, `RowIndex for str`): first by exact name, then again ignoring ASCII case (a FIXME notes that this is not Postgres's own rule). So a missing column costs two scans, and `SKU` finds `sku`. Each `try_get`, whether by name or by index, also calls `T::accepts(column type)` on every row before it decodes. | 1, 5 |
| 5 | **syn 3.0.0's breaking changes barely touch what a derive uses**: `DeriveInput`, `Data`, `Fields`, `Attribute`, `parse_nested_meta` and `split_for_impl` are as in syn 2, and `Field` changed as #3 says (`FieldModifiers` is in the notes' *Modifiers* section). The changes are in types (`Type::BareFn` → `Type::FnPtr`, attributes on every `Type`), expressions, patterns (`Arm::guard` → `Pat::Guard`), items (`Safety`), literals (`LitInt` lost `From<proc_macro2::Literal>`; `Lifetime`'s `Parse` refuses keywords) and `Punctuated::pop` (now `Option<T>`). Source: the 3.0.0 release notes on GitHub. | 3 |
| 6 | **`Column` exposes `table_oid()` and `column_id()`** (`Option`s; `None` for a computed column), so `verify` can look up `pg_attribute.attnotnull` for the columns that come from a table. | 13 |
| 7 | **`cargo expand` runs on stable Rust** (corrected 2026-11-09; this row first said it needs nightly). It wraps `cargo rustc --profile=check -- -Zunpretty=expanded` (its README), and sets `RUSTC_BOOTSTRAP=1` for that compiler (its `src/main.rs`). Verified with the raw command on rustc 1.99.1: without the variable, "the option `Z` is only accepted on the nightly compiler"; with it, the expansion. It is a separate `cargo install cargo-expand`, not installed here. The book's primary way is still `pretty()`, which needs nothing and can be asserted on. | 3 |
| 8 | **Mapping by name costs about 3× mapping by index; the per-row `accepts` check costs nothing measurable.** 100,000 rows of 12 columns (4 `bigint`, 4 `text`, 2 `int`, a `bool`, a `float8`; two nullable; corrected 2026-11-09 from the benchmark's SQL), columns in reverse order of the struct, decoded from an in-memory `Vec<Row>`: by name 33.5–40.4 ms, by index 10.8–11.7 ms, by index with raw bytes and one `accepts` per statement 10.7–11.8 ms (three rounds each; release build, rustc 1.99.1, Apple Silicon, PG 18.6 on localhost). So shelf resolves names once per statement and then reads through the public `Row::try_get(usize)`; skipping the per-row check would need a raw-bytes `FromSql` wrapper and buys nothing. The check once per statement is for the *error* (before the first row, naming field and column), not for speed. | 1, 5 |
| 9 | **A derive must declare its helper attribute**: without `attributes(shelf)` in `#[proc_macro_derive(FromRow, attributes(shelf))]`, the compiler rejects `#[shelf(…)]` itself ("cannot find attribute `shelf` in this scope … `shelf` is in scope, but it is a crate, not an attribute"), before the derive runs. Found when S1a's hygiene test first used `#[shelf(skip)]`. | 4 |
| 10 | **Never write an error message from memory, even Rust's own**: a test expected `TryFromIntError`'s message to be "out of range integral type conversion attempted"; on rustc 1.99.1 a negative `i64` into `u64` says "number too small to fit in target type". The tests now assert on captured text. | 4 |
| 11 | **`#[diagnostic::on_unimplemented]` works through a blanket impl.** shelf's generated code bounds field types by `shelf::FromColumn`, a trait with that attribute and one impl, `impl<T: FromSqlOwned> FromColumn for T`. For a field of type `std::time::Instant`, rustc 1.99.1 then leads with shelf's words (``error[E0277]: `Instant` cannot be read from a PostgreSQL column``, labelled "tokio-postgres has no `FromSql` for this type") instead of an unsatisfied bound on `shelf::__private::tokio_postgres::types::FromSqlOwned`; its own notes follow. The error appears twice, once per generated use of the type (`column::<T>` and `get::<T>`). (`tests/ui/not_readable.stderr`.) | 4 |
| 12 | **Errors land where the user's tokens came from.** quote! keeps the spans of interpolated input, so a type error in generated code points at the field's type in the user's struct, and an error built with `Error::new_spanned(&lit, …)` points at the literal. The tokens the macro writes itself (`::shelf::…`) carry the call site's span. (All seven `.stderr` files.) | 3, 4 |
| 13 | **shelf's mapping, measured** (`cargo run -p shelf --release --example bench_mapping`; median of 5; 100,000 rows × 12 columns as in #8; release build, rustc 1.99.1, Apple Silicon, PG 18.6 on localhost; two runs): by name by hand 38.2–38.5 ms (3.0×), by index by hand 12.4–12.6 ms (1.0×), **shelf resolving once (`from_rows`) 13.0–13.1 ms (1.0×)**, shelf resolving every row (`from_row`) 38.2–38.4 ms (3.0×). The derive costs about 4% over hand-written index code; resolving per row costs what lookups by name cost, which is why `fetch_*` and `from_rows` resolve once. | 5 |
| 14 | **The orphan rule forbids `impl TryFrom<&Row> for (A, B)`** outside tokio-postgres (E0117, "tuples are always foreign", captured on rustc 1.99.1), so tuples need a mapping trait of shelf's own. The sketch of Chapter 1 had said the orphan rule was not a reason. | 1 |
| 15 | **By name in a debug build costs 1.5–1.7× by position** (100,000 rows × 12 columns, best of 3, three runs: by name 197.8–203.3 ms, by position 120.4–129.2 ms; the build `book.py check` uses), and **the order of the `SELECT` list does not matter**: a lookup that matches at position *p* compares *p* + 1 names, so twelve fields compare 78 names per row in any order (measured: 202.7–206.9 ms reversed, 205.2–206.6 ms in struct order). The cost grows with the square of the column count. | 1, 5 |
| 16 | **A `literal` metavariable cannot be pasted as a tuple index** (`$t.$i`: "unexpected token: `literal` metavariable"); a `tt` can. And `${index()}` (metavariable expressions) is still unstable in rustc 1.99.1 (E0658, issue #83527). | 2 |
| 17 | **rustfmt leaves the inside of `model! { … }` alone**, but formats `model!( … )` when its contents parse. | 2 |
| 18 | **What a derive receives** (captured with a probe derive that stores its input as a string): doc comments, other attributes and its helper attributes, but not the `#[derive(…)]` line, and after `cfg` evaluation (a `#[cfg(any())]` field is gone). Its output is added after the item. | 3 |
| 19 | **`proc_macro` outside a proc macro**: `TokenStream::new()` works, `"…".parse::<TokenStream>()` panics with "procedural macro API is used outside of a procedural macro"; a `pub fn` in a proc-macro crate is refused ("`proc-macro` crate types currently cannot export any items other than functions tagged with …"). A panicking derive is reported as "proc-macro derive panicked" at the derive's name, with the message as a `help:`. | 3 |
| 20 | **quote! pastes a `usize` with its suffix** (`columns . 1usize`), which syn refuses as a tuple index ("expected unsuffixed integer"); `syn::Index::from(n)` pastes `1`. Hence `Index` in from_row.rs. | 3 |
| 21 | **sqlx 0.9, from its sources** (sqlx-core, sqlx-macros-core, sqlx-postgres 0.9.0): `#[derive(FromRow)]` is generic over `R: Row` with a `Decode` and a `Type` bound per field and reads each field with `__row.try_get("name")` per row; `PgRow` resolves a name through a `HashMap<UStr, usize>` the statement carries; it strips `r#` with `trim_start_matches`; tuples get `FromRow` for 1–16 elements, one macro invocation per size. SeaORM 2.0.4 depends on sqlx 0.9.0. | 1–3 |
| 22 | **`book.py` does not see fenced blocks inside a blockquote** (`FENCE_OPEN` wants the fence at the start of a line), so a runnable program inside a callout would never be compiled or run. Keep runnable blocks outside callouts. (A tooling finding; no chapter.) | — |
| 23 | **Forgetting the `#[derive(FromRow)]` gives the same error as forgetting `attributes(shelf)`**: "cannot find attribute `shelf` in this scope", with the note "`shelf` is in scope, but it is a crate, not an attribute" (rustc 1.99.1, probes in the scratchpad, the user crate depending on shelf). When the probe's own proc-macro crate, a direct dependency, had another derive declaring `shelf`, rustc instead noted "`shelf` is an attribute that can be used by the derive macro `DarlingRow`, you might be missing a `derive` attribute"; with shelf, whose derive is re-exported, it does not. | 4 |
| 24 | **A bound on a concrete field type in a derive's `where` clause** (`where std::time::Instant: ::shelf::FromColumn`, written by a probe derive) is reported once, but **at the derive's name**, not at the field, and **as the supertrait** ("the trait bound `Instant: …FromSqlOwned` is not satisfied"), without `FromColumn`'s `on_unimplemented` message. The same clause written by hand errors at the predicate, also as the supertrait. Hence `generics.rs` bounds only generic types, and an unreadable type is reported twice, at the field, in shelf's words (§10). | 4 |
| 25 | **Errors per attribute: syn stops, darling goes on.** `parse_nested_meta` returns at the closure's first error (syn 3.0.9, `meta.rs`: `logic(…)?`), so shelf reports at most one parse error per `#[shelf(…)]`; `#[shelf(renamed = "x", skip = 1)]` reports only `renamed`. `meta.error` spans from the key to the last token read (syn keeps a start and an end span and puts them on `compile_error!`'s path and braces). darling 0.24.1 (on syn 3; shelf's parser rewritten in a probe) reports both errors of such an attribute, suggests ("Unknown field: `renamed`. Did you mean `rename`?", default feature `suggestions`), lists enum values ("Unknown value: `kebab-case`. Available values: …"), and words the rest its own way ("Duplicate field `rename`", "Unexpected type `bool`" for `skip = true`); its `SpannedValue` spans the value, not the key. | 4 |
| 26 | **`#[shelf(json)]` on an `Option<T>` field reads `Json<Option<T>>`**: a JSON `null` gives `None`, but an SQL `NULL` fails with `Mapping::Null` ("… is NULL; read it into an Option", misleading here), because postgres-types 0.2.14's `Json<T>` keeps the default `from_sql_null` (`WasNull`) and only `Option<T>` overrides it. `Option<Json<T>>` without the attribute reads an SQL `NULL` as `None`. sqlx 0.9 has `json(nullable)` for this (§11). | 4 |
| 27 | **sqlx 0.9's attribute parsing, from sqlx-macros-core 0.9.0's source**: field keys `rename`, `try_from` (a quoted type only), `default`, `flatten`, `skip`, `json` and `json(nullable)`; the field closure has no branch for an unknown key; `parse_child_attributes(…).unwrap()` and `panic!` for impossible combinations, so a mistake in field attributes panics the derive; `flatten` + `try_from` and `json` + `try_from` are allowed (shelf refuses the latter); `rename_all` has seven cases (with `lowercase`, `UPPERCASE`, `kebab-case`); every field's type is bounded in the `where` clause (each bound mentions `R`). | 4 |

---

## 13. Next steps: phase S1b, step by step

**Resuming in a new session:** read this section first, then §4.0 (what exists) and §12 (what
was learnt). Each step is ticked when it is done and its gates are green, and a step in progress
says how far it got. The work is on the branch `shelf` and may not be committed yet
(`git status`); commit only when the user asks.

**Done before:** S0 (the scaffold), S1a (the mapping) and the sketches, all on 2026-11-07;
Chapters 0–3, written on 2026-11-09 ahead of this phase (§10). S1b changes `fetch.rs` (onto
`Executor`), which Chapters 0 and 1 use but do not quote; rerun `book.py --book orm check` after
it, and keep Chapter 1 §1.5's description of `fetch_all` true.
Each chapter's sketch lists the open questions to settle when it is written. S1a's steps were:
probes (§12 #4–#8), the test harness (`orm/shelf/tests/common/mod.rs`), `FromRow` complete, one
check per statement, expansion/UI/integration tests, and the benchmark (§12 #13).

**S1b, the model** (§4.2–§4.5, Chapters 6–9):

- [ ] **1. Probes** (in the scratchpad), each recorded in §12:
  - an `Executor` trait whose async methods return `impl Future + Send`, used from generic code
    inside an axum handler, inside `tokio::spawn`, and inside the main book's `in_transaction`
    retry helper (main book §12 #6, where capturing references broke `Send`). Do this before
    the trait's shape is fixed;
  - deadpool-postgres 0.14.2: `prepare_cached` on `Object` and on its `Transaction`, and the
    deref chain to `tokio_postgres::Client` (root CLAUDE.md, "Verified API facts");
  - statement strategies through lagproxy (`needs=lagproxy` style: `cargo run -p lagproxy --
    --delay-ms 5`): `prepare` then `query`, `prepare_cached` then `query`, and `query_typed`.
    Count round trips and look at `pg_prepared_statements`;
  - the `Patch` designs (§7, Chapter 9.2): whether design (c),
    `col = CASE WHEN $f THEN $v ELSE col END`, accepts a `None` of the field's type for an
    unset field, for every column type including a domain (main book §12 #1);
  - building SQL from another type's consts: `concat!` takes only literals, so an `Insert`
    whose table comes from `model = Product` needs `LazyLock<String>` (or the table repeated);
    decide which;
  - telling `Stale` from `NotFound` in one round trip for a versioned `UPDATE` (a CTE that
    reports whether the row exists).
- [ ] **2. `Executor`** in `shelf/src/executor.rs`: the calls shelf needs (`prepare` or
  `prepare_cached`, `query`, `query_one`, `query_opt`, `execute`), implemented for
  `tokio_postgres::{Client, Transaction<'_>}` and, behind a default feature `deadpool`, for
  `deadpool_postgres::{Client, Transaction<'_>}`. Move `fetch_*` onto it; keep the tests green.
- [ ] **3. `#[derive(Table)]`** (`shelf-codegen/src/table.rs`): `table`, `schema`,
  `primary_key` (composite keys as tuples), `generated`; identifiers quoted at macro time;
  `TABLE`, `COLUMNS` and the read SQL as consts; `find` (`Error::NotFound`), `find_opt`, `all`,
  `count`. `attrs.rs` learns the new keys; `FromRow` must ignore them (and stop rejecting
  `table`: update `tests/ui/unknown_attributes.rs`).
- [ ] **4. `#[derive(Insert)]`** (`model = Product`): `INSERT … RETURNING` the model's columns,
  database defaults, `generated` fields left out; `update(&self)`, `delete`, `delete_by_key`
  (zero rows → `NotFound`); upsert with `ON CONFLICT … DO UPDATE`.
- [ ] **5. `#[derive(Patch)]`**: measure the three designs (statements prepared, round trips),
  choose one and record why in §10; `version` (→ `Error::Stale`) and `updated_at`.
- [ ] **6. Tests** for each derive and attribute: expansion tests (`tests/expand/`), UI tests
  (`tests/ui/`), integration tests (one file per derive in `shelf/tests/`), deadpool included.
  Measurements recorded in §12 with what they were measured on.
- [ ] **7. Wrap-up:** all gates (both books); §4 rewritten as built for S1b; the status lines in
  this file, orm/CLAUDE.md and the root CLAUDE.md; then replace this checklist with S1c's
  (§9), and ask the user about a commit.
