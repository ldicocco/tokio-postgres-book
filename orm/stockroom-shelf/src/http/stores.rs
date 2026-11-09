//! `/stores`, on shelf: stockroom's handlers, calling this crate's repository.

use axum::Json;
use axum::http::StatusCode;
use stockroom::AppError;
use stockroom::http::{AppJson, Db};

use crate::repo::stores::{self, NewStore, Store};

pub async fn list(Db(db): Db) -> Result<Json<Vec<Store>>, AppError> {
    Ok(Json(stores::list(&db).await?))
}

pub async fn create(
    Db(db): Db,
    AppJson(new): AppJson<NewStore>,
) -> Result<(StatusCode, Json<Store>), AppError> {
    Ok((StatusCode::CREATED, Json(stores::create(&db, &new).await?)))
}
