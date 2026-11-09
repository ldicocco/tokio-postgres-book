# Chapter 0 — Introduction and setup

> **Async PostgreSQL in Rust** · Next: [Chapter 1 — Connecting](01-connecting.md)

This book teaches [`tokio-postgres`](https://docs.rs/tokio-postgres/latest/tokio_postgres/),
the asynchronous PostgreSQL driver that most of Rust's database ecosystem stands on. It takes
you from the first query to a production-shaped HTTP service built with axum, and at every step
it shows what the driver sends over the wire, what the server does with it, and where the sharp
edges are.

Every runnable example was compiled and run against PostgreSQL 18 when the book was written,
and the service the book builds, **stockroom**, is in this repository with its tests. You learn
the driver and a real codebase at the same time.

## Contents

| Chapter | You will learn |
|---|---|
| **0 · Introduction and setup** (this page) | Who the book is for, where tokio-postgres fits, the setup, and stockroom in one page |
| [1 · Connecting](01-connecting.md) | `Client` and `Connection`, spawning the connection, connection strings and `Config`, what failure looks like |
| [2 · Running SQL](02-queries.md) | `execute`, `query` and friends, parameters instead of string formatting, the simple query protocol |
| [3 · Rows and types](03-rows-and-types.md) | Reading rows, the Rust–PostgreSQL type table, NULL, JSON, arrays, borrowing from a row |
| [4 · Prepared statements](04-prepared-statements.md) | What goes over the wire, `Statement`, type inference, `query_typed`, round trips |
| [5 · Custom types](05-custom-types.md) | `ToSql` and `FromSql` by hand and by derive: newtypes, enums, domains, composites |
| [6 · Errors](06-errors.md) | `Error`, `DbError` and `SqlState`, and an error type for an application |
| [7 · Pipelining](07-pipelining.md) | Many queries on one connection at once, and when that beats a pool |
| [8 · Transactions](08-transactions.md) | Isolation levels, savepoints, retries, locks, and selling the last chair exactly once |
| [9 · Streaming](09-streaming.md) | Rows as a stream, backpressure, portals, keyset pagination |
| [10 · COPY](10-copy.md) | Bulk data in and out, text and binary, and how much faster it is |
| [11 · LISTEN/NOTIFY](11-listen-notify.md) | Driving the connection yourself, notifications, notices, reconnecting |
| [12 · Production connections](12-production-connections.md) | Timeouts, cancellation, dead connections, failover and TLS |
| [13 · Pooling](13-pooling.md) | Why one client is not enough, a pool from scratch, deadpool-postgres |
| [14 · The data layer](14-data-layer.md) | Migrations, transactional DDL, a repository layer |
| [15 · axum I](15-axum-api.md) | A JSON API: extractors, errors as responses, ETags, pagination |
| [16 · axum II](16-axum-streaming.md) | Streaming bodies, COPY over HTTP, server-sent events, an outbox worker, shutdown |
| [17 · Testing](17-testing.md) | A database per test, HTTP tests, concurrency tests |
| [18 · Capstone](18-capstone.md) | stockroom end to end, and a map from every feature to its code |
| [A · Appendix](19-appendix.md) | Cheat sheets, a sqlx and diesel phrasebook, a glossary, and the crates built on tokio-postgres |

The whole book is also available as a single PDF, [tokio-postgres-book.pdf](tokio-postgres-book.pdf),
with a linked table of contents. To print one chapter, render it yourself from the repository's
root: `python3 tools/book.py pdf 3` writes `target/book/03-rows-and-types.pdf` (it needs pandoc
and Typst; the README's *PDFs* section has the details).

Chapters 1–6 are the driver's foundations. Chapters 7–12 are what makes it an *async*
driver, and where most production problems live. Chapters 13–18 build the service. Read them
in order the first time; afterwards the appendix is the fastest way back in.

---

## Who this is for, and what it assumes

You know three things well enough to be dangerous:

- **Rust, at an average level.** Ownership and borrowing, structs and enums with `match`,
  traits and generics, `Result` and `?`, closures, iterators. You have probably not had to
  think hard about `Pin`, higher-ranked lifetimes or why a future is not `Send`. The book
  explains each of those where it first matters.
- **Tokio, at a basic level.** You have written `#[tokio::main]`, `.await`ed things,
  called `tokio::spawn`, and seen a channel. `select!`, cancellation and streams are introduced
  as they are needed.
- **SQL, at a basic level.** `SELECT`, `INSERT`, `UPDATE`, `DELETE`, joins, `CREATE TABLE`,
  and the idea of a transaction. Isolation levels, row locks, `COPY`, `LISTEN`/`NOTIFY`,
  domains and composite types are explained when they come up.

You do not need to know anything about how a database driver works. By the end you will know
what yours does on every call.

## Where tokio-postgres fits

Rust has several ways to talk to PostgreSQL, and they are layered more than they compete:

| Library | What it is | Built on |
|---|---|---|
| **tokio-postgres** | An asynchronous driver: connections, queries, transactions, COPY, notifications, type conversions. No query builder, no ORM, no compile-time checking. | Tokio and the PostgreSQL wire protocol |
| `postgres` | The same driver with a blocking API, for programs without an async runtime. | tokio-postgres |
| deadpool-postgres, bb8-postgres | Connection pools. | tokio-postgres |
| refinery | Schema migrations. | tokio-postgres (among others) |
| clorinde (formerly cornucopia) | Generates typed Rust functions from `.sql` files. | tokio-postgres |
| diesel-async (PostgreSQL backend) | Diesel's query builder, asynchronously. | tokio-postgres |
| sqlx | An independent async driver with compile-time-checked queries. | its own protocol implementation |
| SeaORM | An ORM. | sqlx |

Learning tokio-postgres pays off twice. It is a good driver to use directly: small, close to
the protocol, and fast, with features higher layers hide, such as running many queries on one
connection at once (Chapter 7). And whatever you use on top of it, it is what actually runs
when something goes wrong in production, so knowing it is knowing what your stack does.

---

## How each chapter is built

1. **The concept**, explained from scratch, with small runnable programs.
2. **Side by side** sections. Each one states a single task and solves it three ways: at the
   `psql` prompt, with sqlx, and with tokio-postgres. A table of what is the **same** and what
   is **different** follows. `psql` ties the Rust code to SQL you already know and shows what
   the server is asked to do; sqlx shows which choices are specific to tokio-postgres. The sqlx
   snippets are illustrations and are not compiled; the tokio-postgres ones are complete and
   were run. In Part III, where psql has nothing to compare with, the sections drop it.
3. **In stockroom**: where the concept lives in the service this book builds, with a trimmed
   excerpt and a link to the file.
4. **Exercises**, each with a hint, and solutions for the ones whose answers are not obvious.

Four kinds of boxes appear where they are earned:

> **Protocol lens:** what travels over the socket. Most of tokio-postgres's API makes sense the
> moment you know what it sends.

> **SQL lens:** what the server does: isolation, locks, how notifications are delivered.

> **Rust refresher:** a language feature you may be hazy on, at the point where it matters.

> **Pitfall:** a mistake that compiles, passes a quick test, and fails in production.

Code blocks come in two kinds:

- Blocks marked as plain `rust` are **complete programs**. Each one compiles and runs as
  written in the playground described below, and each was run against a fresh database when
  the book was written. Their `assert!`s are part of the explanation: they state what the code
  produces.
- Blocks taken from stockroom or the example programs are **excerpts**. They compile in their
  own crate, not on their own, and their first line says where they come from.

(For maintainers: in the Markdown source, runnable examples are fenced with backticks and
everything else with tildes, `~~~rust`, so that `tools/book.py` can extract and run every
runnable block, and check every excerpt against the file it quotes. An HTML comment before a
block, such as `<!-- check: needs=lagproxy -->`, tells the tool how to run it.)

---

## stockroom in one page

stockroom is a click-and-collect service for a small retail chain: a catalog, stock per store,
and orders that customers place online and pick up in a store. It is small enough to read in
an afternoon and real enough to have the problems real services have: two customers wanting the
last chair, staff screens that must update live, price lists arriving as CSV files, emails
that must go out exactly when an order is ready.

```text
  HTTP clients ──▶ axum router  (http/)  ──▶ repository functions (repo/) ──▶ PostgreSQL 18
  (curl, apps)     extractors, errors,         SQL in full, row mapping,      schema in
                   streaming bodies            transactions, COPY             migrations/
        ▲                                                                          │
        │ server-sent events                                       NOTIFY from a trigger
        │                                                                          ▼
     Hub (notify/) ◀─────── one dedicated LISTEN connection ◀───────────────────────┘
        │
        └─▶ outbox worker (worker/): FOR UPDATE SKIP LOCKED, woken by NOTIFY

  command line:  stockroom migrate · seed (binary COPY) · export (portal, COPY out) · serve
```

| Part of stockroom | What it is good for teaching | Chapters |
|---|---|---|
| [domain/](../stockroom/src/domain/mod.rs) | Mapping a PostgreSQL enum, a composite type and two domains to Rust types, by hand and by derive | 3, 5 |
| [error.rs](../stockroom/src/error.rs) | Turning SQLSTATE codes and constraint names into HTTP responses | 6, 15 |
| [repo/](../stockroom/src/repo/mod.rs) | SQL written out in full, pipelined queries, transactions, COPY, savepoints | 2–10, 14 |
| [db/](../stockroom/src/db/mod.rs) | Connecting, pooling, migrations, retries, cancelling abandoned queries, TLS | 1, 8, 12–14 |
| [notify/](../stockroom/src/notify/listener.rs) | A connection driven by hand, reconnecting, fan-out to many clients | 11, 16 |
| [http/](../stockroom/src/http/mod.rs) | axum on top of all of it | 15, 16 |
| [tests/](../stockroom/tests/concurrency.rs) | A database per test, and proving that no order oversells | 17 |

Five small programs in [apps/](../apps/) each do one thing a chapter talks about:
[pgq](../apps/pgq/src/lib.rs) is a tiny psql, [lagproxy](../apps/lagproxy/src/lib.rs) puts a
slow network between you and the server, [pgload](../apps/pgload/src/lib.rs) bulk-loads rows
five ways and times them, [pglisten](../apps/pglisten/src/lib.rs) tails notifications, and
[minipool](../apps/minipool/src/lib.rs) is a connection pool in about a hundred lines.

---

## Setting up

### PostgreSQL

The book targets **PostgreSQL 18**. Most examples run on 14 or newer; the text says so whenever
a feature needs a newer server (for example, "`MERGE` (PG 15+)"). Any of these will do:

```bash
# The EDB installer (macOS, Windows, Linux): https://www.postgresql.org/download/
# Homebrew (macOS):
brew install postgresql@18 && brew services start postgresql@18
# Docker:
docker run --name pg18 -e POSTGRES_PASSWORD=secret -p 5432:5432 -d postgres:18
```

You also want `psql`, PostgreSQL's command-line client. It comes with all three; with Docker,
`docker exec -it pg18 psql -U postgres` works without installing anything.

Create a database for the book's examples, and tell the examples where it is with an
environment variable:

```bash
psql "postgres://postgres:secret@localhost:5432/postgres" -c 'CREATE DATABASE tpbook'
export DATABASE_URL=postgres://postgres:secret@localhost:5432/tpbook
```

Use your own user, password and port. The examples only create temporary tables, which
disappear when their connection closes, so they leave nothing behind and can run in any order.

### The toolchain

The book uses Rust's 2024 edition and crates that need a recent compiler, so you need Rust 1.89
or newer (uuid 1.29 declares it; the book was written with 1.99):

```bash
rustup update
rustc --version
```

### The playground

Make a scratch crate for running the book's examples. It must be called `playground`, because
the examples use a helper from it:

```bash
cargo new --lib playground
cd playground
```

Replace its `Cargo.toml` with this. These are the crates the book's runnable examples use.
The versions are minimums: any newer compatible release works, and `cargo update` fetches the
latest one.

<!-- mirror: playground/Cargo.toml -->
```toml
[package]
name = "playground"
version = "0.1.0"
edition = "2024"
publish = false

[dependencies]
anyhow = "1.0.104"
async-stream = "0.3.6"
axum = "0.8.9"
bytes = "1.12.1"
deadpool-postgres = "0.14.2"
futures-util = "0.3.34"
http-body-util = "0.1.5"
jiff = { version = "0.2.39", features = ["serde"] }
postgres-types = { version = "0.2.14", features = ["derive"] }
rust_decimal = { version = "1.43.0", features = ["db-tokio-postgres"] }
rustls = { version = "0.23.46", default-features = false, features = ["ring", "std", "logging", "tls12"] }
serde = { version = "1.0.229", features = ["derive"] }
serde_json = "1.0.154"
thiserror = "2.0.21"
tokio = { version = "1.54.1", features = ["full"] }
tokio-postgres = { version = "0.7.18", features = ["with-jiff-0_2", "with-serde_json-1", "with-uuid-1", "array-impls"] }
tokio-postgres-rustls = "0.14.0"
tokio-util = "0.7.20"
tower = { version = "0.5.3", features = ["util"] }
tower-http = { version = "0.7.1", features = ["trace", "timeout", "limit"] }
uuid = { version = "1.29.0", features = ["v7", "serde"] }
```

The `features` on `tokio-postgres` switch on conversions for other crates' types: `jiff`'s
timestamps, `serde_json`'s values, `uuid`'s UUIDs, and Rust arrays. Chapter 3 explains them.
`rustls` and `tokio-postgres-rustls` are for the TLS connections of Chapter 12, and
`tokio-util` and `tower-http` for the services of Part III.

Then replace `src/lib.rs` with the helper every example uses to connect. Chapter 1 explains
every line of it:

<!-- mirror: playground/src/lib.rs#connect -->
~~~rust
use tokio_postgres::{Client, Error, NoTls};

/// Connects to the database named by the `DATABASE_URL` environment variable and spawns the
/// connection task, returning the client.
///
/// # Panics
///
/// If `DATABASE_URL` is not set: examples are programs, not libraries, and a clear panic is
/// the most useful thing they can do about a missing setting.
pub async fn connect() -> Result<Client, Error> {
    let url = std::env::var("DATABASE_URL")
        .expect("set DATABASE_URL, e.g. postgres://postgres:secret@localhost:5432/tpbook");
    let (client, connection) = tokio_postgres::connect(&url, NoTls).await?;
    tokio::spawn(async move {
        if let Err(e) = connection.await {
            eprintln!("connection error: {e}");
        }
    });
    Ok(client)
}
~~~

Put each example in its own file under `examples/`, for instance `examples/ch00_taste.rs`, and
run it with

```bash
cargo run --example ch00_taste
```

### The commands you will use in this repository

```bash
cargo run -p pgq -- -c "SELECT version()"     # the tiny psql from apps/pgq
cargo run -p stockroom -- --help              # the service's command line
cargo test -p stockroom                       # its tests: they create and drop their own databases
python3 tools/book.py check                   # run every example in this book
```

The repository's own `.env.example` lists the environment variables they read. Copy it to
`.env` and fill in your credentials.

---

## A ten-line taste

Connect, send one query with a parameter, and read two typed values out of the row:

```rust
use playground::connect;

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = connect().await?;

    // `$1` is a parameter: its value travels separately from the SQL text.
    let row = client
        .query_one(
            "SELECT $1::text || ', ' || current_user AS greeting, 6 * 7 AS answer",
            &[&"hello"],
        )
        .await?;

    let greeting: &str = row.get("greeting");
    let answer: i32 = row.get("answer");
    println!("{greeting}; the answer is {answer}");
    assert!(greeting.starts_with("hello, "));
    assert_eq!(answer, 42);
    Ok(())
}
```

```text
hello, postgres; the answer is 42
```

Four things in these lines are worth noticing now, and each gets a chapter later:

- `connect().await?` gives you a `Client`, but something else must keep the connection alive
  in the background: the helper spawns it. Forgetting that is the first mistake everyone
  makes (Chapter 1).
- The value `"hello"` is not pasted into the SQL. It is sent as a **parameter**, `$1`, in a
  separate message, which is why SQL injection cannot happen here (Chapter 2).
- `row.get("answer")` turns PostgreSQL's `integer` into Rust's `i32` because you asked for an
  `i32`. Ask for an `i64` and it panics: the conversion is checked, at run time
  (Chapter 3).
- `query_one` sent more than one message and waited for more than one answer. Knowing how
  many is the key to making a database-backed service fast (Chapter 4).

## Exercises

1. **Set up the playground** as described above, save the ten-line taste as
   `examples/ch00_taste.rs`, and run it.
   *Hint:* if it panics with "set DATABASE_URL", the environment variable is not set in the
   shell you ran `cargo` from.
2. **Break it on purpose.** Change `let answer: i32` to `let answer: i64` and run it again.
   Read the panic message: what does it say about the types on each side?
   *Hint:* the SQL expression `6 * 7` has the type `integer`, which is 32 bits wide.
3. **Use the tiny psql.** From this repository's root, run
   `cargo run -p pgq -- -c "SELECT name, setting FROM pg_settings WHERE name LIKE 'max_conn%'"`
   and then the same with `--describe`. What does `--describe` tell you that the plain run
   does not?
   *Hint:* `--describe` prepares the statement instead of running it (Chapter 4 explains what a
   prepare returns).
4. **Run the service.** From the repository's root:
   `cargo run -p stockroom -- migrate`, then `cargo run -p stockroom -- seed`, then
   `cargo run -p stockroom -- serve`, and open [stockroom.http](../stockroom/stockroom.http)
   in an editor with an HTTP client (VS Code's REST Client, or curl by hand). Place an order
   and watch it in another terminal with
   `curl -N 'http://127.0.0.1:8080/orders/events?store=store-01'`.
   *Hint:* `seed` creates the stores `store-01` to `store-03`.

### Solutions

**2.** The panic reads only "error retrieving column answer: error deserializing column 1". The
reason is one level down, in the error's `source()`, which a panic message does not print. With
`row.try_get::<_, i64>("answer")` instead of `get`, you can print the error with `{:?}`:

```text
Error { kind: FromSql(1), cause: Some(WrongType { postgres: Int4, rust: "i64" }) }
```

tokio-postgres knows the column's type from the row description the server sent, and every
Rust type says which PostgreSQL types it accepts. `i64` takes only `int8`. There is no silent
widening: you change the Rust type, or cast in SQL (`(6 * 7)::int8`). Chapter 6 shows how to
get at the cause of any error.

**3.** `--describe` does not run the query. It asks the server to prepare it and reports the
type of every result column (both `text` here) and, for columns that come from a table or view,
which one: `oid 12104, column 1` is the first column of the view `pg_settings`, whose OID
(object id) you can look up with `SELECT 12104::regclass`. The plain run uses the simple query
protocol, in which every value arrives as text and types are not part of the result
(Chapter 2).

---

Next: [Chapter 1 — Connecting](01-connecting.md)
