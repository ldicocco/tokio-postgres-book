//! `/products`, on shelf: CRUD with optimistic concurrency, a batch with savepoints, and the
//! catalog streamed as NDJSON. The CSV export and import are stockroom's.

use axum::Json;
use axum::body::{Body, Bytes};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use futures_util::{Stream, StreamExt, TryStreamExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use shelf::Table;
use stockroom::AppError;
use stockroom::db::CancelOnDrop;
use stockroom::domain::Sku;
use stockroom::http::{AppJson, AppQuery, AppState, Db, etag, if_match};

use crate::app_error;
use crate::repo::products::{self, NewProduct, Product, ProductPatch};

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    tag: Option<String>,
    cursor: Option<String>,
    limit: Option<i64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProductPage {
    items: Vec<Product>,
    /// Pass back as `?cursor=` for the next page; absent on the last page.
    #[serde(skip_serializing_if = "Option::is_none")]
    next_cursor: Option<String>,
}

pub async fn list(
    Db(db): Db,
    AppQuery(query): AppQuery<ListQuery>,
) -> Result<Json<ProductPage>, AppError> {
    let limit = query.limit.unwrap_or(50).clamp(1, 500);
    let after = query.cursor.as_deref().map(decode_cursor).transpose()?;
    // One more than asked for: if it comes back, there is a next page.
    let mut items = products::page(&db, after.as_ref(), query.tag.as_deref(), limit + 1).await?;
    let next_cursor = if items.len() as i64 > limit {
        items.truncate(limit as usize);
        items.last().map(|last| encode_cursor(&last.sku))
    } else {
        None
    };
    Ok(Json(ProductPage { items, next_cursor }))
}

pub async fn get(Db(db): Db, Path(sku): Path<String>) -> Result<impl IntoResponse, AppError> {
    let product = products::by_sku(&db, &parse_sku(&sku)?).await?;
    Ok((etag(product.version), Json(product)))
}

pub async fn create(
    Db(db): Db,
    AppJson(new): AppJson<NewProduct>,
) -> Result<impl IntoResponse, AppError> {
    let product = products::create(&db, &new).await?;
    Ok((StatusCode::CREATED, etag(product.version), Json(product)))
}

/// `PATCH` with `If-Match: "<version>"`: applied only if nobody changed the product since the
/// client read it; otherwise 412 and the current version.
pub async fn update(
    Db(db): Db,
    Path(sku): Path<String>,
    headers: HeaderMap,
    AppJson(patch): AppJson<ProductPatch>,
) -> Result<impl IntoResponse, AppError> {
    let product = products::update(&db, &parse_sku(&sku)?, if_match(&headers)?, patch).await?;
    Ok((etag(product.version), Json(product)))
}

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum BatchItem {
    Created { product: Product },
    Rejected { sku: Sku, error: Value },
}

/// Creates up to 100 products in one transaction, keeping the good ones when some fail: a
/// savepoint per item, as in stockroom. A savepoint is a deadpool `Transaction`, so it is an
/// `Executor` like any other.
pub async fn create_batch(
    Db(mut db): Db,
    AppJson(items): AppJson<Vec<NewProduct>>,
) -> Result<Json<Vec<BatchItem>>, AppError> {
    if items.is_empty() || items.len() > 100 {
        return Err(AppError::invalid("items", "a batch has 1–100 products"));
    }
    let mut tx = db.transaction().await?;
    let mut results = Vec::with_capacity(items.len());
    for item in &items {
        let savepoint = tx.savepoint("item").await?;
        match products::create(&savepoint, item).await {
            Ok(product) => {
                savepoint.commit().await?; // RELEASE SAVEPOINT
                results.push(BatchItem::Created { product });
            }
            Err(error) if error.is_client_error() => {
                savepoint.rollback().await?; // ROLLBACK TO SAVEPOINT
                results.push(BatchItem::Rejected {
                    sku: item.sku.clone(),
                    error: error.body(),
                });
            }
            Err(error) => return Err(error),
        }
    }
    tx.commit().await?;
    Ok(Json(results))
}

/// Every product as newline-delimited JSON, streamed row by row.
///
/// The body owns the pooled connection, as in stockroom, and shelf's stream borrows it, so the
/// query runs inside the body: it starts when the body is first polled. Waiting for the first
/// line here keeps an error before the first row an error response, as stockroom's handler
/// gives, rather than an empty 200.
pub async fn stream(State(state): State<AppState>, Db(db): Db) -> Result<Response, AppError> {
    let guarded = CancelOnDrop::new(db, state.tls.clone());
    let mut lines = Box::pin(ndjson(guarded).peekable());
    if let Some(Err(_)) = lines.as_mut().peek().await
        && let Some(Err(error)) = lines.next().await
    {
        return Err(error);
    }
    let body = Body::from_stream(lines);
    Ok(([(header::CONTENT_TYPE, "application/x-ndjson")], body).into_response())
}

/// The connection's owner and the stream that borrows it, together: the generator owns `db`,
/// and `products` borrows it for as long as rows arrive. A `ModelStream` cannot be returned
/// alongside the connection it borrows, but it can live inside a stream that owns both.
///
/// If the client hangs up first, axum drops the body, and `CancelOnDrop` cancels the query and
/// discards the connection (stockroom's `CancelOnDrop`, main book Chapter 16).
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

fn parse_sku(text: &str) -> Result<Sku, AppError> {
    Sku::parse(text).map_err(|e| AppError::invalid("sku", e.to_string()))
}

/// A cursor is the last SKU of a page, hex-encoded: opaque to clients, trivial for us.
/// (stockroom's, copied: its HTTP helpers are private to it.)
fn encode_cursor(sku: &Sku) -> String {
    sku.as_str().bytes().map(|b| format!("{b:02x}")).collect()
}

fn decode_cursor(cursor: &str) -> Result<Sku, AppError> {
    let invalid = || AppError::invalid("cursor", "not a cursor this service handed out");
    if !cursor.len().is_multiple_of(2) || !cursor.is_ascii() {
        return Err(invalid());
    }
    let bytes = (0..cursor.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&cursor[i..i + 2], 16))
        .collect::<Result<Vec<u8>, _>>()
        .map_err(|_| invalid())?;
    let text = String::from_utf8(bytes).map_err(|_| invalid())?;
    Sku::parse(&text).map_err(|_| invalid())
}
