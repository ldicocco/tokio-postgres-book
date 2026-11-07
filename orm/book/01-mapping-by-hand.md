# Chapter 1 — Mapping rows by hand

> **Building an ORM on tokio-postgres** · Previous: [Chapter 0 — Introduction and setup](00-introduction.md) · Next: [Chapter 2 — Declarative macros](02-declarative-macros.md)

> **Sketch, 7 November 2026.** A plan of this chapter, not its text; it will be replaced by
> the chapter. Part I, second chapter. Builds on main book
> [Chapter 3](../../book/03-rows-and-types.md) (`Row`, `get` and `try_get`, NULL,
> `TryFrom<&Row>`) and [Chapter 4](../../book/04-prepared-statements.md) (what `query` sends).
> shelf: the first form of the `FromRow` trait, with `from_row` only, written by hand in the
> chapter's programs; shelf's own trait is built (S1a) and grows in Chapter 5. Crates:
> tokio-postgres 0.7.18. Findings: PLAN.md §12 #4 (how a name is looked up), #8 (what it
> costs). Plan: [orm/PLAN.md](../PLAN.md) §7, Chapter 1.

You have written this function before. Every struct that comes out of the database in
stockroom has a `TryFrom<&Row>` that names each field and reads each column: `id:
row.try_get("id")?`, `code: row.try_get("code")?`, and so on, once per field, for every
struct. The SQL above it names the same columns again. Change a column and you change three
places, and the compiler checks none of them against the table.

An ORM's first job is to write that function for you. Before writing the macro that does it,
this chapter writes it by hand once more, carefully: as a trait of shelf's own, with the three
ways it can fail, and with a measurement of what reading a column by name actually costs. The
numbers decide the design of the derive two chapters from now.

**In this chapter you will learn:**

- why shelf has its own `FromRow` trait instead of using `TryFrom<&Row>`
- what `Row::try_get("sku")` does in tokio-postgres 0.7.18, and what it costs per row
- the three ways a mapping fails (a missing column, a wrong type, a NULL) and what each error
  says
- how to nest one struct inside another when both come from the same row
- a `fetch_all` helper that runs a query and maps its rows
- which parts of the hand-written code a macro should take over

---

## 1.1 A trait of shelf's own

Plan:
- `trait FromRow: Sized { fn from_row(row: &Row) -> Result<Self, Error>; }`, with
  `tokio_postgres::Error` as the error for now (shelf's own `Error` arrives with Chapter 4's
  better messages).
- Why not `TryFrom<&Row>`: it works (stockroom uses it), but a trait of shelf's own can grow
  associated items (Chapter 5's `Columns`), have impls for tuples (Chapter 2), and carry a
  name the derives can build on. The orphan rule is not the reason: both are fine for your
  own types.
- The first impl, by hand, for a `Product` with five fields.

## 1.2 By name or by index

Draft prose:

`row.try_get("sku")` looks like a map lookup, but a row has no map. In 0.7.18 it walks the
row's columns and compares each name with `"sku"`; if none matches, it walks them again,
ignoring ASCII case. Then, whether you asked by name or by position, it checks that the
column's type can be read into the Rust type you asked for, and only then decodes the bytes.
A twelve-field struct read by name therefore does up to twelve searches per row, each over up
to twelve names.

Plan:
- The source, quoted from `row.rs` (`RowIndex for str`), with the FIXME about case.
- The measurement: 100,000 rows of 12 columns, read by name and by position (PLAN.md §12 #8:
  about 3× in a release build). The chapter's runnable version runs in a debug build, so it
  asserts only that by position is faster, and prints both times.
- What it means: the name lookup, not the decoding, dominates; the per-row type check costs
  nothing measurable.

## 1.3 Three ways to fail

Plan:
- A missing column ("invalid column `sku`"), a wrong type (the `WrongType` message), a NULL
  read into a non-`Option` field (`WasNull`): each provoked and its message captured from a
  real run, as tokio-postgres reports it.
- What those messages lack: the struct and the field. Chapter 4's errors add them.

## 1.4 Nested structs

Plan:
- A `Price { cents, currency }` inside a `Product`, both read from one row: the inner struct's
  `from_row` called from the outer one's.
- The catch: both use names from the same row, so the inner struct's columns must not clash
  with the outer's. Chapter 4's `flatten` does the same with one attribute.

## 1.5 A fetch helper

Plan:
- `fetch_all::<T: FromRow>(client, sql, params)`: `query`, then `from_row` on each row,
  collected into a `Result<Vec<T>, _>`. `fetch_one` and `fetch_opt` beside it.
- Taking tokio-postgres's `GenericClient`, so that a transaction works too (main book
  [Chapter 8](../../book/08-transactions.md)).

## 1.6 Counting the boilerplate

Plan:
- Each field is named three times: in the struct, in the `SELECT` list, and in `try_get`.
  The struct's name appears in every error message you write by hand.
- What a macro can take over (the `from_row` body, the error context), and what it cannot
  (the SQL, until Chapter 6).

## The programs

- `run`: a hand-written `FromRow` trait and its impl for `Product`, reading a temporary table;
  asserts the values, including a NULL into an `Option`.
- `run`: by name against by position on 100,000 generated rows; asserts that by position is
  faster and prints both (the ratio is measured in debug when the chapter is written).
- `run`: the three failures, each matched and its message printed.
- `run`: the nested `Price`, and `fetch_all` over a client and over a transaction.

## Side by side

A row into a struct: by hand (this chapter's impl) · shelf (a preview: `#[derive(FromRow)]`,
built) · sqlx (`#[derive(sqlx::FromRow)]`, illustration).

## The data

A temporary `products` table (`id`, `sku`, `price_cents`, `tags text[]`), and
`generate_series` for the measurement.

## In stockroom

`TryFrom<&Row> for Store` in [stockroom/src/repo/stores.rs](../../stockroom/src/repo/stores.rs),
the code this chapter starts from.

## Claims

- Measured: by name about 3× by position, release build (PLAN.md §12 #8); the debug ratio (to
  measure).
- Docs: `Row::try_get` (docs.rs, 0.7.18) and the source of `row.rs`.
- Opinion: once a project has more than a handful of row types, a mapping trait of its own
  pays for itself.

## When not to use a mapping trait

- A single value: `query_one_scalar` (main book [Chapter 2](../../book/02-queries.md)).
- A one-off query read in place: `row.get(0)` is clearer than a struct.

## Exercises

Plan:
1. Implement `FromRow` for a struct with an `Option<Vec<String>>`. *Hint:* `Option` handles
   the NULL, `Vec` the array.
2. Make a missing column's error name the struct. *Hint:* map the error, add the name.
3. Measure by name with the columns in the struct's order and in reverse. *Hint:* the search
   stops at the first match.

> **Open questions.** (1) The ratio in a debug build, which the runnable example sees, and
> whether a relative assertion stays reliable (to measure). (2) Whether 1.5's `fetch_all` should
> already prepare first, as shelf's does, or leave that to Chapter 5's story. (3) How much of
> `row.rs` to quote: the two searches and the `accepts` check, nothing else.

---

Previous: [Chapter 0 — Introduction and setup](00-introduction.md) · Next: [Chapter 2 — Declarative macros](02-declarative-macros.md)
