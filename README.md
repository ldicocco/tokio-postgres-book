# Async PostgreSQL in Rust

A course book on [`tokio-postgres`](https://docs.rs/tokio-postgres/latest/tokio_postgres/) for
developers who know Rust, a little Tokio and some SQL. It starts with the first query and ends
with **stockroom**, a production-shaped axum service that uses almost every feature of the
driver. Every runnable example in the book is compiled and run against PostgreSQL by
`tools/book.py check`.

| Where | What |
|---|---|
| [book/](book/) | The chapters, one Markdown file each, with their PDFs |
| [playground/](playground/) | The scratch crate the examples run in. `examples/` is generated from the book. |
| `stockroom/` | The capstone service (Phase 1) |
| `apps/` | Small example programs: pgq, lagproxy, pgload, pglisten, minipool (Phase 2) |
| [tools/](tools/) | `book.py`, which checks, extracts and renders, plus the PDF template |
| [PLAN.md](PLAN.md) | Scope, chapter outline, stockroom spec, feature coverage and phases |
| [CLAUDE.md](CLAUDE.md) | Conventions for working on the book and its code |

## Contents

<!-- book:contents:begin -->
| Chapter | You will learn | Status |
|---|---|---|
| [Chapter 0 — Introduction and setup](book/00-introduction.md) | Who the book is for, where tokio-postgres sits among Rust's Postgres libraries, the setup, and stockroom in one page | draft |
| Chapter 1 — Connecting | Client and Connection, spawning the connection task, connection strings and the Config builder, lifecycle and failures | planned |
| Chapter 2 — Running SQL: execute, query and parameters | batch_execute, execute, query and its one/opt/scalar variants, parameters and injection, RETURNING, the simple query protocol | planned |
| Chapter 3 — Rows and the type mapping | Reading rows, the Rust–Postgres type table, feature flags, borrowing from rows, NULL, JSON and arrays | planned |
| Chapter 4 — Prepared statements and the extended protocol | What goes over the wire, Statement and ToStatement, type inference, query_typed, statement caching and round trips | planned |
| Chapter 5 — Custom types: ToSql and FromSql | Implementing the conversion traits by hand, and deriving them for newtypes, enums, domains and composites | planned |
| Chapter 6 — Errors | tokio_postgres::Error, DbError and SqlState, designing an application error, and aborted transactions | planned |
| Chapter 7 — One connection, many tasks: pipelining | Lazy futures and request order, join! and try_join!, sharing a Client, head-of-line blocking and bounded concurrency | planned |
| Chapter 8 — Transactions and concurrency control | Transactions and the borrow checker, isolation levels, savepoints, GenericClient, retries, locks and the oversell problem | planned |
| Chapter 9 — Streaming rows and portals | query_raw and RowStream, backpressure on a shared connection, portals, cursors and keyset pagination | planned |
| Chapter 10 — Bulk data: COPY | COPY in and out in text, CSV and binary formats, failure mid-COPY, staging tables, and a bulk-load benchmark | planned |
| Chapter 11 — LISTEN/NOTIFY and notices | Driving the Connection with poll_message, notifications and notices, triggers, a dedicated listener and fan-out | planned |
| Chapter 12 — Production connections: timeouts, cancellation, health and TLS | Why dropping a future is not cancelling, CancelToken, server-side timeouts, dead connections, failover and TLS | planned |
| Chapter 13 — Connection pooling | Why one client is not enough, a pool from scratch, deadpool-postgres, sizing, and what not to do with pooled connections | planned |
| Chapter 14 — Schema, migrations and the data layer | Embedded migrations under an advisory lock, transactional DDL, and a repository layer over GenericClient | planned |
| Chapter 15 — axum I: a database-backed JSON API | App state and extractors, errors as responses, transactional writes, keyset pagination and ETags | planned |
| Chapter 16 — axum II: streaming, real-time and background work | NDJSON and CSV streaming, CSV import through COPY, server-sent events from NOTIFY, an outbox worker and graceful shutdown | planned |
| Chapter 17 — Testing against a real database | Per-test databases from a template, rollback tests and their limits, HTTP tests, and concurrency tests | planned |
| Chapter 18 — Capstone: stockroom end to end | The whole service: a request's life from HTTP to NOTIFY, the feature map, running it, and where to take it next | planned |
| Appendix — cheat sheets, phrasebook and glossary | The API and type-mapping cheat sheets, SqlState codes with HTTP mappings, a sqlx/diesel phrasebook and a glossary | planned |
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
