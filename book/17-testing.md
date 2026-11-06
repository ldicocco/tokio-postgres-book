# Chapter 17 — Testing against a real database

> **Async PostgreSQL in Rust** · Previous: [Chapter 16 — axum II: streaming, real-time and background work](16-axum-streaming.md) ·
> Next: [Chapter 18 — Capstone: stockroom end to end](18-capstone.md)

The code in this book is mostly SQL and the glue around it, and most of its bugs live in the
glue's assumptions about the SQL: that a constraint has the name the error mapping expects, that
a transaction really rolls back, that fifty concurrent orders cannot sell twenty-one chairs. A
mock of the database can confirm none of that, because it answers whatever the test told it
to. So stockroom's tests run against PostgreSQL itself, and this chapter is about making that
fast, isolated and reliable.

It covers what to test at which layer, two ways to isolate tests from each other (a database
per test, and a transaction per test, with the second one's limits), testing the HTTP layer in
memory, testing concurrency on purpose rather than by luck, and running PostgreSQL in CI. It
ends by provoking each error code the error mapping handles.

---

## 17.1 What to test where

| Layer | What can go wrong | How stockroom tests it | Database? |
|---|---|---|---|
| Pure functions | parsing, checksums, cursors, retry delays | `#[test]` unit tests next to the code | no |
| Types | `ToSql`/`FromSql` against the real column types, domains, enums | `tests/types.rs`: round trips through the database | yes |
| Repository and migrations | SQL that does not parse, wrong columns, constraint names, transactional DDL | through the HTTP tests, plus `tests/migrations.rs` | yes |
| HTTP | status codes, error bodies, headers, streaming | `tests/*.rs` with `oneshot` (§17.4) | yes |
| Concurrency | overselling, deadlocks, lost updates | `tests/concurrency.rs` with many tasks at once (§17.5) | yes |

Most of stockroom's tests are at the HTTP layer, and that is deliberate. A test that sends
`POST /orders` and checks the status, the body and the stock levels afterwards exercises the
extractor, the validation, the transaction, the SQL, the constraints and the error mapping in
one go, through the same interface a client uses, and it survives refactoring of everything
behind that interface. Repository functions get dedicated tests only where the HTTP layer
cannot reach them easily.

Run them with:

```bash
cargo test -p stockroom                    # 10 unit tests, 35 against PostgreSQL
cargo test -p stockroom --test concurrency # just the concurrency tests
```

```text
running 3 tests
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.87s
```

(The concurrency binary, one of eight, measured with PG 18.6 on localhost. Each finishes in under a
second, because tests within a binary run in parallel, each on a database of its own.)

## 17.2 A database per test, cloned from a template

Tests that share a database see each other's rows, and then a test passes or fails depending on
which ran first. The cleanest isolation is a fresh database per test, and PostgreSQL can make one
in milliseconds: `CREATE DATABASE new TEMPLATE old` copies an existing database, schema and
data (block by block through the WAL, the default strategy since PG 15), without running a
single migration. So the harness migrates one *template*
database once, then clones it for every test:

~~~rust
// stockroom/tests/common/mod.rs (excerpt)
pub async fn new() -> TestDb {
    let template = template();
    let name = format!("stockroom_test_{}", Uuid::now_v7().simple());
    let (admin, task) = connect(&config_for("postgres")).await;
    // A session that has just left the template can linger for a moment; cloning
    // fails with 55006 (object in use) until it is gone.
    let mut attempts = 0;
    loop {
        match admin
            .batch_execute(&format!(
                "CREATE DATABASE \"{name}\" TEMPLATE \"{template}\""
            ))
            .await
        {
            Ok(()) => break,
            Err(e) if e.code() == Some(&SqlState::OBJECT_IN_USE) && attempts < 50 => {
                attempts += 1;
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(e) => panic!("cannot create {name}: {e}"),
        }
    }
    // …
~~~

[stockroom/tests/common/mod.rs](../stockroom/tests/common/mod.rs)

The details are where the time went when building it:

- **The template's name includes a checksum of all migrations**
  (`stockroom_template_<checksum>`). Change a migration and the next test run builds a new
  template and drops the old ones; leave the migrations alone and every run reuses the same
  template.
- **Only one process builds it.** Cargo runs test binaries one after another, but a developer
  may run two `cargo test`s at once; an advisory lock (Chapter 14) makes the second wait.
- **The template must have no sessions when it is cloned**, or `CREATE DATABASE … TEMPLATE`
  fails with 55006 (`object_in_use`). The harness waits for its own migration connection's task
  to end, and retries the clone briefly for sessions that linger.
- **It is built outside any test's runtime.** Each `#[tokio::test]` has a runtime of its own,
  and a connection spawned on one test's runtime dies with it. The template is built once, in a
  `OnceLock`, on a thread with a runtime of its own.
- **Names are generated, never input**, which is why `format!` into `CREATE DATABASE` is
  acceptable here: identifiers cannot be parameters, and a UUID is a safe identifier.

### Cleaning up without async `Drop`

A test database must be dropped when the test ends, whether it passed or panicked, and the
natural place is `Drop`. But `Drop::drop` cannot `.await`, and it may run on a Tokio worker
thread, where blocking on a future with `Handle::block_on` panics. The way out is a thread of
its own, with a runtime of its own, which the drop waits for. The same technique, applied to a
schema instead of a database:

```rust
use std::sync::atomic::{AtomicU32, Ordering};
use tokio_postgres::Client;

/// A schema for one test, dropped when the guard is, even if the test panics.
struct TestSchema {
    name: String,
}

impl TestSchema {
    async fn new(client: &Client) -> Result<TestSchema, tokio_postgres::Error> {
        // Generated from the process id and a counter: no input reaches the SQL.
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let name = format!(
            "test_{}_{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        client
            .batch_execute(&format!("CREATE SCHEMA {name}"))
            .await?;
        Ok(TestSchema { name })
    }
}

impl Drop for TestSchema {
    /// `Drop` cannot await, and it may run inside a runtime, where blocking on a future
    /// panics. So the cleanup runs on a thread with a runtime of its own, and drop waits.
    fn drop(&mut self) {
        let sql = format!("DROP SCHEMA IF EXISTS {} CASCADE", self.name);
        let _ = std::thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("a runtime")
                .block_on(async move {
                    let client = playground::connect().await?;
                    client.batch_execute(&sql).await
                })
        })
        .join();
    }
}

async fn exists(client: &Client, name: &str) -> Result<bool, tokio_postgres::Error> {
    let row = client
        .query_one(
            "SELECT EXISTS (SELECT FROM pg_namespace WHERE nspname = $1)",
            &[&name],
        )
        .await?;
    Ok(row.get(0))
}

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = playground::connect().await?;

    // A test that passes.
    let name = {
        let schema = TestSchema::new(&client).await?;
        client
            .batch_execute(&format!("CREATE TABLE {}.item (id int)", schema.name))
            .await?;
        schema.name.clone()
    }; // dropped here
    assert!(!exists(&client, &name).await?);

    // A test that panics: the guard is dropped while the task unwinds.
    let schema = TestSchema::new(&client).await?;
    let name = schema.name.clone();
    let outcome = tokio::spawn(async move {
        let _schema = schema;
        assert_eq!(1 + 1, 3, "a failing assertion");
    })
    .await;
    assert!(outcome.is_err());
    assert!(!exists(&client, &name).await?);
    Ok(())
}
```

(Running it prints the failing assertion's panic message, from the task that was meant to fail.)
stockroom's `TestDb` does exactly this with `DROP DATABASE … WITH (FORCE)`, where `FORCE`
(PG 13+) ends the sessions the test's pool may still hold.

## 17.3 A transaction per test, and its limits

The other common isolation technique opens a transaction at the start of each test, runs the test
inside it, and rolls it back at the end: nothing to create, nothing to clean up, and very fast.
It works for code that takes a `&impl GenericClient` (Chapter 14), which can be handed the test's
transaction. It has blind spots, though, and they are exactly the behaviours this book cares
about:

```rust
use std::time::Duration;
use tokio_postgres::error::SqlState;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (mut test, mut messages) = playground::connect_with_messages().await?;
    let mut other = playground::connect().await?;
    // A real table, visible to both sessions, in a schema of our own.
    let schema = format!("ch17_{}", std::process::id());
    test.batch_execute(&format!(
        "CREATE SCHEMA {schema};
         CREATE TABLE {schema}.item (id int CONSTRAINT item_id_key UNIQUE DEFERRABLE INITIALLY DEFERRED);"
    ))
    .await?;
    for client in [&test, &other] {
        client
            .batch_execute(&format!("SET search_path = {schema}"))
            .await?;
    }
    test.batch_execute("LISTEN items").await?;

    {
        // The test, in a transaction that will be rolled back.
        let tx = test.transaction().await?;
        // A duplicate, but the constraint is deferred: no error until COMMIT.
        tx.execute("INSERT INTO item VALUES (1), (1)", &[]).await?;
        tx.execute("SELECT pg_notify('items', 'added')", &[])
            .await?;
        let mine: i64 = tx.query_one("SELECT count(*) FROM item", &[]).await?.get(0);
        // Any other connection, such as a second pooled one, sees nothing.
        let theirs: i64 = other
            .query_one("SELECT count(*) FROM item", &[])
            .await?
            .get(0);
        assert_eq!((mine, theirs), (2, 0));
    } // rolled back: the test "passed"

    // The notification never left.
    let notified = tokio::time::timeout(Duration::from_millis(200), messages.recv()).await;
    assert!(notified.is_err(), "a rolled-back NOTIFY is never delivered");

    // The same work, committed, fails: at COMMIT, which the rolled-back test never reached.
    let tx = other.transaction().await?;
    tx.execute("INSERT INTO item VALUES (1), (1)", &[]).await?;
    let err = tx.commit().await.unwrap_err();
    println!(
        "at commit: {}",
        err.as_db_error().expect("a server error").message()
    );
    assert_eq!(err.code(), Some(&SqlState::UNIQUE_VIOLATION));

    test.batch_execute(&format!("DROP SCHEMA {schema} CASCADE"))
        .await?;
    Ok(())
}
```

```text
at commit: duplicate key value violates unique constraint "item_id_key"
```

So a transaction per test cannot test:

- **anything that happens at commit**: deferred constraints, `NOTIFY`, the outbox trigger's
  effects as other sessions see them;
- **anything with more than one connection**: the pool, concurrency, a listener, a second
  request that should see the first one's write;
- **code that manages its own transactions**, such as `in_transaction`'s retries, since a nested
  `BEGIN` is a warning and not a transaction, and `COMMIT` would end the test's transaction.

That is most of Part II. stockroom therefore uses a database per test throughout; a transaction
per test remains a good fit for a large suite of plain repository queries.

## 17.4 HTTP tests with `oneshot`

The router is a `tower::Service`, so a test can call it like a function: build a `Request`, call
`oneshot`, read the `Response`. No port, no HTTP client, no server task: Chapters 15 and 16 have
been doing it all along with `playground::call`. stockroom's harness wraps it in a `TestApp`
with helpers for its fixtures, created through the API itself so that fixtures cannot drift from
what clients can do:

~~~rust
// stockroom/tests/orders.rs (excerpt)
#[tokio::test]
async fn an_order_for_more_than_is_available_reserves_nothing() {
    let db = TestDb::new().await;
    let app = shop(&db).await;
    // …
~~~

~~~rust
// stockroom/tests/common/mod.rs (excerpt)
pub async fn order(&self, store: &str, lines: &[(&str, i32)]) -> Reply {
    let lines: Vec<Value> = lines
        .iter()
        .map(|(sku, q)| json!({ "sku": sku, "quantity": q }))
        .collect();
    self.post(
        "/orders",
        json!({ "store": store, "customer": { "name": "Ada", "email": "ada@example.com" }, "lines": lines }),
    )
    .await
}
~~~

[stockroom/tests/common/mod.rs](../stockroom/tests/common/mod.rs) ·
[stockroom/tests/orders.rs](../stockroom/tests/orders.rs)

The router under test is the real one, `stockroom::http::router`, on a pool connected to the
test's database. Tests that need the listener or the outbox worker start them explicitly
(`tests/events.rs`), so that the others do not pay for background tasks they do not use.

## 17.5 Concurrency tests

The bugs that matter most in a service like stockroom appear only when requests overlap, and
they will not show up in a test that sends one request at a time. A concurrency test creates the
overlap on purpose: many tasks at once, on a multi-threaded runtime, against a pool with more
than one connection, followed by an assertion about the *total* outcome:

~~~rust
// stockroom/tests/concurrency.rs (excerpt)
/// 20 chairs, 50 customers each wanting one, all at the same moment: exactly 20 orders.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn fifty_customers_twenty_chairs() {
    let db = TestDb::new().await;
    let app = db.app(16);
    app.store("mitte").await;
    app.product("CHAIR-01", 4999).await;
    app.stock("mitte", "CHAIR-01", 20).await;

    let attempts = (0..50).map(|_| {
        let app = app.clone();
        tokio::spawn(async move { app.order("mitte", &[("CHAIR-01", 1)]).await.status })
    });
    let statuses: Vec<StatusCode> = join_all(attempts)
        .await
        .into_iter()
        .map(Result::unwrap)
        .collect();
    // …
    assert_eq!((created, refused), (20, 30), "{statuses:?}");
    assert_eq!(app.level("mitte", "CHAIR-01").await, (20, 20, 0));
}
~~~

[stockroom/tests/concurrency.rs](../stockroom/tests/concurrency.rs)

Two practices make such tests trustworthy. **Repeat them**: a race that shows up once in fifty
runs passes almost every time, so while building stockroom the oversell test ran 50 times in a
loop, and passed all 50. And **force the interleaving when you can**: instead of hoping two
transactions overlap, drive two connections step by step, so that the conflict happens on every
run. §17.7's example does that for a serialization failure and a deadlock.

`tests/concurrency.rs` also checks that orders listing the same two products the other way round
never deadlock (stockroom locks stock rows in a fixed order, Chapter 8), and that 40 concurrent
transfers between two stores lose no stock.

## 17.6 PostgreSQL in CI: a service, or testcontainers

The tests need a PostgreSQL server, and two setups are common.

**A service container.** The CI system starts PostgreSQL next to the job, and the tests connect
to it through an environment variable, exactly as on a developer's machine. In GitHub Actions:

```yaml
jobs:
  test:
    runs-on: ubuntu-latest
    services:
      postgres:
        image: postgres:18
        env:
          POSTGRES_PASSWORD: postgres
        ports: ["5432:5432"]
        options: >-
          --health-cmd pg_isready --health-interval 2s --health-timeout 3s --health-retries 30
    env:
      ADMIN_DATABASE_URL: postgres://postgres:postgres@localhost:5432/postgres
    steps:
      - uses: actions/checkout@v4
      - run: cargo test --workspace
```

This is what stockroom's harness expects: `ADMIN_DATABASE_URL` names a server and an account
that may create databases.

**testcontainers.** The test itself starts a throwaway container through the Docker API, and
connects to the port it was given. [testcontainers-modules](https://docs.rs/testcontainers-modules/0.15.0/testcontainers_modules/postgres/struct.Postgres.html)
(0.15.0, on testcontainers 0.27; checked 2026-11-06) has a ready-made PostgreSQL image, with
`postgres` as the user, password and database by default:

~~~rust
// testcontainers (illustration): testcontainers-modules = { version = "0.15", features = ["postgres"] }
use testcontainers_modules::postgres::Postgres;
use testcontainers_modules::testcontainers::runners::AsyncRunner;

let container = Postgres::default().start().await?;
let port = container.get_host_port_ipv4(5432).await?;
let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
let (client, connection) = tokio_postgres::connect(&url, NoTls).await?;
// … the container stops when `container` is dropped
~~~

It needs nothing from the CI configuration but Docker, and each test binary can have a server of
its own. The price is startup time, seconds per container rather than milliseconds per cloned
database; combine the two (one container per test binary, a template database cloned per test)
to get both isolation and speed.

## 17.7 Provoking every SQLSTATE the error mapping handles

Chapter 15's `AppError` turns SQLSTATEs into HTTP statuses. Each arm of that `match` is code,
and code that never runs in a test is code you hope works. Every one of these errors is easy to
provoke on purpose, and even the concurrent ones are deterministic if the test drives the
interleaving itself:

```rust
use std::time::Duration;
use tokio_postgres::Client;

type Error = Box<dyn std::error::Error>;

/// The SQLSTATE of the error `result` holds; panics if it succeeded.
fn code<T>(result: Result<T, tokio_postgres::Error>) -> String {
    match result {
        Ok(_) => panic!("expected an error"),
        Err(e) => e
            .code()
            .map(|c| c.code().to_owned())
            .unwrap_or_else(|| e.to_string()),
    }
}

async fn session(schema: &str) -> Result<Client, Error> {
    let client = playground::connect().await?;
    client
        .batch_execute(&format!("SET search_path = {schema}"))
        .await?;
    Ok(client)
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let schema = format!("ch17_codes_{}", std::process::id());
    let setup = playground::connect().await?;
    setup
        .batch_execute(&format!(
            "CREATE SCHEMA {schema};
             SET search_path = {schema};
             CREATE TABLE store (id int PRIMARY KEY);
             CREATE TABLE stock (
                 id    int PRIMARY KEY,
                 store int NOT NULL REFERENCES store,
                 units int NOT NULL CHECK (units >= 0)
             );
             INSERT INTO store VALUES (1);
             INSERT INTO stock VALUES (1, 1, 10), (2, 1, 10);"
        ))
        .await?;
    let (mut a, mut b) = (session(&schema).await?, session(&schema).await?);
    let mut seen = Vec::new();

    // Integrity: 23505, 23514, 23502, 23503.
    seen.push((
        "23505",
        code(a.execute("INSERT INTO stock VALUES (1, 1, 1)", &[]).await),
    ));
    seen.push((
        "23514",
        code(
            a.execute("UPDATE stock SET units = -1 WHERE id = 1", &[])
                .await,
        ),
    ));
    seen.push((
        "23502",
        code(
            a.execute("INSERT INTO stock VALUES (3, NULL, 1)", &[])
                .await,
        ),
    ));
    seen.push((
        "23503",
        code(a.execute("INSERT INTO stock VALUES (3, 9, 1)", &[]).await),
    ));

    // 55P03: a lock that is not available, without waiting.
    let hold = a.transaction().await?;
    hold.execute("SELECT * FROM stock WHERE id = 1 FOR UPDATE", &[])
        .await?;
    seen.push((
        "55P03",
        code(
            b.execute("SELECT * FROM stock WHERE id = 1 FOR UPDATE NOWAIT", &[])
                .await,
        ),
    ));
    hold.rollback().await?;

    // 57014: a statement that runs out of time.
    b.batch_execute("SET statement_timeout = '50ms'").await?;
    seen.push(("57014", code(b.execute("SELECT pg_sleep(1)", &[]).await)));
    b.batch_execute("SET statement_timeout = 0").await?;

    // 40001: write skew under SERIALIZABLE, interleaved step by step.
    {
        let serializable = tokio_postgres::IsolationLevel::Serializable;
        let ta = a
            .build_transaction()
            .isolation_level(serializable)
            .start()
            .await?;
        let tb = b
            .build_transaction()
            .isolation_level(serializable)
            .start()
            .await?;
        ta.query("SELECT sum(units) FROM stock", &[]).await?;
        tb.query("SELECT sum(units) FROM stock", &[]).await?;
        ta.execute("UPDATE stock SET units = units - 1 WHERE id = 1", &[])
            .await?;
        // The conflict is reported by the second UPDATE or by the second COMMIT.
        let second = match tb
            .execute("UPDATE stock SET units = units - 1 WHERE id = 2", &[])
            .await
        {
            Ok(_) => {
                ta.commit().await?;
                tb.commit().await
            }
            Err(e) => Err(e),
        };
        seen.push(("40001", code(second)));
    }

    // 40P01: two transactions that each wait for the other's row.
    {
        let ta = a.transaction().await?;
        let tb = b.transaction().await?;
        ta.execute("UPDATE stock SET units = units WHERE id = 1", &[])
            .await?;
        tb.execute("UPDATE stock SET units = units WHERE id = 2", &[])
            .await?;
        let (first, second) = tokio::join!(
            ta.execute("UPDATE stock SET units = units WHERE id = 2", &[]),
            async {
                tokio::time::sleep(Duration::from_millis(100)).await;
                tb.execute("UPDATE stock SET units = units WHERE id = 1", &[])
                    .await
            }
        );
        // PostgreSQL picks one victim, whose transaction aborts and releases its locks; the
        // other then gets the lock it was waiting for.
        seen.push(("40P01", code(first.and(second))));
    }

    for (expected, got) in &seen {
        println!("{expected}: {got}");
        assert_eq!(expected, got);
    }
    setup
        .batch_execute(&format!("DROP SCHEMA {schema} CASCADE"))
        .await?;
    Ok(())
}
```

```text
23505: 23505
23514: 23514
23502: 23502
23503: 23503
55P03: 55P03
57014: 57014
40001: 40001
40P01: 40P01
```

The deadlock takes about a second: PostgreSQL looks for deadlocks only after a lock wait reaches
`deadlock_timeout` (1 s by default). stockroom's own tests provoke errors through the API, and
check the statuses a client sees: 409 for the duplicate SKU, 422 with the field for the check
violations, 412 for a stale `If-Match`, and 503 with `timeout` for the statement timeout
(`tests/reports.rs`).

---

## In stockroom

| File | Tests | What it covers |
|---|---|---|
| `src/**` (unit) | 10 | SKU parsing and its serde, cents arithmetic, order-status transitions, migration checksums and order, cursors, retry backoff |
| `tests/types.rs` | 5 | domains, enums (and a new label), composites, arrays |
| `tests/migrations.rs` | 5 | idempotence, edited and unknown migrations, rollback, two instances at once |
| `tests/products.rs` | 8 | errors a client can cause, `If-Match`, pagination, savepoints, CSV in and out, NDJSON, a client that hangs up mid-stream |
| `tests/orders.rs` | 5 | pricing and reservations, transitions, cancellation, 404 |
| `tests/concurrency.rs` | 3 | overselling, deadlock-free ordering, transfers |
| `tests/events.rs` | 6 | `NOTIFY` on commit only, reconnect and resync, SSE, outbox, health |
| `tests/reports.rs` | 3 | the dashboard, cancellation of an abandoned report, statement timeout |

All of them run against a database cloned from the migrated template, and leave nothing behind.

## Summary

- Test against PostgreSQL itself: mocks cannot check constraint names, transactional behaviour or
  concurrency. Test mostly through the HTTP layer, in memory with `oneshot`.
- A database per test, cloned with `CREATE DATABASE … TEMPLATE` from a template migrated once
  (named after a checksum of the migrations), gives full isolation in milliseconds. Retry the
  clone on 55006 while sessions linger.
- `Drop` cannot await: clean up from a thread with a runtime of its own, and wait for it.
- A transaction per test is fast but blind to commit-time behaviour, `NOTIFY`, several
  connections and code that manages its own transactions.
- Concurrency tests run many tasks at once on a multi-threaded runtime and assert the total;
  repeat them, and force interleavings step by step where you can.
- In CI, use a service container with `ADMIN_DATABASE_URL`, or testcontainers for a server per
  test binary.
- Provoke every SQLSTATE the error mapping handles, and check the resulting statuses.

## Exercises

1. **Template versus migrations.** Measure creating a test database by cloning stockroom's
   template, and by creating an empty one and running the migrations. Which grows with the size
   of the schema, and which with the size of the data in the template?
   *Hint:* time `CREATE DATABASE … TEMPLATE` and `CREATE DATABASE` plus `migrate::run`.
2. **Break the guard.** In stockroom's `orders::place`, replace the guarded reservation
   (`… AND available >= $3`) with a read-then-write: `SELECT available`, check it in Rust, then an
   unguarded `UPDATE`. Run `fifty_customers_twenty_chairs` a few times. What fails, and what
   keeps the data correct anyway?
   *Hint:* look at the `CHECK` constraints on `stock` in `0001_init.sql`.
3. **57P01.** Add a case to §17.7's example for `admin_shutdown`: a query that is running when
   its backend is terminated. What does the client see, and what does stockroom's `AppError` make
   of it?
   *Hint:* run `pg_sleep` in a spawned task, then `pg_terminate_backend` from another session.
4. **Fixtures through SQL.** stockroom creates fixtures through the API. When would inserting
   them with SQL be the better choice?
   *Hint:* think of a report over a year of data.

### Solutions

**1.** Cloning copies the template's blocks, so its time grows with the size of its data and is nearly
independent of how many migrations built it; running migrations grows with the number and cost
of the migrations, and is independent of data (an empty database has none). For stockroom both
are fast, and the template wins by skipping the DDL; the gap widens as migrations accumulate.

**2.** At `READ COMMITTED`, two transactions can both read `available = 1` and both decide to
reserve, and the unguarded `UPDATE`s then raise `reserved` above `on_hand`. The constraint
`stock_reserved_within_on_hand` (`reserved <= on_hand`) refuses the second one with 23514, so no
chair is sold twice. But `AppError` maps that check violation to 422, and the test, which accepts
only 201 and 409, fails whenever two orders interleave that way; with fifty tasks on sixteen
connections, that is likely. The guarded `UPDATE` makes the check and the write one atomic statement,
re-evaluated after any lock wait (Chapter 8), so the race cannot happen; the constraint is the
safety net that turns the bug into a wrong status instead of oversold stock.

**3.** The running query fails with a `DbError` of severity `FATAL` and code 57P01 ("terminating
connection due to administrator command"), and the connection is closed afterwards. stockroom maps
`ADMIN_SHUTDOWN` to `AppError::Unavailable`, a 503, and the pool discards the closed connection
on its next checkout:

```rust
use std::sync::Arc;
use std::time::Duration;
use tokio_postgres::error::SqlState;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let victim = Arc::new(playground::connect().await?);
    let admin = playground::connect().await?;
    let pid: i32 = victim
        .query_one("SELECT pg_backend_pid()", &[])
        .await?
        .get(0);

    let running = {
        let victim = Arc::clone(&victim);
        tokio::spawn(async move { victim.execute("SELECT pg_sleep(5)", &[]).await })
    };
    tokio::time::sleep(Duration::from_millis(100)).await;
    admin
        .execute("SELECT pg_terminate_backend($1)", &[&pid])
        .await?;

    let err = running.await?.unwrap_err();
    let db = err.as_db_error().expect("a server error");
    println!("{} {}: {}", db.severity(), db.code().code(), db.message());
    assert_eq!(db.code(), &SqlState::ADMIN_SHUTDOWN);
    Ok(())
}
```

**4.** When the API would make the fixture slow or impossible to build: a report test needs a
year of orders with dates in the past, which no endpoint creates (`POST /orders` stamps the
current time). Insert those rows with SQL, or binary `COPY` as `stockroom seed` does, and keep
creating through the API the fixtures that the API can create.

---

Previous: [Chapter 16 — axum II: streaming, real-time and background work](16-axum-streaming.md) ·
Next: [Chapter 18 — Capstone: stockroom end to end](18-capstone.md)
