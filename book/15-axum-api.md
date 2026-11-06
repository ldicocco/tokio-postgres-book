# Chapter 15 — axum I: a database-backed JSON API

> **Async PostgreSQL in Rust** · Previous: [Chapter 14 — Schema, migrations and the data layer](14-data-layer.md) ·
> Next: [Chapter 16 — axum II: streaming, real-time and background work](16-axum-streaming.md)

Everything so far comes together in an HTTP service. A request arrives, borrows a connection
from the pool, runs a repository function or a transaction, and turns the result, or the
error, into a JSON response with the right status code. Most of the work is in the seams:
how a handler gets its connection, how a unique violation becomes a 409 and a check
violation a 422, how a transaction that hits a deadlock is retried without the client
noticing, how a list is paged, and how two clients editing the same product are kept from
overwriting each other.

This chapter builds those seams with [axum](https://docs.rs/axum/0.8.9/axum/) 0.8, one at a
time, each as a small runnable service. None of the examples binds a port: they send requests
to the router in memory, which is also how Chapter 17 tests stockroom. Chapter 16 adds the
long-lived parts: streamed responses, server-sent events and background work.

---

## 15.1 The shape of the app

An axum service is a `Router`: routes, each with a handler, plus *state* that every handler can
reach, plus layers (middleware) around them all. For a database service the state is, at
least, the pool:

```rust
use axum::extract::State;
use axum::http::{Request, StatusCode};
use axum::routing::get;
use axum::{Json, Router};
use deadpool_postgres::Pool;
use serde_json::{Value, json};

/// What every handler can reach. It is cloned for each request, so every field is a cheap
/// handle: a `Pool` is an `Arc` inside.
#[derive(Clone)]
struct AppState {
    pool: Pool,
}

async fn ready(State(state): State<AppState>) -> Result<Json<Value>, StatusCode> {
    let db = state
        .pool
        .get()
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    db.check_connection()
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let pool = state.pool.status();
    Ok(Json(
        json!({ "status": "ready", "size": pool.size, "available": pool.available }),
    ))
}

fn router(state: AppState) -> Router {
    Router::new().route("/readyz", get(ready)).with_state(state)
}

#[tokio::main]
async fn main() {
    let app = router(AppState {
        pool: playground::pool(4),
    });
    let request = Request::get("/readyz")
        .body(axum::body::Body::empty())
        .expect("a valid request");
    let (status, _headers, body) = playground::call(&app, request).await;
    println!("{status} {body}");
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ready");
}
```

```text
200 OK {"available":0,"size":1,"status":"ready"}
```

(`available` is 0 because the handler itself holds the pool's only open connection while it
reports.) Two helpers from the playground make these examples short. `pool(n)` builds a
deadpool pool as Chapter 13 did:

<!-- mirror: playground/src/lib.rs#pool -->
~~~rust
use deadpool_postgres::{Manager, ManagerConfig, Pool, RecyclingMethod, Runtime};

/// A deadpool pool of at most `max_size` connections to `DATABASE_URL` (Chapter 13). Part
/// III's examples mostly use a pool of one, so that every request sees the same session, and
/// with it the example's temporary tables.
pub fn pool(max_size: usize) -> Pool {
    let config: tokio_postgres::Config = std::env::var("DATABASE_URL")
        .expect("set DATABASE_URL, e.g. postgres://postgres:secret@localhost:5432/tpbook")
        .parse()
        .expect("DATABASE_URL is a valid connection string");
    let manager = Manager::from_config(
        config,
        NoTls,
        ManagerConfig {
            recycling_method: RecyclingMethod::Fast,
        },
    );
    Pool::builder(manager)
        .max_size(max_size)
        .runtime(Runtime::Tokio1)
        .wait_timeout(Some(std::time::Duration::from_secs(5)))
        .build()
        .expect("a valid pool configuration")
}
~~~

And `call` sends one request through a router with `tower::ServiceExt::oneshot`, which is all a
`Router` needs to answer: no socket, no port, no server.

<!-- mirror: playground/src/lib.rs#call -->
~~~rust
use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

/// Sends one request through `app`, in memory, without binding a port, and returns the
/// response's status, headers and body: parsed as JSON, as a JSON string if it is plain text,
/// or `Null` when it is empty.
pub async fn call(
    app: &axum::Router,
    request: Request<Body>,
) -> (StatusCode, HeaderMap, serde_json::Value) {
    let response = app
        .clone()
        .oneshot(request)
        .await
        .expect("a Router's error type is Infallible");
    let (parts, body) = response.into_parts();
    let bytes = body
        .collect()
        .await
        .expect("an in-memory body cannot fail")
        .to_bytes();
    let json = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|_| {
            serde_json::Value::String(String::from_utf8_lossy(&bytes).into_owned())
        })
    };
    (parts.status, parts.headers, json)
}
~~~

In a real service the router is served on a socket with `axum::serve(listener, app)`, and the
request goes through layers first: tracing, a timeout, a body limit. stockroom's
`http::router` (§ In stockroom) shows the full set, and its `main` sets up `tracing` so that
every request, and every repository call, logs its duration.

## 15.2 A `Db` extractor

Every handler that touches the database starts the same way: get a connection from the pool,
and answer 503 if none is free. An *extractor* says it once. Implement `FromRequestParts`, and
any handler can take the connection as an argument:

```rust
use axum::body::Body;
use axum::extract::{FromRequestParts, Path};
use axum::http::request::Parts;
use axum::http::{Request, StatusCode};
use axum::routing::get;
use axum::{Json, Router};
use deadpool_postgres::{Object, Pool};
use serde_json::{Value, json};

#[derive(Clone)]
struct AppState {
    pool: Pool,
}

/// A pooled connection, checked out for as long as the handler runs.
struct Db(Object);

impl FromRequestParts<AppState> for Db {
    type Rejection = (StatusCode, String);

    async fn from_request_parts(
        _parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let conn = state
            .pool
            .get()
            .await
            .map_err(|e| (StatusCode::SERVICE_UNAVAILABLE, e.to_string()))?;
        Ok(Db(conn))
    }
}

async fn product(Db(db): Db, Path(sku): Path<String>) -> Result<Json<Value>, (StatusCode, String)> {
    let row = db
        .query_opt("SELECT sku, name FROM product WHERE sku = $1", &[&sku])
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, format!("no product {sku}")))?;
    let (sku, name): (String, String) = (row.get(0), row.get(1));
    Ok(Json(json!({ "sku": sku, "name": name })))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let pool = playground::pool(1);
    pool.get()
        .await?
        .batch_execute(
            "CREATE TEMP TABLE product (sku text PRIMARY KEY, name text NOT NULL);
             INSERT INTO product VALUES ('CHAIR-01', 'Oak chair');",
        )
        .await?;
    let app = Router::new()
        .route("/products/{sku}", get(product))
        .with_state(AppState { pool });

    let (status, _, body) = playground::call(
        &app,
        Request::get("/products/CHAIR-01").body(Body::empty())?,
    )
    .await;
    assert_eq!(
        (status, body["name"].as_str()),
        (StatusCode::OK, Some("Oak chair"))
    );

    let (status, _, body) =
        playground::call(&app, Request::get("/products/NOPE-01").body(Body::empty())?).await;
    println!("{status} {body}");
    assert_eq!(status, StatusCode::NOT_FOUND);
    Ok(())
}
```

```text
404 Not Found "no product NOPE-01"
```

The connection goes back to the pool when the handler's future ends, whatever the outcome.
Two details deserve a sentence each:

- **Extractor order.** axum runs extractors in argument order, and only the last one may read
  the body (`Json`, `Body`). A handler that takes `Db` first holds a connection while its body
  arrives, which is fine for small JSON bodies and wasteful for uploads, as Chapter 16 notes.
- **One connection per request.** A handler that needs two connections at once, to run two
  transactions say, is almost always a design mistake, and with a pool of one it would
  deadlock against itself.

## 15.3 `AppError`: from SQLSTATE to HTTP status

Chapter 6 classified database errors by SQLSTATE. A service turns that classification into HTTP
statuses, once, in one `From<tokio_postgres::Error>` implementation, so that handlers can use
`?` everywhere and still answer correctly:

| SQLSTATE | Meaning | Status |
|---|---|---|
| 23505 `unique_violation` | the thing already exists | 409 Conflict |
| 23514 `check_violation`, 23502 `not_null_violation`, 23503 `foreign_key_violation` | the request breaks a rule of the data | 422 Unprocessable Entity |
| 40001 `serialization_failure`, 40P01 `deadlock_detected`, 55P03 `lock_not_available` | concurrency, not the request; retry | 503, with `Retry-After` |
| 57014 `query_canceled` | the query hit a timeout | 503 |
| 57P01 `admin_shutdown`, a closed connection, no connection from the pool | the database is unavailable | 503 |
| anything else | a bug, or something for an operator | 500, details only in the log |

Two more rules make the mapping useful to a client. The *constraint name* (`DbError::constraint`,
Chapter 6) says which rule was broken, and so which request field to blame. And the server's
own message never reaches the client: it can contain table names and the values of other rows
(Chapter 6), so it goes to the log, and the client gets a stable code and a message
written for it. The example maps the most common codes; stockroom's `AppError` (§ In stockroom)
maps every row of the table.

```rust
use axum::body::Body;
use axum::extract::{FromRequestParts, Path};
use axum::http::request::Parts;
use axum::http::{HeaderValue, Request, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use deadpool_postgres::{Object, Pool, PoolError};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio_postgres::error::SqlState;

#[derive(Debug)]
enum AppError {
    NotFound(&'static str),
    Invalid {
        field: Option<&'static str>,
        message: &'static str,
    },
    Conflict {
        field: Option<&'static str>,
        message: &'static str,
    },
    Busy,
    Unavailable,
    Internal,
}

impl From<tokio_postgres::Error> for AppError {
    fn from(error: tokio_postgres::Error) -> Self {
        let Some(db) = error.as_db_error() else {
            return if error.is_closed() {
                AppError::Unavailable
            } else {
                AppError::Internal
            };
        };
        // The constraint's name says which rule, and so which field, the request broke.
        let field = match db.constraint() {
            Some("product_pkey") => Some("sku"),
            Some("product_price_positive") => Some("priceCents"),
            _ => None,
        };
        match *db.code() {
            SqlState::UNIQUE_VIOLATION => AppError::Conflict {
                field,
                message: "this already exists",
            },
            SqlState::CHECK_VIOLATION
            | SqlState::NOT_NULL_VIOLATION
            | SqlState::FOREIGN_KEY_VIOLATION => AppError::Invalid {
                field,
                message: "the request breaks a rule of the data",
            },
            SqlState::T_R_SERIALIZATION_FAILURE | SqlState::T_R_DEADLOCK_DETECTED => AppError::Busy,
            _ => {
                eprintln!("database error: {}", db.message()); // the log, not the client
                AppError::Internal
            }
        }
    }
}

impl From<PoolError> for AppError {
    fn from(_: PoolError) -> Self {
        AppError::Unavailable
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, body) = match self {
            AppError::NotFound(what) => (
                StatusCode::NOT_FOUND,
                json!({ "error": "not_found", "message": format!("{what} not found") }),
            ),
            AppError::Invalid { field, message } => (
                StatusCode::UNPROCESSABLE_ENTITY,
                json!({ "error": "invalid", "field": field, "message": message }),
            ),
            AppError::Conflict { field, message } => (
                StatusCode::CONFLICT,
                json!({ "error": "conflict", "field": field, "message": message }),
            ),
            AppError::Busy | AppError::Unavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                json!({ "error": "unavailable", "message": "try again" }),
            ),
            AppError::Internal => (
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "internal", "message": "something went wrong on our side" }),
            ),
        };
        let mut response = (status, Json(body)).into_response();
        if status == StatusCode::SERVICE_UNAVAILABLE {
            response
                .headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
        }
        response
    }
}

#[derive(Clone)]
struct AppState {
    pool: Pool,
}

struct Db(Object);

impl FromRequestParts<AppState> for Db {
    type Rejection = AppError;

    async fn from_request_parts(_: &mut Parts, state: &AppState) -> Result<Self, AppError> {
        Ok(Db(state.pool.get().await?))
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NewProduct {
    sku: String,
    name: String,
    price_cents: i64,
}

async fn create(Db(db): Db, Json(new): Json<NewProduct>) -> Result<StatusCode, AppError> {
    let stmt = db
        .prepare_cached("INSERT INTO product (sku, name, price_cents) VALUES ($1, $2, $3)")
        .await?;
    db.execute(&stmt, &[&new.sku, &new.name, &new.price_cents])
        .await?;
    Ok(StatusCode::CREATED)
}

async fn get_one(Db(db): Db, Path(sku): Path<String>) -> Result<Json<Value>, AppError> {
    let row = db
        .query_opt(
            "SELECT name, price_cents FROM product WHERE sku = $1",
            &[&sku],
        )
        .await?
        .ok_or(AppError::NotFound("product"))?;
    let (name, price): (String, i64) = (row.get(0), row.get(1));
    Ok(Json(
        json!({ "sku": sku, "name": name, "priceCents": price }),
    ))
}

fn post_json(uri: &str, body: Value) -> Request<Body> {
    Request::post(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .expect("a valid request")
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let pool = playground::pool(1);
    pool.get()
        .await?
        .batch_execute(
            "CREATE TEMP TABLE product (
                 sku         text   PRIMARY KEY,
                 name        text   NOT NULL,
                 price_cents bigint NOT NULL CONSTRAINT product_price_positive CHECK (price_cents > 0)
             )",
        )
        .await?;
    let app = Router::new()
        .route("/products", post(create))
        .route("/products/{sku}", get(get_one))
        .with_state(AppState { pool });

    let chair = json!({ "sku": "CHAIR-01", "name": "Oak chair", "priceCents": 4999 });
    let (status, _, _) = playground::call(&app, post_json("/products", chair.clone())).await;
    assert_eq!(status, StatusCode::CREATED);

    let (status, _, body) = playground::call(&app, post_json("/products", chair)).await;
    println!("{status} {body}");
    assert_eq!(
        (status, body["field"].as_str()),
        (StatusCode::CONFLICT, Some("sku"))
    );

    let free = json!({ "sku": "LAMP-01", "name": "Lamp", "priceCents": -1 });
    let (status, _, body) = playground::call(&app, post_json("/products", free)).await;
    println!("{status} {body}");
    assert_eq!(
        (status, body["field"].as_str()),
        (StatusCode::UNPROCESSABLE_ENTITY, Some("priceCents"))
    );

    let missing = Request::get("/products/NOPE-01").body(Body::empty())?;
    let (status, _, body) = playground::call(&app, missing).await;
    println!("{status} {body}");
    assert_eq!(status, StatusCode::NOT_FOUND);
    Ok(())
}
```

```text
409 Conflict {"error":"conflict","field":"sku","message":"this already exists"}
422 Unprocessable Entity {"error":"invalid","field":"priceCents","message":"the request breaks a rule of the data"}
404 Not Found {"error":"not_found","message":"product not found"}
```

Note what the handlers do *not* do. `create` does not check whether the SKU exists before
inserting it: that check would race with a concurrent insert, and the unique constraint is the
only check that cannot. It does not validate the price either: the `CHECK` constraint does,
and its name tells `AppError` which field to name. The database already enforces these rules,
and the error mapping turns its verdicts into answers.

> **Pitfall:** mapping *every* database error to 400 or 422 blames the client for your bugs.
> A syntax error in your SQL (42601), a missing column (42703) or a wrong parameter type is a
> 500. Classify the codes you mean, and let everything else be internal.

## 15.4 Writes in transactions

A handler that writes several rows opens a transaction on its connection, and Chapter 8's rules
apply unchanged: `?` anywhere drops the `Transaction`, which rolls it back, so an error halfway
through leaves nothing behind. The connection is bound with `mut` (`let mut db` here,
`Db(mut db)` with the extractor) because `transaction()` takes `&mut self`:

```rust
use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, StatusCode, header};
use axum::routing::post;
use axum::{Json, Router};
use deadpool_postgres::Pool;
use serde::Deserialize;
use serde_json::json;
use tokio_postgres::error::SqlState;

#[derive(Deserialize)]
struct Transfer {
    from: String,
    to: String,
    amount: i64,
}

/// A database error as a response: a broken rule is the client's 422, the rest our 500.
fn db_error(error: tokio_postgres::Error) -> (StatusCode, String) {
    match error.code() {
        Some(&SqlState::CHECK_VIOLATION) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "insufficient funds".to_owned(),
        ),
        _ => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal error".to_owned(),
        ),
    }
}

async fn transfer(
    State(pool): State<Pool>,
    Json(t): Json<Transfer>,
) -> Result<StatusCode, (StatusCode, String)> {
    let mut db = pool
        .get()
        .await
        .map_err(|_| (StatusCode::SERVICE_UNAVAILABLE, "no connection".to_owned()))?;
    let tx = db.transaction().await.map_err(db_error)?;
    // Credit first, on purpose: when the debit fails, the credit must be undone.
    let credited = tx
        .execute(
            "UPDATE account SET balance = balance + $2 WHERE id = $1",
            &[&t.to, &t.amount],
        )
        .await
        .map_err(db_error)?;
    let debited = tx
        .execute(
            "UPDATE account SET balance = balance - $2 WHERE id = $1",
            &[&t.from, &t.amount],
        )
        .await
        .map_err(db_error)?; // a CHECK violation returns here; `tx` is dropped and rolls back
    if credited != 1 || debited != 1 {
        return Err((StatusCode::NOT_FOUND, "no such account".to_owned())); // rolls back too
    }
    tx.commit().await.map_err(db_error)?;
    Ok(StatusCode::NO_CONTENT)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let pool = playground::pool(1);
    pool.get()
        .await?
        .batch_execute(
            "CREATE TEMP TABLE account (
                 id      text   PRIMARY KEY,
                 balance bigint NOT NULL CHECK (balance >= 0)
             );
             INSERT INTO account VALUES ('alice', 100), ('bob', 0);",
        )
        .await?;
    let app = Router::new()
        .route("/transfers", post(transfer))
        .with_state(pool.clone());
    let send = |amount: i64| {
        Request::post("/transfers")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({ "from": "alice", "to": "bob", "amount": amount }).to_string(),
            ))
            .expect("a valid request")
    };

    let (status, _, _) = playground::call(&app, send(60)).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _, body) = playground::call(&app, send(60)).await; // only 40 left
    println!("{status} {body}");
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    // The failed transfer's credit to bob was rolled back.
    let balances: Vec<i64> = pool
        .get()
        .await?
        .query("SELECT balance FROM account ORDER BY id", &[])
        .await?
        .iter()
        .map(|row| row.get(0))
        .collect();
    assert_eq!(balances, [40, 60]);
    Ok(())
}
```

```text
422 Unprocessable Entity "insufficient funds"
```

This handler takes `State<Pool>` and gets the connection itself, which also works: the state
can be the pool alone when nothing else is shared.

Transactions that can collide with others, at `REPEATABLE READ` or `SERIALIZABLE`, or that can
deadlock, also need a retry. Chapter 8's `in_transaction` helper does it, and in a handler it is
called with an `async move` closure that owns the request:

~~~rust
// stockroom/src/http/orders.rs (excerpt)
pub async fn place(
    Db(mut db): Db,
    AppJson(order): AppJson<NewOrder>,
) -> Result<impl IntoResponse, AppError> {
    // `async move`: the closure owns the request, and each attempt borrows it from there.
    let order = db::in_transaction(&mut db, IsolationLevel::ReadCommitted, async move |tx| {
        orders::place(tx, &order).await
    })
    .await?;
    Ok((StatusCode::CREATED, etag(order.version), Json(order)))
}
~~~

[stockroom/src/http/orders.rs](../stockroom/src/http/orders.rs) ·
[stockroom/src/db/retry.rs](../stockroom/src/db/retry.rs)

> **Rust refresher:** axum requires a handler's future to be `Send`, since the server may move it
> between threads. An async closure that captures a *reference* (to `order`, to any local, even
> a `&'static str`) makes the retry helper's future fail that check with "implementation of
> `Send` is not general enough", a current compiler limitation (Chapter 8). `async move`, with
> the closure owning `order`, compiles; each attempt then lends `&order` to `orders::place`
> from inside the closure.

`in_transaction` returns `AppError::Busy` when its retries run out, and `Busy` becomes a 503
with `Retry-After: 1`: the client is told to try again, not that it did something wrong.

## 15.5 Keyset pagination with an opaque cursor

Chapter 9 explained why lists are paged by key, not by `OFFSET`. Over HTTP the key travels as
an opaque cursor: the client gets `nextCursor` with each page and sends it back as `?cursor=`
for the next, without knowing what is in it. The handler asks for one row more than the page,
to know whether there is a next page without another query:

```rust
use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::{Request, StatusCode};
use axum::routing::get;
use axum::{Json, Router};
use deadpool_postgres::Pool;
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
struct ListQuery {
    cursor: Option<String>,
    limit: Option<i64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Page {
    items: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    next_cursor: Option<String>,
}

/// The last SKU of a page, hex-encoded: opaque to clients, trivial for us.
fn encode_cursor(sku: &str) -> String {
    sku.bytes().map(|b| format!("{b:02x}")).collect()
}

fn decode_cursor(cursor: &str) -> Option<String> {
    let bytes: Option<Vec<u8>> = (0..cursor.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(cursor.get(i..i + 2)?, 16).ok())
        .collect();
    String::from_utf8(bytes?).ok()
}

async fn list(
    State(pool): State<Pool>,
    Query(query): Query<ListQuery>,
) -> Result<Json<Page>, StatusCode> {
    let limit = query.limit.unwrap_or(50).clamp(1, 500);
    let after = match query.cursor.as_deref() {
        Some(cursor) => Some(decode_cursor(cursor).ok_or(StatusCode::UNPROCESSABLE_ENTITY)?),
        None => None,
    };
    let db = pool
        .get()
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let stmt = db
        .prepare_cached(
            "SELECT sku FROM product WHERE sku > coalesce($1, '') ORDER BY sku LIMIT $2",
        )
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    // One more than asked for: if it comes back, there is a next page.
    let mut items: Vec<String> = db
        .query(&stmt, &[&after, &(limit + 1)])
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .iter()
        .map(|row| row.get(0))
        .collect();
    let next_cursor = if items.len() as i64 > limit {
        items.truncate(limit as usize);
        items.last().map(|last| encode_cursor(last))
    } else {
        None
    };
    Ok(Json(Page { items, next_cursor }))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let pool = playground::pool(1);
    pool.get()
        .await?
        .batch_execute(
            "CREATE TEMP TABLE product (sku text PRIMARY KEY);
             INSERT INTO product SELECT 'SKU-' || i FROM generate_series(1, 7) i;",
        )
        .await?;
    let app = Router::new().route("/products", get(list)).with_state(pool);

    let mut uri = "/products?limit=3".to_owned();
    let mut pages = Vec::new();
    loop {
        let (status, _, body) =
            playground::call(&app, Request::get(&uri).body(Body::empty())?).await;
        assert_eq!(status, StatusCode::OK);
        pages.push(body["items"].as_array().map_or(0, Vec::len));
        match body["nextCursor"].as_str() {
            Some(cursor) => uri = format!("/products?limit=3&cursor={cursor}"),
            None => break,
        }
    }
    assert_eq!(pages, [3, 3, 1]); // and no empty request to discover the end
    Ok(())
}
```

`sku` is unique, so it can be the whole key. For a sort column with duplicates, the cursor
holds the column and a unique tie-breaker, as Chapter 9's Exercise 3 did. Hex is enough to
make a cursor opaque; it is not a secret, and clients that decode it learn only a SKU they
already saw. A cursor that encodes something private should be signed or encrypted.

## 15.6 ETag and `If-Match`: optimistic concurrency over HTTP

Chapter 8 kept two writers from overwriting each other with a `version` column: an update
succeeds only if the version is the one the writer read. HTTP has a matching mechanism. The
server sends the version with the resource as an *entity tag*, `ETag: "3"`, and a client that
wants to update sends it back, `If-Match: "3"`. If the resource has changed meanwhile, the server
answers 412 Precondition Failed instead of applying the change:

```rust
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, Request, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use deadpool_postgres::Pool;
use serde::Deserialize;
use serde_json::{Value, json};

fn etag(version: i32) -> [(header::HeaderName, HeaderValue); 1] {
    let value = HeaderValue::from_str(&format!("\"{version}\"")).expect("digits and quotes");
    [(header::ETAG, value)]
}

/// The version an `If-Match` header names; `None` without one.
fn if_match(headers: &HeaderMap) -> Result<Option<i32>, StatusCode> {
    let Some(value) = headers.get(header::IF_MATCH) else {
        return Ok(None);
    };
    let text = value.to_str().map_err(|_| StatusCode::BAD_REQUEST)?;
    text.trim()
        .trim_matches('"')
        .parse()
        .map(Some)
        .map_err(|_| StatusCode::BAD_REQUEST)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Patch {
    price_cents: i64,
}

type ApiError = (StatusCode, Json<Value>);

fn api_error(status: StatusCode, code: &str) -> ApiError {
    (status, Json(json!({ "error": code })))
}

async fn get_one(State(pool): State<Pool>, Path(sku): Path<String>) -> Result<Response, ApiError> {
    let unavailable = |_| api_error(StatusCode::SERVICE_UNAVAILABLE, "unavailable");
    let internal = |_| api_error(StatusCode::INTERNAL_SERVER_ERROR, "internal");
    let db = pool.get().await.map_err(unavailable)?;
    let row = db
        .query_opt(
            "SELECT price_cents, version FROM product WHERE sku = $1",
            &[&sku],
        )
        .await
        .map_err(internal)?
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "not_found"))?;
    let (price, version): (i64, i32) = (row.get(0), row.get(1));
    Ok((etag(version), Json(json!({ "priceCents": price }))).into_response())
}

async fn update(
    State(pool): State<Pool>,
    Path(sku): Path<String>,
    headers: HeaderMap,
    Json(patch): Json<Patch>,
) -> Result<Response, ApiError> {
    let expected = if_match(&headers).map_err(|status| api_error(status, "bad_if_match"))?;
    let db = pool
        .get()
        .await
        .map_err(|_| api_error(StatusCode::SERVICE_UNAVAILABLE, "unavailable"))?;
    let internal = |_| api_error(StatusCode::INTERNAL_SERVER_ERROR, "internal");
    // Applied only if the version is still the expected one (or no version was given).
    let updated = db
        .query_opt(
            "UPDATE product SET price_cents = $3, version = version + 1
             WHERE sku = $1 AND version = coalesce($2, version)
             RETURNING version",
            &[&sku, &expected, &patch.price_cents],
        )
        .await
        .map_err(internal)?;
    if let Some(row) = updated {
        return Ok((etag(row.get(0)), StatusCode::OK).into_response());
    }
    // Nothing updated: no such product, or someone else changed it first.
    let current = db
        .query_opt("SELECT version FROM product WHERE sku = $1", &[&sku])
        .await
        .map_err(internal)?;
    Err(match current {
        None => api_error(StatusCode::NOT_FOUND, "not_found"),
        Some(row) => (
            StatusCode::PRECONDITION_FAILED,
            Json(json!({ "error": "stale_version", "currentVersion": row.get::<_, i32>(0) })),
        ),
    })
}

fn patch(price: i64, if_match: Option<&str>) -> Request<Body> {
    let mut request =
        Request::patch("/products/CHAIR-01").header(header::CONTENT_TYPE, "application/json");
    if let Some(tag) = if_match {
        request = request.header(header::IF_MATCH, tag);
    }
    request
        .body(Body::from(json!({ "priceCents": price }).to_string()))
        .expect("a valid request")
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let pool = playground::pool(1);
    pool.get()
        .await?
        .batch_execute(
            "CREATE TEMP TABLE product (
                 sku text PRIMARY KEY, price_cents bigint NOT NULL, version int NOT NULL DEFAULT 1
             );
             INSERT INTO product VALUES ('CHAIR-01', 4999);",
        )
        .await?;
    let app = Router::new()
        .route("/products/{sku}", get(get_one).patch(update))
        .with_state(pool);

    // Two clients read the product, and both see version 1.
    let (_, headers, _) = playground::call(
        &app,
        Request::get("/products/CHAIR-01").body(Body::empty())?,
    )
    .await;
    assert_eq!(headers[header::ETAG], "\"1\"");

    // The first one's change applies, and the version moves on.
    let (status, headers, _) = playground::call(&app, patch(4500, Some("\"1\""))).await;
    assert_eq!(
        (status, &headers[header::ETAG]),
        (StatusCode::OK, &HeaderValue::from_static("\"2\""))
    );

    // The second one's change was based on version 1: refused, with the current version.
    let (status, _, body) = playground::call(&app, patch(5500, Some("\"1\""))).await;
    println!("{status} {body}");
    assert_eq!(status, StatusCode::PRECONDITION_FAILED);
    assert_eq!(body["currentVersion"], 2);

    // Without If-Match, the last writer wins: allowed here, though an API may require it.
    let (status, _, _) = playground::call(&app, patch(5500, None)).await;
    assert_eq!(status, StatusCode::OK);
    Ok(())
}
```

```text
412 Precondition Failed {"currentVersion":2,"error":"stale_version"}
```

The whole check is the `WHERE version = coalesce($2, version)` of one `UPDATE`: no lock is held
between the client's read and its write, which may be minutes apart, and nothing is read before
writing. A client that gets 412 re-reads the resource, shows the user what changed, and tries
again with the new tag.

## 15.7 Validation at the edge

Three layers check a request, and each catches what the others cannot:

1. **Deserialization** turns the body into a typed struct, and rejects what does not fit: a
   missing field, a string where a number belongs, a SKU that `Sku::parse` refuses (stockroom's
   `#[serde(try_from = "String")]`, Chapter 14).
2. **The handler** checks rules that need several fields, or that the database cannot express
   cheaply: "a batch has 1–100 products", "a period does not end before it starts".
3. **The database** enforces the rest, as constraints (§15.3).

axum's `Json` extractor rejects a bad body by itself, but with a plain-text body and its own
status codes. To keep one error format for the whole API, wrap it. Implementing
`FromRequest` by hand shows what the wrapper does:

```rust
use axum::body::Body;
use axum::extract::{FromRequest, Request};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::json;

/// `Json`, but its rejections use the API's error format.
struct AppJson<T>(T);

impl<S, T> FromRequest<S> for AppJson<T>
where
    S: Send + Sync,
    T: DeserializeOwned,
{
    type Rejection = Response;

    async fn from_request(request: Request, state: &S) -> Result<Self, Response> {
        match Json::<T>::from_request(request, state).await {
            Ok(Json(value)) => Ok(AppJson(value)),
            Err(rejection) => Err((
                rejection.status(),
                Json(json!({ "error": "invalid", "message": rejection.body_text() })),
            )
                .into_response()),
        }
    }
}

/// A SKU, checked when it is made: deserializing one runs `parse`.
#[derive(Debug, Deserialize)]
#[serde(try_from = "String")]
struct Sku(String);

impl TryFrom<String> for Sku {
    type Error = String;
    fn try_from(text: String) -> Result<Self, String> {
        let ok = (3..=12).contains(&text.len())
            && text
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'-');
        if ok {
            Ok(Sku(text))
        } else {
            Err(format!("{text:?} is not a SKU"))
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct NewProduct {
    sku: Sku,
    name: String,
}

async fn create(AppJson(new): AppJson<NewProduct>) -> impl IntoResponse {
    // Rules that need the whole request belong here, after the types are known to be right.
    if new.name.trim().is_empty() {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({ "error": "invalid", "field": "name", "message": "a name is required" })),
        );
    }
    (StatusCode::CREATED, Json(json!({ "sku": new.sku.0 })))
}

#[tokio::main]
async fn main() {
    let app = Router::new().route("/products", post(create));
    let post = |body: &str, content_type: &str| {
        axum::http::Request::post("/products")
            .header(header::CONTENT_TYPE, content_type)
            .body(Body::from(body.to_owned()))
            .expect("a valid request")
    };
    let json = "application/json";

    for (body, content_type) in [
        (r#"{"sku": "chair", "name": "Oak chair"}"#, json),
        (
            r#"{"sku": "CHAIR-01", "name": "Oak chair", "colour": "red"}"#,
            json,
        ),
        (r#"{"sku": "CHAIR-01""#, json),
        (r#"{"sku": "CHAIR-01", "name": "Oak chair"}"#, "text/plain"),
        (r#"{"sku": "CHAIR-01", "name": " "}"#, json),
        (r#"{"sku": "CHAIR-01", "name": "Oak chair"}"#, json),
    ] {
        let (status, _, body) = playground::call(&app, post(body, content_type)).await;
        println!(
            "{} {}",
            status.as_u16(),
            body["message"].as_str().unwrap_or("-")
        );
    }
}
```

```text
422 Failed to deserialize the JSON body into the target type: sku: "chair" is not a SKU at line 1 column 15
422 Failed to deserialize the JSON body into the target type: colour: unknown field `colour`, expected `sku` or `name` at line 1 column 49
400 Failed to parse the request body as JSON: EOF while parsing an object at line 1 column 18
415 Expected request with `Content-Type: application/json`
422 a name is required
201 -
```

axum's statuses are worth knowing: 400 for a body that is not JSON, 415 for the wrong content
type, 422 for JSON of the wrong shape. The wrapper above keeps them. stockroom maps all three to
422 with the same JSON format, through a one-line derive
(`#[from_request(via(axum::Json), rejection(AppError))]`, with axum's `macros` feature), and
`QueryRejection` the same way for query strings.

## 15.8 The pipelined dashboard

`GET /reports/dashboard` runs five independent aggregates. On one pooled connection they are
pipelined with `try_join!` (Chapter 7), so the handler costs about one round trip, not five:

~~~rust
// stockroom/src/http/reports.rs (excerpt)
pub async fn dashboard(Db(db): Db) -> Result<Json<Dashboard>, AppError> {
    // `&db` is a `&Object`; deref coercion turns it into the `&tokio_postgres::Client` the
    // report needs for `query_one_scalar`.
    Ok(Json(reports::dashboard(&db).await?))
}
~~~

[stockroom/src/http/reports.rs](../stockroom/src/http/reports.rs) ·
[stockroom/src/repo/reports.rs](../stockroom/src/repo/reports.rs)

The comment points at a small trap. `query_one_scalar` exists only on `tokio_postgres::Client`
(added in 0.7.16, Chapter 2), not on `Transaction` or either `GenericClient`, so
`reports::dashboard` takes `&Client`, and the handler passes `&db`: a `&Object` that derefs, twice,
to `&Client`. A function that must also run inside a transaction would use `query_one` and
`get(0)` instead.

### Side by side — *an axum handler*

The task: `GET /products/{sku}` returns the product as JSON, or 404.

~~~rust
// sqlx (illustration)
#[derive(sqlx::FromRow, Serialize)]
struct Product {
    sku: String,
    name: String,
}

async fn product(
    State(pool): State<PgPool>,
    Path(sku): Path<String>,
) -> Result<Json<Product>, StatusCode> {
    sqlx::query_as::<_, Product>("SELECT sku, name FROM product WHERE sku = $1")
        .bind(&sku)
        .fetch_optional(&pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}
~~~

```rust
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{Request, StatusCode};
use axum::routing::get;
use axum::{Json, Router};
use deadpool_postgres::Pool;
use serde::Serialize;

#[derive(Serialize)]
struct Product {
    sku: String,
    name: String,
}

async fn product(
    State(pool): State<Pool>,
    Path(sku): Path<String>,
) -> Result<Json<Product>, StatusCode> {
    let db = pool
        .get()
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let row = db
        .query_opt("SELECT sku, name FROM product WHERE sku = $1", &[&sku])
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    Ok(Json(Product {
        sku: row.get("sku"),
        name: row.get("name"),
    }))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let pool = playground::pool(1);
    pool.get()
        .await?
        .batch_execute(
            "CREATE TEMP TABLE product (sku text PRIMARY KEY, name text NOT NULL);
             INSERT INTO product VALUES ('CHAIR-01', 'Oak chair');",
        )
        .await?;
    let app = Router::new()
        .route("/products/{sku}", get(product))
        .with_state(pool);
    let (status, _, body) = playground::call(
        &app,
        Request::get("/products/CHAIR-01").body(Body::empty())?,
    )
    .await;
    assert_eq!(
        (status, body["name"].as_str()),
        (StatusCode::OK, Some("Oak chair"))
    );
    let (status, _, _) =
        playground::call(&app, Request::get("/products/NOPE-01").body(Body::empty())?).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    Ok(())
}
```

| | sqlx | tokio-postgres |
|---|---|---|
| **Same** | The pool is the state; the handler runs one parameterized query and maps "no row" to 404 | The pool is the state; the handler runs one parameterized query and maps "no row" to 404 |
| **Different** | The query runs on the pool directly; `FromRow` derives the mapping | A connection is checked out first (`pool.get()`); the mapping is written by hand, or with `TryFrom<&Row>` |

---

## In stockroom

The router lists every endpoint, then wraps them all in a timeout and request tracing:

~~~rust
// stockroom/src/http/mod.rs (excerpt)
pub fn router(state: AppState, options: RouterOptions) -> Router {
    Router::new()
        .route("/healthz", get(health::live))
        .route("/readyz", get(health::ready))
        .route("/admin/sessions", get(health::sessions))
        .route("/stores", get(stores::list).post(stores::create))
        // …
        .route("/orders", post(orders::place))
        .route("/orders/events", get(events::orders))
        .route("/orders/{id}", get(orders::get))
        .route("/orders/{id}/transition", post(orders::transition))
        .route("/reports/dashboard", get(reports::dashboard))
        .route("/reports/sales", get(reports::sales))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::SERVICE_UNAVAILABLE,
            options.request_timeout,
        ))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
~~~

[stockroom/src/http/mod.rs](../stockroom/src/http/mod.rs)

When a handler takes longer than the request timeout, `TimeoutLayer::with_status_code`
(tower-http 0.7) drops the handler's future and answers 503. Chapter 12 showed that dropping a
future is not the same as stopping its query, which is why the slow report guards its query
with `CancelOnDrop`.

`AppError` (`src/error.rs`) is §15.3 in full: eleven variants, a stable `error` code for each, the
fields a client needs (`currentVersion` for 412, the SKU and quantities for insufficient
stock), `Retry-After` on `Busy`, and server errors logged with the `DbError`'s code, message,
detail, hint, table and constraint, but sent to
the client as "something went wrong on our side". Its mapping from the database, by SQLSTATE
and then by constraint name:

~~~rust
// stockroom/src/error.rs (excerpt)
match *db.code() {
    SqlState::UNIQUE_VIOLATION => {
        let (field, message) =
            constraint_message(db).unwrap_or((None, "this already exists"));
        AppError::Conflict {
            field,
            message: message.into(),
        }
    }
    SqlState::CHECK_VIOLATION
    | SqlState::NOT_NULL_VIOLATION
    | SqlState::FOREIGN_KEY_VIOLATION => {
        let (field, message) = constraint_message(db)
            .unwrap_or((None, "the request breaks a rule of the data"));
        AppError::Invalid {
            field,
            message: message.into(),
        }
    }
    SqlState::T_R_SERIALIZATION_FAILURE
    | SqlState::T_R_DEADLOCK_DETECTED
    | SqlState::LOCK_NOT_AVAILABLE => AppError::Busy(error),
    SqlState::QUERY_CANCELED => AppError::Timeout(error),
    SqlState::ADMIN_SHUTDOWN
    | SqlState::CANNOT_CONNECT_NOW
    | SqlState::TOO_MANY_CONNECTIONS => AppError::Unavailable(db.message().to_owned()),
    _ => AppError::Db(error),
}
~~~

[stockroom/src/error.rs](../stockroom/src/error.rs)

The rest of `http/` follows this chapter's patterns: `products::list` pages with a hex cursor
(§15.5), `products::get` and `update` send and check `ETag`/`If-Match` (§15.6), as does
`orders::transition`; `products::create_batch` uses a savepoint per item (Chapter 8) to keep the
good items of a batch when some fail; and `stockroom/stockroom.http` holds a request for every
endpoint, ready to send from an editor.

## Summary

- The `Router`'s state holds the pool (and other cheap handles); layers add tracing, timeouts and
  body limits. Tests and examples drive the router in memory with `oneshot`.
- A `Db` extractor (`FromRequestParts`) checks a connection out for the handler; the connection
  returns to the pool when the handler's future ends. Put body extractors last.
- Map database errors once, in `From<tokio_postgres::Error>`: 23505 → 409, 23514/23502/23503 →
  422 (with the field named by the constraint), 40001/40P01/55P03 → 503 with `Retry-After`,
  57014 → 503, the rest → 500 with details only in the log.
- Let constraints do the checking; do not check-then-insert. Transactions roll back on `?`.
  Retried transactions in handlers use an `async move` closure that owns the request.
- Page with an opaque keyset cursor and one extra row. Turn the version column into `ETag`, and
  `If-Match` into `WHERE version = …`; answer 412 with the current version.
- Validate in three layers: types (deserialization), the handler, the database. Wrap `Json` to
  keep one error format.

## Exercises

1. **Map lock timeouts.** stockroom's sessions have `lock_timeout = 2000`. Which status does a
   request that waited too long for a row lock get, and is that the right one? Would you retry it
   inside `in_transaction`?
   *Hint:* 55P03 is classified with serialization failures.
2. **A 409 with details.** Change §15.3's `create` so that a duplicate SKU answers 409 with the
   existing product's name in the body, without a check-then-insert race.
   *Hint:* `INSERT … ON CONFLICT (sku) DO NOTHING RETURNING …`, then a `SELECT` when nothing came
   back.
3. **Require If-Match.** Change §15.6's `update` to answer 428 Precondition Required when the
   request has no `If-Match`. Which clients does that protect, and which does it annoy?
   *Hint:* RFC 6585 defines 428 for exactly this.
4. **A cursor that lies.** In §15.5, what happens when a client sends `?cursor=zz`, and when it
   sends a valid hex cursor for a SKU that was deleted since? Is either a problem?
   *Hint:* `sku > $1` does not need `$1` to exist.

### Solutions

**1.** `AppError::Busy`, so 503 with `Retry-After: 1`, and `in_transaction` retries it like a
serialization failure. That is right for short waits behind another request's transaction: the
retry usually succeeds once that transaction ends. Behind something long (a migration, a stuck
session) the arithmetic changes: eight attempts of up to two seconds each could take sixteen
seconds, so the router's 10-second request timeout answers first, with a plain 503 and no
`Retry-After`, after holding a pool connection the whole time. A service where that matters
gives `lock_timeout`, the attempts and the request timeout budgets that fit together.

**2.** Insert with `ON CONFLICT DO NOTHING`; a row back means created, no row means it existed,
and the follow-up `SELECT` reads the existing one. There is still a tiny window (the existing
product could be deleted between the two statements), which the code below handles by
still answering 409, without the name:

```rust
use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, StatusCode, header};
use axum::routing::post;
use axum::{Json, Router};
use deadpool_postgres::Pool;
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
struct NewProduct {
    sku: String,
    name: String,
}

async fn create(
    State(pool): State<Pool>,
    Json(new): Json<NewProduct>,
) -> Result<(StatusCode, Json<Value>), StatusCode> {
    let db = pool
        .get()
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let inserted = db
        .query_opt(
            "INSERT INTO product (sku, name) VALUES ($1, $2)
             ON CONFLICT (sku) DO NOTHING RETURNING sku",
            &[&new.sku, &new.name],
        )
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if inserted.is_some() {
        return Ok((StatusCode::CREATED, Json(json!({ "sku": new.sku }))));
    }
    let existing = db
        .query_opt("SELECT name FROM product WHERE sku = $1", &[&new.sku])
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::CONFLICT)?; // deleted in between: still a conflict, without details
    let name: String = existing.get(0);
    Ok((
        StatusCode::CONFLICT,
        Json(json!({ "error": "conflict", "field": "sku", "existingName": name })),
    ))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let pool = playground::pool(1);
    pool.get()
        .await?
        .batch_execute("CREATE TEMP TABLE product (sku text PRIMARY KEY, name text NOT NULL)")
        .await?;
    let app = Router::new()
        .route("/products", post(create))
        .with_state(pool);
    let send = |name: &str| {
        Request::post("/products")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({ "sku": "CHAIR-01", "name": name }).to_string(),
            ))
            .expect("a valid request")
    };
    let (status, _, _) = playground::call(&app, send("Oak chair")).await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _, body) = playground::call(&app, send("Pine chair")).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["existingName"], "Oak chair");
    Ok(())
}
```

**3.** Return `StatusCode::PRECONDITION_REQUIRED` when `if_match` returns `None`. It protects
every client from silently overwriting someone else's change, including clients written by
people who never heard of `ETag`; it annoys scripts and quick `curl` commands, which must now
`GET` first. Many APIs require it for `PATCH` and `PUT` on contended resources and not
elsewhere.

**4.** `zz` is not hex, so `decode_cursor` returns `None` and the handler answers 422: a client
error, correctly. A cursor for a deleted SKU is fine: `sku > $1` compares values and does not
need the row to exist, so the next page starts right after where the deleted row would have
been. That robustness is one of keyset pagination's advantages over cursors that point at rows.

---

Previous: [Chapter 14 — Schema, migrations and the data layer](14-data-layer.md) ·
Next: [Chapter 16 — axum II: streaming, real-time and background work](16-axum-streaming.md)
