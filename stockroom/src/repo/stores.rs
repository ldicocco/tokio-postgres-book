//! Stores: where orders are collected.

use deadpool_postgres::GenericClient;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use tokio_postgres::Row;
use uuid::Uuid;

use super::from_rows;
use crate::AppError;
use crate::domain::Address;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Store {
    pub id: Uuid,
    pub code: String,
    pub name: String,
    pub address: Address,
    pub created_at: Timestamp,
}

impl TryFrom<&Row> for Store {
    type Error = tokio_postgres::Error;

    fn try_from(row: &Row) -> Result<Self, Self::Error> {
        Ok(Store {
            id: row.try_get("id")?,
            code: row.try_get("code")?,
            name: row.try_get("name")?,
            address: row.try_get("address")?, // the composite, through the derive
            created_at: row.try_get("created_at")?,
        })
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct NewStore {
    pub code: String,
    pub name: String,
    pub address: Address,
}

#[tracing::instrument(level = "debug", skip_all)]
pub async fn list(db: &impl GenericClient) -> Result<Vec<Store>, AppError> {
    let stmt = db
        .prepare_cached("SELECT id, code, name, address, created_at FROM stores ORDER BY code")
        .await?;
    from_rows(&db.query(&stmt, &[]).await?)
}

#[tracing::instrument(level = "debug", skip_all, fields(code = %new.code))]
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

#[tracing::instrument(level = "debug", skip_all, fields(code))]
pub async fn id_by_code(db: &impl GenericClient, code: &str) -> Result<Uuid, AppError> {
    let stmt = db
        .prepare_cached("SELECT id FROM stores WHERE code = $1")
        .await?;
    match db.query_opt(&stmt, &[&code]).await? {
        Some(row) => Ok(row.try_get(0)?),
        None => Err(AppError::NotFound("store")),
    }
}
