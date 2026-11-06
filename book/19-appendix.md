# Appendix — cheat sheets, phrasebook, glossary and ecosystem

> **Async PostgreSQL in Rust** · Previous: [Chapter 18 — Capstone: stockroom end to end](18-capstone.md)

Reference material, for looking things up rather than reading through: every method of `Client`
and `Transaction` with its return type, the type-mapping table, a SQLSTATE table with suggested
HTTP statuses, a phrasebook for readers coming from sqlx or diesel, a glossary, the crates built
on tokio-postgres, and where to read further. Everything here is for tokio-postgres 0.7.18,
postgres-types 0.2.14 and deadpool-postgres 0.14.2, taken from their source and documentation on
2026-11-06.

---

## A. API cheat sheet

### `Client`

All methods take `&self` except the two that start a transaction. "Text or statement" means any
`T: ToStatement`: a `&str` (prepared on the fly, two round trips, Chapter 4) or a `&Statement`.
`params` is `&[&(dyn ToSql + Sync)]`, and typed params are `&[(&(dyn ToSql + Sync), Type)]`.

| Method | Takes | Returns | Chapter |
|---|---|---|---|
| `batch_execute` | SQL text, any number of statements | `Result<(), Error>` | 2 |
| `simple_query` | SQL text | `Result<Vec<SimpleQueryMessage>, Error>` | 2 |
| `simple_query_raw` | SQL text | `Result<SimpleQueryStream, Error>` | 9 |
| `execute` | text or statement, params | `Result<u64, Error>` (rows affected) | 2 |
| `query` | text or statement, params | `Result<Vec<Row>, Error>` | 2 |
| `query_one` | text or statement, params | `Result<Row, Error>` (error unless exactly one row) | 2 |
| `query_opt` | text or statement, params | `Result<Option<Row>, Error>` (error if more than one) | 2 |
| `query_scalar` (0.7.16+) | text or statement, params | `Result<Vec<R>, Error>`, `R: FromSqlOwned` | 2 |
| `query_one_scalar` (0.7.16+) | text or statement, params | `Result<R, Error>` | 2 |
| `query_opt_scalar` (0.7.16+) | text or statement, params | `Result<Option<R>, Error>` | 2 |
| `query_raw` | text or statement, iterator of `BorrowToSql` | `Result<RowStream, Error>` | 9 |
| `execute_raw` | text or statement, iterator | `Result<u64, Error>` | 9 |
| `prepare` | SQL text | `Result<Statement, Error>` | 4 |
| `prepare_typed` | SQL text, `&[Type]` | `Result<Statement, Error>` | 4 |
| `query_typed` (0.7.11+) | SQL text, typed params | `Result<Vec<Row>, Error>` (one round trip) | 4 |
| `query_typed_one`, `query_typed_opt` (0.7.17+) | SQL text, typed params | `Result<Row, Error>`, `Result<Option<Row>, Error>` | 4 |
| `query_typed_raw` | SQL text, iterator of `(value, Type)` | `Result<RowStream, Error>` | 4, 9 |
| `execute_typed` (0.7.17+) | SQL text, typed params | `Result<u64, Error>` | 4 |
| `copy_in` | a `COPY … FROM STDIN` statement | `Result<CopyInSink<U>, Error>`, `U: Buf` | 10 |
| `copy_out` | a `COPY … TO STDOUT` statement | `Result<CopyOutStream, Error>` | 10 |
| `transaction` | `&mut self` | `Result<Transaction<'_>, Error>` | 8 |
| `build_transaction` | `&mut self` | `TransactionBuilder<'_>` | 8 |
| `cancel_token` | | `CancelToken` | 12 |
| `check_connection` (0.7.15+) | | `Result<(), Error>` (one round trip, no statement) | 12 |
| `is_closed` | | `bool` | 1, 12 |
| `clear_type_cache` | | `()` | 5 |
| `cancel_query`, `cancel_query_raw` | | deprecated since 0.6.0: use `cancel_token()` | 12 |

### `Transaction<'_>`

The same query methods as `Client` (`batch_execute`, `simple_query`, `execute`, `query`,
`query_one`, `query_opt`, `query_raw`, `execute_raw`, `prepare`, `prepare_typed`, the
`query_typed` family, `execute_typed`, `copy_in`, `copy_out`, `cancel_token`), **without** the
`query_*_scalar` methods and `simple_query_raw`, plus:

| Method | Returns | Chapter |
|---|---|---|
| `commit(self)` | `Result<(), Error>` | 8 |
| `rollback(self)` | `Result<(), Error>`; dropping rolls back too, but loses the rollback's error | 8 |
| `transaction(&mut self)` | `Result<Transaction<'_>, Error>`: a savepoint with a generated name | 8 |
| `savepoint(&mut self, name)` | `Result<Transaction<'_>, Error>`: a named savepoint | 8 |
| `bind(&statement, params)`, `bind_raw` | `Result<Portal, Error>` | 9 |
| `query_portal(&portal, max_rows)` | `Result<Vec<Row>, Error>` | 9 |
| `query_portal_raw(&portal, max_rows)` | `Result<RowStream, Error>` | 9 |
| `client()` | `&Client` | 8 |

`TransactionBuilder`: `isolation_level(IsolationLevel)`, `read_only(bool)`, `deferrable(bool)`,
`start() -> Result<Transaction<'_>, Error>`. `IsolationLevel`: `ReadUncommitted`,
`ReadCommitted`, `RepeatableRead`, `Serializable`.

### `GenericClient`

Sealed, `#[async_trait]`, implemented for `Client` and `Transaction<'_>`: `execute`,
`execute_raw`, `execute_typed`, `query`, `query_one`, `query_opt`, `query_raw`, `query_typed`,
`query_typed_one`, `query_typed_opt`, `query_typed_raw`, `prepare`, `prepare_typed`,
`transaction`, `batch_execute`, `simple_query`, `client`. deadpool-postgres has its own
`GenericClient`, also sealed and `#[async_trait]`, implemented for `Object` and deadpool's
`Transaction`, which adds `prepare_cached` and `prepare_typed_cached` (Chapters 13, 14).

### The rest of the crate

| Item | What it is | Chapter |
|---|---|---|
| `connect(url, tls)` | `Result<(Client, Connection<Socket, T::Stream>), Error>` | 1 |
| `Config` | parsed from a URL or key/value string; builder setters; `connect`, `connect_raw` | 1, 12 |
| `Connection` | a `Future<Output = Result<(), Error>>`; `poll_message`, `parameter(name)` | 1, 11 |
| `AsyncMessage` | `Notice(DbError)`, `Notification(Notification)`; non-exhaustive | 11 |
| `Row` | `get`, `try_get` (by index or name), `columns`, `len`, `is_empty`, `raw_size_bytes` | 3 |
| `Statement` | `params() -> &[Type]`, `columns() -> &[Column]` | 4 |
| `RowStream` | `Stream<Item = Result<Row, Error>>`, `!Unpin`; `rows_affected()` after the end | 9 |
| `SimpleQueryMessage` | `Row`, `CommandComplete(u64)`, `RowDescription` (0.7.11+); non-exhaustive | 2 |
| `CopyInSink<T>` | `Sink<T>`; `finish()` returns the row count | 10 |
| `CopyOutStream` | `Stream<Item = Result<Bytes, Error>>`, one chunk per row | 10 |
| `binary_copy::{BinaryCopyInWriter, BinaryCopyOutStream, BinaryCopyOutRow}` | binary `COPY` with declared types | 10 |
| `CancelToken` | `cancel_query(tls)`, `cancel_query_raw(stream, tls)` | 12 |
| `error::{Error, DbError, SqlState, Severity, ErrorPosition}` | Chapter 6's error model | 6 |
| `tls::{MakeTlsConnect, TlsConnect, TlsStream, NoTls}` | the TLS connector traits | 12 |
| `types` | re-export of postgres-types: `ToSql`, `FromSql`, `Type`, `Kind`, `Json`, … | 3, 5 |

## B. Types

### Built in

From the documentation of `ToSql` and `FromSql` in postgres-types 0.2.14:

| Rust type | PostgreSQL type(s) |
|---|---|
| `bool` | `BOOL` |
| `i8` | `"char"` |
| `i16` | `SMALLINT`, `SMALLSERIAL` |
| `i32` | `INT`, `SERIAL` |
| `u32` | `OID` |
| `i64` | `BIGINT`, `BIGSERIAL` |
| `f32` | `REAL` |
| `f64` | `DOUBLE PRECISION` |
| `&str`, `String` | `VARCHAR`, `CHAR(n)`, `TEXT`, `CITEXT`, `NAME`, `UNKNOWN`, `LTREE`, `LQUERY`, `LTXTQUERY` |
| `&[u8]`, `Vec<u8>` (and `[u8; N]` for `ToSql`) | `BYTEA` |
| `HashMap<String, Option<String>>` | `HSTORE` |
| `SystemTime` | `TIMESTAMP`, `TIMESTAMP WITH TIME ZONE` |
| `IpAddr` | `INET` |
| `Option<T>` | any of the above, nullable |
| `Vec<T>`, `Box<[T]>`, and `&[T]` for `ToSql`; `[T; N]` with the `array-impls` feature | one-dimensional arrays |
| `Json<T>` (`T: Serialize`/`Deserialize`) | `JSON`, `JSONB` (needs `with-serde_json-1`) |

There is no built-in mapping for `numeric` (use `rust_decimal`'s `db-tokio-postgres` feature,
Chapter 3), `interval`, or `money`. Read them as text (`::text`) or with a crate that provides
one, or implement `FromSql` yourself (Chapter 5).

### Behind features

Enable these on tokio-postgres (it forwards them to postgres-types). The book uses
`with-jiff-0_2`, `with-serde_json-1`, `with-uuid-1` and `array-impls`.

| Feature | Rust type(s) | PostgreSQL type(s) |
|---|---|---|
| `with-chrono-0_4` | `chrono::NaiveDateTime`, `DateTime<Utc \| Local \| FixedOffset>`, `NaiveDate`, `NaiveTime` | `TIMESTAMP`, `TIMESTAMPTZ`, `DATE`, `TIME` |
| `with-time-0_2`, `with-time-0_3` | `time::PrimitiveDateTime`, `OffsetDateTime`, `Date`, `Time` | `TIMESTAMP`, `TIMESTAMPTZ`, `DATE`, `TIME` |
| `with-jiff-0_1`, `with-jiff-0_2` | `jiff::civil::DateTime`, `jiff::Timestamp`, `jiff::civil::Date`, `jiff::civil::Time` | `TIMESTAMP`, `TIMESTAMPTZ`, `DATE`, `TIME` |
| `with-uuid-0_8`, `with-uuid-1` | `uuid::Uuid` | `UUID` |
| `with-serde_json-1` | `serde_json::Value`, `Json<T>` | `JSON`, `JSONB` |
| `with-cidr-0_2`, `with-cidr-0_3` | `cidr::IpCidr`, `cidr::IpInet` | `CIDR`, `INET` |
| `with-eui48-0_4`, `with-eui48-1` | `eui48::MacAddress` | `MACADDR` |
| `with-geo-types-0_6`, `with-geo-types-0_7` | `geo_types::Point<f64>`, `Rect<f64>`, `LineString<f64>` | `POINT`, `BOX`, `PATH` |
| `with-bit-vec-0_6` … `with-bit-vec-0_9` | `bit_vec::BitVec` | `BIT`, `VARBIT` |
| `with-smol_str-01` | `smol_str::SmolStr` | the text types |
| `array-impls` | Rust arrays `[T; N]` (and `[u8; N]` for `ToSql`) | arrays (`BYTEA` for `[u8; N]`) |

### Derived (postgres-types `derive` feature, Chapter 5)

| Attribute | For |
|---|---|
| `#[derive(ToSql, FromSql)]` on an enum | a PostgreSQL `ENUM`; `#[postgres(name = "…")]` on the type and variants; `rename_all = "…"`; `allow_mismatch` to tolerate labels on either side |
| on a struct with named fields | a composite type, field by field |
| on a newtype with `#[postgres(transparent)]` | the inner type, exactly |
| on a newtype without `transparent` | a domain over the inner type, matched by name |

## C. SQLSTATE cheat sheet

The codes a service meets most, the `SqlState` constant to match in Rust (Chapter 6), and a
suggested HTTP status (Chapter 15). "Retry" means the whole transaction may succeed if run again.

| SQLSTATE | `SqlState::…` | Meaning | Suggested status |
|---|---|---|---|
| 23505 | `UNIQUE_VIOLATION` | duplicate key | 409 |
| 23503 | `FOREIGN_KEY_VIOLATION` | missing or still referenced row | 422 (or 409 on delete) |
| 23502 | `NOT_NULL_VIOLATION` | a required value is missing | 422 |
| 23514 | `CHECK_VIOLATION` | a `CHECK` constraint (domains included) failed | 422 |
| 23P01 | `EXCLUSION_VIOLATION` | an exclusion constraint (overlapping ranges) failed | 409 |
| 22001 | `STRING_DATA_RIGHT_TRUNCATION` | value too long for `varchar(n)` | 422 |
| 22003 | `NUMERIC_VALUE_OUT_OF_RANGE` | a number overflowed its type | 422 |
| 22P02 | `INVALID_TEXT_REPRESENTATION` | text that does not convert (`'abc'::int`) | 422 |
| 22007, 22008 | `INVALID_DATETIME_FORMAT`, `DATETIME_FIELD_OVERFLOW` | a bad date or time | 422 |
| 22P04 | `BAD_COPY_FILE_FORMAT` | malformed `COPY` data | 422, with `where_()` |
| 21000 | `CARDINALITY_VIOLATION` | one row changed twice by `ON CONFLICT`/`MERGE` | 422 |
| 40001 | `T_R_SERIALIZATION_FAILURE` | serialization failure | retry, then 503 + `Retry-After` |
| 40P01 | `T_R_DEADLOCK_DETECTED` | deadlock | retry, then 503 + `Retry-After` |
| 55P03 | `LOCK_NOT_AVAILABLE` | `NOWAIT` or `lock_timeout` | retry, then 503 |
| 57014 | `QUERY_CANCELED` | `statement_timeout` or a cancel request | 503 (or 504) |
| 25P02 | `IN_FAILED_SQL_TRANSACTION` | a statement after an error in the transaction | 500 (a bug) |
| 25001 | `ACTIVE_SQL_TRANSACTION` | e.g. `CREATE INDEX CONCURRENTLY` inside a transaction | 500 |
| 25P01 | `NO_ACTIVE_SQL_TRANSACTION` | e.g. `DECLARE` outside a transaction | 500 |
| 25P03 | `IDLE_IN_TRANSACTION_SESSION_TIMEOUT` | session ended (FATAL) | 503 |
| 25P04 | (no constant in 0.7.18) | `transaction_timeout` (PG 17+), session ended | 503 |
| 34000 | `INVALID_CURSOR_NAME` | a portal or cursor that no longer exists | 500 |
| 0A000 | `FEATURE_NOT_SUPPORTED` | includes "cached plan must not change result type" | 500 |
| 42601, 42703, 42P01, 42883, 42804 | `SYNTAX_ERROR`, `UNDEFINED_COLUMN`, `UNDEFINED_TABLE`, `UNDEFINED_FUNCTION`, `DATATYPE_MISMATCH` | the SQL is wrong | 500 (a bug) |
| 42501 | `INSUFFICIENT_PRIVILEGE` | missing grant | 500 (a deployment bug) |
| 53300 | `TOO_MANY_CONNECTIONS` | `max_connections` reached | 503 |
| 57P01, 57P02, 57P03 | `ADMIN_SHUTDOWN`, `CRASH_SHUTDOWN`, `CANNOT_CONNECT_NOW` | the server is going away or starting | 503 |
| 08000, 08006 | `CONNECTION_EXCEPTION`, `CONNECTION_FAILURE` | connection problems | 503 |
| 08P01 | `PROTOCOL_VIOLATION` | e.g. a binary `COPY` with the wrong type size | 422 for `COPY` input, else 500 |
| 28000, 28P01 | `INVALID_AUTHORIZATION_SPECIFICATION`, `INVALID_PASSWORD` | rejected by `pg_hba.conf`, or a wrong password | at startup: fail fast |
| 55006 | `OBJECT_IN_USE` | e.g. cloning a template that has sessions | retry briefly |
| P0001 | `RAISE_EXCEPTION` | `RAISE EXCEPTION` in PL/pgSQL | depends on the function |

A connection that closed, or an error from the client side (a conversion), has no SQLSTATE at
all: `Error::code()` returns `None`, and `Error::is_closed()` tells the first case apart.

## D. Phrasebook: from sqlx and diesel

How common tasks read in each library. The sqlx column matches the illustrations in the
chapters; the diesel column shows diesel's query builder, with diesel-async for asynchronous
code (§F covers how diesel-async sits on tokio-postgres).

| Task | sqlx | tokio-postgres | Chapter |
|---|---|---|---|
| Connect | `PgPool::connect(url)` | `tokio_postgres::connect(url, tls)` + spawn the `Connection` | 1 |
| A pool | `PgPoolOptions::new().max_connections(8)` | deadpool-postgres `Pool::builder(manager).max_size(8)` | 13 |
| Run DDL | `sqlx::raw_sql(sql).execute(&pool)` | `client.batch_execute(sql)` | 2 |
| Insert, count rows | `sqlx::query(sql).bind(a).execute(&pool)` → `rows_affected()` | `client.execute(sql, &[&a])` → `u64` | 2 |
| One row into a struct | `query_as::<_, T>(sql).bind(a).fetch_one(&pool)` with `#[derive(FromRow)]` | `client.query_one(sql, &[&a])` + `TryFrom<&Row>` | 2, 3 |
| Zero or one row | `fetch_optional` | `query_opt` | 2 |
| A single value | `query_scalar(sql).fetch_one` | `query_one_scalar` (0.7.16+), or `query_one(…)?.get(0)` | 2 |
| Stream rows | `.fetch(&pool)` | `query_raw` → `RowStream` | 9 |
| Compile-time checked SQL | `query!`, `query_as!` macros | no macros; Cornucopia generates code ahead of time (§F) | 4, 14 |
| Prepared statements | cached implicitly per connection | explicit `prepare`, or deadpool's `prepare_cached` | 4, 13 |
| Concurrent queries | a pool: one connection per query in flight | pipelining on one connection (`join!`) | 7 |
| Transaction | `pool.begin()`, `tx.commit()` | `client.transaction()`, `tx.commit()`; rollback on drop in both | 8 |
| Isolation level | `SET TRANSACTION ISOLATION LEVEL …` as SQL | `build_transaction().isolation_level(…)` | 8 |
| `COPY FROM STDIN` | `conn.copy_in_raw(sql)`, `send`, `finish` | `copy_in` → `CopyInSink`, `send`, `finish`; binary rows with `BinaryCopyInWriter` | 10 |
| `LISTEN` | `PgListener::connect_with(&pool)`, `listen`, `recv` (reconnects itself) | drive the `Connection` with `poll_message` (reconnecting is yours) | 11 |
| Cancel a query | (see sqlx's documentation) | `cancel_token().cancel_query(tls)` | 12 |
| Error code | `err.as_database_error()?.code()` | `err.code() == Some(&SqlState::…)` | 6 |

| Task | diesel (with diesel-async for async) | tokio-postgres |
|---|---|---|
| Schema | `table!` macros generated by `diesel print-schema` | none: SQL text |
| A typed row | `#[derive(Queryable, Selectable)]` | `TryFrom<&Row>`, or `#[derive(FromSql)]` for composite values |
| A query | the query builder: `products.filter(sku.eq(x)).first(&mut conn)` | SQL text with `$1` parameters |
| Raw SQL | `diesel::sql_query(sql).bind::<Text, _>(x).load(&mut conn)` | `client.query(sql, &[&x])` |
| Migrations | `diesel_migrations::embed_migrations!` | your own (Chapter 14) or refinery |

The biggest conceptual differences, beyond syntax: tokio-postgres makes every round trip
visible (preparing, pipelining, portals), exposes the protocol features the others hide
(`COPY` as a sink, notifications as messages, cancel requests), and leaves the mapping between
rows and types to you.

## E. Glossary

**Backend.** The server process that serves one connection (one session). Its PID is
`pg_backend_pid()`, and a `Notification`'s `process_id()` names the sender's.

**Binary format.** The encoding of values that tokio-postgres uses for parameters and results,
and that binary `COPY` uses; as opposed to the text format of the simple protocol and text `COPY`.

**Cancel request.** A message sent on a new connection, naming a backend by PID and secret key,
asking it to stop its current statement (Chapter 12).

**Connection (tokio-postgres).** The half of a connection that does the IO: a future to spawn,
or to drive with `poll_message`. The other half is the `Client`.

**Cursor.** A portal created with SQL `DECLARE`, read with `FETCH` (Chapter 9).

**Domain.** A named type over a base type with constraints (`CREATE DOMAIN sku AS text CHECK
…`). Parameters that target a domain column are described as the domain (Chapter 5).

**Extended protocol.** Parse, Bind, Describe, Execute and Sync messages: typed parameters,
prepared statements and portals. What `query`, `execute` and friends use.

**Head-of-line blocking.** A request on a connection waiting behind a slow one ahead of it,
because a session runs one statement at a time (Chapter 7).

**MVCC.** Multi-version concurrency control: each transaction sees a snapshot of committed row
versions, so readers do not block writers (Chapter 8).

**Notice.** A non-error message from the server (`RAISE NOTICE`, "already exists, skipping"),
delivered as `AsyncMessage::Notice` (Chapter 11).

**Notification.** A message delivered to sessions that `LISTEN` on a channel, when a
transaction that ran `NOTIFY` commits (Chapter 11).

**Outbox.** A table of jobs written in the same transaction as the change they report, delivered
afterwards by a worker, at least once (Chapter 16).

**Pipelining.** Sending several requests on one connection before their answers arrive;
tokio-postgres does it whenever several futures are in flight on one client (Chapter 7).

**Pool.** A set of open connections lent out to tasks, one at a time each (Chapter 13).

**Portal.** The server-side state of an executing query, from which rows can be fetched in
batches; named portals live until the end of their transaction (Chapter 9).

**Prepared statement.** A parsed and described statement kept on the server for its session,
executed many times with different parameters (Chapter 4).

**Round trip.** One request and its answer over the network: the unit of latency that most of
this book counts.

**Savepoint.** A point inside a transaction to which it can roll back without ending (Chapter 8).

**Session.** The server-side state of one connection: settings, prepared statements, temporary
tables, `LISTEN`s, advisory locks.

**Simple protocol.** One `Query` message with SQL text, possibly several statements, no
parameters, results as text (Chapter 2).

**SQLSTATE.** The five-character error code of a server error, grouped in classes by its first
two characters (Chapter 6).

**SSI.** Serializable snapshot isolation: PostgreSQL's `SERIALIZABLE`, which detects
dangerous patterns of reads and writes and fails one transaction with 40001 (Chapter 8).

**Statement timeout.** The server-side limit on how long one statement may run
(`statement_timeout`), ending it with 57014 (Chapter 12).

**Sync.** The extended-protocol message that ends a request and makes the server answer
`ReadyForQuery`; every tokio-postgres request ends with one.

## F. The crates built on tokio-postgres

On 2026-11-06, crates.io listed 883 crates that depend on tokio-postgres directly. This section
covers the ones a reader of this book is most likely to meet, grouped by what they do, with
what each wraps or exposes, how it relates to the chapters, and when to choose it. Versions,
dates and download counts are crates.io's on that day; API facts come from each crate's
documentation and published source.

| Crate | Version (released) | Downloads | What it is |
|---|---|---:|---|
| `postgres` | 0.19.14 (2026-06-12) | 21.4 M | the synchronous wrapper |
| `postgres-types` | 0.2.14 (2026-06-12) | 89.8 M | `ToSql`/`FromSql`, the derives |
| `postgres-protocol` | 0.6.12 (2026-06-12) | 90.7 M | the wire protocol, SCRAM, escaping |
| `deadpool-postgres` | 0.14.2 (2026-08-26) | 28.2 M | a pool (Chapter 13) |
| `bb8-postgres` | 0.9.0 (2024-12-09) | 7.2 M | a pool |
| `mobc-postgres` | 0.9.0 (2025-06-25) | 0.1 M | a pool |
| `tokio-postgres-rustls` | 0.14.0 (2026-05-21) | 19.5 M | TLS with rustls (Chapter 12) |
| `tokio-postgres-rustls-improved` | 0.17.1 (2026-10-13) | 0.08 M | a fork of the above |
| `postgres-native-tls`, `postgres-openssl` | 0.5.3 (2026-03-30) | 10.5 M, 5.4 M | TLS with native-tls, OpenSSL |
| `diesel-async` | 0.9.2 (2026-06-19) | 17.0 M | Diesel's async connections |
| `toasty`, `toasty-driver-postgresql` | 0.11.0 (2026-09-25) | 0.09 M | the Tokio project's ORM |
| `refinery` | 0.10.0 (2026-10-02) | 12.8 M | migrations (Chapter 14) |
| `cornucopia` | 1.0.1 (2026-08-14) | 0.2 M | code generation from SQL (Chapter 14) |
| `postgres-from-row` | 0.5.2 (2023-05-26) | 0.4 M | `#[derive(FromRow)]` |
| `tokio-pg-mapper` | 0.2.0 (2021-01-04) | 0.3 M | `#[derive(PostgresMapper)]` |
| `rust_decimal` | 1.43.0 (2026-09-02) | 171.6 M | `numeric` as `Decimal` |
| `pgvector` | 0.4.2 (2026-05-22) | 21.8 M | the `vector` extension's types |

### The family: `postgres`, `postgres-types`, `postgres-protocol`

All three live in the [rust-postgres](https://github.com/rust-postgres/rust-postgres) repository
with tokio-postgres.

- **`postgres`** is "a lightweight wrapper over tokio-postgres": its `Client` holds a
  `tokio_postgres::Client` and a private current-thread Tokio runtime that drives the
  `Connection` with `poll_message`, and every method blocks on that runtime and takes `&mut self`.
  It re-exports tokio-postgres's `types`, `error`, `Statement`, `Portal`, `IsolationLevel` and
  `NoTls`, so the type and error chapters of this book apply unchanged. Choose it for
  synchronous programs (scripts, CLIs, threads); never call it from inside a Tokio runtime, where
  its `block_on` panics with "Cannot start a runtime from within a runtime". It has no
  `query_scalar` family in 0.19.14; `check_connection` arrived in 0.19.12 and the typed one-row
  methods in 0.19.13.
- **`postgres-types`** is where `ToSql`, `FromSql`, `Type` and `Json` live; `tokio_postgres::types`
  is a re-export of it. Depend on it directly for two reasons: the `derive` feature (Chapter 5),
  which only it has, and implementing the traits for your own types. Third-party crates do the
  same: `rust_decimal` and `pgvector` depend on `postgres-types`, not on tokio-postgres.
- **`postgres-protocol`** is the message layer underneath: frontend and backend messages, SCRAM
  authentication, the binary formats. It is "not typically used directly", with two useful
  exceptions: `password::scram_sha_256(password)` hashes a password on the client before an
  `ALTER ROLE … PASSWORD`, so the plain text is never sent to the server, and
  `escape::escape_identifier`, an alternative to pglisten's `quote_ident` for the identifiers that
  cannot be parameters.

### Pools

- **deadpool-postgres** is the pool of Chapter 13 and stockroom.
- **bb8-postgres**: `PostgresConnectionManager<Tls>::new(tokio_postgres::Config, tls)`, for the
  generic `bb8` pool; `pool.get()` returns a `PooledConnection` that derefs to
  `tokio_postgres::Client`. It spawns each `Connection` as a plain future, so the connection's
  error is dropped, and notices only logged (Chapter 11). bb8 tests every connection on checkout
  by default (`test_on_check_out`, an empty query: one round trip, like deadpool's `Verified`)
  and closes idle and old connections in the background. It has no statement cache. Its last
  release is from December 2024.
- **mobc-postgres**: `PgConnectionManager<Tls>::new(config, tls)` for the `mobc` pool, also
  lending `tokio_postgres::Client`s, with a health check (an empty query) on every checkout by
  default. Its feature list stops at `with-uuid-0_8` and `with-time-0_2`: enable newer ones on
  tokio-postgres directly. The crate's listed repository is archived; the code now lives in
  mobc's repository.

All three hand out plain `tokio_postgres::Client`s, so everything in Chapters 2–12 works on
their connections. The differences are deadpool's `prepare_cached` and recycling methods,
and how much checking happens on checkout (§13.3).

### TLS connectors

All implement `MakeTlsConnect<Socket>` (Chapter 12).

- **tokio-postgres-rustls** 0.14: `MakeRustlsConnect::new(rustls::ClientConfig)`, plus
  `with_webpki_roots()` and `with_native_certs()` behind features of the same names; you enable
  rustls's crypto provider (`ring` or `aws-lc-rs`) yourself. It sets no ALPN, so direct TLS needs
  `alpn_protocols` on the `ClientConfig` (§12.6). Channel binding follows the certificate's
  signature algorithm (SHA-256, SHA-384 or SHA-512), with none for Ed25519, for which RFC 5929
  defines no digest. **Use 0.14 or later:** in 0.13.0 a certificate-parsing mistake made channel
  binding silently absent for *every* certificate, so `channel_binding=require` failed and
  `prefer` quietly did without; 0.14.0 fixed it
  ([PR #32](https://github.com/jbg/tokio-postgres-rustls/pull/32)).
- **tokio-postgres-rustls-improved** is a fork created while that fix was waiting upstream. With
  the fix released, what it still adds is channel binding for ECDSA-with-SHA-512 certificates
  (with the `aws-lc-rs` provider), `aws-lc-rs` as the default provider, and a connector whose
  configuration can be reloaded at run time.
- **postgres-native-tls** (`MakeTlsConnector::new(native_tls::TlsConnector)`) uses the platform's
  TLS (Security.framework, SChannel or OpenSSL) and its trust store; **postgres-openssl**
  (`MakeTlsConnector::new(SslConnector)`, with `set_callback` for per-connection settings) uses
  OpenSSL. Both have `set_postgresql_alpn()`, "required when using `sslnegotiation=direct`", since
  0.5.1.

Several other rustls connectors exist (`postgres_rustls`, `rustls-tokio-postgres`,
`tokio-postgres-generic-rustls`); they do the same job with different defaults.

### Above the driver: ORMs, code generation, migrations

- **diesel-async** (feature `postgres`) gives Diesel 2.3 an asynchronous `AsyncPgConnection`,
  which wraps a `tokio_postgres::Client`. `AsyncPgConnection::establish(url)` connects without
  TLS; for TLS, connect with tokio-postgres yourself and call
  `AsyncPgConnection::try_from_client_and_connection(client, connection)`, which drives the
  `Connection` with `poll_message` and exposes its notifications. Its documentation says
  pipelining "happens automatically when futures are polled concurrently", as Chapter 7 showed for
  the client underneath, and `cancel_token()` returns tokio-postgres's `CancelToken`. Choose it
  when you want Diesel's compile-time-checked query builder in async code; its pools come from
  bb8, deadpool (the generic crate) or mobc, through `diesel_async::pooled_connection`.

  ~~~rust
  // diesel-async (illustration), adapted from its pooled-with-rustls example
  let tls = tokio_postgres_rustls::MakeRustlsConnect::new(client_config);
  let (client, connection) = tokio_postgres::connect(&url, tls).await?;
  let mut conn = AsyncPgConnection::try_from_client_and_connection(client, connection).await?;
  ~~~

- **toasty** (`features = ["postgresql"]`) is an asynchronous ORM from the Tokio project for
  SQLite, PostgreSQL, MySQL and DynamoDB. Its PostgreSQL driver holds a `tokio_postgres::Client`
  (`impl From<tokio_postgres::Client>` for its connection), prepares every statement with
  `prepare_typed` and caches up to 100, and uses tokio-postgres-rustls for TLS. It is young: its
  2024 announcement called it a preview, and it is at 0.11 with breaking changes in each release.
- **cornucopia** 1.0 (§14.6) generates a crate of typed query functions from annotated SQL. The
  generated code defines its own `GenericClient` trait, implemented for `tokio_postgres::Client`
  and `Transaction` and, with its default `deadpool` feature, for deadpool's `Client` and
  `Transaction`, where it uses `prepare_cached`; a `sync` mode targets `postgres` instead. It
  absorbed the Clorinde fork in 2026, and the separate `cornucopia_async` runtime crate is no
  longer needed.
- **refinery** (§14.3): with the `tokio-postgres` feature, `run_async` takes a `&mut
  tokio_postgres::Client` and runs each migration in a transaction of its own; it takes no lock.
  Note that its `tokio-postgres-rustls` feature still asks for tokio-postgres-rustls 0.13, the
  release with broken channel binding: connect yourself with 0.14 and pass the client in.

### Row mapping

Three crates derive the `TryFrom<&Row>` that Chapter 3 writes by hand. Check their dates first:

- **postgres-from-row** 0.5.2 (May 2023): `#[derive(FromRow)]` gives `from_row(&Row)`, which
  panics on a mismatch, and `try_from_row(&Row) -> Result<Self, tokio_postgres::Error>`, with
  `#[from_row(flatten)]`, `rename`, `from` and `try_from` attributes. Columns are matched by
  name.
- **tokio-pg-mapper** 0.2.0 (January 2021): `#[derive(PostgresMapper)]` with a
  `#[pg_mapper(table = "…")]` attribute, which is required (the derive panics without it, though
  the README's example leaves it out), and `from_row` / `from_row_ref`.
- **serde_postgres** 0.2.0 (2020) depends on tokio-postgres **0.5**, so its `from_row` cannot
  take a 0.7 `Row` at all.

For new code, a `TryFrom<&Row>` per type (Chapter 14) is a few lines and has no such risk;
`postgres-from-row` is the reasonable choice if a derive is wanted.

### Types that plug in through postgres-types

- **rust_decimal** with `db-tokio-postgres` (identical to `db-postgres`): `Decimal` for `numeric`.
  `NaN` and the infinities, which `numeric` can hold, fail to convert, as do values
  beyond its 96-bit mantissa (about 28 digits); digits beyond scale 28 are rounded away. For
  exact arbitrary-precision values, read `numeric` as text, or use a big-decimal crate.
- **pgvector** with `postgres`: `Vector`, `HalfVector` (feature `halfvec`), `SparseVector` and
  `Bit` for the extension's types, matched by type *name* because extension OIDs differ between
  databases, the same reason Chapter 5's derives match custom types by name.
- **fastnum** (feature `tokio-postgres`) is another decimal type with the same integration.

### Next to the driver, not on it

- **pgwire-replication** 0.5.0 is a logical-replication client (`pgoutput`) written directly on
  the wire protocol; it "intentionally avoids … `tokio-postgres`" for the replication path, and
  uses tokio-postgres only in its tests. tokio-postgres 0.7.18 has no replication protocol of its
  own (rust-postgres issue #116 is open), so change-data capture means a crate like this one, or
  polling and `LISTEN` (Chapter 11).
- **tokio-cron-scheduler** (feature `postgres_storage`) keeps its job schedule in PostgreSQL
  through tokio-postgres, and Cloudflare's **worker** crate implements tokio-postgres's
  `TlsConnect` for its sockets, so that Workers can use tokio-postgres directly: two examples of
  how far the driver travels beyond servers like stockroom.

## G. Further reading

- **tokio-postgres** documentation, 0.7.18:
  [docs.rs/tokio-postgres/0.7.18](https://docs.rs/tokio-postgres/0.7.18/tokio_postgres/), and
  its source and issues at [github.com/rust-postgres/rust-postgres](https://github.com/rust-postgres/rust-postgres).
- **postgres-types**: [docs.rs/postgres-types/0.2.14](https://docs.rs/postgres-types/0.2.14/postgres_types/),
  for `ToSql`, `FromSql` and the derives.
- **deadpool-postgres**: [docs.rs/deadpool-postgres/0.14.2](https://docs.rs/deadpool-postgres/0.14.2/deadpool_postgres/).
- **The PostgreSQL protocol**: [Frontend/Backend Protocol](https://www.postgresql.org/docs/18/protocol.html),
  especially [Message Flow](https://www.postgresql.org/docs/18/protocol-flow.html), which explains
  pipelining, `COPY`, cancel requests and TLS negotiation at the level this book works at.
- **Concurrency**: [Transaction Isolation](https://www.postgresql.org/docs/18/transaction-iso.html)
  and [Explicit Locking](https://www.postgresql.org/docs/18/explicit-locking.html).
- **Errors**: [PostgreSQL Error Codes](https://www.postgresql.org/docs/18/errcodes-appendix.html),
  the complete SQLSTATE list.
- **`COPY`**, **`NOTIFY`**, **`DECLARE`**: the SQL reference pages,
  [COPY](https://www.postgresql.org/docs/18/sql-copy.html),
  [NOTIFY](https://www.postgresql.org/docs/18/sql-notify.html),
  [DECLARE](https://www.postgresql.org/docs/18/sql-declare.html).
- **Server settings** used in the book: [Client Connection Defaults](https://www.postgresql.org/docs/18/runtime-config-client.html)
  (`statement_timeout`, `lock_timeout`, `idle_in_transaction_session_timeout`,
  `transaction_timeout`).
- **axum**: [docs.rs/axum/0.8.9](https://docs.rs/axum/0.8.9/axum/), and tower-http's
  [timeout](https://docs.rs/tower-http/0.7.1/tower_http/timeout/index.html) and
  [limit](https://docs.rs/tower-http/0.7.1/tower_http/limit/index.html) layers.
- **Release notes** of the PostgreSQL versions whose features the book uses:
  [15](https://www.postgresql.org/docs/release/15.0/) (`MERGE`, `HEADER MATCH`),
  [16](https://www.postgresql.org/docs/release/16.0/) (`pg_input_is_valid`),
  [17](https://www.postgresql.org/docs/release/17.0/) (`ON_ERROR`, `MERGE … RETURNING`,
  `transaction_timeout`, direct TLS),
  [18](https://www.postgresql.org/docs/release/18.0/) (`uuidv7()`, virtual generated columns,
  `RETURNING old/new`, `REJECT_LIMIT`).

---

Previous: [Chapter 18 — Capstone: stockroom end to end](18-capstone.md)
