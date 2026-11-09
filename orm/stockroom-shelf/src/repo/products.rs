//! Products, on shelf: the catalog's reads and writes from stockroom's `repo/products.rs`.
//!
//! The CSV import (`MERGE`) and export (`COPY`) are not here: they stay stockroom's, whose
//! handlers the router falls back to. Their SQL is the point of them.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use shelf::{Executor, FromRow, Insert, Patch, Table};
use stockroom::AppError;
use stockroom::domain::{Cents, Sku};
use uuid::Uuid;

use crate::app_error;

#[derive(Debug, Clone, Serialize, FromRow, Table)]
#[serde(rename_all = "camelCase")]
#[shelf(table = "products")]
pub struct Product {
    #[shelf(primary_key, generated)]
    pub id: Uuid,
    pub sku: Sku,
    pub name: String,
    pub price_cents: Cents,
    pub tags: Vec<String>,
    pub attributes: Value,
    #[shelf(version)]
    pub version: i32,
    #[shelf(generated)]
    pub created_at: Timestamp,
    #[shelf(updated_at)]
    pub updated_at: Timestamp,
}

#[derive(Debug, Clone, Deserialize, Insert)]
#[serde(rename_all = "camelCase")]
#[shelf(model = Product)]
pub struct NewProduct {
    pub sku: Sku,
    pub name: String,
    pub price_cents: Cents,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default = "empty_object")]
    pub attributes: Value,
}

fn empty_object() -> Value {
    Value::Object(Default::default())
}

/// A partial update: absent fields keep their value.
#[derive(Debug, Clone, Default, Deserialize, Patch)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[shelf(model = Product)]
pub struct ProductPatch {
    pub name: Option<String>,
    pub price_cents: Option<Cents>,
    pub tags: Option<Vec<String>>,
    pub attributes: Option<Value>,
    /// The version the patch is made against: not in the request body, but set by
    /// [`update`] from `If-Match` or from the row it reads.
    #[serde(skip)]
    #[shelf(version)]
    pub version: i32,
}

/// One page of products in SKU order, starting after `after` (keyset pagination). Each
/// combination of `after` and `tag` renders a statement of its own: four, each prepared once
/// per pooled connection.
#[tracing::instrument(level = "debug", skip_all)]
pub async fn page(
    db: &impl Executor,
    after: Option<&Sku>,
    tag: Option<&str>,
    limit: i64,
) -> Result<Vec<Product>, AppError> {
    let mut query = Product::query().order_by(Product::SKU.asc()).limit(limit);
    if let Some(after) = after {
        query = query.after(Product::SKU, after);
    }
    if let Some(tag) = tag {
        query = query.filter(Product::TAGS.contains(tag));
    }
    query.all(db).await.map_err(app_error)
}

#[tracing::instrument(level = "debug", skip_all, fields(%sku))]
pub async fn by_sku(db: &impl Executor, sku: &Sku) -> Result<Product, AppError> {
    let query = Product::query().filter(Product::SKU.eq(sku));
    let product = query.first(db).await.map_err(app_error)?;
    product.ok_or(AppError::NotFound("product"))
}

#[tracing::instrument(level = "debug", skip_all, fields(sku = %new.sku))]
pub async fn create(db: &impl Executor, new: &NewProduct) -> Result<Product, AppError> {
    new.insert(db).await.map_err(app_error)
}

/// Applies `patch` if the product is still at `expected_version` (`None`: whatever it is).
///
/// The route names the product by its SKU and shelf patches by key, so the row is read first;
/// the read also gives the version to patch at when the client named none. A shelf patch always
/// checks a version, so when another write lands between the read and the patch, the patch is
/// stale: with no version asked for, it is made again against the newer row. Each retry means
/// that someone else's write succeeded, so the loop always makes progress.
#[tracing::instrument(level = "debug", skip_all, fields(%sku))]
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
