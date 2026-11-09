# Chapter 9 — Partial updates and optimistic locking

> **Building an ORM on tokio-postgres** · Previous: [Chapter 8 — Writing rows: insert, update, delete, upsert](08-writing.md) · Next: [Chapter 10 — A typed query builder](10-query-builder.md)

> **Sketch, 9 November 2026.** A plan of this chapter, not its text; it will be replaced by
> the chapter. Part II, fourth chapter. Builds on Chapter 8, and on main book
> [Chapter 4](../../book/04-prepared-statements.md) (statements per shape),
> [Chapter 8](../../book/08-transactions.md) (§8.8, optimistic concurrency) and
> [Chapter 15](../../book/15-axum-api.md) (ETag and `If-Match`). shelf: `#[derive(Patch)]` in
> `shelf-codegen/src/patch.rs`, `Patch` and `patch_sql` in `shelf/src/patch.rs`, the `version`
> and `updated_at` attributes, `Error::Stale` (built, S1b). Findings: §12 #30, #31, #33. Crates: tokio-postgres 0.7.18, serde. Findings: root PLAN.md §12 #1
> (domain-typed parameters). Plan: [orm/PLAN.md](../PLAN.md) §7, Chapter 9; §13, S1b steps 1
> and 5.

An admin edits a product's price and nothing else. The request body is `{"price_cents": 990}`,
and the update should touch that column only; another admin's change to the name, made a
second earlier, must survive. Chapter 8's `update` writes every column, so it would undo that
change. A partial update needs to know which fields were sent, and JSON has a third state that
`Option` cannot express: a field that was sent as `null`, which means "set it to NULL", is not
a field that was left out.

Then there is the edit that arrives late. Two tabs load version 7 of the product; both save.
Without a check, the second save silently overwrites the first. With a version column, the
second `UPDATE` finds no row with version 7, and the application can say so. This chapter
builds both: `#[derive(Patch)]` in one of three designs, chosen by measurement, and a
`version` attribute that turns a lost update into an error.

**In this chapter you will learn:**

- how to tell "absent" from "null" in a request body, with `Option<Option<T>>` and serde
- three ways to write a partial `UPDATE`, and what each costs in statements and round trips
- how `#[derive(Patch)]` generates the one shelf chose
- how a version column turns a lost update into `Error::Stale`, and how to tell it from
  `NotFound`
- what `updated_at = now()` records (the transaction's start, not the statement's)
- how ETags and `If-Match` fit on top, in axum

---

## 9.1 Absent or null

Plan:
- `Option<T>` for a non-nullable column (absent or a value), `Option<Option<T>>` for a
  nullable one (absent, null, or a value).
- serde: deserializing the double `Option` needs a helper (a `deserialize_with` or a small
  crate; check what is current).

## 9.2 Three designs

Draft prose:

There are three ways to send a partial update, and each gives something up. The first builds
the `SET` list at run time from the fields that are present: the SQL is exactly what you
would write by hand, but every combination of fields is a different statement, prepared and
cached separately. The second sends every column, and writes `col = coalesce($n, col)`: one
statement for every patch, but `coalesce` cannot tell a NULL you want from a value you did not
send, so it can never set a column to NULL. The third sends a flag beside each value, `col =
CASE WHEN $f THEN $v ELSE col END`: one statement, NULLs included, at the price of twice the
parameters.

Plan:
- Each design run against the same table, through lagproxy (measured, PLAN.md §12 #33: (a)
  caches a statement per shape, 7 for 3 fields; (b) cannot write NULL; (c) one statement, NULLs
  included). shelf chose (c) (§10).
- Design (c)'s catch was checked: an unset field sends a typed `None`, accepted for every
  column type tried, and a domain's parameter is typed as its base type (§12 #30).
- The statement is built once, at first use, in a `LazyLock<String>`: its `$n` numbers depend
  on the model's key. A **Rust refresher** on `LazyLock` (stable since Rust 1.80; check), and
  why a `const` could not do it here (Chapter 6).

## 9.3 Version columns

Plan:
- `#[shelf(version)]` on an `i32` field: `WHERE "id" = $1 AND "version" = $2`, and
  `"version" = "version" + 1` in the `SET` list.
- Zero rows: either the row is gone (`NotFound`) or the version moved on (`Stale`). Told apart
  in one round trip by a CTE that also returns the row's current version (§12 #31), which
  `Stale` carries, for a 412.
- The main book's §8.8 did this by hand; stockroom's `PATCH /products/{sku}` does it with
  `coalesce($2, version)`.

## 9.4 `updated_at`

Plan:
- `#[shelf(updated_at)]` writes `"updated_at" = now()`.
- A **SQL lens** box: `now()` is the transaction's start time; two updates in one transaction
  record the same instant (`clock_timestamp()` does not).

## 9.5 ETags in axum

Plan:
- The version as the ETag; `If-Match` into the patch's expected version; `Stale` → 412 with
  the current version, as stockroom does.
- A handler driven with `oneshot`.

## The programs

- `run` (proposed): absent, null and a value deserialized into a patch struct.
- `run`: the three designs against one table, with the prepared statements counted.
- `run`: two tabs on one row, the second save getting `Error::Stale`.
- `run`: `updated_at` twice in one transaction, the same instant.
- `run`: an axum handler with `If-Match`, through `oneshot` (412 on a stale version).

## Side by side

An edit from two admin tabs: by hand (stockroom's `PATCH /products/{sku}`) · shelf
(`ProductPatch { …, version }.apply(&db, &id)`, built) · SeaORM (`ActiveModel` with `Set`
and `NotSet`, check).

## In stockroom

The `update` in [stockroom/src/repo/products.rs](../../stockroom/src/repo/products.rs):
`coalesce` for the fields and for the version.

## Claims

- Measured (to measure): statements prepared per design, round trips per patch.
- Docs: PostgreSQL's date/time functions (`now()` against `clock_timestamp()`).
- Opinion: design (c) is the right default for a model layer; (a) when the SQL must look
  hand-written.

## When not to use `Patch`

- A change that is a business operation (an order's status transition, stockroom's
  `POST /orders/{id}/transition`): write a function with its own rules.
- Counters (`stock = stock - 1`): an expression, not a value from the client.

## Exercises

Plan:
1. Make a patch with no fields a no-op that sends nothing. *Hint:* check before the round
   trip.
2. Return the changed columns' names along with the model. *Hint:* the flags already say
   which.
3. Add `#[shelf(version)]` on a `bigint`. *Hint:* the bound on the field's type.

> **Open questions.** Settled in S1b: design (c), by measurement (§12 #33, §10); `Stale` and
> `NotFound` in one round trip (§12 #31); the patch carries the version it was made against, in
> a `#[shelf(version)]` field. Still open: the serde helper for `Option<Option<T>>` (check
> which crate or function is current), and `json` fields on a patch.

---

Previous: [Chapter 8 — Writing rows: insert, update, delete, upsert](08-writing.md) · Next: [Chapter 10 — A typed query builder](10-query-builder.md)
