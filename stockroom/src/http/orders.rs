//! `/orders`

use axum::Json;
use axum::extract::Path;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use serde::Deserialize;
use tokio_postgres::IsolationLevel;
use uuid::Uuid;

use super::{AppJson, Db, etag, if_match};
use crate::domain::OrderStatus;
use crate::repo::orders::{self, NewOrder};
use crate::{AppError, db};

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

#[derive(Debug, Deserialize)]
pub struct TransitionRequest {
    to: OrderStatus,
}

/// Moves an order to another status, with an optional `If-Match: "<version>"`.
pub async fn transition(
    Db(mut db): Db,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    AppJson(request): AppJson<TransitionRequest>,
) -> Result<impl IntoResponse, AppError> {
    let (to, expected) = (request.to, if_match(&headers)?);
    let moved = db::in_transaction(&mut db, IsolationLevel::ReadCommitted, async move |tx| {
        orders::transition(tx, id, to, expected).await
    })
    .await?;
    Ok((etag(moved.order.version), Json(moved)))
}
