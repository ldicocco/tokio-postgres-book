# Chapter 6 — Tables: the model derive

> **Building an ORM on tokio-postgres** · Previous: [Chapter 5 — Checking a statement once, not every row](05-column-indices.md) · Next: [Chapter 7 — The executor and shelf's errors](07-executor.md)

> **Sketch, 7 November 2026.** A plan of this chapter, not its text; it will be replaced by
> the chapter. Part II, first chapter. Builds on Chapters 3–5, and on main book
> [Chapter 2](../../book/02-queries.md) (`quote_ident`), [Chapter 4](../../book/04-prepared-statements.md)
> (the "cached plan must not change result type" error) and
> [Chapter 14](../../book/14-data-layer.md) (the repository layer). shelf: `#[derive(Table)]`
> in `shelf-codegen/src/table.rs`, the `Table` trait and `Error::NotFound` (proposed, S1b
> step 3). Crates: syn 3.0.9, tokio-postgres 0.7.18. Decisions: PLAN.md §10 (SQL as literals
> generated at compile time). Plan: [orm/PLAN.md](../PLAN.md) §4.2, §4.3, §7, Chapter 6; §13,
> S1b step 3.

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
- A model is a `FromRow` struct plus a table: `TABLE`, `COLUMNS`, the key's type and a way to
  get it from a value (proposed names, settled in S1b).
- `Table` requires `FromRow` (a supertrait): a model is always readable.

## 6.2 `#[derive(Table)]`

Plan:
- `#[shelf(table = "products")]` on the struct (and `schema` when needed);
  `#[shelf(primary_key)]` on one field, or on several for a composite key read as a tuple;
  `#[shelf(generated)]` for columns the database fills (`uuidv7()`, `now()`, identity),
  which writes will leave out (Chapter 8).
- The new keys added to `attrs.rs`; `FromRow` ignores them, so the UI test that refuses
  `table` today changes (PLAN.md §13).

## 6.3 SQL at compile time

Plan:
- The derive builds `SELECT "id", "sku", … FROM "products" WHERE "id" = $1` as a literal and
  emits it as an associated `const` (proposed).
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
- `TABLE` and `COLUMNS` as consts are free. A literal cannot include another type's const,
  though: `concat!` takes literals only, so SQL that needs a `Table`'s name from elsewhere (an
  `Insert` struct naming its model, Chapter 8) is built once in a `LazyLock<String>`.
- A **Rust refresher** on `LazyLock` (stable since Rust 1.80; check) and why it is not a
  `const`.

## The programs

- `run` (proposed): a model's `TABLE` and `COLUMNS` printed; the generated SQL printed.
- `run`: `find`, `find_opt`, `all` and `count` against a temporary table.
- `run`: a table called `order` with a column called `type`.
- `run`: `find` on a missing key, matching `Error::NotFound`.
- `run`: the expansion of a small model, printed by running the generator.

## Side by side

Fetching by primary key: by hand (`fetch_one` with the SQL written out) · shelf
(`Product::find(&db, &id)`, proposed) · SeaORM (`Entity::find_by_id(id).one(&db)`, check).

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
2. Support `schema` and print the qualified name. *Hint:* quote each part separately.
3. Find by a composite key. *Hint:* the key type is a tuple.

> **Open questions.** (1) The trait's exact items and names (`TABLE`, `COLUMNS`, `Key`,
> `key()`) are proposed; settle them in S1b. (2) How `find` takes a composite key: a tuple of
> references or of owned values. (3) `concat!` against `LazyLock` (S1b probe). (4) Whether
> `count` belongs here or in the query builder (Chapter 10).

---

Previous: [Chapter 5 — Checking a statement once, not every row](05-column-indices.md) · Next: [Chapter 7 — The executor and shelf's errors](07-executor.md)
