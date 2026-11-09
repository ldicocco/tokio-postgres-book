# Chapter 13 — Trusting the mapping: schema checks and tests

> **Building an ORM on tokio-postgres** · Previous: [Chapter 12 — Bulk writes and streams](12-bulk-and-streams.md) · Next: [Chapter 14 — Capstone: stockroom on shelf](14-capstone.md)

> **Sketch, 9 November 2026 (revised after S1c).** A plan of this chapter, not its text; it
> will be replaced by the chapter. Part III, fourth chapter. Builds on Chapters 4 and 5, and on
> main book [Chapter 4](../../book/04-prepared-statements.md) (`Statement::columns`) and
> [Chapter 17](../../book/17-testing.md) (testing against a real database). shelf:
> `shelf::verify`, `Error::Schema` and `Mismatch` (`shelf/src/verify.rs`, `error.rs`),
> `Table::FIELDS` and `Executor::prepare_uncached` (built, S1c); the test suites (built, S1a):
> [tests/expand.rs](../shelf-codegen/tests/expand.rs), [tests/ui.rs](../shelf/tests/ui.rs),
> [tests/from_row.rs](../shelf/tests/from_row.rs), [tests/verify.rs](../shelf/tests/verify.rs)
> and the harness [tests/common/mod.rs](../shelf/tests/common/mod.rs). Crates: tokio-postgres
> 0.7.18, trybuild 1.0.122. Findings: PLAN.md §12 #6, #11, #12, #40, #43. Plan:
> [orm/PLAN.md](../PLAN.md) §4.0c, §6, §7, Chapter 13; §10 (`verify`).

A struct and a table are two descriptions of the same data, kept in different places by
different people. A migration renames a column; a column becomes nullable; a `bigint` becomes
`numeric`. Chapter 5's check catches each of those, but only when the query runs, which may
be the first request after a deploy, or a nightly job that runs at three in the morning.

This chapter moves the check earlier. `shelf::verify` prepares each model's own `SELECT` at
startup, compares its columns with the struct's fields, asks the catalog which columns may be
NULL, and reports every mismatch at once. Then it turns to the other side of trust, the
macros themselves, and shows the three kinds of test shelf has used since phase S1a: expansion
tests, UI tests and tests against the database, with what each one catches and what it cannot.

**In this chapter you will learn:**

- what can drift between a struct and its table, and when each kind of drift is caught
- how `verify` uses `Statement::columns()` and the catalog to check a model before it is used
- where to run it: at startup, and in a test per model
- how to test a derive: expansion tests, UI tests, integration tests
- what compile-time checking (sqlx's `query!`, Cornucopia) adds, and what it costs

---

## 13.1 What can drift

Plan:
- A renamed or dropped column, a changed type, a column made nullable, a new `NOT NULL`
  column a `New…` struct lacks: each with when it is caught today (Chapter 5: at the query; a
  NULL: at the row).

## 13.2 `verify`

Plan:
- Prepare `SELECT * FROM` the table (no rows read). Not the model's own `SELECT`: a statement
  that names a missing column fails at the first one, and `verify` wants them all. The derive
  lists each field's column and the type it reads in `Table::FIELDS` (`Field::new::<T>`, `fn`
  pointers to `column::<T>` and a NULL test), and each is checked against the statement.
- Prepared without the statement cache (`Executor::prepare_uncached`): a cached statement
  describes the table as it was, and a test shows `verify` missing a dropped column through
  deadpool's cache (§12 #43).
- Nullability is not in a row description. `Column::table_oid()` and `column_id()` (§12 #6)
  identify the table column, and `pg_attribute.attnotnull`, one query for every column, says
  whether it can be NULL; a nullable column read into a field that refuses NULL is
  `Mapping::Nullable`. A generated column is nullable unless declared `NOT NULL`, which PG 18
  allows on a virtual one (§12 #40).
- A view's columns report the view's own OID, where every column looks nullable: `verify`
  checks their names and types and leaves NULLs alone (`pg_class.relkind`).
- Every mismatch in `Error::Schema { model, mismatches }`, in field order; its message,
  captured from the test, printed.

## 13.3 Where to run it

Plan:
- At startup, before the server accepts requests, so a bad deploy fails its readiness check
  (main book Chapter 16).
- In a test per model against a migrated database (main book Chapter 17's template).

## 13.4 Testing a derive

Draft prose:

shelf tests its derives three ways, because each catches what the others cannot. An expansion
test runs the generator on a struct and compares the pretty-printed code with a file: it
catches any change to what users get, and its diff is the review. A UI test compiles a broken
struct and compares the compiler's errors with a file: it pins down the message and where it
points. An integration test derives for real and runs queries against PostgreSQL: it is the
only one that proves the generated code does the right thing.

Plan:
- Excerpts: `tests/expand.rs` and one `.expanded.rs` file; `tests/ui.rs` and one `.stderr`
  file; one integration test with the harness.
- Blessing: `SHELF_BLESS=1` and `TRYBUILD=overwrite`, and why the diff must be read.

## 13.5 Compile-time checking

Plan:
- sqlx's `query!` checks SQL against a live database (or saved metadata in offline mode) at
  compile time (docs; check the current details); Cornucopia generates Rust from SQL (main
  book [Chapter 14](../../book/14-data-layer.md)).
- What they buy (no drift reaches a build) and cost (a database at build time, or metadata to
  keep in sync); where `verify` sits between.

## The programs

- `run` (proposed): `verify` on a model that matches its table.
- `run`: a renamed column, the report printed.
- `run`: a nullable column read into `String`, reported from the catalog.
- Excerpts of the test files named above, checked by `book.py excerpts`.

## Side by side

Catching a renamed column: by hand (at the first query) · shelf (`verify` at startup) · sqlx
(`query_as!` at compile time, check).

## Claims

- Docs: PostgreSQL's `pg_attribute` catalog; sqlx's offline mode (check).
- Opinion: check the mapping at startup; a deploy that cannot run should fail before it takes
  traffic.

## When not to use `verify`

- Queries written by hand: `verify` checks models' own SQL; a `FromRow` struct read from a
  custom query is checked when that query is prepared (Chapter 5).

## Exercises

Plan:
1. Run `verify` for every model in one call. *Hint:* a list of functions, one per model.
2. Report extra columns the struct does not read. *Hint:* the statement has more columns than
   `resolve` used.
3. Write an expansion test for a struct of your own. *Hint:* add a file to `tests/expand/`.

> **Open questions.** Settled in S1c (PLAN.md §10): a view's columns are checked for names and
> types, not NULLs; `Error::Schema { model, mismatches }`, in field order; `New…` structs are
> not checked (an exercise). Still open: whether exercise 1 needs a trait object over models
> (`verify` is generic) or a list of function pointers.

---

Previous: [Chapter 12 — Bulk writes and streams](12-bulk-and-streams.md) · Next: [Chapter 14 — Capstone: stockroom on shelf](14-capstone.md)
