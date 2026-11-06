# Chapter 1 — Connecting

> **Async PostgreSQL in Rust** · Previous: [Chapter 0 — Introduction and setup](00-introduction.md) ·
> Next: [Chapter 2 — Running SQL: execute, query and parameters](02-queries.md)

Every other chapter starts with a connection, so this one looks at what a connection *is* in
tokio-postgres, and why it comes in two halves. That split is the most important design
decision in the crate. It is why one connection can be shared by many tasks, why queries can be
pipelined (Chapter 7), and why notifications need special handling (Chapter 11). It is also
the reason for the first bug everyone writes: a program that connects successfully and then
hangs forever on its first query.

The rest of the chapter is practical: connection strings and the `Config` builder, what the
server tells you when you connect, how a connection ends, and what each kind of failure looks
like, so you recognize them when they happen.

---

## 1.1 Two halves: `Client` and `Connection`

`tokio_postgres::connect` returns a pair:

```rust
use tokio_postgres::NoTls;

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL is set");

    // Two values: the client you call methods on, and the connection that does the IO.
    let (client, connection) = tokio_postgres::connect(&url, NoTls).await?;

    // The connection is a future. Until something polls it, nothing is sent or received:
    // hand it to the runtime as a task of its own.
    let task = tokio::spawn(connection);

    let version: String = client.query_one("SHOW server_version", &[]).await?.get(0);
    println!("connected to PostgreSQL {version}");
    assert!(!version.is_empty());

    // Dropping the client tells the connection there is nothing more to do: it says goodbye
    // to the server and its future completes, with `Ok(())`.
    drop(client);
    task.await.expect("the task did not panic")?;
    Ok(())
}
```

`NoTls` says "no encryption"; Chapter 12 replaces it with a real TLS connector. The rest is
the shape of every program in this book.

The two halves divide the work like this:

```text
  your tasks ──query──▶ Client ──requests──▶ Connection ──bytes──▶ PostgreSQL
             ◀──rows───        ◀─responses──    (a task)  ◀──bytes──
```

- The **`Connection`** owns the socket. It is a `Future` that, as long as it is polled, writes
  queued requests to the server, reads whatever the server sends, and routes each response to
  whoever is waiting for it. It finishes when the client is gone, or with an error when the
  connection breaks.
- The **`Client`** owns no socket at all. It holds the sending end of a channel to the
  connection. A method such as `query_one` turns your call into protocol messages, puts them on
  the channel, and waits for its own response to come back.

Two consequences matter right away. First, the `Client`'s methods take `&self`, not
`&mut self`, so one client can be shared by many tasks at once (wrapped in an `Arc`); their
requests queue up on the same connection. Second, nothing happens unless the connection is
polled, which brings us to the first bug.

> **Rust refresher:** a `Future` is a value that describes work; it does nothing until it is
> polled, and `.await` is one way to poll it. `tokio::spawn(fut)` is another: it hands the
> future to the runtime, which polls it in the background, on any of its threads. That is why
> a spawned future must be `Send` (movable between threads) and `'static` (owning its data).
> `Connection` is both, by design.

## 1.2 Spawning the connection, and forgetting to

If you keep the connection but never poll it, your queries wait forever:

```rust
use std::time::Duration;
use tokio_postgres::NoTls;

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL is set");
    let (client, connection) = tokio_postgres::connect(&url, NoTls).await?;

    // Nobody polls `connection`. The request is queued for it, and nothing happens.
    let waited = tokio::time::timeout(
        Duration::from_millis(200),
        client.query_one("SELECT 1", &[]),
    )
    .await;
    assert!(
        waited.is_err(),
        "the query timed out: it never left the process"
    );

    // Start the connection, and the next query goes through. (So does the abandoned
    // `SELECT 1`: it was still queued, and its answer is thrown away.)
    tokio::spawn(connection);
    let one: i32 = client.query_one("SELECT 1", &[]).await?.get(0);
    assert_eq!(one, 1);
    Ok(())
}
```

Note what `connect` itself did: it opened the socket, logged in and waited for the server to be
ready, all *before* returning. `connect().await` succeeding means the server accepted you. The
connection's background work only begins once you spawn it.

Spawning is not the only way to poll a connection; it is just the convenient one. A short
program can poll the connection and its query together, in the same task, with `select!`:

```rust
use tokio_postgres::NoTls;

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL is set");
    let (client, connection) = tokio_postgres::connect(&url, NoTls).await?;

    // Poll both: the query finishes first, because the connection only finishes when the
    // client is gone (or the server goes away).
    let answer: i32 = tokio::select! {
        row = client.query_one("SELECT 40 + 2", &[]) => row?.get(0),
        ended = connection => panic!("the connection ended first: {ended:?}"),
    };
    assert_eq!(answer, 42);
    Ok(())
}
```

That works, but it is fragile: the moment two queries are in flight, or one runs after the
`select!`, the connection is no longer being polled. In real programs, spawn it, and do
something with its result:

~~~rust
let (client, connection) = tokio_postgres::connect(&url, NoTls).await?;
tokio::spawn(async move {
    if let Err(e) = connection.await {
        eprintln!("connection error: {e}");
    }
});
~~~

The error matters. When the server restarts or the network drops, this is where you hear
about it first; every query in flight fails with "connection closed" a moment later.

> **Pitfall:** `tokio::spawn(connection);` alone compiles, runs, and throws the connection's
> error away (the `JoinHandle` is dropped, and with it the `Result`). In a service, log it.

> **Protocol lens:** what `connect` does before it returns. The client sends a *StartupMessage*
> with the user, the database and any settings. The server asks for a password (for a
> `scram-sha-256` server, a SCRAM exchange of two more round trips), then sends a series of
> *ParameterStatus* messages (the server's version, encoding, time zone and so on), one
> *BackendKeyData* message (a process id and a secret, kept for cancelling queries, Chapter 12),
> and finally *ReadyForQuery*. That is three to four round trips before your first query; on a
> slow network you feel it, which is one reason connections are pooled (Chapter 13).

## 1.3 Connection strings

`tokio_postgres::connect` takes a string in either of the two forms libpq, PostgreSQL's C
library, understands:

- **Key/value pairs**, separated by spaces: `host=localhost port=5432 user=shop dbname=stock`.
  A value with spaces, or an empty one, goes in single quotes: `password='two words'`.
- **A URL**: `postgres://user:password@host:port/database?key=value&key=value`. Every
  key/value setting can go in the query string. Characters with a meaning in URLs must be
  percent-encoded: a password `s3cr@t` is written `s3cr%40t`.

Both parse into a `Config`, and you can parse one yourself to look inside:

```rust
use std::time::Duration;
use tokio_postgres::Config;
use tokio_postgres::config::Host;

fn main() -> Result<(), tokio_postgres::Error> {
    let from_url: Config =
        "postgres://shop:s3cr%40t@db.example.com:5433/stock?application_name=reports&connect_timeout=5"
            .parse()?;
    let from_pairs: Config = "host=db.example.com port=5433 user=shop password='s3cr@t' \
                              dbname=stock application_name=reports connect_timeout=5"
        .parse()?;

    for config in [&from_url, &from_pairs] {
        assert_eq!(config.get_user(), Some("shop"));
        assert_eq!(config.get_password(), Some(&b"s3cr@t"[..])); // bytes, decoded
        assert_eq!(config.get_dbname(), Some("stock"));
        assert_eq!(config.get_hosts(), [Host::Tcp("db.example.com".into())]);
        assert_eq!(config.get_ports(), [5433]);
        assert_eq!(config.get_application_name(), Some("reports"));
        assert_eq!(config.get_connect_timeout(), Some(&Duration::from_secs(5)));
    }

    // Printing a Config never shows the password.
    let printed = format!("{from_url:?}");
    assert!(printed.contains("password: Some(_)") && !printed.contains("s3cr"));
    Ok(())
}
```

The settings you will meet most often:

| Key | Meaning | Default |
|---|---|---|
| `host` | Host name or IP address; a value starting with `/` is a Unix-socket directory | required |
| `port` | TCP port (or the socket file's suffix) | 5432 |
| `user`, `password` | Who you log in as | none (`user` is required) |
| `dbname` | The database | the user's name |
| `application_name` | How the session shows up in `pg_stat_activity` and the server log | none |
| `connect_timeout` | Seconds to wait for each connection attempt | wait as long as the OS does |
| `options` | Settings for the session, as server command-line options: `-c statement_timeout=5000` | none |
| `sslmode` | `disable`, `prefer` or `require` (Chapter 12) | `prefer` |

Several hosts can be listed, separated by commas (`host=primary,replica`), and they are tried
in order. Chapter 12 uses that for failover.

> **Pitfall:** tokio-postgres reads **no environment variables** and no files. libpq-based
> tools such as psql also look at `PGHOST`, `PGPASSWORD`, `PGSERVICE`, `~/.pgpass` and more; a
> connection string that works in psql because `PGPASSWORD` is set will fail with "password
> missing" from Rust. Everything must be in the string or the `Config`. The convention for
> passing that string to a program is the `DATABASE_URL` environment variable, which your
> program reads itself, as the playground helper does.

## 1.4 The `Config` builder

When a program needs settings of its own on top of what the operator configured, parse the
string and add to it. Every setter takes `&mut self` and returns `&mut Config`, so calls chain:

```rust
use std::time::Duration;
use tokio_postgres::{Config, NoTls};

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    // Start from what the operator gave us...
    let mut config: Config = std::env::var("DATABASE_URL")
        .expect("DATABASE_URL is set")
        .parse()?;
    // ...and add what this program needs.
    config
        .application_name("chapter-1")
        .connect_timeout(Duration::from_secs(3))
        // Session settings, applied by the server before the first query.
        .options("-c statement_timeout=1500 -c TimeZone=UTC");

    let (client, connection) = config.connect(NoTls).await?;
    tokio::spawn(connection);

    let row = client
        .query_one(
            "SELECT current_setting('application_name'),
                    current_setting('statement_timeout'),
                    current_setting('TimeZone')",
            &[],
        )
        .await?;
    assert_eq!(row.get::<_, &str>(0), "chapter-1");
    assert_eq!(row.get::<_, &str>(1), "1500ms");
    assert_eq!(row.get::<_, &str>(2), "UTC");
    Ok(())
}
```

`options` deserves a moment. It is passed to the server at startup, so its settings are in
force from the first statement of the session, without a round trip of their own, and a pool
(Chapter 13) applies them to every connection it opens. stockroom uses it to give every session
a `statement_timeout`, a `lock_timeout` and an `idle_in_transaction_session_timeout` (see
*In stockroom* below).

Some settings are lists, and their setters *add* to them. That catches people who want to
"point the configuration somewhere else":

```rust
use tokio_postgres::Config;
use tokio_postgres::config::Host;

fn main() -> Result<(), tokio_postgres::Error> {
    let mut config: Config = "postgres://app@db.internal:5432/shop".parse()?;

    // Meant: "connect to the local copy instead". Did: "try db.internal, then localhost".
    config.host("localhost").port(5433);
    assert_eq!(
        config.get_hosts(),
        [
            Host::Tcp("db.internal".into()),
            Host::Tcp("localhost".into())
        ]
    );
    assert_eq!(config.get_ports(), [5432, 5433]);

    // A Unix-domain socket is a host too: the directory that holds the socket file.
    let local: Config = "host=/tmp port=5432 user=app".parse()?;
    assert!(matches!(local.get_hosts(), [Host::Unix(dir)] if dir.as_os_str() == "/tmp"));
    Ok(())
}
```

> **Pitfall:** `Config::host` and `Config::port` append; there is no setter that replaces the
> list. The first host still wins, so the mistake is silent: the program connects, just not
> where you think. To redirect a connection, build a fresh `Config` from the parts you need, or
> open the socket yourself and use `Config::connect_raw` (Chapter 12). The example programs in
> this repository do both, and both started with exactly this bug.

## 1.5 What the server told you, and how a connection ends

While logging you in, the server sent a list of its settings, and the `Connection` kept them.
`Connection::parameter` reads them, and it is the only way to: once the connection has been
moved into a task, you can no longer reach it, so read what you need first.

The client has two more methods about the connection itself. `is_closed` answers immediately
from local state: is the connection task still there? `check_connection` (added in 0.7.15) makes
one round trip to the server and fails if it does not come back.

```rust
use tokio_postgres::NoTls;
use tokio_postgres::error::SqlState;

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL is set");
    let (client, connection) = tokio_postgres::connect(&url, NoTls).await?;

    // Reported by the server at startup: read them before the connection moves away.
    let version = connection
        .parameter("server_version")
        .unwrap_or("?")
        .to_owned();
    let encoding = connection.parameter("client_encoding").map(str::to_owned);
    let task = tokio::spawn(connection);
    println!("PostgreSQL {version}");
    assert_eq!(encoding.as_deref(), Some("UTF8"));

    client.check_connection().await?; // one round trip, no statement
    assert!(!client.is_closed());

    // End our own session from the server side, as an administrator, a restart or a crash
    // would. The query that triggers it fails with SQLSTATE 57P01, "admin shutdown".
    let error = client
        .execute("SELECT pg_terminate_backend(pg_backend_pid())", &[])
        .await
        .unwrap_err();
    assert_eq!(error.code(), Some(&SqlState::ADMIN_SHUTDOWN));

    // The connection task ends with an error, and from then on the client knows it is closed.
    assert!(task.await.expect("the task did not panic").is_err());
    assert!(client.is_closed());
    let error = client.check_connection().await.unwrap_err();
    assert!(error.is_closed());
    Ok(())
}
```

The server reports more than a dozen such parameters. The ones worth knowing:
`server_version` (for feature checks), `server_encoding` and `client_encoding` (always `UTF8`
for tokio-postgres, which insists on it), `TimeZone`, `DateStyle`, `integer_datetimes`,
`application_name`, `is_superuser`, `in_hot_standby` (whether you are on a read-only replica,
PG 14+) and, from PG 18, `search_path`. The server also sends an update whenever one of them
changes during the session, and `parameter` returns the latest value it has seen.

A connection ends in one of three ways:

| How | The connection future returns | The client |
|---|---|---|
| You drop every `Client` (and every `Transaction`, which borrows one) | `Ok(())`, after telling the server goodbye | gone |
| The server ends the session (restart, `pg_terminate_backend`, `idle_session_timeout`) or the network breaks | `Err(…)` | `is_closed()` turns true; every call fails with "connection closed" |
| You drop the `Connection` future (abort its task) | — | `is_closed()` turns true, and the server sees the socket close |

There is no reconnecting: a closed client stays closed. Making a new connection is the
caller's job, which is a large part of what a pool does (Chapter 13). Chapter 12 covers how to
notice a connection that died silently, without the server telling you.

## 1.6 What failure looks like

Connecting can fail in several distinct ways, and the error says which, if you know where to
look:

```rust
use std::time::{Duration, Instant};
use tokio_postgres::error::SqlState;
use tokio_postgres::{Config, NoTls};

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let good: Config = std::env::var("DATABASE_URL")
        .expect("DATABASE_URL is set")
        .parse()?;

    // The server is there, but says no: there is no such database.
    let mut config = good.clone();
    config.dbname("no_such_database");
    let error = config.connect(NoTls).await.err().expect("refused");
    assert_eq!(error.code(), Some(&SqlState::INVALID_CATALOG_NAME)); // 3D000
    // The error's own text is terse; the server's message is one level down.
    assert_eq!(error.to_string(), "db error");
    let db = error.as_db_error().expect("an error from the server");
    assert_eq!(db.message(), "database \"no_such_database\" does not exist");

    // Nothing listens on the port: an IO error, and no SQLSTATE.
    let mut config = Config::new();
    config.host("localhost").port(1).user("nobody");
    let error = config.connect(NoTls).await.err().expect("refused");
    assert!(error.code().is_none() && error.as_db_error().is_none());
    let cause = std::error::Error::source(&error).expect("an underlying IO error");
    println!("{error}: {cause}"); // "error connecting to server: Connection refused …"

    // A host that never answers. Without connect_timeout, this waits as long as the
    // operating system lets a TCP connection attempt wait: often over a minute.
    let mut config = Config::new();
    config
        .host("10.255.255.1")
        .user("nobody")
        .connect_timeout(Duration::from_millis(300));
    let started = Instant::now();
    assert!(config.connect(NoTls).await.is_err());
    assert!(started.elapsed() < Duration::from_secs(2));
    Ok(())
}
```

The pattern to remember: **an error from the server has a SQLSTATE code and a `DbError` with
the server's message; anything else (refused, unreachable, timed out, TLS) has neither, and its
cause is the error's `source()`.** A few codes you will meet when connecting:

| SQLSTATE | `SqlState` constant | Meaning |
|---|---|---|
| `28P01` | `INVALID_PASSWORD` | Wrong password |
| `28000` | `INVALID_AUTHORIZATION_SPECIFICATION` | No `pg_hba.conf` entry lets you in (wrong host, user or SSL mode) |
| `3D000` | `INVALID_CATALOG_NAME` | No such database |
| `53300` | `TOO_MANY_CONNECTIONS` | `max_connections` reached |
| `57P03` | `CANNOT_CONNECT_NOW` | The server is starting up or shutting down |

> **Pitfall:** `error.to_string()` for any server error is just `db error`. Logging errors
> with `{}` hides the one thing you need. Log with `{:?}`, or reach for `as_db_error()`;
> Chapter 6 builds an error type that does it for you.

### Side by side — *connecting*

The task: connect to the book's database as an application called `demo`, and ask for the
server's version.

```sql
-- psql: the connection string is the argument; \conninfo describes the connection
-- (as a table since psql 18).
--   psql "postgres://postgres:secret@localhost:5432/tpbook?application_name=demo"
tpbook=# \conninfo
      Connection Information
      Parameter       |   Value
----------------------+-----------
 Database             | tpbook
 Client User          | postgres
 Host                 | localhost
 Host Address         | ::1
 Server Port          | 5432
 Options              |
 Protocol Version     | 3.0
 Password Used        | true
 GSSAPI Authenticated | false
 Backend PID          | 95981
 SSL Connection       | false
 Superuser            | on
 Hot Standby          | off
(13 rows)

tpbook=# SHOW server_version;
 server_version
----------------
 18.6
(1 row)
```

~~~rust
// sqlx (illustration)
use sqlx::Connection;
let mut conn = sqlx::PgConnection::connect(&url).await?; // no task to spawn
let version: String = sqlx::query_scalar("SHOW server_version").fetch_one(&mut conn).await?;
~~~

```rust
use tokio_postgres::{Config, NoTls};

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let mut config: Config = std::env::var("DATABASE_URL")
        .expect("DATABASE_URL is set")
        .parse()?;
    config.application_name("demo");
    let (client, connection) = config.connect(NoTls).await?;
    tokio::spawn(connection);

    let version: String = client.query_one("SHOW server_version", &[]).await?.get(0);
    assert!(version.chars().next().is_some_and(|c| c.is_ascii_digit()));
    Ok(())
}
```

| | psql | sqlx | tokio-postgres |
|---|---|---|---|
| **Same** | A connection string in the URL or key/value form; one session on the server, with the same startup and login | The same | The same |
| **Different** | Built on libpq: reads `PG*` environment variables, `~/.pgpass` and service files | Reads the `PG*` variables too. A `PgConnection` does its IO inside each call, so it needs `&mut` and runs one query at a time; a pool provides concurrency | Reads nothing implicitly. The `Connection` is a separate task, so `Client` methods take `&self`: a client can be shared, and its queries pipelined (Chapter 7). You must spawn the connection |

## 1.7 The playground's `connect`

From here on, the book's examples start with `playground::connect()`, which Chapter 0 asked
you to put in your playground's `src/lib.rs`. Now every line of it should make sense:

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

It returns `Result<Client, Error>`, so examples write `connect().await?` in a `main` that
returns `Result<(), tokio_postgres::Error>`, and any failure, including at startup, ends the
program with the error printed.

---

## In stockroom

stockroom never calls `tokio_postgres::connect` with a bare string. Its settings start from the
`--database-url` flag (or `STOCKROOM_DATABASE_URL`) and then get the defaults every session of
the service should have, unless the operator's string already says otherwise:

~~~rust
// stockroom/src/config.rs (excerpt)
pub fn pg_config(&self) -> Result<Config, tokio_postgres::Error> {
    let mut config: Config = self.database_url.parse()?;
    if config.get_application_name().is_none() {
        config.application_name("stockroom");
    }
    if config.get_connect_timeout().is_none() {
        config.connect_timeout(Duration::from_secs(5));
    }
    // Notice a dead peer in about a minute rather than the default two hours.
    config.keepalives_idle(Duration::from_secs(30));
    config.keepalives_interval(Duration::from_secs(10));
    config.keepalives_retries(3);

    // Guard rails on the server: runaway statements, lock waits, and transactions left
    // open by a crashed or stuck client are all ended by PostgreSQL itself.
    let mut options = config.get_options().unwrap_or_default().to_owned();
    // … appends "-c statement_timeout=…", "-c lock_timeout=2000" and
    //   "-c idle_in_transaction_session_timeout=60000" to whatever options were given …
    config.options(options);
    Ok(config)
}
~~~

The application name is what makes stockroom's sessions recognizable in `pg_stat_activity`
(the `/admin/sessions` endpoint shows them). The timeouts and keepalives are Chapter 12's
subject. Note the care taken with `options`: it is a single string, so new settings are
appended to the operator's rather than replacing them.

Most of stockroom's connections come from a pool, which spawns each connection and discards its
error (Chapter 13). The commands that need one connection of their own (`migrate`, `seed`,
`export`) get it from [db/connect.rs](../stockroom/src/db/connect.rs), which spawns the
connection the careful way. Rather than just awaiting it, it *drives* it, so that the
server's notices end up in the log:

~~~rust
// stockroom/src/db/connect.rs (excerpt)
pub async fn connect(config: &Config, tls: &MakeTls) -> Result<Client, tokio_postgres::Error> {
    let (client, mut connection) = config.connect(tls.clone()).await?;
    tokio::spawn(async move {
        let mut messages = stream::poll_fn(move |cx| connection.poll_message(cx));
        while let Some(message) = messages.next().await {
            match message {
                // … log notices and notifications …
                Err(error) => {
                    tracing::error!(%error, "database connection failed");
                    break;
                }
            }
        }
    });
    Ok(client)
}
~~~

`poll_message` is Chapter 11's subject. For now, notice the outline: it is the same
spawn-and-log as in §1.2, with a loop where the single `.await` was.

## Summary

- `connect` returns a `Client`, which you call, and a `Connection`, a future that does all the
  IO. Spawn the connection, and log its error: that is where a broken connection shows up
  first.
- If the connection is not polled, queries wait forever. `connect().await` succeeding means
  only that the server accepted you.
- `Client` methods take `&self`: one client can be shared by many tasks.
- Connection strings come as key/value pairs or URLs; both parse into a `Config`.
  tokio-postgres reads no environment variables or password files.
- `Config` setters chain. `options` carries session settings to the server at startup.
  `host` and `port` *append* to lists.
- `Connection::parameter` reads what the server reported at startup; read it before spawning.
  `is_closed` is local and free; `check_connection` costs a round trip.
- Errors from the server have a SQLSTATE and a `DbError`; IO errors have neither and keep
  their cause in `source()`. `Display` of a server error is only "db error".

## Exercises

1. **A connection with a name.** Connect with `application_name` set to your own name, then,
   from the same client, find your session in `pg_stat_activity` and print its `pid`,
   `application_name` and `backend_start`.
   *Hint:* `pg_backend_pid()` is your session's pid.
2. **Two clients, one connection?** `Client` is not `Clone`. How would you give ten tasks the
   same client? Write it, with each task running `SELECT pg_backend_pid()`, and check that
   all ten see the same pid.
   *Hint:* `Arc`.
3. **Settings at startup versus `SET`.** Set `statement_timeout` once through `options` and
   once with `client.batch_execute("SET statement_timeout = 1000")` after connecting. Both
   work. Name one situation where only the first one is safe.
   *Hint:* think of what happens to `SET` on a connection that a pool hands to someone else
   later, and of the connection's very first statement.
4. **What the server reports.** Print every startup parameter listed in §1.5. Then change your
   session's time zone with `SET TimeZone = 'Asia/Tokyo'` and check whether
   `connection.parameter("TimeZone")` notices.
   *Hint:* you need to keep polling the connection yourself to look at it after the `SET`;
   `select!` from §1.2 can do that in a small program.

### Solutions

**2.**

```rust
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = Arc::new(playground::connect().await?);
    let mut tasks = Vec::new();
    for _ in 0..10 {
        let client = Arc::clone(&client);
        tasks.push(tokio::spawn(async move {
            let row = client.query_one("SELECT pg_backend_pid()", &[]).await?;
            Ok::<i32, tokio_postgres::Error>(row.get(0))
        }));
    }
    let mut pids = Vec::new();
    for task in tasks {
        pids.push(task.await.expect("no panic")?);
    }
    pids.dedup();
    assert_eq!(pids.len(), 1, "one connection, one server process");
    Ok(())
}
```

All ten requests went through one connection, served by one server process. Chapter 7 shows
that they did not wait for each other.

**3.** A `SET` sticks to the session. On a pooled connection it outlives your use of it and
surprises the next borrower, and it is easy to forget to undo it on an error path. Settings
in `options` are part of how every connection is made, before any statement runs, so they
cannot be skipped, left over or applied late. The other difference is cost: `SET` is one more
round trip per connection.

**4.** It does: the server sends a fresh *ParameterStatus* message when a reported setting
changes, and the connection updates its copy. To look at it afterwards you must still own the
connection, so poll it by reference, in the same task, while the `SET` runs:

```rust
use tokio_postgres::NoTls;

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL is set");
    let (client, mut connection) = tokio_postgres::connect(&url, NoTls).await?;
    let before = connection.parameter("TimeZone").map(str::to_owned);

    // `&mut connection`: polled here, but still ours afterwards.
    tokio::select! {
        result = client.batch_execute("SET TimeZone = 'Asia/Tokyo'") => result?,
        ended = &mut connection => panic!("the connection ended: {ended:?}"),
    }
    assert_eq!(connection.parameter("TimeZone"), Some("Asia/Tokyo"));
    assert_ne!(before.as_deref(), Some("Asia/Tokyo"));
    Ok(())
}
```

The server sends the new value just before it reports that the `SET` is done, so by the time
`batch_execute` returns, the connection has already read it.

---

Previous: [Chapter 0 — Introduction and setup](00-introduction.md) ·
Next: [Chapter 2 — Running SQL: execute, query and parameters](02-queries.md)
