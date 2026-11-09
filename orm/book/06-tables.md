# Chapter 6 — Tables: the model derive

> **Building an ORM on tokio-postgres** · Previous: [Chapter 5 — Checking a statement once, not every row](05-column-indices.md) · Next: [Chapter 7 — The executor and shelf's errors](07-executor.md)

> **Sketch, 9 November 2026.** A plan of this chapter, not its text; it will be replaced by
> the chapter. Part II, first chapter. Builds on Chapters 3–5, and on main book
> [Chapter 2](../../book/02-queries.md) (`quote_ident`), [Chapter 4](../../book/04-prepared-statements.md)
> (the "cached plan must not change result type" error) and
> [Chapter 14](../../book/14-data-layer.md) (the repository layer). shelf: `#[derive(Table)]`
> in `shelf-codegen/src/table.rs`, `sql.rs`, the `Table` trait in `shelf/src/table.rs`,
> `Error::NotFound`, and the key ownership in `attrs.rs` (built, S1b). Crates: syn 3.0.9,
> tokio-postgres 0.7.18. Decisions: PLAN.md §10 (SQL as literals; who reports which error; the
> `Table` trait; what `Table` refuses). Findings: §12 #32. Plan: [orm/PLAN.md](../PLAN.md)
> §4.0b, §4.2, §4.3, §7, Chapter 6.

`FromRow` reads any query's rows, but you still write every query. Most of them are the same
few: fetch one row by its key, fetch them all, count them. The struct already knows its columns;
what it lacks is the table they come from and which of them is the key. Two attributes supply
that, and with them the derive can write the everyday `SELECT`s itself.

It writes them at compile time. The macro knows every name, so it can assemble each statement
as a string literal, with every identifier quoted, and leave nothing to build at run time.
Values never go into those strings: they stay `$1` parameters, as the main book insisted from
its second chapter. What the macro cannot put in a literal, because it lives in another type,
is where `LazyLock` comes in.

**In this chapter you will learn:**

- what a table adds to a row type: a name, a key and a column list
- how `#[derive(Table)]` reads `table`, `schema`, `primary_key` and `generated`
- how a derive writes SQL at compile time, quoting identifiers so that a table called `order`
  works
- `find`, `find_opt`, `all` and `count`, and the SQL each one sends
- why shelf never writes `SELECT *`
- where const evaluation stops, and what `LazyLock` is for

---

## 6.1 What a table adds

Plan:
- A model is a `FromRow` struct plus a table: `type Key`, the constants `MODEL`, `TABLE`,
  `COLUMNS`, `FIND`, `ALL`, `COUNT`, and `key()` and `key_params()` (built, S1b; PLAN.md
  §4.0b).
- `Table` requires `FromRow` (a supertrait): a model is always readable.

## 6.2 `#[derive(Table)]`

Plan:
- `#[shelf(table = "products")]` on the struct (and `schema` when needed);
  `#[shelf(primary_key)]` on one field, or on several for a composite key read as a tuple;
  `#[shelf(generated)]` for columns the database fills (`uuidv7()`, `now()`, identity),
  which writes will leave out (Chapter 8).
- The new keys in `attrs.rs`, and who reports a mistake in them: each key has an owner, so a
  struct with `FromRow` and `Table` hears about each mistake once (PLAN.md §10). Chapter 4's
  §4.2 already shows `Derive::reports`. What `Table` refuses: no `table`, no key, `flatten`,
  type parameters (`tests/ui/table.rs`).

## 6.3 SQL at compile time

Plan:
- The derive builds `SELECT "id", "sku", … FROM "products" WHERE "id" = $1` as a literal and
  emits it as an associated `const` (built: `tests/expand/table.expanded.rs`).
- Quoting at macro time: a column called `type` or a table called `order` works; the
  `quote_ident` of the main book's Chapter 2, run by the compiler.
- A **Pitfall** box: an identifier is quoted, a value is a parameter; nothing a user types at
  run time ever reaches the generated SQL.

## 6.4 Reading

Plan:
- `find(&db, &key)` (one row, else `Error::NotFound`), `find_opt`, `all`, `count`: each with
  its SQL printed and its round trips counted.
- An explicit column list, never `*`: a cached statement that selected `*` fails with 0A000
  after `ALTER TABLE … ADD COLUMN` (root PLAN.md §12 #42), and `*` would hand `resolve` columns
  the struct does not have.

## 6.5 Metadata as consts, and where they stop

Plan:
- `TABLE` and `COLUMNS` as consts are free. `concat!` takes literals only, but `const fn`s can
  join another type's constants into an array in a `const` item (PLAN.md §12 #32): that is
  how Chapter 8's `Insert` names its model's table, and the statement is still a constant.
- Where it stops: a generic impl whose inner constants mention its parameters (E0401, captured
  in §12 #32), and computing, such as numbering `$n` after another type's key, which is where
  Chapter 9's `Patch` uses a `LazyLock`. A **Rust refresher** on `const` items and `const fn`.

## The programs

- `run` (proposed): a model's `TABLE` and `COLUMNS` printed; the generated SQL printed.
- `run`: `find`, `find_opt`, `all` and `count` against a temporary table.
- `run`: a table called `order` with a column called `type`.
- `run`: `find` on a missing key, matching `Error::NotFound`.
- `run`: the expansion of a small model, printed by running the generator.

## Side by side

Fetching by primary key: by hand (`fetch_one` with the SQL written out) · shelf
(`Product::find(&db, &id)`, built) · SeaORM (`Entity::find_by_id(id).one(&db)`, check).

## The data

A temporary `products` table with an identity key; a second table with a composite key
(`stock (store_id, product_id)`, as in stockroom).

## Claims

- Docs: PostgreSQL's rules for identifiers and key words (the documentation's "Lexical
  Structure"); root PLAN.md §12 #42 for the cached-plan error.
- Opinion: an ORM should make the SQL it sends easy to predict; constants make it easy to
  print.

## When not to use `Table`

- Reports, joins and aggregates: SQL and `fetch_all` with a `FromRow` struct.
- Views: a `FromRow` struct suffices, since there is no key to find by (or declare one; check
  whether that is useful).

## Exercises

Plan:
1. Add `exists(&db, &key)`. *Hint:* `SELECT EXISTS (SELECT 1 FROM … WHERE …)`.
2. Let a model leave its key out of `all`'s `ORDER BY`. *Hint:* a container key, and its
   owner in `KEYS`.
3. Find several rows by key in one statement. *Hint:* `= ANY($1)`, which Chapter 11 needs too.

> **Open questions.** Settled in S1b (PLAN.md §10): the trait's items; a composite key as an
> owned tuple, `find(&db, &(store, product))`; compile-time joining rather than `LazyLock`; and
> `count` here, not in the query builder. Still open: whether `flatten` can join a model once
> `FromRow` knows its own columns.

---

Previous: [Chapter 5 — Checking a statement once, not every row](05-column-indices.md) · Next: [Chapter 7 — The executor and shelf's errors](07-executor.md)
