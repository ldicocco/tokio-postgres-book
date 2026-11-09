# orm/CLAUDE.md — the shelf child project

Guidance for Claude Code working under `orm/`: **shelf**, a small ORM on tokio-postgres, and
its book, *Building an ORM on tokio-postgres*.

**[orm/PLAN.md](PLAN.md) is the source of truth** for this project: its scope, the target
design, the chapter outline, the phases and the decisions taken. Read it before starting any
phase or chapter, and update it in the same change when a decision changes.

**Everything in the root [CLAUDE.md](../CLAUDE.md) applies here**: the style contract, the
code conventions, the database rules, the verified tokio-postgres facts, and the rule against
writing output from memory. This file lists only what differs or is added.

## Status

- Work happens on the branch **`shelf`**. It is merged into `main` in phase S4, not before.
- **Phase S0 (scaffold) is done** (2026-11-07): the crates, `tools/book.py --book`, and a
  Chapter 0 stub (`draft`) with two runnable examples.
- **Phase S1a (the mapping) is done** (2026-11-07): `#[derive(FromRow)]` complete, with
  attributes, errors, generics, tuples, one check per statement and `fetch_*`. PLAN.md §4.0 says
  what exists and where, and §12 #4–#13 what building it taught.
- **Every chapter is sketched** (2026-11-07): `orm/book/NN-*.md` holds each chapter's sketch
  (status `sketch`), in the format of [SKETCHES.md](SKETCHES.md), and the book PDF
  (`orm/book/shelf-book.pdf`) contains them.
- **Chapters 0–4 are written** (2026-11-09), ahead of S1b at the user's request. The other
  chapters are still sketches; those of 6–13 describe the design as built.
- **Phase S1b (the model) is done** (2026-11-09): the `Executor`, `#[derive(Table)]`,
  `#[derive(Insert)]` (with `Upsert`), `#[derive(Patch)]`, `version` and `updated_at`. PLAN.md
  §4.0b says what exists and where, §10 the decisions, §12 #28–#33 the probes and measurements.
- **Phase S1c (queries and relations) is done** (2026-11-09): the query builder, `has_many` and
  `belongs_to`, `insert_many` and `copy_in`, streams, and `verify`. PLAN.md §4.0c says what
  exists and where, §10 the decisions, §12 #34–#46 the probes and measurements.
- **Phase S2 (the capstone) is done** (2026-11-09): `orm/stockroom-shelf` serves stockroom's
  API with stores, products and orders on shelf and the rest through stockroom's router; one
  test suite runs against both routers. PLAN.md §4.0d says what exists and where, §10 the
  decisions, §12 #47–#56 the probes and measurements.
- **Phase S3a (Part I) is done** (2026-11-09): Chapter 5 written, Chapter 0's §0.3 a runnable
  program, Chapters 0–5 `current`. PLAN.md §10 has its decisions (no `Columns` cache; duplicate
  column names), §12 #57–#62 its probes and measurements.
- **Phase S3b (Part II) is done** (2026-11-09): Chapters 6–9 written, `current`. PLAN.md §10 has
  its decisions (`flatten` in a `Table`, `DO NOTHING`, `json` on a `Patch`, the serde helper),
  §12 #63–#68 its probes and measurements. The `Patch` derive's error for `json` now names the
  wrapper types to use instead.
- **Phase S3c (Part III) is done** (2026-11-09): Chapters 10–14 and the appendix written, every
  chapter `current`, 141 runnable examples. PLAN.md §10 has its decisions, §12 #69–#73 its probes
  and measurements. Writing Chapter 12 found that `insert_many` broke on a composite column; it now
  puts its `unnest`s in the select list (§12 #71), with a test.
- **Next: phase S4, polish and merge.** PLAN.md §13 is its checklist; a new session starts there,
  after checking that no other session works in the repository (two did on 2026-11-09, and one
  overwrote the other's file).

## Commands

```bash
python3 tools/book.py --book orm list                  # status of the shelf book
python3 tools/book.py --book orm check                 # everything, against a fresh tpbook_shelf_check
python3 tools/book.py --book orm check --chapter 3     # one chapter's examples
python3 tools/book.py --book orm fmt --chapter 3       # rustfmt one chapter's ```rust blocks
python3 tools/book.py --book orm index                 # regenerate orm/README.md's contents table
python3 tools/book.py --book orm pdf 3                 # a local preview, in target/book/shelf/
python3 tools/book.py --book orm book                  # orm/book/shelf-book.pdf (once chapters are current)
cargo test -p shelf -p shelf-codegen                   # shelf's own tests (needs ADMIN_DATABASE_URL)
cargo test -p shelf --test from_row                    # one test file
SHELF_BLESS=1 cargo test -p shelf-codegen --test expand   # accept changed expansions (read the diff)
TRYBUILD=overwrite cargo test -p shelf --test ui       # accept changed compiler errors (read the diff)
cargo run -p shelf --release --example bench_mapping   # the mapping benchmark (DATABASE_URL)
cargo run -p shelf --release --example bench_relations # N+1 against the loaders (via lagproxy)
cargo run -p shelf --release --example bench_bulk -- --rows 20000 [--keyed]   # bulk writes
cargo test -p stockroom-shelf                          # the capstone: both routers, oversell
cargo run -p stockroom-shelf -- verify                 # the models against STOCKROOM_DATABASE_URL
cargo run -p stockroom-shelf -- serve                  # :8080, stockroom's API (stockroom.http works)
cargo run -p stockroom-shelf --release --example line_for_line [-- --delay-ms 5]   # Chapter 14.3
```

`--book` also goes after the command (`book.py check --book orm`). The root gates still apply:
the workspace's cargo test, clippy and fmt cover the orm crates, and the main book's
`python3 tools/book.py check` must stay green on this branch.

## The crates

- `orm/shelf` is the runtime, and re-exports the derives. `orm/shelf-macros` is the proc-macro
  crate. `orm/shelf-codegen` holds what the derives generate. `orm/playground`
  (`shelf-playground`) holds the book's examples. `orm/stockroom-shelf` is the capstone, on the
  `stockroom` crate.
- **stockroom-shelf reuses stockroom; it never changes it.** Its handlers are stockroom's,
  copied (stockroom's handler modules are private), and call its own repository; every route it
  does not port goes to stockroom's router (PLAN.md §10, §12 #50). A ported route must answer as
  stockroom's does: add its scenario to `tests/routes.rs`, which runs it against both routers.
- **All code generation lives in shelf-codegen**, as `pub fn expand(&DeriveInput) ->
  syn::Result<TokenStream>` per derive. shelf-macros stays a shell: `parse_macro_input!`, then
  `expand`, then `into_compile_error`. Never add logic to it.
- **Generated code uses absolute paths only**: `::shelf::…`, `::core::…`, `::std::…`, and
  tokio-postgres through `::shelf::__private::tokio_postgres`. Never write a bare `Result`,
  `Option`, `Vec` or `Ok`. `orm/shelf/tests/derive.rs` defines `type Result = ();` to catch
  that.
- **Codegen never panics on user input.** Report a `syn::Error` spanned on the offending tokens
  (`Error::new_spanned`), and combine several with `Error::combine`.
- **Every attribute has an expansion test** (`shelf-codegen/tests/expand/`) and an integration
  test (in shelf, against PostgreSQL, through `tests/common::TestDb`). **Every error a derive
  reports has a trybuild UI test** (`shelf/tests/ui/`).
- **A new derive** gets: a module in shelf-codegen with `pub fn expand`, a
  `#[proc_macro_derive(Name, attributes(shelf))]` shell in shelf-macros, a re-export with its
  attribute table in `shelf/src/lib.rs`, an arm in `shelf-codegen/tests/expand.rs`'s `expand`,
  and its keys in `attrs.rs`: a branch in `parse_key`, the key in `FIELD_KEYS` or
  `CONTAINER_KEYS`, and its owner in `KEYS`. Every derive parses every key and reports only the
  errors of the keys it owns (`Derive::reports`), so a struct with two derives hears about a
  mistake once; a derive alone on its struct (`Insert`, `Patch`) reports everything.
- **SQL that combines another type's constants** is joined at compile time with
  `__private::{total_len, join, utf8}` when it is plain text (`Insert`), and built once in a
  `LazyLock` when it needs computing, such as `$n` numbers that depend on the model's key
  (`Patch`). Generated code spells primitives `::core::primitive::str` (the hygiene test
  defines `type str`).
- **Generated code reads columns through `__private::{column, get}`**, which are bounded by
  `shelf::FromColumn` (not `FromSqlOwned`) so that a type error reads well (PLAN.md §12 #11).
- **SQL written by a derive** quotes identifiers at macro time, passes every value as a `$n`
  parameter, and never selects `*`.
- **Shape the code to grow** (PLAN.md §3): one module per derive and per concern, so that later
  chapters add code instead of rewriting what earlier chapters quote.

## Writing the shelf book

These add to the root style contract:

- **Writing a chapter replaces its sketch**: same file; the card goes, the open questions are
  settled (in PLAN.md §10 or §12), and the status becomes `draft`, then `current`. Read the
  sketch first: its claims say what is measured, what is "(to measure)" and what is
  "(check)". Revise a sketch in place (with a new date on its card) when the plan changes.
- After changing any chapter or sketch, rebuild the PDF: `python3 tools/book.py --book orm
  book` (it contains every chapter, sketches included).

- Chapter files are `orm/book/NN-slug.md`, registered in `orm/book/book.toml`. Its `[book]`
  table holds the settings `book.py` reads.
- **The banner** names this book, then links the neighbouring chapters, as in the main book:
  `> **Building an ORM on tokio-postgres** · Previous: [Chapter N-1 — …](…) · Next: […](…)`.
- **Side-by-side sections** have three columns: **by hand** (a runnable block), **shelf** (a
  runnable block, possibly the same program), and an established crate as a ` ~~~rust `
  illustration whose first line names it (`// SeaORM (illustration)`). The closing table is
  `| | by hand | shelf | SeaORM |`, with **Same** and **Different** rows.
- **Callouts:** the four from the main book, plus `> **Macro lens:**` for what the compiler
  hands a macro and takes back (tokens, spans, hygiene, expansion order).
- **"In shelf" excerpts** start with `// orm/shelf-codegen/src/from_row.rs (excerpt)` (the
  path from the repository root) and are followed by a link to the file.
- **The build-along rules** (PLAN.md §3): each stage first as a runnable single-file program;
  the finished crate in excerpts; behaviour through runnable examples that use the real
  derives.
- **Expansions and compiler errors are captured, never typed:** run shelf-codegen through
  `shelf_playground::pretty`, and take errors from trybuild's `.stderr` files or a real build.
- **Links to the main book** are relative: `../../book/03-rows-and-types.md#anchor`.
- Examples become `orm/playground/examples/shelf_chNN_KK.rs`, which is generated and
  gitignored. The playground's own helpers sit between `// region: NAME` markers in
  `orm/playground/src/lib.rs`, and the chapter that introduces one mirrors it
  (`<!-- mirror: orm/playground/src/lib.rs#pretty -->`).

## Databases

The root list applies, plus:

- `tpbook_shelf_check` is **dropped and recreated** by every `book.py --book orm check`.
- `shelf_test` holds shelf's integration tests: `tests/common/mod.rs` creates it on first use
  and gives each test a schema `t_<uuidv7>` (on the connection's `search_path`), dropped
  afterwards.
- stockroom-shelf's tests and its `line_for_line` example use stockroom's own harness (by
  `#[path]`), so their databases are stockroom's: `stockroom_test_<uuid>`, cloned from
  `stockroom_template_<checksum>` and dropped at the end.

## Verified facts (S0, S1a, Chapters 0–4, S1b, S1c, S2, S3a, S3b, S3c)

- syn 3.0.9's derive types have the same shape as syn 2's (`DeriveInput`, `Data`, `Fields`),
  except `Field`: `mutability` became `modifiers: FieldModifiers`, and `default` is new.
  `syn::ext::IdentExt::unraw` strips the `r#` from a raw identifier.
- prettyplease 0.3.0 (on syn 3) formats a derive's output as rustfmt would. Its exact output
  for `FromRow` is pinned by `from_row::tests::each_field_reads_the_column_of_its_name`.
- A trait and a derive macro can share a name (`shelf::FromRow`), because traits and macros
  live in different namespaces.
- The two playgrounds build their examples into the same `target/debug/examples/`, hence the
  `shelf_` prefix (`example_prefix` in `orm/book/book.toml`).
- In tokio-postgres 0.7.18, `Row::try_get("name")` searches the columns by exact name, then
  again ignoring ASCII case; every `try_get`, by name or index, checks `accepts` per row. shelf
  matches names exactly (PostgreSQL folds unquoted names itself) and resolves them once.
- `Column::table_oid()` and `column_id()` exist; tokio-postgres reports 0 as `None`.
- syn 3.0.0 changed almost nothing a derive uses (`DeriveInput`, `Fields`, `Attribute`,
  `parse_nested_meta` are as before; `Field` as above); its breaking changes are in types,
  expressions, patterns and literals.
- A derive must declare its helper attribute (`attributes(shelf)`), or the compiler rejects
  `#[shelf(…)]` before the derive runs.
- `#[diagnostic::on_unimplemented]` works through a blanket impl (`FromColumn`).
- `cargo expand` is not installed on this machine, but it runs on stable: it sets
  `RUSTC_BOOTSTRAP=1` for `-Zunpretty=expanded` (PLAN.md §12 #7). The raw command works here
  the same way: `RUSTC_BOOTSTRAP=1 cargo rustc -p shelf-playground --example NAME
  --profile=check -- -Zunpretty=expanded`.
- `book.py` ignores fenced blocks inside blockquotes: never put a runnable block in a callout.
- `parse_nested_meta` stops at the first error inside one attribute, so shelf reports at most
  one parse error per `#[shelf(…)]`; darling 0.24.1 goes on (PLAN.md §12 #25).
- `#[diagnostic::on_unimplemented]` was stabilized in Rust 1.78.0 (the release notes in the
  toolchain's `share/doc/rust/html/releases.md`).
- A bound on a concrete type in a derive's `where` clause is reported at the derive's name, as
  the supertrait, without `FromColumn`'s message (PLAN.md §12 #24): keep bounds for generic
  types only.
- `#[shelf(json)]` reads through shelf's own `__private::Json<T>`, which reads an SQL `NULL` as
  JSON `null` (postgres-types' `Json<T>` refuses it whatever `T` is; PLAN.md §12 #26).
- The sources of darling 0.24.1 and sqlx 0.9.0 (sqlx-core, sqlx-macros-core, sqlx-postgres)
  are in `~/.cargo/registry/src/`; quote their behaviour from there.
- An executor trait whose methods return `impl Future + Send` works in axum handlers,
  `tokio::spawn` and the retry helper; `async fn` in the trait does not (PLAN.md §12 #28).
  Pass a pooled connection as `&object`: `&**object` would skip deadpool's statement cache.
- `CASE WHEN $f THEN $v ELSE col END` and `coalesce($n, col)` type a domain column's parameter as
  the base type; `SET col = $n` and `VALUES ($n)` as the domain itself (PLAN.md §12 #30).
- A `const` item can join `&'static str` constants of other types with `const fn`s, but not in
  a generic impl whose inner consts mention its parameters (E0401, PLAN.md §12 #32).
- A `const _: () = { if … { panic!("…") } };` in generated code turns a cross-type rule into a
  build error (E0080, "evaluation panicked: …"), as `Patch` does for the version field.
- **The query builder's values take no numeric conversions**: one `impl Value<i64> for i32`
  makes an integer literal fall back to `i32`, and `gt(3_000_000_000)` stops compiling
  (PLAN.md §12 #34). An inherent constant shadows a trait constant of the same name
  (`Product::VERSION` is the column): generated code writes `<M as Table>::…` (§12 #35).
- `filter(impl Into<Expr<M>>)` reports another model's condition as a missing `From` impl; a
  trait of shelf's own with `#[diagnostic::on_unimplemented]` (`Condition<M>`) says it plainly
  (§12 #41). Spanning generated calls on the attribute's tokens (`quote_spanned!`) puts a
  relation's type errors at `key = …`.
- An inherent `async fn`'s future leaks its auto traits, so it is `Send` for a generic
  executor; clippy (`manual_async_fn`) prefers it to `-> impl Future + Send` outside traits.
  Inside a trait, keep `impl Future + Send` (§12 #28).
- **`insert_many` puts its `unnest`s in the select list** (`SELECT unnest($1::…[]), …`): in
  `FROM`, `unnest` expands a composite into its fields (§12 #71). A statement takes at most
  65,535 parameters (the protocol counts them in 16 bits).
- **`unnest` needs a cast per array, and the prepared single-row `INSERT` gives the right one**
  (a domain as the domain); an array column cannot go through `unnest` (§12 #37). **Preparing a
  `COPY` checks nothing**, not even the table, so the guard for #789 is the prepared `INSERT`
  (§12 #38).
- **A cached statement describes a table as it was**: `verify` prepares without the cache
  (`Executor::prepare_uncached`, §12 #43). A view's columns report the view's OID, where
  `attnotnull` is always false (§12 #40).
- **Outside a transaction, every insert commits and waits for its WAL flush** (20,000 pipelined:
  0.91–0.98 s against 0.13–0.14 s in one transaction, §12 #46); pgload's timings include its
  setup and commit round trips, `bench_bulk`'s do not (§12 #45).
- A test that holds a `Transaction` open when `TestDb` drops deadlocks: the drop blocks the
  test's runtime while `DROP SCHEMA` waits for the transaction's locks, and the rollback it
  queued cannot be sent. End transactions explicitly in tests.
- **Probing compiler errors**: a scratch workspace with a proc-macro crate and a user crate that
  depends on shelf by path (`shelf = { path = "…/orm/shelf" }`) builds with `cargo build
  --offline`; run cargo from the scratch workspace's root, so that paths in the errors read
  `user/src/bin/NAME.rs`.
- **stockroom from another crate** (S2): every module is public, except `http`'s handler
  modules and `app::cancel_on_signal`; its test harness is reusable by `#[path]`, without an
  `#[allow(dead_code)]` on the module (the file has one; clippy refuses the duplicate).
  `impl From<shelf::Error> for AppError` is E0117 outside both crates: convert with
  `stockroom_shelf::app_error` (PLAN.md §12 #47, #49).
- **axum 0.8.9 refuses a `Router` in `route_service`** ("Use `Router::nest` instead");
  `router.into_service()` is accepted and keeps the path. A route `/x/{param}` captures `/x/y`
  even when another router behind `fallback_service` has `/x/y` (§12 #50).
- **A virtual generated column is nullable in the catalog** unless declared `NOT NULL`, so
  `verify` refuses a non-`Option` field for stockroom's `stock.available` (§12 #48).
- **A shelf `Patch` always checks its version** (`version = $n`): "any version" takes a read and
  a retry on `Stale` (§12 #51). A `ModelStream` can live inside an `async_stream` generator that
  owns its connection, which is how an HTTP body streams models (§12 #52).
- **`Config::host` and `Config::port` append** (multi-host): a copied config keeps its first
  host; build a new `Config` to point a connection elsewhere (§12 #56).
- **deadpool's `StatementCache::remove(sql, &[])`** returns the statement if the connection
  prepared it: the way a test tells which SQL a route sent (§12 #53).
- **Every row of a statement shares its column list** (`std::ptr::eq(row.columns(),
  statement.columns())`, 0.7.18). One `resolve` of 12 fields costs 262–268 ns in release
  against a 46–47 µs localhost round trip, so `Columns` is not cached (§12 #57, #59). Only
  `Send` on `FromRow::Columns` is required by shelf's code (§12 #60).
- **A row with a duplicated column name**: shelf and `Row::try_get` read the first, sqlx 0.9 the
  last; sqlx checks a type only for non-NULL values (§12 #61). tokio-postgres's per-row
  `accepts` turns a row read with another statement's positions into an error (§12 #62).
- **An impl may write `async fn` for a trait method declared `-> impl Future + Send`**; the
  compiler checks that impl's future, and clippy's `manual_async_fn` prefers it to an
  `async move` body (§12 #65).
- **Preparing a write checks less than it seems**: a `GENERATED ALWAYS` column named in an
  `INSERT` fails at *Parse* (428C9), but an `ON CONFLICT` target with no matching unique index
  prepares fine and fails when run (42P10, found while planning). An upsert that updates still
  draws an identity value (§12 #66).
- **Output that changes per session** breaks a captured `text` block: a temporary domain is
  reported as `pg_temp_86.sku`, with a number per session. Put such objects in a named schema
  that the program creates and drops (§12 #66).
- **A `json` column in a `Patch`** takes a field of type `Option<tokio_postgres::types::Json<T>>`
  or `Option<serde_json::Value>`, without the attribute, which the derive refuses (§10).
- **serde reads `null` into `Option<Option<T>>` as `None`**, like an absent field; the one-line
  `Option::<T>::deserialize(d).map(Some)` with `#[serde(default)]` tells them apart (serde_with
  3.24.0's `serde_with::rust::double_option`, not a dependency; §12 #67).
- **A cached statement after a migration** (S3c): dropping a column it names makes its next
  execution fail with 42703 (PostgreSQL analyzes its text again); adding a column makes a cached
  `SELECT *` fail with 0A000. Either way the client's description is the old one (§12 #72).
- **Capturing a session** (S3c): `NO_COLOR=1` keeps tracing's colour codes out of a log, and a
  panic's first line carries the thread id (`thread 'main' (25976639)`), which changes per run.
- **SeaORM 2.0.4 and diesel 2.3.14 sources are in `~/.cargo/registry/src/`**, beside sqlx 0.9.0
  and darling 0.24.1: the side-by-sides quote them from there (§12 #68).
