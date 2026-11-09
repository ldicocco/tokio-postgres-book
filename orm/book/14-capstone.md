# Chapter 14 — Capstone: stockroom on shelf

> **Building an ORM on tokio-postgres** · Previous: [Chapter 13 — Trusting the mapping: schema checks and tests](13-trusting-the-mapping.md) · Next: [Appendix — attribute reference, generated code and phrasebook](15-appendix.md)

> **Sketch, 9 November 2026.** First sketched on 7 November, and revised to the capstone as
> built in phase S2. A plan of this chapter, not its text; it will be replaced by the chapter. Part
> III, fifth chapter. Builds on every chapter of this book, and on main book
> [Chapter 18](../../book/18-capstone.md) (stockroom end to end) and stockroom's repository
> layer ([stockroom/src/repo/](../../stockroom/src/repo/)). shelf: `orm/stockroom-shelf`
> (built, S2), a library and binary on the `stockroom` crate. Crates: axum 0.8.9,
> deadpool-postgres 0.14.2, async-stream 0.3.6. Plan: [orm/PLAN.md](../PLAN.md) §4.0d, §7,
> Chapter 14; §10 (the S2 rows); §12 #47–#56.

stockroom's data layer is written by hand: about a dozen repository functions, each with its
SQL, its parameters and its `TryFrom<&Row>`. It is the code this book set out to generate.
This chapter rewrites part of it with shelf, behind the same axum routes, against the same
schema, with the same domain types, and compares the two line for line.

The comparison is honest in both directions. shelf removes the mapping code and almost all of
the everyday SQL, and the ported routes answer exactly as stockroom's do: one test suite runs
against both routers and compares every reply. But an order read back costs a round trip more,
because shelf has no joins, and the orders module ends up longer than stockroom's. And some
code shelf should not touch at all: the guarded `UPDATE` that keeps orders from overselling,
the state machine of an order, the `MERGE` import, the COPY export. Knowing where an ORM stops
is the last thing this book teaches.

**In this chapter you will learn:**

- how a port can reuse an application's schema, types, errors and routes, and replace only its
  data layer
- how shelf's models fit a real schema: a domain, a composite, an enum, a `jsonb` document, a
  virtual column, a composite key
- what each repository function looks like rewritten with shelf, and what it costs in lines,
  statements and round trips, measured
- how to prove that a rewrite behaves as the original: one suite, two routers
- where shelf steps aside, and how raw SQL fills the gap

---

## 14.1 The shape

Plan:
- [orm/stockroom-shelf](../stockroom-shelf/): reused from stockroom unchanged are the
  migrations, the domain types, `AppError`, `in_transaction`, `CancelOnDrop`, the `Db`
  extractor and the router for every route not ported (§12 #47). New: the models and the
  repository (`src/repo/`), the handlers (stockroom's, copied: its handler modules are
  private), the error conversion, `serve` with `verify` first.
- The router: the ported routes, then stockroom's static paths that `/products/{sku}` would
  capture, routed to stockroom's router by name (`into_service`: `route_service` refuses a
  `Router`), then everything else by fallback (§12 #50, measured).
- The errors: `impl From<shelf::Error> for AppError` is refused by the orphan rule (E0117,
  captured; §12 #49), so the repository converts with `map_err(app_error)`. The same rule as
  Chapter 1's tuples, met from the other side.
- `stockroom-shelf verify` and `serve`: every model checked at startup, all mismatches at once.

## 14.2 Models for a real schema

Plan:
- `Store` (the composite `Address`, through postgres-types' derive), `Product` (the domain
  `Sku` with its hand-written `accepts`, `Cents`, `text[]`, `jsonb`, `version`, `updated_at`),
  each deriving serde beside shelf: the model is the JSON.
- `Stock`, keyed by `(store_id, product_id)`, with the virtual column `available`, read only
  (`generated`). **`verify` refused it as an `i32`** (measured, §12 #48): a generated column is
  nullable in the catalog unless declared `NOT NULL`, though `on_hand - reserved` of two
  `NOT NULL` columns never is. The model says `Option<i32>`; the schema is stockroom's.
- Orders: the API's order names its store by code and each line's product by SKU, which no row
  holds. So stockroom's request and response types are reused, the tables get models of their
  own (`OrderRow` with `belongs_to(Store)` and `has_many(LineRow)`, `LineRow` with
  `belongs_to(Product)`), and the response is assembled from them. `NewLineRow` sends the
  domain `Quantity` through `insert_many` (Chapter 12's cast from the prepared `INSERT`).

## 14.3 Line for line

Plan:
- Side by side: `stores::create` (13 lines, then 4), `products::update` (45, then 16, with the
  retry below), `orders::get` (46, then 10 plus the assembly).
- Measured, statements and round trips (§12 #54: a counter of Parse and Execute messages, then
  lagproxy at 5 ms each way; a round trip 14.4 ms):

  | Function | stockroom | shelf |
  |---|---|---|
  | stores, product reads and creates | 1 statement, 1 round trip | the same |
  | `products::update` | 1, 1 | 2, 2: the read by SKU, then the patch |
  | `orders::place`, 3 lines | 9 run, 2 re-prepared per call, 9 round trips | 7 run, 6 round trips |
  | `orders::get`, 3 lines | 2, 1 (two joins, pipelined) | 4, 2 (no joins) |

- Measured, lines of Rust (§12 #55): stores 57 against 34, products' ported items 150 against
  87, orders' `place` and `get` 148 against 175. Hand-written SQL: 15 statements against 1.
- A `PATCH` without `If-Match`: shelf's `Patch` always checks a version, stockroom's does not.
  The port patches at the version it read, and tries again when it is stale (§12 #51; 30
  concurrent patches, all applied).
- The NDJSON stream: shelf's `ModelStream` borrows its connection, and the body must own it, so
  the stream lives inside a generator that owns both (§12 #52).

## 14.4 Where shelf steps aside

Plan:
- The reservation: `UPDATE stock … WHERE available >= $3`, pipelined in product-id order, keeps
  its SQL inside a `place` whose reads and inserts are shelf's. stockroom's oversell and
  deadlock tests pass on the port.
- Unported, on purpose: an order's transitions (one `UPDATE` checks the state machine and the
  version, `RETURNING old.status`), stock adjustments (`on_hand = stock.on_hand +
  EXCLUDED.on_hand`, which an upsert that sets columns cannot say), serializable transfers, the
  `MERGE` import, the COPY export, `LISTEN`, the reports.
- The escape hatch for `orders::get`: stockroom's two joins as raw SQL, read with `FromRow`
  structs through `fetch_one` and `fetch_all`: one round trip again, at the price of the SQL.
- The pattern: shelf for the everyday reads and writes, SQL for the statements whose wording is
  a rule.

## 14.5 What a production ORM would add

Plan:
- Joins (the round trip `get` lost), migrations generated from models, lazy relations, change
  tracking, other databases: what each would cost shelf in simplicity, and which crate already
  does it (diesel, SeaORM, toasty).

## The programs

- `run` (proposed): the ported router on a pool of one, driven with `oneshot`: create a store
  and a product, patch it with `If-Match`, place an order, read it back. It needs the shelf
  playground to depend on stockroom-shelf.
- The tests the chapter points to: `tests/routes.rs` (11 scenarios against both routers, 63
  replies compared, and which handler answered, from the statement cache; §12 #53),
  `tests/concurrency.rs`, `tests/models.rs`.
- `examples/line_for_line.rs`, the measurement (§12 #54).

## Side by side

This chapter is one long side by side: stockroom's hand-written repository functions against
the shelf versions, with the same routes on top. The third column is the comparison of costs.

## The data

stockroom's schema, migrated by stockroom's own migrations; the tests clone stockroom's test
template, through stockroom's own harness.

## Claims

- Measured: the statements and round trips per ported function (§12 #54), the lines (§12
  #55), the routes' replies (§12 #53), `verify` on stockroom's schema (§12 #48).
- Opinion: an ORM should cover the boring 80% and get out of the way of the rest.

## When not to use shelf

- Every statement in 14.4, and any statement whose `WHERE` clause carries a business rule.
- A response that joins several tables, when its round trip matters.

## Exercises

Plan:
1. Port `GET /stores/{code}/stock`. *Hint:* a store by code, its `Stock` rows by filter, their
   products by loader; `available` is read only.
2. Give shelf's `Patch` an optional version (`version = coalesce($n, version)`), and remove the
   retry from `products::update`. *Hint:* `patch_sql`, and what `None` must mean.
3. Write `orders::get` with joins through `FromRow`, and measure it with `line_for_line`.
   *Hint:* two structs, two pipelined `fetch_*` calls.

> **Open questions.** (1) Whether the chapter's runnable examples drive stockroom-shelf from
> the shelf playground (a dependency of the playground on the capstone), or show it through
> excerpts and its tests only. (2) How much of `place` to print: it is 86 lines, most of them
> stockroom's validation.

---

Previous: [Chapter 13 — Trusting the mapping: schema checks and tests](13-trusting-the-mapping.md) · Next: [Appendix — attribute reference, generated code and phrasebook](15-appendix.md)
