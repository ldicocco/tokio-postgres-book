# Chapter 16 — axum II: streaming, real-time and background work

> **Async PostgreSQL in Rust** · Previous: [Chapter 15 — axum I: a database-backed JSON API](15-axum-api.md) ·
> Next: [Chapter 17 — Testing against a real database](17-testing.md)

Chapter 15's handlers all had the same shape: borrow a connection, run a query, return a
response, give the connection back. This chapter is about the responses and the work that do not
fit that shape. A body that streams a whole catalog must keep its connection for as long as it
streams. An upload must flow into `COPY` as it arrives. A live feed of order events must stay
open for minutes, fed by the single `LISTEN` connection of Chapter 11. A slow report that the
client abandoned must be cancelled on the server. And some work belongs to nobody's request at
all: the outbox worker that delivers jobs in the background.

Each section is one of stockroom's endpoints or tasks, reduced to a runnable example. The last
sections cover what ties them together at the end: shutting down without cutting anything off,
and telling an orchestrator whether the service is alive and ready.

---

## 16.1 NDJSON from a `RowStream`: who owns the connection?

Chapter 9 streamed rows with `query_raw`, and found that a `RowStream` does not borrow the
client. In a handler that is both what makes streaming possible and a trap. The handler can
return a body built from the stream; the body outlives the handler; and the pooled `Object` the
handler borrowed is dropped when the handler returns, going back to the pool *while the stream
still runs on it*. The next request to borrow that connection queues its query behind the rest of
the rows.

The fix is to move the `Object` into the body's stream, so that the connection stays checked out
until the last row has been sent (or the client goes away and the body is dropped). Here are
both versions, side by side in one program, with a pool of two connections; while the first
response's body is still unread, a second request needs a connection:

```rust
use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use deadpool_postgres::Pool;
use futures_util::TryStreamExt;
use http_body_util::BodyExt;
use std::pin::pin;
use std::time::Duration;
use tower::ServiceExt;

const MANY: &str = "SELECT i FROM generate_series(1, 200000) i";

fn unavailable<E>(_: E) -> StatusCode {
    StatusCode::SERVICE_UNAVAILABLE
}

/// Wrong: `db` goes back to the pool when this function returns, while `rows` still uses it.
async fn leaky(State(pool): State<Pool>) -> Result<Response, StatusCode> {
    let db = pool.get().await.map_err(unavailable)?;
    let rows = db
        .query_raw(MANY, playground::slice_iter(&[]))
        .await
        .map_err(unavailable)?;
    let lines = rows.map_ok(|row| Bytes::from(format!("{}\n", row.get::<_, i32>(0))));
    Ok(Body::from_stream(lines).into_response())
}

/// Right: the body's stream owns `db`, so the connection stays checked out until it ends.
async fn owned(State(pool): State<Pool>) -> Result<Response, StatusCode> {
    let db = pool.get().await.map_err(unavailable)?;
    let rows = db
        .query_raw(MANY, playground::slice_iter(&[]))
        .await
        .map_err(unavailable)?;
    let lines = async_stream::try_stream! {
        let _checked_out = db;
        let mut rows = pin!(rows);
        while let Some(row) = rows.try_next().await? {
            yield Bytes::from(format!("{}\n", row.get::<_, i32>(0)));
        }
    };
    Ok(Body::from_stream::<_>(lines.map_err(|e: tokio_postgres::Error| e)).into_response())
}

async fn one(State(pool): State<Pool>) -> Result<String, StatusCode> {
    let db = pool.get().await.map_err(unavailable)?;
    let row = db.query_one("SELECT 1", &[]).await.map_err(unavailable)?;
    Ok(row.get::<_, i32>(0).to_string())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let pool = playground::pool(2);
    for (name, stream_route) in [("leaky", get(leaky)), ("owned", get(owned))] {
        let app = Router::new()
            .route("/stream", stream_route)
            .route("/one", get(one))
            .with_state(pool.clone());

        // The streaming response arrives; its body is not read yet.
        let streaming = app
            .clone()
            .oneshot(Request::get("/stream").body(Body::empty())?)
            .await?;
        // Meanwhile, another request.
        let other = tokio::time::timeout(
            Duration::from_millis(500),
            playground::call(&app, Request::get("/one").body(Body::empty())?),
        )
        .await;
        println!(
            "{name}: the other request {}",
            if other.is_ok() {
                "was answered"
            } else {
                "timed out"
            }
        );
        assert_eq!(other.is_ok(), name == "owned");

        // Either way the stream itself is complete.
        let body = streaming.into_body().collect().await?.to_bytes();
        assert_eq!(body.iter().filter(|&&b| b == b'\n').count(), 200_000);
    }
    Ok(())
}
```

```text
leaky: the other request timed out
owned: the other request was answered
```

In the leaky version, the pool's only idle connection was the one that had just "returned", so
the second request got it, and its `SELECT 1` waited behind 200,000 unread rows. In the owned version, the stream kept its connection, so the second request got
the other one. `let _checked_out = db;` is the whole fix: the underscore-prefixed name keeps the
value alive to the end of the stream, where a bare `_` would drop it at once.

Backpressure works end to end through this chain. A client that reads slowly stops the body;
the body stops polling the stream; the stream's channel fills; the connection stops reading the
socket; and the server waits in `ClientWrite` (Chapter 9). No layer buffers more than a little,
so memory stays flat however large the result and however slow the client. The price is that a
slow client holds a pooled connection for as long as it takes: set the pool size, and consider
a cap on concurrent streams, with that in mind.

> **Pitfall:** once the first bytes of a streamed body are sent, the status (200) is final. An
> error after that can only cut the body short, which the client sees as a truncated response:
> an NDJSON client should treat a missing final newline, or a count that does not match, as a
> failure. Decide before streaming whatever can still fail early, and stream only after.

## 16.2 A CSV export with `copy_out`

When the response is CSV, PostgreSQL can produce it: `COPY … TO STDOUT (FORMAT csv, HEADER)`
formats every row, quoting included, and `copy_out` (Chapter 10) hands the chunks over as they
come. The handler only moves bytes, and the same ownership rule applies:

~~~rust
// stockroom/src/http/products.rs (excerpt)
pub async fn export_csv(Db(db): Db) -> Result<Response, AppError> {
    let copy = db.copy_out(products::EXPORT_CSV).await?;
    let body = Body::from_stream(async_stream::stream! {
        let _checked_out = db;
        let mut copy = pin!(copy);
        while let Some(chunk) = copy.next().await {
            yield chunk;
        }
    });
    Ok((
        [
            (header::CONTENT_TYPE, "text/csv"),
            (
                header::CONTENT_DISPOSITION,
                "attachment; filename=\"products.csv\"",
            ),
        ],
        body,
    )
        .into_response())
}
~~~

[stockroom/src/http/products.rs](../stockroom/src/http/products.rs)

The next section's example exports with exactly this shape and reads the result back in.

## 16.3 A CSV import: from the request body into `COPY`

The other direction is the same idea reversed. axum's `Body` is a stream of `Bytes`
(`body.into_data_stream()`), and a `CopyInSink` accepts `Bytes` (Chapter 10), so an upload can
go from the socket into `COPY` chunk by chunk, never held whole in memory. Chapter 10's staging
table makes it safe: the rows land in a temporary table, are checked and merged in one
transaction, and the upload goes in completely or not at all. A body limit keeps a client from
uploading a terabyte:

```rust
use axum::Json;
use axum::Router;
use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use deadpool_postgres::Pool;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::pin::pin;
use tower_http::limit::RequestBodyLimitLayer;

type ApiError = (StatusCode, Json<Value>);

fn error(status: StatusCode, message: impl ToString) -> ApiError {
    (status, Json(json!({ "error": message.to_string() })))
}

async fn export(State(pool): State<Pool>) -> Result<Response, ApiError> {
    let db = pool
        .get()
        .await
        .map_err(|e| error(StatusCode::SERVICE_UNAVAILABLE, e))?;
    let copy = db
        .copy_out("COPY (SELECT sku, price_cents FROM product ORDER BY sku) TO STDOUT (FORMAT csv, HEADER)")
        .await
        .map_err(|e| error(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let body = Body::from_stream(async_stream::stream! {
        let _checked_out = db;
        let mut copy = pin!(copy);
        while let Some(chunk) = copy.next().await {
            yield chunk;
        }
    });
    Ok(([(header::CONTENT_TYPE, "text/csv")], body).into_response())
}

async fn import(State(pool): State<Pool>, body: Body) -> Result<Json<Value>, ApiError> {
    let internal = |e: tokio_postgres::Error| error(StatusCode::INTERNAL_SERVER_ERROR, e);
    let mut db = pool
        .get()
        .await
        .map_err(|e| error(StatusCode::SERVICE_UNAVAILABLE, e))?;
    let tx = db.transaction().await.map_err(internal)?;
    tx.batch_execute(
        "CREATE TEMP TABLE price_import (sku text, price_cents bigint) ON COMMIT DROP",
    )
    .await
    .map_err(internal)?;
    let sink = tx
        .copy_in("COPY price_import FROM STDIN (FORMAT csv, HEADER MATCH)")
        .await
        .map_err(internal)?;
    let mut sink = pin!(sink);
    let mut chunks = body.into_data_stream();
    while let Some(chunk) = chunks.next().await {
        // A body error here is the client's: an upload cut short, or one over the limit.
        let chunk = chunk.map_err(|e| error(StatusCode::BAD_REQUEST, e))?;
        sink.send(chunk)
            .await
            .map_err(|e| error(StatusCode::UNPROCESSABLE_ENTITY, e))?;
    }
    let rows = sink.as_mut().finish().await.map_err(|e| {
        // A malformed line is reported with its line number (Chapter 10).
        let db = e.as_db_error();
        let at = db.and_then(|d| d.where_()).unwrap_or_default();
        let message = db
            .map(|d| d.message())
            .unwrap_or("could not read the upload");
        error(
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("{message} ({at})"),
        )
    })?;
    let changed = tx
        .execute(
            "MERGE INTO product AS p USING price_import AS i ON p.sku = i.sku
             WHEN MATCHED THEN UPDATE SET price_cents = i.price_cents
             WHEN NOT MATCHED THEN INSERT VALUES (i.sku, i.price_cents)",
            &[],
        )
        .await
        .map_err(internal)?;
    tx.commit().await.map_err(internal)?;
    Ok(Json(json!({ "rows": rows, "changed": changed })))
}

fn upload(csv: &str) -> Request<Body> {
    Request::post("/import")
        .header(header::CONTENT_TYPE, "text/csv")
        .header(header::CONTENT_LENGTH, csv.len())
        .body(Body::from(csv.to_owned()))
        .expect("a valid request")
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let pool = playground::pool(1);
    pool.get()
        .await?
        .batch_execute("CREATE TEMP TABLE product (sku text PRIMARY KEY, price_cents bigint)")
        .await?;
    let app = Router::new()
        .route("/export.csv", get(export))
        .route(
            "/import",
            post(import).layer(RequestBodyLimitLayer::new(1024)),
        )
        .with_state(pool);

    let (status, _, body) = playground::call(
        &app,
        upload("sku,price_cents\nA-1,4999\nA-2,12900\nA-3,2500\n"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["rows"], 3);

    let (_, _, csv) =
        playground::call(&app, Request::get("/export.csv").body(Body::empty())?).await;
    print!("{}", csv.as_str().unwrap_or_default());

    // A bad line: nothing is imported, and the client learns where.
    let (status, _, body) =
        playground::call(&app, upload("sku,price_cents\nA-1,1\nA-2,lots\n")).await;
    println!("{status} {}", body["error"]);
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    // Too big: refused before the handler runs.
    let big = format!("sku,price_cents\n{}", "A-1,1\n".repeat(200));
    let (status, _, _) = playground::call(&app, upload(&big)).await;
    println!("{status}");
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    Ok(())
}
```

```text
sku,price_cents
A-1,4999
A-2,12900
A-3,2500
422 Unprocessable Entity "invalid input syntax for type bigint: \"lots\" (COPY price_import, line 3, column price_cents: \"lots\")"
413 Payload Too Large
```

Three details carry over to stockroom's `POST /products/import`:

- **The limit is a layer on the route**, `RequestBodyLimitLayer` from tower-http. A request with
  a `Content-Length` over the limit gets 413 before the handler runs; one without (a chunked
  upload) has its body cut off at the limit, and the handler sees a body error, which aborts the
  `COPY` and rolls back the transaction.
- **Errors in the data are the client's**, class 22, reported with the line `DbError::where_()`
  names. They quote the client's own CSV back, which is the one case where a `DbError`'s text may
  reach a client (Chapter 6).
- **The handler holds a connection while the upload arrives**, since the `COPY` runs as the bytes
  come in. A slow uploader holds a pooled connection; the body limit and a request timeout bound
  how long.

## 16.4 Server-sent events from `LISTEN`

`GET /orders/events` keeps a response open and sends an event each time an order changes:
[server-sent events](https://html.spec.whatwg.org/multipage/server-sent-events.html), a
`text/event-stream` body of `event:` and `data:` lines that browsers read with `EventSource`.
The events come from Chapter 11's design: one dedicated `LISTEN` connection for the whole
process, feeding a `tokio::sync::broadcast` channel, from which each open SSE response
subscribes. So a thousand open event streams cost one database connection, not a thousand.

axum's `Sse` turns a `Stream` of `Event`s into such a response, and `KeepAlive` sends a comment
line every 15 seconds by default, so that proxies do not close an idle stream:

```rust
use axum::Router;
use axum::body::Body;
use axum::extract::State;
use axum::http::Request;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::get;
use futures_util::{Stream, StreamExt};
use http_body_util::BodyExt;
use std::convert::Infallible;
use std::time::Duration;
use tokio::sync::broadcast::{self, error::RecvError};
use tokio_postgres::AsyncMessage;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

#[derive(Clone)]
struct AppState {
    events: broadcast::Sender<String>,
    shutdown: CancellationToken,
}

async fn order_events(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let mut events = state.events.subscribe();
    let stream = async_stream::stream! {
        loop {
            match events.recv().await {
                Ok(payload) => yield Ok(Event::default().event("order").data(payload)),
                // Missed some: tell the client to reload, rather than pretend (Chapter 11).
                Err(RecvError::Lagged(_)) => yield Ok(Event::default().event("resync").data("reload")),
                Err(RecvError::Closed) => break,
            }
        }
    };
    // End with the service, or a graceful shutdown would wait for this stream forever.
    let stream = stream.take_until(state.shutdown.clone().cancelled_owned());
    Sse::new(stream).keep_alive(KeepAlive::default())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // The one listener connection, forwarding notifications into the broadcast channel.
    let (listener, mut messages) = playground::connect_with_messages().await?;
    listener.batch_execute("LISTEN order_events").await?;
    let (events, _) = broadcast::channel(64);
    let forward = events.clone();
    tokio::spawn(async move {
        while let Some(message) = messages.recv().await {
            if let AsyncMessage::Notification(n) = message {
                let _ = forward.send(n.payload().to_owned()); // Err: no subscriber right now
            }
        }
    });

    let shutdown = CancellationToken::new();
    let app = Router::new()
        .route("/orders/events", get(order_events))
        .with_state(AppState {
            events,
            shutdown: shutdown.clone(),
        });

    // A client connects...
    let response = app
        .oneshot(Request::get("/orders/events").body(Body::empty())?)
        .await?;
    println!("{:?}", response.headers()["content-type"]);
    let mut body = response.into_body();

    // ...an order changes, in another session...
    let writer = playground::connect().await?;
    writer
        .execute(
            "SELECT pg_notify('order_events', $1)",
            &[&r#"{"orderId":42,"status":"paid"}"#],
        )
        .await?;

    // ...and the event arrives on the open response.
    let frame = body.frame().await.expect("an event")?;
    print!(
        "{}",
        String::from_utf8_lossy(frame.data_ref().expect("data"))
    );

    // At shutdown the stream ends, and with it the response.
    shutdown.cancel();
    let end = tokio::time::timeout(Duration::from_secs(1), body.frame()).await?;
    assert!(end.is_none(), "the body has ended");
    Ok(())
}
```

```text
"text/event-stream"
event: order
data: {"orderId":42,"status":"paid"}

```

Notice what the SSE handler does *not* hold: a database connection. It holds a broadcast
receiver, so an idle event stream costs a few hundred bytes. If the handler needs the database
first (stockroom looks up the store a client filters on), it borrows a connection for that
lookup and lets it go before returning the stream.

The `take_until` line matters more than it looks. axum's graceful shutdown (§16.7) waits for
open responses to finish, and an event stream never finishes by itself: without it, a SIGTERM
would leave the process waiting for every connected browser to leave. stockroom's test for this
measured a shutdown in under 50 ms with an SSE client attached.

## 16.5 Timeouts and client disconnects

A request can be abandoned two ways: the router's timeout fires, or the client disconnects. In
both cases axum *drops the handler's future*, and Chapter 12 showed what that leaves behind: a
query still running on the server, and a pooled connection that goes back to the pool busy.

`tower_http::timeout::TimeoutLayer` gives a slow handler a deadline and answers 503 when it
passes. A guard that owns the pooled connection, and cancels its query when dropped before the
handler finishes, makes the abandonment complete:

```rust
use axum::Router;
use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, StatusCode};
use axum::routing::get;
use deadpool_postgres::{Object, Pool};
use std::ops::Deref;
use std::time::Duration;
use tokio_postgres::NoTls;
use tower_http::timeout::TimeoutLayer;

/// Owns a pooled connection while a slow query runs. Dropped before `finish` (the request was
/// abandoned), it cancels the query and discards the connection.
struct CancelOnDrop(Option<Object>);

impl CancelOnDrop {
    fn finish(mut self) -> Object {
        self.0.take().expect("present until finished or dropped")
    }
}

impl Deref for CancelOnDrop {
    type Target = Object;
    fn deref(&self) -> &Object {
        self.0.as_ref().expect("present until finished or dropped")
    }
}

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        let Some(conn) = self.0.take() else { return };
        let token = conn.cancel_token();
        // `drop` cannot await: send the cancel request from a task.
        tokio::spawn(async move {
            let _ = token.cancel_query(NoTls).await;
            // Never back to the pool: a late cancel request could hit its next user.
            drop(Object::take(conn));
        });
    }
}

async fn unguarded(State(pool): State<Pool>) -> Result<String, StatusCode> {
    let db = pool
        .get()
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let row = db
        .query_one("SELECT 'done' FROM pg_sleep(3) /* unguarded report */", &[])
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(row.get(0))
}

async fn guarded(State(pool): State<Pool>) -> Result<String, StatusCode> {
    let db = CancelOnDrop(Some(
        pool.get()
            .await
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?,
    ));
    let row = db
        .query_one("SELECT 'done' FROM pg_sleep(3) /* guarded report */", &[])
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    drop(db.finish()); // finished: disarm the guard, and the connection goes back to the pool
    Ok(row.get(0))
}

/// How many sessions are running the query marked `marker`, after `wait`.
async fn running(observer: &tokio_postgres::Client, marker: &str, wait: Duration) -> i64 {
    tokio::time::sleep(wait).await;
    observer
        .query_one(
            "SELECT count(*) FROM pg_stat_activity
             WHERE state = 'active' AND query LIKE '%' || $1 || '%' AND pid <> pg_backend_pid()",
            &[&marker],
        )
        .await
        .expect("a query on pg_stat_activity")
        .get(0)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let observer = playground::connect().await?;
    let app = Router::new()
        .route("/unguarded", get(unguarded))
        .route("/guarded", get(guarded))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::SERVICE_UNAVAILABLE,
            Duration::from_millis(200),
        ))
        .with_state(playground::pool(4));

    for (uri, marker) in [
        ("/unguarded", "unguarded report"),
        ("/guarded", "guarded report"),
    ] {
        let (status, _, _) = playground::call(&app, Request::get(uri).body(Body::empty())?).await;
        let left = running(&observer, marker, Duration::from_millis(300)).await;
        println!("{uri}: {status}, queries still running on the server: {left}");
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(left, if uri == "/guarded" { 0 } else { 1 });
    }
    Ok(())
}
```

```text
/unguarded: 503 Service Unavailable, queries still running on the server: 1
/guarded: 503 Service Unavailable, queries still running on the server: 0
```

Both clients got their 503 after 200 ms. Only one of the servers stopped working. stockroom's
`CancelOnDrop` (Chapter 12) is this guard with tracing and a configurable TLS connector, and
`GET /reports/sales?delayMs=5000` uses it; a test in `tests/reports.rs`
(`an_abandoned_report_is_cancelled_on_the_server`) checks that no `pg_sleep` is left running after
a 300 ms router timeout.

A server-side `statement_timeout` (Chapter 12) is the other half: the guard handles requests
that were abandoned, the timeout handles queries that run too long whether or not anyone is
waiting. stockroom sets a 5-second statement timeout on every session, and its test
`statement_timeout_is_a_503` checks that a query ended by it becomes a 503 with the error code
`timeout`.

## 16.6 The outbox worker

Some work must happen *because* a transaction committed, but not *inside* it: sending the "your
order is ready" email, calling a partner's webhook. Doing it in the request is wrong both ways.
Sent before the commit, the email goes out for an order that may still roll back; sent after, it
is lost if the process dies between the commit and the send.

The *transactional outbox* splits the work in two. The transaction that changes the order also
inserts a job row into an `outbox` table (stockroom's trigger does it, Chapter 11), so the job
exists if and only if the change committed. A background worker then claims due jobs, delivers
them, and marks them done. Several workers can share the table safely because of `FOR UPDATE
SKIP LOCKED`: each worker locks the rows it claims, and the others *skip* locked rows instead
of waiting for them (Chapter 8). A worker that crashes mid-batch releases its locks with its
connection, and its jobs become claimable again:

```rust
type Error = Box<dyn std::error::Error>;

const CLAIM: &str = "SELECT id FROM outbox
     WHERE done_at IS NULL ORDER BY id LIMIT $1
     FOR UPDATE SKIP LOCKED";

async fn claim(tx: &tokio_postgres::Transaction<'_>, limit: i64) -> Result<Vec<i64>, Error> {
    Ok(tx
        .query(CLAIM, &[&limit])
        .await?
        .iter()
        .map(|row| row.get(0))
        .collect())
}

async fn mark_done(tx: &tokio_postgres::Transaction<'_>, ids: &[i64]) -> Result<(), Error> {
    tx.execute(
        "UPDATE outbox SET done_at = now() WHERE id = ANY($1)",
        &[&ids],
    )
    .await?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    // Workers are separate sessions, so the table must be a real one: in a schema of our own.
    let schema = format!("ch16_{}", std::process::id());
    let mut a = playground::connect().await?;
    let mut b = playground::connect().await?;
    let mut c = playground::connect().await?;
    a.batch_execute(&format!(
        "CREATE SCHEMA {schema};
         CREATE TABLE {schema}.outbox (id bigint PRIMARY KEY, done_at timestamptz);
         INSERT INTO {schema}.outbox SELECT generate_series(1, 5);"
    ))
    .await?;
    for worker in [&a, &b, &c] {
        worker
            .batch_execute(&format!("SET search_path = {schema}"))
            .await?;
    }
    let mut delivered: Vec<i64> = Vec::new();

    // Worker A claims three jobs and is still delivering them...
    let tx_a = a.transaction().await?;
    let batch_a = claim(&tx_a, 3).await?;
    // ...while worker B claims too: it skips A's rows instead of waiting for them.
    let tx_b = b.transaction().await?;
    let batch_b = claim(&tx_b, 3).await?;
    assert_eq!(
        (batch_a.as_slice(), batch_b.as_slice()),
        (&[1, 2, 3][..], &[4, 5][..])
    );

    delivered.extend(&batch_a);
    mark_done(&tx_a, &batch_a).await?;
    tx_a.commit().await?;

    // B delivers its jobs, then dies before committing: its locks go, the jobs stay due.
    delivered.extend(&batch_b);
    drop(tx_b);
    drop(b);

    // Worker C finds them, and delivers them again.
    let tx_c = c.transaction().await?;
    let batch_c = claim(&tx_c, 3).await?;
    delivered.extend(&batch_c);
    mark_done(&tx_c, &batch_c).await?;
    tx_c.commit().await?;

    println!("deliveries: {delivered:?}");
    assert_eq!(delivered, [1, 2, 3, 4, 5, 4, 5]); // at least once, not exactly once
    let pending: i64 = c
        .query_one("SELECT count(*) FROM outbox WHERE done_at IS NULL", &[])
        .await?
        .get(0);
    assert_eq!(pending, 0);

    a.batch_execute(&format!("DROP SCHEMA {schema} CASCADE"))
        .await?;
    Ok(())
}
```

```text
deliveries: [1, 2, 3, 4, 5, 4, 5]
```

That last line is the outbox's guarantee and its cost. Every job is delivered *at least once*:
none is lost, whatever crashes when. Some may be delivered twice, so the receiving side must be
idempotent, for instance by deduplicating on the job id that travels with each delivery.

stockroom's worker adds two things. It does not poll in a tight loop: it sleeps until the
listener's `outbox` notification wakes it (Chapter 11), with a 30-second poll as a safety net for
a missed wake-up, and it drains full batches back to back. And a failed delivery is not retried
at once: the job's `available_at` moves forward with exponential backoff (1 s, 2 s, 4 s, up to
an hour), and `last_error` records why.

~~~rust
// stockroom/src/worker/outbox.rs (excerpt)
pub async fn run(pool: Pool, hub: Hub, deliver: impl Deliver, shutdown: CancellationToken) {
    loop {
        match drain(&pool, &deliver).await {
            Ok(n) if n as i64 == BATCH => continue, // a full batch: there may be more
            Ok(_) => {}
            Err(error) => tracing::warn!(%error, "outbox drain failed"),
        }
        tokio::select! {
            () = hub.outbox_woken() => {}
            () = tokio::time::sleep(POLL_INTERVAL) => {}
            () = shutdown.cancelled() => return,
        }
    }
}
~~~

[stockroom/src/worker/outbox.rs](../stockroom/src/worker/outbox.rs) ·
[stockroom/src/repo/outbox.rs](../stockroom/src/repo/outbox.rs)

One trade-off is deliberate: the worker holds its claimed rows locked, and a pooled connection,
while it delivers. That is what makes a job belong to one worker at a time, and it means
deliveries need short timeouts of their own. A design that must make slow calls instead marks
jobs as "in progress until" a lease time, commits, delivers, and comes back to mark them done.

## 16.7 Graceful shutdown

When an orchestrator stops the service (SIGTERM, Ctrl-C), in-flight requests should finish,
background tasks should stop between units of work, and the pool should close its connections
cleanly. One `tokio_util::sync::CancellationToken`, cloned into everything long-lived, carries
the signal:

~~~rust
// stockroom/src/app.rs (excerpt)
let tcp = TcpListener::bind(args.bind).await?;
tracing::info!("listening on http://{}", tcp.local_addr()?);
tokio::spawn(cancel_on_signal(shutdown.clone()));

// Stop accepting connections when `shutdown` fires, then wait for in-flight requests.
axum::serve(tcp, app)
    .with_graceful_shutdown(shutdown.clone().cancelled_owned())
    .await?;

shutdown.cancel();
let _ = tokio::join!(listener, worker);
pool.close();
tracing::info!("shut down");
~~~

[stockroom/src/app.rs](../stockroom/src/app.rs)

The order is the point. `with_graceful_shutdown` stops accepting connections when the token
fires and returns once in-flight responses are done; event streams end with the token
(`take_until`, §16.4), so "done" comes promptly. The listener and the worker see the same
token in their `select!` loops and return; the worker finishes the batch it is delivering first,
because the token is only checked between batches. Then `pool.close()` closes the idle
connections. The process exits with nothing half-done that matters: an outbox batch interrupted
by a hard kill is simply delivered again (§16.6).

## 16.8 Health and readiness

Orchestrators ask two different questions, and conflating them causes outages:

- **Liveness**, `GET /healthz`: is the process working at all? If not, restart it. stockroom's
  answer is a constant `"ok"`: it deliberately does not touch the database, because a database
  outage is not fixed by restarting every instance of every service that uses it, and a
  liveness check that fails during one makes things worse.
- **Readiness**, `GET /readyz`: should this instance receive traffic right now? Here the database
  matters: stockroom borrows a connection and calls `check_connection()` (one round trip, no
  statement, Chapter 12), and reports the pool's state, so a load balancer stops sending
  requests to an instance that cannot serve them.

~~~rust
// stockroom/src/http/health.rs (excerpt)
pub async fn ready(State(state): State<AppState>) -> Result<Json<Value>, AppError> {
    let db = state.pool.get().await?;
    db.check_connection().await?; // one round trip, no statement
    let pool = state.pool.status();
    Ok(Json(json!({
        "status": "ready",
        "pool": { "maxSize": pool.max_size, "size": pool.size, "available": pool.available, "waiting": pool.waiting },
        "listening": state.hub.server_info().is_some(),
    })))
}
~~~

[stockroom/src/http/health.rs](../stockroom/src/http/health.rs)

When the pool cannot lend a connection within its wait timeout, `AppError` turns that into 503,
which is exactly what a readiness probe should see. The `waiting` count is worth graphing: a
pool that often has waiters is too small, or its connections are held too long (Chapter 13).

---

## In stockroom

| Endpoint or task | Section | File |
|---|---|---|
| `GET /products/stream` (NDJSON) | 16.1 | `http/products.rs` |
| `GET /products/export.csv` | 16.2 | `http/products.rs` |
| `POST /products/import` (64 MiB limit) | 16.3 | `http/products.rs`, `repo/products.rs` |
| `GET /orders/events` (SSE, `?store=` filter) | 16.4 | `http/events.rs`, `notify/` |
| `GET /reports/sales?delayMs=` with `CancelOnDrop` | 16.5 | `http/reports.rs`, `db/cancel.rs` |
| the outbox worker | 16.6 | `worker/outbox.rs`, `repo/outbox.rs` |
| graceful shutdown | 16.7 | `app.rs` |
| `GET /healthz`, `GET /readyz`, `GET /admin/sessions` | 16.8 | `http/health.rs` |

`stockroom/stockroom.http` has a request for each; for the event stream, run
`curl -N localhost:8080/orders/events` in one terminal and place an order from another.

## Summary

- A streamed body outlives its handler, and a `RowStream` or `CopyOutStream` does not borrow
  the connection: move the pooled `Object` into the body's stream (`let _checked_out = db;`), or
  the next borrower queues behind the unread rows. Backpressure then reaches the server.
- Once a streamed body starts, its status is final; errors can only truncate it.
- CSV out: `copy_out` straight into the body. CSV in: `body.into_data_stream()` into `copy_in`,
  through a staging table and a merge, in one transaction, under `RequestBodyLimitLayer`.
- SSE: one `LISTEN` connection feeds a `broadcast`; each response subscribes, turns `Lagged`
  into a resync event, keeps alive, and ends with the shutdown token.
- An abandoned request leaves its query running unless a guard cancels it on drop and discards
  the connection; pair it with a server-side `statement_timeout`.
- The outbox: jobs written in the changing transaction, claimed with `FOR UPDATE SKIP LOCKED`,
  delivered at least once; woken by `NOTIFY`, with polling as a fallback.
- Shut down in order: stop accepting, let responses finish (streams end on the token), stop
  background tasks between units of work, close the pool. Liveness ignores the database;
  readiness checks it.

## Exercises

1. **Count the rows.** Make §16.1's NDJSON stream end with a trailer line,
   `{"count": 200000}`, so that a client can tell a complete response from a truncated one.
   *Hint:* `async_stream::try_stream!` can keep a counter and `yield` once more after the loop.
2. **Filtered events.** Add a `?store=` query parameter to §16.4's handler that only forwards
   events whose JSON has that `storeId`. Where should the filtering happen: in SQL, in the
   listener task, or in each subscriber?
   *Hint:* there is one `LISTEN` for all subscribers.
3. **A lease instead of a lock.** Sketch the SQL for an outbox where a worker claims jobs by
   setting `locked_until = now() + interval '1 minute'`, commits, delivers, and then marks them
   done. What happens to a job whose worker died, and what new failure mode appears?
   *Hint:* `WHERE done_at IS NULL AND (locked_until IS NULL OR locked_until < now())`.
4. **Shutdown with a slow request.** In stockroom, a request is running a 4-second report when
   SIGTERM arrives. What does the client get, and how long does the process take to exit?
   *Hint:* graceful shutdown waits for in-flight requests; the request timeout still applies.

### Solutions

**1.**

```rust
use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use deadpool_postgres::Pool;
use futures_util::TryStreamExt;
use std::pin::pin;

async fn stream(State(pool): State<Pool>) -> Result<Response, StatusCode> {
    let db = pool
        .get()
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let rows = db
        .query_raw(
            "SELECT i FROM generate_series(1, 1000) i",
            playground::slice_iter(&[]),
        )
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let lines = async_stream::try_stream! {
        let _checked_out = db;
        let mut rows = pin!(rows);
        let mut count = 0u64;
        while let Some(row) = rows.try_next().await? {
            count += 1;
            yield Bytes::from(format!("{{\"i\": {}}}\n", row.get::<_, i32>(0)));
        }
        yield Bytes::from(format!("{{\"count\": {count}}}\n"));
    };
    Ok(Body::from_stream::<_>(lines.map_err(|e: tokio_postgres::Error| e)).into_response())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let app = Router::new()
        .route("/stream", get(stream))
        .with_state(playground::pool(1));
    let (_, _, body) = playground::call(&app, Request::get("/stream").body(Body::empty())?).await;
    let text = body.as_str().unwrap_or_default().to_owned();
    let last = text.lines().last().unwrap_or_default();
    assert_eq!(last, r#"{"count": 1000}"#);
    assert_eq!(text.lines().count(), 1001);
    Ok(())
}
```

A client reads lines until the trailer and compares the count with the lines it saw; a response
without a trailer was cut short.

**2.** In each subscriber, as stockroom does. The single listener receives every event for every
store, and the broadcast hands each one to every subscriber, which keeps the one it cares about.
Filtering in SQL would need one channel per store, and so one `LISTEN` per store on the shared
connection, managed as subscribers come and go; it pays off only when stores are many and events
frequent.

**3.** Claim with `UPDATE outbox SET locked_until = now() + interval '1 minute' WHERE id IN
(SELECT id FROM outbox WHERE done_at IS NULL AND (locked_until IS NULL OR locked_until < now())
ORDER BY id LIMIT $1 FOR UPDATE SKIP LOCKED) RETURNING id, topic, payload`, commit, deliver, then
`UPDATE outbox SET done_at = now() WHERE id = ANY($1)`. A dead worker's jobs become claimable
again when their lease expires, without holding a connection meanwhile. The new failure mode is a
*slow* worker: one whose delivery outlasts the lease, so a second worker claims and delivers the
same job while the first is still at it. Deliveries must still be idempotent, and the lease
longer than any delivery's timeout.

**4.** The client still gets its answer: graceful shutdown stops accepting new connections but
lets the in-flight request finish. If the report finishes within the request timeout (10 s by
default in stockroom), the client gets 200 and the process exits right after, about 4 seconds
after the signal. If the report would run longer, the router's timeout answers 503 first, and
`CancelOnDrop` cancels the query, so the process still exits within the request timeout.

---

Previous: [Chapter 15 — axum I: a database-backed JSON API](15-axum-api.md) ·
Next: [Chapter 17 — Testing against a real database](17-testing.md)
