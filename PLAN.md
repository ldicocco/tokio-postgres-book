# Plan — *Async PostgreSQL in Rust*: a course on tokio-postgres

This plan is for a course book on [`tokio-postgres`](https://docs.rs/tokio-postgres/latest/tokio_postgres/).
The course goes from the first query to a production-shaped HTTP service built with axum.
Its form and voice follow the Rust course in
another repository ("the reference course").

Status: **Phase 0 (scaffold) done on 2026-11-06. Next is Phase 1 (stockroom).** Phases and
acceptance criteria are in §9.

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
- **Per-chapter PDFs plus one book PDF**, rendered with the same pandoc → Typst pipeline,
  copied from the reference course.

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
│   └── *.pdf, *.pdf.source-sha256, tokio-postgres-book.pdf
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

#### Schema (PostgreSQL 18)

```sql
CREATE TYPE order_status AS ENUM ('pending', 'paid', 'ready', 'collected', 'cancelled');
CREATE TYPE address      AS (street text, city text, postcode text, country char(2));
CREATE DOMAIN sku        AS text    CHECK (VALUE ~ '^[A-Z0-9]{3,12}(-[A-Z0-9]{1,6})?$');
CREATE DOMAIN quantity   AS integer CHECK (VALUE >= 0);

stores          (id uuid PK, code text UNIQUE, name text, address address, created_at timestamptz)
products        (id uuid PK, sku sku UNIQUE, name text, price_cents bigint CHECK (> 0),
                 tags text[], attributes jsonb, version int, created_at, updated_at)
stock           (store_id, product_id, on_hand quantity, reserved quantity,
                 CHECK (reserved <= on_hand), PK (store_id, product_id))
orders          (id uuid PK, store_id, status order_status, customer jsonb, total_cents bigint,
                 version int, created_at, updated_at)
order_lines     (order_id, product_id, quantity int CHECK (> 0), unit_price_cents bigint)
stock_movements (id bigserial, store_id, product_id, delta int, reason text, at timestamptz)
outbox          (id bigserial, topic text, payload jsonb, attempts int, available_at, done_at)
-- trigger: an orders status change → pg_notify('order_events', json) + an outbox row
```

stockroom uses modern PostgreSQL where it fits. The book notes the version each feature needs
the first time it appears:

- `DEFAULT uuidv7()` for every UUID key (PG 18). The `uuid` crate maps the keys to `Uuid`.
- `RETURNING old.status, new.status` in order transitions (PG 18).
- A virtual generated column, `stock.available = on_hand - reserved` (PG 18).
- `MERGE` for the import upsert (PG 15). The book compares it with `INSERT … ON CONFLICT`.

#### HTTP API (axum 0.8)

| Method and path | What it shows |
|---|---|
| `GET /healthz`, `GET /readyz` | `check_connection`, pool status, `simple_query("SELECT 1")` |
| `GET /stores`, `POST /stores` | A **composite** type (`address`) through a derive |
| `GET /products?tag=&cursor=&limit=` | **Keyset pagination**, `= ANY($1)` with an array parameter, `prepare_cached` |
| `GET /products/{sku}`, `POST /products` | A **domain** type (`sku`), `UNIQUE_VIOLATION` → 409, `CHECK_VIOLATION` → 422 |
| `PATCH /products/{sku}` with `If-Match` | **Optimistic concurrency**: a version column, and 412 on a mismatch |
| `GET /products/stream` | `query_raw` → `RowStream` → an NDJSON response body, with backpressure |
| `GET /products/export.csv` | `copy_out` → a streamed CSV response |
| `POST /products/import` (CSV body) | The request body stream → `copy_in` into a temp staging table → upsert in one transaction |
| `GET /stores/{code}/stock`, `POST /stores/{code}/stock/adjust` | A transaction plus a ledger row |
| `POST /orders` | **Reserve stock**: a `SERIALIZABLE` transaction with a retry on 40001/40P01, savepoints, `GenericClient` repository functions |
| `GET /orders/{id}`, `POST /orders/{id}/transition` | A **Postgres enum** ↔ a Rust enum state machine, `SELECT … FOR UPDATE` |
| `GET /orders/events` | **SSE**: a dedicated `LISTEN` connection driven with `poll_message` → `broadcast` → `Sse` |
| `GET /reports/dashboard` | **Pipelining**: five aggregates with `try_join!` on one connection |
| `GET /reports/sales?from=&to=` | A heavy query with a deadline: `statement_timeout`, plus a `CancelToken` when the client disconnects |
| `GET /admin/sessions` | `simple_query` over `pg_stat_activity`, and `Connection::parameter` |

#### Command line (`stockroom <subcommand>`)

| Subcommand | What it shows |
|---|---|
| `migrate` | Embedded migrations, `pg_advisory_lock`, `batch_execute`, logging server **notices** through `poll_message` |
| `seed --products N` | **Binary COPY** with `BinaryCopyInWriter` for bulk data |
| `export --batch N` | A `REPEATABLE READ, READ ONLY, DEFERRABLE` transaction with a **portal** (`bind` + `query_portal`), and `BinaryCopyOutStream` |
| `serve` | Everything above, plus the outbox worker (`FOR UPDATE SKIP LOCKED`, woken by `NOTIFY`) and graceful shutdown |

**Cross-cutting concerns:**

- **Errors.** `AppError` uses `thiserror` and maps `SqlState` to an HTTP response, and never
  leaks `DbError` text to clients.
- **Logging.** `tracing` spans record the duration of each SQL statement.
- **Configuration.** Settings come from environment variables, and `Config` is built from
  `DATABASE_URL` plus `application_name`, `connect_timeout`, `keepalives` and `options`
  (`-c statement_timeout=…`).
- **TLS.** An optional cargo feature `tls` uses `tokio-postgres-rustls`.
- **Tests.** Each test gets its own database, cloned from a migrated template. HTTP tests use
  `tower::ServiceExt::oneshot`. A concurrency test reserves stock from 50 tasks at once and
  proves that no order oversells.

### 5.2 Example programs (`apps/`)

These are small, single-purpose tools. Each one is the "real program" behind one or two chapters.

| App | Chapter(s) | Features |
|---|---|---|
| **pgq** | 2, 4 | A mini psql. It runs SQL with `simple_query` (several statements, `RowDescription`, `CommandComplete`), prints aligned tables, and has an `--describe` mode that uses `prepare` to show parameter and column types and `\timing`. |
| **lagproxy** | 4, 7, 13 | A TCP proxy (about 60 lines of Tokio) that delays every packet by N ms. With it, round trips, pipelining and pool sizing show up in measurements on localhost. It is also used by `connect_raw` in Chapter 12. |
| **pgload** | 10 | Loads a CSV into a table using one of five strategies: row-by-row `INSERT`, pipelined prepared `INSERT`, `UNNEST` arrays, text `COPY`, and binary `COPY`. It prints a timing table that Chapter 10 reproduces. |
| **pglisten** | 11, 12 | `LISTEN` on N channels, printing notifications and notices. It reconnects with backoff and runs a catch-up query after reconnecting. |
| **minipool** | 13 | A bounded pool built from a `Semaphore` and a `Mutex<Vec<Client>>`, with a guard that returns its connection on `Drop` and a health check on checkout. It includes a benchmark against deadpool-postgres. |

### 5.3 Feature coverage matrix

The rule is that every public item of `tokio-postgres` 0.7.18 gets at least one runnable
example in the book, and the important ones also appear in stockroom. Chapter numbers refer to
§7.

| API item | Ch | stockroom / app |
|---|---|---|
| `connect`, `NoTls`, `Client`/`Connection` split, spawning the connection | 1 | `db/connect.rs` |
| `Config` (`FromStr`, key/value and URL forms, builder setters) | 1, 12 | `config.rs` |
| `Connection::parameter` | 1 | `/admin/sessions` |
| `Client::is_closed`, `check_connection` | 1, 12 | `/healthz`, pglisten |
| `batch_execute` | 2 | `migrate` |
| `execute`, `query`, `query_one`, `query_opt` | 2 | the repository layer |
| `query_scalar`, `query_one_scalar`, `query_opt_scalar` | 2 | the repository layer |
| `simple_query`, `SimpleQueryMessage` (`Row`, `CommandComplete`, `RowDescription`), `SimpleQueryRow`, `SimpleColumn` | 2 | pgq, `/admin/sessions` |
| `simple_query_raw`, `SimpleQueryStream` | 9 | pgq (streaming mode) |
| `Row` (`get`, `try_get`, `columns`, `len`, `is_empty`, `raw_size_bytes`), `Column` | 3 | models |
| `prepare`, `Statement` (`params`, `columns`), `ToStatement` | 4 | pgq `--describe` |
| `prepare_typed` | 4 | — |
| `query_typed`, `query_typed_one`, `query_typed_opt`, `query_typed_raw`, `execute_typed` | 4 | the repository layer (PgBouncer-safe paths) |
| `clear_type_cache` | 5 | `migrate` (after `ALTER TYPE`) |
| `types::{ToSql, FromSql, FromSqlOwned, IsNull, Type, Kind, Field, WrongType, WasNull, accepts!, to_sql_checked!}` | 3, 5 | `domain/*.rs` |
| `#[derive(ToSql, FromSql)]`: enum, domain, composite, `transparent`, `rename_all`, `allow_mismatch` | 5 | `OrderStatus`, `Address`, `Sku`, `Cents` |
| `types::Json<T>`, the `with-serde_json-1` feature | 3, 5 | `products.attributes`, `orders.customer` |
| Feature flags `with-uuid-1`, `with-jiff-0_2`, `array-impls` (others shown in a table) | 3 | the whole project |
| `error::{Error, DbError, SqlState, Severity, ErrorPosition}`, `Error::{code, as_db_error, is_closed, into_source}` | 6 | `error.rs` |
| Pipelining (`join!`, `try_join!`, `buffer_unordered`), `Arc<Client>` | 7 | `/reports/dashboard` |
| `transaction`, `Transaction::{commit, rollback}`, rollback on drop | 8 | `POST /orders` |
| `build_transaction`, `TransactionBuilder::{isolation_level, read_only, deferrable, start}`, `IsolationLevel` | 8 | `place_order`, `export` |
| `Transaction::{transaction, savepoint}` (nested transactions) | 8 | `place_order` (partial failure) |
| `GenericClient` | 8, 14 | `repo/*.rs` |
| `query_raw`, `RowStream` (`rows_affected`), `BorrowToSql`, `execute_raw` | 9 | `/products/stream` |
| `Transaction::{bind, bind_raw, query_portal, query_portal_raw}`, `Portal` | 9 | `export` |
| `copy_in`, `CopyInSink` (`finish`), `copy_out`, `CopyOutStream` | 10 | import and export endpoints, pgload |
| `binary_copy::{BinaryCopyInWriter, BinaryCopyOutStream, BinaryCopyOutRow}` | 10 | `seed`, `export`, pgload |
| `Connection::poll_message`, `AsyncMessage::{Notification, Notice}`, `Notification` | 11 | the SSE listener, `migrate` notices, pglisten |
| `cancel_token`, `CancelToken::{cancel_query, cancel_query_raw}` | 12 | `/reports/sales`, the cancel-on-drop guard |
| `Config::{connect_timeout, keepalives*, tcp_user_timeout, target_session_attrs, load_balance_hosts, hostaddr, host_path, options, channel_binding, ssl_mode, ssl_negotiation}` | 12 | `config.rs` |
| `tls::{MakeTlsConnect, TlsConnect}`, `postgres-native-tls`, `postgres-openssl`, `tokio-postgres-rustls` | 12 | the `tls` cargo feature |
| `Config::connect_raw`, `Socket`, the `runtime` feature | 12 | lagproxy |
| deadpool-postgres (`Manager`, `RecyclingMethod`, `prepare_cached`, `Object`), bb8 for comparison | 13 | `db/pool.rs`, minipool |

The deprecated `Client::cancel_query` is mentioned only to say that `cancel_token` replaces it.
The `js` feature, which targets wasm, is not covered.

---

## 6. Tooling and verification

The reference course guarantees that every ` ```rust ` block compiles and runs. This book keeps
that guarantee against a real database.

**`tools/book.py`** (Python 3, no third-party packages; the PDF commands need pandoc and Typst):

| Command | What it does |
|---|---|
| `list` | Every chapter with its status (`planned`/`draft`/`current`), TODO count and PDF freshness |
| `extract` | Writes each runnable block to `playground/examples/chNN_KK.rs`, with a header naming the source file and line and marking the file as generated |
| `check [--chapter N] [--no-run]` | Runs `extract`, recreates the database `tpbook_check`, runs `cargo build`, `clippy -Dwarnings` and `fmt --check` on the examples, then runs each example with a timeout and `DATABASE_URL` pointing at that database. Prints a pass/fail table. |
| `links` | Checks that every relative link resolves |
| `excerpts` | Checks every ` ~~~rust ` excerpt whose first line names a file: the file must exist, and every line that is not an ellipsis must appear in the file. This catches drift between the book and stockroom. |
| `pdf [id\|--stale\|--all]`, `book` | Renders per-chapter PDFs and the full book PDF with a provenance stamp, using the reference course's pandoc → Typst pipeline |

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
  diesel/diesel-async and SeaORM. Why learning it pays off: deadpool, refinery and clorinde all
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
- 14.6 Generating code from SQL with clorinde, in brief.
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
- F. Further reading.

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
| **4. Polish** | Per-chapter PDFs and the book PDF; a full read-through for voice, cross-references and solutions; a coverage audit against §5.3 | `book.py list` shows every chapter `current` with a fresh PDF |

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

- **The `AsyncFnMut` retry helper over `Transaction<'_>`** (Chapter 8). Verify that it compiles
  on 1.99. If it does not, fall back to boxed futures with `for<'a> FnMut(&'a mut Transaction)
  -> BoxFuture<'a, …>` and explain why.
- **tower-http 0.7** may have changed `TimeoutLayer` and `RequestBodyLimitLayer`. Check against
  docs.rs.
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
