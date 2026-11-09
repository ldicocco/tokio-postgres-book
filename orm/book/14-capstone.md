# Chapter 14 — Capstone: stockroom on shelf

> **Building an ORM on tokio-postgres** · Previous: [Chapter 13 — Trusting the mapping: schema checks and tests](13-trusting-the-mapping.md) · Next: [Appendix — attribute reference, generated code and phrasebook](15-appendix.md)

stockroom's data layer is written by hand: a few dozen repository functions, each with its SQL,
its parameters and its `TryFrom<&Row>` (main book [Chapter 18](../../book/18-capstone.md)). It is
the code this book set out to generate. This chapter rewrites part of it with shelf, behind the
same axum routes, against the same schema, with the same domain types, and compares the two
function by function: in lines, in statements and in round trips.

The comparison is honest in both directions. shelf removes the mapping code and almost all of the
everyday SQL, and the ported routes answer exactly as stockroom's do: one test suite runs against
both routers and compares every reply. But reading an order back costs a round trip more, because
shelf has no joins, and the orders module ends up longer than stockroom's. And some code shelf
should not touch at all: the guarded `UPDATE` that keeps orders from overselling, the state
machine of an order, the `MERGE` import, the COPY export. Knowing where an ORM stops is the last
thing this book teaches.

**In this chapter you will learn:**

- how a port can reuse an application's schema, types, errors and routes, and replace only its
  data layer
- how shelf's models fit a real schema: a domain, a composite, an enum, a `jsonb` document, a
  virtual column, a composite key
- what each ported function looks like, and what it costs in lines, statements and round trips,
  measured
- how to prove that a rewrite behaves as the original: one suite, two routers
- where shelf steps aside, and how raw SQL fills the gap

---

## 14.1 The shape

[`orm/stockroom-shelf`](../stockroom-shelf/) is a library and a binary that depend on the
`stockroom` crate and on shelf. It replaces stockroom's data layer for three resources, and reuses
everything else unchanged:

| From stockroom, reused | New in stockroom-shelf |
|---|---|
| the migrations and the schema; `migrate` and `seed` stay stockroom's commands | the models and the repository: `src/repo/{stores,products,stock,orders}.rs` |
| the domain types: `Sku`, `Cents`, `Quantity`, `OrderStatus`, `Address`, `Customer` | the handlers for the ported routes, copied from stockroom's (its handler modules are private) and calling the new repository: `src/http/` |
| `AppError`, `in_transaction`, `CancelOnDrop`, `AppState`, the `Db`, `AppJson` and `AppQuery` extractors, `etag` and `if_match` | `app_error`, which converts shelf's errors into `AppError`: `src/error.rs` |
| the request and response types of orders (`NewOrder`, `Order`, `OrderLine`) | `verify`, run before `serve` accepts a request: `src/app.rs` |
| the router, for every route not ported | the router that puts the two together: `src/http/mod.rs` |

stockroom-shelf never changes stockroom. Every module of the `stockroom` crate is public except its
handlers (orm/PLAN.md §12 #47), which is what makes the reuse possible, and the reason the handlers
are copies.

The ported routes are the catalog and orders: `GET` and `POST /stores`; `GET` and `POST
/products`, `GET` and `PATCH /products/{sku}`, `POST /products/batch`, `GET /products/stream`;
`POST /orders` and `GET /orders/{id}`. Stock levels and adjustments, transfers, an order's
transitions, the CSV import and export, the event stream, the reports and the health checks stay
stockroom's, for reasons §14.5 gives.

### Two routers, one API

The port's router serves its routes and hands everything else to stockroom's router, unchanged.
Two details of axum 0.8.9 shaped it (orm/PLAN.md §12 #50):

~~~rust
// orm/stockroom-shelf/src/http/mod.rs (excerpt)
/// Static paths of stockroom's that a ported route with a parameter would capture
/// (`/products/{sku}` matches `/products/import`), so they are routed to stockroom by name.
const STOCKROOM_PATHS: [&str; 3] = ["/products/export.csv", "/products/import", "/orders/events"];

pub fn router(state: AppState, options: RouterOptions) -> Router {
    let request_timeout = options.request_timeout;
    let stockroom = stockroom::http::router(state.clone(), options);
    let mut router = Router::new()
        .route("/stores", get(stores::list).post(stores::create))
        .route("/products", get(products::list).post(products::create))
        .route("/products/batch", post(products::create_batch))
        .route("/products/stream", get(products::stream))
        .route(
            "/products/{sku}",
            get(products::get).patch(products::update),
        )
        .route("/orders", post(orders::place))
        .route("/orders/{id}", get(orders::get))
        // Only on the routes above: stockroom's router has these layers of its own.
        .route_layer(TimeoutLayer::with_status_code(
            StatusCode::SERVICE_UNAVAILABLE,
            request_timeout,
        ))
        .route_layer(TraceLayer::new_for_http());
    // `into_service`: axum refuses a `Router` as a route's service, and asks for `nest`, which
    // would strip the path that stockroom's router routes by.
    for path in STOCKROOM_PATHS {
        router = router.route_service(path, stockroom.clone().into_service());
    }
    router.fallback_service(stockroom).with_state(state)
}
~~~

[orm/stockroom-shelf/src/http/mod.rs](../stockroom-shelf/src/http/mod.rs)

First, a ported route with a parameter captures paths that stockroom's router serves as static
routes: `/products/{sku}` matches `/products/import`, and a fallback is consulted only when no
route matches, so the import would reach the port's `PATCH` handler with the SKU "import". Each
such path is therefore routed to stockroom by name. Second, `route_service(path, stockroom)`
panics at startup: "Invalid route: `Router::route_service` cannot be used with `Router`s. Use
`Router::nest` instead". `nest` would strip the prefix that stockroom's router routes by;
`into_service()` turns the router into a plain service, which axum accepts and which passes the
whole path on. The timeout and trace layers are `route_layer`s, so they wrap the ported routes
only; stockroom's routes go through stockroom's own layers, once.

### Errors across two crates

shelf's repository functions return `shelf::Error`; stockroom's handlers speak `AppError`. The
natural bridge is `impl From<shelf::Error> for AppError`, so that `?` converts, and the compiler
refuses it: E0117, "only traits defined in the current crate can be implemented for types defined
outside of the crate", because neither type belongs to stockroom-shelf (orm/PLAN.md §12 #49).
That is Chapter 1's orphan rule again, the one that stopped `impl TryFrom<&Row> for (A, B)`, met
from the other side. The port converts with a function and `map_err`:

~~~rust
// orm/stockroom-shelf/src/error.rs (excerpt)
pub fn app_error(error: shelf::Error) -> AppError {
    match error {
        shelf::Error::NotFound { model } => AppError::NotFound(noun(model)),
        shelf::Error::Stale {
            current_version, ..
        } => match i32::try_from(current_version) {
            Ok(current) => AppError::StaleVersion { current },
            Err(_) => AppError::Internal(format!("version {current_version} is not an i32")),
        },
        shelf::Error::Db(error) => error.into(),
        other => AppError::Internal(other.to_string()),
    }
}
~~~

[orm/stockroom-shelf/src/error.rs](../stockroom-shelf/src/error.rs)

`NotFound` becomes 404 with stockroom's word for the model ("product not found"), `Stale` becomes
412 with the current version, and a database error goes through stockroom's own conversion, which
maps SQLSTATEs and constraint names: a duplicate SKU written through shelf's `insert` is the same
409 with `"field": "sku"` as when stockroom wrote it. A mapping or schema error is a bug, so 500.

## 14.2 Models for a real schema

stockroom's schema uses most of what PostgreSQL offers a typed client: a domain, an enum, a
composite, `jsonb`, arrays, a virtual generated column, a composite key, and identity and
`uuidv7()` defaults. shelf's models take its types as they are, because shelf passes any type with
a `FromSql` and `ToSql` through (Chapter 4), and stockroom's domain types have them.

~~~rust
// orm/stockroom-shelf/src/repo/stores.rs (excerpt)
#[derive(Debug, Clone, Serialize, FromRow, Table)]
#[serde(rename_all = "camelCase")]
#[shelf(table = "stores")]
pub struct Store {
    #[shelf(primary_key, generated)]
    pub id: Uuid,
    pub code: String,
    pub name: String,
    pub address: Address, // the composite, through postgres-types' derive
    #[shelf(generated)]
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, Deserialize, Insert)]
#[shelf(model = Store)]
pub struct NewStore {
    pub code: String,
    pub name: String,
    pub address: Address,
}
~~~

[orm/stockroom-shelf/src/repo/stores.rs](../stockroom-shelf/src/repo/stores.rs)

The model is also the JSON: `Serialize` with stockroom's `camelCase` beside shelf's derives, so a
store is read and returned without a conversion, and `NewStore` is the request body. `Product` adds
the domain `Sku` (whose hand-written `accepts` takes the domain and `text`), `Cents`, a `text[]`,
`jsonb` as a `serde_json::Value`, and Chapter 9's `version` and `updated_at`; `ProductPatch` is the
`PATCH` body.

**The virtual column.** `stock.available` is `on_hand - reserved`, generated, virtual, from two `NOT
NULL` columns, so it is never NULL. The catalog says otherwise: a generated column is nullable
unless declared `NOT NULL`, and stockroom's is not. `verify` refused the model with `available:
i32`, exactly as Chapter 13 showed for `doubled` (orm/PLAN.md §12 #48):

~~~rust
// orm/stockroom-shelf/src/repo/stock.rs (excerpt)
#[derive(Debug, Clone, FromRow, Table)]
#[shelf(table = "stock")]
pub struct Stock {
    #[shelf(primary_key)]
    pub store_id: Uuid,
    #[shelf(primary_key)]
    pub product_id: Uuid,
    pub on_hand: i32,
    pub reserved: i32,
    /// `on_hand - reserved`, the virtual generated column (PG 18): never written. It cannot be
    /// NULL, but the catalog does not say so (a generated column is nullable unless declared
    /// `NOT NULL`), so `verify` would refuse an `i32` here.
    #[shelf(generated)]
    pub available: Option<i32>,
}
~~~

[orm/stockroom-shelf/src/repo/stock.rs](../stockroom-shelf/src/repo/stock.rs)

Two fixes were possible: `ALTER TABLE stock ALTER COLUMN available SET NOT NULL` in stockroom's
schema, or `Option<i32>` in the model. The schema belongs to stockroom, so the model gives way. The
composite key, `(store_id, product_id)`, is a `(Uuid, Uuid)` key (Chapter 6).

**Orders.** The API's order names its store by *code* and each line's product by *SKU*. No row of
`orders` or `order_lines` holds either; stockroom's `get` joins them in. So the port keeps
stockroom's request and response types, gives the two tables models of their own, named after
their rows, and assembles the response from them:

~~~rust
// orm/stockroom-shelf/src/repo/orders.rs (excerpt)
#[derive(Debug, Clone, FromRow, Table)]
#[shelf(table = "orders", belongs_to(Store, key = store_id))]
#[shelf(has_many(LineRow, key = order_id, as = lines))]
pub struct OrderRow {
    #[shelf(primary_key, generated)]
    pub id: Uuid,
    pub store_id: Uuid,
    pub status: OrderStatus,
    #[shelf(json)]
    pub customer: Customer,
    pub total_cents: Cents,
    #[shelf(version)]
    pub version: i32,
    #[shelf(generated)]
    pub created_at: Timestamp,
    #[shelf(updated_at)]
    pub updated_at: Timestamp,
}

/// A new order's row; the status, the version and the times are the table's defaults.
#[derive(Debug, Insert)]
#[shelf(model = OrderRow)]
pub struct NewOrderRow<'a> {
    pub store_id: Uuid,
    #[shelf(json)]
    pub customer: &'a Customer,
    pub total_cents: Cents,
}

#[derive(Debug, Clone, FromRow, Table)]
#[shelf(table = "order_lines", belongs_to(Product, key = product_id))]
pub struct LineRow {
    #[shelf(primary_key)]
    pub order_id: Uuid,
    #[shelf(primary_key)]
    pub product_id: Uuid,
    pub quantity: Quantity,
    pub unit_price_cents: Cents,
}
~~~

[orm/stockroom-shelf/src/repo/orders.rs](../stockroom-shelf/src/repo/orders.rs)

Every attribute of the book is here: an enum (`OrderStatus`, postgres-types' derive), a `json`
document (`Customer`), `version` and `updated_at`, two `belongs_to` and a `has_many`, a borrowed
`json` field on an `Insert` (`&'a Customer`, written through Chapter 8's `Json::from_ref`), and the
domain `Quantity`, which `NewLineRow::insert_many` sends as an array cast to `positive_quantity[]`,
the cast learnt from the prepared `INSERT` (Chapter 12). An `AFTER INSERT` trigger on `orders`
writes the outbox row and the notification; it fires for shelf's insert as for any other.

## 14.3 Line for line

### Stores: the everyday case

stockroom's `stores::create`, then the port's:

~~~rust
// stockroom/src/repo/stores.rs (excerpt)
pub async fn create(db: &impl GenericClient, new: &NewStore) -> Result<Store, AppError> {
    let stmt = db
        .prepare_cached(
            "INSERT INTO stores (code, name, address) VALUES ($1, $2, $3)
             RETURNING id, code, name, address, created_at",
        )
        .await?;
    let row = db
        .query_one(&stmt, &[&new.code, &new.name, &new.address])
        .await?;
    Ok(Store::try_from(&row)?)
}
~~~

[stockroom/src/repo/stores.rs](../../stockroom/src/repo/stores.rs)

~~~rust
// orm/stockroom-shelf/src/repo/stores.rs (excerpt)
pub async fn create(db: &impl Executor, new: &NewStore) -> Result<Store, AppError> {
    new.insert(db).await.map_err(app_error)
}
~~~

[orm/stockroom-shelf/src/repo/stores.rs](../stockroom-shelf/src/repo/stores.rs)

Both send one statement: an `INSERT … RETURNING` with the columns named and the values as
parameters, prepared once per pooled connection. The difference is who wrote it, and what keeps
it in step with the struct. In stockroom, three lists must agree, the column list, the parameter
list and `TryFrom<&Row>`'s twelve lines, and a review is what checks them; in the port, the
derives write all three from the struct, and `verify` checks the struct against the table at
startup.

### Products: the version check, and a retry

stockroom's `PATCH /products/{sku}` is one statement: `coalesce` per column (Chapter 9's design
(b), safe here because no patchable column is nullable), `WHERE sku = $1 AND version =
coalesce($2, version)`, so a request without `If-Match` patches whatever version is current. A
shelf `Patch` always checks the version it was made against, and applies by key, not by SKU. So
the port reads the product first, then patches at the version the client sent, or at the one it
read:

~~~rust
// orm/stockroom-shelf/src/repo/products.rs (excerpt)
pub async fn update(
    db: &impl Executor,
    sku: &Sku,
    expected_version: Option<i32>,
    mut patch: ProductPatch,
) -> Result<Product, AppError> {
    loop {
        let product = by_sku(db, sku).await?;
        patch.version = expected_version.unwrap_or(product.version);
        match patch.apply(db, &product.id).await {
            Err(shelf::Error::Stale { .. }) if expected_version.is_none() => continue,
            result => return result.map_err(app_error),
        }
    }
}
~~~

[orm/stockroom-shelf/src/repo/products.rs](../stockroom-shelf/src/repo/products.rs)

Without `If-Match`, another write can land between the read and the patch; the patch is then
`Stale`, and the loop reads again and patches the newer row. Each retry means that someone else's
write succeeded, so the loop always makes progress. A test fires 30 patches without `If-Match` at
once, and every one returns 200 with the row at version 31 (`concurrent_patches_without_if_match_all_apply`,
orm/PLAN.md §12 #51). The cost is a second statement and a second round trip; Exercise 2 gives
`Patch` an optional version and removes both.

### Orders: placing one

`place` is stockroom's function with its reads and writes moved to shelf. Validation (one line per
SKU, at most 100 lines, a total that fits) is unchanged and elided here:

~~~rust
// orm/stockroom-shelf/src/repo/orders.rs (excerpt)
pub async fn place(tx: &Transaction<'_>, order: &NewOrder) -> Result<Order, AppError> {
    // … validation, unchanged from stockroom: the wanted quantity per SKU
    let skus = wanted.keys().copied();
    let (store, products) = tokio::try_join!(stores::by_code(tx, &order.store), async {
        let query = Product::query().filter(Product::SKU.any(skus));
        query.all(tx).await.map_err(app_error)
    })?;
    // … each wanted SKU paired with its product, sorted by product id; the total
    // Every reservation at once: pipelined, sent in this order, run one by one by the server.
    let reserve = tx.prepare_cached(RESERVE).await?;
    let (reserve, store_id) = (&reserve, &store.id);
    let reserved = future::try_join_all(lines.iter().map(|(product, quantity)| async move {
        let units = quantity.get();
        tx.execute(reserve, &[store_id, &product.id, &units]).await
    }))
    .await?;
    // … a reservation that updated no row: InsufficientStock, with what is available
    let head = NewOrderRow {
        store_id: store.id,
        customer: &order.customer,
        total_cents: total,
    }
    .insert(tx)
    .await
    .map_err(app_error)?;
    // … one NewLineRow per line
    // All lines in one statement: one array per column, zipped back into rows by unnest.
    let rows = NewLineRow::insert_many(tx, &rows)
        .await
        .map_err(app_error)?;
    // … the response, assembled from `head` and `rows`
}
~~~

[orm/stockroom-shelf/src/repo/orders.rs](../stockroom-shelf/src/repo/orders.rs)

The store and the products are read at once, pipelined, through the builder (`= ANY` with the
SKUs, which takes stockroom's `Sku` because a comparison types the parameter as the domain's base
type, Chapter 10). The reservations keep their SQL (§14.5). The order and its lines are written by
`insert` and `insert_many`, whose `RETURNING` rows give the response directly. stockroom reads the
order back after writing it; the port does not need to, which is where it saves round trips.

### Measured

`line_for_line`, an example in the capstone, calls each ported function on both sides through one
pooled connection that passes two proxies in the same process: a counter of the protocol messages
the client sends (each *Parse* is a statement prepared, each *Execute* one run), then lagproxy, at
5 ms each way. A call's time divided by one round trip's (a cached `SELECT 1`, 13.9 ms) is its
round trips:

~~~bash
cargo run -p stockroom-shelf --release --example line_for_line -- --delay-ms 5
~~~

| Function | Side | Prepared, cold | Prepared, warm | Run | Round trips | ms (3 runs) |
|---|---|---:|---:|---:|---:|---|
| `stores::list`, `stores::create`, `products::page`, `by_sku`, `products::create` | both | 1 | 0 | 1 | 1.0 | 13.5–14.4 |
| `products::update`, with or without `If-Match` | stockroom | 1 | 0 | 1 | 1.0 | 13.7–14.3 |
| | shelf | 2 | 0 | 2 | 2.0–2.1 | 27.3–28.5 |
| `orders::place`, 3 lines | stockroom | 7 | 2 | 9 | 9.0–9.1 | 124.4–125.7 |
| | shelf | 6 | 0 | 7 | 5.9–6.1 | 82.4–85.0 |
| `orders::get`, 3 lines | stockroom | 2 | 0 | 2 | 1.0 | 13.8–14.3 |
| | shelf | 4 | 0 | 4 | 2.0 | 27.6–28.1 |

(Median of five warm calls per function and side, three runs; release build, rustc 1.99.1, Apple
Silicon, PostgreSQL 18.6 on the same machine. "Prepared, cold" is what a call prepares on an empty
statement cache; "warm", what it still prepares when the cache is full. `BEGIN` and `COMMIT` are
simple queries: in the round trips, not in the counts.)

Four rows say something:

- **The everyday functions cost the same.** One statement, one round trip, on both sides: shelf's
  generated statements are what stockroom wrote by hand.
- **`products::update` costs one round trip more**, the read by SKU, for the reasons above.
- **`orders::place` costs three fewer.** stockroom sends its two `INSERT`s as SQL text, which
  tokio-postgres prepares on every call (2 prepared, warm), and reads the order back with two
  joins; the port's inserts are cached statements, and their `RETURNING` rows are the response.
  `insert_many` prepares the single-row `INSERT` for its column types and never runs it, hence 6
  prepared and 7 run.
- **`orders::get` costs one round trip more**, the cost of having no joins: the order and its lines
  pipelined, then the store and the products pipelined, where stockroom's two joined statements
  go in one round trip (Chapter 11, In stockroom).

### Lines of Rust

Counted per item, blank lines and comments left out, attributes counted (orm/PLAN.md §12 #55):

| | stockroom | stockroom-shelf |
|---|---:|---:|
| Stores: the model and the ported functions | 57 | 34 |
| Products: the model, the request types and the ported functions | 150 | 87 |
| Orders: `place` and `get`, with the models and the response's assembly | 148 | 175 |
| Hand-written SQL statements in the ported functions | 15 | 1 |

Stores and products shrink by about 40%: the `TryFrom<&Row>` impls disappear (12 and 16 lines),
`create` goes from 13 and 23 lines to 4, `update` from 45 to 16. Orders grow, because the API's
order is not a row: the port needs two table models and their `Insert`s (44 lines) and a function
that assembles the response from them (33), where stockroom's joins put the response's columns in
one row. The one hand-written statement left is the reservation.

### The NDJSON stream

`GET /products/stream` sends every product as a line of JSON, as it is read. Its body must own the
pooled connection (main book [§16.1](../../book/16-axum-streaming.md#161-ndjson-from-a-rowstream-who-owns-the-connection)),
and a shelf `ModelStream` borrows its connection (Chapter 12, §12.4), so it cannot be returned
beside it. The port puts both in one generator, which owns the connection and lends it to the
stream inside:

~~~rust
// orm/stockroom-shelf/src/http/products.rs (excerpt)
fn ndjson(db: CancelOnDrop) -> impl Stream<Item = Result<Bytes, AppError>> + Send {
    async_stream::try_stream! {
        let query = Product::query().order_by(Product::SKU.asc());
        let mut products = query.stream(&*db).await.map_err(app_error)?;
        while let Some(product) = products.try_next().await.map_err(app_error)? {
            let mut line = serde_json::to_vec(&product)?;
            line.push(b'\n');
            yield Bytes::from(line);
        }
        drop(products);
        drop(db.finish()); // every row sent: the connection goes back to the pool
    }
}
~~~

[orm/stockroom-shelf/src/http/products.rs](../stockroom-shelf/src/http/products.rs)

`db` is stockroom's `CancelOnDrop`: if the client hangs up mid-stream, axum drops the body, the
generator is dropped with the connection inside it, and `CancelOnDrop` cancels the query and
discards the connection rather than returning it busy. stockroom's test for that, 50,000 rows and a
client that leaves after the first, passes on the port (orm/PLAN.md §12 #52). The handler awaits
the first line before answering, so that an error before the first row is an error response, as in
stockroom, rather than an empty `200`.

## 14.4 One suite, two routers

A rewrite is only as good as the evidence that it behaves as the original. The capstone's route
tests are stockroom's own tests for the ported routes, run against both routers: each scenario is
an async closure that sends requests and returns the replies, and `both` runs it on stockroom's
router and on the port's, each with a database of its own, and compares the replies one by one:

~~~rust
// orm/stockroom-shelf/tests/common/mod.rs (excerpt)
pub async fn both(scenario: impl AsyncFn(&TestApp) -> Vec<Reply>) {
    let (stockroom_db, shelf_db) = (TestDb::new().await, TestDb::new().await);
    let stockroom = scenario(&stockroom_db.app(4)).await;
    let shelf = scenario(&shelf_app(&shelf_db, 4)).await;
    assert_eq!(
        stockroom.len(),
        shelf.len(),
        "the scenario sent as many requests"
    );
    for (i, (a, b)) in stockroom.iter().zip(&shelf).enumerate() {
        assert_eq!(
            a.status, b.status,
            "reply {i}: status\n{}\n{}",
            a.text, b.text
        );
        for header in ["etag", "content-type"] {
            assert_eq!(
                a.headers.get(header),
                b.headers.get(header),
                "reply {i}: {header}"
            );
        }
        assert_eq!(masked(a), masked(b), "reply {i}: body");
    }
}
~~~

[orm/stockroom-shelf/tests/common/mod.rs](../stockroom-shelf/tests/common/mod.rs)

The status, the `ETag`, the content type and the body must match, once what differs between two
databases is masked: generated UUIDs and timestamps. The databases come from stockroom's harness,
included by `#[path]` (a test helper cannot be a dependency), so both sides start from the same
migrated template (main book §17.2). Eleven scenarios send 63 requests whose replies are compared:
stores created and listed and a duplicate refused, a product created and read and every error a
client can cause, updates with and without `If-Match`, keyset pages with and without a tag, a batch
that keeps its good items through savepoints, the stream, an order placed and read back, an order
that cannot be placed and reserves nothing, an order placed on shelf and moved through stockroom's
transitions, unknown orders, and the routes that are not ported.

**Which handler answered?** Identical replies cannot say, since that is their point. The statement
cache can: on a pool of one connection, each ported route leaves shelf's SQL in deadpool's cache,
and a stockroom route leaves its own:

~~~rust
// orm/stockroom-shelf/tests/routes.rs (excerpt)
    let sku = Sku::parse("CHAIR-01").unwrap();
    let by_sku = Product::query().filter(Product::SKU.eq(&sku));
    let shelf_statements = [
        NewStore::INSERT.to_owned(),
        NewProduct::INSERT.to_owned(),
        OrderRow::FIND.to_owned(),
        format!("{} LIMIT 1", by_sku.sql()),
        Store::query().order_by(Store::CODE.asc()).sql(),
    ];
    let connection = app.state.pool.get().await.unwrap();
    for sql in &shelf_statements {
        let cached = connection.statement_cache.remove(sql, &[]);
        assert!(cached.is_some(), "not sent: {sql}");
    }
~~~

[orm/stockroom-shelf/tests/routes.rs](../stockroom-shelf/tests/routes.rs)

The statements are shelf's own constants and the builder's rendered SQL, so the test does not
restate any SQL; `StatementCache::remove(sql, &[])` returns the statement if this connection
prepared that text (orm/PLAN.md §12 #53).

**What it covers, and what not.** A deliberate break in the port, reporting "orders not found" with
the wrong word, failed the comparison. A break in the helper that names models in errors, for
"Product", did not: `by_sku` builds its own `NotFound`, so no scenario reached the broken branch. A
comparison covers exactly the paths its scenarios take; the scenarios are the specification, and a
reviewer reads them as such (Chapter 13, §13.4).

stockroom's concurrency tests run on the port too. Fifty customers order the last twenty chairs at
once, on sixteen pooled connections: exactly twenty orders succeed, thirty are refused with 409,
and the stock reads 20 on hand, 20 reserved, 0 available. Orders that share products, listing them
in either order, queue and never deadlock, because every order locks its stock rows in product-id
order. Both properties come from the reservation's SQL, which the port kept; the tests check that
nothing around it, the reads and inserts that moved to shelf, changed what it guarantees.

## 14.5 Where shelf steps aside

The reservation is one statement whose `WHERE` clause is a business rule:

~~~rust
// orm/stockroom-shelf/src/repo/orders.rs (excerpt)
/// The reservation, as in stockroom: atomic at READ COMMITTED, and refused when it would take
/// more than is available.
const RESERVE: &str = "UPDATE stock SET reserved = reserved + $3
     WHERE store_id = $1 AND product_id = $2 AND available >= $3";
~~~

[orm/stockroom-shelf/src/repo/orders.rs](../stockroom-shelf/src/repo/orders.rs)

`available >= $3` is checked and the row is updated in one step, under the row's lock, so two
orders for the last chair cannot both pass: the second sees the first's update and matches no row
(main book Chapter 8). A model layer would read the stock, check in Rust and write it back, which
is a race unless the read takes a lock, and then the code says less clearly what it guarantees.
shelf could express it as a raw fragment inside some `update_where`, but the SQL *is* the rule, and
a reviewer should see it as written.

The routes left unported stay stockroom's for the same reason, each because its statement says
something a model layer cannot say better:

| Route | Its statement | Why it stays SQL |
|---|---|---|
| `POST /orders/{id}/transition` | `UPDATE orders SET status = $2, … WHERE id = $1 AND status = ANY($3) AND version = coalesce($4, version) RETURNING old.status` | the state machine and the version in one `WHERE`, and `RETURNING old` (PG 18) to report what the status was |
| `POST /stores/{code}/stock/adjust` | `INSERT … ON CONFLICT (store_id, product_id) DO UPDATE SET on_hand = stock.on_hand + EXCLUDED.on_hand` | an upsert that *adds*; shelf's upsert sets columns (Chapter 8) |
| `POST /stock/transfers` | a read and two writes at `SERIALIZABLE`, retried | the isolation level is the point (main book Chapter 8) |
| `POST /products/import` | `COPY` into a staging table, then `MERGE … RETURNING merge_action()` | a set operation over a client's file, with errors by line |
| `GET /products/export.csv` | `COPY (SELECT …) TO STDOUT` | a byte stream, not models |
| `GET /orders/events` | `LISTEN` on a dedicated connection | not a query at all |
| `GET /reports/dashboard`, `/reports/sales` | aggregates and joins | results that are not one table's rows |

Each of these is a few lines of SQL in stockroom's repository, and a `FromRow` struct reads the
result where there is one. The port's router reaches them by fallback, so the service answers the
whole API; one of the route scenarios moves an order placed by the port's `place` through
stockroom's `transition`, checking that the two data layers agree on every row they share.

**The escape hatch for `get`.** The round trip `orders::get` lost is not a limit of the approach,
only of the builder: stockroom's two joined statements can be read with two `FromRow` structs
through `fetch_one` and `fetch_all`, pipelined, which is one round trip again, and no assembly code. Exercise 3 does it. That is the pattern for the whole port: shelf for the everyday
reads and writes, SQL for the statements whose wording is a rule or whose shape is not a table's.

## 14.6 Running it

stockroom's commands migrate and seed the database; the port's binary checks its models and
serves:

```bash
cargo run -p stockroom -- migrate                    # stockroom's schema, by stockroom
cargo run -p stockroom -- seed --reset               # stores, products, stock
cargo run -p stockroom-shelf -- verify
cargo run -p stockroom-shelf -- serve                # :8080, verifies first
```

A session, against the development database (`STOCKROOM_DATABASE_URL`), the service's log
included:

```text
$ cargo run -q -p stockroom -- migrate
2026-11-09T22:24:51.536123Z  INFO stockroom::db::connect: relation "schema_migrations" already exists, skipping severity="NOTICE" code="42P07"
the schema is up to date
$ cargo run -q -p stockroom -- seed --reset
seeded 3 stores, 1000 products, 2995 stock rows, 20000 movements
$ cargo run -q -p stockroom-shelf -- verify
every model matches its table
$ cargo run -q -p stockroom-shelf -- serve
2026-11-09T22:24:58.689362Z  INFO stockroom::db::connect: relation "schema_migrations" already exists, skipping severity="NOTICE" code="42P07"
2026-11-09T22:24:58.724041Z  INFO stockroom_shelf::app: listening on http://127.0.0.1:8080
2026-11-09T22:24:58.752009Z  INFO stockroom::notify::listener: listening for order events
2026-11-09T22:24:59.207408Z  INFO stockroom::db::cancel: cancelled a query nobody was waiting for
```

and, from another terminal, while it ran:

```text
$ curl -s -i localhost:8080/products/SKU00001
HTTP/1.1 200 OK
content-type: application/json
etag: "1"
content-length: 249
date: Mon, 09 Nov 2026 22:24:59 GMT

{"id":"01a1c26a-0ad9-7eef-91bd-b763151e312e","sku":"SKU00001","name":"Product 1","priceCents":79100,"tags":["kitchen"],"attributes":{"weightGrams":9024},"version":1,"createdAt":"2026-11-09T22:24:51.909645Z","updatedAt":"2026-11-09T22:24:51.909645Z"}
$ curl -s localhost:8080/stores/store-01/stock | head -c 120
[{"sku":"SKU00001","onHand":50,"reserved":0,"available":50},{"sku":"SKU00002","onHand":89,"reserved":0,"available":89},{
$ curl -s localhost:8080/products/stream | head -2 | cut -c 1-80
{"id":"01a1c26a-0ad9-7eef-91bd-b763151e312e","sku":"SKU00001","name":"Product 1"
{"id":"01a1c26a-0ada-754a-930b-b7526425ad3f","sku":"SKU00002","name":"Product 2"
```

The first request was the port's: `Product` read by the builder and serialized as stockroom's JSON,
with stockroom's `ETag` helper on top. The second was stockroom's own handler, reached by the
fallback, since stock levels are not ported. The third was the port's stream, and `head -2` hung
up after two lines: the service's last log line is stockroom's `CancelOnDrop`, cancelling the rest
of the query and discarding the connection, exactly as for stockroom's own stream. The
`schema_migrations` notice is the migration runner's `CREATE TABLE IF NOT EXISTS`, which both
binaries run at start, logged because stockroom logs every notice (main book Chapter 6).

`stockroom/stockroom.http` works against the port unchanged, since the API is the same. The tests:

```bash
cargo test -p stockroom-shelf                        # both routers, oversell, models
cargo test -p stockroom-shelf --test concurrency     # the oversell and deadlock tests
```

## 14.7 What a production ORM would add

shelf is complete as a teaching ORM, and deliberately small. What it would need next, roughly in
the order a real project would feel the lack:

- **Joins.** The round trip `orders::get` lost. A typed join needs a model of the joined row and a
  way to name two tables' columns in one query, which is where diesel's type-level schema earns
  its complexity. A middle way, as SeaORM 2 does, joins one-to-one relations into the parent's
  query and loads one-to-many ones in batches (Chapter 11).
- **Composite keys in relations**, and many-to-many through a junction table (Chapter 11, Exercise
  3, and SeaORM's `via`).
- **Migrations from models**, or at least a check that every model's table exists before the first
  migration that needs it. `verify` is the read-only half of that.
- **Hooks**: `before_insert`, `after_update`, for timestamps and audit rows that the database's
  defaults and triggers do not already cover.
- **Optional versions** on a patch (Exercise 2) and conditional updates in general
  (`update_where`), for the cases that are rules but simple ones.
- **Other databases.** shelf is PostgreSQL only, by design: `= ANY`, `unnest`, binary COPY,
  `RETURNING`, the catalog. diesel, SeaORM and sqlx pay for portability with abstractions shelf
  does not need (Chapter 0, §0.2).

What it should not add, in this book's opinion, is lazy loading and an identity map: both hide
round trips and state, and Chapter 11 argued why every round trip should be a call you can see.

## Summary

- `orm/stockroom-shelf` serves stockroom's whole API: stores, products and orders through
  handlers on shelf, everything else through stockroom's router by fallback, with the static paths
  a ported `{param}` would capture routed by name (`into_service`).
- The port reuses stockroom's migrations, domain types, `AppError`, extractors and transaction
  helper unchanged; shelf's errors become `AppError` through `map_err(app_error)`, since the orphan
  rule refuses the `From` impl.
- shelf's models take stockroom's types as they are: a domain, a composite, an enum, `jsonb`, arrays,
  a composite key. A virtual generated column is nullable in the catalog, so the model reads it as an
  `Option`.
- The everyday functions cost one statement and one round trip on both sides; `products::update`
  costs one more (a read by SKU, and a retry without `If-Match`), `orders::get` one more (no joins),
  and `orders::place` three fewer (cached inserts whose `RETURNING` rows are the response).
- Stores and products shrink by about 40% in lines, orders grow, because the API's order is not a
  row; 15 hand-written statements become 1.
- One suite runs against both routers and compares every reply; the statement cache shows which
  handler answered. It covers the paths its scenarios take, and no others.
- shelf steps aside for statements whose wording is a rule (the reservation, transitions, an
  adding upsert, a serializable transfer) or whose shape is not a table's (COPY, `MERGE`,
  `LISTEN`, reports).

## Exercises

These extend stockroom-shelf itself. Each needs a scenario in `tests/routes.rs` that runs it against
both routers.

1. **Port `GET /stores/{code}/stock`.** Return the stock of a store, one entry per product with its
   SKU, as stockroom does.
   *Hint:* the store by code, its `Stock` rows by a filter on `store_id`, and their products by a
   loader; `available` is read only, and an `Option` in the model, but the API's is a number.
2. **An optional version.** Give shelf's `Patch` a way to patch whatever the current version is
   (`version = coalesce($n, version)`), and remove the read and the retry from
   `products::update`.
   *Hint:* `patch_sql` in `shelf/src/patch.rs` writes the version check; decide what `None` means
   for the generated field, and keep `Stale` for a version that was given.
3. **`orders::get` with joins.** Rewrite it with stockroom's two joined statements, read through
   `FromRow` structs, and measure it with `line_for_line`.
   *Hint:* two structs shaped like the joined rows, two `fetch_*` calls in one `try_join!`.

### Solutions

**1.** A `StockLine` response type (`sku`, `on_hand`, `reserved`, `available`), serialized like
stockroom's. In the repository: `stores::by_code`, then `Stock::query().filter(Stock::STORE_ID.eq(store.id))`
ordered as stockroom orders it, then `Stock::load_product`-style loading of the products, which
needs a `belongs_to(Product, key = product_id)` on `Stock`. Map `available: Option<i32>` with
`unwrap_or_default()` or, better, compute `on_hand - reserved` in Rust, which is what the virtual
column means. Three round trips against stockroom's one join; pipelining the store lookup is not
possible, since the stock query needs its id, so the escape hatch of §14.5 is worth considering
here too. Add the path to the router, and a `both` scenario that adjusts stock through stockroom's
route and reads it through the port's.

**2.** In `shelf/src/patch.rs`, `patch_sql` writes `"version" = $n` into the `WHERE` clause; make
it `"version" = coalesce($n, "version")` and the generated parameter an `Option<i32>`, so that
`None` means "any version". The `#[shelf(version)]` field on a `Patch` becomes `Option<…>` (the
derive's `const` check that the model has a version stays), and `Stale` can only come from a
`Some`. `products::update` then needs the key: either a `Patch` that applies by a unique column
(`apply_by(Product::SKU, &sku)`, a larger change), or keep the read by SKU and drop only the loop.
Update the expansion test (`SHELF_BLESS=1`, read the diff), the `patch` integration tests, and
Chapter 9's text about the design.

**3.** Two structs, `OrderHead { id, store, status, customer, total_cents, version, created_at,
updated_at }` (with `store` the joined `s.code`, and `#[shelf(json)] customer`) and `LineView { sku,
quantity, unit_price_cents }`, then `tokio::try_join!(fetch_opt::<OrderHead>(db, HEAD, &[&id]),
fetch_all::<LineView>(db, LINES, &[&id]))` with stockroom's two statements as constants. The
parameter slice must be bound to a name before the `try_join!`, since both futures borrow it (the
main book's verified facts). `line_for_line` should then report 2 statements and 1.0 round trip for
`get` on both sides, and `view` disappears. The port keeps its models for writing; reading the
response's shape directly is SQL's job.

---

Previous: [Chapter 13 — Trusting the mapping: schema checks and tests](13-trusting-the-mapping.md) · Next: [Appendix — attribute reference, generated code and phrasebook](15-appendix.md)
