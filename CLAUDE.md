# CLAUDE.md

Guidance for Claude Code working in this repository.

## What this is

A course book, **Async PostgreSQL in Rust**, about
[`tokio-postgres`](https://docs.rs/tokio-postgres/latest/tokio_postgres/). It comes with the
software the book teaches from:

- `book/`: one Markdown chapter per file, and the whole book as one PDF.
- `stockroom/`: the capstone, an axum HTTP service that uses almost every tokio-postgres feature.
- `apps/`: small example programs, each behind one or two chapters (pgq, lagproxy, pgload,
  pglisten, minipool).
- `playground/`: the reader's scratch crate. Its `examples/` directory is **generated** from the
  book.
- `tools/book.py`: extracts the examples, checks them, renders the PDFs.

**Audience:** developers with average Rust, basic Tokio and basic SQL. The book ends with them
building and operating a Postgres-backed axum service.

**[PLAN.md](PLAN.md) is the source of truth** for scope, the chapter outline, the stockroom
spec, the feature coverage matrix (§5.3), the phases and the decisions taken. Read it before
starting any phase or chapter. When a decision changes, update PLAN.md in the same change.

The style model is an existing Rust course in another repository ("the reference course"). Its
location on this machine is in `CLAUDE.local.md`, which is gitignored; if that file is missing,
ask for the path. Read the course, never modify it. When unsure how a section should look, open
the matching chapter there. For example, `10-async.md` shows the section rhythm, and
`14-capstone.md` shows a capstone.

## Status

Track it in `book/book.toml`: each chapter is `planned`, `draft` or `current`.
`python3 tools/book.py list` prints the table.

- **Phase 0 (scaffold) is done** (2026-11-06): the workspace, the playground, `tools/book.py`
  with its tests, the PDF pipeline, the docker TLS server and the registry all exist.
  Chapter 0 is a `draft` stub with one runnable example.
- **Phase 1 (stockroom) is done** (2026-11-06). PLAN.md §5.1 describes it as built: every
  endpoint and subcommand with the file that implements it. 43 tests pass, and the oversell test
  passed 50 out of 50 runs. PLAN.md §12 lists 17 findings from building it, each tagged with
  the chapter that should teach it.
- **Phase 2 (apps) is done** (2026-11-06): lagproxy, pgq, pgload, pglisten and minipool, each a
  library plus a binary with tests. PLAN.md §5.2 describes them as built, with measured
  benchmark tables. `book.py check` now runs `needs=lagproxy` examples through lagproxy.
- **Phase 3a (Part I, Chapters 0–6) is done** (2026-11-06): all `current`, 59 runnable
  examples, the book PDF rendered. Writing them corrected several facts; see PLAN.md
  §12, rows 27–33.
- **Phases 3b and 3c (Part II, Part III and the appendix) are done** (2026-11-06): Chapters
  7–18 `current`, 156 runnable examples in all, every gate green (including the three
  `needs=tls` examples against docker compose). The playground gained the helpers
  `slice_iter`, `connect_with_messages`, `pool` and `call`, and the crates `rustls`,
  `tokio-postgres-rustls`, `tokio-util` and `tower-http`. Findings: PLAN.md §12, rows 34–45.
  Notable: in 0.7.18 a `COPY … FROM STDIN` rejected as it starts kills the connection
  (rust-postgres issue #789, Chapter 10).
- **Phase 4 (polish) is done** (2026-11-06): the book PDF rebuilt (270 pages), banners and
  exercise hints made uniform (17 Part I and II exercises lacked a *Hint*), cross-references
  checked, and the §5.3 coverage audit closed with one new example in §12.4. Appendix F
  (crates built on tokio-postgres) was researched from crates.io, docs.rs and the crates' source.

## Commands

```bash
# Gates: run all of them before calling work done
cargo test --workspace                                   # stockroom + apps; needs ADMIN_DATABASE_URL (.env)
cargo clippy --workspace --all-targets -- -Dwarnings
cargo clippy --workspace --all-targets --features stockroom/tls -- -Dwarnings
cargo fmt --all -- --check
python3 tools/book.py check                              # registry, versions, mirrors, links, excerpts,
                                                         # then build + clippy + rustfmt + RUN every
                                                         # ```rust block against a fresh tpbook_check
python3 -m unittest discover tools                       # the tool's own tests

# Narrower
python3 tools/book.py check --chapter 8                  # only chapter 8's examples (all else still checked)
python3 tools/book.py check --no-run                     # compile and lint only; no database
python3 tools/book.py check --keep-db                    # leave tpbook_check behind to inspect
python3 tools/book.py links                              # just links and #anchors
python3 tools/book.py excerpts                           # just ~~~ excerpts vs their source files
python3 tools/book.py fmt --chapter 8                    # rustfmt the chapter's ```rust blocks, in place
python3 tools/book.py extract                            # regenerate playground/examples/
python3 tools/book.py list                               # chapter status, examples, TODOs; the book PDF's freshness
python3 tools/book.py index                              # regenerate README.md's contents table
python3 tools/book.py book                               # the book PDF: the only PDF in the repository
python3 tools/book.py pdf 3                              # one chapter, locally, into target/book/ (never committed)
docker/gen-certs.sh && docker compose up -d --wait       # optional TLS server (Chapter 12)

# stockroom (reads .env: STOCKROOM_DATABASE_URL; every flag also has an env var, see --help)
cargo run -p stockroom -- migrate
cargo run -p stockroom -- seed --reset                   # binary COPY: stores, products, stock, ledger
cargo run -p stockroom -- export products --out p.ndjson # portal batches; `movements` = binary COPY out
cargo run -p stockroom -- serve                          # :8080; then stockroom/stockroom.http
cargo test -p stockroom --test concurrency               # the oversell / deadlock / transfer tests
cargo run -p stockroom --features tls -- --db-ca-file docker/certs/ca.crt migrate   # over TLS (:5434)

# The example apps (DATABASE_URL from .env)
cargo run -p pgq -- -c "SELECT 1; SELECT now()"          # also --describe, --stream, --timing
cargo run -p pglisten -- order_events outbox             # tail notifications
cargo run -p lagproxy -- --delay-ms 5                    # :6543 → :5433, +5 ms each way
cargo run -p pgload --release -- --rows 20000 [--delay-ms 1]    # Chapter 10's table
cargo run -p minipool --release -- --tasks 64 --pool-size 8 [--delay-ms 2]   # Chapter 13's table
```

## Database

- **The target server is PostgreSQL 18.6** (EDB installer, `localhost:5433`, `ssl = off`).
  Superuser `postgres`; its password is in `.env`. Every runnable example and all of stockroom
  are verified on it.
- `psql` is **not on the PATH**: the Homebrew PostgreSQL 14 that provided it was uninstalled on
  2026-11-06. Use `/Library/PostgreSQL/18/bin/psql`. `book.py` finds it by itself, or you can
  set `$PSQL`. Nothing listens on `:5432` any more.
- **Modern features are welcome**, for example `uuidv7()`, `RETURNING old/new`, virtual
  generated columns, `MERGE`, and COPY `ON_ERROR`. The first time the book uses a feature newer
  than PG 14, it names the version that introduced it ("`MERGE` (PG 15+)"). Verify such claims
  on the server before writing them.
- **`.env` is gitignored and holds the real credentials.** Never commit it, and never write the
  password into any tracked file, the book included. Tracked files use `.env.example` and its
  placeholders.
- Databases:
  - `tpbook` is the scratch database for manual experiments.
  - `tpbook_check` is **dropped and recreated** by every `book.py check`.
  - `stockroom` is for development.
  - `stockroom_template_<checksum>` (migrated once, rebuilt when the migrations change) and
    the `stockroom_test_<uuid>` databases belong to the stockroom tests
    (`stockroom/tests/common/mod.rs`). Each test clones the template and drops its clone.
  - Do not touch any other database on the server.
- The optional `docker compose up -d --wait` runs Postgres 18 **with TLS on `:5434`**. It
  accepts TLS connections only (`hostssl`), and its certificate chains to
  `docker/certs/ca.crt`, which `docker/gen-certs.sh` creates and which is gitignored. Only the
  examples marked `needs=tls` use it. `check` skips them, and says why, when
  `TLS_DATABASE_URL` is unset or nothing listens on it.
- `localhost:5433` belongs to the EDB server, so the docker server cannot use that port.

## Writing the book: the style contract

These conventions are fixed. They come from the reference course and are adapted as PLAN.md §2
and §3 describe.

**Chapter files**

- Name: `book/NN-slug.md`. First line: `# Chapter N — Title`, with an em dash.
- Directly under the heading comes the banner:
  `> **Async PostgreSQL in Rust** · Previous: [Chapter N-1 — …](…) · Next: [Chapter N+1 — …](…)`.
  The same Previous/Next line also closes the chapter, after a `---`.
- After the banner: one or two intro paragraphs on why the topic matters, then `---`.
- Sections are numbered `## N.M Title`. Subsections use `###` without numbers.
- The chapter ends with `## Summary` (bullets), then `## Exercises` (a numbered list, each item
  with a *Hint*), then `### Solutions` (only for exercises whose answers are not obvious).
- Register every chapter in `book/book.toml` (id, path, title, summary, status). Its `title`
  must equal the chapter's `# ` heading.

**Side-by-side sections**

- Heading: `### Side by side — *concept*`.
- Open with one sentence stating the task. Then come three blocks: a `sql` block for psql (with
  the output, if it helps), a ` ~~~rust ` block for sqlx (illustration only, with
  `// sqlx (illustration)` as its first line), and a runnable ` ```rust ` block for
  tokio-postgres.
- End with a table: `| | psql | sqlx | tokio-postgres |` with the rows **Same** and
  **Different**.
- Part III drops the psql column.

**Callouts**, used where they are earned and not in every section:

- `> **Protocol lens:**` covers what goes over the socket.
- `> **SQL lens:**` covers server-side semantics.
- `> **Rust refresher:**` covers a language feature the audience may be hazy on.
- `> **Pitfall:**` covers a mistake that compiles and then fails in production.

**"In stockroom" sections** show trimmed excerpts. Each excerpt's first line is
`// stockroom/src/path.rs (excerpt)`, and a link to the file follows the excerpt. Mark omissions
with `// …`.

**Code blocks.** There are two kinds, and they must not be mixed up:

- ` ```rust ` is a **complete, runnable program**. `book.py check` extracts it, compiles it with
  clippy `-Dwarnings`, and runs it against a fresh database. Its `assert!`s state what the
  program produces. From Chapter 1 on it may use `playground::connect()` and the other helpers
  in `playground/src/lib.rs`. Those helpers are printed verbatim in the chapter that introduces
  them, so keep the two in sync.
- ` ~~~rust ` is an excerpt, a sqlx illustration, an intentionally broken snippet for an
  exercise, or any other fragment. It is never compiled alone. (Pandoc cannot parse info strings
  like ` ```rust,ignore `, which is why tildes are used.)
- If a runnable block needs special handling, put an HTML comment on the line before it:
  `<!-- check: no-run -->`, `<!-- check: needs=tls -->`, `<!-- check: needs=lagproxy -->` or
  `<!-- check: should-panic -->`.
  - With `needs=lagproxy`, `DATABASE_URL` goes through lagproxy (+5 ms each way, a lower
    bound), `DIRECT_DATABASE_URL` does not, and `LAGPROXY_DELAY_MS` holds the delay.
- Output shown to the reader goes in a ` ```text ` block, SQL in ` ```sql `, and shell commands
  in ` ```bash `.

**Example hygiene**

- Every runnable example can run in any order, and more than once. Use `CREATE TEMP TABLE`, or
  create a uniquely named schema and drop it at the end.
- Examples never depend on each other's data.
- Assert on **relative** timings, for example "pipelined < sequential / 2". Never assert on
  absolute timings.
- Runnable axum examples never bind a port. They drive the router with
  `tower::ServiceExt::oneshot`.

**Voice**

- Second person and plain English, in the reference course's tone: direct and precise, saying
  *why* before *how*.
- Name the exact API and version whenever behaviour depends on it, for example "in 0.7.18 the
  per-request response channel holds one message".
- Every claim about behaviour should be backed by a runnable example, or by a link to docs.rs or
  the PostgreSQL docs.
- Do not invent API: check `~/.cargo/registry/src/*/tokio-postgres-0.7.18/src/` or docs.rs. If
  something is uncertain, verify it with a probe in the scratchpad before writing it down.
- **Never write program output from memory.** psql output, error messages, timings and
  `Debug` output must be captured from a real run (psql 18, which prints `\conninfo` as a
  table for instance) and pasted. Measured numbers say what they were measured on.

## Code conventions (stockroom, apps, playground)

- Edition 2024. Declare dependency versions **once**, in the root `[workspace.dependencies]`.
  Members use `{ workspace = true }`.
- Default rustfmt formatting (4 spaces), clippy clean with `-Dwarnings`.
- **SQL is always parameterized** (`$1…$n`). Never `format!` a value into SQL. Identifiers that
  have to be dynamic go through a `quote_ident` helper, and the code says why.
- SQL constants live with the repository function that uses them, or in `.sql` files pulled in
  with `include_str!`. Do not scatter them through handlers.
- Errors:
  - Library code uses `thiserror`. `anyhow` is allowed only at the top level: `main.rs`, and
    the startup and CLI paths (`app.rs`, `cli/*`, `db::tls`, `notify::listener`'s internals).
  - stockroom's `AppError` maps `SqlState` and constraint names to HTTP statuses. Constraint
    names are a contract: a new or renamed constraint in a migration needs its entry in
    `error::constraint_message`.
  - `DbError` messages never reach an HTTP client. The only exception is COPY data errors
    (class 22), which quote the client's own CSV back with its line number.
- stockroom's shapes, to reuse rather than reinvent:
  - repository functions take `&impl deadpool_postgres::GenericClient` (or `&Transaction<'_>`)
    and use `prepare_cached`;
  - handlers get a connection from the `Db` extractor and a JSON body from `AppJson`;
  - transactions that can collide go through `db::in_transaction(&mut db, isolation, async move
    |tx| repo::f(tx, &input).await)`, with the closure owning `input`;
  - row locks are always taken in a fixed order (by id).
  - a streamed response body (`RowStream`, `CopyOutStream`) owns its pooled connection through
    `CancelOnDrop`, and calls `finish()` after the last chunk: a client that hangs up
    mid-stream gets its query cancelled and the connection discarded, not returned busy.
- `#![allow(clippy::clone_on_copy)]` in `stockroom/src/lib.rs` is deliberate: `MakeTls` is the
  `Copy` type `NoTls` without the `tls` feature, and the clones are needed with it.
- `tracing` handles logging. Log the SQL statement name and its duration, never parameter values
  (they may contain personal data).
- stockroom targets PG 18. UUID keys default to `uuidv7()` in the schema and map to `uuid::Uuid`.
- If a stockroom change alters code the book quotes, run `book.py excerpts` and fix the chapter
  in the same change.

## Verified API facts (tokio-postgres 0.7.18). Do not "correct" these from memory

- `Client` has `query_scalar`, `query_one_scalar` and `query_opt_scalar`. It also has
  `query_typed`, `query_typed_one`, `query_typed_opt`, `query_typed_raw`, `execute_typed` and
  `check_connection`.
  - **Which version added what**, from the crate's CHANGELOG. The book says so when it uses
    one of these:
    - 0.7.11: `SimpleQueryMessage::RowDescription`, and `query_typed` on `Client`,
      `Transaction` and `GenericClient`.
    - 0.7.15: `check_connection`.
    - 0.7.16: `query_scalar`, `query_one_scalar` and `query_opt_scalar`, on **`Client`
      only** (not on `Transaction` or `GenericClient`).
    - 0.7.17: `execute_typed`, `query_typed_one` and `query_typed_opt`, on all three.
- `Client::transaction` and `build_transaction` take **`&mut self`**. `TransactionBuilder` has
  `isolation_level`, `read_only`, `deferrable` and `start`.
- `IsolationLevel` has four variants: `ReadUncommitted`, `ReadCommitted`, `RepeatableRead` and
  `Serializable`.
- A `Transaction` **rolls back on drop**, and errors from that rollback are lost. Call
  `rollback()` to see them.
- `Transaction` has `transaction()` and `savepoint(name)` for nesting, `bind`/`bind_raw` and
  `query_portal(&portal, max_rows: i32)`/`query_portal_raw` for portals, and `client()`.
- `GenericClient` is **sealed** and async-trait style: its methods return boxed `Send` futures.
  It is implemented by `Client` and `Transaction<'_>`, and it is not dyn-compatible. deadpool
  has its own `GenericClient`, which adds `prepare_cached`.
- `SqlState` constants **can be used as `match` patterns**. This was verified by compiling
  `match *code { SqlState::UNIQUE_VIOLATION => … }`.
- `AsyncMessage` and `SimpleQueryMessage` are `#[non_exhaustive]`, so a `match` needs a `_` arm.
  The variants are `AsyncMessage::{Notice(DbError), Notification(Notification)}` and
  `SimpleQueryMessage::{Row, CommandComplete(u64), RowDescription(Arc<[SimpleColumn]>)}`.
- `Connection` is a `Future<Output = Result<(), Error>>`. To receive notifications and notices,
  drive it with `poll_message(cx)` instead of awaiting it.
- `Client` sends requests over an mpsc channel. **Each request's response channel is bounded at
  1** (`client.rs`), so a slow consumer of one `RowStream` stalls the whole connection.
- `RowStream::rows_affected()` returns an `Option<u64>`, which becomes available once the stream
  is exhausted.
- `CopyInSink<T: Buf>` is a `Sink`, and you must call `finish()` on it. Dropping it unfinished
  aborts the COPY. `SinkExt::send` flushes per item (one CopyData message each).
- **A `COPY … FROM STDIN` that the server rejects as it starts (missing table or column,
  `ON_ERROR ignore` with binary) kills the connection** in 0.7.18: the next request fails
  with "unexpected message from server" (rust-postgres issue #789). Errors during the copy
  or from `finish()` are fine.
- `drop(x)` on a value pinned with `pin!` drops only the reference: an unfinished
  `RowStream` or `BinaryCopyInWriter` then stalls the connection. Scope it in a block.
- `BinaryCopyInWriter::new(sink, &[Type])` is used with `write(&[&dyn ToSql])` and then
  `finish() -> u64`, and the writer must be pinned. `BinaryCopyOutStream::new(stream, &[Type])`
  yields `BinaryCopyOutRow` values, which have `get`/`try_get` by index.
- `Config` accepts key/value and URL strings.
  - `SslMode` is only `Disable`, `Prefer` (the default) or `Require`. There is **no**
    `verify-full`; verifying the certificate is the TLS connector's job.
  - Other settings: `SslNegotiation` (`direct` needs PG 17+), `TargetSessionAttrs`,
    `LoadBalanceHosts` and `ChannelBinding`.
  - `connect_raw` only applies user, password, dbname, options and application_name.
- `Client::cancel_query` is deprecated. Use `cancel_token()` and then
  `CancelToken::cancel_query(tls)`.
- deadpool-postgres 0.14.2 defaults to `RecyclingMethod::Fast`. Its other methods are
  `Verified`, `Clean` and `Custom`.
  - Its `GenericClient` is, like tokio-postgres's, **sealed and `#[async_trait]`** (boxed
    `Send` futures; corrected 2026-11-06 from the source, `generic_client.rs`), plus
    `prepare_cached`. It is implemented for `Object` (= `deadpool_postgres::Client`) and
    deadpool's `Transaction`.
  - It is built on deadpool 0.13.1 (Cargo.lock), not 0.12.
  - `Object` derefs to `ClientWrapper`, which derefs to `tokio_postgres::Client`, so
    `&object` coerces to `&tokio_postgres::Client`.
  - `Object::take(obj)` detaches a connection from the pool.
- **Domain types** (verified on PG 18.6):
  - A *parameter* that targets a domain column is typed as the domain, so `&str` and `i32` are
    refused on the client. Use a cast (`$1::text`), `query_typed` with the base `Type`, a
    derived domain newtype, or a hand-written `accepts`.
  - A domain *result column* is described as its base type.
  - In `col = $1` and `col = ANY($1)` comparisons, the parameter is the base type.
- **`AsyncFnMut(&Transaction<'_>)` helpers work in axum handlers and spawned tasks if the
  closure owns what it uses** (`async move |tx| place(tx, &order).await`, with `order` owned by
  the handler). A closure whose captures contain *any* reference (to the caller's locals, or
  even a `&'static str`) fails there with
  "implementation of `Send` is not general enough", a compiler limitation (verified with
  rustc 1.99, isolated in six variants: PLAN.md §12 #6). stockroom's `db::in_transaction` takes
  an `AsyncFnMut`.
- A query future borrows its parameter slice. When futures are joined, bind the slice to a name
  or build it inside an `async move` block.

## Workflow

- **Work in the order of PLAN.md §9.** Build stockroom before writing the chapters that quote it.
- **Write one chapter at a time**, and verify it before starting the next. For each chapter:
  1. Reread its outline in PLAN.md §7 and its rows in the coverage matrix §5.3.
  2. Probe uncertain API in the scratchpad.
  3. Write the chapter.
  4. Run `book.py fmt --chapter N`, then `book.py check --chapter N` (which includes excerpts
     and links).
  5. Set its status in `book.toml`.
  6. Rebuild the book PDF (`book.py book`) once the content is stable. Chapter PDFs are not
     committed: `book.py pdf N` renders one into `target/book/` for a local look.
- Each exercise solution that contains code is a runnable block too.
- After any chapter is added or renamed, update the contents table in `00-introduction.md` and
  the neighbouring chapters' Previous/Next links.
- Do not edit `playground/examples/`. It is regenerated from the book.
