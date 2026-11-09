# Chapter 12 — Bulk writes and streams

> **Building an ORM on tokio-postgres** · Previous: [Chapter 11 — Relations without N+1](11-relations.md) · Next: [Chapter 13 — Trusting the mapping: schema checks and tests](13-trusting-the-mapping.md)

> **Sketch, 9 November 2026 (revised after S1c).** A plan of this chapter, not its text; it
> will be replaced by the chapter. Part III, third chapter. Builds on Chapters 5, 7 and 8, and
> on main book [Chapter 5](../../book/05-custom-types.md) (domains and arrays),
> [Chapter 9](../../book/09-streaming.md) (`RowStream`, backpressure, `pin!`) and
> [Chapter 10](../../book/10-copy.md) (COPY, binary COPY, pgload). shelf: `Insert::insert_many`
> and `Insert::copy_in` in `shelf/src/bulk.rs`, with `COLUMNS`, `COPY` and `arrays` from the
> derive; `fetch_stream`, `Query::stream` and `ModelStream` in `shelf/src/stream.rs`;
> `Executor::query_raw` and `copy_in` (built, S1c); tests `tests/bulk.rs`, `tests/stream.rs`,
> `tests/ui/stream.rs`; the benchmark `examples/bench_bulk.rs`. Crates: tokio-postgres 0.7.18,
> futures-util. Findings: §12 #37–#39, #44–#46; root PLAN.md §12 #1, #4, #13, #22, #34–#37.
> Plan: [orm/PLAN.md](../PLAN.md) §4.0c, §7, Chapter 12; §10 (bulk writes, streams).

Inserting ten thousand rows one `insert` at a time is the N+1 problem again, in the other
direction: ten thousand round trips. The main book measured it with pgload: one `INSERT` per
row was about a hundred times slower than binary COPY on localhost, and pipelining or `unnest`
batches recovered most of the difference (root PLAN.md §12 #22). A model layer that offers only
`insert` pushes every bulk load into that slow path.

shelf adds two bulk writes, both generated from the `New…` struct the reader already has:
`insert_many`, one `INSERT … SELECT * FROM unnest(…)` with an array per column, and a binary
COPY writer for the largest loads. Then it turns to the other large direction, reading, where
a `Vec` of a million models is a million allocations held at once, and gives queries a stream
of models that resolves its columns once and respects backpressure.

**In this chapter you will learn:**

- how `insert_many` sends a batch as one array per column through `unnest`
- why arrays of domain-typed columns need casts, and how shelf adds them
- how the derive writes a binary COPY row, and which types it must declare
- what each bulk strategy costs, measured against pgload's table
- how to stream models from a `RowStream`, resolving columns once
- how a COPY rejected at its start can kill a connection in 0.7.18, and how shelf avoids it

---

## 12.1 `insert_many`

Plan:
- One statement, `INSERT INTO "t" ("a", "b") SELECT * FROM unnest($1::"pg_catalog"."text"[],
  …) RETURNING …`, with one `Vec<&T>` per column built by the derive's `arrays` from the slice
  of `New…` values (`__private::array`).
- In the select list, `unnest` keeps a composite whole; in `FROM` it expands it into fields
  (root PLAN.md §12 #4), which matters for composite-typed columns.
- The casts: `unnest` of an untyped parameter cannot be resolved, so each array needs its
  type. shelf learns it from the struct's own `INSERT`, prepared: `Statement::params()` holds
  each column's type as PostgreSQL types the parameter, a domain as the domain, an enum by
  name, so a batch accepts exactly what `insert` accepts (§12 #37). A prepared `SELECT …
  LIMIT 0`, the first plan, would describe a domain column as its base type, which a domain
  newtype refuses.
- An array column cannot go through `unnest`: tokio-postgres cannot send `Vec<Vec<String>>`,
  and `unnest` flattens a 2-D array (§12 #37). `Error::Unsupported`, before anything is sent,
  pointing to `copy_in`.
- `RETURNING` gives the rows back in the order PostgreSQL inserted them, which in practice is
  the slice's; SQL does not promise it (a **SQL lens**).

## 12.2 Binary COPY from the derive

Plan:
- `NewProduct::copy_in(&db, &rows)`: the derive's `COPY "t" ("a", "b") FROM STDIN (FORMAT
  binary)`, joined at compile time, and a `BinaryCopyInWriter` given the prepared `INSERT`'s
  types, fed each row's `params()`: the same values `insert` sends.
- The main book's warning: binary COPY trusts the declared types, and a wrong type of the same
  size loads wrong data silently (root §12 #37); shelf's types are the columns' own, from the
  server.
- Pinned writer: `pin!` inside the function, which drops it on every return; `drop` on a pinned
  reference drops nothing (root §12 #34). A row refused on the client leaves the connection
  usable once the writer is dropped (§12 #38).

## 12.3 Measured

Plan:
- `bench_bulk`, pgload's rows and table, five ways (§12 #44): 20,000 rows in 0.77 s by a loop
  of `insert`, 0.15 s pipelined, 0.016–0.018 s by `insert_many`, 0.006 s by `copy_in`, beside
  pgload's 0.756, 0.120, 0.014 and 0.008 s; and 2,000 rows through lagproxy at 1 ms, where the
  loop takes 9.4–9.8 s and every batched form 9–14 ms.
- Why the two tables differ at 1 ms: pgload's timer also covers five round trips of setup and
  commit (§12 #45). A benchmark says what it times.
- A **Pitfall**: outside a transaction, every insert commits and waits for its WAL flush:
  20,000 pipelined inserts took 0.91–0.98 s that way and 0.13–0.14 s in one transaction
  (§12 #46).
- A keyed table: WAL and an index add to every strategy, and make the batched ones noisy.

## 12.4 Streams of models

Plan:
- `Product::query().….stream(&db)` and `fetch_stream::<T>(&db, sql, params)` over
  `Executor::query_raw`: the columns resolved once from the prepared statement, so a wrong
  mapping fails before the first row, and each row decoded as it arrives. `ModelStream` boxes
  the `RowStream`, so it is `Unpin` and needs no `pin!` (a **Rust refresher** on why).
- Backpressure: each request's response channel holds one message, so an unread stream stalls
  every later request on the connection; a test shows a `SELECT 1` waiting until the stream is
  dropped (`tests/stream.rs`; main book Chapter 9).
- Who owns the connection while the stream lives (root PLAN.md §12 #13): `ModelStream<'a, T>`
  borrows the executor, so returning a pooled connection mid-stream is a compile error (E0505,
  `tests/ui/stream.stderr`). An HTTP body that must own its connection is Chapter 14's.

## 12.5 A COPY rejected at its start

Plan:
- In 0.7.18 a `COPY … FROM STDIN` that the server rejects before it starts (a missing table or
  column) leaves the connection unusable: rust-postgres issue #789 (root PLAN.md §12 #36).
- Preparing the `COPY` checks nothing, not even the table (§12 #38). shelf's guard is the
  prepared `INSERT` of 12.1, which fails cleanly for a missing table or column; a test drops a
  column and shows the error and the connection still working (`tests/bulk.rs`).

## The programs

- `run` (proposed): `insert_many` of a thousand rows, counted back.
- `run`: binary COPY of the same rows, with the types learnt from the `INSERT`.
- `run`: the strategies timed on ten thousand rows in a transaction; asserts COPY is faster
  than the loop by at least ten times (relative; about 125× in release, §12 #44; to check in a
  debug build).
- `run`: an array column refused by `insert_many`, then loaded by `copy_in`.
- `run`: a stream of a hundred thousand models, counted without holding them.
- `run`: a missing column caught by the guard, and the connection still usable.

## Side by side

Loading ten thousand rows: by hand (pgload's `unnest` batches) · shelf (`insert_many`) ·
sqlx (`QueryBuilder::push_values`, or `copy_in_raw`; check).

## In stockroom

`stockroom seed` writes products and the ledger with binary COPY by hand
([stockroom/src/cli/seed.rs](../../stockroom/src/cli/seed.rs)).

## Claims

- Measured: root PLAN.md §12 #22 (pgload); shelf's strategies, §12 #44–#46.
- Docs: rust-postgres issue #789; PostgreSQL's `COPY`.
- Opinion: offer `insert_many` by default, COPY when the batch is large and trusted.

## When not to use shelf's bulk writes

- A client's CSV: COPY text through a staging table (main book Chapter 16), with errors
  reported by line.
- A load that needs `ON CONFLICT` per row: `insert_many` with a conflict clause, or `MERGE`.

## Exercises

Plan:
1. Split `insert_many` into batches of a given size. *Hint:* chunks of the slice.
2. Make the stream report its row count at the end. *Hint:* `rows_affected()` once
   exhausted.
3. Load through COPY into a temporary table, then upsert. *Hint:* main book §10.6.

> **Open questions.** Settled in S1c (PLAN.md §10): the types come from the prepared
> `INSERT`; the stream borrows its connection. Still open: the debug-build ratio for a relative
> timing assertion.

---

Previous: [Chapter 11 — Relations without N+1](11-relations.md) · Next: [Chapter 13 — Trusting the mapping: schema checks and tests](13-trusting-the-mapping.md)
