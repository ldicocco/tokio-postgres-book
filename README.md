# Async PostgreSQL in Rust

A course book on [`tokio-postgres`](https://docs.rs/tokio-postgres/latest/tokio_postgres/) for
developers who know Rust, a little Tokio and some SQL. It starts with the first query and ends
with **stockroom**, a production-shaped axum service that uses almost every feature of the
driver. Every runnable example in the book is compiled and run against PostgreSQL by
`tools/book.py check`.

| Where | What |
|---|---|
| [book/](book/) | The chapters, one Markdown file each, and the whole book as one PDF |
| [playground/](playground/) | The scratch crate the examples run in. `examples/` is generated from the book. |
| [stockroom/](stockroom/) | The capstone: a click-and-collect API (axum + tokio-postgres + deadpool) with migrations, a CLI (`migrate`, `seed`, `export`, `serve`), 43 tests, and a manual session in [stockroom.http](stockroom/stockroom.http) |
| [apps/](apps/) | Small example programs, each a library and a binary: [pgq](apps/pgq/) (a mini psql), [lagproxy](apps/lagproxy/) (adds network latency), [pgload](apps/pgload/) (five ways to bulk-load), [pglisten](apps/pglisten/) (a LISTEN tail that reconnects), [minipool](apps/minipool/) (a pool from scratch and a benchmark) |
| [tools/](tools/) | `book.py`, which checks, extracts and renders, plus the PDF template |
| [PLAN.md](PLAN.md) | Scope, chapter outline, stockroom spec, feature coverage and phases |
| [CLAUDE.md](CLAUDE.md) | Conventions for working on the book and its code |

## Contents

<!-- book:contents:begin -->
| Chapter | You will learn | Status |
|---|---|---|
| [Chapter 0 — Introduction and setup](book/00-introduction.md) | Who the book is for, where tokio-postgres sits among Rust's Postgres libraries, the setup, and stockroom in one page | current |
| [Chapter 1 — Connecting](book/01-connecting.md) | Client and Connection, spawning the connection task, connection strings and the Config builder, lifecycle and failures | current |
| [Chapter 2 — Running SQL: execute, query and parameters](book/02-queries.md) | batch_execute, execute, query and its one/opt/scalar variants, parameters and injection, RETURNING, the simple query protocol | current |
| [Chapter 3 — Rows and the type mapping](book/03-rows-and-types.md) | Reading rows, the Rust–Postgres type table, feature flags, borrowing from rows, NULL, JSON and arrays | current |
| [Chapter 4 — Prepared statements and the extended protocol](book/04-prepared-statements.md) | What goes over the wire, Statement and ToStatement, type inference, query_typed, statement caching and round trips | current |
| [Chapter 5 — Custom types: ToSql and FromSql](book/05-custom-types.md) | Implementing the conversion traits by hand, and deriving them for newtypes, enums, domains and composites | current |
| [Chapter 6 — Errors](book/06-errors.md) | tokio_postgres::Error, DbError and SqlState, designing an application error, and aborted transactions | current |
| [Chapter 7 — One connection, many tasks: pipelining](book/07-pipelining.md) | Lazy futures and request order, join! and try_join!, sharing a Client, head-of-line blocking and bounded concurrency | current |
| [Chapter 8 — Transactions and concurrency control](book/08-transactions.md) | Transactions and the borrow checker, isolation levels, savepoints, GenericClient, retries, locks and the oversell problem | current |
| [Chapter 9 — Streaming rows and portals](book/09-streaming.md) | query_raw and RowStream, backpressure on a shared connection, portals, cursors and keyset pagination | current |
| [Chapter 10 — Bulk data: COPY](book/10-copy.md) | COPY in and out in text, CSV and binary formats, failure mid-COPY, staging tables, and a bulk-load benchmark | current |
| [Chapter 11 — LISTEN/NOTIFY and notices](book/11-listen-notify.md) | Driving the Connection with poll_message, notifications and notices, triggers, a dedicated listener and fan-out | current |
| [Chapter 12 — Production connections: timeouts, cancellation, health and TLS](book/12-production-connections.md) | Why dropping a future is not cancelling, CancelToken, server-side timeouts, dead connections, failover and TLS | current |
| [Chapter 13 — Connection pooling](book/13-pooling.md) | Why one client is not enough, a pool from scratch, deadpool-postgres, sizing, and what not to do with pooled connections | current |
| [Chapter 14 — Schema, migrations and the data layer](book/14-data-layer.md) | Embedded migrations under an advisory lock, transactional DDL, and a repository layer over GenericClient | current |
| [Chapter 15 — axum I: a database-backed JSON API](book/15-axum-api.md) | App state and extractors, errors as responses, transactional writes, keyset pagination and ETags | current |
| [Chapter 16 — axum II: streaming, real-time and background work](book/16-axum-streaming.md) | NDJSON and CSV streaming, CSV import through COPY, server-sent events from NOTIFY, an outbox worker and graceful shutdown | current |
| [Chapter 17 — Testing against a real database](book/17-testing.md) | Per-test databases from a template, rollback tests and their limits, HTTP tests, and concurrency tests | current |
| [Chapter 18 — Capstone: stockroom end to end](book/18-capstone.md) | The whole service: a request's life from HTTP to NOTIFY, the feature map, running it, and where to take it next | current |
| [Appendix — cheat sheets, phrasebook, glossary and ecosystem](book/19-appendix.md) | The API and type-mapping cheat sheets, SqlState codes with HTTP mappings, a sqlx/diesel phrasebook, a glossary, and the crates built on tokio-postgres | draft |
<!-- book:contents:end -->

## Getting started

You need Rust (edition 2024), PostgreSQL 18 with `psql` (most examples run on 14 or newer; the book marks what needs a newer version), and Python 3.11
or newer for the tooling. The PDFs also need pandoc and Typst (`brew install pandoc typst`).

```bash
cp .env.example .env          # then fill in your local credentials
psql "$ADMIN_DATABASE_URL" -c 'CREATE DATABASE tpbook'

python3 tools/book.py list    # chapter status
python3 tools/book.py check   # verify everything; run every example against a fresh database
```

The optional `docker compose up -d` starts PostgreSQL 18 with TLS on port 5434, for the TLS
examples in Chapter 12. Run `docker/gen-certs.sh` once before the first start.

## PDFs

The repository holds one PDF: the whole book, [book/tokio-postgres-book.pdf](book/tokio-postgres-book.pdf),
with a cover, a linked table of contents and every chapter whose status is `current`. After
changing a chapter, rebuild it (`python3 tools/book.py check` warns when it has fallen behind):

```bash
python3 tools/book.py book
```

Any chapter can also be rendered on its own, for printing or reading offline. Chapter PDFs are
not kept in the repository; they are written to `target/book/`, which git ignores:

```bash
python3 tools/book.py pdf 3                    # by number: target/book/03-rows-and-types.pdf
python3 tools/book.py pdf rows-and-types       # or by id, as in book/book.toml
python3 tools/book.py pdf 3 4 5                # several at once
python3 tools/book.py pdf --all                # every chapter written so far
```

Both commands need [pandoc](https://pandoc.org) 3.x and [Typst](https://typst.app) 0.13 or newer
(`brew install pandoc typst`). They use the same template as the Rust course this book's style follows,
in [tools/pdf/](tools/pdf/), and need no network.
