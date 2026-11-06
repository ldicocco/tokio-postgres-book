# Chapter 18 — Capstone: stockroom end to end

> **Async PostgreSQL in Rust** · Previous: [Chapter 17 — Testing against a real database](17-testing.md) ·
> Next: [Appendix — cheat sheets, phrasebook, glossary and ecosystem](19-appendix.md)

stockroom has appeared in pieces in every chapter: a migration here, a repository function
there, a handler, a guard, a worker. This chapter puts it back together. It shows the whole
shape of the service, follows one order from the HTTP request to the email it triggers, maps
every tokio-postgres feature the book covered to the file that uses it, and runs the service
for real, with the output of an actual session. It ends with what a production deployment would
add, and with exercises that extend the project.

stockroom is a click-and-collect backend for a small retail chain: stores hold stock of
products, customers order for pickup at a store, and staff move orders from *pending* through
*paid* and *ready* to *collected*. About 3,900 lines of Rust and two SQL migrations.

---

## 18.1 The shape

```text
 HTTP clients ──▶ axum Router: TraceLayer → TimeoutLayer → handlers (http/)
      ▲              │ Db extractor            │ db::in_transaction (retries 40001/40P01)
      │              ▼                         ▼
      │           repo/ (all SQL) ──▶ deadpool Pool (Fast, 16 connections) ──┐
      │                                        ▲                             │
      │ SSE                                    │ worker::outbox              │
      │                                        │ (SKIP LOCKED)               │
 notify::Hub ◀── notify::listener ◀── one LISTEN connection (poll_message)   │
 (broadcast + Notify)     │                    ▲                             │
                          └─ wakes the worker  │ pg_notify at commit         ▼
                                     PostgreSQL 18: tables, constraints, triggers
```

| Directory | What it holds | Chapters |
|---|---|---|
| `migrations/` | the schema: domains, enum, composite, constraints with names, triggers | 5, 11, 14 |
| `src/db/` | connecting, the pool, migrations, the retry helper, `CancelOnDrop`, TLS | 1, 8, 12, 13, 14 |
| `src/domain/` | `Sku`, `Cents`, `Quantity`, `OrderStatus`, `Address`, `Customer` | 3, 5, 14 |
| `src/repo/` | every SQL statement, one module per aggregate | 2–10, 14 |
| `src/http/` | the router, extractors and handlers | 15, 16 |
| `src/error.rs` | `AppError`: SQLSTATE and constraint names → HTTP | 6, 15 |
| `src/notify/` | the `LISTEN` connection and the in-process hub | 11, 16 |
| `src/worker/` | the outbox worker | 16 |
| `src/cli/` | `seed` (binary `COPY`) and `export` (portal, binary `COPY` out) | 9, 10 |
| `tests/` | 34 tests against PostgreSQL, a database each | 17 |

Three connection roles, three rules. **Pooled connections** serve requests and the worker,
and never keep session state (Chapter 13). **One dedicated connection** listens, driven with
`poll_message` and reconnected with backoff (Chapter 11). **Short-lived connections** run
migrations, `seed` and `export`, with their notices logged (Chapters 10, 14).

## 18.2 A request's life

Follow `POST /orders` for two units of `SKU00001` at `store-01`, as it happened in the session of
§18.4.

**1. The router.** `TraceLayer` opens a span; `TimeoutLayer` starts the 10-second clock. The
`Db` extractor borrows a connection from the pool (or answers 503 after 5 seconds of waiting);
`AppJson<NewOrder>` deserializes the body, and `Sku` and `Quantity` validate themselves on the way
(Chapters 14, 15).

**2. The transaction.** The handler calls `db::in_transaction` at `READ COMMITTED` with an `async
move` closure that owns the order (Chapters 8, 15). Inside, `repo::orders::place`:

- merges repeated SKUs and checks the line count;
- looks up the store and the products in **one round trip**: two queries pipelined with
  `try_join!` (Chapter 7), the products with `sku = ANY($1)`, where `$1` is `text[]` because a
  domain compares as its base type (Chapter 5);
- sorts the lines by product id, so that every order takes its stock locks in the same order and
  two orders can wait for each other but never deadlock (Chapter 8);
- reserves every line with a **guarded `UPDATE`**, `… SET reserved = reserved + $3 WHERE …
  AND available >= $3`, all of them pipelined in order with `try_join_all`; a line that updates
  0 rows means not enough stock, answered with 409 and the available quantity (Chapters 7, 8);
- inserts the order, with the customer as `Json<Customer>` into `jsonb` (Chapter 3), and all its
  lines in one statement with `unnest` over three arrays (Chapter 10).

~~~rust
// stockroom/src/repo/orders.rs (excerpt)
// The same lock order in every transaction: orders sharing products queue, never deadlock.
lines.sort_by_key(|line| line.product_id);
// …
// Every reservation at once: pipelined, sent in this order, run one by one by the server.
let reserve = tx
    .prepare_cached(
        "UPDATE stock SET reserved = reserved + $3
         WHERE store_id = $1 AND product_id = $2 AND available >= $3",
    )
    .await?;
~~~

[stockroom/src/repo/orders.rs](../stockroom/src/repo/orders.rs)

**3. The trigger.** The `INSERT INTO orders` fires `announce_order_event()`, in the same
transaction: it queues a `pg_notify('order_events', …)` with a small JSON event, inserts a job
row into `outbox`, and queues a payload-less `pg_notify('outbox', '')` (Chapter 11). None of it
is visible to anyone yet.

**4. The commit.** `in_transaction` commits. If the commit or anything before it had failed with
a serialization failure or a deadlock, it would have rolled back and tried again, up to eight
attempts in all, with jittered backoff (Chapter 8). At the commit, PostgreSQL delivers the two
notifications, and the order, its reservations and the outbox job become visible together. The
handler answers 201 with `ETag: "1"`.

**5. The listener.** The dedicated connection's driver task receives both notifications through
`poll_message` (Chapter 11). `order_events` becomes `HubEvent::Order` on the broadcast channel;
`outbox` wakes the worker through a `Notify`.

**6. The event stream.** Every open `GET /orders/events` response (filtered by store) receives
the event from its broadcast subscription and writes it as an SSE `event: order` (Chapter 16).
None of them holds a database connection.

**7. The outbox worker.** Woken, it borrows a pooled connection, claims due jobs with `FOR UPDATE
SKIP LOCKED`, delivers them (stockroom's delivery logs a line, standing in for an email), marks
them done, and commits. A crash in the middle would leave the jobs due, for the next worker to
deliver again: at least once (Chapter 16). In the session below, the job was delivered 16 ms
after the order was created.

Then a member of staff marks the order paid, with `If-Match: "1"`; the update checks the version
in its `WHERE` clause (Chapters 8, 15), the status trigger announces the change, and the whole
chain runs again.

## 18.3 The feature map

Every public item of tokio-postgres 0.7.18 that the book teaches, with the chapter that teaches it
and where stockroom or an app uses it. (PLAN.md §5.3 has the same matrix, which the book was
planned from.)

| API | Chapter | Used in |
|---|---|---|
| `connect`, `NoTls`, spawning the `Connection` | 1 | `db/connect.rs`, `tests/common` |
| `Config` (URL and key/value forms, setters) | 1, 12 | [`config.rs`](../stockroom/src/config.rs) |
| `Connection::parameter` | 1 | [`notify/listener.rs`](../stockroom/src/notify/listener.rs) → `/admin/sessions` |
| `is_closed`, `check_connection` | 1, 12, 13 | [`http/health.rs`](../stockroom/src/http/health.rs) (`/readyz`), pglisten |
| `batch_execute` | 2 | [`db/migrate.rs`](../stockroom/src/db/migrate.rs), staging tables |
| `execute`, `query`, `query_one`, `query_opt` | 2 | [`repo/`](../stockroom/src/repo/mod.rs) |
| `query_scalar`, `query_one_scalar`, `query_opt_scalar` | 2 | [`repo/reports.rs`](../stockroom/src/repo/reports.rs) (dashboard) |
| `simple_query`, `SimpleQueryMessage`, `SimpleQueryRow` | 2 | pgq, `repo/reports.rs` (`sessions`) |
| `Row` (`get`, `try_get`), `Column` | 3 | `TryFrom<&Row>` in `repo/` |
| `prepare`, `Statement` (`prepare_typed` in Chapter 4 only) | 4 | pgq `--describe`, `cli/export.rs` |
| `query_typed` and family | 4 | minipool's benchmark |
| `ToSql`, `FromSql`, derives, `Json<T>`, feature flags | 3, 5 | [`domain/`](../stockroom/src/domain/mod.rs) |
| `clear_type_cache` | 5 | `db/migrate.rs` |
| `Error`, `DbError`, `SqlState` | 6 | [`error.rs`](../stockroom/src/error.rs) |
| pipelining (`join!`, `try_join!`, `try_join_all`) | 7 | `repo/orders.rs`, `repo/reports.rs` |
| `transaction`, `build_transaction`, `IsolationLevel`, savepoints | 8 | [`db/retry.rs`](../stockroom/src/db/retry.rs), `http/products.rs` (batch), `cli/export.rs` |
| `GenericClient` | 8, 14 | `repo/` (deadpool's) |
| `query_raw`, `RowStream`, `rows_affected` | 9 | `http/products.rs` (`/products/stream`) |
| `bind`, `query_portal`, `Portal` | 9 | [`cli/export.rs`](../stockroom/src/cli/export.rs) |
| `simple_query_raw` | 9 | pgq `--stream` |
| `copy_in`, `CopyInSink`, `copy_out`, `CopyOutStream` | 10 | `repo/products.rs` (import), `http/products.rs` (export) |
| `binary_copy` | 10 | [`cli/seed.rs`](../stockroom/src/cli/seed.rs), `cli/export.rs`, pgload |
| `poll_message`, `AsyncMessage`, `Notification` | 11 | `notify/listener.rs`, `db/connect.rs`, pglisten |
| `cancel_token`, `CancelToken` | 12 | [`db/cancel.rs`](../stockroom/src/db/cancel.rs) |
| timeouts, keepalives, `options` | 12 | `config.rs` |
| `MakeTlsConnect`, tokio-postgres-rustls | 12 | [`db/tls.rs`](../stockroom/src/db/tls.rs) (`--features tls`) |
| `connect_raw` | 12 | lagproxy users: pgload, minipool |
| deadpool-postgres (`Pool`, `prepare_cached`, `Object::take`) | 13 | [`db/pool.rs`](../stockroom/src/db/pool.rs), `db/cancel.rs` |

## 18.4 Running it

With the database of Chapter 0 and `STOCKROOM_DATABASE_URL` set in `.env`:

```bash
cargo run -p stockroom -- migrate
cargo run -p stockroom -- seed --reset
cargo run -p stockroom -- serve
```

`migrate` on an already migrated database logs the server's notice and does nothing; `seed`
fills the tables with binary `COPY`, deterministically (the same `--seed` gives the same data):

```text
INFO stockroom::db::connect: relation "schema_migrations" already exists, skipping severity="NOTICE" code="42P07"
the schema is up to date

seeded 3 stores, 1000 products, 2995 stock rows, 20000 movements
```

`serve` listens on `127.0.0.1:8080`, migrating first unless told not to, and starts the listener
and the worker:

```text
INFO stockroom::app: listening on http://127.0.0.1:8080
INFO stockroom::notify::listener: listening for order events
```

The session below was run with curl; `stockroom/stockroom.http` has the same requests, and more,
for an editor's REST client. Readiness first: the pool has opened one connection of its 16, and
the listener is connected:

```bash
curl -s localhost:8080/readyz
```

```text
{"listening":true,"pool":{"available":0,"maxSize":16,"size":1,"waiting":0},"status":"ready"}
```

In another terminal, a client subscribes to the store's events:

```bash
curl -N 'localhost:8080/orders/events?store=store-01'
```

An order for two units of `SKU00001`, of which `store-01` has 50 available:

```bash
curl -s -i -X POST localhost:8080/orders -H 'content-type: application/json' \
  -d '{"store":"store-01","customer":{"name":"Ada","email":"ada@example.com"},"lines":[{"sku":"SKU00001","quantity":2}]}'
```

```text
HTTP/1.1 201 Created
content-type: application/json
etag: "1"
content-length: 316
date: Fri, 06 Nov 2026 20:01:04 GMT

{"id":"01a1b273-50f5-71ea-8e39-c645892c55f3","store":"store-01","status":"pending","customer":{"name":"Ada","email":"ada@example.com"},"totalCents":158200,"version":1,"createdAt":"2026-11-06T20:01:04.242203Z","updatedAt":"2026-11-06T20:01:04.242203Z","lines":[{"sku":"SKU00001","quantity":2,"unitPriceCents":79100}]}
```

(The id is a UUID version 7, from `uuidv7()` in the schema: its first digits are a timestamp, so
ids sort by creation time.) Marking it paid, with the version from the `ETag`, and then trying
again with the same, now stale, version:

```bash
curl -s -i -X POST localhost:8080/orders/$ID/transition -H 'content-type: application/json' \
  -H 'If-Match: "1"' -d '{"to":"paid"}'
curl -s -i -X POST localhost:8080/orders/$ID/transition -H 'content-type: application/json' \
  -H 'If-Match: "1"' -d '{"to":"ready"}'
```

```text
HTTP/1.1 200 OK
etag: "2"
{"previous":"pending","order":{"id":"01a1b273-50f5-71ea-8e39-c645892c55f3","store":"store-01","status":"paid", … ,"version":2, … }}
HTTP/1.1 412 Precondition Failed
{"currentVersion":2,"error":"stale_version","message":"the resource has changed: its current version is 2"}
```

The stock shows the reservation, and the event stream received both changes:

```text
[{'sku': 'SKU00001', 'onHand': 50, 'reserved': 2, 'available': 48}]
```

```text
event: order
id: 01a1b273-50f5-71ea-8e39-c645892c55f3:1
data: {"orderId":"01a1b273-50f5-71ea-8e39-c645892c55f3","storeId":"01a1b273-085c-7487-b7e8-500106fafe63","status":"pending","previous":null,"version":1}

event: order
id: 01a1b273-50f5-71ea-8e39-c645892c55f3:2
data: {"orderId":"01a1b273-50f5-71ea-8e39-c645892c55f3","storeId":"01a1b273-085c-7487-b7e8-500106fafe63","status":"paid","previous":"pending","version":2}
```

The server's log shows the outbox worker delivering each job (16 ms after the order's
`createdAt`, for the first):

```text
INFO stockroom::worker::outbox: delivered topic="order.pending" payload={"orderId":"01a1b273-50f5-71ea-8e39-c645892c55f3","previous":null,"status":"pending","storeId":"01a1b273-085c-7487-b7e8-500106fafe63","version":1}
INFO stockroom::worker::outbox: delivered topic="order.paid" payload={"orderId":"01a1b273-50f5-71ea-8e39-c645892c55f3","previous":"pending","status":"paid","storeId":"01a1b273-085c-7487-b7e8-500106fafe63","version":2}
```

And the pipelined dashboard (abridged):

```text
{"products": 1000, "stores": 3, "openOrders": 1, "ordersToday": 1, "revenueTodayCents": 158200,
 "averageOrderCents": "158200.00", "lowStock": [{"store": "store-02", "sku": "SKU00447", "available": 3}, …]}
```

Finally, SIGTERM, with the event stream still connected. The stream ended with the shutdown token,
the server finished in-flight requests, the worker and listener returned, and the pool closed:

```text
INFO stockroom::app: shutting down
INFO stockroom::app: shut down
```

The process exited 1.5 ms after the signal (measured on this machine, localhost).

Over TLS, against the `docker compose` server of Chapter 12:

```bash
cargo run -p stockroom --features tls -- --db-ca-file docker/certs/ca.crt migrate
```

## 18.5 What production would add

stockroom is complete as a teaching service, and deliberately stops short of a few things a
production deployment would need:

- **Metrics.** The pool's `status()` (size, available, waiting), query durations from the
  `tracing` spans every repository function already opens, the outbox's pending count
  (`repo::outbox::pending`), and `pg_notification_queue_usage()`. Export them with an OpenTelemetry
  or Prometheus layer; alert on pool waiters and outbox age.
- **Read replicas.** Reports and the catalog can read from a replica: a second pool whose
  `Config` lists the replicas with `target_session_attrs=read-only` and `load_balance_hosts=random`
  (Chapter 12), and repository functions that say which pool they need. Mind replication lag:
  a client that just wrote must read from the primary.
- **Partitioning.** `stock_movements` grows forever. Partition it by month (`PARTITION BY RANGE
  (at)`), so that old months can be detached and archived, and queries over recent movements
  touch only recent partitions.
- **PgBouncer.** Many service instances, each with a pool of 16, exhaust `max_connections`.
  PgBouncer in transaction mode multiplexes them onto fewer server connections; stockroom
  already keeps no session state between transactions, and `prepare_cached` works with PgBouncer
  1.21+ (Chapter 13). The `LISTEN` connection must bypass it: in transaction mode a session
  does not survive between transactions.
- **Backups and migrations as a deployment step**, with `--migrate false` on the instances, and
  migrations written to be compatible with the previous release (Chapter 14).
- **Delivery for real.** `LogDelivery` stands in for an email service; a real `Deliver`
  implementation needs timeouts shorter than the pool's patience, idempotency keys (the job id),
  and a dead-letter state after too many attempts.

## Summary

- stockroom combines three kinds of connections: pooled (requests and the worker, no session
  state), one dedicated listener, and short-lived ones for the command-line tools.
- An order is one retried transaction: pipelined lookups, sorted locks, guarded pipelined
  reservations, `unnest` inserts. A trigger in the same transaction queues the notifications and
  the outbox job, which the commit releases together.
- After the commit, the listener fans the event out to SSE clients through a broadcast channel,
  and wakes the outbox worker, which delivers at least once with `SKIP LOCKED`.
- Every tokio-postgres feature the book teaches is used by stockroom or one of the apps; §18.3
  maps each to its file.
- Production adds metrics, replicas, partitioning, an external pooler, a migration step and real
  delivery, all on the foundations already in place.

## Exercises

These extend stockroom itself. Each needs a migration, repository code, a handler or a task, and
tests in `tests/`.

1. **Pickup deadlines.** Orders that stay `ready` for more than three days should be cancelled and
   their stock released. Add a background task that does it every minute, safely with several
   instances running.
   *Hint:* `UPDATE … WHERE status = 'ready' AND updated_at < now() - interval '3 days'` with
   `FOR UPDATE SKIP LOCKED` in a subquery, through the existing transition logic so that the
   trigger announces each cancellation.
2. **A low-stock alert.** When a reservation brings a product's available stock below 5, publish
   a `low_stock` event to SSE clients, at most once per product per hour.
   *Hint:* a trigger on `stock`, and a table that remembers when each alert was last sent.
3. **Idempotent orders.** Mobile clients retry `POST /orders` when the network drops, creating
   duplicate orders. Accept an `Idempotency-Key` header and return the original order for a
   repeated key.
   *Hint:* a unique column for the key, `INSERT … ON CONFLICT DO NOTHING RETURNING`, and the
   same transaction as the order.
4. **A read replica for reports.** Add a second, optional pool for `/reports/*`, configured with
   `--replica-url`, falling back to the primary pool when it is not set. How does
   `/readyz` change?
   *Hint:* `AppState` gets an `Option<Pool>`; the readiness check should report both pools.

### Solutions

**1.** Claim the expired orders in batches inside a transaction: `SELECT id FROM orders WHERE
status = 'ready' AND updated_at < now() - interval '3 days' ORDER BY id LIMIT 100 FOR UPDATE SKIP
LOCKED`, then run `repo::orders::transition(tx, id, Cancelled, None)` for each, which releases the
reservation and fires the status trigger. `SKIP LOCKED` lets several instances run the task at
once without waiting for each other or cancelling an order twice, and a staff member's concurrent
transition of the same order is serialized by the row lock. Test it by creating a `ready` order,
moving its `updated_at` back with SQL (§17's fixtures through SQL), running one pass, and checking
the stock levels and the event on the hub.

**2.** An `AFTER UPDATE OF reserved, on_hand ON stock` trigger compares the old and new
`available` values; when the new one crosses below 5, it inserts or updates a row in a
`stock_alerts (store_id, product_id, sent_at)` table with `ON CONFLICT … DO UPDATE … WHERE
stock_alerts.sent_at < now() - interval '1 hour'`, and calls `pg_notify('stock_events', …)` only
when that statement affected a row. The listener gains a channel and a `HubEvent::LowStock`; the
SSE handler a second event type. The hour-long throttle lives in the database, so it holds across
instances and restarts.

**3.** Add `idempotency_key text UNIQUE` to `orders` (nullable, so that keyless orders are
unaffected). In `place`, before reserving anything, try `SELECT id FROM orders WHERE
idempotency_key = $1`: if found, return that order with 200. Otherwise proceed, inserting the key
with the order; a concurrent request with the same key then fails on the unique constraint at the
insert, and its transaction rolls back, reservations included, so the handler can catch that
specific constraint (`orders_idempotency_key_key`) and return the winner's order. The key must be
scoped to the customer in a real system, and expire.

**4.** `AppState { pool, reports: Option<Pool>, … }`, and the report handlers use
`state.reports.as_ref().unwrap_or(&state.pool)`. The replica pool's `Config` comes from
`--replica-url` with `target_session_attrs=read-only`. `/readyz` should check the primary pool as
now and report the replica pool's status separately, without failing readiness when only the
replica is down, since reports can fall back to the primary; whether they should, under load, is
an operational decision worth a flag.

---

Previous: [Chapter 17 — Testing against a real database](17-testing.md) ·
Next: [Appendix — cheat sheets, phrasebook, glossary and ecosystem](19-appendix.md)
