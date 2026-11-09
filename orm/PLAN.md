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
integration tests and a benchmark. **Phase S1b (the model) done on 2026-11-09** (§4.0b): the
`Executor`, `#[derive(Table)]`, `#[derive(Insert)]` with upserts, `#[derive(Patch)]`, `version`
and `updated_at`, each probed or measured first (§12 #28–#33). **Phase S1c (queries and
relations) done on 2026-11-09** (§4.0c): the typed query builder, `has_many` and `belongs_to`
with batched loaders, `insert_many` and `copy_in`, streams of models, and `verify`, each probed
first (§12 #34–#41), with the N+1 and bulk measurements (§12 #42, #44–#46). Every chapter has a
**sketch** in its place ([SKETCHES.md](SKETCHES.md)); those of Chapters 6–13 describe the
design as built. **Chapters 0–4 were written on 2026-11-09**, ahead of S1b at the user's request
(§10): Chapters 1–4 are `current`, and Chapter 0 is a `draft` until its §0.3 runs (S3a, now
that S1c is built). **Phase S2 (the capstone) done on 2026-11-09** (§4.0d):
`orm/stockroom-shelf` serves stockroom's API with stores, products and orders on shelf, its
ported routes answering as stockroom's do (one test suite run against both routers), each step
probed or measured first (§12 #47–#56). **Phase S3a (Part I) done on 2026-11-09**: Chapter 5
written (its open questions settled in §10 and §12 #57–#62), Chapter 0's §0.3 a runnable
program, and Chapters 0–5 `current`. **Phase S3b (Part II) done on 2026-11-09**: Chapters 6–9
written and `current` (41 runnable examples, four through lagproxy), their open questions settled
in §10 and §12 #63–#68; a clearer error for `json` on a `Patch` field, with a test. The book PDF,
`orm/book/shelf-book.pdf`, shows the written chapters and the sketches. **Phase S3c (Part III)
done on 2026-11-09**: Chapters 10–14 and the appendix written and `current` (48 runnable examples
more, 141 in all; Chapter 14 shows the capstone through excerpts, its tests and a real session),
their open questions settled in §10 and §12 #69–#73. Writing Chapter 12 found a bug in
`insert_many` (a composite column, §12 #71): fixed, tested, and the bulk and capstone measurements
re-run. Every chapter is now `current`. The next phase is S4: polish and merge (§9, and §13 step
by step).

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

Items marked **(built, S1a)**, **(built, S1b)** or **(built, S1c)** exist and are tested; since
S1c, every item of the target design does. §4.0 describes what S1a built, §4.0b what S1b built,
§4.0c what S1c built, and §4.0d the capstone, built in S2.

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
| `from_rows`, `fetch_all`, `fetch_one`, `fetch_opt` | `shelf/src/row.rs`, `fetch.rs` | `fetch_*` take an `Executor` (since S1b), prepare, resolve against the statement (so a wrong mapping fails before the query runs, with no extra round trip), then query |
| `__private` | `shelf/src/lib.rs` | What generated code names: `column`, `column_at`, `get`, `get_try_from`, `tokio_postgres`, `Json`; since S1b also `param`, `flag`, `patch_sql`, and the compile-time joining `total_len`, `join`, `utf8` |
| `__private::Json<T>` | `shelf/src/json.rs` | What `#[shelf(json)]` reads through: postgres-types' `Json<T>`, except that an SQL `NULL` is deserialized as JSON `null`, so `T` decides (added 2026-11-09, §12 #26). Since S1b also what it writes through: `#[repr(transparent)]`, viewed in place with `Json::from_ref` |

### 4.0b As built: the model (S1b, 2026-11-09)

| Item | Where | What it is |
|---|---|---|
| `trait Executor` | `shelf/src/executor.rs` | `: Sync`; `prepare`, `query`, `query_one`, `query_opt`, `execute`, each returning `impl Future<Output = …> + Send`. Implemented by one `macro_rules!` for `tokio_postgres::{Client, Transaction}` (prepare every time) and, with the default feature `deadpool`, `deadpool_postgres::{Client, Transaction}` (`prepare_cached`; a pooled connection is passed as `&object`) |
| Attribute ownership | `shelf-codegen/src/attrs.rs` | `enum Derive { FromRow, Table, Insert, Patch }`, `KEYS` (each key's owner), `Derive::reports`: a derive reports only its keys' errors; `FromRow` also reports unknown keys and syntax mistakes; `Insert` and `Patch`, alone on their structs, report everything |
| `trait Table` and `#[derive(Table)]` | `shelf/src/table.rs`, `shelf-codegen/src/table.rs` | `type Key`; consts `MODEL`, `TABLE`, `COLUMNS`, `FIND`, `ALL`, `COUNT`, `UPDATE`, `DELETE`, `KEY_COLUMNS`, `VERSION`, `UPDATED_AT`, all literals written at macro time with identifiers quoted (`sql.rs`); `key()`, `key_params()`, `update_params()`; provided `find` (`NotFound`), `find_opt`, `all` (ordered by key), `count`, `update(&self)` (returns the row; `Stale` for a versioned model), `delete`, `delete_by_key`. Refuses a missing `table` or key, `flatten`, type parameters. `#[diagnostic::on_unimplemented]` for a type that is not a model |
| `trait Insert`, `trait Upsert`, `#[derive(Insert)]` | `shelf/src/write.rs`, `shelf-codegen/src/insert.rs` | `type Model: Table`; `INSERT` (and `UPSERT`) joined at compile time from the model's consts (`shelf/src/sql.rs`); `params()`; provided `insert`, `upsert`. `#[shelf(conflict)]` fields name the upsert's key; `Upsert` exists only with them |
| `trait Patch`, `#[derive(Patch)]` | `shelf/src/patch.rs`, `shelf-codegen/src/patch.rs` | `type Model: Table`; `COLUMNS`; `sql()` (a `LazyLock<String>` from `patch_sql::<Model>`); `params()` (a `flag` and a value per field, then the expected version); provided `apply(&db, &key)`. Every field an `Option`; a `#[shelf(version)]` field exactly when the model has one, checked by a `const` assertion |
| `version`, `updated_at` | the `Table` derive, `patch_sql` | `SET "version" = "version" + 1`, `"updated_at" = now()`; a versioned `UPDATE` is a CTE that also returns `shelf_updated` and `shelf_version`, read by `write::written` into the row, `Stale { current_version }` or `NotFound` (§12 #31) |
| `shelf::Error` | `shelf/src/error.rs` | `Mapping`, `NotFound { model }`, `Stale { model, current_version }`, `Db` |
| Tests | `orm/shelf/tests/`, `shelf-codegen/tests/expand/` | `executor.rs`, `table.rs`, `insert.rs`, `patch.rs` against PostgreSQL (deadpool included, through `TestDb::pool`); UI tests `table.rs`, `insert.rs`, `patch.rs` (every error the three derives report); expansions `table`, `insert`, `patch`, `versions`; the hygiene test with `Table` and `Insert` beside `type str` and `type u8` |

### 4.0c As built: queries, relations, bulk writes, streams and `verify` (S1c, 2026-11-09)

| Item | Where | What it is |
|---|---|---|
| `Column<M, T>` | `shelf/src/query/column.rs`, `shelf-codegen/src/columns.rs` | A quoted column name with a marker `PhantomData<fn() -> (M, T)>`; `Table` generates one per column as an inherent constant, named after the field in capitals, as visible as the field (`Product::PRICE_CENTS: Column<Product, i64>`); `T` is what the column holds: the field's type, a `try_from` source type, `serde_json::Value` for `json`. Conditions: `eq`, `ne`, `lt`, `le`, `gt`, `ge`, `between`, `any` (`= ANY($n)`, one array); `like` on text columns; `is_null`, `is_not_null` on `Option` columns; `contains` (`@> $n`, a one-element array) on array columns; `asc`, `desc` |
| `trait Value<T>` | `query/column.rs` | What a condition takes for a `T` column: `T`, `&T`, `&str` for `String`, `T` for `Option<T>`, `&str` for `Option<String>`; no numeric conversions, so a literal takes the column's type (§12 #34). `#[diagnostic::on_unimplemented]` |
| `Expr<M>`, `Raw<M>`, `trait Condition<M>` | `query/expr.rs` | A tree of conditions (`and`, `or`, `!` through `std::ops::Not`) owning its values (`Box<dyn ToSql + Sync + Send>`); `Expr::raw(sql).bind(v)` for SQL the builder does not model, its `$1…` renumbered (a scanner that skips quoted text, dollar quotes and comments; a placeholder without a value panics when the raw SQL becomes a condition). `Condition<M>` is what `filter`, `and` and `or` take, with its own message for another model's condition (§12 #41) |
| `Query<M>` | `query/mod.rs`, `query/render.rs` | `Table::query()`, then `filter` (several are ANDed), `order_by`, `limit`, `offset` (parameters), keyset `after(column, value)` (`"col" > $n`, ordered by it first); `all`, `first` (`LIMIT 1`), `count`, `exists` (both ignore order and paging), `stream`; `sql()` and `params()` show what is sent. Without `order_by`, key order. Rendered at each call; a deadpool connection caches each shape |
| Relations | `shelf-codegen/src/relations.rs`, `shelf/src/relations.rs` | Container keys `has_many(Child, key = fk, as = name)` and `belongs_to(Parent, key = fk[, as = name])`, owned by `Table`, several per model. `has_many` adds `parent.name(&db)` and `Parent::load_name(&db, &parents) -> Vec<Vec<Child>>` (aligned with `parents`); `belongs_to` adds `child.name(&db)` (`Result<Parent>`, or `Result<Option<Parent>>` when the field is written `Option<…>`) and `Child::load_name(&db, &children) -> HashMap<Key, Parent>`; the name defaults to the model's in snake case. Each loader is one query through the builder (`= ANY($1)`) and groups in a `HashMap`. Single-column keys only (a composite key is refused for `has_many`). Calls spanned on the `key = …` tokens |
| `insert_many`, `copy_in` | `shelf/src/bulk.rs`, `shelf-codegen/src/insert.rs` | Provided on `Insert`; the derive adds `COLUMNS`, `COPY` (joined at compile time) and `arrays(rows)`, one `Vec<&T>` per column. Both prepare the single-row `INSERT` first, for the column types and as the guard for #789 (§12 #37, #38). `insert_many`: `INSERT … SELECT unnest($1::"schema"."type"[]), … RETURNING …` (the `unnest`s in the select list since S3c, so a composite stays whole, §12 #71); an array column is `Error::Unsupported`; no columns: `SELECT FROM generate_series`. `copy_in`: binary COPY with the `INSERT`'s types, returns the count |
| Streams | `shelf/src/stream.rs` | `fetch_stream::<T>(&db, sql, params)` and `Query::stream(&db)` return `ModelStream<'a, T>`, a `Stream` of `Result<T, Error>`: resolved once from the statement, then each row as it arrives, over `query_raw`. It borrows the executor, so a pooled connection cannot go back to the pool mid-stream (UI test `stream.rs`) |
| `verify::<M>(&db)` | `shelf/src/verify.rs` | Prepares `SELECT * FROM` the table without the statement cache (`Executor::prepare_uncached`, §12 #43), checks each field of `Table::FIELDS` (hidden: the column and the type it reads, `Field::new::<T>`), then asks `pg_attribute` which of the non-`Option` fields' columns can be NULL (tables only, not views). `Error::Schema { model, mismatches }`, every mismatch at once, in field order |
| `Executor` | `shelf/src/executor.rs` | Gained `query_raw` (streams), `copy_in` (COPY) and `prepare_uncached` (a default, `prepare`; deadpool's types override it to skip the cache). It is not sealed: the relations test counts round trips with an executor of its own |
| `shelf::Error` | `shelf/src/error.rs` | Gained `Schema { model, mismatches: Vec<Mismatch> }`, `Unsupported { model, message }`, and `Mapping::Nullable` |
| Tests | `orm/shelf/tests/`, `shelf-codegen/tests/expand/` | `query.rs`, `relations.rs`, `bulk.rs`, `stream.rs`, `verify.rs` against PostgreSQL; UI tests `query.rs`, `relations.rs`, `stream.rs`; expansions `columns`, `relations`; the hygiene test with relations through a raw-identifier foreign key; benchmarks `bench_relations` and `bench_bulk` (§12 #42, #44) |

### 4.0d As built: the capstone, `orm/stockroom-shelf` (S2, 2026-11-09)

| Item | Where | What it is |
|---|---|---|
| The crate | `orm/stockroom-shelf/` | A library and a binary depending on `stockroom` and `shelf`. Reused from stockroom unchanged: the migrations, the domain types, `AppError`, `in_transaction`, `CancelOnDrop`, `AppState` and the `Db`, `AppJson` and `AppQuery` extractors, `etag` and `if_match`, the request and response types of orders, and stockroom's router for every route not ported |
| Models | `src/repo/{stores,products,stock,orders}.rs` | `Store` and `NewStore`; `Product`, `NewProduct` and `ProductPatch` (its `version` set by `update`, `#[serde(skip)]`); `Stock` (composite key, `available: Option<i32>`, `generated`); `OrderRow` (`belongs_to(Store)`, `has_many(LineRow, as = lines)`, `#[shelf(json)] customer`), `NewOrderRow<'a>` (a borrowed `json` field), `LineRow` (`belongs_to(Product)`, composite key) and `NewLineRow`. Each derives serde beside shelf, and stores' and products' models are their JSON |
| The repository | `src/repo/` | stockroom's functions for the ported routes, over `&impl shelf::Executor`: `stores::{list, create, by_code}`, `products::{page, by_sku, create, update}`, `orders::{place, get}`; `verify_models` checks the five models, pipelined, every error at once. `place` keeps the reservation's SQL (`RESERVE`), pipelined in product-id order, and inserts the lines with `insert_many` |
| Errors | `src/error.rs` | `app_error`, used with `map_err` (§10, §12 #49) |
| The router | `src/http/` | stockroom's handlers for the ported routes, calling this repository; `/products/stream` runs shelf's `ModelStream` inside a body that owns the connection (§12 #52); the static paths `/products/export.csv`, `/products/import` and `/orders/events` go to stockroom by name, the rest by fallback (§12 #50) |
| The binary | `src/main.rs`, `src/app.rs` | `stockroom-shelf verify` (exit status 1 and every mismatch) and `stockroom-shelf serve` (stockroom's `serve`, with `verify` before the first request); migrating and seeding are stockroom's commands |
| Tests | `tests/` | `routes.rs`: 11 scenarios against both routers (§12 #53), the statement-cache check, the client that hangs up mid-stream; `concurrency.rs`: the oversell and deadlock tests on the port, and 30 concurrent patches without `If-Match`; `models.rs`: `verify` on stockroom's schema, and on one that moved on |
| Measurements | `examples/line_for_line.rs` | Statements prepared and run, and round trips, per ported function, through a message counter and lagproxy (§12 #54); lines counted per item (§12 #55) |

### 4.1 Crates

| Crate | Kind | Role |
|---|---|---|
| `orm/shelf` | lib | The runtime: the traits, the executor, errors, the query builder, the relation loaders, bulk writes, streams and `verify`. It re-exports the derives, and `__private` re-exports what generated code needs (tokio-postgres) so that users' crates do not have to depend on it directly. |
| `orm/shelf-macros` | proc-macro | A thin shell: `parse_macro_input!`, then `shelf_codegen::<derive>::expand`, then `into_compile_error` on failure. |
| `orm/shelf-codegen` | lib | Everything the derives generate, as functions `expand(&DeriveInput) -> syn::Result<TokenStream>`. Unit and expansion tests run here. |
| `orm/playground` | lib + generated examples | The book's runnable examples and helpers. |
| `orm/stockroom-shelf` | lib + bin **(built, S2)** | The capstone: stockroom's schema, migrations and domain types (`Sku`, `Address`, `OrderStatus`, `Cents`, `Quantity`, `Customer`) reused through the `stockroom` crate, a repository layer on shelf, and the same axum routes for stores, products and orders, with stockroom's router behind them for the rest (§4.0d). |

**Why three crates:** a proc-macro crate can export only macros, so code generation that lives
in one cannot be called by unit tests in another crate, by the book's examples, or by anything
else. In a plain library it can. The cost is one more crate, and one crate boundary that the
book explains (Chapter 3).

### 4.2 Derives

| Derive | Goes on | Generates | Ch |
|---|---|---|---|
| `FromRow` **(built, S1a)** | a struct with named fields | `impl FromRow`: `Columns`, `resolve` and `from_row_at` (§4.0) | 3–5 |
| `Table` **(built, S1b, S1c)** | a `FromRow` struct | `impl Table` (§4.0b) with `FIELDS` for `verify`, and an inherent impl with the column constants (`Product::SKU`) and the relation methods (§4.0c) | 6, 8–13 |
| `Insert` **(built, S1b, S1c)** | a `New…` struct | `impl Insert` with `type Model`, the `INSERT … RETURNING` SQL and the parameters in order, the columns, the `COPY` and one array per column for the bulk writes; and `impl Upsert` with `conflict` fields | 8, 12 |
| `Patch` **(built, S1b)** | a struct of `Option` fields | `impl Patch`: design (c), one statement for every patch (§10) | 9 |

`FromRow` also covers tuples, `(A,)` up to 12 elements, read by position **(built, S1a)**.
These impls are written with a `macro_rules!` macro (Chapter 2).

### 4.3 Attributes

The container attributes (`#[shelf(…)]` on the struct):

| Attribute | On | Meaning |
|---|---|---|
| `table = "products"`, `schema = "inventory"` **(built, S1b)** | `Table` | The table, quoted at macro time |
| `model = Product` **(built, S1b)** | `Insert`, `Patch` | The model that `RETURNING` maps into, which also gives the table |
| `rename_all = "…"` **(built, S1a)** | `FromRow` | A naming convention for columns: four cases, written by hand |
| `belongs_to(Store, key = store_id)`, `has_many(OrderLine, key = order_id, as = lines)` **(built, S1c)** | `Table` | Relations. `has_many` needs `as`: no pluralising guesswork; `belongs_to` defaults to the model's name in snake case |

The field attributes:

| Attribute | Meaning |
|---|---|
| `rename = "col"` **(built, S1a)** | Read the column `col` |
| `skip` **(built, S1a)** | Do not read; use `Default::default()` |
| `flatten` **(built, S1a)** | A nested `FromRow` read from the same row |
| `json` **(built, S1a)** | Read through `Json<T>`; an SQL `NULL` reads as JSON `null`. `Insert` and `update` write through it. A `Patch` refuses the key and needs none: a field of type `Option<tokio_postgres::types::Json<T>>` or `Option<serde_json::Value>` writes JSON itself (settled in S3b, §10) |
| `try_from = SqlType` **(built, S1a)** | Read `SqlType`, then `TryFrom` into the field. The type may also be quoted, as serde writes it |
| `primary_key` **(built, S1b)** | Part of the key; several fields make a composite key (a tuple) |
| `generated` **(built, S1b)** | The database fills it (`uuidv7()`, `now()`, identity): never written by shelf |
| `version` **(built, S1b)** | On a model: optimistic locking, `WHERE … AND version = $n`, `SET version = version + 1`, and `Stale`. On a patch: the version it was made against |
| `updated_at` **(built, S1b)** | `SET updated_at = now()` on every update |
| `conflict` **(built, S1b)** | On an `Insert` struct: part of the unique key an upsert collides on |

### 4.4 The API in use (illustration)

All of it is built (S1b, S1c), and Chapter 0's §0.3 runs it. Three details differ from the
first design: `load_lines` returns a `Vec` per order, aligned with the slice; binary COPY is
`NewProduct::copy_in`; and `insert_many` refuses an array column such as `tags` (§12 #37), so
§0.3 writes its batch with `copy_in`.

~~~rust
// shelf
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
NewProduct::insert_many(&db, &batch).await?;                  // unnest: Error::Unsupported here (tags)
NewProduct::copy_in(&db, &batch).await?;                      // binary COPY
shelf::verify::<Product>(&db).await?;                         // the struct against the table
~~~

### 4.5 The executor and errors

- **`Executor`** (built, S1b). Neither `tokio_postgres::GenericClient` nor deadpool's version
  can be implemented outside its crate (both are sealed), and only deadpool's has
  `prepare_cached`. shelf therefore defines a trait of its own, with the calls it needs. Its
  methods return `impl Future<Output = …> + Send`. It is implemented for `tokio_postgres::{Client,
  Transaction<'_>}`, which prepare every time, and, behind the default feature `deadpool`, for
  `deadpool_postgres::{Client, Transaction<'_>}`, which use the connection's statement cache.
  The three strategies were measured through lagproxy (§12 #29); `query_typed` is not used.
  S1c added `query_raw` (streams), `copy_in` (COPY) and `prepare_uncached` (`verify`).
- **`shelf::Error`**: `Mapping`, `Db` (S1a), `NotFound { model }` and `Stale { model,
  current_version }` (S1b), `Schema { model, mismatches }` from `verify` and `Unsupported {
  model, message }` from the bulk writes (S1c).
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
| `json`, `try_from`, `flatten`, `rename`, `skip` | 4 | `types::Json<T>`, `FromSql` + `TryFrom` | 3.5, 3.7, 5 |
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
| Query builder | 10 | `$n` parameters, `= ANY($1)`, `@>`, keyset pagination | 2.3, 3.8, 9.6 |
| Relations | 11 | arrays as parameters, pipelining with `try_join!` | 3.8, 7 |
| `insert_many` | 12 | `unnest` of arrays, casts for domains | 10, 5 |
| Binary COPY from the derive | 12 | `BinaryCopyInWriter`, types from a prepared statement | 10 |
| Streams of models | 12 | `query_raw`, `RowStream`, backpressure | 9 |
| `verify` | 13 | `prepare`, `Statement::columns()`, the catalog for nullability | 4, 5 |
| The capstone | 14 | deadpool, axum, `in_transaction`, `CancelOnDrop`; a domain, a composite, an enum, `jsonb` and a virtual column through shelf | 13–18 |

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
    (captured with rustc 1.99.1; no `rust-toolchain` file pins it), and read the diff.
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

There are 15 chapters and an appendix, in three parts, at about 600–1,000 lines each. That is a
guide, not a limit: a chapter whose material runs longer stays longer (decided 2026-11-09;
Chapter 4 runs to about 1,780 lines). Each
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

**Chapter 5 — Checking a statement once, not every row** (written 2026-11-09)

- 5.1 The answer belongs to the statement: every row shares its statement's column list
  (§12 #57), so positions and types are per statement, decoding per row.
- 5.2 A statement knows its columns: `prepare`, `columns()` before any row, the
  *RowDescription*; the two-step trait as a stage program (`Columns`, `resolve`,
  `from_row_at`, a `fetch_all`), exact names only.
- 5.3 Types checked once: `accepts` per column, shelf's `column` and `check`; an `INSERT …
  RETURNING` that does not fit never runs; what is left per row (`Null`, `Decode`).
- 5.4 What the derive generates: the stage and the expansion side by side, a resolved
  `Columns` printed (`(2, (1, 0), ())`), the trait's bounds (§12 #60), `from_rows` and its
  empty-slice pitfall.
- 5.5 Measured: `bench_mapping`'s table, and a debug-build program (§12 #58, #59).
- 5.6 Where shelf resolves: `fetch_*`, the model methods, streams, `verify`; once per call, not
  once per connection (§10).
- Side by side: reading 100,000 rows (by hand with positions found once, shelf, sqlx's map per
  statement, §12 #61).
- In shelf: `get` and `decode_error` in `shelf/src/row.rs`.
- Exercises: one statement and two executions; two columns of one name (§12 #61); positions from
  another statement (§12 #62).

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

- 12.1 `insert_many`: one `INSERT … SELECT unnest($1::t[]), …` with one array per
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
| **S1b. shelf: the model** (done 2026-11-09) | `Table`, `Insert` and `Patch`; `Executor` with the `deadpool` feature; `shelf::Error`; CRUD and upsert; `version` and `updated_at`; the `Patch` design chosen by measurement | The gates pass; the statement-strategy and `Patch` measurements are in §12 |
| **S1c. shelf: queries and relations** (done 2026-11-09) | The query builder, relations, `insert_many` and COPY, streams, and `verify` | The gates pass; the N+1 and bulk tables are in §12; the query builder's compile errors have UI tests |
| **S2. stockroom-shelf** (done 2026-11-09) | The capstone app and its tests | The gates pass; the ported routes give the same responses as stockroom's |
| **S3a. Part I** (done 2026-11-09; Chapters 0–4 written ahead of S1b) | Chapters 0–5 | `book.py --book orm check` passes, including excerpts and links; the chapters are `current` |
| **S3b. Part II** (done 2026-11-09) | Chapters 6–9 | The same |
| **S3c. Part III** (done 2026-11-09) | Chapters 10–14 and the appendix | The same |
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
| SQL for model CRUD | literals generated at compile time; SQL that needs another type's names joined at compile time by `const fn`s (§12 #32; `LazyLock` was the earlier plan) | building SQL at run time on every call |
| The capstone | a port of part of stockroom, reusing the `stockroom` crate | a new domain |
| Chapters not written yet | a sketch in each chapter's place, in the PDF (as *Decisions in the Loop* does; requested 2026-11-07) | sketches in a separate document; planned chapters left out |
| Writing Chapters 0–4 before S1b | done on 2026-11-09, at the user's request: they quote only what S1a built (`FromRow`, the tuples, `fetch_*`, `pretty`, and for Chapter 4 `attrs.rs`, `case.rs`, `generics.rs`, `FromColumn` and the UI tests), and `book.py excerpts` flags any drift when S1b changes those files. S1b's `table` key changes what Chapter 4 says about it (§4.2, "One namespace") and `unknown_attributes.stderr` | waiting for S1b–S2, as §9 orders |
| An unreadable field type is reported twice (Chapter 4, §4.5–4.6) | kept: once per generated use (`column`, `get`), each in `FromColumn`'s words at the field. Bounding concrete field types in the `where` clause reports it once, but at the derive's name and in the driver's words (§12 #24) | one bound site per field |
| Two fields that read the same column | allowed: reading one column twice is odd but legal (a `jsonb` as a typed value and as `serde_json::Value`). The check is Chapter 4's Exercise 2 | refusing it in the derive |
| An SQL `NULL` in a `#[shelf(json)]` column | read as JSON `null`, through shelf's own `__private::Json<T>` (chosen 2026-11-09): `Option<T>` takes both nulls as `None`, as any `Option` field takes a NULL; a type that refuses `null` fails with `Mapping::Null`, whose advice then fits; reading `Option<Json<Option<T>>>` without the attribute still tells the two apart | a `json(nullable)` key, as sqlx has; a better error message only |
| darling in Chapter 4 | shelf's parser rewritten in darling 0.24.1 in a scratch probe, shown as an illustration with the errors it produced (§12 #25); darling is not a dependency | none |
| The executor's shape (S1b) | `trait Executor: Sync` with methods returning `impl Future<Output = …> + Send`, implemented for the two tokio-postgres and the two deadpool types (§12 #28); shelf's own async methods are written the same way | `async fn` in the trait (its futures are not known to be `Send`); `#[async_trait]` (boxed futures) |
| Statement strategy per executor (S1b) | a plain `Client` or tokio-postgres `Transaction` prepares, then queries (two round trips, as `query(&str)` costs anyway); a deadpool connection or transaction uses `prepare_cached` (one round trip after the first); measured in §12 #29 | `query_typed` (one round trip, but it needs PostgreSQL types shelf cannot know, and no check before the query); a cache of shelf's own for a plain `Client` (Chapter 7's exercise) |
| SQL that names another type's table (S1b) | built at compile time by two `const fn`s in `__private` (§12 #32): no `LazyLock`, no dependency. `Insert` refuses a model that mentions its own type parameters | `LazyLock<String>` (§10's earlier default, which fails in the same generic case); repeating the table on the `New…` struct; the `const_format` crate |
| Which derive reports an attribute's error (S1b) | every derive parses every key; each key has an owner in `attrs.rs` (`KEYS`), and only the owner reports its errors. Unknown keys and syntax mistakes go to `FromRow`, which every `Table` has; `Table` generates nothing for a struct `FromRow` refuses. A struct with both derives hears about each mistake once | every derive reporting everything (each mistake twice); one derive that knows the others (impossible: a derive does not see the `#[derive]` list, §12 #18) |
| The `Table` trait (S1b) | `type Key` (the field's type, or a tuple in field order); consts `MODEL`, `TABLE` (quoted, schema-qualified), `COLUMNS`, `FIND`, `ALL` (ordered by the key), `COUNT`; `key(&self)` (clones the key fields), `key_params(&Key)`; provided `find` (`NotFound`), `find_opt`, `all`, `count`. `find` takes `&Key`, a composite one as `&(a, b)` of owned values. `count` lives here, not in the query builder | `LazyLock` SQL; keys as tuples of references |
| What `Table` refuses (S1b) | a model without `table` or without a `primary_key`; `flatten` fields (a flattened struct's columns are not known at macro time); type parameters (the SQL names one table). **`flatten` settled in S3b (Chapter 6, §6.2):** kept refused. `FromRow` could publish its column list as a constant for `Table` to join at compile time (§12 #32), but a trait constant must exist for every impl, and a generic `FromRow` that flattens `T` would join in a generic impl (E0401, captured for §6.5) | `FromRow` exposing its column list as a const, which a generic `flatten` could not have |
| Writes (S1b, step 4) | `Insert` on a `New…` struct (`model = Product`): `INSERT … RETURNING` the model's columns, joined at compile time; the struct may borrow but not have type parameters, and `Insert` reports every key's error, being the only shelf derive on its struct. `update(&self)` writes every column that is neither key nor `generated` and returns the row (`UPDATE … RETURNING`; with nothing to write, it reads the row); `delete`, `delete_by_key`; zero rows is `NotFound`. An upsert's target is the fields marked `#[shelf(conflict)]`, and a separate `Upsert` trait exists only when there are some (`ON CONFLICT (…) DO UPDATE SET` every other column; with none, the key is set to itself so the row is still returned). `json` fields are written through `__private::Json`, a `#[repr(transparent)]` view of the field (`Json::from_ref`), so that parameters still borrow from the struct. `Table` carries `#[diagnostic::on_unimplemented]` for a `model` that is not one. Generated code spells primitives `::core::primitive::str` | an upsert target given as an argument; `DO NOTHING` (no row returned); boxed, owned parameters |
| The `Patch` design (S1b, step 5) | (c), `col = CASE WHEN $f THEN $v ELSE col END`: one statement for every patch, NULLs included, domains typed as their base type (§12 #30, #33). Its statement numbers the key's parameters after its own, and the key's length is the model's, so it is built once, at first use, in a `LazyLock<String>` by a function in shelf from the model's constants: the case where `LazyLock` is the right tool | (a), a statement per shape; (b), `coalesce`, which cannot set NULL; `const fn`s that write `$n` numbers |
| `ON CONFLICT DO NOTHING` (Chapter 8's open question, S3b) | no method: on a conflict it returns no row, so the method would return `Option<Model>` and `None` would not say which row was in the way; reading it takes a second query and a race. A `New…` struct with only its conflict key upserts with `SET key = excluded.key`, which returns the existing row in the same statement. Chapter 8's Exercise 1 writes `insert_if_absent` on the public constants | an `insert_if_absent` method returning `Option` |
| `json` fields on a `Patch` (S3b, Chapter 9) | no key: a field of type `Option<tokio_postgres::types::Json<T>>` or `Option<serde_json::Value>` writes JSON through its own `ToSql`, as the capstone's `ProductPatch` already did; the derive refuses `json` with that advice (the message changed in S3b; `tests/patch.rs` `a_json_column_is_patched_through_json`) | a `json` key that writes through `__private::Json`, which would need a view of an `Option<T>` as `Option<Json<T>>` |
| Reading `null` into `Option<Option<T>>` (S3b, Chapter 9) | a one-line `deserialize_with` written in the chapter (`Option::<T>::deserialize(d).map(Some)` with `#[serde(default)]`); serde_with 3.24.0 ships it as `serde_with::rust::double_option`, named, not added | serde_with as a dependency of the playground; a derive attribute of shelf's |
| `flatten` in a `Table` (S3b, Chapter 6) | refused, as in S1b; Chapter 6 §6.2 explains why (the names live in another item; a `FromRow` constant of its column list would be impossible for a generic struct that flattens `T`, E0401) | `FromRow::COLUMN_NAMES` joined at compile time |
| Chapter 0's §0.3 (the API in ten lines) | a runnable program since S3a (2026-11-09), against temporary tables: `insert`, `find`, `copy_in`, the query builder (its SQL printed), `load_lines`, `verify`. The batch goes through `copy_in`, because `insert_many` refuses the array column `tags` (§12 #37); the bullets say so | the illustration it was until S1c; dropping `tags` to show `insert_many` |
| Caching `Columns` per statement (Chapter 5's open question) | no: `resolve` runs on every call, prepared or cached. One `resolve` of 12 fields costs 262–268 ns, a localhost round trip 46–47 µs (§12 #59); a cache would need a map per connection keyed by statement and type, and staleness rules | a `Columns` cache beside deadpool's statement cache; `fetch_*` taking a `Statement` (Chapter 5's Exercise 1 does it by hand) |
| `FromRow::Columns`' bounds | `Send + Sync + 'static`, kept: only `Send` is required by shelf's code (§12 #60); the others hold for any tuple of positions and promise what a cache would need | `Send` alone |
| Two result columns with one name | the first is read, as `Row::try_get` does; not refused (a join's two `id`s are legal, and the struct may read neither). A precise check (only names the struct reads) would walk the whole list per field; Chapter 5's Exercise 2 writes a coarse one | refusing every duplicated name; refusing a duplicated name the struct reads |
| The query builder's values (S1c) | a trait of shelf's own, `Value<T>`: the column's type, a reference to it, `&str` for text, a plain value for a nullable column; no numeric conversions (§12 #34) | `impl Into<T>` (a literal falls back to `i32`, so `3_000_000_000` stops compiling); widening impls (the same) |
| Column constants (S1c) | inherent constants on the model, the field's name in capitals, as visible as the field; they shadow a `Table` constant of the same name, which shelf always names in full (§12 #35) | a generated module (diesel's `products::sku`); an enum of columns (SeaORM's `Column::Sku`) |
| A query's values (S1c) | owned, `Box<dyn ToSql + Sync + Send>` (one allocation each), so a query is built in one function and run in another, across an `.await` and `tokio::spawn`; `LIMIT` and `OFFSET` are parameters, so a page size is not a new statement | borrowed values with a lifetime on `Query` |
| A query's order (S1c) | key order when there is no `order_by`, as `all` returns rows; keyset `after(column, value)` on one unique column | no `ORDER BY` (pages in no stable order); row comparisons for a composite cursor (an exercise) |
| Raw SQL in the builder (S1c) | `Expr::raw(sql).bind(value)`, its `$1…` renumbered by a scanner that skips quoted text; a placeholder with no value panics where the condition is built | `?` placeholders (`?` is a `jsonb` operator); no raw SQL (every function modelled) |
| `count` (S1c, Chapter 6's question) | on `Table` (every row) and on `Query` (the filtered rows); `exists` on `Query` | one of the two |
| Relations (S1c) | generated methods that call loaders in shelf, each one query through the builder; `has_many` returns a `Vec` per parent, aligned with the slice; `belongs_to` a `HashMap` by key (each parent loaded once), and an `Option` when the foreign key field is written `Option<…>`; single-column keys; names explicit for `has_many` | a `HashMap` for both; composite foreign keys (an exercise); lazy loading (never: §7, 11.6) |
| The column types for `insert_many` and `copy_in` (S1c) | learnt from the struct's own `INSERT`, prepared: each column's type as PostgreSQL types the parameter, so a batch accepts what `insert` accepts; preparing it is also the guard against #789 (§12 #37, #38) | an attribute naming SQL types; a prepared `SELECT … LIMIT 0` (the sketch's plan: it describes a domain column as its base type, which a domain newtype refuses: postgres-derive 0.4.9's `accepts` wants the domain's name and `Kind::Domain`) |
| An array column in `insert_many` (S1c) | `Error::Unsupported`, before anything is sent, pointing to `copy_in` | a silent fallback to a loop |
| What `insert_many` returns (S1c) | the rows, as the model (`RETURNING`), in the order PostgreSQL returns them, which in practice is the slice's | the count only (`copy_in` returns the count) |
| Streams (S1c, Chapter 12's question) | `ModelStream<'a, T>` borrows the executor: a pooled connection cannot be returned with rows on the way (UI test); an HTTP body that must own its connection is the capstone's to build, as stockroom's `CancelOnDrop` | a stream that owns a pooled connection (Chapter 14) |
| `verify` (S1c, Chapter 13's questions) | `SELECT * FROM` the table, prepared without a cache; type and name from `Table::FIELDS`, NULLs from `pg_attribute` for tables only (a view's columns are not checked for NULLs); every mismatch in `Error::Schema`, in field order; `New…` structs are not checked | the model's own `SELECT` (stops at the first missing column); reporting a view's columns as unknown; checking `NOT NULL` columns without defaults against `New…` structs (an exercise) |
| What the capstone ports (S2) | stores (`GET`, `POST /stores`), products (`GET`, `POST /products`, `GET`, `PATCH /products/{sku}`, `POST /products/batch`, `GET /products/stream`), orders (`POST /orders`, `GET /orders/{id}`). Stock levels, adjustments and transfers, order transitions, the CSV import and export, events, reports and health stay stockroom's: their statements are the point of them (an adjustment adds to `on_hand`, which a shelf upsert cannot say) | porting only reads (the sketch's proposal); porting everything |
| How the two routers meet (S2) | the port's routes first, then `route_service` with `stockroom_router.into_service()` for each static path a ported `{param}` would capture, then `fallback_service(stockroom_router)`; the port's timeout and trace layers are `route_layer`s, so stockroom's routes pass through stockroom's layers alone (§12 #50) | copying stockroom's whole router; making stockroom's handler modules public |
| shelf's errors in the API (S2) | `stockroom_shelf::app_error(shelf::Error) -> AppError` with `map_err`, since the orphan rule refuses the `From` impl (§12 #49): `NotFound` → 404 with stockroom's word for the model, `Stale` → 412, `Db` through `AppError::from`, the rest 500 | an error type of the port's own (but `in_transaction` and the `Db` extractor speak `AppError`) |
| The orders' types (S2) | stockroom's request and response types (`NewOrder`, `Order`, `OrderLine`), reused, and models named after their tables' rows (`OrderRow`, `LineRow`, `NewOrderRow`, `NewLineRow`); the response is assembled from them (`view`). Stores and products serialize their models directly, with the same JSON | response types of the port's own |
| A `PATCH` without `If-Match` (S2) | read the row (the route has the SKU, `apply` wants the key), patch at the version read, and on `Stale` read and patch again (§12 #51) | a `version: Option<i32>` meaning "any" in shelf's `Patch` (`coalesce($n, version)`): a change to Chapter 9's design, left as an exercise |
| `Stock.available` (S2) | `Option<i32>`, as the catalog has it (§12 #48) | `ALTER … SET NOT NULL` on stockroom's virtual column (a change to the main book's schema) |
| `orders::get` and `place` (S2) | `get`: the order and its lines pipelined, then the store and the products pipelined, 2 round trips (the relation methods in sequence would take 3; raw SQL with joins and `FromRow`, 1, is the escape hatch Chapter 14.4 shows); `place` builds its response from the `RETURNING` rows, with no read-back (§12 #54) | the relation methods; stockroom's read-back |
| The capstone's tests (S2) | stockroom's harness by `#[path]` (its template, its databases `stockroom_test_<uuid>`), one scenario run against both routers (`both`), the statement cache to show which handler answered, stockroom's oversell and deadlock tests on the port (§12 #53) | a harness of the port's own, in a schema per test (shelf's way) |
| The `$n` scanner in Chapter 10 (S3c) | shown finished, in an excerpt of its unit test, after a stage program whose naive renumbering changes a string literal (`'was $5'`): every branch of the scanner is one case of the test, and building it step by step would be a lexer lesson | built step by step |
| Composite foreign keys (Chapter 11's open question, S3c) | an exercise, not a feature: Chapter 11's Exercise 3 loads `stock_movements` for `stock (store_id, product_id)` with a raw fragment, `("store_id", "product_id") IN (SELECT * FROM unnest($1::int8[], $2::int8[]))`, grouped by the key tuple; shelf keeps refusing `has_many` on a composite key | a `has_many` that generates two arrays and the row comparison |
| Where `insert_many`'s `unnest`s go (S3c, Chapter 12) | in the select list, `SELECT unnest($1::…[]), unnest($2::…[])`: a composite value stays whole there, and set-returning functions in a select list run in step (PG 10+); test `a_composite_column_goes_through_insert_many_whole` (§12 #71) | `SELECT * FROM unnest(…)` in `FROM`, S1c's form and pgload's, which expands a composite into its fields |
| Chapter 12's timing assertion | a factor of ten for both `insert_many` and `copy_in` against the loop, on 10,000 rows in a debug build, where the measured factors are 50 and 125 (§12 #71) | the sketch's "at least ten times" for COPY only |
| Chapter 13's Exercise 1 (its open question, S3c) | a `Vec` of boxed futures, `verify::<M>(db).boxed()` per model, polled with `join_all`: `Table` has associated constants and is not dyn-compatible, so there is no trait object of models; the list of futures is the list of models | a list of function pointers returning boxed futures; a `macro_rules!` that writes the `join!` |
| Chapter 14's programs (its open questions, S3c) | none runnable, as in the main book's Chapter 18: the port shown through excerpts, its tests and `line_for_line`, a real session pasted (`migrate`, `seed`, `verify`, `serve`, three `curl`s), and the exercises' solutions in prose; `place` printed trimmed to what moved to shelf, its validation elided | the playground depending on stockroom-shelf; `place` in full (86 lines, mostly stockroom's validation) |
| Where the comparison with other crates lives | a short table in §0.2 (who writes the SQL, when a mapping mistake is caught), toasty in prose; the appendix's phrasebook maps API names | the full table in the appendix |
| Chapter 1's fetch helpers | the simple form (`query`, then map); shelf's prepare-first version is described and left to Chapter 5 | preparing first already in Chapter 1 |
| Chapter 3's stage generator | new code that writes Chapter 1's impl (`from_row` with `try_get` by name), so the stage reads as Chapter 1 automated; the excerpts of the finished generator elide what Chapters 4 and 5 add | reproducing S0's generator, which was not kept |
| Seeing an expansion | `pretty` first; `cargo expand` recommended too, since it runs on stable (§12 #7) | `pretty` only |

---

## 11. Risks and items to verify

- **Drift between the build-along text and the finished crate.** §3 handles this. Shape the
  crates to grow (§3, point 5) from S1a on.
- **`Send` futures from the `Executor`** when used in generic code inside axum handlers and with
  the retry helper (main book §12 #6). Resolved in S1b (§12 #28); S1c's inherent `async fn`s
  (the builder, the loaders) are `Send` for a generic executor too, checked by compile-only
  functions in `tests/query.rs` and `tests/relations.rs`.
- **trybuild's `.stderr` files** change when rustc's wording changes. They were captured with rustc
  1.99.1 (the repository has no `rust-toolchain` file), and are regenerated deliberately.
- **syn 3 is recent.** Most examples online show syn 2. Resolved for Chapter 3: the
  differences a derive meets are two fields of `Field` (§12 #3, #5).
- **`cargo expand`**: resolved, it runs on stable (§12 #7).
- **`Patch` design (c)** sends a typed value for every field, even unset ones. Resolved in S1b:
  a typed `None` serialized for every column type tried, domains included (§12 #30).
- **Query builder ergonomics:** resolved in S1c (§12 #34): `&str` works for text columns, and an
  integer literal takes the column's type because shelf converts no numbers.
- **Nullability is not in a row description.** Resolved in S1c (§12 #40): the catalog, through
  `table_oid()` and `column_id()`, for a table's own columns.
- **Two sessions in one working tree.** On 2026-11-09 a second Claude session resumed S1c from
  this one's transcript while it was still running, and overwrote `examples/bench_bulk.rs` (its
  version, closer to pgload, was kept). Before resuming a phase, check that no other session is
  working in the repository (`git status`, file times).
- **Size.** 15 chapters and an appendix. Write and verify one chapter at a time. Length follows
  the material (§7): do not cut verified content to meet the guide.

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
| 26 | **`#[shelf(json)]` on an `Option<T>` field reads `Json<Option<T>>`**: a JSON `null` gives `None`, but an SQL `NULL` fails with `Mapping::Null` ("… is NULL; read it into an Option", misleading here), because postgres-types 0.2.14's `Json<T>` keeps the default `from_sql_null` (`WasNull`) and only `Option<T>` overrides it. `Option<Json<T>>` without the attribute reads an SQL `NULL` as `None`. sqlx 0.9 has `json(nullable)` for this. **Fixed** the same day (§10): `#[shelf(json)]` now reads through shelf's own `__private::Json<T>`, whose `from_sql_null` deserializes `T` from `serde_json::Value::Null`, mapping a refusal to `WasNull`; the generated code is unchanged. | 4 |
| 27 | **sqlx 0.9's attribute parsing, from sqlx-macros-core 0.9.0's source**: field keys `rename`, `try_from` (a quoted type only), `default`, `flatten`, `skip`, `json` and `json(nullable)`; the field closure has no branch for an unknown key; `parse_child_attributes(…).unwrap()` and `panic!` for impossible combinations, so a mistake in field attributes panics the derive; `flatten` + `try_from` and `json` + `try_from` are allowed (shelf refuses the latter); `rename_all` has seven cases (with `lowercase`, `UPPERCASE`, `kebab-case`); every field's type is bounded in the `where` clause (each bound mentions `R`). | 4 |
| 28 | **An executor trait with `Send` futures works everywhere shelf will be called** (S1b probe, rustc 1.99.1): methods written `fn prepare(&self, sql: &str) -> impl Future<Output = …> + Send` (return-position `impl Trait` in a trait), the trait `: Sync`, implemented for `tokio_postgres::{Client, Transaction<'_>}` and `deadpool_postgres::{Object, Transaction<'_>}`. A trait-provided method (`fn find(db: &impl Executor, id: &i64) -> impl Future<…> + Send { async move { … } }`) and a free generic `async fn` both ran inside an axum handler (through `oneshot`), inside `tokio::spawn`, and inside stockroom's `in_transaction` with an `async move` closure that owns its data. With `async fn` in the trait instead, a generic caller inside `tokio::spawn` fails: "future cannot be sent between threads safely … await occurs here on type `impl Future<…>`, which is not `Send`", and rustc's help proposes the `impl Future + Send` form. Pass a pooled connection as `&object`: `&*object` is deadpool's `ClientWrapper`, which has no impl, and `&**object` is the plain `Client`, which would skip the statement cache. | 7 |
| 29 | **Three ways to send a statement, measured through lagproxy at 5 ms each way** (a round trip about 13.5 ms here; release build, three runs, 20 calls each): `query(&str)` 26.3–28.0 ms a call, `prepare` then `query` 26.7–27.4 ms (two round trips each), `query_typed` 12.4–13.9 ms (one), deadpool `prepare_cached` 28.4–29.7 ms the first time and 13.6–13.9 ms after (one). Statements left on the session: none for the first three (tokio-postgres closes a `Statement` when it is dropped), one for the cache. A deadpool `Transaction` shares its `Object`'s cache: `prepare_cached` of the same SQL took 0.003 ms and sent nothing. `query_typed` needs each parameter's PostgreSQL `Type`, which shelf cannot derive from a Rust type, and gives no `Statement` to resolve against before the query runs; shelf does not use it (§10). | 7 |
| 30 | **The parameter types PostgreSQL infers for the three `Patch` designs** (PG 18.6, a table with `text`, `int4`, `int8`, `bool`, `float8`, `jsonb`, `text[]`, `timestamptz`, `uuid`, a nullable `text`, an enum and a domain over `text`): (a) `SET sku = $n` types the domain's parameter as the domain, so a `String` is refused on the client ("error serializing parameter 1"), as in an `INSERT` (main book §12 #1); (b) `coalesce($n, sku)` and (c) `CASE WHEN $f THEN $v ELSE sku END` type it as the base type, `text`, because a `CASE` or `coalesce` whose inputs differ resolves a domain to its base type, and the domain's check still runs on assignment (23514 for a bad value). Design (c) accepted a typed `None` for every one of those types with its flag false, and set a column to NULL with its flag true. | 9 |
| 31 | **`Stale` and `NotFound` in one round trip**: `WITH updated AS (UPDATE … WHERE id = $1 AND version = $2 RETURNING …) SELECT updated.*, (SELECT version FROM t WHERE id = $1) AS current_version FROM (VALUES (1)) AS one LEFT JOIN updated ON true` always returns one row: an updated row means success; no updated row and a current version means `Stale` (and gives the version to report, for a 412); neither means `NotFound`. The subquery sees the snapshot from before the `UPDATE`, so after a successful update it reports the old version. | 9 |
| 32 | **SQL from another type's consts, at compile time, on stable Rust**: two `const fn`s (the total length of a `&[&str]`, and the parts joined into a `[u8; N]`), inner `const` items (`const PARTS: &[&str] = &["INSERT INTO ", <Product as Table>::TABLE, …]; const BYTES: [u8; total_len(PARTS)] = join(PARTS);`) and `core::str::from_utf8` in a `const` build the whole statement as an associated `const`, stored in the binary as one string. It works in a generic impl as long as the inner consts do not mention its parameters; a model that does (`<Page<T> as Table>::TABLE`) fails with E0401, "can't use generic parameters from outer item", and a `static` in a generic function has the same limit, so `LazyLock` would not help there either. | 6, 8 |
| 33 | **The three `Patch` designs, measured through lagproxy at 5 ms each way** (release build, three runs; a deadpool connection with its statement cache; 70 patches cycling through the 7 non-empty shapes of 3 fields, 20 of them setting a nullable column to NULL): (a) a `SET` list per shape: 7 statements cached, 15.0–15.8 ms a patch (each new shape costs a prepare); (b) `coalesce($n, col)`: 1 statement, 13.8–14.6 ms, and **0 of 20 NULLs written**; (c) `CASE WHEN $f THEN $v ELSE col END`: 1 statement, 13.9–14.7 ms, 20 of 20 NULLs written. With n patchable fields, (a) caches up to 2^n − 1 statements on every pooled connection. shelf uses (c) (§10). | 9 |
| 34 | **The query builder's value types** (S1c probe, rustc 1.99.1): a trait `Value<T>`, "a value for a column of type `T`", with five impls (`T` itself, `&T` cloned, `&str` for `String`, `T` for `Option<T>`, `&str` for `Option<String>`) has no overlapping impls, and an integer literal infers as the column's type: `Product::ID.gt(3_000_000_000)` on an `i64` column compiles. **One widening impl more (`i32` for `i64`) breaks that**: the literal falls back to `i32`, and rustc refuses it ("literal out of range for `i32`"; `overflowing_literals` is deny by default). So shelf converts no numbers: an `i32` variable for a `bigint` column is a compile error in `#[diagnostic::on_unimplemented]`'s words ("`i32` cannot be compared with a column of type `i64`"), and a column of another model is E0308 ("expected `Expr<Product>`, found `Expr<Order>`"). | 10 |
| 35 | **An inherent constant shadows a trait constant of the same name**: with `impl Table for Product { const VERSION: … }` and `impl Product { const VERSION: Column<Product, i32> }`, `Product::VERSION` is the column and `<Product as Table>::VERSION` the trait's (S1c probe). shelf's generated and generic code always write the qualified form, so a field called `version` or `table` is safe. | 10 |
| 36 | **How PostgreSQL types the builder's parameters** (PG 18.6, from `Statement::params()`): `"price" > $1` and `BETWEEN $1 AND $2` take the column's type; `"tags" @> $1` takes the array type (`_text`), `@> ARRAY[$1]` the element; `LIMIT $n` and `OFFSET $n` take `int8`. Against a domain column, `= $1`, `= ANY($1)` and `LIKE $1` all type the parameter as the base type (`text`, `_text`), so a `String` is accepted there, unlike in an `INSERT`. `Vec<&i64>` is sent as `int8[]`. | 10, 11 |
| 37 | **`insert_many` through `unnest` needs a cast per column, and the prepared single-row `INSERT` supplies them**: `Statement::params()` of `INSERT … VALUES ($1, …)` holds each column's own type (a domain as the domain, an enum by name), with `schema()` and `name()`, so `unnest($1::"s"."sku_d"[], $2::"s"."status_e"[], $3::"pg_catalog"."jsonb"[], …)` accepts exactly what `insert` accepts: a domain newtype, and not a `String` ("error serializing parameter 0", as for one row); the domain's check still runs (23514). **An array column cannot go through `unnest`**: tokio-postgres cannot send `Vec<Vec<String>>` as `text[]` ("error serializing parameter 0"), and `unnest` of a 2-D array flattens it (4 rows from a 2×2). Arrays of unequal length are padded with NULLs (shelf builds them from one slice, so they never differ). | 12 |
| 38 | **Binary COPY with the `INSERT`'s types, and the guard for #789**: `BinaryCopyInWriter::new(sink, insert.params())` loads a domain newtype, `jsonb` and `text[]`; a value refused on the client ("error serializing parameter 0") leaves the connection usable once the writer is dropped; a bad domain value fails at `finish()` with the check's message, and the connection lives. **Preparing a `COPY` checks nothing**, not even that the table exists (0.7.18 prepares it; the server checks when it runs). Preparing the `INSERT` with a missing column fails cleanly ("column \"nope\" of relation \"p\" does not exist"), while the same `COPY` fails as it starts ("db error") and the next query gets "unexpected message from server" (issue #789). So shelf prepares the `INSERT` before every COPY. | 12 |
| 39 | **Streams through the executor**: `query_raw(statement, params.iter().copied())` behind a trait method returning `impl Future + Send` works for all four executors and inside `tokio::spawn` (100,000 rows). The `RowStream` it returns does not borrow the client, which is root PLAN §12 #13's hazard with a pool; shelf's stream therefore borrows the executor it reads from. | 12 |
| 40 | **What `verify` can learn about NULLs** (PG 18.6): a table column's `table_oid()` and `column_id()` lead to `pg_attribute.attnotnull` (one query for all of them, `unnest($1::oid[], $2::int2[])` joined to `pg_attribute`); an expression has neither (`None`). **A view's columns report the view's own OID**, whose `attnotnull` is always false, so `verify` cannot know them and leaves them out (`pg_class.relkind`). Generated columns, virtual and stored, are nullable in the catalog unless declared `NOT NULL`, which PG 18 accepts on a virtual one. | 13 |
| 41 | **Where the builder's and the relations' type errors land** (trybuild, rustc 1.99.1; `tests/ui/query.rs`, `relations.rs`): `filter(impl Into<Expr<M>>)`, there for raw SQL, turned another model's column into "the trait bound `Expr<Product>: From<Expr<Order>>` is not satisfied … but trait `From<Raw<Product>>` is implemented"; a trait of shelf's own, `Condition<M>`, with `#[diagnostic::on_unimplemented]`, says "`Expr<Order>` is not a condition on `Product`", and rustc adds that `Condition<Order>` is implemented. A relation's generated calls are spanned on the `key = …` tokens (`quote_spanned!`), so a missing column constant (E0599, E0609, with rustc's "a field with a similar name exists") and a foreign key of the wrong type (E0308, and `ForeignKey`'s own message) point into the attribute; one copy of the latter is still reported at `Table`, from the method's return type. A method missing for the column's type (`like` on an `i64`, `is_null` on a `String`) is E0599, with "the method was found for `Column<M, Option<T>>`". | 10, 11 |
| 42 | **N+1, measured** (`cargo run -p shelf --release --example bench_relations`; 100 orders × 3 lines, median of 3, three runs; release build, rustc 1.99.1, Apple Silicon, PG 18.6 on localhost). Through lagproxy at 5 ms each way (a cached `find` 13.3–13.8 ms): the loop `order.lines(&db)` 2,732–2,753 ms on a `Client` (≈200 round trips: prepare, then query) and 1,348–1,366 ms pooled (≈100, the statement cached); the same loop pipelined with `try_join_all` on one `Client` 32–35 ms (2–3); **`Order::load_lines` 26.7–28.5 ms on a `Client` (2) and 14.0–14.6 ms pooled (1)**. Directly on localhost (a round trip 0.05 ms): the loop 10.6–12.6 ms and 5.0–5.5 ms, pipelined 3.3–3.5 ms, batched 0.4–0.6 ms and 0.2 ms. Pipelining hides the latency of 100 queries but not their work; batching removes both. | 11 |
| 43 | **A statement cache hides a migration from `verify`**: through deadpool's `prepare_cached`, `SELECT * FROM products` keeps the description it had when first prepared, so after `ALTER TABLE products ALTER COLUMN sku DROP NOT NULL, DROP COLUMN note` on the same pooled connection, `verify` still found `note` and reported only `sku` (its `NOT NULL` comes from a live catalog query). Prepared without the cache (`Executor::prepare_uncached`), both are reported. (`tests/verify.rs`; checked by putting the cached call back, which fails the test.) | 13 |
| 44 | **Bulk writes through shelf, measured beside pgload** (`cargo run -p shelf --release --example bench_bulk -- [--rows N] [--keyed]`, written by a second session from pgload's code: pgload's rows and its temporary table without constraints, one pooled connection with its statement cache, each run in a transaction rolled back, an md5 fingerprint showing every strategy stores the same rows; median of 3, ranges over 3 runs; release build, rustc 1.99.1, Apple Silicon, PG 18.6 on localhost). **20,000 rows:** `insert` in a loop 0.766–0.774 s, pipelined (64 in flight) 0.147–0.150 s, `insert_many` in batches of 1,000 0.018 s, in one call 0.016–0.017 s, `copy_in` 0.006 s; pgload, run right after: 0.756 s, 0.120 s, `unnest` batches 0.014 s, CSV COPY 0.012 s, binary COPY 0.008 s. **Into a table with a primary key and `NOT NULL` columns (`--keyed`, 5 runs):** loop 0.831–0.851 s, pipelined 0.179–0.190 s, and the batched forms noisy (WAL): batches 0.030–0.091 s, one call 0.029–0.084 s, `copy_in` 0.016–0.033 s. **2,000 rows through lagproxy at 1 ms each way:** loop 9.41–9.76 s, pipelined 0.159–0.165 s, batches 0.013–0.014 s, one call 0.009 s, `copy_in` 0.012–0.013 s; pgload 9.48–9.53 s, 0.192 s, 0.043–0.046 s, 0.045–0.047 s, 0.045–0.046 s. shelf's `copy_in` matches pgload's binary COPY; `insert_many` costs a few ms more than pgload's `unnest` because it returns and maps the rows. | 12 |
| 45 | **pgload and `bench_bulk` time different spans**, which is why shelf's batched writes look faster through lagproxy (#44): pgload's timer wraps its whole `load()` (`apps/pgload/src/lib.rs`), with `CREATE TEMP TABLE`, `BEGIN`, a `count(*)` sent as a string (prepare, then execute) and `COMMIT`, five round trips besides the strategy; `bench_bulk` times the strategy alone. At 1 ms each way a round trip costs about 6 ms here (two batches of `insert_many`: 13–14 ms), so the five come to about 30 ms, the gap in every row. On localhost they are invisible. | 12 |
| 46 | **Outside a transaction, each insert pays a commit**: 20,000 inserts pipelined (64 in flight) into a table with a primary key took 0.91–0.98 s when each one committed, and 0.13–0.14 s between one `BEGIN` and `COMMIT` (six runs each, a scratchpad probe on tokio-postgres alone; `synchronous_commit = on`, `wal_sync_method = open_datasync`, PG 18.6 on localhost): a commit waits for its WAL flush. So shelf's loops and pipelines belong in a transaction (main book Chapter 8), and a benchmark must say whether it uses one. | 12, 7 |
| 47 | **What the `stockroom` crate offers a port** (S2 probe, read from its source): every module is `pub`, so its migrations (`db::migrate::{MIGRATIONS, run}`), its domain types, `AppError` (with `From<tokio_postgres::Error>`, which maps SQLSTATEs and constraint names), `db::{in_transaction, CancelOnDrop, MakeTls}`, `http::{AppState, Db, AppJson, AppQuery, etag, if_match, router, RouterOptions}` and the repository's request and response types (`repo::orders::{NewOrder, Order, OrderLine}`) can all be reused. Two things cannot: `http`'s handler modules are private (`mod products;`), so ported routes need handlers of their own; and the test harness lives in `stockroom/tests/common/mod.rs`, which no other crate can depend on, though its `TestApp { router, state }` has public fields, so a `#[path]` module can reuse it with another router. | 14 |
| 48 | **shelf models fit stockroom's schema** (S2 probe: stockroom's own migrations run into a fresh schema of `tpbook`): `verify` passed for `Store` (the composite `Address`, postgres-types' derive passed through), `Product` (the domain `Sku` with its hand-written `accepts`, `Cents`, `text[]`, `jsonb` as `serde_json::Value`, `version`, `updated_at`), `Order` (the enum `OrderStatus`, `#[shelf(json)] customer: Customer`) and `OrderLine` (the domain `Quantity`, a composite key). **It refused `Stock`**: the virtual generated column `available` (`on_hand - reserved`, two `NOT NULL` columns) is nullable in the catalog, so an `i32` field is `Nullable` (#40); the model reads it as `Option<i32>`, since changing stockroom's schema is not this port's business. The writes worked as written: `NewOrderLine::insert_many` sends `Quantity` through `unnest($n::"…"."positive_quantity"[])`, a borrowed `#[shelf(json)] customer: &'a Customer` on an `Insert`, `Stock::find(&(store_id, product_id))`; `Product::SKU` with `gt`, `eq` and `any` takes a `Sku` (a domain parameter there is `text`, #36); the `AFTER INSERT` trigger on `orders` fired for a shelf insert (one outbox row). A constraint violation comes back as `Error::Db`, which `AppError::from` maps as stockroom does (`products_price_positive` → 422, `field: "priceCents"`). | 14 |
| 49 | **`impl From<shelf::Error> for AppError` is refused in a third crate** (E0117, "only traits defined in the current crate can be implemented for types defined outside of the crate", with "`shelf::Error` is not defined in the current crate" and "`AppError` is not defined in the current crate", rustc 1.99.1): neither type is the port's. So the port converts with a function and `map_err`, and `?` alone does not do it (the sketch's open question 3). | 14 |
| 50 | **Ported routes beside stockroom's, in axum 0.8.9** (S2 probe): a router with the ported routes and `fallback_service(stockroom_router)` sends unported paths to stockroom, except that a ported `/products/{sku}` captures stockroom's static `/products/stream` and `/products/export.csv` (the parameter matches; the static routes live in the other router). `route_service("/products/export.csv", stockroom_router)` panics: "Invalid route: `Router::route_service` cannot be used with `Router`s. Use `Router::nest` instead"; `stockroom_router.into_service()` is accepted, and passes the whole path on, so stockroom routes it as usual. The port names each static path a ported parameter would capture. | 14 |
| 51 | **A shelf `Patch` always checks a version, and stockroom's `PATCH` lets a client send none** (`If-Match` absent: `version = coalesce($2, version)`, one statement). The port reads the product first anyway, since the route names a SKU and `Patch::apply` takes the key, and patches at the version it read; when another write lands in between, the patch is `Stale`, and without `If-Match` it is made again against the newer row. Each retry means another write succeeded, so the loop makes progress. `concurrent_patches_without_if_match_all_apply`: 30 patches at once, each `200`, the row at version 31. | 9, 14 |
| 52 | **A streamed HTTP body that owns its connection, with shelf's borrowing stream inside it**: an `async_stream::try_stream!` generator takes stockroom's `CancelOnDrop` by value and calls `Product::query().stream(&*db)` inside, so the `ModelStream` borrows a local of the generator, which axum's `Body::from_stream` accepts (`Send`; compiled at the first try, rustc 1.99.1). The query then starts when the body is first polled, after the handler has returned `200`; the handler awaits the first line through `StreamExt::peekable` to keep an error before the first row an error response. stockroom's two stream tests pass on the port: one NDJSON line per product with the connection back in the pool, and a client hanging up mid-stream (50,000 rows) leaves no busy connection. | 12, 14 |
| 53 | **One test suite, two routers**: `orm/stockroom-shelf/tests/common/mod.rs` includes stockroom's harness by `#[path]` and builds a `TestApp` with the port's router from stockroom's `AppState`; `both(scenario)` runs an `AsyncFn(&TestApp) -> Vec<Reply>` against each router, on a database of its own, and compares status, `ETag`, `Content-Type` and body reply by reply, with UUIDs and timestamps masked. 11 scenarios, 63 compared replies (counted by a run). **The replies cannot say which handler answered**, being meant to be the same; the statement cache can: on a pool of one connection, each ported route leaves shelf's SQL in deadpool's cache (`StatementCache::remove(sql, &[])`), and a stockroom route its own. A deliberate break (`OrderRow` → "orders not found") failed the comparison; a break in `noun("Product")` did not, because `by_sku` builds its own `NotFound`: a comparison covers only the paths its scenarios take. | 13, 14 |
| 54 | **Statements and round trips per ported function** (`cargo run -p stockroom-shelf --release --example line_for_line`: one pooled connection through a counter of the client's Parse and Execute messages, then lagproxy at 5 ms each way; a round trip, a cached `SELECT 1`, 14.4 ms; median of 5 warm calls, three runs; release build, rustc 1.99.1, Apple Silicon, PG 18.6). `stores::list`, `stores::create`, `products::page` (first page; with a cursor and a tag), `by_sku` and `products::create`: 1 statement, 1 round trip on both sides (13.8–15.7 ms). **`products::update`**: stockroom 1 statement, 1 round trip (14.0–15.8 ms); shelf 2 statements (the read by SKU, the patch), 2 round trips (29.1–30.5 ms). **`orders::place`, 3 lines, in `in_transaction`**: stockroom prepares 7 statements on an empty cache, and 2 more on every call (the two `INSERT`s sent as strings), runs 9, 9.1–9.5 round trips (132.7–134.9 ms); shelf prepares 6 (`insert_many` prepares its single-row `INSERT` for the column types, and never runs it), none warm, runs 7, 5.9–6.1 round trips (85.0–90.3 ms): its inserts are cached and it builds the response from their `RETURNING` rows instead of reading the order back. **`orders::get`, 3 lines**: stockroom 2 statements, pipelined, 1 round trip (13.2–14.8 ms); shelf 4 statements (the order and its lines pipelined, then the store and the products), 2 round trips (28.4–29.9 ms): no joins. BEGIN and COMMIT are simple queries, in the round trips and not in the counts. | 14 |
| 55 | **Lines of Rust, stockroom's repository against the port's** (blank lines and comments not counted, attributes counted; a scratchpad script per item). Stores: 57 against 34 (`TryFrom<&Row>` 12 lines gone, three derive lines added; `create` 13 → 4). Products, the ported items: 150 against 87 (`TryFrom<&Row>` 16 gone; `by_sku` 13 → 6, `create` 23 → 4, `update` 45 → 16, `page` 18 → 16). Orders, `place` and `get`: 148 against 175: the two table models and their `Insert`s (44 lines) and the assembly of the response from them (`view`, 33) are new, where stockroom's joins put the response's columns in one row; `get` is 46 lines against 10 plus `view`, `place` 96 against 86. Hand-written SQL in the ported functions: 15 statements against 1 (the reservation). | 14 |
| 56 | **`Config::host` and `Config::port` add to a config's lists** (tokio-postgres 0.7.18, multi-host configs): setting them on a copy of a config leaves its own host first, so the measurement's first pool connected around its proxies, and every count read 0. A new `Config` with the same user, password, database and options fixes it. (A tooling finding, met while building #54.) | — |
| 57 | **Every row of a statement shares the statement's column list** (tokio-postgres 0.7.18: `Row` holds its `Statement`, and `Row::columns()` returns `self.statement.columns()`): `std::ptr::eq(row.columns(), statement.columns())` holds for every row of a prepared statement, and the rows of `query(&str)` share one list too. A `Column` keeps the name, the table OID and column number (`None` for 0), the type modifier and the `Type` from the *RowDescription*; an expression column has neither table nor number. (Chapter 5's first two programs.) | 5 |
| 58 | **Chapter 5's measurements, again** (written 2026-11-09). `bench_mapping` (release, median of 5, three runs, rustc 1.99.1, Apple Silicon, PG 18.6 on localhost, with other sessions' builds on the machine): by name 40.6–41.7 ms, by index 13.2–13.5 ms, `from_rows` 14.1–14.4 ms (4–8% over by index), `from_row` per row 41.3–42.7 ms; the ratios match #13. In a debug build (the book's examples), `from_rows` 134.5–135.6 ms against `from_row` per row 225.6–227.2 ms, 1.7× in three runs: a relative assertion (`once < per_row`) has a wide margin. | 5 |
| 59 | **One `resolve` costs well under a microsecond; a round trip costs tens** (12 fields, 10,000 calls; a prepared `SELECT 1`, 100 to 1,000 calls; localhost): release 262–268 ns against 46–47 µs (preparing the 12-column `SELECT`: 56 µs); debug 815–874 ns against 61–116 µs (Chapter 5's program, 100 calls: 77–116 µs in three runs). So resolving is about 0.6% of the cheapest call, and shelf does not cache `Columns` per statement (§10). | 5 |
| 60 | **Only `Send` on `FromRow::Columns` is load-bearing** (probe, 2026-11-09: each bound removed in turn, `cargo check -p shelf --all-targets`). Without `Send`, `fetch_all`, `fetch_one`, `fetch_opt`, `Table::find` and `all` and `insert_many` stop compiling: their `+ Send` futures hold a `T::Columns` across an `.await`. Without `Sync`, or without `'static`, everything still compiles. Kept as promises to generic code (a shareable, storable `Columns`, as a cache would need). | 5 |
| 61 | **Two columns of one name**: tokio-postgres's `position` (so shelf's `column`, and `Row::try_get`) finds the first; sqlx-postgres 0.9.0 builds `column_names: HashMap<UStr, usize>` by inserting each column in order (`connection/resolve.rs`), so the last wins. sqlx-core 0.9.0's `Row::try_get` checks `T::compatible` only for a non-NULL value, so a type mismatch in an empty result or an all-NULL column is never reported. Neither library refuses the duplicate; shelf does not either (§10, Chapter 5's Exercise 2). | 5 |
| 62 | **tokio-postgres's per-row `accepts` is the safety net for a misused `Columns`**: `from_row_at` with a row of another statement whose columns are swapped fails as `Mapping::Decode` with a `WrongType` cause; a row with fewer columns fails as `Decode` with no cause (`invalid column`). Without the per-row check, `String`'s `FromSql` would decode the eight bytes of `1299::int8` as `"\0\0\0\0\0\0\u{5}\u{13}"`, silently. `decode_error`'s comment said a type mismatch has no cause; corrected. (Chapter 5's Exercise 3.) | 5 |
| 63 | **Chapter 6's compile errors, captured** (rustc 1.99.1, a scratch crate on shelf by path): `concat!("SELECT count(*) FROM ", Product::TABLE)` is "expected a literal" with the note that only literals can be passed to `concat!()`; a `for` loop in a `const fn` is E0015 ("cannot use `for` loop on `&[&str]` in constant functions"), which is why `join` uses `while`; the E0401 of #32 again, word for word, with rustc's help proposing a `let` binding (which cannot size an array). Without `ORDER BY`, a row updated in a small temp table comes back last from a sequential scan (Chapter 6's Exercise 3). | 6 |
| 64 | **Chapter 7's measurements** (lagproxy at 5 ms each way, PG 18.6 on the same machine, Apple Silicon, 20 calls a strategy, three runs in debug and three in release): prepare-then-run 27.0–27.9 ms debug, 27.5–27.6 ms release; `prepare_cached` 13.7–14.2 / 13.6 ms after a first call of 26.4–28.8 ms; `query_typed` 13.6–13.9 / 13.7–13.9 ms. Statements left on the session (counted with `query_typed`, whose unnamed statement `pg_prepared_statements` does not list): 0 for a plain `Client`, 1 for the cache. `try_join!` of `find` and `count` on one plain `Client`: 26.0–27.9 ms against 53.5–56.6 ms in sequence (2 round trips against 4). Captured: implementing tokio-postgres's `GenericClient` for a wrapper is E0277 on `private::Sealed` plus E0046 listing 17 missing methods; `&*object` is E0277 "`ClientWrapper: Executor` is not satisfied". | 7 |
| 65 | **An impl may write `async fn` for a trait method declared `-> impl Future<…> + Send`**: the compiler checks that impl's future for `Send` (Chapter 7's Exercise 2, a statement cache whose `prepare` takes a `Mutex` before and after an `.await`), and clippy's `manual_async_fn` flags the `fn … -> impl Future + Send { async move { … } }` form in an impl. Inside the trait's declaration, the `+ Send` must still be written (#28). | 7 |
| 66 | **What PostgreSQL 18.6 checks when a write is prepared**: an `INSERT` naming a `GENERATED ALWAYS` identity column fails at *Parse* (428C9, with a DETAIL and the HINT `OVERRIDING SYSTEM VALUE`); an `ON CONFLICT` target that matches no unique index prepares fine and fails only when run (42P10), because the arbiter index is inferred when the statement is planned. An `ON CONFLICT DO UPDATE` that updates still draws an identity value (the next row's id skips one); `ON CONFLICT DO NOTHING RETURNING` returns no row on a conflict; `MERGE … RETURNING merge_action()` (PG 17+) works with `USING (VALUES ($1::text, $2::bigint))`, the casts typing the parameters. A domain parameter refused on the client names the domain with its schema (`pg_temp_86.sku` for a temporary domain, a number that changes per session; the chapter uses a named schema). An `UPDATE` that changes no value still moves the row's `ctid`. | 8 |
| 67 | **Chapter 9's checks**: serde reads a JSON `null` into `Option<Option<T>>` as `None`, like an absent field; `Option::<T>::deserialize(d).map(Some)` with `#[serde(default)]` tells them apart. The three `Patch` designs again, 14 patches (7 shapes, 4 writing NULL) in a debug build through lagproxy: (a) 7 statements, 22.0–22.7 ms a patch; (b) 1, 0 of 4 NULLs, 15.6–16.4 ms; (c) 1, 15.5–15.8 ms. An empty patch on a versioned model still adds one to the version (and sets `updated_at`), since those are always in the `SET` list. A `bigint` version column with an `i64` field works, `Stale` included, past `i32::MAX`. `now()` is the same for two patches 20 ms apart in one transaction, and at least 20 ms apart outside one. | 9 |
| 68 | **SeaORM 2.0.4, from its source** (for the side-by-sides of Chapters 6–9): `Entity::find_by_id(k).one(db)` adds `LIMIT $n` as a parameter; the dense entity format (`#[sea_orm::model]`) still needs `impl ActiveModelBehavior for ActiveModel {}`; `ActiveModel::insert` runs `before_save`/`after_save` around `exec_with_returning`; nothing in its update path handles a version column, and `update_many().set(…).filter(…).exec(db)` returns `UpdateResult { rows_affected }`. sqlx 0.9's `Executor` is implemented for `&Pool` and `&mut PgConnection` only, taken by value; a function running two queries on one generic executor takes `Acquire`. | 6–9 |
| 69 | **Chapter 10's checks** (rustc 1.99.1, PG 18.6): a stage `Value<T>` with one widening impl (`i32` for `i64`) fails `gt(3_000_000_000)` with "literal out of range for `i32`" (`overflowing_literals`), and compiles without it; a stage `filter(impl Into<Expr<M>>)` with `From<Raw<M>>` reports another model's condition as "the trait bound `Expr<Product>: From<Expr<Order>>` is not satisfied … but trait `From<Raw<Product>>` is implemented for it" (captured for §10.6). **Two fields that differ only in case** (`sku`, `SKU`) both become the constant `SKU`: E0592 "duplicate definitions with name `SKU`", reported at `Table` although the constant's name carries the field's span. `NOTE.eq(None)` renders `= $1` with a NULL and matches no row (`is_null` matched 2), and `ne("fragile")` matched 0 of 3 rows, the NULLs left out. **Seven requests of `page` with two optional filters prepared four statements** on a pool of one (`pg_prepared_statements`, counted through `query_typed`): one per shape, values and `LIMIT` being parameters. A nullable column's value prints as `Some(0)` in `params()`; a panic's first line carries the thread id (`thread 'main' (25976639)`), which changes per run. | 10 |
| 70 | **Chapter 11's measurements** (debug build, lagproxy at 5 ms each way, PG 18.6 on the same machine, Apple Silicon, three runs): the lines of 50 orders × 3 lines on a plain `Client`: the loop 1.5 s each run (51 statements, counted by an executor of the chapter's own that wraps a `Client`), the loop pipelined with `try_join_all` 29.0–30.5 ms, `Order::load_lines` 28.6–30.9 ms (2 statements); through the proxy the pipelined loop is as fast as the loader, and only the localhost column of #42 shows its extra work. `load_lines` then `load_store` 52.7–58.6 ms in sequence, 28.5–28.7 ms with `try_join!`. Checked in the sources: syn 3.0.9's `parse_meta_path` reads a nested key with `Ident::parse_any`, so `as = lines` parses as a key; SeaORM 2.0.4's `LoaderTrait::load_many` returns `Vec<Vec<Model>>` and builds `(cols) IN ((…), …)` with one bound value per distinct key (`loader.rs`, `prepare_condition_with_save_as`), composite keys included. | 11 |
| 71 | **`insert_many` broke on a composite column, found while writing Chapter 12**: `INSERT INTO stores (code, address) SELECT * FROM unnest($1::text[], $2::address[])` fails at *Parse* with 42601 "INSERT has more expressions than target columns", since a multi-argument `unnest` in `FROM` expands a composite into its fields (root PLAN.md §12 #4). Fixed: `SELECT unnest($1::…[]), unnest($2::…[])` keeps it whole (plan: `ProjectSet` over `Result`), pads unequal arrays with NULLs as before, and is tested (`a_composite_column_goes_through_insert_many_whole`, with `copy_in` too). Re-measured after the fix (release, rustc 1.99.1, Apple Silicon, PG 18.6 on localhost; `bench_bulk` three runs, median of 3 each): 20,000 rows: loop 0.767–0.782 s, pipelined 0.146–0.151 s, `insert_many` in batches 0.015 s, one call 0.014 s (0.016–0.018 s before), `copy_in` 0.006 s; pgload in the same session (two runs) 0.733–0.761 s, 0.120–0.121 s, `unnest` batches 0.013–0.014 s, CSV COPY 0.013 s, binary COPY 0.007–0.008 s. `--keyed`: 0.822–0.834 s, 0.180–0.202 s, 0.030–0.058 s, 0.026–0.038 s, 0.017–0.028 s. 2,000 rows through lagproxy at 1 ms: 9.480–9.513 s, 0.156–0.157 s, 0.013–0.018 s, 0.008–0.009 s, 0.012 s (a COPY takes two round trips, a cached `insert_many` one). Debug build, 10,000 rows in a transaction (Chapter 12's program, three runs): loop 1.2 s, `insert_many` 23.2–23.7 ms, `copy_in` 9.1–9.7 ms. **A statement takes at most 65,535 parameters**: postgres-protocol 0.6.12 writes the count as a `u16`; with 65,536, PG 18.6 prepares the statement and tokio-postgres 0.7.18 fails to read its *ParameterDescription* ("error parsing response from server", caused by "invalid message length: parameters is not drained"), and the connection still works. `shelf::Error::Db`'s display is "db error"; the server's message is `as_db_error()`'s. A binary COPY's `where_` names the line and column, not the value. | 12 |
| 72 | **Chapter 13's checks** (PG 18.6): after `ALTER TABLE … DROP COLUMN note`, the model's cached `SELECT` (deadpool) fails on its next execution with 42703 `column "note" does not exist`, PostgreSQL analyzing its text again, not with 0A000 (which is what a `SELECT *` gets after a column is added, main book §4.6). A column made nullable under a model maps every row until the first NULL (`Mapping::Null`), and `verify` reports it before any row exists. `varchar(5)` under a `String` field passes `verify` and fails the next long insert with 22001: a type modifier is not in what `accepts` sees. A stage expansion test with a hand-typed expectation failed on one line that prettyplease breaks (`column::<\n String,\n >(…)`), which is why expansions are captured. From the sources: sqlx-postgres 0.9.0 infers a column's nullability from `pg_attribute.attnotnull` (one `UNION ALL` query, three binds per column) and patches it with `EXPLAIN (VERBOSE, FORMAT JSON)` (`connection/describe.rs`); sqlx-macros-core 0.9.0 uses `DATABASE_URL` unless `SQLX_OFFLINE` is set, then `query-<hash>.json` in `SQLX_OFFLINE_DIR`, the crate's `.sqlx` or the workspace's (`query/mod.rs`). | 13 |
| 73 | **`line_for_line` again, after §12 #71's change to `insert_many`** (release, lagproxy at 5 ms each way, a round trip 13.9 ms, three runs, median of five warm calls): the everyday functions 13.5–14.4 ms on both sides; `products::update` stockroom 13.7–14.3 ms, shelf 27.3–28.5 ms (2 statements, 2.0–2.1 round trips); `orders::place` stockroom 124.4–125.7 ms (7 prepared cold, 2 warm, 9 run, 9.0–9.1 round trips), shelf 82.4–85.0 ms (6, 0, 7, 5.9–6.1); `orders::get` stockroom 13.8–14.3 ms (1 round trip), shelf 27.6–28.1 ms (2). The counts match #54. A session for Chapter 14.6: `seed --reset` printed "seeded 3 stores, 1000 products, 2995 stock rows, 20000 movements"; `verify` "every model matches its table"; `curl … /products/stream | head -2` made the service log stockroom's "cancelled a query nobody was waiting for" (`CancelOnDrop`); `NO_COLOR=1` keeps tracing's colour codes out of a captured log. | 14 |

---

## 13. Next steps: phase S4, step by step

**Resuming in a new session:** read this section first, then §9's S4 row, §5 (the coverage
matrix) and the written book (`orm/book/`, all `current`). Each step is ticked when it is done and
its gates are green, and a step in progress says how far it got. The work is on the branch `shelf`
and may not be committed yet (`git status`); commit only when the user asks. **Check first that no
other session is working in this repository** (§11): two sessions in one working tree overwrite
each other's files.

**Done before:** S0, S1a and the sketches (2026-11-07); Chapters 0–4, S1b, S1c, S2, S3a, S3b and
S3c, all on 2026-11-09: every chapter `current`, 141 runnable examples. S3c's checklist, as it was
done: Chapter 10 (18 examples, the `$n` scanner shown finished; §12 #69), Chapter 11 (10, two
through lagproxy, composite foreign keys an exercise; §12 #70), Chapter 12 (12; the composite bug
in `insert_many` fixed, the bulk benchmarks re-run; §12 #71), Chapter 13 (8; Exercise 1 a list of
boxed futures; §12 #72), Chapter 14 (no runnable programs, as the main book's capstone; a real
session; `line_for_line` re-run, §12 #73), the appendix (excerpts of four expansion tests, the
phrasebook checked in the sources).

**How S3c worked, to repeat in any later chapter edit:** probes in the scratchpad (a single-file
`rustc` program for a compiler error that needs no crate; a scratch workspace with shelf by path
otherwise), outputs captured by `cargo run -p shelf-playground --example shelf_chNN_KK` after
`book.py --book orm check --chapter N`, timings through a lagproxy started on a spare port, three
runs for each range the text quotes. Twice in S3c a typed expectation was wrong (a pretty-printed
line break, the error a cached statement gets): capture, never type.

**S4, polish and merge** (§9: the book PDF, a read-through, a coverage audit against §5, and
merging `shelf` into `main`; done when `book.py --book orm list` shows every chapter `current` and
the PDF fresh):

- [x] **1. Read-through**, Chapters 0–15 in order: cross-references between chapters written on
  different days (Chapters 0–4 were written before S1b), terms against the appendix's glossary,
  figures quoted twice (a measurement in a chapter and in §12) agreeing, banners and
  Previous/Next lines, every *Hint* present. *Done 2026-11-09:* banners, Previous/Next, hints
  and every in-book `§N.M` checked by script; the prose read in five ranges, each finding checked
  against the source before it was fixed (63 fixes). Notable: Chapter 0 still described sketches;
  "Rust 1.85 or newer" in both books (and 1.88 in the root README) is wrong, since the locked uuid
  1.29 declares 1.89 and the code uses let chains (1.88), and clippy's `incompatible_msrv` at 1.89
  is clean, so both books and the README now say 1.89; Chapter 9's version-check SQL had
  impossible `$n`s (the version is always `$2n+1`); Chapter 11 quoted #54's superseded timings
  for `orders::get` (now #73's); Chapter 14's router paragraph named the import where #50 found
  the export captured; Chapter 14's Solution 3 bound its parameters inline, which it said not to
  do; Chapter 13 and this file said the toolchain is pinned, and nothing pins it.
- [x] **2. Coverage audit** against §5's matrix: each row's chapter shows the feature, with a
  runnable example or an excerpt; fix the matrix where a chapter moved a topic. *Done:* every
  row holds; three main-book section numbers were off (arrays are §3.8, rows into structs §3.5).
- [x] **3. Small debts found in S3c:** `orm/shelf/src/lib.rs`'s crate documentation still says
  "Phase S1a built the mapping" and lists only the mapping; its `Table` derive documentation does
  not mention `has_many` and `belongs_to`. orm/SKETCHES.md describes sketches that no longer
  exist in `orm/book/` (keep it as the method, say so). *Done:* the crate documentation covers
  the three parts, the `Table` documentation has a relations table (`cargo doc` clean),
  SKETCHES.md opens by saying no sketch remains, and orm/README.md's status is current.
- [x] **4. The book PDF** rebuilt (`book.py --book orm book`) after the read-through, and
  `book.py --book orm list` showing it fresh. *Done:* 312 pages; the main book's PDF was rebuilt
  too (270 pages), since its Chapter 0 changed (the Rust version).
- [ ] **5. All gates**, both books; then ask the user about merging `shelf` into `main`, and after
  the merge run the gates on `main` and update the status lines (this file, orm/CLAUDE.md, the
  root CLAUDE.md and PLAN.md). *In progress:* every gate green on `shelf` after steps 1–4 (fmt,
  both clippy runs, 175 workspace tests, the tool's tests, 156 main-book and 141 shelf-book
  examples run); `main` has no commit that `shelf` lacks, so the merge is a fast-forward. Waiting
  for the user's go-ahead to commit and merge.
