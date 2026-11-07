# Chapter 7 — The executor and shelf's errors

> **Building an ORM on tokio-postgres** · Previous: [Chapter 6 — Tables: the model derive](06-tables.md) · Next: [Chapter 8 — Writing rows: insert, update, delete, upsert](08-writing.md)

> **Sketch, 7 November 2026.** A plan of this chapter, not its text; it will be replaced by
> the chapter. Part II, second chapter. Builds on Chapter 6, and on main book
> [Chapter 4](../../book/04-prepared-statements.md) (round trips),
> [Chapter 6](../../book/06-errors.md) (`DbError`, `SqlState`),
> [Chapter 8](../../book/08-transactions.md) (the sealed `GenericClient`, the retry helper) and
> [Chapter 13](../../book/13-pooling.md) (deadpool's `prepare_cached`). shelf: the `Executor`
> trait in `shelf/src/executor.rs`, the `deadpool` feature, `Error::NotFound` and `Error::Stale`
> (proposed, S1b step 2); `fetch_*` move onto it. Crates: tokio-postgres 0.7.18,
> deadpool-postgres 0.14.2. Findings: root PLAN.md §12 #6, #33, #42. Plan:
> [orm/PLAN.md](../PLAN.md) §4.5, §7, Chapter 7; §13, S1b steps 1–2.

So far shelf's functions take tokio-postgres's `GenericClient`, which covers a `Client` and a
`Transaction`. A service holds neither: it holds a deadpool connection, whose own
`GenericClient` adds the one thing a model layer wants most, a statement cache. Both traits are
sealed, so shelf cannot implement either for anything, and code generic over one does not
accept the other.

The way out is a trait of shelf's own, with only the calls shelf needs, implemented for the
four types a program actually has. Deciding what goes in it means deciding how each statement
reaches the server, which is a question of round trips the main book taught you to measure.
The chapter measures three strategies through lagproxy, picks one per executor, and gives
shelf an error type that says what an application can do about each failure.

**In this chapter you will learn:**

- why neither `GenericClient` will do, and what a trait of shelf's own must contain
- how to write async methods in a trait whose futures are `Send`, and why that matters in axum
- three ways to send a statement (`prepare`, `prepare_cached`, `query_typed`) and their round
  trips, measured
- how shelf calls run inside a transaction, and with the main book's retry helper
- `shelf::Error`, and how an application maps it to HTTP statuses

---

## 7.1 Why not `GenericClient`

Plan:
- tokio-postgres's `GenericClient` is sealed; deadpool's is sealed too, and only it has
  `prepare_cached` (root CLAUDE.md, "Verified API facts").
- An attempt to implement one for a wrapper, and the captured error.

## 7.2 The `Executor` trait

Plan:
- The calls shelf needs: prepare (cached where possible), `query`, `query_one`, `query_opt`,
  `execute` (proposed list).
- Methods returning `impl Future<Output = …> + Send`, and a **Rust refresher** on why the
  `Send` must be written: a handler's future must be `Send`, and a trait method's future is
  not known to be unless the trait says so.
- Impls for `tokio_postgres::Client`, `tokio_postgres::Transaction<'_>` and, behind the
  default feature `deadpool`, `deadpool_postgres::Client` and its `Transaction<'_>`.
- `fetch_*` moved onto it; the tests unchanged.

## 7.3 Three ways to send a statement

Plan:
- `prepare` then execute: two round trips every time (Parse/Describe, then Bind/Execute).
- `prepare_cached` (deadpool): two the first time, one after; the cache is per connection
  (root PLAN.md §12 #42: about 1 ms the first time, about 3 µs after).
- `query_typed`: one round trip and no named statement, but the parameter types must be known,
  and `resolve` has no `Statement` to check against before the query.
- Measured through lagproxy at 5 ms each way (root PLAN.md §12 #33: a round trip cost 15.3 ms);
  the choice per executor recorded in PLAN.md §10.

## 7.4 Transactions

Plan:
- shelf calls on a `&Transaction` see the transaction's own writes.
- The main book's `in_transaction` retry helper with an `AsyncFnMut` closure that calls shelf
  (root PLAN.md §12 #6: the closure must own its data).

## 7.5 `shelf::Error`

Plan:
- The variants: `Mapping` and `Db` (built), `NotFound` and `Stale` (proposed), each with what
  the caller can do about it.
- Constraint names stay the application's: stockroom's `AppError` maps `SqlState` and
  constraint names to statuses; an `impl From<shelf::Error> for AppError` shows the pattern.

## The programs

- A ` ~~~rust ` fragment that tries to implement `GenericClient`, with its captured error.
- `run` (proposed): one repository function called with a client, a transaction and a pooled
  connection.
- `needs=lagproxy`: the three strategies timed; asserts that a cached statement beats
  prepare-every-time by at least a round trip (relative).
- `run`: the retry helper running shelf calls under `SERIALIZABLE`.
- `run`: a unique violation from shelf mapped to an application error.

## Side by side

One repository function over a client, a transaction and a pool: by hand (generic over
deadpool's `GenericClient`, as stockroom does) · shelf (`&impl Executor`, proposed) · sqlx
(`impl Executor<'_, Database = Postgres>`, check).

## Claims

- Measured: root PLAN.md §12 #33 (round trip through lagproxy), #42 (`prepare_cached` timings);
  shelf's three strategies (to measure).
- Docs: deadpool-postgres 0.14.2's `GenericClient` and `prepare_cached`.
- Opinion: an ORM's executor trait should be small enough to implement for a new client type
  in ten lines.

## When not to use the executor

- Pipelining many independent queries on one connection (main book
  [Chapter 7](../../book/07-pipelining.md)): use the `Client` directly, or `try_join!` shelf
  calls on one shared client (check that the executor's futures allow it).

## Exercises

Plan:
1. Implement `Executor` for a wrapper that logs each statement's name and duration. *Hint:*
   delegate, and time around the await.
2. Add a statement cache for a plain `Client`. *Hint:* a wrapper that owns the client and a
   map.
3. Map `Error::Stale` to 412 and `NotFound` to 404 in an axum handler. *Hint:* main book
   Chapter 15.

> **Open questions.** (1) The `Send` futures inside axum handlers and the retry helper must
> be probed before the trait is fixed (S1b step 1). (2) What a plain `Client` does without a
> cache: prepare every time, or `query_typed`. (3) Where `resolve`'s result lives when
> statements are cached (Chapter 5's open question).

---

Previous: [Chapter 6 — Tables: the model derive](06-tables.md) · Next: [Chapter 8 — Writing rows: insert, update, delete, upsert](08-writing.md)
