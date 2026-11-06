# Chapter 12 — Production connections: timeouts, cancellation, health and TLS

> **Async PostgreSQL in Rust** · Previous: [Chapter 11 — LISTEN/NOTIFY and notices](11-listen-notify.md) ·
> Next: [Chapter 13 — Connection pooling](13-pooling.md)

On a laptop, a connection is a socket to `localhost` that never fails, and every query
finishes. In production, queries run long because a table grew, locks are held by someone
else, the network between the service and the database drops packets or disappears, the
database fails over to a replica, and the traffic crosses networks that you do not own, which
is why it must be encrypted.

This chapter is about the settings and techniques that keep a service healthy through all of
that: what really happens when you stop waiting for a query, how to cancel one, how to make the
server enforce limits, how to notice that a connection has died, how to connect to several
hosts, and how to connect over TLS, which tokio-postgres treats differently from libpq. The
last section opens the connection yourself, with `connect_raw`.

---

## 12.1 Dropping a future does not cancel the query

`tokio::time::timeout` around a query looks like a query timeout. It is not one. When the
timeout fires, it drops the query's future, so *your code* stops waiting. The request was
already sent, though, and the server knows nothing about your future: it keeps executing the
statement. And since the connection answers requests in order (Chapter 7), the next query on
that connection waits until the abandoned one finishes:

```rust
use std::time::{Duration, Instant};

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = playground::connect().await?;
    let observer = playground::connect().await?;
    let pid: i32 = client
        .query_one("SELECT pg_backend_pid()", &[])
        .await?
        .get(0);

    let started = Instant::now();
    let gave_up = tokio::time::timeout(
        Duration::from_millis(100),
        client.execute("SELECT pg_sleep(1)", &[]),
    )
    .await;
    assert!(gave_up.is_err()); // we stopped waiting...

    // ...but the server did not stop working,
    let row = observer
        .query_one(
            "SELECT state, query FROM pg_stat_activity WHERE pid = $1",
            &[&pid],
        )
        .await?;
    let (state, query): (Option<String>, Option<String>) = (row.get(0), row.get(1));
    println!("server: {state:?}, {query:?}");
    assert_eq!(state.as_deref(), Some("active"));

    // and the next query on the connection waits for it.
    client.query_one("SELECT 1", &[]).await?;
    let elapsed = started.elapsed();
    println!("the next query answered after {elapsed:?}");
    assert!(elapsed > Duration::from_millis(800));
    Ok(())
}
```

```text
server: Some("active"), Some("SELECT pg_sleep(1)")
the next query answered after 1.003539792s
```

> **Pitfall:** the same happens when axum drops a handler's future because the client hung up,
> or when a `select!` picks another branch. The query you abandoned keeps the server busy,
> holds its locks, and, on a pooled connection, delays whoever borrows the connection next.
> Abandoning a query is free for your code and not free for anyone else.

## 12.2 `CancelToken`: asking the server to stop

To stop a running statement you must ask the server, and you cannot ask on the busy connection:
it is waiting for the answer. PostgreSQL's protocol has a separate mechanism
([Canceling Requests in Progress](https://www.postgresql.org/docs/18/protocol-flow.html#PROTOCOL-FLOW-CANCELING-REQUESTS)):
the client "opens a new connection to the server and sends a CancelRequest message", which
names the backend by its process ID and a secret key the server handed out at startup.

[`Client::cancel_token`](https://docs.rs/tokio-postgres/0.7.18/tokio_postgres/struct.Client.html#method.cancel_token)
returns a
[`CancelToken`](https://docs.rs/tokio-postgres/0.7.18/tokio_postgres/struct.CancelToken.html)
holding exactly that: the key, plus how to reach the server. It is `Clone` and `Send`, and it
does not borrow the client, so it can go to whichever task decides that a query has run long
enough. `cancel_query(tls)` connects and sends the request; the statement then fails with
SQLSTATE 57014, `query_canceled`:

```rust
use std::sync::Arc;
use std::time::Duration;
use tokio_postgres::NoTls;
use tokio_postgres::error::SqlState;

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = Arc::new(playground::connect().await?);
    let token = client.cancel_token();

    let slow = {
        let client = Arc::clone(&client);
        tokio::spawn(async move { client.execute("SELECT pg_sleep(5)", &[]).await })
    };
    tokio::time::sleep(Duration::from_millis(100)).await;
    token.cancel_query(NoTls).await?;

    let err = slow.await.expect("no panic").unwrap_err();
    println!("{}", err.as_db_error().expect("a server error").message());
    assert_eq!(err.code(), Some(&SqlState::QUERY_CANCELED)); // 57014

    // Only the statement was cancelled; the connection is fine.
    client.query_one("SELECT 1", &[]).await?;
    Ok(())
}
```

```text
canceling statement due to user request
```

The `tls` argument is a connector of the same kind as the connection's (`NoTls` here), because
the cancel request travels on a new connection with the same TLS settings. `Client::cancel_query`
also exists, deprecated since 0.6.0 in favour of the token.

> **Protocol lens:** the server never answers a cancel request: "for reasons of both security
> and efficiency, the frontend has no direct way to tell whether a cancel request has
> succeeded". `cancel_query` returns once the request is sent. The effect is a signal to the
> backend process, and the documentation is candid about the timing: "if it arrives after the
> backend has finished processing the query, then it will have no effect." The proof that it
> worked is the 57014 error on the original query.

## 12.3 Timeouts that stop the work

### A timeout that cancels

Put the two together: wait with a timeout, and when it fires, cancel *and keep waiting* for
the query to end. Waiting matters. The query then fails with 57014 (or succeeds, if it finished
just as the cancel was sent), and when the helper returns, the connection is free for the next
request:

```rust
use std::future::Future;
use std::pin::pin;
use std::time::Duration;
use tokio_postgres::error::SqlState;
use tokio_postgres::{Client, Error, NoTls};

/// Runs `query`, and asks the server to cancel it if it takes longer than `limit`. After a
/// cancel it still waits for the query to end, so the connection is idle when this returns.
async fn cancel_after<T>(
    client: &Client,
    limit: Duration,
    query: impl Future<Output = Result<T, Error>>,
) -> Result<T, Error> {
    let mut query = pin!(query);
    match tokio::time::timeout(limit, query.as_mut()).await {
        Ok(result) => result,
        Err(_elapsed) => {
            client.cancel_token().cancel_query(NoTls).await?;
            query.await // usually 57014 now
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let client = playground::connect().await?;
    let limit = Duration::from_millis(200);

    let fast = cancel_after(&client, limit, client.query_one("SELECT 1", &[])).await?;
    assert_eq!(fast.get::<_, i32>(0), 1);

    let err = cancel_after(&client, limit, client.execute("SELECT pg_sleep(5)", &[]))
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some(&SqlState::QUERY_CANCELED));
    Ok(())
}
```

The helper has one gap, which follows from the protocol lens above. If the query finishes at
the very moment the cancel request is sent, the request may reach the backend while it runs
the connection's *next* statement, and cancel that one instead. Code that owns its connection
can live with a rare spurious 57014. A pool cannot hand that risk to the next borrower, which
is why stockroom discards a connection after cancelling on it (§ In stockroom).

### Limits enforced by the server

A client-side timeout needs the client to be alive and well. Server-side limits work even when
it is not, and they are the first line of defence:

| Setting | Ends | Error |
|---|---|---|
| `statement_timeout` | a statement that runs longer than this | 57014, the statement |
| `lock_timeout` | a wait for a lock (row, table, advisory) longer than this | 55P03, the statement |
| `idle_in_transaction_session_timeout` | a session idle inside a transaction for longer than this | 25P03, the whole session |
| `transaction_timeout` (PG 17+) | a transaction older than this, idle or not | 25P04, the whole session |

There are three places to set them. `SET LOCAL` changes a setting until the end of the current
transaction, which is the right scope for "this report may take 30 seconds". A startup option
in `Config::options` (`-c name=value`, as `PGOPTIONS` does for psql) sets it for every
statement of the session from the first. And `ALTER ROLE … SET` makes it the default for every
session of a role.

```rust
use tokio_postgres::error::SqlState;
use tokio_postgres::{Config, NoTls};

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    // For one transaction: SET LOCAL ends with the transaction.
    let mut client = playground::connect().await?;
    let tx = client.transaction().await?;
    tx.batch_execute("SET LOCAL statement_timeout = '100ms'")
        .await?;
    let err = tx.execute("SELECT pg_sleep(1)", &[]).await.unwrap_err();
    println!("{}", err.as_db_error().expect("a server error").message());
    assert_eq!(err.code(), Some(&SqlState::QUERY_CANCELED));
    drop(tx); // rolled back, and the setting with it
    let after: String = client
        .query_one("SHOW statement_timeout", &[])
        .await?
        .get(0);
    assert_eq!(after, "0");

    // For a whole session: startup options, in force before the first query.
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
    let mut config: Config = url.parse()?;
    config.options("-c statement_timeout=100ms -c lock_timeout=50ms");
    let (guarded, connection) = config.connect(NoTls).await?;
    tokio::spawn(async move {
        if let Err(e) = connection.await {
            eprintln!("connection error: {e}");
        }
    });
    let settings: Vec<String> = guarded
        .query(
            "SELECT setting FROM pg_settings
             WHERE name IN ('lock_timeout', 'statement_timeout') ORDER BY name",
            &[],
        )
        .await?
        .iter()
        .map(|row| row.get(0))
        .collect();
    assert_eq!(settings, ["50", "100"]); // in milliseconds

    // lock_timeout: wait at most 50 ms for an advisory lock that someone else holds.
    client.execute("SELECT pg_advisory_lock(912)", &[]).await?;
    let err = guarded
        .execute("SELECT pg_advisory_lock(912)", &[])
        .await
        .unwrap_err();
    println!("{}", err.as_db_error().expect("a server error").message());
    assert_eq!(err.code(), Some(&SqlState::LOCK_NOT_AVAILABLE)); // 55P03
    client
        .execute("SELECT pg_advisory_unlock(912)", &[])
        .await?;
    Ok(())
}
```

```text
canceling statement due to statement timeout
canceling statement due to lock timeout
```

The last two settings in the table end the *session*, because a transaction cannot be safely
continued once the server has given up on it. The client sees its connection close:

```rust
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let mut client = playground::connect().await?;
    client
        .batch_execute("SET idle_in_transaction_session_timeout = '200ms'")
        .await?;
    let tx = client.transaction().await?;
    tx.execute("SELECT 1", &[]).await?;
    // A slow HTTP call, made while the transaction is open...
    tokio::time::sleep(Duration::from_millis(500)).await;
    // ...and the server has ended the session meanwhile.
    let err = tx.execute("SELECT 1", &[]).await.unwrap_err();
    println!("{err}");
    assert!(err.is_closed());
    Ok(())
}
```

```text
connection error: db error
connection closed
```

The first line comes from `playground::connect`'s connection task, which received the server's
FATAL message (25P03) as the connection's final error; the query sees only "connection closed".
`transaction_timeout` reports its FATAL 25P04 to the running statement. Note that 0.7.18's
`SqlState` has no named constant for 25P04, which is newer than the crate's table:
compare `code().code()` with `"25P04"` if you need to match it.

stockroom sets `statement_timeout`, `lock_timeout` and `idle_in_transaction_session_timeout`
for every session (§ In stockroom). The last one is the safety net for a bug that otherwise
causes slow, mysterious trouble: a code path that starts a transaction and then awaits
something slow, holding locks and a snapshot all the while.

## 12.4 Dead connections

A connection can die in two ways, and they are noticed very differently.

**Loudly.** The server ends the session (an administrator's `pg_terminate_backend`, a
restart, a session timeout) and sends a FATAL message, or the operating system resets the TCP
connection. The connection task reads that at once, finishes with an error, and
`Client::is_closed()` turns `true`:

```rust
use std::time::{Duration, Instant};

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = playground::connect().await?;
    let admin = playground::connect().await?;
    let pid: i32 = client
        .query_one("SELECT pg_backend_pid()", &[])
        .await?
        .get(0);
    client.check_connection().await?; // one round trip, no statement

    admin
        .execute("SELECT pg_terminate_backend($1)", &[&pid])
        .await?;
    let started = Instant::now();
    while !client.is_closed() && started.elapsed() < Duration::from_secs(5) {
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    println!("is_closed() after {:?}", started.elapsed());
    assert!(client.is_closed());
    let err = client.check_connection().await.unwrap_err();
    assert!(err.is_closed());
    Ok(())
}
```

```text
connection error: db error
is_closed() after 2.552166ms
```

`is_closed()` costs nothing and asks no one: it reports whether the connection task has ended.
[`check_connection()`](https://docs.rs/tokio-postgres/0.7.18/tokio_postgres/struct.Client.html#method.check_connection)
(0.7.15+) asks the server, with one round trip and no statement, so it also catches a
connection that died a moment ago and whose task has not noticed yet. Chapter 13 weighs the two
for a pool.

**Silently.** A network path that disappears (a NAT that forgets the connection, a cable, a
cloud load balancer's idle timeout, a virtual machine that freezes) sends nothing. An idle
connection then looks alive forever, and a query sent on it waits until TCP gives up, which
takes many minutes by default. Three settings shorten that:

| `Config` setting | Default in 0.7.18 | What it detects |
|---|---|---|
| `keepalives(bool)` | `true` | turns TCP keepalive probes on |
| `keepalives_idle(Duration)` | 2 hours | how long an idle connection waits before the first probe |
| `keepalives_interval(Duration)`, `keepalives_retries(u32)` | the operating system's | the time between probes, and how many may go unanswered |
| `tcp_user_timeout(Duration)` | none | how long sent data may stay unacknowledged; **Linux only** (`connect_socket.rs` sets it only there) |
| `connect_timeout(Duration)` | none | how long one connection attempt may take, per host |

With keepalives probing after 30 seconds, every 10 seconds, 3 times, a dead idle connection
is found within about a minute, as in stockroom's settings. A query timeout (§12.3) covers
the case keepalives cannot: a connection that is not idle but waiting for an answer.

In code, with each setting spelled out, plus `host_path`, which connects through a Unix-domain
socket instead of TCP where the server offers one (its directory is `unix_socket_directories` in
the server's configuration; this book's server uses `/tmp`):

```rust
use std::path::Path;
use std::time::Duration;
use tokio_postgres::config::LoadBalanceHosts;
use tokio_postgres::{Config, NoTls};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut config: Config = std::env::var("DATABASE_URL")?.parse()?;
    config
        .keepalives(true)
        .keepalives_idle(Duration::from_secs(30))
        .keepalives_interval(Duration::from_secs(10))
        .keepalives_retries(3)
        .tcp_user_timeout(Duration::from_secs(60)) // applied on Linux only
        .connect_timeout(Duration::from_secs(5))
        .load_balance_hosts(LoadBalanceHosts::Disable);
    let (client, connection) = config.connect(NoTls).await?;
    tokio::spawn(connection);
    client.check_connection().await?;
    assert_eq!(config.get_keepalives_idle(), Duration::from_secs(30));

    // The same server through its Unix-domain socket, if it has one in /tmp: `host_path` names
    // the directory, and the port picks the socket file in it.
    let port = config.get_ports()[0];
    let dir = Path::new("/tmp");
    if dir.join(format!(".s.PGSQL.{port}")).exists() {
        let mut local = Config::new();
        local
            .user(config.get_user().expect("a user"))
            .password(config.get_password().expect("a password"))
            .dbname(config.get_dbname().expect("a database"))
            .host_path(dir)
            .port(port);
        let (client, connection) = local.connect(NoTls).await?;
        tokio::spawn(connection);
        // No client IP address: the connection did not use TCP.
        let address: Option<String> = client
            .query_one("SELECT inet_client_addr()::text", &[])
            .await?
            .get(0);
        assert_eq!(address, None);
        println!(
            "connected through {}",
            dir.join(format!(".s.PGSQL.{port}")).display()
        );
    }
    Ok(())
}
```

```text
connected through /tmp/.s.PGSQL.5433
```

What to do about a dead connection depends on who owns it. A pool discards it and opens
another (Chapter 13). A long-lived connection of your own, like Chapter 11's listener, needs a
reconnect loop, with a backoff so that a database that is down is not hammered with attempts.
pglisten's is typical:

~~~rust
// apps/pglisten/src/lib.rs (excerpt)
let mut delay = Duration::from_millis(100);
loop {
    let started = Instant::now();
    let error = match session(&config, &options, &mut on_event, &shutdown).await {
        Ok(()) => return, // shut down
        Err(error) => error,
    };
    if started.elapsed() > Duration::from_secs(60) {
        delay = Duration::from_millis(100); // a new failure, not the same one again
    }
    // … report the loss, then sleep for `delay` unless shut down …
    delay = (delay * 2).min(Duration::from_secs(30));
}
~~~

[apps/pglisten/src/lib.rs](../apps/pglisten/src/lib.rs)

## 12.5 Several hosts

A `Config` holds a list of hosts, and `connect` tries them in order until one accepts, with
`connect_timeout` applying to each. In a URL they are comma-separated
(`postgres://user@db1:5432,db2:5432/app`), and in key/value form, `host=db1,db2 port=5432,5432`.
Two settings refine the choice:

- `target_session_attrs` (`Any`, `ReadWrite`, `ReadOnly`) checks each server after connecting
  and moves on if it is not the kind asked for. `ReadWrite` finds the primary of a replicated
  cluster after a failover. In 0.7.18 the check is a `SHOW transaction_read_only` query on each
  new connection (`connect.rs`), so it costs one round trip per host tried.
- `load_balance_hosts` (`Disable`, `Random`) shuffles the list first, spreading clients over
  read replicas.

```rust
use tokio_postgres::config::{Host, TargetSessionAttrs};
use tokio_postgres::{Config, NoTls};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::var("DATABASE_URL")?;
    let base: Config = url.parse()?;
    let Host::Tcp(host) = &base.get_hosts()[0] else {
        panic!("a TCP host")
    };

    // `host` *appends*: this is a two-host configuration now, not a redirected one.
    let mut appended = base.clone();
    appended.host("replica.example.internal");
    assert_eq!(appended.get_hosts().len(), 2);

    // A first host where nothing listens, then the real server: the second one is used.
    let mut config = Config::new();
    config
        .user(base.get_user().expect("a user"))
        .password(base.get_password().expect("a password"))
        .dbname(base.get_dbname().expect("a database"))
        .host("localhost")
        .port(1)
        .host(host)
        .port(base.get_ports()[0]);
    let (client, connection) = config.connect(NoTls).await?;
    tokio::spawn(async move {
        if let Err(e) = connection.await {
            eprintln!("connection error: {e}");
        }
    });
    let port: i32 = client
        .query_one("SELECT inet_server_port()", &[])
        .await?
        .get(0);
    assert_eq!(port, i32::from(base.get_ports()[0]));

    // Ask for a read-only server: this one is a primary, so no host qualifies.
    config.target_session_attrs(TargetSessionAttrs::ReadOnly);
    let Err(err) = config.connect(NoTls).await else {
        panic!("this connection attempt should fail")
    };
    println!(
        "{err}: {}",
        std::error::Error::source(&err).expect("a cause")
    );
    Ok(())
}
```

```text
error connecting to server: database is not read only
```

The `host` call in the middle of that example is a trap worth remembering: `Config::host` and
`Config::port` add to the lists, so "parse the URL, then point it somewhere else" keeps the
original host first and silently connects there. Build a new `Config`, as the example does,
when you need different hosts.

## 12.6 TLS

### What `sslmode` means in tokio-postgres

In libpq, `sslmode` has six values, and `verify-ca` and `verify-full` make libpq check the
server's certificate. tokio-postgres splits the job in two. The connection string decides
*whether* TLS is used, with three values only
([`SslMode`](https://docs.rs/tokio-postgres/0.7.18/tokio_postgres/config/enum.SslMode.html)):

| `sslmode` | Meaning |
|---|---|
| `disable` | never TLS |
| `prefer` (the default) | TLS if the connector and the server both support it; otherwise plaintext, silently |
| `require` | TLS, or fail |

And the **connector** you pass to `connect` decides *how* TLS is done, including whether the
certificate is trusted. `NoTls` cannot do TLS at all. The rustls, native-tls and OpenSSL
connectors verify the certificate chain and the host name by default, which is libpq's
`verify-full`. So `sslmode=require` with a verifying connector is the secure setting, and the
two common mistakes look like this, against this book's server (which has TLS turned off):

```rust
use std::sync::Arc;
use tokio_postgres::Config;
use tokio_postgres::config::{ChannelBinding, SslMode};

fn a_rustls_connector() -> tokio_postgres_rustls::MakeRustlsConnect {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("the ring provider supports the default versions")
        .with_root_certificates(rustls::RootCertStore::empty())
        .with_no_client_auth();
    tokio_postgres_rustls::MakeRustlsConnect::new(config)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config: Config = std::env::var("DATABASE_URL")?.parse()?;

    // 1. A TLS connector, and the default sslmode=prefer: plaintext, without a word.
    let (client, connection) = config.connect(a_rustls_connector()).await?;
    tokio::spawn(connection);
    let ssl: bool = client
        .query_one(
            "SELECT ssl FROM pg_stat_ssl WHERE pid = pg_backend_pid()",
            &[],
        )
        .await?
        .get(0);
    assert!(!ssl, "connected, but not encrypted");

    // 2. sslmode=require, as it should be: now the missing TLS is an error.
    let mut required = config.clone();
    required.ssl_mode(SslMode::Require);
    let Err(err) = required.connect(a_rustls_connector()).await else {
        panic!("this connection attempt should fail")
    };
    println!(
        "{err}: {}",
        std::error::Error::source(&err).expect("a cause")
    );

    // 3. NoTls with sslmode=require: the server says "no TLS" first, so the same error. (A
    //    server that offers TLS gets "no TLS implementation configured" instead.)
    let Err(err) = required.connect(tokio_postgres::NoTls).await else {
        panic!("this connection attempt should fail")
    };
    println!(
        "{err}: {}",
        std::error::Error::source(&err).expect("a cause")
    );

    // Channel binding (SCRAM-SHA-256-PLUS) needs TLS too: requiring it over plaintext fails.
    let mut binding = config.clone();
    binding.channel_binding(ChannelBinding::Require);
    let Err(err) = binding.connect(tokio_postgres::NoTls).await else {
        panic!("this connection attempt should fail")
    };
    println!(
        "{err}: {}",
        std::error::Error::source(&err).expect("a cause")
    );
    Ok(())
}
```

```text
error performing TLS handshake: server does not support TLS
error performing TLS handshake: server does not support TLS
authentication error: server did not use channel binding
```

> **Pitfall:** with `sslmode=prefer`, any server that does not offer TLS, or anything in the
> network pretending to be that server, gets a plaintext connection, password exchange
> included. Put `sslmode=require` in every production connection string, and pass a connector
> that verifies.

### The connectors

The connector is anything that implements
[`MakeTlsConnect<Socket>`](https://docs.rs/tokio-postgres/0.7.18/tokio_postgres/tls/trait.MakeTlsConnect.html):
given the host name, `make_tls_connect` returns a
[`TlsConnect`](https://docs.rs/tokio-postgres/0.7.18/tokio_postgres/tls/trait.TlsConnect.html),
whose `connect` turns the socket into a TLS stream. The stream type also reports the
certificate data that channel binding needs. You will rarely implement these traits; three
crates do, for the three common TLS libraries:

| Crate (version checked 2026-11-06) | TLS library | Trusts by default |
|---|---|---|
| `tokio-postgres-rustls` 0.14.0 | rustls, pure Rust | what you put in its `ClientConfig` |
| `postgres-native-tls` 0.5.3 | the platform's: Security.framework, SChannel or OpenSSL | the operating system's store |
| `postgres-openssl` 0.5.3 | OpenSSL | OpenSSL's configured store |

This book uses rustls, which needs no system library and builds the same everywhere. The
example below connects to the TLS server of `docker compose` (on port 5434, accepting TLS
connections only), trusting the certificate authority that `docker/gen-certs.sh` created:

<!-- check: needs=tls -->
```rust
use rustls::pki_types::CertificateDer;
use rustls::pki_types::pem::PemObject;
use std::sync::Arc;
use tokio_postgres_rustls::MakeRustlsConnect;

/// A rustls connector that trusts the certificate authorities in a PEM file.
fn rustls_connector(ca_file: &str) -> Result<MakeRustlsConnect, Box<dyn std::error::Error>> {
    let mut roots = rustls::RootCertStore::empty();
    for cert in CertificateDer::pem_file_iter(ca_file)? {
        roots.add(cert?)?;
    }
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(MakeRustlsConnect::new(config))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // For this example DATABASE_URL is the TLS server's: …@localhost:5434/tpbook?sslmode=require
    let url = std::env::var("DATABASE_URL")?;
    let ca_file = std::env::var("TLS_CA_FILE")?;
    let (client, connection) = tokio_postgres::connect(&url, rustls_connector(&ca_file)?).await?;
    tokio::spawn(async move {
        if let Err(e) = connection.await {
            eprintln!("connection error: {e}");
        }
    });
    let row = client
        .query_one(
            "SELECT ssl, version, cipher FROM pg_stat_ssl WHERE pid = pg_backend_pid()",
            &[],
        )
        .await?;
    let (ssl, version, cipher): (bool, String, String) = (row.get(0), row.get(1), row.get(2));
    println!("ssl: {ssl}, {version}, {cipher}");
    assert!(ssl);
    Ok(())
}
```

```text
ssl: true, TLSv1.3, TLS_AES_256_GCM_SHA384
```

To run it yourself: `docker/gen-certs.sh && docker compose up -d --wait`, then set
`TLS_DATABASE_URL` and `TLS_CA_FILE` as in `.env.example`, and point `DATABASE_URL` at the
former. In production, trust the roots your platform provides (stockroom loads them with
`rustls-native-certs`) or the CA of your database provider.

Verification fails in the two ways it should. An unknown certificate authority, and a
certificate for another name, are both refused during the handshake; `hostaddr` makes the
second one easy to show, by connecting to 127.0.0.1 while presenting another host name:

<!-- check: needs=tls -->
```rust
use std::sync::Arc;
use tokio_postgres::Config;
use tokio_postgres::config::SslMode;

fn connector(roots: rustls::RootCertStore) -> tokio_postgres_rustls::MakeRustlsConnect {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("the ring provider supports the default versions")
        .with_root_certificates(roots)
        .with_no_client_auth();
    tokio_postgres_rustls::MakeRustlsConnect::new(config)
}

fn trusted_roots() -> Result<rustls::RootCertStore, Box<dyn std::error::Error>> {
    use rustls::pki_types::{CertificateDer, pem::PemObject};
    let mut roots = rustls::RootCertStore::empty();
    for cert in CertificateDer::pem_file_iter(std::env::var("TLS_CA_FILE")?)? {
        roots.add(cert?)?;
    }
    Ok(roots)
}

fn cause(err: &tokio_postgres::Error) -> String {
    std::error::Error::source(err)
        .map(|e| e.to_string())
        .unwrap_or_default()
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config: Config = std::env::var("DATABASE_URL")?.parse()?;

    // A connector that trusts nobody: the server's certificate has an unknown issuer.
    let Err(err) = config
        .connect(connector(rustls::RootCertStore::empty()))
        .await
    else {
        panic!("this connection attempt should fail")
    };
    println!("{}", cause(&err));

    // The right CA, but the name we connect by is not on the certificate.
    let mut renamed = Config::new();
    renamed
        .user(config.get_user().expect("a user"))
        .password(config.get_password().expect("a password"))
        .dbname(config.get_dbname().expect("a database"))
        .host("db.example.internal") // the name TLS checks
        .hostaddr("127.0.0.1".parse()?) // the address TCP uses
        .port(config.get_ports()[0])
        .ssl_mode(SslMode::Require);
    let Err(err) = renamed.connect(connector(trusted_roots()?)).await else {
        panic!("this connection attempt should fail")
    };
    println!("{}", cause(&err));

    // NoTls against a server that accepts TLS connections only.
    let mut plain = config.clone();
    plain.ssl_mode(SslMode::Prefer);
    let Err(err) = plain.connect(tokio_postgres::NoTls).await else {
        panic!("this connection attempt should fail")
    };
    println!("{}", err.as_db_error().expect("a server error").message());
    Ok(())
}
```

```text
invalid peer certificate: UnknownIssuer
invalid peer certificate: certificate not valid for name "db.example.internal"; certificate is only valid for DnsName("localhost") or IpAddress(127.0.0.1)
no pg_hba.conf entry for host "192.168.65.1", user "postgres", database "tpbook", no encryption
```

(The last message is the server's: its `pg_hba.conf` accepts only `hostssl` connections from
outside the container, so a plaintext connection is refused before authentication.)

### Direct TLS and channel binding

Two newer options make TLS connections faster and harder to intercept:

- **`sslnegotiation=direct`** (PG 17+ servers). Normally the client first asks "do you speak
  TLS?" (an *SSLRequest*) and waits for the answer before starting the handshake. With direct
  negotiation it starts the handshake at once, saving a round trip. The server then insists on
  the TLS ALPN extension with the protocol name `postgresql`
  ([protocol docs](https://www.postgresql.org/docs/18/protocol-flow.html#PROTOCOL-FLOW-SSL)),
  and tokio-postgres-rustls 0.14 does not set it: add it to the `ClientConfig` yourself, or the
  server closes the connection. (postgres-native-tls and postgres-openssl have a
  `set_postgresql_alpn()` method for this.)
- **`channel_binding=require`**. With SCRAM authentication over TLS, the client can bind the
  password exchange to the server's certificate (SCRAM-SHA-256-PLUS), so that a
  man-in-the-middle holding a different certificate cannot relay the authentication. The
  default, `prefer`, uses it when it can, which also means it hides a connector that never
  can: tokio-postgres-rustls 0.13 had exactly that bug, fixed in 0.14 ([Appendix F](19-appendix.md#f-the-crates-built-on-tokio-postgres)).

<!-- check: needs=tls -->
```rust
use rustls::pki_types::{CertificateDer, pem::PemObject};
use std::sync::Arc;
use tokio_postgres::Config;
use tokio_postgres::config::{ChannelBinding, SslNegotiation};

fn connector(
    alpn: bool,
) -> Result<tokio_postgres_rustls::MakeRustlsConnect, Box<dyn std::error::Error>> {
    let mut roots = rustls::RootCertStore::empty();
    for cert in CertificateDer::pem_file_iter(std::env::var("TLS_CA_FILE")?)? {
        roots.add(cert?)?;
    }
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .with_root_certificates(roots)
        .with_no_client_auth();
    if alpn {
        config.alpn_protocols = vec![b"postgresql".to_vec()];
    }
    Ok(tokio_postgres_rustls::MakeRustlsConnect::new(config))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut config: Config = std::env::var("DATABASE_URL")?.parse()?; // sslmode=require
    config
        .ssl_negotiation(SslNegotiation::Direct)
        .channel_binding(ChannelBinding::Require);

    // Without ALPN, the server hangs up on a direct TLS connection.
    let Err(err) = config.connect(connector(false)?).await else {
        panic!("this connection attempt should fail")
    };
    println!("without ALPN: {err}");
    assert!(err.is_closed());

    // With it: one round trip less, and the authentication bound to the certificate.
    let (client, connection) = config.connect(connector(true)?).await?;
    tokio::spawn(connection);
    let ssl: bool = client
        .query_one(
            "SELECT ssl FROM pg_stat_ssl WHERE pid = pg_backend_pid()",
            &[],
        )
        .await?
        .get(0);
    println!("with ALPN: ssl = {ssl}");
    assert!(ssl);
    Ok(())
}
```

```text
without ALPN: connection closed
with ALPN: ssl = true
```

## 12.7 Opening the connection yourself: `connect_raw`

`tokio_postgres::connect` and `Config::connect` do the networking: resolve the host, open a TCP
or Unix socket, set its options, try the next host on failure. That is the `runtime` feature,
on by default, which brings `connect`, `Config::connect`, the `Socket` type, and
`CancelToken::cancel_query`. Underneath,
[`Config::connect_raw`](https://docs.rs/tokio-postgres/0.7.18/tokio_postgres/config/struct.Config.html#method.connect_raw)
runs the PostgreSQL startup over any stream you give it: anything `AsyncRead + AsyncWrite +
Unpin`, a TCP stream you opened yourself, a stream through a proxy or tunnel, an in-memory
pipe in a test. lagproxy's users (pgload, minipool) connect that way, and without the `runtime`
feature it is the only way to connect at all.

```rust
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio_postgres::config::Host;
use tokio_postgres::error::SqlState;
use tokio_postgres::{Config, NoTls};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config: Config = std::env::var("DATABASE_URL")?.parse()?;
    let Host::Tcp(host) = &config.get_hosts()[0] else {
        panic!("a TCP host")
    };
    let address = (host.as_str(), config.get_ports()[0]);

    let stream = TcpStream::connect(address).await?;
    assert!(!stream.nodelay()?); // the operating system's default: Nagle's algorithm on
    stream.set_nodelay(true)?; // as `connect` would have done
    let (client, connection) = config.connect_raw(stream, NoTls).await?;
    tokio::spawn(async move {
        if let Err(e) = connection.await {
            eprintln!("connection error: {e}");
        }
    });
    let client = Arc::new(client);
    let one: i32 = client.query_one("SELECT 1", &[]).await?.get(0);
    assert_eq!(one, 1);

    // Cancelling works the same way: bring your own stream to cancel_query_raw.
    let slow = {
        let client = Arc::clone(&client);
        tokio::spawn(async move { client.execute("SELECT pg_sleep(5)", &[]).await })
    };
    tokio::time::sleep(Duration::from_millis(100)).await;
    client
        .cancel_token()
        .cancel_query_raw(TcpStream::connect(address).await?, NoTls)
        .await?;
    let err = slow.await?.unwrap_err();
    assert_eq!(err.code(), Some(&SqlState::QUERY_CANCELED));
    Ok(())
}
```

`connect_raw` uses the stream exactly as given, and that is the catch. `connect` sets
`TCP_NODELAY` on every socket (`connect_socket.rs`), because PostgreSQL's small request and
response messages suffer badly from Nagle's algorithm; with `connect_raw`, setting it is your
job, and so are keepalives and the connect timeout. Of the connection settings in the `Config`,
`connect_raw` uses only those of the startup itself: user, password, database name, options and
application name, plus the TLS mode and channel binding.

### Side by side — *a query timeout*

The task: make a query that runs longer than 100 ms fail, and stop it on the server.

```sql
tpbook=# SET statement_timeout = '100ms';
SET
tpbook=# SELECT pg_sleep(1);
ERROR:  canceling statement due to statement timeout
```

~~~rust
// sqlx (illustration)
let mut conn = pool.acquire().await?;
sqlx::query("SET statement_timeout = '100ms'").execute(&mut *conn).await?;
let err = sqlx::query("SELECT pg_sleep(1)").execute(&mut *conn).await.unwrap_err();
// err.as_database_error().and_then(|e| e.code()) == Some("57014".into())
~~~

```rust
use tokio_postgres::error::SqlState;

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = playground::connect().await?;
    client
        .batch_execute("SET statement_timeout = '100ms'")
        .await?;
    let err = client.execute("SELECT pg_sleep(1)", &[]).await.unwrap_err();
    assert_eq!(err.code(), Some(&SqlState::QUERY_CANCELED));
    Ok(())
}
```

| | psql | sqlx | tokio-postgres |
|---|---|---|---|
| **Same** | `statement_timeout` on the server: the statement fails with 57014 and the session goes on | `statement_timeout` on the server: the statement fails with 57014 and the session goes on | `statement_timeout` on the server: the statement fails with 57014 and the session goes on |
| **Different** | Ctrl-C sends a cancel request for the running statement | Set per connection, on a connection borrowed with `&mut` | Set per connection or per transaction; a `CancelToken` can also cancel from any task |

---

## In stockroom

Every stockroom session gets the same guard rails, applied to the `Config` before the pool or
the listener sees it. The connection string can override the application name and connect
timeout; the keepalives and server-side limits are always set:

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
    for setting in [
        format!("statement_timeout={}", self.statement_timeout_ms),
        "lock_timeout=2000".to_owned(),
        "idle_in_transaction_session_timeout=60000".to_owned(),
    ] {
        // … appended to `options` as "-c name=value" …
    }
    config.options(options);
    Ok(config)
}
~~~

[stockroom/src/config.rs](../stockroom/src/config.rs)

`GET /reports/sales` can be slow, and its request may be abandoned: the router's timeout
fires, or the client disconnects, and axum drops the handler's future. `CancelOnDrop` owns the
pooled connection while the report runs. If it is dropped before `finish`, it cancels the
query and then discards the connection instead of returning it, for the race of §12.3:

~~~rust
// stockroom/src/db/cancel.rs (excerpt)
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        let Some(conn) = self.conn.take() else { return };
        let token = conn.cancel_token();
        let tls = self.tls.clone();
        // `drop` cannot await, so the cancel request goes out from a task of its own.
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        runtime.spawn(async move {
            match token.cancel_query(tls).await {
                Ok(()) => tracing::info!("cancelled a query nobody was waiting for"),
                Err(error) => tracing::warn!(%error, "could not cancel an abandoned query"),
            }
            // Detach from the pool and close.
            drop(Object::take(conn));
        });
    }
}
~~~

[stockroom/src/db/cancel.rs](../stockroom/src/db/cancel.rs) ·
[stockroom/src/http/reports.rs](../stockroom/src/http/reports.rs)

When the router's timeout fires before a `?delayMs=5000` report is done, the client gets 503,
and `pg_stat_activity` shows no `pg_sleep` left running: §16.5 demonstrates it, and stockroom's
test `an_abandoned_report_is_cancelled_on_the_server` checks it with a 300 ms timeout.

TLS is a cargo feature. Without it, `MakeTls` is `NoTls`; with `--features tls`, it is
rustls, trusting `--db-ca-file` or the system's roots:

~~~rust
// stockroom/src/db/tls.rs (excerpt)
#[cfg(not(feature = "tls"))]
pub type MakeTls = tokio_postgres::NoTls;

#[cfg(feature = "tls")]
pub type MakeTls = tokio_postgres_rustls::MakeRustlsConnect;
~~~

[stockroom/src/db/tls.rs](../stockroom/src/db/tls.rs)

The rest of the code is written against `MakeTls` and compiles either way. One wrinkle:
`NoTls` is `Copy`, while rustls's connector is only `Clone`, so code that must work with both
calls `.clone()`, and clippy's `clone_on_copy` lint, which fires in the `NoTls` build, is
allowed once for the crate. The same binary runs against the TLS server:

```bash
cargo run -p stockroom --features tls -- --db-ca-file docker/certs/ca.crt migrate
```

## Summary

- Dropping a query future (a timeout, a `select!`, a disconnected HTTP client) does not stop
  the query: the server keeps working, and the connection's next request waits behind it.
- `client.cancel_token()` gives a `Clone + Send` token; `cancel_query(tls)` sends a cancel
  request on a new connection, and the query fails with 57014. The server never confirms, and
  a late request can hit the next statement.
- A client-side timeout should cancel and then wait for the query to end. Server-side,
  `statement_timeout` and `lock_timeout` end statements; `idle_in_transaction_session_timeout`
  and `transaction_timeout` (PG 17+) end sessions. Set them with `SET LOCAL`, startup options
  (`Config::options`) or `ALTER ROLE`.
- `is_closed()` reports a connection whose task has ended; `check_connection()` asks the server.
  Keepalives (default: first probe after 2 hours) and `tcp_user_timeout` (Linux) detect silent
  network failures; long-lived connections need a reconnect loop with backoff.
- Several hosts are tried in order; `target_session_attrs` picks a primary or a replica, and
  `load_balance_hosts` shuffles. `Config::host` appends.
- In tokio-postgres, `sslmode` (disable, prefer, require) decides whether TLS is used, and the
  connector decides whether the certificate is trusted. Use `sslmode=require` with a verifying
  connector; `prefer` falls back to plaintext silently.
- `sslnegotiation=direct` (PG 17+) needs ALPN `postgresql`, which you set on the rustls
  `ClientConfig`; `channel_binding=require` binds SCRAM to the TLS certificate.
- `connect_raw` runs the startup over a stream you opened; then `TCP_NODELAY`, keepalives and
  timeouts are yours to set.

## Exercises

1. **A timeout for a transaction.** Put a whole transaction (begin, two statements, commit),
   written as an `async` block, under a timeout that cancels. Why can't it be passed to
   `cancel_after` as written? What happens to the transaction when the cancel lands in the
   middle, and in what state is the connection afterwards?
   *Hint:* a cancelled statement is an error like any other; Chapter 8 said what an error does
   to a transaction, and what dropping a `Transaction` does.
2. **Who is waiting for whom?** Reproduce a lock wait between two sessions on a real table, and
   from a third session list the blocked and the blocking backend with `pg_blocking_pids()`.
   Then end it twice: once with a cancel request from the waiting client's `CancelToken`, once
   with `lock_timeout`.
   *Hint:* `SELECT pid, pg_blocking_pids(pid), wait_event_type FROM pg_stat_activity WHERE
   cardinality(pg_blocking_pids(pid)) > 0`.
3. **Reconnect.** Write a `connect_with_backoff(config: &Config) -> Client` that retries with
   delays of 100 ms, 200 ms, 400 ms … up to 5 s, and test it by starting it against a port where
   nothing listens.
   *Hint:* `tokio::time::timeout` around the whole retry loop keeps the test bounded.
4. **Prefer is not secure.** Explain why `sslmode=prefer` with a connector that verifies
   certificates still does not protect the password against an attacker on the network path.
   *Hint:* what does the attacker answer to the client's SSLRequest?

### Solutions

**1.** `cancel_after` takes the client by `&Client`, and the transaction needs it by `&mut`
(`transaction()` takes `&mut self`), so the two borrows cannot coexist. Take the
`CancelToken` first, since it borrows nothing, and inline the helper. The cancel makes the
running statement fail with 57014; the `?` in the async block returns that error, the
`Transaction` is dropped and rolls back, and only then does the timeout code get its answer,
so the connection is idle and outside any transaction:

```rust
use std::pin::pin;
use std::time::Duration;
use tokio_postgres::error::SqlState;
use tokio_postgres::{Error, NoTls};

#[tokio::main]
async fn main() -> Result<(), Error> {
    let mut client = playground::connect().await?;
    client
        .batch_execute("CREATE TEMP TABLE audit (note text)")
        .await?;

    let token = client.cancel_token(); // before the transaction borrows the client
    let err = {
        let work = async {
            let tx = client.transaction().await?;
            tx.execute("INSERT INTO audit VALUES ('started')", &[])
                .await?;
            tx.execute("SELECT pg_sleep(5)", &[]).await?; // cancelled here
            tx.commit().await
        };
        let mut work = pin!(work);
        match tokio::time::timeout(Duration::from_millis(200), work.as_mut()).await {
            Ok(result) => result.unwrap_err(),
            Err(_elapsed) => {
                token.cancel_query(NoTls).await?;
                work.await.unwrap_err()
            }
        }
    }; // the pinned block ends here, and with it the borrow of `client`
    assert_eq!(err.code(), Some(&SqlState::QUERY_CANCELED));

    // Rolled back, and the connection is usable, outside any transaction.
    let rows: i64 = client
        .query_one("SELECT count(*) FROM audit", &[])
        .await?
        .get(0);
    assert_eq!(rows, 0);
    Ok(())
}
```

The block around the pinned future is not decoration: `pin!` keeps the future, and its borrow
of `client`, alive until the end of the enclosing block (Chapter 9's pitfall), so without it the
last query would not compile.

**2.** The waiting session shows `wait_event_type = 'Lock'` and the blocker's PID in
`pg_blocking_pids`. A cancel request ends the wait with 57014 ("canceling statement due to user
request"), `lock_timeout` with 55P03 ("canceling statement due to lock timeout"); in both cases
only the waiting statement fails, and the blocker never notices.

**3.**

```rust
use std::time::Duration;
use tokio_postgres::{Client, Config, Error, NoTls};

async fn connect_with_backoff(config: &Config) -> Client {
    let mut delay = Duration::from_millis(100);
    loop {
        match config.connect(NoTls).await {
            Ok((client, connection)) => {
                tokio::spawn(async move {
                    if let Err(e) = connection.await {
                        eprintln!("connection error: {e}");
                    }
                });
                return client;
            }
            Err(error) => {
                eprintln!("connect failed: {error}; retrying in {delay:?}");
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(Duration::from_secs(5));
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    // Nothing listens on port 1: the loop keeps trying, and the timeout stops it.
    let mut unreachable = Config::new();
    unreachable.host("localhost").port(1).user("nobody");
    let attempt = tokio::time::timeout(
        Duration::from_millis(500),
        connect_with_backoff(&unreachable),
    )
    .await;
    assert!(attempt.is_err());

    // The real server answers on the first try.
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
    let client = connect_with_backoff(&url.parse()?).await;
    client.check_connection().await?;
    Ok(())
}
```

A production version adds jitter (Chapter 8) so that a hundred instances restarting together
do not retry in lockstep, and resets the delay after a connection has been healthy for a while,
as pglisten does.

**4.** With `prefer`, the client sends an SSLRequest and accepts either answer. An attacker in
the middle answers "no" (`N`), and the client continues in plaintext, sending its
authentication through the attacker; the connector never runs, so its careful verification
never happens. Only `require` turns "no TLS" into an error. (SCRAM never sends the password
itself, but a plaintext exchange can still be relayed to the real server, which is what
channel binding prevents, and every query and result after it is readable.)

---

Previous: [Chapter 11 — LISTEN/NOTIFY and notices](11-listen-notify.md) ·
Next: [Chapter 13 — Connection pooling](13-pooling.md)
