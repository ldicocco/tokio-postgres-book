//! Stores, on shelf: stockroom's `repo/stores.rs` with the SQL and the mapping derived.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use shelf::{Executor, FromRow, Insert, Table};
use stockroom::AppError;
use stockroom::domain::Address;
use uuid::Uuid;

use crate::app_error;

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

#[tracing::instrument(level = "debug", skip_all)]
pub async fn list(db: &impl Executor) -> Result<Vec<Store>, AppError> {
    let query = Store::query().order_by(Store::CODE.asc());
    query.all(db).await.map_err(app_error)
}

#[tracing::instrument(level = "debug", skip_all, fields(code = %new.code))]
pub async fn create(db: &impl Executor, new: &NewStore) -> Result<Store, AppError> {
    new.insert(db).await.map_err(app_error)
}

#[tracing::instrument(level = "debug", skip_all, fields(code))]
pub async fn by_code(db: &impl Executor, code: &str) -> Result<Store, AppError> {
    let query = Store::query().filter(Store::CODE.eq(code));
    let store = query.first(db).await.map_err(app_error)?;
    store.ok_or(AppError::NotFound("store"))
}
