# Chapter 8 — Writing rows: insert, update, delete, upsert

> **Building an ORM on tokio-postgres** · Previous: [Chapter 7 — The executor and shelf's errors](07-executor.md) · Next: [Chapter 9 — Partial updates and optimistic locking](09-patch-and-versions.md)

> **Sketch, 7 November 2026.** A plan of this chapter, not its text; it will be replaced by
> the chapter. Part II, third chapter. Builds on Chapters 6 and 7, and on main book
> [Chapter 2](../../book/02-queries.md) (`RETURNING`, `ON CONFLICT`),
> [Chapter 6](../../book/06-errors.md) (constraint errors) and
> [Chapter 10](../../book/10-copy.md) (`MERGE`). shelf: `#[derive(Insert)]` in
> `shelf-codegen/src/insert.rs`, `update`, `delete`, `delete_by_key` and upsert in
> `shelf/src/write.rs` (proposed, S1b step 4). Crates: tokio-postgres 0.7.18. PostgreSQL:
> `uuidv7()` (PG 18), `MERGE … RETURNING` (PG 17). Plan: [orm/PLAN.md](../PLAN.md) §4.2–§4.4,
> §7, Chapter 8; §13, S1b step 4.

Reading is half of a model layer. The other half is writing, and the first question is what
type you write. A `Product` read from the database has an `id` and a `created_at`, but a
product you are about to create has neither: the database will assign them. Making those
fields `Option`s would spread `unwrap()`s through every reader of the model. shelf does what
stockroom already does by hand, and gives creation its own struct, `NewProduct`, with only
the columns you supply.

`#[derive(Insert)]` on that struct writes the `INSERT`, and `RETURNING` hands back the full
model, defaults filled in, in the same round trip. Updates and deletes go through the model
itself, by its key, and an upsert is one more clause. Each method's SQL is printed, and each
one says what happens when a constraint refuses it.

**In this chapter you will learn:**

- why creation gets its own struct, and how `#[derive(Insert)]` names its model
- how `RETURNING` gives back the whole model, with the database's defaults, in one round trip
- `update`, `delete` and `delete_by_key`, and what zero rows affected means
- upserts with `ON CONFLICT`, and when `MERGE` is the better statement
- what constraint violations from shelf's writes look like, and where to map them

---

## 8.1 `#[derive(Insert)]`

Plan:
- `#[shelf(model = Product)] struct NewProduct { sku, name, price_cents, tags }`: the table
  and the `RETURNING` list come from the model.
- The SQL: `INSERT INTO "products" ("sku", …) VALUES ($1, …) RETURNING "id", …`, built once
  (`LazyLock`, Chapter 6) because the model's names are not literals here.
- `new_product.insert(&db) -> Result<Product, Error>` (proposed).

## 8.2 Database defaults

Plan:
- `uuidv7()` (PG 18; verify on the server), `now()`, identity columns: the model marks them
  `generated`, the `New…` struct leaves them out.
- What happens if a `New…` struct names a `generated` column: a compile error from the derive
  (proposed), or the database's.

## 8.3 Update and delete

Plan:
- `product.update(&db)`: every column that is neither key nor generated, `WHERE` the key.
  Zero rows affected means the row is gone: `Error::NotFound`.
- `product.delete(&db)`, `Product::delete_by_key(&db, &key)`.
- A **SQL lens** box on what `UPDATE` of every column costs (a new row version either way).

## 8.4 Upsert

Plan:
- `ON CONFLICT ("sku") DO UPDATE SET … = excluded.…`: the conflict target named by an
  attribute or an argument (open question).
- `DO NOTHING` returns no row on conflict, so the method returns an `Option`.
- `MERGE … RETURNING merge_action()` (PG 17) as the statement for a batch with several
  outcomes; main book Chapter 10's import does it by hand.

## 8.5 Constraint errors

Plan:
- A duplicate `sku`: `Error::Db` carrying 23505 and the constraint's name; the application's
  mapping (Chapter 7) turns it into 409.
- A `CHECK` violation: 23514, 422.

## The programs

- `run` (proposed): insert and read back, with the defaults filled.
- `run`: `uuidv7()` keys ordered by creation time.
- `run`: update, then delete, then update again (`NotFound`).
- `run`: an upsert twice, the second updating.
- `run`: a duplicate key's error, its constraint name printed.

## Side by side

Inserting a row and reading it back: by hand (stockroom's `stores::create`) · shelf
(`NewStore { … }.insert(&db)`, proposed) · SeaORM (`ActiveModel { … }.insert(&db)`, check).

## In stockroom

`create` in [stockroom/src/repo/stores.rs](../../stockroom/src/repo/stores.rs): the `New…`
struct and the `INSERT … RETURNING` this chapter generates.

## The data

A temporary `products` table with an identity key and a unique `sku`; a `uuid` key with
`DEFAULT uuidv7()` for 8.2.

## Claims

- Docs: PostgreSQL's `INSERT … ON CONFLICT`, `MERGE` (`RETURNING` since PG 17) and `uuidv7()`
  (PG 18); verify each on the 18.6 server before writing.
- Opinion: a separate struct for creation is clearer than `Option` fields on the model.

## When not to use shelf's writes

- A guarded update such as stockroom's oversell check (`UPDATE … WHERE available >= $3`):
  the condition is the point, so write it.
- Bulk loads: Chapter 12.

## Exercises

Plan:
1. Add `insert_returning_id` that returns only the key. *Hint:* a second `RETURNING` list.
2. Make `update` refuse a model whose key is a default value. *Hint:* there is no row to
   update.
3. Write the upsert with `MERGE` and compare. *Hint:* `merge_action()` tells you which branch
   ran.

> **Open questions.** (1) Naming the model (`model = Product`) and building the SQL with
> `LazyLock` (S1b probe). (2) How the upsert's conflict target is given: an attribute on the
> `New…` struct, or an argument. (3) Whether `update` writes every column or only changed ones
> (that is `Patch`, Chapter 9).

---

Previous: [Chapter 7 — The executor and shelf's errors](07-executor.md) · Next: [Chapter 9 — Partial updates and optimistic locking](09-patch-and-versions.md)
