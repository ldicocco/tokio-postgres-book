# Chapter 6 — Errors

> **Async PostgreSQL in Rust** · Previous: [Chapter 5 — Custom types: ToSql and FromSql](05-custom-types.md) ·
> Next: [Chapter 7 — One connection, many tasks: pipelining](07-pipelining.md)

Every chapter so far has produced errors: a refused connection, a wrong type, a missing row, a
violated constraint. They all arrive as the same Rust type, `tokio_postgres::Error`, and the
first thing to learn is that its `Display` text almost never tells you what happened. The
information is there, one level down, in a structure the server filled in with remarkable
care: a five-character SQLSTATE code, the constraint and table involved, a detail, a hint, the
position of a syntax error.

This chapter takes the error apart: its causes, the server's `DbError` and its fields, and the
`SqlState` codes worth matching on. Then it builds what an application needs on top: one error
type for your program, classified once, with messages a user can be shown and details that go
only to the log. It ends with two things that are not errors in the usual sense: a transaction
that refuses to continue after one, and notices.

---

## 6.1 One type, many causes

`tokio_postgres::Error` covers everything that can go wrong, from a refused TCP connection to a
deadlock detected by the server. Internally it records a *kind* and an optional *cause*. The
kind is private, so what you can ask an error is limited to four questions:

| Method | Answers |
|---|---|
| `as_db_error()` | Did the server report this? Then here is its `DbError` |
| `code()` | The SQLSTATE, if the server reported it (a shortcut through `as_db_error`) |
| `is_closed()` | Is it because the connection is closed? |
| `source()` (from `std::error::Error`) | The underlying cause: the `DbError`, an `io::Error`, a conversion error |

Here is what each kind of failure looks like from the outside, as observed on PostgreSQL 18:

| What happened | `Display` | `code()` | `source()` |
|---|---|---|---|
| A constraint, a syntax error, a deadlock: anything the server reports | `db error` | the SQLSTATE | the server's message, formatted as `ERROR: … DETAIL: …` |
| The connection was refused | `error connecting to server` | — | `Connection refused (os error 61)` |
| The connection is closed | `connection closed` | — | — |
| A value did not fit the column's type (reading) | `error deserializing column 0` | — | `cannot convert between the Rust type … and the Postgres type …` |
| A value did not fit the parameter's type (sending) | `error serializing parameter 0` | — | the same "cannot convert" |
| No such column | ``invalid column `x` `` | — | — |
| Wrong number of parameters | `expected 1 parameters but got 0` | — | — |
| `query_one`/`query_opt` saw the wrong number of rows | `query returned an unexpected number of rows` | — | — |
| A connection string that does not parse | `invalid connection string` | — | ``invalid value for option `port` `` |

So `Display` is a category, and the explanation is the cause. To see both, walk the chain:

```rust
use std::error::Error;

/// An error and every cause under it.
fn report(error: &dyn Error) -> String {
    let mut text = error.to_string();
    let mut cause = error.source();
    while let Some(c) = cause {
        text.push_str(&format!("\n  caused by: {c}"));
        cause = c.source();
    }
    text
}

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE products (sku text PRIMARY KEY, price_cents bigint);
             INSERT INTO products VALUES ('CHAIR-01', 4999);",
        )
        .await?;

    // From the server: the DbError is the cause.
    let duplicate = client
        .execute("INSERT INTO products VALUES ('CHAIR-01', 1)", &[])
        .await
        .unwrap_err();
    let text = report(&duplicate);
    assert!(text.starts_with("db error\n  caused by: ERROR: duplicate key value"));
    assert!(text.contains("DETAIL: Key (sku)=(CHAIR-01) already exists."));

    // From the client: the conversion error is the cause.
    let row = client
        .query_one("SELECT price_cents FROM products", &[])
        .await?;
    let wrong = row.try_get::<_, i32>(0).unwrap_err();
    assert_eq!(
        report(&wrong),
        "error deserializing column 0\n  caused by: cannot convert between \
         the Rust type `i32` and the Postgres type `int8`"
    );

    // Some errors have no cause at all: the category is the whole story.
    let none = client
        .query_one("SELECT 1 WHERE false", &[])
        .await
        .unwrap_err();
    assert_eq!(report(&none), "query returned an unexpected number of rows");
    assert!(none.as_db_error().is_none() && none.code().is_none());
    Ok(())
}
```

Logging `{error:?}` (`Debug`) shows the kind and the cause too, in a more technical form:
`Error { kind: FromSql(0), cause: Some(WrongType { postgres: Int8, rust: "i32" }) }`. Either is
fine for a log; `{error}` alone is not.

> **Pitfall:** there is no public way to ask "was this the row-count error?" or "was this a
> conversion error?". Only the server's errors have a code. Design so that you do not need to
> ask: use `query_opt` and handle `None` instead of catching `query_one`'s failure, and treat a
> conversion error as the bug it is.

## 6.2 `DbError`: the server's report

When the server reports an error, it sends far more than a message, and `DbError` keeps all of
it. The fields that matter most:

| Method | Content |
|---|---|
| `code()` | The SQLSTATE: five characters, the first two being the class (§6.3) |
| `message()` | The primary message, one line, meant for a person |
| `detail()` | More detail, often with the offending values: `Key (sku)=(CHAIR-01) already exists.` |
| `hint()` | A suggestion for fixing it, when the server has one |
| `severity()` / `parsed_severity()` | `ERROR` or `FATAL` (the session is over) or `PANIC`, as text or as a `Severity` |
| `position()` | For an error in the statement's text: where, as a 1-based character position |
| `schema()`, `table()`, `column()`, `datatype()`, `constraint()` | What the error is about, by name |
| `where_()` | The call stack, for errors raised inside functions and triggers |
| `routine()`, `file()`, `line()` | Where in PostgreSQL's own source the error was raised |

```rust
use tokio_postgres::error::{ErrorPosition, Severity, SqlState};

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE products (
                 sku         text   PRIMARY KEY,
                 name        text   NOT NULL,
                 price_cents bigint CONSTRAINT price_positive CHECK (price_cents > 0)
             )",
        )
        .await?;

    // A CHECK constraint: which one, on which table, and the row that broke it.
    let error = client
        .execute("INSERT INTO products VALUES ('LAMP-01', 'Lamp', 0)", &[])
        .await
        .unwrap_err();
    let db = error.as_db_error().unwrap();
    assert_eq!(db.code(), &SqlState::CHECK_VIOLATION);
    assert_eq!(db.constraint(), Some("price_positive"));
    assert_eq!(db.table(), Some("products"));
    assert_eq!(
        db.detail(),
        Some("Failing row contains (LAMP-01, Lamp, 0).")
    );
    assert_eq!(db.parsed_severity(), Some(Severity::Error));

    // NOT NULL names the column instead of a constraint.
    let error = client
        .execute("INSERT INTO products VALUES ('LAMP-01', NULL, 1)", &[])
        .await
        .unwrap_err();
    let db = error.as_db_error().unwrap();
    assert_eq!(db.code(), &SqlState::NOT_NULL_VIOLATION);
    assert_eq!(db.column(), Some("name"));

    // A syntax error points at the place in the statement.
    let error = client.execute("SELEC 1", &[]).await.unwrap_err();
    let db = error.as_db_error().unwrap();
    assert_eq!(db.code(), &SqlState::SYNTAX_ERROR);
    assert!(matches!(db.position(), Some(ErrorPosition::Original(1))));

    // Your own errors, raised in SQL, with your own code, detail and hint.
    let error = client
        .batch_execute(
            "DO $$ BEGIN
                 RAISE EXCEPTION 'order % is already collected', 42
                     USING ERRCODE = 'SR001', DETAIL = 'collected at 12:00',
                           HINT = 'only open orders can be cancelled';
             END $$",
        )
        .await
        .unwrap_err();
    let db = error.as_db_error().unwrap();
    assert_eq!(db.code().code(), "SR001");
    assert_eq!(db.message(), "order 42 is already collected");
    assert_eq!(db.hint(), Some("only open orders can be cancelled"));
    assert!(db.where_().unwrap().contains("at RAISE"));
    Ok(())
}
```

That last example matters more than it looks. A trigger or a function can enforce a rule the
schema cannot express, and report it with a code of your own. The SQL standard reserves
classes starting with `0` to `4` and `A` to `H`; the rest are for implementations, and
PostgreSQL itself uses several of them (`53`, `55`, `57`, `P0`, `XX`). A class it does not
use, such as `SR` above, is yours, and as easy for your Rust code to match as a built-in
code.

> **Pitfall:** `detail()` often contains the data that caused the error: the duplicate email
> address, the failing row. Returning it to an API client can leak another customer's data,
> and logging it can put personal data in your logs. Decide deliberately where each field goes;
> §6.5 shows a policy.

## 6.3 `SqlState`: the codes worth matching

A SQLSTATE is five characters. The first two are a *class* that groups related errors; the
whole code names one condition. tokio-postgres has a constant for each code PostgreSQL defines,
and the constants work directly as patterns in a `match`. Most applications need a dozen:

| Class | Code | `SqlState` constant | Usually means |
|---|---|---|---|
| `23` integrity | `23505` | `UNIQUE_VIOLATION` | duplicate: 409 Conflict |
| | `23503` | `FOREIGN_KEY_VIOLATION` | a reference to something that does not exist (or is still referenced) |
| | `23502` | `NOT_NULL_VIOLATION` | a missing required value |
| | `23514` | `CHECK_VIOLATION` | a rule of the data: 422 |
| | `23P01` | `EXCLUSION_VIOLATION` | an overlapping booking (exclusion constraints) |
| `40` transaction rollback | `40001` | `T_R_SERIALIZATION_FAILURE` | a concurrent transaction won: **retry** (Chapter 8) |
| | `40P01` | `T_R_DEADLOCK_DETECTED` | a deadlock, and you were chosen to fail: **retry** |
| `55` object state | `55P03` | `LOCK_NOT_AVAILABLE` | `lock_timeout` or `NOWAIT`: retry later |
| `57` operator intervention | `57014` | `QUERY_CANCELED` | `statement_timeout` or a cancel request (Chapter 12) |
| | `57P01` | `ADMIN_SHUTDOWN` | the server is shutting down, or your session was terminated |
| `53` resources | `53300` | `TOO_MANY_CONNECTIONS` | `max_connections` reached |
| `25` transaction state | `25P02` | `IN_FAILED_SQL_TRANSACTION` | an earlier error aborted this transaction (§6.6) |
| `42` syntax and access | `42P01`, `42703` | `UNDEFINED_TABLE`, `UNDEFINED_COLUMN` | a bug, or a schema the code does not match |

Matching by code is precise; matching by class handles groups. A small classifier shows both:

```rust
use tokio_postgres::Error;
use tokio_postgres::error::SqlState;

#[derive(Debug, PartialEq)]
enum Verdict {
    Duplicate,
    BadInput,
    RetryNow,
    RetryLater,
    TooSlow,
    Bug,
    Unavailable,
}

fn classify(error: &Error) -> Verdict {
    let Some(code) = error.code() else {
        return if error.is_closed() {
            Verdict::Unavailable
        } else {
            Verdict::Bug // client-side: conversions, row counts, parameters
        };
    };
    match *code {
        SqlState::UNIQUE_VIOLATION => Verdict::Duplicate,
        SqlState::T_R_SERIALIZATION_FAILURE | SqlState::T_R_DEADLOCK_DETECTED => Verdict::RetryNow,
        SqlState::LOCK_NOT_AVAILABLE => Verdict::RetryLater,
        SqlState::QUERY_CANCELED => Verdict::TooSlow,
        // The rest of class 23 (integrity) and class 22 (data exceptions, like a bad number).
        _ if code.code().starts_with("23") || code.code().starts_with("22") => Verdict::BadInput,
        // Class 08 (connection), 53 (resources), 57P0x (shutting down).
        _ if code.code().starts_with("08") || code.code().starts_with("53") => Verdict::Unavailable,
        SqlState::ADMIN_SHUTDOWN | SqlState::CANNOT_CONNECT_NOW => Verdict::Unavailable,
        _ => Verdict::Bug,
    }
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let client = playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE t (id int PRIMARY KEY, n int CHECK (n > 0));
             INSERT INTO t VALUES (1, 1);",
        )
        .await?;
    let verdict = |sql: &'static str| {
        let client = &client;
        async move { classify(&client.execute(sql, &[]).await.unwrap_err()) }
    };
    assert_eq!(
        verdict("INSERT INTO t VALUES (1, 1)").await,
        Verdict::Duplicate
    );
    assert_eq!(
        verdict("INSERT INTO t VALUES (2, 0)").await,
        Verdict::BadInput
    );
    assert_eq!(verdict("SELECT 1 / 0").await, Verdict::BadInput); // 22012
    assert_eq!(verdict("SELECT * FROM nowhere").await, Verdict::Bug); // 42P01
    assert_eq!(
        verdict("SET statement_timeout = 10; SELECT pg_sleep(1)").await,
        Verdict::Bug,
        "two statements in one prepared statement: 42601"
    );
    Ok(())
}
```

Notice the last case: it looks like a timeout test, but `execute` uses the extended protocol,
which takes one statement, so the server refused the text before anything ran. Even a
classifier needs tests.

> **Rust refresher:** `match *code { SqlState::UNIQUE_VIOLATION => … }` matches a value against
> a *constant pattern*. It works because `SqlState`'s constants are plain values of a type that
> derives `PartialEq` and `Eq`, so the compiler can compare structurally. The `*` dereferences
> the `&SqlState` that `code()` returns; you could also write `Some(&SqlState::…)` against
> `error.code()` directly.

## 6.4 `is_closed`, and the error you cannot classify

`is_closed()` is true when the connection is gone: the server restarted, a network device
dropped the TCP connection, an administrator terminated the session. Nothing on this client
will work again, and the remedy is a new connection, which a pool does for you (Chapter 13).

There is one situation no classification can resolve. If the connection breaks while a
`COMMIT` is in flight, the client cannot know whether the server committed before the
connection broke. The data may or may not be there. Retrying blindly can apply a payment
twice; not retrying can lose it.

> **Pitfall:** "commit outcome unknown" is rare and real. Operations that must not happen twice
> need a way to find out what happened: an idempotency key stored in the same transaction
> (`INSERT … ON CONFLICT DO NOTHING` on the key tells you on retry whether the first attempt
> committed), or a status the client can query. stockroom's outbox (Chapter 16) is built on
> the same idea: the job row commits together with the change, or not at all.

## 6.5 An error type for an application

A program should classify database errors once, at the boundary, into its own error type, and
decide there what is shown to whom. Three rules make that work:

1. **One `From<tokio_postgres::Error>`** that looks at the code and turns the cases the
   application handles into variants of its own: not found, conflict, invalid, busy.
   Everything else stays a database error.
2. **Constraint names are part of the schema's contract.** Name every constraint a user can
   trip over (`CONSTRAINT price_positive CHECK (…)`), and map the name to a message. The
   server's text is for developers; your message is for users.
3. **The server's text goes to the log, never to the user.** It can contain table names,
   values and other internals (§6.2).

```rust
use tokio_postgres::error::SqlState;
use tokio_postgres::{Client, Error};

#[derive(Debug, thiserror::Error)]
enum AppError {
    #[error("{0} not found")]
    NotFound(&'static str),
    #[error("{0}")]
    Conflict(&'static str),
    #[error("{0}")]
    Invalid(&'static str),
    #[error("the database is busy, try again")]
    Busy(#[source] Error),
    #[error("database error")]
    Db(#[source] Error),
}

/// What each named constraint means to a user of the API.
fn constraint_message(name: &str) -> Option<&'static str> {
    Some(match name {
        "products_pkey" => "a product with this SKU already exists",
        "price_positive" => "the price must be positive",
        _ => return None,
    })
}

impl From<Error> for AppError {
    fn from(error: Error) -> Self {
        let Some(db) = error.as_db_error() else {
            return AppError::Db(error);
        };
        let message = db.constraint().and_then(constraint_message);
        match (db.code(), message) {
            (&SqlState::UNIQUE_VIOLATION, Some(message)) => AppError::Conflict(message),
            (&SqlState::CHECK_VIOLATION, Some(message)) => AppError::Invalid(message),
            (&SqlState::T_R_SERIALIZATION_FAILURE | &SqlState::T_R_DEADLOCK_DETECTED, _) => {
                AppError::Busy(error)
            }
            _ => AppError::Db(error),
        }
    }
}

async fn create_product(client: &Client, sku: &str, price: i64) -> Result<(), AppError> {
    client
        .execute("INSERT INTO products VALUES ($1, $2)", &[&sku, &price])
        .await?; // classified here, by the From impl
    Ok(())
}

async fn price_of(client: &Client, sku: &str) -> Result<i64, AppError> {
    let row = client
        .query_opt("SELECT price_cents FROM products WHERE sku = $1", &[&sku])
        .await?;
    Ok(row.ok_or(AppError::NotFound("product"))?.get(0))
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let client = playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE products (
                 sku text PRIMARY KEY,
                 price_cents bigint CONSTRAINT price_positive CHECK (price_cents > 0))",
        )
        .await?;

    create_product(&client, "CHAIR-01", 4999).await.unwrap();
    let duplicate = create_product(&client, "CHAIR-01", 1).await.unwrap_err();
    assert_eq!(
        duplicate.to_string(),
        "a product with this SKU already exists"
    );
    let free = create_product(&client, "LAMP-01", 0).await.unwrap_err();
    assert_eq!(free.to_string(), "the price must be positive");
    assert!(matches!(
        price_of(&client, "VASE-01").await,
        Err(AppError::NotFound("product"))
    ));

    // Anything unplanned keeps the original error as its source, for the log.
    let other = AppError::from(
        client
            .execute("INSERT INTO nowhere VALUES (1)", &[])
            .await
            .unwrap_err(),
    );
    assert_eq!(other.to_string(), "database error");
    let cause = std::error::Error::source(&other).unwrap();
    assert!(
        cause
            .source()
            .unwrap()
            .to_string()
            .contains("\"nowhere\" does not exist")
    );
    Ok(())
}
```

The `?` in `create_product` and `price_of` is where classification happens, through `From`. The
rest of the program sees `AppError`s with messages fit for users, and the original
`tokio_postgres::Error` rides along as the `source()` of the variants that keep it, ready for
the log. Chapter 15 turns each variant into an HTTP status.

## 6.6 An error inside a transaction

After any error, a PostgreSQL transaction refuses to do anything else until it is rolled back:

```rust
use tokio_postgres::error::SqlState;

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let mut client = playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE products (sku text PRIMARY KEY);
             INSERT INTO products VALUES ('CHAIR-01');",
        )
        .await?;

    let tx = client.transaction().await?;
    // One statement fails...
    let duplicate = tx
        .execute("INSERT INTO products VALUES ('CHAIR-01')", &[])
        .await;
    assert!(duplicate.is_err());
    // ...and every statement after it is refused, however harmless.
    let refused = tx.query_one("SELECT 1", &[]).await.unwrap_err();
    assert_eq!(refused.code(), Some(&SqlState::IN_FAILED_SQL_TRANSACTION)); // 25P02
    assert_eq!(
        refused.as_db_error().unwrap().message(),
        "current transaction is aborted, commands ignored until end of transaction block"
    );
    tx.rollback().await?;

    // A savepoint limits the damage: roll back to it, and the transaction carries on.
    let mut tx = client.transaction().await?;
    let attempt = tx.savepoint("attempt").await?;
    let duplicate = attempt
        .execute("INSERT INTO products VALUES ('CHAIR-01')", &[])
        .await;
    assert!(duplicate.is_err());
    attempt.rollback().await?; // ROLLBACK TO SAVEPOINT attempt
    tx.execute("INSERT INTO products VALUES ('LAMP-01')", &[])
        .await?;
    tx.commit().await?;

    let count: i64 = client
        .query_one("SELECT count(*) FROM products", &[])
        .await?
        .get(0);
    assert_eq!(count, 2);
    Ok(())
}
```

So "try the insert, and if it is a duplicate, do something else" does not work inside a
transaction, unless the attempt runs in a savepoint. Chapter 8 covers savepoints properly, and
stockroom's batch endpoint uses one per item. Often there is a better SQL answer that avoids
the error altogether, such as `INSERT … ON CONFLICT DO NOTHING`.

> **Pitfall:** a 25P02 in your logs is never the real problem; it is the *second* error. Look
> for the first one, earlier in the same transaction. Code that swallows an error inside a
> transaction (a `let _ =` on a failed `execute`) produces 25P02s far away from the cause.

## 6.7 Notices are not errors

The server can also send *notices*: messages that do not stop anything. `DROP TABLE IF EXISTS`
on a missing table sends one, as does `CREATE TABLE IF NOT EXISTS` on an existing table, or a
`RAISE NOTICE` in a function. The statement succeeds:

```rust
#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = playground::connect().await?;
    // Both succeed; each also makes the server send a notice.
    client.batch_execute("DROP TABLE IF EXISTS nowhere").await?;
    client
        .batch_execute("DO $$ BEGIN RAISE NOTICE 'stock is low'; END $$")
        .await?;
    Ok(())
}
```

Where did the notices go? A notice is not part of any query's response: it arrives on the
connection like a notification would, and only the `Connection` sees it. When the connection
is simply awaited, as the playground's `connect` does, tokio-postgres passes each notice to the
`log` crate at the `info` level, so you see it only if your program installed a logger for
`log`. To handle notices yourself, drive the connection with `poll_message` instead of
awaiting it (Chapter 11). stockroom's migration runner does exactly that, which is how
"relation … already exists, skipping" ends up in its log.

### Side by side — handling a unique violation

The task: insert a product whose SKU already exists, and recognize the error as a duplicate,
with the constraint's name.

```sql
-- psql
tpbook=# INSERT INTO products VALUES ('CHAIR-01', 1);
ERROR:  duplicate key value violates unique constraint "products_pkey"
DETAIL:  Key (sku)=(CHAIR-01) already exists.
tpbook=# \errverbose
ERROR:  23505: duplicate key value violates unique constraint "products_pkey"
DETAIL:  Key (sku)=(CHAIR-01) already exists.
SCHEMA NAME:  pg_temp_12
TABLE NAME:  products
CONSTRAINT NAME:  products_pkey
LOCATION:  _bt_check_unique, nbtinsert.c:673
```

~~~rust
// sqlx (illustration)
match sqlx::query("INSERT INTO products VALUES ($1, $2)").bind("CHAIR-01").bind(1_i64).execute(&pool).await {
    Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
        assert_eq!(e.constraint(), Some("products_pkey"));
    }
    other => { /* … */ }
}
~~~

```rust
use tokio_postgres::error::SqlState;

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE products (sku text PRIMARY KEY, price_cents bigint);
             INSERT INTO products VALUES ('CHAIR-01', 4999);",
        )
        .await?;
    match client
        .execute("INSERT INTO products VALUES ('CHAIR-01', 1)", &[])
        .await
    {
        Err(e) if e.code() == Some(&SqlState::UNIQUE_VIOLATION) => {
            let constraint = e.as_db_error().and_then(|db| db.constraint());
            assert_eq!(constraint, Some("products_pkey"));
        }
        other => panic!("expected a duplicate, got {other:?}"),
    }
    Ok(())
}
```

| | psql | sqlx | tokio-postgres |
|---|---|---|---|
| **Same** | The SQLSTATE, the constraint, the detail: everything the server sent | The same fields, from the same message | The same fields, from the same message |
| **Different** | Shows the message and detail; `\errverbose` shows every field of the last error | An `Error::Database` variant, with helpers such as `is_unique_violation()`; other errors are separate variants of one public enum | One opaque `Error`: `code()` and `as_db_error()` for server errors, the cause in `source()` for the rest, and `Display` that is only a category |

---

## In stockroom

stockroom classifies database errors in exactly one place,
[error.rs](../stockroom/src/error.rs), with the shape of §6.5 and a few more variants:

~~~rust
// stockroom/src/error.rs (excerpt)
impl From<tokio_postgres::Error> for AppError {
    fn from(error: tokio_postgres::Error) -> Self {
        let Some(db) = error.as_db_error() else {
            // Not from the server: IO, a closed connection, or a conversion on our side.
            return if error.is_closed() {
                AppError::Unavailable("the connection to the database was lost".into())
            } else {
                AppError::Db(error)
            };
        };
        match *db.code() {
            SqlState::UNIQUE_VIOLATION => {
                let (field, message) =
                    constraint_message(db).unwrap_or((None, "this already exists"));
                AppError::Conflict {
                    field,
                    message: message.into(),
                }
            }
            // … CHECK, NOT NULL and FOREIGN KEY violations become AppError::Invalid …
            SqlState::T_R_SERIALIZATION_FAILURE
            | SqlState::T_R_DEADLOCK_DETECTED
            | SqlState::LOCK_NOT_AVAILABLE => AppError::Busy(error),
            SqlState::QUERY_CANCELED => AppError::Timeout(error),
            SqlState::ADMIN_SHUTDOWN
            | SqlState::CANNOT_CONNECT_NOW
            | SqlState::TOO_MANY_CONNECTIONS => AppError::Unavailable(db.message().to_owned()),
            _ => AppError::Db(error),
        }
    }
}
~~~

`constraint_message` maps every named constraint in the schema to the request field it is
about and a message, so a client gets `{"error": "conflict", "field": "sku", "message": "a
product with this SKU already exists"}` instead of a server message. `Busy` is what the
transaction helper of Chapter 8 retries. When an error *is* the server's fault (a 5xx), its
fields go to the log as structured data and the client gets a generic message:

~~~rust
// stockroom/src/error.rs (excerpt)
fn log_server_error(error: &AppError) {
    let db = match error {
        AppError::Busy(e) | AppError::Timeout(e) | AppError::Db(e) => e.as_db_error(),
        _ => None,
    };
    match db {
        Some(db) => tracing::error!(
            code = db.code().code(),
            message = db.message(),
            detail = db.detail(),
            hint = db.hint(),
            table = db.table(),
            constraint = db.constraint(),
            "{error}"
        ),
        None => tracing::error!(error = ?error, "{error}"),
    }
}
~~~

There is one deliberate exception to "server text never reaches the client": errors in an
uploaded CSV file (class 22, data exceptions, raised by `COPY`). Their message and position
quote the client's own data back, "invalid input syntax for type bigint: "cheap" (COPY
product_import, line 3, column price_cents: "cheap")", which is exactly what the uploader needs.
[repo/products.rs](../stockroom/src/repo/products.rs) makes that exception in a function of
its own, `input_error`, so that it cannot spread by accident.

## Summary

- Every failure is a `tokio_postgres::Error`. Its `Display` is a category (`db error`); the
  explanation is in `source()`. Log `{:?}`, or walk the source chain.
- `as_db_error()` and `code()` exist for errors the server reported; client-side errors (row
  counts, conversions, parameters) have no code and no public kind.
- `DbError` carries the SQLSTATE, message, detail, hint, position, the schema, table, column,
  datatype and constraint involved, and the call stack of functions and triggers.
- Match `SqlState` constants as patterns; match classes (`23`, `40`, `08`) by prefix.
- `is_closed()` means a new connection is needed. A broken connection during `COMMIT` leaves
  the outcome unknown: design operations to be safely retried.
- Classify once, in a `From<tokio_postgres::Error>` for your own error type. Name your
  constraints, and map the names to messages. Keep the server's text out of responses.
- After an error, a transaction is aborted (25P02) until rolled back; savepoints contain the
  damage.
- Notices are not errors. When the connection is just awaited they go to the `log` crate;
  `poll_message` hands them to you.

## Exercises

1. **A better log line.** Write `fn describe(error: &tokio_postgres::Error) -> String` that
   produces `23505 unique_violation on products (products_pkey): duplicate key value …` for
   server errors and falls back to the source chain for the others.
   *Hint:* the SQLSTATE's name is not in tokio-postgres; a short `match` on the codes you
   care about is enough.
2. **Retry or not?** For each, say whether a request that failed with it should be retried as
   is, retried after a delay, or not retried: `40001`, `40P01`, `55P03`, `57014`, `23505`,
   `53300`, `57P01`, a closed connection during `SELECT`, a closed connection during `COMMIT`.
   *Hint:* ask of each whether the same request could succeed a moment later, and whether it may
   already have taken effect.
3. **A rule in a trigger.** Write a trigger that refuses to delete a product that has stock,
   with `RAISE EXCEPTION … USING ERRCODE = 'SR002'`, and map `SR002` to a 409 Conflict variant
   of your `AppError`.
   *Hint:* a custom code has no named constant; compare `db.code().code()` with `"SR002"`, or
   with `SqlState::from_code("SR002")`.
4. **Find the first error.** A colleague's code logs "current transaction is aborted" every few
   minutes. What do you look for, and what is a likely cause in Rust code?
   *Hint:* the useful line is the error *before* the first "aborted" one on that connection;
   look for code that ignores a statement's error (`let _ =`, `.ok()`) and carries on in the
   same transaction.

### Solutions

**2.** `40001` and `40P01`: retry the whole transaction, immediately or after a short random
pause (they are caused by concurrency, not by the request). `55P03` (lock timeout): retry after
a delay, or report "busy". `57014`: do not retry blindly: it was too slow once, and will
probably be too slow again; report it, and look at the query. `23505`: never; the request
conflicts with data. `53300` and `57P01`: retry after a delay, preferably on another connection
or server. A closed connection during `SELECT`: retry on a new connection, since reading is
safe to repeat. A closed connection during `COMMIT`: the outcome is unknown; only retry if the
operation is idempotent or you can check what happened first (§6.4).

**4.** Look for the error *before* it, in the same transaction, on the same connection: the
25P02 is the consequence. In Rust code, the classic cause is an error that was caught and
ignored inside a transaction, such as `let _ = tx.execute(…).await;` or a `match` that logs
and continues, after which the transaction is used again. Either use a savepoint around the
statement that may fail, or let the error end the transaction.

---

Previous: [Chapter 5 — Custom types: ToSql and FromSql](05-custom-types.md) ·
Next: [Chapter 7 — One connection, many tasks: pipelining](07-pipelining.md)
