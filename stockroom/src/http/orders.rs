//! `/orders`

use axum::Json;
use axum::extract::Path;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use futures_util::FutureExt;
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
    let order = db::in_transaction(
        &mut db,
        IsolationLevel::ReadCommitted,
        &order,
        |tx, order| orders::place(tx, order).boxed(),
    )
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
    let input = (id, request.to, if_match(&headers)?);
    let moved = db::in_transaction(
        &mut db,
        IsolationLevel::ReadCommitted,
        &input,
        |tx, &(id, to, expected)| orders::transition(tx, id, to, expected).boxed(),
    )
    .await?;
    Ok((etag(moved.order.version), Json(moved)))
}
