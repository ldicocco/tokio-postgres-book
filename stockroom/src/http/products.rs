//! `/products`: CRUD with optimistic concurrency, a batch with savepoints, and three ways of
//! moving many rows over HTTP (NDJSON from a row stream, CSV out and in through COPY).

use std::pin::pin;

use axum::Json;
use axum::body::{Body, Bytes};
use axum::extract::Path;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use deadpool_postgres::{GenericClient, Object};
use futures_util::{Stream, StreamExt, TryStreamExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_postgres::RowStream;

use super::{AppJson, AppQuery, Db, etag, if_match};
use crate::domain::Sku;
use crate::repo::products::{self, ImportReport, NewProduct, Product, ProductPatch};
use crate::{AppError, db};

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
    let product = products::update(&db, &parse_sku(&sku)?, if_match(&headers)?, &patch).await?;
    Ok((etag(product.version), Json(product)))
}

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum BatchItem {
    Created { product: Product },
    Rejected { sku: Sku, error: Value },
}

/// Creates up to 100 products in one transaction, keeping the good ones when some fail.
///
/// After an error, a PostgreSQL transaction refuses every further statement ("current
/// transaction is aborted"). A savepoint per item limits the damage: a failed insert is
/// rolled back to its savepoint, and the transaction carries on.
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

/// Every product as newline-delimited JSON, streamed row by row: memory use stays flat
/// however large the catalog.
pub async fn stream(Db(db): Db) -> Result<Response, AppError> {
    let stmt = db.prepare_cached(products::ALL_IN_SKU_ORDER).await?;
    let rows = db.query_raw(&stmt, db::slice_iter(&[])).await?;
    let body = Body::from_stream(ndjson(db, rows));
    Ok(([(header::CONTENT_TYPE, "application/x-ndjson")], body).into_response())
}

/// The body stream owns the pooled connection, so the connection stays checked out until the
/// last row is sent. `RowStream` does not borrow the client: had `db` gone back to the pool
/// when the handler returned, the next borrower's queries would queue behind these rows.
///
/// Once the first bytes are out, the status is 200 for good; an error after that can only
/// cut the body short, which the client sees as a truncated response.
fn ndjson(db: Object, rows: RowStream) -> impl Stream<Item = Result<Bytes, AppError>> + Send {
    async_stream::try_stream! {
        let _checked_out = db;
        let mut rows = pin!(rows);
        while let Some(row) = rows.try_next().await? {
            let mut line = serde_json::to_vec(&Product::try_from(&row)?)?;
            line.push(b'\n');
            yield Bytes::from(line);
        }
    }
}

/// The catalog as CSV, straight from `COPY … TO STDOUT`: PostgreSQL formats it, and the
/// handler only passes the bytes on.
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

/// A CSV price list, streamed from the request body into `COPY … FROM STDIN`.
pub async fn import_csv(Db(mut db): Db, body: Body) -> Result<Json<ImportReport>, AppError> {
    let tx = db.transaction().await?;
    let report = products::import_csv(&tx, body.into_data_stream()).await?;
    tx.commit().await?;
    Ok(Json(report))
}

fn parse_sku(text: &str) -> Result<Sku, AppError> {
    Sku::parse(text).map_err(|e| AppError::invalid("sku", e.to_string()))
}

/// A cursor is the last SKU of a page, hex-encoded: opaque to clients, trivial for us.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursors_round_trip_and_reject_junk() {
        let sku = Sku::parse("CHAIR-01").unwrap();
        assert_eq!(decode_cursor(&encode_cursor(&sku)).unwrap(), sku);
        for junk in ["", "4", "zz", "6368616972"] {
            assert!(decode_cursor(junk).is_err(), "{junk:?}");
        }
    }
}
