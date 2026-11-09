# Chapter 11 — Relations without N+1

> **Building an ORM on tokio-postgres** · Previous: [Chapter 10 — A typed query builder](10-query-builder.md) · Next: [Chapter 12 — Bulk writes and streams](12-bulk-and-streams.md)

> **Sketch, 9 November 2026 (revised after S1c).** A plan of this chapter, not its text; it
> will be replaced by the chapter. Part III, second chapter. Builds on Chapters 6, 7 and 10,
> and on main book [Chapter 3](../../book/03-rows-and-types.md) (arrays as parameters),
> [Chapter 7](../../book/07-pipelining.md) (pipelining) and lagproxy
> ([apps/lagproxy](../../apps/lagproxy/)). shelf: the `has_many` and `belongs_to` keys in
> `attrs.rs`, the methods in `shelf-codegen/src/relations.rs`, the loaders in
> `shelf/src/relations.rs` (built, S1c); tests `tests/relations.rs`, `tests/ui/relations.rs`;
> the benchmark `examples/bench_relations.rs`. Crates: tokio-postgres 0.7.18, futures-util.
> Findings: §12 #41, #42; root PLAN.md §12 #21. Plan: [orm/PLAN.md](../PLAN.md) §4.0c, §7,
> Chapter 11; §10 (relations).

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
  `#[shelf(belongs_to(Order, key = order_id))]` on `OrderLine`. `has_many` needs `as`: no
  pluralizing; `belongs_to` takes the model's name in snake case (`order_line`) unless `as`
  says otherwise. Parsing `as`, a keyword, works because syn's nested-meta paths accept
  keywords (`Ident::parse_any`). A **Macro lens** on `key = …`: it names a field of the *other*
  struct for `has_many`, which the derive cannot see, so the generated code names
  `OrderLine::ORDER_ID` and `line.order_id` and lets the compiler check them, spanned on the
  attribute's tokens (the captured errors of `tests/ui/relations.stderr`, §12 #41).
- Generated: `order.lines(&db) -> Vec<OrderLine>`, `Order::load_lines(&db, &orders) ->
  Vec<Vec<OrderLine>>` (aligned with `orders`), `line.order(&db) -> Order` (an `Option` when
  the field is written `Option<…>`), `OrderLine::load_order(&db, &lines) -> HashMap<i64,
  Order>`; the expansion of `relations.rs` printed.

## 11.2 N+1, measured

Draft prose:

Through lagproxy at 5 ms each way, one round trip cost 13.3–13.8 ms on this machine (§12 #42).
Loading the lines of a hundred orders one order at a time took 2.7 seconds on a plain `Client`,
which prepares every statement and so pays two round trips per order, and 1.35 seconds on a
pooled connection, whose statement cache brings it down to one. The loop is not slow because
PostgreSQL is slow; on localhost the same loop took 5 to 12 milliseconds. It is slow because it
waits for the network a hundred times.

Plan:
- The loop timed through lagproxy, and counted with an executor of our own that wraps a client
  and counts its statements (`Counting` in `tests/relations.rs`: `Executor` is not sealed).

## 11.3 Batched loading

Plan:
- One query through the builder, `WHERE "order_id" = ANY($1)` with the orders' keys as one
  array, then a `HashMap` from key to children; the loader returns them in the parents' order.
  A nullable foreign key (`ForeignKey`, implemented for `K` and `Option<K>`) skips NULLs.
- Measured through lagproxy (§12 #42): 26.7–28.5 ms on a `Client` (two round trips, prepare
  and query) and 14.0–14.6 ms pooled (one), against 2.7 s and 1.35 s for the loop.
- Keys need `Hash + Eq + Clone`; relations follow single-column keys (`has_many` on a model
  with a composite key is refused at the attribute).

## 11.4 Pipelining independent loads

Plan:
- Lines and the store for a page of orders: two loaders, independent, joined with `try_join!`
  on one connection (a test does), costing about one round trip (main book Chapter 7; root §12
  #21: one pipelined client beat a pool over latency). Every executor allows it: the loaders
  take `&db`, and a `Transaction` is shared by reference too.
- Pipelining the N+1 loop itself (`try_join_all` of `order.lines(&db)`) took 32–35 ms through
  lagproxy, nearly the batched time, but on localhost 3.3–3.5 ms against 0.2 ms batched: it
  hides the latency of a hundred queries, not their work (§12 #42).

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
  loop for fifty orders (relative; for a hundred orders, 95–100× in release, §12 #42; to
  check in a debug build).
- `run`: the counting executor: the loop runs 101 statements, the loader 2.
- `needs=lagproxy`: two loaders pipelined with `try_join!`.
- `run`: a join with `flatten`, and `json_agg` into `Json<Vec<…>>`.

## Side by side

Orders with their lines: by hand (stockroom's `orders::get`, two pipelined queries) · shelf
(`Order::load_lines`) · SeaORM (`find_with_related`, check).

## The data

Temporary `orders` and `order_lines` tables with fifty orders and a few lines each;
stockroom's schema in the capstone.

## Claims

- Measured: §12 #42 (N+1, pipelined and batched, through lagproxy and on localhost); root
  PLAN.md §12 #21 (pipelining against a pool).
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

> **Open questions.** Settled in S1c (PLAN.md §10): the names and signatures; a `Vec` per
> parent for `has_many`, a `HashMap` for `belongs_to`; single-column keys only; pipelining
> works through any executor. Still open: whether composite foreign keys become an exercise.

---

Previous: [Chapter 10 — A typed query builder](10-query-builder.md) · Next: [Chapter 12 — Bulk writes and streams](12-bulk-and-streams.md)
