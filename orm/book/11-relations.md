# Chapter 11 — Relations without N+1

> **Building an ORM on tokio-postgres** · Previous: [Chapter 10 — A typed query builder](10-query-builder.md) · Next: [Chapter 12 — Bulk writes and streams](12-bulk-and-streams.md)

> **Sketch, 7 November 2026.** A plan of this chapter, not its text; it will be replaced by
> the chapter. Part III, second chapter. Builds on Chapters 6, 7 and 10, and on main book
> [Chapter 3](../../book/03-rows-and-types.md) (arrays as parameters),
> [Chapter 7](../../book/07-pipelining.md) (pipelining) and lagproxy
> ([apps/lagproxy](../../apps/lagproxy/)). shelf: the `belongs_to` and `has_many` attributes,
> their generated methods, and `shelf/src/relations.rs` (proposed, S1c). Crates:
> tokio-postgres 0.7.18. Findings: root PLAN.md §12 #21, #33. Plan: [orm/PLAN.md](../PLAN.md)
> §4.3, §7, Chapter 11.

An order has lines. Showing a page of fifty orders with their lines is the textbook case where
an ORM is either a great help or a disaster. The disaster is the obvious loop: fetch the
orders, then, for each one, fetch its lines. That is fifty-one queries, and over a network
each one costs a round trip, so the page that took a millisecond on localhost takes a second
in production. It is called the N+1 problem, and the cure is older than ORMs: fetch all the
lines for all fifty orders in one query, and sort them into their orders in Rust.

shelf makes the cure the easy path. A `has_many` attribute on `Order` generates a loader that
takes a slice of orders and returns their lines grouped by order, in one query with
`order_id = ANY($1)`. The chapter measures the loop and the loader through lagproxy, then
looks at the two other answers, a join and pipelining, and explains why shelf has no lazy
loading at all.

**In this chapter you will learn:**

- how `belongs_to` and `has_many` declare a relation, and the methods they generate
- what the N+1 problem costs, measured through lagproxy
- how batched loading with `= ANY($1)` fetches every child in one round trip
- how to pipeline independent loads on one connection
- when a join is the better query, and what it costs in duplicated rows
- why shelf has no lazy loading

---

## 11.1 Declaring relations

Plan:
- `#[shelf(has_many(OrderLine, key = order_id, as = lines))]` on `Order`;
  `#[shelf(belongs_to(Order, key = order_id))]` on `OrderLine` (proposed syntax; PLAN.md
  §4.3). Names are explicit: no pluralizing.
- Generated (proposed): `order.lines(&db)`, `Order::load_lines(&db, &orders)`,
  `line.order(&db)`.

## 11.2 N+1, measured

Draft prose:

Through lagproxy at 5 ms each way, one round trip cost 15.3 ms on this machine (root PLAN.md §12
#33). Fifty orders and their lines fetched one order at a time is fifty-one of them, three
quarters of a second before the server has done any real work, and the time grows with the
page size. The loop is not slow because PostgreSQL is slow; it is slow because it waits for
the network fifty-one times.

Plan:
- The loop and its time through lagproxy (to measure); the server's own time from
  `EXPLAIN ANALYZE` beside it, to show where the time goes.

## 11.3 Batched loading

Plan:
- One query, `WHERE order_id = ANY($1)` with the orders' keys as an array, then a `HashMap`
  from key to children; the loader returns the children in the parents' order.
- Measured through lagproxy: two round trips for the page, whatever its size (to measure).
- The foreign key type needs `Hash + Eq`; a composite key needs a tuple (open question).

## 11.4 Pipelining independent loads

Plan:
- Lines and the store for a page of orders: two loaders, independent, joined with `try_join!`
  on one connection, costing about one round trip (main book Chapter 7; §12 #21: one pipelined
  client beat a pool over latency).
- Which executors allow it: a shared `Client`, not a `Transaction` held mutably (check with
  the executor of Chapter 7).

## 11.5 Joins

Plan:
- A join returns each order once per line: duplicated parent columns, and a `FromRow` with
  `flatten` to read them.
- `json_agg` or `array_agg` of a composite to return one row per order; when that wins (a
  small number of children, one round trip).

## 11.6 Why no lazy loading

Plan:
- Lazy loading needs a hidden connection in the model and a hidden round trip in a field
  access; in async Rust it would also need `.await` on a getter. shelf makes every round trip
  visible instead (opinion, argued).

## The programs

- `needs=lagproxy` (proposed): the N+1 loop timed.
- `needs=lagproxy`: the batched loader timed; asserts it is at least ten times faster than the
  loop for fifty orders (relative; to measure).
- `needs=lagproxy`: two loaders pipelined with `try_join!`.
- `run`: a join with `flatten`, and `json_agg` into `Json<Vec<…>>`.

## Side by side

Orders with their lines: by hand (stockroom's `orders::get`, two pipelined queries) · shelf
(`Order::load_lines`, proposed) · SeaORM (`find_with_related`, check).

## The data

Temporary `orders` and `order_lines` tables with fifty orders and a few lines each;
stockroom's schema in the capstone.

## Claims

- Measured: root PLAN.md §12 #33 (a round trip through lagproxy), #21 (pipelining against a
  pool); N+1 against batched (to measure).
- Opinion: no lazy loading; every round trip should be visible at the call site.

## When not to use relation loaders

- A report across several tables: one SQL query.
- A deep tree (categories of categories): a recursive CTE.

## Exercises

Plan:
1. Load lines and products for a page of orders, two levels. *Hint:* the second loader takes
   the first one's children.
2. Return lines ordered by a column, inside each group. *Hint:* order in SQL, group in Rust.
3. Write the `json_agg` version as a `FromRow` struct. *Hint:* `#[shelf(json)]`.

> **Open questions.** (1) The names and signatures of the generated methods, and whether the
> loader returns a `HashMap` or a `Vec` of pairs. (2) Composite foreign keys. (3) Whether
> loaders can be pipelined through the `Executor` trait as designed in Chapter 7 (check).

---

Previous: [Chapter 10 — A typed query builder](10-query-builder.md) · Next: [Chapter 12 — Bulk writes and streams](12-bulk-and-streams.md)
