# Chapter 14 — Capstone: stockroom on shelf

> **Building an ORM on tokio-postgres** · Previous: [Chapter 13 — Trusting the mapping: schema checks and tests](13-trusting-the-mapping.md) · Next: [Appendix — attribute reference, generated code and phrasebook](15-appendix.md)

> **Sketch, 7 November 2026.** A plan of this chapter, not its text; it will be replaced by
> the chapter. Part III, fifth chapter. Builds on every chapter of this book, and on main book
> [Chapter 18](../../book/18-capstone.md) (stockroom end to end) and stockroom's repository
> layer ([stockroom/src/repo/](../../stockroom/src/repo/)). shelf: `orm/stockroom-shelf`, a
> library and binary reusing the `stockroom` crate's migrations and domain types (proposed,
> S2). Crates: axum 0.8.9, deadpool-postgres 0.14.2. Plan: [orm/PLAN.md](../PLAN.md) §4.1,
> §7, Chapter 14; §9, phase S2.

stockroom's data layer is written by hand: about a dozen repository functions, each with its
SQL, its parameters and its `TryFrom<&Row>`. It is the code this book set out to generate.
This chapter rewrites part of it with shelf, behind the same axum routes, against the same
schema, with the same domain types, and compares the two line for line.

The comparison is honest in both directions. shelf removes the mapping code and most of the
everyday SQL, and the routes keep their behaviour; the chapter counts lines, statements and
round trips to show it. It also leaves alone the code shelf should not touch: the guarded
`UPDATE` that keeps orders from overselling, the `MERGE` import, the COPY export, the `LISTEN`
connection. Knowing where an ORM stops is the last thing this book teaches.

**In this chapter you will learn:**

- how shelf's models fit a real schema: a domain, a composite, an enum, a `jsonb` document, a
  virtual column
- what a repository function looks like rewritten with shelf, beside the original
- how much code, how many statements and how many round trips the rewrite changes, measured
- where shelf steps aside, and how raw SQL with `FromRow` fills the gap
- what a production ORM would add next

---

## 14.1 The shape

Plan:
- `orm/stockroom-shelf`: the `stockroom` crate as a dependency for its migrations, its domain
  types (`Sku`, `Address`, `OrderStatus`, `Cents`) and its error mapping; a repository layer on
  shelf; the routes for stores, products and orders, built with the same router shape.
- Its tests: stockroom's expectations for the ported routes, through `oneshot`, against a
  database cloned from stockroom's template (main book Chapter 17).

## 14.2 Models for a real schema

Plan:
- `Store` with an `address` composite (postgres-types' derive, passed through); `Product` with
  a `sku` domain (stockroom's hand-written `accepts`), `tags text[]`, `attributes jsonb` and a
  `version`; `StockLevel` with the virtual column `available`, read-only (never written by
  `Insert` or `Patch`: `generated`).
- `Order` and `OrderLine` with `has_many`, and `OrderStatus` as a Postgres enum.

## 14.3 Line for line

Plan:
- Three functions side by side: `stores::create`, `products::update` (versioned patch), and
  `orders::get` (an order with its lines).
- Counted (to measure): lines of Rust, statements prepared, round trips per request.

## 14.4 Where shelf steps aside

Plan:
- The oversell-proof reservation: `UPDATE stock … WHERE available >= $3`, pipelined in
  product-id order (main book Chapter 8): kept as SQL, its result read with a `FromRow` struct.
- The CSV import's `MERGE` and the CSV export's `COPY`, the `LISTEN` connection, the outbox
  worker's `SKIP LOCKED`: unchanged.
- The pattern: shelf for the everyday reads and writes, hand-written SQL for the statements
  whose wording is the point.

## 14.5 What a production ORM would add

Plan:
- Migrations generated from models, a query builder with joins, lazy relations,
  multi-database support, change tracking: what each would cost shelf in simplicity, and which
  existing crate already does it (diesel, SeaORM, toasty).

## The programs

- `run` (proposed): the ported router on a pool of one, driven with `oneshot`: create a
  store, create a product, patch it with `If-Match`, place an order, read it back.
- `orm/stockroom-shelf/tests/`: stockroom's route tests for the ported routes.

## Side by side

This chapter is one long side by side: stockroom's hand-written repository functions against
the shelf versions, with the same routes on top.

## The data

stockroom's schema, migrated by stockroom's own migrations, seeded with `stockroom seed`.

## Claims

- Measured (to measure): lines, statements and round trips per ported route.
- Opinion: an ORM should cover the boring 80% and get out of the way of the rest.

## When not to use shelf

- Every statement in 14.4, and any statement whose `WHERE` clause carries a business rule.

## Exercises

Plan:
1. Port `GET /stores/{code}/stock`. *Hint:* the virtual column is read-only.
2. Add a route that lists a store's open orders with their lines. *Hint:* a filter, then a
   batched loader.
3. Count the round trips of `POST /orders` in both versions. *Hint:* `pg_stat_statements`, or
   lagproxy's timings.

> **Open questions.** (1) Which routes to port: stores, products and orders' reads are the
> proposal. (2) Whether stockroom-shelf reuses stockroom's test harness as a dependency or
> copies its pattern. (3) Whether `AppError` comes from stockroom unchanged, with
> `From<shelf::Error>`.

---

Previous: [Chapter 13 — Trusting the mapping: schema checks and tests](13-trusting-the-mapping.md) · Next: [Appendix — attribute reference, generated code and phrasebook](15-appendix.md)
