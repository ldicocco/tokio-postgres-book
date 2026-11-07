# Chapter 5 — Checking a statement once, not every row

> **Building an ORM on tokio-postgres** · Previous: [Chapter 4 — Attributes, errors and generics](04-attributes.md) · Next: [Chapter 6 — Tables: the model derive](06-tables.md)

> **Sketch, 7 November 2026.** A plan of this chapter, not its text; it will be replaced by
> the chapter. Part I, sixth chapter. Builds on Chapters 1 and 4, and on main book
> [Chapter 4](../../book/04-prepared-statements.md) (`prepare`, `Statement::columns`).
> shelf: `FromRow::{Columns, resolve, from_row_at}`, `from_rows` and `fetch_*` in
> [row.rs](../shelf/src/row.rs) and [fetch.rs](../shelf/src/fetch.rs), and the benchmark
> [bench_mapping.rs](../shelf/examples/bench_mapping.rs) (all built, S1a). Crates:
> tokio-postgres 0.7.18. Findings: PLAN.md §12 #4, #8, #13. Plan:
> [orm/PLAN.md](../PLAN.md) §4.0, §7, Chapter 5.

Chapter 1 measured what a lookup by name costs, and Chapter 3's derive does one per field per
row. Everything that lookup finds depends only on the statement, not on the row: the position
of `sku` is the same in the first row and in the millionth. So shelf finds each field's column
once, when it sees the statement, and then reads every row by position.

The same step is the right moment to check types. Instead of discovering on the first row
that `price_cents` is `text` and the field is `i64`, shelf checks every field against the
statement's columns before the query runs, and reports the problem with the struct's name, the
field's and the column's. Because tokio-postgres prepares a statement anyway when you pass it
a string, the check costs no extra round trip.

**In this chapter you will learn:**

- what a statement knows about its columns before any row exists
- how `resolve` finds each field's column and checks its type once per statement
- what the derive generates for this: a `Columns` tuple and `from_row_at`
- why a wrong mapping fails before the query runs, even an `INSERT … RETURNING`
- what it is worth, measured on 100,000 rows

---

## 5.1 What a lookup by name costs, again

Draft prose:

On 100,000 rows of twelve columns, reading a struct by name took 38.2–38.5 ms in a release
build, and reading it by position 12.4–12.6 ms (PLAN.md §12 #13, Apple Silicon, PostgreSQL 18.6
on localhost). The decoding is the same in both; the difference is the searching. Checking
each column's type on every row, which tokio-postgres does whether you ask by name or by
position, made no measurable difference (§12 #8).

## 5.2 A statement knows its columns

Plan:
- `client.prepare(sql)` returns a `Statement` whose `columns()` give each column's name and
  type, before any row; a `Row` carries the same columns.
- `resolve(columns) -> Result<Columns, Error>`: each field's position, found by exact name
  (PostgreSQL folds unquoted names itself, so shelf does not retry ignoring case, unlike
  `Row::try_get`; §12 #4).

## 5.3 Types checked once

Plan:
- `FromSql::accepts(column.type_())`, called once per field per statement, under shelf's
  `FromColumn` bound.
- The errors: `Mapping::MissingColumn`, `Mapping::WrongType` (with the SQL type and the Rust
  type, shortened: `Option<String>`, not `core::option::Option<alloc::string::String>`), and,
  per row, `Mapping::Null` and `Mapping::Decode`.
- The proof that the check comes first: an `INSERT … RETURNING` whose result lacks a field's
  column fails, and the table still has the rows it had (from `tests/from_row.rs`).

## 5.4 What the derive generates

Plan:
- The expansion of a two-field struct, printed: `type Columns = (usize, usize)`, `resolve`
  calling `__private::column::<T>` per field, `from_row_at` calling `__private::get::<T>` with
  `columns.0`, `columns.1`.
- `flatten` puts the nested type's `Columns` inside the tuple; `skip` puts `()`.
- `from_row` stays, as a provided method that resolves and reads one row.

## 5.5 Measured

Draft prose:

shelf resolving once took 13.0–13.1 ms on the same 100,000 rows: about 4% over the
hand-written code that reads by position, and a third of the time of reading by name.
Resolving again for every row, which is what calling `from_row` in a loop does, took
38.2–38.4 ms, as much as the lookups by name it repeats (§12 #13). That is why `fetch_all`
and `from_rows` resolve once, and why the chapter's advice is to use them.

Plan:
- The table printed by `cargo run -p shelf --release --example bench_mapping`, pasted as
  captured.

## 5.6 Where it is used

Plan:
- `fetch_all`, `fetch_one`, `fetch_opt`: prepare, resolve against the statement, query.
- `from_rows` for rows you already have; streams resolve once too (Chapter 12).

## The programs

- `run`: `fetch_all` into a struct, and the same rows through `from_row` in a loop, with both
  times printed and a relative assertion (debug build; to measure).
- `run`: the `INSERT … RETURNING` that never runs, and the row count unchanged.
- `run`: a wrong type reported for a query that returns no rows.
- The benchmark, as a `bash` command and its table (release; not run by `book.py check`).

## Side by side

Reading 100,000 rows: by hand, by position · shelf (`from_rows`) · sqlx, whose rows look up
names in a map built per statement (check in sqlx-postgres's source).

## In shelf

The trait from `row.rs`, `column` and `get` from `row.rs`, and `fetch_all` from `fetch.rs`.

## Claims

- Measured: §12 #8 and #13 (release, Apple Silicon, PG 18.6); the debug ratio (to measure).
- Docs: `Statement::columns`, `Column::type_` (docs.rs, 0.7.18).
- Opinion: an error before the first row is worth more than the speed.

## When not to resolve once

- A single row: `from_row` is the same work, and simpler to call.
- Rows from different statements: their `Columns` are not interchangeable.

## Exercises

Plan:
1. Prepare a statement once, resolve once, and run it twice with different parameters.
   *Hint:* `Columns` belongs to the statement, not to the execution.
2. Measure with three columns instead of twelve. *Hint:* the search grows with both.
3. Make `resolve` refuse a result with two columns of the same name. *Hint:* a join's `id`.

> **Open questions.** (1) Whether a relative timing assertion is reliable in a debug build
> (to measure). (2) How sqlx finds columns by name (check). (3) Whether S1b's executor should
> cache `Columns` with each prepared statement, so that `resolve` runs once per connection
> rather than once per call; a design question for Chapter 7.

---

Previous: [Chapter 4 — Attributes, errors and generics](04-attributes.md) · Next: [Chapter 6 — Tables: the model derive](06-tables.md)
