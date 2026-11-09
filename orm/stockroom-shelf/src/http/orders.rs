//! `/orders` and `/orders/{id}`, on shelf. `/orders/{id}/transition` is stockroom's.

use axum::Json;
use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use stockroom::http::{AppJson, Db, etag};
use stockroom::repo::orders::NewOrder;
use stockroom::{AppError, db};
use tokio_postgres::IsolationLevel;
use uuid::Uuid;

use crate::repo::orders;

/// Places an order. `in_transaction` retries it if it deadlocks with another order.
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

pub async fn get(Db(db): Db, Path(id): Path<Uuid>) -> Result<impl IntoResponse, AppError> {
    let order = orders::get(&db, id).await?;
    Ok((etag(order.version), Json(order)))
}
