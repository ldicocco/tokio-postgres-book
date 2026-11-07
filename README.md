# Async PostgreSQL in Rust

**A free course book on [tokio-postgres](https://docs.rs/tokio-postgres/0.7.18/tokio_postgres/):
from the first query to a production-shaped [axum](https://docs.rs/axum/0.8.9/axum/) service.
Every one of its 156 runnable examples is compiled, linted and run against PostgreSQL 18 by the
book's own tooling.**

[![Code: MIT OR Apache-2.0](https://img.shields.io/badge/code-MIT%20OR%20Apache--2.0-blue)](#license)
[![Text: CC BY 4.0](https://img.shields.io/badge/text-CC%20BY%204.0-lightgrey)](#license)
[![tokio-postgres 0.7.18](https://img.shields.io/badge/tokio--postgres-0.7.18-orange)](https://docs.rs/tokio-postgres/0.7.18/tokio_postgres/)
[![PostgreSQL 18](https://img.shields.io/badge/PostgreSQL-18-336791)](https://www.postgresql.org/docs/18/)
[![Rust edition 2024](https://img.shields.io/badge/Rust-edition%202024-b7410e)](https://doc.rust-lang.org/edition-guide/rust-2024/)

**Read it:** [the whole book as one PDF](book/tokio-postgres-book.pdf) (270 pages) ·
[start with Chapter 0](book/00-introduction.md) · [see the contents](#contents)

## Who it is for

Developers who know Rust reasonably well, have used Tokio a little, and can write SQL. You do not
need to know the PostgreSQL wire protocol or any other database library; the book compares
tokio-postgres with psql and sqlx as it goes, so it also works as a guide for readers coming from
those.

## What you will learn

- **How the driver works:** the `Client`/`Connection` split, the simple and extended protocols,
  prepared statements, and what each round trip costs, measured through a latency proxy.
- **Pipelining:** many independent queries on one connection for about the price of one round
  trip, and its limits (head-of-line blocking, transactions).
- **Types:** the full Rust–PostgreSQL mapping, `ToSql`/`FromSql` by hand, and derives for enums,
  domains and composite types.
- **Errors as data:** `DbError`, `SqlState`, constraint names, and turning them into HTTP statuses.
- **Transactions and concurrency:** isolation levels, savepoints, retrying serialization failures
  and deadlocks, `SKIP LOCKED`, and an overselling test with fifty concurrent orders.
- **Large results and bulk data:** `RowStream` and backpressure, portals, keyset pagination, and
  `COPY` in text, CSV and binary (about 100 times faster than one `INSERT` per row, measured).
- **Real time:** `LISTEN`/`NOTIFY`, notices, triggers, and fan-out to server-sent events.
- **Production connections:** cancellation, server-side timeouts, dead connections, failover,
  and TLS with rustls, including direct TLS negotiation and channel binding.
- **Building the service:** pooling with deadpool-postgres, migrations, a repository layer,
  axum handlers, streaming responses, an outbox worker, graceful shutdown, and testing against a
  real database.

## What you will build

**[stockroom](stockroom/)**, the capstone: a click-and-collect API for a small retail chain,
built with axum, tokio-postgres and deadpool-postgres. It uses almost every feature of the driver:
pipelined reports, retried transactions with row locks in a fixed order, keyset pagination,
ETags with optimistic concurrency, NDJSON and CSV streaming, CSV import through `COPY`,
server-sent events fed by `LISTEN`, a transactional outbox worker, cancellation of abandoned
queries, and TLS. It comes with migrations, a CLI (`migrate`, `seed`, `export`, `serve`), 45
tests that each run on a fresh database cloned from a template, and a manual session in
[stockroom.http](stockroom/stockroom.http).

Five smaller programs in [apps/](apps/) each back a chapter or two:
[pgq](apps/pgq/) (a tiny psql), [lagproxy](apps/lagproxy/) (adds network latency, so round trips
show up on localhost), [pgload](apps/pgload/) (five ways to bulk-load, timed),
[pglisten](apps/pglisten/) (a `LISTEN` tail that reconnects and catches up) and
[minipool](apps/minipool/) (a connection pool from scratch, benchmarked against deadpool).

## Things this book found out the hard way

Every claim in the book is backed by a runnable example or a link to the documentation, and
writing it turned up behaviour that is easy to get wrong. A few, each shown in the chapter named:

- In tokio-postgres 0.7.18, a `COPY … FROM STDIN` that the server rejects as it starts (a missing
  table, say) breaks the connection: the next query fails with "unexpected message from server"
  ([Chapter 10](book/10-copy.md)).
- Dropping a query future (a timeout, a `select!`, a client that hung up) does not stop the query
  on the server, and the next request on that connection waits for it
  ([Chapter 12](book/12-production-connections.md)).
- A `RowStream` read slowly stalls its whole connection, and `drop()` on a stream pinned with
  `pin!` does not drop it ([Chapter 9](book/09-streaming.md)).
- An async closure passed to a retry helper fails to compile in an axum handler with
  "implementation of `Send` is not general enough" when it captures a reference, even a
  `&'static str` ([Chapter 8](book/08-transactions.md)).
- deadpool's default recycling can hand out a connection that died while idle, and keeps whatever
  session state the last borrower left; a cached statement fails with "cached plan must not
  change result type" after a schema change ([Chapter 13](book/13-pooling.md)).
- In binary `COPY`, a wrong declared type of the same size loads wrong data without an error
  ([Chapter 10](book/10-copy.md)).

## Coming next: building an ORM

A second book is under way in [orm/](orm/README.md), on the branch `shelf`. In
*Building an ORM on tokio-postgres* you build **shelf**, a small ORM: derive macros that map
rows to structs, then a model layer, a typed query builder and relations, all on the driver this
book teaches. Its plan is in [orm/PLAN.md](orm/PLAN.md).

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
| [Appendix — cheat sheets, phrasebook, glossary and ecosystem](book/19-appendix.md) | The API and type-mapping cheat sheets, SqlState codes with HTTP mappings, a sqlx/diesel phrasebook, a glossary, and the crates built on tokio-postgres | current |
<!-- book:contents:end -->

## Getting started

You need Rust 1.88 or newer (edition 2024; tested with 1.99), PostgreSQL 18 with `psql` (most
examples run on 14 or newer, and the book marks what needs a newer version), and Python 3.11 or
newer for the tooling.

```bash
cp .env.example .env          # then fill in your local credentials
psql "$ADMIN_DATABASE_URL" -c 'CREATE DATABASE tpbook'

python3 tools/book.py list    # chapter status
python3 tools/book.py check   # verify everything; run every example against a fresh database
cargo test --workspace        # stockroom's and the apps' tests
```

To follow along, create the playground crate as [Chapter 0](book/00-introduction.md) describes
and paste examples into it. The optional `docker compose up -d` starts PostgreSQL 18 with TLS on
port 5434, for the TLS examples in Chapter 12; run `docker/gen-certs.sh` once before the first
start.

## How the book is checked

`python3 tools/book.py check` treats the book as a test suite. It:

- extracts every ```` ```rust ```` block into [playground/examples/](playground/) and builds it
  with clippy (`-D warnings`) and rustfmt;
- runs each one against a freshly created database, where its `assert!`s state what it produces
  (TLS examples run against the `docker compose` server, latency examples through lagproxy);
- compares every excerpt of stockroom and the apps with the file it quotes, and every printed
  helper with the playground's source;
- checks every link and `#anchor`, and that the playground's dependency versions match the
  workspace.

Program output shown in the book (psql sessions, error messages, timings) was captured from real
runs, and measured numbers say what they were measured on. Tested with tokio-postgres 0.7.18,
postgres-types 0.2.14, deadpool-postgres 0.14.2, axum 0.8.9 and PostgreSQL 18.6.

## Repository layout

| Where | What |
|---|---|
| [book/](book/) | The chapters, one Markdown file each, and the whole book as one PDF |
| [playground/](playground/) | The scratch crate the examples run in. `examples/` is generated from the book. |
| [stockroom/](stockroom/) | The capstone service, its migrations, CLI and tests |
| [apps/](apps/) | The five small example programs, each a library and a binary |
| [tools/](tools/) | `book.py`, which checks, extracts and renders, plus the PDF template |
| [orm/](orm/README.md) | The child project, in progress: shelf, a small ORM, and the book that builds it |
| [docker/](docker/), [docker-compose.yml](docker-compose.yml) | The optional PostgreSQL 18 server with TLS |
| [PLAN.md](PLAN.md) | Scope, chapter outline, stockroom spec, feature coverage, and what building it taught |
| [CLAUDE.md](CLAUDE.md) | Conventions for working on the book and its code |

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
(`brew install pandoc typst`). The Typst template is in [tools/pdf/](tools/pdf/), and rendering
needs no network. Only openly licensed fonts are used (Libertinus Serif and DejaVu Sans Mono ship
with Typst; Libertinus Sans is in tools/pdf/fonts/), so every machine renders the same PDF.

## Contributing

Corrections are welcome as issues or pull requests. The most useful report of a mistake is a
runnable example that shows the actual behaviour; `python3 tools/book.py check --chapter N`
verifies a chapter, and [CLAUDE.md](CLAUDE.md) describes the conventions the book follows.

## License

The code (stockroom, the apps, the playground and the tooling) is licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE)), or
- MIT license ([LICENSE-MIT](LICENSE-MIT)),

at your option. The text of the book (the Markdown files in [book/](book/) and the PDF built from
them) is licensed under the Creative Commons Attribution 4.0 International License
([LICENSE-CC-BY-4.0](LICENSE-CC-BY-4.0)). The Libertinus Sans font files in
[tools/pdf/fonts/](tools/pdf/fonts/), which the PDF embeds, are under the SIL Open Font License
([OFL.txt](tools/pdf/fonts/OFL.txt)).

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in
the work by you, as defined in the Apache-2.0 license, shall be licensed as above, without any
additional terms or conditions.
