# Plan — *Async PostgreSQL in Rust*: a course on tokio-postgres

This plan is for a course book on [`tokio-postgres`](https://docs.rs/tokio-postgres/latest/tokio_postgres/).
The course goes from the first query to a production-shaped HTTP service built with axum.
Its form and voice follow an existing Rust course that is not part of this repository ("the
reference course").

Status: **All phases (0–4) done on 2026-11-06**: the scaffold, stockroom, the apps, Chapters 0–18
and the appendix, all `current`; 156 runnable examples; every gate green; the book PDF (270
pages) fresh. The §5.3 coverage audit found `host_path`, the keepalive setters and
`tcp_user_timeout` without a runnable example, now added in §12.4. Phases and acceptance criteria are in §9; what building the software and
writing the chapters taught is in §12.

---

## 1. Goal and audience

**Goal.** When readers finish, they can design, build, test and operate a PostgreSQL-backed
Rust service with `tokio-postgres` and axum. They should know what every call does on the wire,
how to keep data correct under concurrency, and where the sharp edges are. The book ends with
**stockroom**, a complete service that uses almost every feature of the crate.

**Audience.**

| They know | They do not need to know |
|---|---|
| **Rust, at an average level.** They know ownership and borrowing, structs, enums and `match`, traits and generics, `Result` and `?`, closures, and iterators. | Pinning, higher-ranked lifetimes, async closures and the `Send` puzzles of async code. The book explains each one where it first matters, in a **Rust refresher** box. |
| **Tokio, at a basic level.** They know `#[tokio::main]`, `.await`, `tokio::spawn`, and have seen a channel. | `select!`, `JoinSet`, cancellation semantics and streams. The book introduces each one when it is needed. |
| **SQL, at a basic level.** They know `SELECT`/`INSERT`/`UPDATE`/`DELETE`, joins, `CREATE TABLE`, and what a transaction is in principle. | Isolation levels, row locking, the wire protocol, `COPY`, `LISTEN`/`NOTIFY`, domains and composites. A **SQL lens** box covers each one. |

The reference course was written for TypeScript and C++ developers learning Rust. This book is
for Rust developers learning a database driver well, so its comparisons and callouts change
(§3). Its structure, rigour and conventions stay the same.

---

## 2. What is carried over from the reference course

These elements are carried over unchanged. They are written down in [CLAUDE.md](CLAUDE.md) as
the style contract.

- **One Markdown file per chapter**, named `NN-slug.md`, with the heading `# Chapter N — Title`.
- **A banner under the heading** with the book's name and the Previous/Next links, and the same
  links again at the foot of the chapter.
- An intro of one or two paragraphs saying why the topic matters, followed by `---` and
  **numbered sections** (`## N.M Title`).
- **Concept first.** Each idea is explained from scratch with a small, complete, runnable program.
- **"Side by side — *concept*"** sections. Each one states a single task and solves it in each
  column, then ends with a **Same / Different** table.
- **"In stockroom"** sections. These replace the reference course's "The codebase" sections and
  point to where the concept lives in the capstone project, with trimmed excerpts and a link.
- **Summary** (bullets), **Exercises** (each with a hint), and **Solutions** for the exercises
  whose answers are not obvious.
- **Two kinds of code block.**
  - ` ```rust ` is a complete program that the tooling compiles and **runs** (§6). Its
    `assert!`s state what the program produces.
  - ` ~~~rust ` is an excerpt or an intentionally incomplete fragment, and is never compiled
    alone. An excerpt's first line is a comment naming the file it comes from.
- The reference course has a **TS/C++ → Rust phrasebook** appendix. This book has an appendix
  of cheat sheets instead (§7, Appendix).
- **One book PDF**, rendered with the reference course's pandoc → Typst pipeline, copied into
  `tools/pdf/`. Unlike the reference course, chapters get no PDFs of their own in
  the repository; any chapter can be rendered locally on demand (`book.py pdf N`, into
  `target/book/`).

---

## 3. What changes for this book

### Side-by-side columns

The reference course compared TypeScript, C++ and Rust. This book compares three things:

| Column | Role | Compiled? |
|---|---|---|
| **SQL / psql** | The same task at the database prompt. This ties the Rust code to SQL the reader already knows, and shows what the server is asked to do. | Run by hand, and the output is shown |
| **sqlx** | The most common alternative Rust driver. It shows which design choices are specific to tokio-postgres: runtime-checked queries versus compile-time-checked macros, explicit statements versus implicit caching, and pipelining on a single connection. | No, it is an illustration |
| **tokio-postgres** | The subject of the book. | Yes, a runnable ` ```rust ` block |

In Part III (axum), psql has nothing to compare with, so the side-by-side sections use two
columns: sqlx and tokio-postgres.

### Callouts

| Box | Used for |
|---|---|
| `> **Protocol lens:**` | What travels over the socket: Parse, Bind, Describe, Execute and Sync messages, round trips, and the simple and extended protocols. This is how the book explains *why* the API is shaped the way it is. |
| `> **SQL lens:**` | Server-side semantics: isolation, MVCC, locks, how `NOTIFY` delivery works, and constraint names. |
| `> **Rust refresher:**` | A language feature the audience may be hazy on, at the point where it first matters: `Pin`, `dyn Trait + Sync`, lifetimes tied to a `Row`, `AsyncFnMut`, `Send` futures. |
| `> **Pitfall:**` | A mistake that compiles and then fails in production. Examples: a dropped future that is not a cancelled query, `BEGIN` on a shared client, a COPY sink that was never finished. |

---

## 4. Repository layout (target)

```text
tokio-postgres-book/
├── PLAN.md                  this file
├── CLAUDE.md                guidance for future development sessions
├── README.md                hub: what is here and how to start (Phase 0)
├── Cargo.toml               workspace: members + [workspace.dependencies], versions declared once
├── .env.example             DATABASE_URL templates (the real .env is gitignored)
├── docker-compose.yml       optional: Postgres 18 with TLS on :5434 (Ch 12)
├── book/
│   ├── book.toml            chapter registry: id, path, title, summary, status (drives tooling)
│   ├── 00-introduction.md … 18-capstone.md, 19-appendix.md
│   └── tokio-postgres-book.pdf (+ .source-sha256): the only PDF tracked
├── playground/              the reader's scratch crate, mirrored
│   ├── src/lib.rs           `connect()` and `slice_iter()` helpers (printed verbatim in Ch 1 and Ch 9)
│   └── examples/            GENERATED from ```rust blocks — never edited by hand
├── apps/                    small, focused example programs (§5.2)
│   ├── pgq/                 a mini psql
│   ├── lagproxy/            a TCP proxy that adds latency, so round trips become visible
│   ├── pgload/              a bulk CSV loader that compares INSERT, UNNEST and COPY
│   ├── pglisten/            a LISTEN/NOTIFY tail with reconnect and catch-up
│   └── minipool/            a connection pool from scratch, with a benchmark against deadpool
├── stockroom/               the capstone service (§5.1)
│   ├── migrations/          0001_init.sql …
│   ├── src/                 lib + bin (`stockroom serve|migrate|seed|export`)
│   ├── tests/               integration tests against a real Postgres
│   └── stockroom.http       a manual request collection (VS Code REST Client format)
└── tools/
    ├── book.py              list / extract / check / links / excerpts / pdf / book
    └── pdf/                 doc.typ, filters.lua (copied from the reference course's pipeline)
```

The examples are compiled in their own crate, but the chapter Markdown is the source of truth
for them. `tools/book.py extract` regenerates `playground/examples/` from the book, which is how
the reference course's maintainer pipeline works too.

---

## 5. The software

### 5.1 The project: **stockroom**

stockroom is a small service for one retail chain. It holds the product catalog, stock per
store, and **click-and-collect orders**: the customer orders online and picks up in store. The
domain sits close to the author's daily work, but stockroom is a new and independent codebase.

The domain was chosen because it raises every interesting database problem in a natural way:

- **Overselling.** Two customers reserve the last unit at the same moment. This motivates
  transactions, isolation levels, row locks, retries and `CHECK` constraints.
- **A live pickup board.** Store staff see orders change state in real time. This motivates
  `LISTEN`/`NOTIFY`, server-sent events (SSE) and fan-out.
- **Price list imports and sales exports.** These motivate `COPY` in and out, streaming, and
  binary COPY.
- **Email and receipt jobs.** These motivate an outbox table and a worker that uses
  `FOR UPDATE SKIP LOCKED`.
- **Edits from two admin tabs.** These motivate optimistic concurrency with ETags and a
  version column.

#### Schema (PostgreSQL 18), as built

The schema is in [stockroom/migrations/](stockroom/migrations/). Every constraint has a name,
and `src/error.rs` turns each name into a message for API clients.

```sql
CREATE TYPE order_status AS ENUM ('pending', 'paid', 'ready', 'collected', 'cancelled');
CREATE TYPE address      AS (street text, city text, postcode text, country char(2));
CREATE DOMAIN sku               AS text    CONSTRAINT sku_format CHECK (VALUE ~ '^[A-Z0-9]{3,12}(-[A-Z0-9]{1,6})?$');
CREATE DOMAIN positive_quantity AS integer CONSTRAINT positive_quantity_check CHECK (VALUE > 0);

stores          (id uuid PK DEFAULT uuidv7(), code text UNIQUE, name text, address address, created_at)
products        (id, sku sku UNIQUE, name, price_cents bigint CHECK (> 0), tags text[], attributes jsonb,
                 version int, created_at, updated_at)
stock           (store_id, product_id, on_hand int CHECK (>= 0), reserved int CHECK (>= 0),
                 available int GENERATED ALWAYS AS (on_hand - reserved) VIRTUAL,
                 CHECK (reserved <= on_hand), PK (store_id, product_id))
orders          (id, store_id, status order_status, customer jsonb, total_cents bigint, version int, …)
order_lines     (order_id, product_id, quantity positive_quantity, unit_price_cents bigint)
stock_movements (id bigint IDENTITY, store_id, product_id, delta int, reason text, at timestamptz)
outbox          (id bigint IDENTITY, topic, payload jsonb, attempts, last_error, available_at, done_at)
-- 0002: trigger on INSERT / UPDATE OF status → pg_notify('order_events', json), an outbox row,
--       and pg_notify('outbox', '')
```

stockroom uses modern PostgreSQL where it fits. The book notes the version each feature needs
the first time it appears:

- `DEFAULT uuidv7()` for every UUID key (PG 18). The `uuid` crate maps the keys to `Uuid`.
- `RETURNING old.status` in order transitions (PG 18).
- A virtual generated column, `stock.available` (PG 18). PG 18 refuses one over domain-typed
  columns ("generation expression uses user-defined type"), so `on_hand` and `reserved` are
  plain integers, and the domain sits on `order_lines.quantity` instead.
- `MERGE … RETURNING merge_action()` for the CSV import (PG 15 and 17). The book compares it with
  `INSERT … ON CONFLICT`.
- `COPY … (FORMAT csv, HEADER MATCH)` (PG 15).

#### HTTP API (axum 0.8), as built

| Method and path | What it shows | Code |
|---|---|---|
| `GET /healthz`, `GET /readyz` | Liveness without the database; readiness with `check_connection` and the pool's `status()` | `http/health.rs` |
| `GET /admin/sessions` | `simple_query` over `pg_stat_activity` (every value comes back as text), and what `Connection::parameter` reported to the listener | `http/health.rs`, `repo/reports.rs` |
| `GET /stores`, `POST /stores` | A **composite** type (`address`) through a derive; 409 and 422 from constraint names | `repo/stores.rs` |
| `GET /products?tag=&cursor=&limit=` | **Keyset pagination** with an opaque cursor, `tags @> ARRAY[$2]`, `prepare_cached` | `http/products.rs`, `repo/products.rs` |
| `GET /products/{sku}`, `POST /products` | A **domain** parameter (`Sku`'s hand-written `accepts`), an ETag | `repo/products.rs`, `domain/sku.rs` |
| `PATCH /products/{sku}` with `If-Match` | **Optimistic concurrency**: `WHERE version = coalesce($2, version)`, and 412 with the current version | `repo/products.rs` |
| `POST /products/batch` | **Savepoints**: one per item, so a failed insert does not abort the transaction | `http/products.rs` |
| `GET /products/stream` | `query_raw` → `RowStream` → an NDJSON body that owns the pooled connection | `http/products.rs` |
| `GET /products/export.csv` | `copy_out` → a streamed CSV body (NULL versus `""` in CSV) | `http/products.rs` |
| `POST /products/import` | The request body streamed into `copy_in` (a temp table, `HEADER MATCH`), a duplicate check, and `MERGE`; COPY errors reported with their line | `repo/products.rs` |
| `GET /stores/{code}/stock`, `POST /stores/{code}/stock/adjust` | The virtual column; a plain `transaction()` plus a ledger row | `http/stock.rs`, `repo/stock.rs` |
| `POST /stock/transfers` | **SERIALIZABLE with retries** (`db::in_transaction`), with rows locked in a fixed order so that opposite transfers do not deadlock | `repo/stock.rs`, `db/retry.rs` |
| `POST /orders` | **Reserving stock without overselling**: guarded `UPDATE … WHERE available >= $3` at READ COMMITTED, **pipelined** with `try_join_all`, locked in product-id order; lines inserted with `unnest` of three arrays | `repo/orders.rs` |
| `GET /orders/{id}` | Two pipelined queries; `Json<Customer>` from `jsonb` | `repo/orders.rs` |
| `POST /orders/{id}/transition` | A **Postgres enum** ↔ a Rust state machine; `status = ANY($3)` with an enum array; `RETURNING old.status`; stock effects | `repo/orders.rs`, `domain/order_status.rs` |
| `GET /orders/events?store=` | **SSE** from a dedicated `LISTEN` connection driven with `poll_message`, via `broadcast`; `resync` on lag or reconnect; ends at shutdown | `notify/listener.rs`, `http/events.rs` |
| `GET /reports/dashboard` | **Pipelining**: five aggregates with `try_join!` on one connection; `numeric` → `rust_decimal` | `repo/reports.rs` |
| `GET /reports/sales?from=&to=&delayMs=` | `statement_timeout` (503 `timeout`), and **`CancelOnDrop`**: an abandoned query is cancelled with the `CancelToken` and its connection discarded | `http/reports.rs`, `db/cancel.rs` |

#### Command line (`stockroom <subcommand>`), as built

| Subcommand | What it shows | Code |
|---|---|---|
| `migrate` | Embedded migrations, `pg_advisory_lock`, transactional DDL, checksums, and server **notices** through `poll_message` | `db/migrate.rs`, `db/connect.rs` |
| `seed [--reset] --stores N --products N --movements N` | **Binary COPY** with `BinaryCopyInWriter` (2,000 products and 50,000 ledger rows in under a second), and an array of a composite type through `unnest` | `cli/seed.rs` |
| `export products\|movements [--out F] [--batch N]` | Products through a **portal** in a `SERIALIZABLE, READ ONLY, DEFERRABLE` transaction; movements through `copy_out` and `BinaryCopyOutStream` to CSV | `cli/export.rs` |
| `serve` | Everything above, plus the listener, the outbox worker (`FOR UPDATE SKIP LOCKED`, woken by `NOTIFY`, delivery through a `Deliver` trait), and graceful shutdown on SIGTERM | `app.rs`, `worker/outbox.rs` |

**Cross-cutting concerns:**

- **Errors.** `AppError` (`error.rs`, with thiserror) classifies a `tokio_postgres::Error` once, by
  SQLSTATE and by constraint name. `DbError` text goes to the log, never to a client. The one
  exception is COPY data errors (class 22), which quote the client's own file back with its
  line number.
- **Logging.** `tracing`: each repository function is a debug span, and `RUST_LOG=stockroom=debug`
  logs its duration.
- **Configuration.** clap flags with environment fallbacks (`config.rs`), plus `.env` through
  dotenvy. `Config` adds `application_name`, `connect_timeout`, keepalives, and server-side
  `statement_timeout`, `lock_timeout` and `idle_in_transaction_session_timeout` through
  `options`.
- **TLS.** The cargo feature `tls` (`db/tls.rs`) uses tokio-postgres-rustls with the ring
  provider. It was verified against the docker server: it connects with `--db-ca-file`, fails
  with `UnknownIssuer` on the system roots, and `sslmode=disable` is refused.
- **Tests.** 43 tests in `tests/`: one database per test, cloned from a template; HTTP through
  `oneshot`. The concurrency tests are oversell (passed 50 out of 50 runs), orders sharing
  products, and transfers in both directions. Event tests cover the listener, a reconnect after
  `pg_terminate_backend`, SSE, the outbox and two `SKIP LOCKED` workers. There are also tests of
  cancellation on the server and of `statement_timeout`.

### 5.2 Example programs (`apps/`), as built

These are small, single-purpose tools. Each one is a library (which the tests and the chapters
use) plus a binary with `--help`, and each is the "real program" behind one or two chapters.
They read `DATABASE_URL` from the environment or from `.env`.

| App | Chapter(s) | What it shows | Tests |
|---|---|---|---|
| **lagproxy** | 4, 7, 10, 12, 13 | A TCP proxy that delays every chunk by N ms each way. It adds latency without limiting bandwidth: chunks are timestamped and forwarded in order. It works as a library (`Proxy::start`, used in-process by pgload and minipool) and as a binary (used by `book.py check` for `needs=lagproxy`). The delay is a lower bound: on this Mac a round trip costs `2 × delay` plus 3 to 6 ms, because of timer granularity (measured: 15.3 ms at 5 ms, 26 ms at 10 ms). | 2 (an echo server, no database) |
| **pgq** | 2, 4, 9 | A mini psql. It uses `simple_query` with `RowDescription` and `CommandComplete`, renders tables like psql, prints notices through `poll_message`, and shows errors with their SQLSTATE. `--describe` uses `prepare` to show parameter and column types and source columns. `--stream` uses `simple_query_raw`. It also has `--timing` and an interactive mode. | 4 |
| **pgload** | 10 | The same generated rows (with commas and quotes in them) loaded five ways: one `INSERT` at a time, pipelined `INSERT`, `unnest` batches, CSV `COPY`, and binary `COPY`. It prints a Markdown timing table, and with `--delay-ms` it connects through an in-process lagproxy using `connect_raw`. The test checks that all five store identical rows (an md5 fingerprint). | 1 |
| **pglisten** | 11, 12 | A `LISTEN` tail built on a `run(config, options, on_event, shutdown)` library. It drives the connection with `poll_message`, quotes channel names with `quote_ident`, and reconnects with backoff. A catch-up query runs after each `LISTEN`, and an optional `check_connection` ping runs on an interval. | 4 (one unit test, three against PostgreSQL) |
| **minipool** | 13 | A pool in about 100 lines: a `Semaphore`, a `Mutex<Vec<Client>>` used as a stack, a `Conn` guard (`Deref`/`DerefMut`, `detach`), a wait timeout, and `is_closed` plus an optional `check_connection` on checkout. The binary benchmarks one shared client, minipool and deadpool with `query_typed` (one round trip). | 7 |

**Measured** (release build, PG 18.6 on localhost, Apple Silicon; Chapter 10 and 13 reproduce
these):

```text
pgload --rows 20000                         pgload --rows 2000 --delay-ms 1     (re-run 2026-11-06)
| Strategy                 | Time    | ×     | Time    | ×      |
| INSERT, one at a time    | 0.761 s | 103.0 | 9.663 s | 220.1  |
| INSERT, pipelined        | 0.118 s |  16.0 | 0.200 s |   4.6  |
| INSERT … unnest, batches | 0.012 s |   1.7 | 0.047 s |   1.1  |
| COPY, CSV                | 0.011 s |   1.5 | 0.052 s |   1.2  |
| COPY, binary             | 0.007 s |   1.0 | 0.044 s |   1.0  |

minipool --tasks 64 --pool-size 8           ops/s, no delay    ops/s, 2 ms delay   (re-run 2026-11-06)
one shared client (pipelined), query        63,403             8,350
minipool, 8 connections, query              78,587             1,038
deadpool, 8 connections, query              77,514               962
minipool / deadpool, transaction            29,786 / 30,006    334 / 352
```

### 5.3 Feature coverage matrix

The rule is that every public item of `tokio-postgres` 0.7.18 gets at least one runnable
example in the book, and the important ones also appear in stockroom. Chapter numbers refer to
§7.

| API item | Ch | stockroom (paths under `stockroom/src/`) / app |
|---|---|---|
| `connect`, `NoTls`, `Client`/`Connection` split, spawning the connection | 1 | `db/connect.rs`, `tests/common` |
| `Config` (`FromStr`, key/value and URL forms, builder setters) | 1, 12 | `config.rs` |
| `Connection::parameter` | 1 | `notify/listener.rs` → `/admin/sessions` |
| `Client::is_closed`, `check_connection` | 1, 12 | `http/health.rs` (`/readyz`), pglisten |
| `batch_execute` | 2 | `db/migrate.rs`, `repo/products.rs` (staging table) |
| `execute`, `query`, `query_one`, `query_opt` | 2 | `repo/*.rs` |
| `query_scalar`, `query_one_scalar`, `query_opt_scalar` | 2 | `repo/reports.rs` (dashboard) |
| `simple_query`, `SimpleQueryMessage` (`Row`, `CommandComplete`, `RowDescription`), `SimpleQueryRow`, `SimpleColumn` | 2 | pgq, `repo/reports.rs` (`sessions`) |
| `simple_query_raw`, `SimpleQueryStream` | 9 | pgq (streaming mode) |
| `Row` (`get`, `try_get`, `columns`, `len`, `is_empty`, `raw_size_bytes`), `Column` | 3 | `TryFrom<&Row>` in `repo/*.rs` |
| `prepare`, `Statement` (`params`, `columns`), `ToStatement` | 4 | pgq `--describe`, `cli/export.rs`, `tests/types.rs` |
| `prepare_typed` | 4 | — |
| `query_typed`, `query_typed_one`, `query_typed_opt`, `query_typed_raw`, `execute_typed` | 4 | playground only (stockroom prepares and caches) |
| `clear_type_cache` | 5 | `db/migrate.rs` |
| `types::{ToSql, FromSql, FromSqlOwned, IsNull, Type, Kind, Field, WrongType, WasNull, accepts!, to_sql_checked!}` | 3, 5 | `domain/*.rs` |
| `#[derive(ToSql, FromSql)]`: enum, domain, composite, `transparent`, `rename_all`, `allow_mismatch` | 5 | `domain/`: `OrderStatus`, `Address`, `Quantity`, `Cents`; `Sku` by hand |
| `types::Json<T>`, the `with-serde_json-1` feature | 3, 5 | `repo/orders.rs` (`Json<Customer>`), `products.attributes` (`Value`) |
| Feature flags `with-uuid-1`, `with-jiff-0_2`, `array-impls` (others shown in a table) | 3 | the whole project |
| `error::{Error, DbError, SqlState, Severity, ErrorPosition}`, `Error::{code, as_db_error, is_closed, into_source}` | 6 | `error.rs`, `repo/products.rs` (`input_error`) |
| Pipelining (`join!`, `try_join!`, `buffer_unordered`), `Arc<Client>` | 7 | `repo/reports.rs`, `repo/orders.rs` (reservations, `get`), `repo/stock.rs` |
| `transaction`, `Transaction::{commit, rollback}`, rollback on drop | 8 | `http/stock.rs` (`adjust`), `db/retry.rs` |
| `build_transaction`, `TransactionBuilder::{isolation_level, read_only, deferrable, start}`, `IsolationLevel` | 8 | `db/retry.rs`, `cli/export.rs` |
| `Transaction::{transaction, savepoint}` (nested transactions) | 8 | `http/products.rs` (`create_batch`) |
| `GenericClient` (tokio-postgres's; deadpool's in stockroom) | 8, 14 | `repo/*.rs` |
| `query_raw`, `RowStream` (`rows_affected`), `BorrowToSql`, `execute_raw` | 9 | `http/products.rs` (`stream`), `db::slice_iter` |
| `Transaction::{bind, bind_raw, query_portal, query_portal_raw}`, `Portal` | 9 | `cli/export.rs` |
| `copy_in`, `CopyInSink` (`finish`), `copy_out`, `CopyOutStream` | 10 | `repo/products.rs` (import), `http/products.rs` (export), pgload |
| `binary_copy::{BinaryCopyInWriter, BinaryCopyOutStream, BinaryCopyOutRow}` | 10 | `cli/seed.rs`, `cli/export.rs`, pgload |
| `Connection::poll_message`, `AsyncMessage::{Notification, Notice}`, `Notification` | 11 | `notify/listener.rs`, `db/connect.rs` (notices), pglisten |
| `cancel_token`, `CancelToken::{cancel_query, cancel_query_raw}` | 12 | `db/cancel.rs` (`CancelOnDrop`) |
| `Config::{connect_timeout, keepalives*, tcp_user_timeout, target_session_attrs, load_balance_hosts, hostaddr, host_path, options, channel_binding, ssl_mode, ssl_negotiation}` | 12 | `config.rs` (timeouts, keepalives, options); the rest in the playground |
| `tls::{MakeTlsConnect, TlsConnect}`, `postgres-native-tls`, `postgres-openssl`, `tokio-postgres-rustls` | 12 | `db/tls.rs` (the `tls` feature) |
| `Config::connect_raw`, `Socket`, the `runtime` feature | 12 | lagproxy |
| deadpool-postgres (`Manager`, `RecyclingMethod`, `prepare_cached`, `Object`, `Object::take`), bb8 for comparison | 13 | `db/pool.rs`, `db/cancel.rs`, `http/mod.rs` (`Db`), minipool |

The deprecated `Client::cancel_query` is mentioned only to say that `cancel_token` replaces it.
The `js` feature, which targets wasm, is not covered.

---

## 6. Tooling and verification

The reference course guarantees that every ` ```rust ` block compiles and runs. This book keeps
that guarantee against a real database.

**`tools/book.py`** (Python 3, no third-party packages; the PDF commands need pandoc and Typst):

| Command | What it does |
|---|---|
| `list` | Every chapter with its status (`planned`/`draft`/`current`), example and TODO counts, and whether the book PDF is fresh |
| `extract` | Writes each runnable block to `playground/examples/chNN_KK.rs`, with a header naming the source file and line and marking the file as generated |
| `check [--chapter N] [--no-run]` | Runs `extract`, recreates the database `tpbook_check`, runs `cargo build`, `clippy -Dwarnings` and `fmt --check` on the examples, then runs each example with a timeout and `DATABASE_URL` pointing at that database. Prints a pass/fail table. |
| `links` | Checks that every relative link resolves |
| `excerpts` | Checks every ` ~~~rust ` excerpt whose first line names a file: the file must exist, and every line that is not an ellipsis must appear in the file. This catches drift between the book and stockroom. |
| `book` | Renders the whole book (the `current` chapters) to `book/tokio-postgres-book.pdf` with a provenance stamp, using the reference course's pandoc → Typst pipeline; `check` warns when it is stale |
| `pdf N\|ID… \| --all` | Renders chapters for local use into `target/book/` (gitignored, never committed) |

**Directives for code blocks.** Pandoc cannot parse info strings like ` ```rust,ignore `, so a
runnable block that needs special treatment is preceded by an HTML comment, which the PDF drops:

```text
<!-- check: no-run -->            compile only (e.g. a server that listens forever)
<!-- check: needs=tls -->         run only when TLS_DATABASE_URL is set (docker compose)
<!-- check: needs=lagproxy -->    the tool starts lagproxy and points DATABASE_URL through it
<!-- check: should-panic -->      the example demonstrates a panic (e.g. Row::get on a NULL)
```

**Example hygiene.**

- Examples run in any order and can be re-run. They use `CREATE TEMP TABLE`, or create their
  own schema and drop it at the end.
- Examples never depend on each other's data.
- Runnable axum examples do not bind a port. They call the router with
  `tower::ServiceExt::oneshot` and assert on the response, so they terminate.

**Project gates** (the equivalent of the reference repository's CI):

```bash
cargo test --workspace            # stockroom integration tests need DATABASE_URL (see .env)
cargo clippy --workspace --all-targets -- -Dwarnings
cargo fmt --all -- --check
python3 tools/book.py check       # every runnable block in the book
python3 tools/book.py excerpts    # the book agrees with stockroom
python3 tools/book.py links
```

---

## 7. Chapter outline

There are 19 chapters and an appendix, in three parts. The length targets follow the reference
course, at about 700–1,100 lines per chapter. Each chapter lists its sections, its side-by-side
topic, where it appears **in stockroom**, and its exercises.

### Part I — Foundations

**Chapter 0 — Introduction and setup**

- Who the book is for. What tokio-postgres is and what it is not: a driver, not an ORM.
- Where it sits among the alternatives: `postgres` (its synchronous wrapper), sqlx,
  diesel/diesel-async and SeaORM. Why learning it pays off: deadpool, refinery and Cornucopia all
  build on it.
- How chapters are built: the callouts, the two code-block kinds, and the side-by-side columns.
- Setup: a local PostgreSQL 18 (an installer, Homebrew or Docker) and `psql`; `DATABASE_URL`; the playground crate
  and its `Cargo.toml`; a running example extracted from the book.
- stockroom in one page (a diagram).
- A ten-line taste.
- Exercises.

**Chapter 1 — Connecting**

- 1.1 Two halves: `Client` sends requests and `Connection` does the IO. The connection task is
  an actor behind an mpsc channel.
- 1.2 Spawning the connection: what happens if you forget (everything hangs), and logging its
  error.
- 1.3 Connection strings: the key/value and URL forms, `DATABASE_URL`, and parsing a `Config`
  with `FromStr`.
- 1.4 The `Config` builder: `user`, `password`, `dbname`, `host` and `port`, Unix sockets,
  `application_name`, `connect_timeout`, and `options` (`-c statement_timeout=…`).
- 1.5 Lifecycle: dropping the `Client` ends the `Connection`; `is_closed`; `check_connection`;
  `Connection::parameter` (`server_version`, `TimeZone`).
- 1.6 What connection failures look like: a wrong password (28P01), a refused connection, a
  timeout.
- 1.7 The playground's `connect()` helper, printed in full. Later chapters use it.
- Side by side: connecting.
- In stockroom: `db/connect.rs`.
- Exercises include "make a connection with a 1-second timeout to a black-hole address".

**Chapter 2 — Running SQL: execute, query and parameters**

- 2.1 `batch_execute` for schema scripts.
- 2.2 `execute` and the row count it returns.
- 2.3 Parameters `$1…$n`. Includes a **Rust refresher** on `&[&(dyn ToSql + Sync)]`.
  `format!` is SQL injection, shown with a live demo. Identifiers cannot be parameters, so the
  section writes a `quote_ident` helper.
- 2.4 `query`, `query_one` (exactly one row) and `query_opt`.
- 2.5 The scalar shortcuts `query_scalar`, `query_one_scalar` and `query_opt_scalar`.
- 2.6 `RETURNING`, and upserts with `INSERT … ON CONFLICT`.
- 2.7 The simple query protocol: `simple_query`, `SimpleQueryMessage`, everything as text, and
  when you would want it.
- Side by side: inserting a row and reading it back.
- In stockroom: the repository basics. Introduces **pgq**.

**Chapter 3 — Rows and the type mapping**

- 3.1 `Row`: `get` versus `try_get`, by index or by name, and `columns()`.
- 3.2 The core type table.
- 3.3 There are no unsigned integers except `oid`; what `WrongType` looks like.
- 3.4 Feature flags: uuid, serde_json, jiff/chrono/time, `rust_decimal` (`db-tokio-postgres`)
  for `NUMERIC`, cidr, geo-types, and `array-impls`.
- 3.5 Borrowing from a row (`&str`, `&[u8]`), and `FromSql<'a>` versus `FromSqlOwned`.
  Includes a Rust refresher on lifetimes.
- 3.6 NULL ↔ `Option` and `WasNull`.
- 3.7 Turning rows into structs: `TryFrom<&Row>`, and the trade-offs between reading by name
  and by index.
- 3.8 JSON and JSONB: `serde_json::Value` and `Json<T>`.
- 3.9 Arrays: `Vec<T>`, the `= ANY($1)` idiom in place of an `IN` list, and the
  multi-dimensional caveat.
- Side by side: a row into a struct.
- In stockroom: `model/*.rs`.

**Chapter 4 — Prepared statements and the extended protocol**

- 4.1 What `query("…")` sends: Parse/Describe and then Bind/Execute/Sync, which is two round
  trips. This section is a **Protocol lens**.
- 4.2 `prepare` and `Statement` (`params`, `columns`): reusing a statement, and the fact that a
  statement belongs to one connection.
- 4.3 `ToStatement`: passing `&str` or `&Statement`.
- 4.4 Type inference: why `SELECT $1` fails, a cast versus `prepare_typed`.
- 4.5 `query_typed` and `execute_typed`: one round trip and no named statement, which suits
  PgBouncer transaction mode and one-off queries.
- 4.6 Statement caching by hand (a `HashMap<String, Statement>`), and the
  "cached plan must not change result type" error after DDL.
- 4.7 Measuring round trips through **lagproxy**.
- Side by side: prepared statements (psql `PREPARE`/`EXECUTE`, sqlx's implicit cache, explicit
  statements in tokio-postgres).
- In stockroom: `prepare_cached`.

**Chapter 5 — Custom types: ToSql and FromSql**

- 5.1 The traits: `to_sql` into a `BytesMut`, `accepts`, `to_sql_checked!`, `from_sql`, and
  `from_sql_null`.
- 5.2 A newtype by hand: `Cents(i64)`, and a `Sku` that is validated on the way in.
- 5.3 `#[postgres(transparent)]`.
- 5.4 Enums ↔ Postgres `ENUM`: `rename_all` and `allow_mismatch`.
- 5.5 Domains.
- 5.6 Composites, and arrays of composites.
- 5.7 `Type` and `Kind` introspection. The type cache, and `clear_type_cache` after
  `ALTER TYPE`.
- Side by side: a Postgres enum.
- In stockroom: `domain/*.rs`.

**Chapter 6 — Errors**

- 6.1 One `Error` type with many causes: database, IO, closed connection, conversion and parse
  errors; `source()`.
- 6.2 `DbError`: severity, code, message, detail, hint, position, schema, table, column,
  constraint and `where_`.
- 6.3 `SqlState` constants in a `match`. This was verified: they work as patterns.
- 6.4 `is_closed`.
- 6.5 Designing `AppError` with thiserror, using constraint names as part of the API.
- 6.6 Error states inside a transaction: 25P02, "current transaction is aborted".
- 6.7 Notices are not errors (a preview of Chapter 11).
- Side by side: handling a unique violation.
- In stockroom: `error.rs`.

### Part II — The async power tools

**Chapter 7 — One connection, many tasks: pipelining**

- 7.1 Futures are lazy, and requests are sent in the order the futures are *first polled*.
- 7.2 `join!` and `try_join!`: N queries cost about one round trip, measured through lagproxy.
- 7.3 Sharing one `Client` with `Arc<Client>` (it is `Send + Sync`, and its methods take `&self`).
- 7.4 Under the hood: requests go into an mpsc queue, so ordering is guaranteed. Head-of-line
  blocking: the server still runs queries one after another.
- 7.5 Bounded concurrency with `buffer_unordered` and `FuturesUnordered`.
- 7.6 When not to pipeline: dependent queries and transactions.
- Side by side: concurrent queries. sqlx needs a pool for this; tokio-postgres does it on one
  connection.
- In stockroom: `/reports/dashboard`.

**Chapter 8 — Transactions and concurrency control**

- 8.1 `transaction()`, `commit`, rollback on drop, and an explicit `rollback`.
- 8.2 Why `transaction` takes `&mut self`: the borrow checker enforces "nothing else on this
  connection". A **Pitfall**: running `BEGIN` through `batch_execute` on a shared
  `Arc<Client>`, demonstrated.
- 8.3 `build_transaction`: `IsolationLevel`, `read_only` and `deferrable`. A **SQL lens** on
  MVCC and SSI.
- 8.4 Savepoints: `tx.transaction()` and `tx.savepoint("name")`, used to recover from a failed
  statement.
- 8.5 `GenericClient`. It is sealed and uses async-trait. Functions take a generic
  `C: GenericClient`.
- 8.6 Retrying serialization failures and deadlocks with an `AsyncFnMut` retry helper,
  exponential backoff and jitter. A **Rust refresher** on async closures and higher-ranked
  lifetimes. **Verify** that this compiles when the chapter is written.
- 8.7 Locks: `FOR UPDATE`, `SKIP LOCKED`, `NOWAIT`, `lock_timeout`, and advisory locks.
- 8.8 Optimistic concurrency with a version column, where 0 rows affected means a conflict.
- 8.9 Overselling worked through with three strategies: a guarded `UPDATE` with a `CHECK`,
  `FOR UPDATE`, and `SERIALIZABLE` with a retry. Each is proven with 50 concurrent tasks.
- Side by side: a transfer in a transaction.
- In stockroom: `place_order`.

**Chapter 9 — Streaming rows and portals**

- 9.1 Why a `Vec<Row>` is not always enough.
- 9.2 `query_raw` and `RowStream`: parameters as an `ExactSizeIterator` of `BorrowToSql`, the
  `slice_iter` helper, `pin!`, `try_next`, and `rows_affected()`. A **Rust refresher** on `Pin`.
- 9.3 Backpressure. A **Protocol lens**: each request's response channel is bounded at 1, so a
  slow consumer stalls the *whole* connection. This was verified in the 0.7.18 source and will
  be demonstrated in the chapter.
- 9.4 Portals: `bind` with `query_portal(n)` inside a transaction, and `query_portal_raw`.
- 9.5 The SQL alternative: `DECLARE` and `FETCH` cursors.
- 9.6 Keyset pagination, which is the HTTP-friendly answer.
- 9.7 `simple_query_raw`.
- Side by side: iterating over a million rows.
- In stockroom: `/products/stream` and `export`.

**Chapter 10 — Bulk data: COPY**

- 10.1 `copy_in` and `CopyInSink`: `SinkExt::send`, `finish`, and a **Pitfall**: dropping the
  sink before `finish()` aborts the COPY.
- 10.2 `copy_out` and `CopyOutStream`.
- 10.3 Binary COPY: `BinaryCopyInWriter` and `BinaryCopyOutStream`/`BinaryCopyOutRow`.
- 10.4 Failure in the middle of a COPY: the staging-table pattern, and COPY's `ON_ERROR ignore`
  (PG 17) with `REJECT_LIMIT` (PG 18).
- 10.5 The **pgload** benchmark table: INSERT versus pipelined INSERT versus UNNEST versus COPY
  text versus COPY binary.
- 10.6 COPY into a temp table, then `INSERT … ON CONFLICT`.
- Side by side: a bulk load (psql `\copy`, sqlx `copy_in_raw`, tokio-postgres).
- In stockroom: import and export, and `seed`.

**Chapter 11 — LISTEN/NOTIFY and notices**

- 11.1 A **SQL lens** on `NOTIFY`: delivered at commit, payloads of at most 8,000 bytes, no
  persistence.
- 11.2 Driving the `Connection` yourself: `poll_message` with `stream::poll_fn`, forwarding into
  a channel, and the requirement that you keep polling it.
- 11.3 `AsyncMessage` (non-exhaustive): `Notification` (`process_id`, `channel`, `payload`) and
  `Notice` (`RAISE NOTICE`).
- 11.4 Triggers that call `pg_notify` with a JSON payload.
- 11.5 A dedicated listener connection, and why it must not come from the pool.
- 11.6 Reliability: listen first, then run a catch-up query, then deduplicate.
- 11.7 Fan-out with `broadcast`, and handling `Lagged`.
- Side by side: subscribing to events.
- In stockroom: `notify/listener.rs`. Also **pglisten**.

**Chapter 12 — Production connections: timeouts, cancellation, health and TLS**

- 12.1 A **Pitfall**: dropping a future does not cancel the query. Demonstrated with `pg_sleep`
  and `pg_stat_activity`.
- 12.2 `CancelToken::cancel_query`: it opens a separate connection, and the query fails with
  57014.
- 12.3 A `with_timeout` helper that calls cancel when the timeout elapses. Server-side
  `statement_timeout`, `lock_timeout` and `idle_in_transaction_session_timeout`, set with
  `SET LOCAL`.
- 12.4 Dead connections: keepalives and `tcp_user_timeout`, detecting them, and a reconnect
  loop with backoff.
- 12.5 Several hosts: `target_session_attrs` and `load_balance_hosts`.
- 12.6 TLS:
  - `MakeTlsConnect` and `TlsConnect`.
  - **What `sslmode` means in tokio-postgres.** It has only disable, prefer and require.
    Verifying the certificate is the connector's job.
  - The native-tls, openssl and rustls connectors.
  - `sslnegotiation=direct` (PG 17+), which runs against the docker compose server.
  - `channel_binding`.
- 12.7 `connect_raw` over your own stream (through lagproxy), and the `runtime` feature.
- Side by side: a query timeout.
- In stockroom: `config.rs` and `db/cancel.rs`.

### Part III — Building the service

**Chapter 13 — Connection pooling**

- 13.1 Why one `Client` is not enough: transactions need `&mut`, head-of-line blocking, and
  failure isolation.
- 13.2 **minipool**, a pool built from scratch: a `Semaphore`, a `Mutex<Vec<Client>>`, a guard
  that returns the client on `Drop`, and a health check on checkout.
- 13.3 deadpool-postgres: `Manager`, `ManagerConfig`, `RecyclingMethod` (Fast, Verified, Clean,
  Custom), `Pool::builder`, and timeouts.
- 13.4 `Object`, `prepare_cached`, `transaction`, and `Object::take`.
- 13.5 Sizing a pool against `max_connections`, measured through lagproxy.
- 13.6 bb8 in brief; PgBouncer and `query_typed`.
- 13.7 What not to do with pooled connections: `LISTEN`, long streams, and session `SET`.
- Side by side: a pool.
- In stockroom: `db/pool.rs`.

**Chapter 14 — Schema, migrations and the data layer**

- 14.1 Embedded migrations: `include_str!`, a migrations table, and checksums.
- 14.2 Running them safely: `pg_advisory_lock`, and transactional DDL, which is a **SQL lens**
  (the exception is `CREATE INDEX CONCURRENTLY`). Logging notices.
- 14.3 refinery, the ready-made alternative.
- 14.4 The repository layer: functions over `GenericClient` (or deadpool's version, which
  supports `prepare_cached`), with SQL kept in one place.
- 14.5 Domain types at the boundary.
- 14.6 Generating code from SQL with Cornucopia, in brief (Clorinde merged back into Cornucopia 1.0 in 2026; checked 2026-11-06).
- In stockroom: `migrations/`, `db/migrate.rs` and `repo/`.

**Chapter 15 — axum I: a database-backed JSON API**

- 15.1 The shape of the app: `AppState`, the router, `axum::serve` and tracing.
- 15.2 A `DbConn` extractor that implements `FromRequestParts`.
- 15.3 `AppError` → `IntoResponse`, with `SqlState` mapped to 409, 412, 422, 404 and 503.
- 15.4 Writes in transactions, with the retry helper.
- 15.5 Keyset pagination with an opaque cursor.
- 15.6 ETag and `If-Match` ↔ the version column.
- 15.7 Validation at the edge.
- 15.8 The pipelined dashboard.
- Side by side: an axum handler with sqlx and with tokio-postgres.
- In stockroom: `http/`.

**Chapter 16 — axum II: streaming, real-time and background work**

- 16.1 NDJSON from a `RowStream`. The pooled `Object` has to live inside the body stream (an
  ownership puzzle), and the section covers backpressure.
- 16.2 A CSV export with `copy_out`.
- 16.3 A CSV import: the request body goes into `copy_in`, through staging, into an upsert,
  with a body limit.
- 16.4 SSE from `LISTEN` through `broadcast`, with keep-alive.
- 16.5 Timeouts and client disconnects: a `CancelOnDrop` guard.
- 16.6 The outbox worker: `SKIP LOCKED`, woken by `NOTIFY` with polling as a fallback, giving
  at-least-once delivery.
- 16.7 Graceful shutdown with `CancellationToken`.
- 16.8 Health and readiness checks.
- In stockroom.

**Chapter 17 — Testing against a real database**

- 17.1 What to test at each layer.
- 17.2 One database per test, cloned from a migrated template (`CREATE DATABASE … TEMPLATE`),
  and a cleanup guard. Async drop is not available, and the section explains how to work
  around that.
- 17.3 Rolling back a transaction per test, and its limits: it cannot cover commit, `NOTIFY`, or
  several connections.
- 17.4 HTTP tests with `oneshot`.
- 17.5 Concurrency tests: overselling, and forced serialization conflicts.
- 17.6 testcontainers-modules, and Postgres in CI.
- 17.7 Provoking every `SqlState` the error mapping handles.
- In stockroom: `tests/`.

**Chapter 18 — Capstone: stockroom end to end**

- 18.1 The shape.
- 18.2 A request's life: `POST /orders` → transaction → trigger → `NOTIFY` → SSE, and the outbox
  job.
- 18.3 The feature map: every API item mapped to the file that uses it (§5.3, with links).
- 18.4 Running it: migrate, seed, then a session in `stockroom.http`.
- 18.5 What production would add: metrics, read replicas, partitioning, and PgBouncer.
- Exercises that extend the project.

**Appendix (19)**

- A. An API cheat sheet: every `Client` and `Transaction` method and its return type.
- B. The complete type-mapping table, with feature flags.
- C. A `SqlState` cheat sheet with suggested HTTP mappings.
- D. A phrasebook from sqlx and diesel to tokio-postgres.
- E. A glossary: simple and extended protocol, statement, portal, round trip, pipelining, MVCC,
  SSI, and others.
- F. **The crates built on tokio-postgres** (requested 2026-11-06). The most-used reverse
  dependencies on crates.io, researched when the appendix is written, never from memory. For
  each crate:
  - what it is for;
  - which tokio-postgres types it exposes or wraps (`Client`, `GenericClient`, `ToSql`/`FromSql`,
    the `Connection`);
  - how it relates to the book's chapters, and when to choose it.

  Candidates: `postgres` (the blocking wrapper), `postgres-types`/`postgres-protocol` (the
  layers below), deadpool-postgres and bb8-postgres (pools), the TLS connectors
  (postgres-native-tls, postgres-openssl, tokio-postgres-rustls), refinery (migrations),
  Cornucopia (code generation; Clorinde merged back into it), diesel-async's PostgreSQL backend, and row-mapping
  helpers (`postgres-from-row`, `tokio-pg-mapper`).
- G. Further reading.

---

## 8. Versions and environment (verified 2026-11-06)

| Item | Version | Notes |
|---|---|---|
| rustc / cargo | 1.99.1 | edition 2024 |
| tokio-postgres | 0.7.18 | Source read in `~/.cargo/registry`. It includes `query_scalar*`, `query_typed*`, `execute_typed` and `check_connection`. |
| postgres-types | 0.2.14 | `derive` feature |
| tokio | 1.54.1 | |
| axum | 0.8.9 | `/{param}` path syntax |
| tower-http | 0.7.1 | Newer than the reference course; check the API changes when writing Part III |
| deadpool-postgres | 0.14.2 | Its default `RecyclingMethod` is `Fast` |
| bb8-postgres | 0.9.0 | Mentioned only |
| tokio-postgres-rustls / postgres-native-tls / postgres-openssl | 0.14.0 / 0.5.3 / 0.5.3 | |
| jiff / uuid / serde_json / rust_decimal | 0.2.39 / 1.29.0 / 1.0.154 / 1.43.0 | |
| thiserror / anyhow / tracing / tracing-subscriber | 2.0.21 / 1.0.104 / 0.1.44 / 0.3.23 | |
| testcontainers-modules / refinery | 0.15.0 / 0.10.0 | |
| **PostgreSQL (local)** | **18.6** (EDB installer, `localhost:5433`, `ssl = off`) | **The target.** Every example and all of stockroom are verified on it. |
| PostgreSQL (docker, optional) | 18 with TLS on `:5434`, `hostssl` only | Only for the TLS examples (Chapter 12) |
| pandoc / typst | installed | For the PDFs |

The book targets **PostgreSQL 18** and uses its features freely. The first time it uses a
feature newer than PG 14, it says which version introduced it, for example "`MERGE` (PG 15+)"
or "`uuidv7()` (PG 18+)". Readers on older servers then know what to change.

---

## 9. Phases

Each phase ends with its gates green, and the chapter registry is updated with each chapter's
status.

| Phase | Deliverable | Done when |
|---|---|---|
| **0. Scaffold** | `git init`, workspace `Cargo.toml`, `.env`, docker compose, `playground` crate, `tools/book.py` (`list`/`extract`/`check`/`links`/`excerpts`), PDF template copied over, `book/book.toml` with 20 `planned` entries, README hub | `book.py check` passes on a stub chapter that contains one runnable block |
| **1. stockroom** | Migrations, domain types, repository layer, pool, axum API, streaming, COPY, LISTEN/SSE, outbox worker, CLI, tests, `stockroom.http` | All three cargo gates pass; the oversell test passes 50 out of 50 runs; a manual `.http` session works |
| **2. Apps** | pgq, lagproxy, pgload, pglisten, minipool | Each has `--help` and a smoke test; pgload produces its benchmark table |
| **3a. Part I** | Chapters 0–6 | `book.py check`, `excerpts` and `links` pass, and chapter status is `current` |
| **3b. Part II** | Chapters 7–12 | The same gates; TLS examples pass against docker compose |
| **3c. Part III** | Chapters 13–18 and the appendix | The same gates; the feature map in Chapter 18 links resolve |
| **4. Polish** | The book PDF; a full read-through for voice, cross-references and solutions; a coverage audit against §5.3 | `book.py list` shows every chapter `current` and the book PDF fresh |

stockroom is built **before** the chapters, so that every "In stockroom" excerpt can point at
real code and the `excerpts` check has a stable target. If writing a chapter later shows that
stockroom should change, change stockroom first, then rerun `excerpts`.

---

## 10. Decisions taken (recommended defaults, easy to revisit)

| Decision | Choice | Alternatives |
|---|---|---|
| PostgreSQL version | **18** (local 18.6 on :5433), with version notes for features newer than 14 | a PG 14 baseline (rejected: end of life on 12 Nov 2026); verifying on both 14 and 18 |
| Side-by-side columns | psql + sqlx + tokio-postgres | psql + node-postgres; no comparison at all |
| Project domain | stockroom (retail click-and-collect) | event ticket booking; a bank ledger |
| Time crate | **jiff 0.2** (`Timestamp` ↔ `timestamptz` is exact). The type table shows chrono and time too. | chrono 0.4, the most common in existing code |
| Money | `Cents(i64)` → `bigint` with a hand-written/derived `ToSql`; `rust_decimal` only for `NUMERIC` aggregates | `NUMERIC` everywhere |
| Pool | deadpool-postgres, after a pool built by hand | bb8 |
| Migrations | hand-written (it teaches advisory locks and transactional DDL); refinery shown as an alternative | refinery throughout |
| Where examples live | Markdown is the source; `playground/examples/` is generated | code files as the source, with include markers |
| Indentation | rustfmt default (4 spaces) everywhere | — |
| Language | English | — |

## 11. Risks and items to verify while writing

- ~~**The `AsyncFnMut` retry helper over `Transaction<'_>`** (Chapter 8).~~ **Resolved.** It
  works in axum handlers when the closure owns its data (`async move`); capturing a reference
  to the caller's locals is what fails ("implementation of `Send` is not general enough").
  stockroom uses it; Chapter 8 shows the failing and the working capture. §12 #6 has the
  details.
- ~~**tower-http 0.7**~~ **Checked:** `TimeoutLayer::new` is deprecated in favour of
  `TimeoutLayer::with_status_code(status, duration)`, and `RequestBodyLimitLayer::new` is
  unchanged.
- **Pipelining and round-trip measurements** depend on lagproxy. Assert on relative numbers,
  never absolute ones, so the examples stay deterministic.
- **The head-of-line demonstration** (Chapter 9) relies on how the 0.7.18 internals behave
  (a bounded channel of 1). State that this is an implementation detail and give the version.
- **TLS examples** need docker. `check` skips them when `TLS_DATABASE_URL` is unset or nothing
  is listening on it, and says so in its report.
- **Version claims** must be checked on the 18.6 server before they go into a chapter:
  `ON_ERROR` and `REJECT_LIMIT`, `RETURNING old/new`, virtual generated columns, and `uuidv7()`.
- **Size.** About 19 chapters of 700–1,100 lines each. Write and verify one chapter at a time,
  never in batches without verification.

## 12. Findings from building the software (material for the chapters)

Each finding was hit for real while building stockroom on PG 18.6 and tokio-postgres 0.7.18,
and each one is pinned down by a test or a comment in the code. Each belongs in the chapter
named, as a runnable example where it can be one.

| # | Finding | Chapter |
|---|---|---|
| 1 | **A parameter that targets a domain column is described as the domain itself** (`sku` → `Kind::Domain(Text)`), so a plain `&str` or `i32` fails *on the client* with "error serializing parameter 0: cannot convert between the Rust type `&str` and the Postgres type `sku`". It works with a SQL cast (`$1::text`), with `query_typed` and `Type::TEXT`, with a derived domain newtype, or with a hand-written `accepts`. In `WHERE sku = $1` and `sku = ANY($1)` the parameter is inferred as `text` / `text[]`, and a plain `&str` works. | 5 |
| 2 | **A result column of a domain type is described as its base type** (`text`, `int4`). A derived domain `FromSql` accepts both. | 5 |
| 3 | **PG 18 virtual generated columns may not use user-defined types**, domains over `integer` included ("generation expression uses user-defined type"). | 5 (aside) |
| 4 | **`unnest($1::address[])` in `FROM` expands each composite into its fields**, which breaks `INSERT … SELECT * FROM unnest(…)`. In the select list, `unnest` keeps each value whole. | 5 |
| 5 | **A query future borrows its parameter slice.** A temporary `&[&id]` inside `try_join!`, or inside a `map` closure for `try_join_all`, does not live long enough. Fixes: bind the slice to a name (`let params: &[&(dyn ToSql + Sync)] = &[&id];`), or build it inside an `async move` block. | 7 |
| 6 | **An `AsyncFnMut(&Transaction<'_>)` helper works from axum handlers and spawned tasks, as long as the closure owns what it uses.** The first diagnosis ("async closures can't be `Send` here") was wrong. Isolating six variants on rustc 1.99 showed the real trigger: a closure whose captures contain a reference (to the caller's locals, and, as Chapter 8's example showed, even a `&'static str`) fails with "implementation of `Send` is not general enough"; `async move` with owned data (`String`, a request struct) works, with tokio-postgres's and deadpool's transactions alike. stockroom switched from a `BoxFuture`-plus-argument helper to the `AsyncFnMut` one. | 8 |
| 7 | **Two SERIALIZABLE transfers in opposite directions deadlock** when each updates "its" row first. PostgreSQL detects it only after `deadlock_timeout` (1 s), and retries then run out. Locking both rows first, in a fixed order, turns this into fast serialization failures that retry cleanly. The 40-transfer test went from failing in 9 s to passing 10/10 at about 0.7 s each. | 8 |
| 8 | **Guarded `UPDATE … WHERE available >= $3` at READ COMMITTED needs no retries**: the re-check after a lock wait (EvalPlanQual) sees the committed row, even through a virtual column. Sorting lines by product id prevents deadlocks between orders. Oversell test: 50 out of 50 runs. | 8 |
| 9 | **`MERGE` with a duplicated *new* key in its source fails with a unique violation**, not a cardinality violation (that error is for an *existing* row matched twice). Check for duplicates in the staging table first. | 10 |
| 10 | **CSV COPY writes NULL as an empty field and an empty string as `""`.** Export "no tags" as NULL (`nullif`) so that it reads back the same. | 10 |
| 11 | **COPY data errors (class 22) carry the line**, in `DbError::where_()`: "COPY product_import, line 3, column price_cents: …". `HEADER MATCH` (PG 15) gives "column name mismatch in header line field 2". | 6, 10 |
| 12 | **Cancelling an abandoned query has a race**: the cancel request is asynchronous, and the connection, back in the pool, may already be running someone else's query. `CancelOnDrop` therefore discards the connection (`Object::take`) after cancelling. Verified: a 1 s router timeout left no `pg_sleep` running. | 12, 16 |
| 13 | **A streaming body must own its pooled connection.** `RowStream` and `CopyOutStream` do not borrow the client; if the `Object` returns to the pool, the next borrower's queries queue behind the remaining rows. Added 2026-11-06 after review: when the client hangs up mid-stream, the dropped stream leaves the connection discarding the remaining rows, so stockroom's stream and CSV export hold it through `CancelOnDrop` (test `a_client_that_hangs_up_mid_stream_leaves_no_busy_connection`). | 9, 16 |
| 14 | **SSE streams must end at shutdown** (`take_until(shutdown.cancelled_owned())`), or `with_graceful_shutdown` waits forever. Verified: SIGTERM stopped the server in under 50 ms with an SSE client attached. | 16 |
| 15 | **`clone_on_copy`**: `NoTls` is `Copy` and rustls's connector is only `Clone`, so feature-generic code needs the clones and one crate-level `allow`. | 12 |
| 16 | **`LISTEN` reconnect needs a resync**: anything notified while disconnected is lost. The listener publishes `Resync` after each successful `LISTEN`. Tested with `pg_terminate_backend`. | 11 |
| 17 | **Per-test databases from a template** (`CREATE DATABASE … TEMPLATE`): cloning fails with 55006 while a session lingers on the template, so the harness retries. `Drop` cannot be async, so the cleanup runs on a thread with its own runtime. A full suite of 43 tests runs in about 6 s. | 17 |
| 18 | **`Config::host` and `Config::port` *append*** to the lists of hosts and ports to try, so "redirecting" a parsed configuration silently keeps the original host first. Build a new `Config`, or use `connect_raw` with a stream you opened (pgload). | 1, 12 |
| 19 | **`connect_raw` uses the stream as given.** `connect` sets `TCP_NODELAY` itself (`connect_socket.rs:27`); with `connect_raw` that is your job, along with keepalives and timeouts. | 12 |
| 20 | **Tokio sleeps are late by about 1.3 ms on macOS** (1 ms took 2.28 ms; 10 ms took 11.87 ms), so lagproxy's delay is a lower bound. Examples must compare timings with each other, not with constants. | 4, 7 |
| 21 | **Over latency, one pipelined client beats a pool** for independent queries: 64 tasks with a 2 ms delay ran 9,259 ops/s on one shared client against about 1,075 on a pool of 8, because the pool allows only 8 requests in flight. On localhost, pools are slightly faster thanks to server-side parallelism. Pools exist for transactions and isolation, not raw query throughput. | 7, 13 |
| 22 | **Bulk loading:** one `INSERT` at a time is about 100× slower than binary COPY on localhost, and about 200× with a 2 ms round trip. Pipelining recovers most of it, and `unnest` batches come within 10% of COPY. (Table in §5.2.) | 10 |
| 23 | **A pool that checks only `is_closed()` can hand out a connection that died while idle**, until its connection task has seen the server's FATAL message. A `check_connection()` on checkout costs a round trip and closes the gap. This is deadpool's `Fast` versus `Verified`. | 13 |
| 24 | **The client must be back on the idle stack before the semaphore permit is released**, or a waiter can open a connection beyond `max_size`. In minipool this holds whatever the field order, because `Conn`'s `Drop::drop` returns the client before any field (the permit included) is dropped; field order would matter only if a field's own `Drop` returned the client. (Corrected 2026-11-06: the first version said field order mattered.) | 13 |
| 25 | **A `NOTIFY` in a rolled-back transaction is never delivered**, and a committed one is delivered at commit. Quoted channel names are case-sensitive and match `pg_notify('Name', …)` exactly; unquoted `LISTEN Name` folds to lower case. | 11 |
| 26 | **`SimpleQueryMessage::CommandComplete` carries only a row count**, not the command tag: DDL reports 0, and there is no "CREATE TABLE" to print as psql does. | 2 |
| 27 | **Writing Part I corrected several "known" facts**, each now pinned by a runnable example or a captured output: `SimpleQueryMessage::RowDescription` arrived in 0.7.11, not 0.7.10; `now() - $1` infers `timestamptz`, not `interval`; `WHERE x IN ($1)` with an array prepares fine (`$1` is `text`) and fails on the client; a wrong-type panic message does not include the cause; psql 18 prints `\conninfo` as a table. | all |
| 28 | **A strict derived enum refuses every value once the database type gains a label**, on any connection that loads the type afresh. stockroom's `OrderStatus` now uses `allow_mismatch`, with a regression test (`tests/types.rs`). | 5 |
| 29 | **Types and domains can be created in `pg_temp`** and work with the derives (which match on the name, not the schema), so custom-type examples leave nothing behind. | 5 |
| 30 | **When a `Connection` is merely awaited, notices go to the `log` crate** at `info`; nothing shows them unless a logger is installed. | 6, 11 |
| 31 | **`DbError`'s `Display` is multi-line** (`ERROR: … DETAIL: … HINT: …`), and the `detail` of a unique violation contains the duplicate value: decide where it may go. | 6 |
| 32 | **`Statement::name()` is crate-private**; prepared statements are visible from SQL in `pg_prepared_statements` as `s0`, `s1` and so on (a global counter). | 4 |
| 33 | **lagproxy's measured round trips**: 15.3 ms at a 5 ms delay, 26 ms at 10 ms; text queries cost exactly twice a prepared or `query_typed` one. | 4 |
| 34 | **`drop(x)` on a value pinned with `pin!` drops only the `Pin<&mut _>`**; the value lives to the end of its block. An unfinished `RowStream` kept that way stalls the connection forever (the next query never returns); a `BinaryCopyInWriter` keeps its COPY open, and the next query waits behind it. Scope it in a block, or use `Box::pin`. Found twice, by a probe that hung and by a Chapter 10 example that timed out. | 9, 10 |
| 35 | **An unread `RowStream` stalls its connection, and dropping it does not stop the query.** The server waits in `ClientWrite` (seen in `pg_stat_activity`); after the drop, the connection reads and discards the rest: 3 million rows cost 181 ms, against 0.55 ms for a portal that fetched 10 rows and committed. A portal saves work only if the plan is lazy: `FROM generate_series(…)` materializes first (111 ms). A portal's last batch reports `rows_affected` for that batch only (4 of 7). psql 18's `FETCH_COUNT` declares no cursor. | 9 |
| 36 | **In 0.7.18, a `COPY … FROM STDIN` that the server rejects as it starts kills the connection** (missing table or column, `ON_ERROR ignore` with binary): `copy_in` sends Bind/Execute/Sync, the error arrives before CopyInResponse, the dropped sender then makes the connection send CopyFail + Sync, and the second ReadyForQuery is "unexpected message from server". Errors during the copy or from `finish` are fine. Upstream: rust-postgres issue #789 (open since 2021). stockroom is safe because it copies into a temp table created in the same transaction. | 10 |
| 37 | **Binary COPY trusts the declared types.** A Rust value that does not match its declared `Type` fails on the client ("error serializing parameter 0"); a declared type of the wrong size fails on the server (08P01 "insufficient data left in message", with the line); a wrong type of the *same* size loads silently wrong data (`int4` 5 into a `real` column stored 7e-45). | 10 |
| 38 | **COPY details:** `SinkExt::send` flushes per item, so each `send` is one CopyData message (the sink's 4 KiB gathering helps only with `feed`); `copy_out` yields one chunk per row; `ON_ERROR ignore` skips only data type errors (an extra column is fatal) and reports skips as notices; with a key duplicated in the source, `INSERT … ON CONFLICT DO UPDATE` always fails with 21000, while `MERGE` fails with 23505 for a new key and 21000 for an existing one (refines #9). | 10 |
| 39 | **NOTIFY, measured:** the payload must be *shorter* than 8,000 bytes (exactly 8,000 fails with 22023); identical notifications in one transaction collapse to the first, in first-sent order; a listener inside a transaction receives nothing until it ends; notices arrive during execution and notifications at commit, so in one batch the notices come first; `RAISE DEBUG` is not sent at the default `client_min_messages`. deadpool awaits its connections (notifications dropped) and `RecyclingMethod::Clean` runs `UNLISTEN *`. A *bounded* forwarder that fills stops the driver, and with it the client's own statements (Exercise 11.3 shows a batch that never completes). | 11 |
| 40 | **Timeouts and dead connections, measured:** an abandoned `pg_sleep(1)` stays `active` and delays the next query to ~1 s; `cancel_query` turns it into 57014 within ~5 ms; `idle_in_transaction_session_timeout` ends the session (the connection task gets the FATAL 25P03, the next query only "connection closed"); `transaction_timeout` (PG 17) reports FATAL 25P04 to the running statement, and 0.7.18's `SqlState` has no constant for 25P04; `lock_timeout` applies to advisory locks (55P03); `is_closed()` turned true 2.5 ms after `pg_terminate_backend`. `tcp_user_timeout` is applied on Linux only (`connect_socket.rs`). `target_session_attrs` runs `SHOW transaction_read_only` on each host (not `in_hot_standby`), and `connect_raw` skips it. | 12 |
| 41 | **TLS, measured against the docker server:** tokio-postgres-rustls 0.14 sets no ALPN, so `sslnegotiation=direct` (PG 17+) is closed by the server until `alpn_protocols = [b"postgresql"]` is added to the rustls `ClientConfig`; `channel_binding=require` works over rustls with the RSA/SHA-256 test certificate and fails over plaintext ("server did not use channel binding"); `NoTls` + `sslmode=require` asks the server first ("server does not support TLS" from a TLS-less server, "no TLS implementation configured" from one that offers TLS); `sslmode=prefer` with a TLS connector against a TLS-less server connects in plaintext without a word. Connector crates checked 2026-11-06: tokio-postgres-rustls 0.14.0, postgres-native-tls 0.5.3, postgres-openssl 0.5.3, and a fork, tokio-postgres-rustls-improved 0.17.1. Resolved for Appendix F: tokio-postgres-rustls **0.13.0 parsed the certificate wrongly and never did channel binding** (hidden by the default `prefer`); fixed upstream in 0.14.0 (PR #32), which is why `require` works here. refinery's `tokio-postgres-rustls` feature still pins ^0.13. postgres-native-tls and postgres-openssl have `set_postgresql_alpn()` for direct TLS. | 12, A |
| 42 | **deadpool, measured:** `Fast` *and* `Verified` hand the next borrower whatever session state was left (a `SET`, a temp table, a `LISTEN`); only `Clean` resets it. A connection terminated while idle was handed out by `Fast` and its first query failed on every run here; `Verified` gave a fresh one. `prepare_cached`: about 1 ms the first time, about 3 µs after. A cached `SELECT *` fails with 0A000 "cached plan must not change result type" after `ALTER TABLE … ADD COLUMN`, until `statement_caches.clear()`. The idle queue is FIFO by default (`QueueMode`); `build()` refuses timeouts without a runtime. deadpool-postgres 0.14.2 sits on deadpool **0.13.1**, and its `GenericClient` is **sealed and `#[async_trait]`** (CLAUDE.md had said native `async fn`; corrected from `generic_client.rs`). minipool re-run: one pipelined client 8× a pool of 8 at a 2 ms delay. | 13 |
| 43 | **Migrations and code generation, checked:** refinery 0.10 takes no lock (its tokio-postgres driver opens one transaction per migration and nothing else), so concurrent startups need an advisory lock around `run_async`. Clorinde merged back into **Cornucopia 1.0** (2026) and is no longer maintained. `CREATE INDEX CONCURRENTLY` fails with 25001 inside a transaction *and* inside a multi-statement `batch_execute` (one implicit transaction); on a temporary table PostgreSQL builds non-concurrently anyway. | 14 |
| 44 | **axum, measured:** a streaming body that does not own its `Object` made the next request (pool of 2) wait behind 200,000 unread rows; owning it fixed that. `TimeoutLayer` (200 ms) alone left the `pg_sleep` running; with a cancel-on-drop guard, nothing. `RequestBodyLimitLayer` answers 413 up front when `Content-Length` is over the limit. The outbox demo delivered `[1,2,3,4,5,4,5]` (at least once). An SSE body ends when its `take_until` token fires; `stockroom serve` exited 1.5 ms after SIGTERM with an SSE client attached, and the outbox delivered 16 ms after an order's creation. axum's own rejection statuses are 400 (syntax), 415 (content type) and 422 (shape). Handlers returning `Result<_, Response>` trip clippy's `result_large_err`: use a small error type. | 15, 16, 18 |
| 45 | **Testing, measured:** a deadlock victim's locks are released at the error, before any `ROLLBACK`, so the other transaction proceeds (§17.7 does not hang); a write skew under `SERIALIZABLE` is reproducible step by step on two connections (40001 at the second `UPDATE` or `COMMIT`). stockroom builds order lines from a `BTreeMap` keyed by SKU, so its lock order is fixed even without the `sort_by_key` (an exercise premise had to change). `cargo test -p stockroom`: 10 unit + 34 database tests, each binary under 2 s. | 17 |

