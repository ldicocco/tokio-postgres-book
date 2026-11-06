//! Products: the catalog. Keyset pagination, optimistic concurrency, and CSV in and out
//! through COPY.

use std::fmt::Display;
use std::pin::pin;

use bytes::Bytes;
use deadpool_postgres::{GenericClient, Transaction};
use futures_util::{SinkExt, Stream, StreamExt};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_postgres::Row;
use uuid::Uuid;

use super::from_rows;
use crate::AppError;
use crate::domain::{Cents, Sku};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Product {
    pub id: Uuid,
    pub sku: Sku,
    pub name: String,
    pub price_cents: Cents,
    pub tags: Vec<String>,
    pub attributes: Value,
    pub version: i32,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

impl TryFrom<&Row> for Product {
    type Error = tokio_postgres::Error;

    fn try_from(row: &Row) -> Result<Self, Self::Error> {
        Ok(Product {
            id: row.try_get("id")?,
            sku: row.try_get("sku")?,
            name: row.try_get("name")?,
            price_cents: row.try_get("price_cents")?,
            tags: row.try_get("tags")?,
            attributes: row.try_get("attributes")?,
            version: row.try_get("version")?,
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
        })
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
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
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProductPatch {
    pub name: Option<String>,
    pub price_cents: Option<Cents>,
    pub tags: Option<Vec<String>>,
    pub attributes: Option<Value>,
}

/// One page of products in SKU order, starting after `after` (keyset pagination: the cost of
/// a page does not grow with how deep it is, unlike `OFFSET`).
#[tracing::instrument(level = "debug", skip_all)]
pub async fn page(
    db: &impl GenericClient,
    after: Option<&Sku>,
    tag: Option<&str>,
    limit: i64,
) -> Result<Vec<Product>, AppError> {
    let stmt = db
        .prepare_cached(
            "SELECT id, sku, name, price_cents, tags, attributes, version, created_at, updated_at
             FROM products
             WHERE sku > coalesce($1, '') AND ($2::text IS NULL OR tags @> ARRAY[$2])
             ORDER BY sku
             LIMIT $3",
        )
        .await?;
    from_rows(&db.query(&stmt, &[&after, &tag, &limit]).await?)
}

#[tracing::instrument(level = "debug", skip_all, fields(%sku))]
pub async fn by_sku(db: &impl GenericClient, sku: &Sku) -> Result<Product, AppError> {
    let stmt = db
        .prepare_cached(
            "SELECT id, sku, name, price_cents, tags, attributes, version, created_at, updated_at
             FROM products WHERE sku = $1",
        )
        .await?;
    match db.query_opt(&stmt, &[sku]).await? {
        Some(row) => Ok(Product::try_from(&row)?),
        None => Err(AppError::NotFound("product")),
    }
}

#[tracing::instrument(level = "debug", skip_all, fields(%sku))]
pub async fn id_by_sku(db: &impl GenericClient, sku: &Sku) -> Result<Uuid, AppError> {
    let stmt = db
        .prepare_cached("SELECT id FROM products WHERE sku = $1")
        .await?;
    match db.query_opt(&stmt, &[sku]).await? {
        Some(row) => Ok(row.try_get(0)?),
        None => Err(AppError::NotFound("product")),
    }
}

#[tracing::instrument(level = "debug", skip_all, fields(sku = %new.sku))]
pub async fn create(db: &impl GenericClient, new: &NewProduct) -> Result<Product, AppError> {
    // `$1` targets the `sku` column, so its type is the domain `sku`: `Sku`'s hand-written
    // `accepts` takes it, where a plain `&str` would be refused.
    let stmt = db
        .prepare_cached(
            "INSERT INTO products (sku, name, price_cents, tags, attributes)
             VALUES ($1, $2, $3, $4, $5)
             RETURNING id, sku, name, price_cents, tags, attributes, version, created_at, updated_at",
        )
        .await?;
    let row = db
        .query_one(
            &stmt,
            &[
                &new.sku,
                &new.name,
                &new.price_cents,
                &new.tags,
                &new.attributes,
            ],
        )
        .await?;
    Ok(Product::try_from(&row)?)
}

/// Applies `patch` if the product is still at `expected_version` (`None`: whatever it is).
#[tracing::instrument(level = "debug", skip_all, fields(%sku))]
pub async fn update(
    db: &impl GenericClient,
    sku: &Sku,
    expected_version: Option<i32>,
    patch: &ProductPatch,
) -> Result<Product, AppError> {
    let stmt = db
        .prepare_cached(
            "UPDATE products SET
                 name        = coalesce($3, name),
                 price_cents = coalesce($4, price_cents),
                 tags        = coalesce($5, tags),
                 attributes  = coalesce($6, attributes),
                 version     = version + 1,
                 updated_at  = now()
             WHERE sku = $1 AND version = coalesce($2, version)
             RETURNING id, sku, name, price_cents, tags, attributes, version, created_at, updated_at",
        )
        .await?;
    let row = db
        .query_opt(
            &stmt,
            &[
                sku,
                &expected_version,
                &patch.name,
                &patch.price_cents,
                &patch.tags,
                &patch.attributes,
            ],
        )
        .await?;
    if let Some(row) = row {
        return Ok(Product::try_from(&row)?);
    }
    // Nothing updated: either there is no such product, or someone else changed it first.
    let stmt = db
        .prepare_cached("SELECT version FROM products WHERE sku = $1")
        .await?;
    let current: Option<i32> = db.query_opt(&stmt, &[sku]).await?.map(|row| row.get(0));
    Err(match current {
        None => AppError::NotFound("product"),
        Some(current) => AppError::StaleVersion { current },
    })
}

/// Every product, as CSV, for `copy_out` (Chapter 10). In CSV, COPY writes NULL as nothing
/// and an empty string as `""`; `nullif` makes "no tags" the former, which reads back the same.
pub const EXPORT_CSV: &str = "COPY (
         SELECT sku, name, price_cents, nullif(array_to_string(tags, '|'), '') AS tags
         FROM products ORDER BY sku
     ) TO STDOUT (FORMAT csv, HEADER)";

/// Every product, for `query_raw` (Chapter 9).
pub const ALL_IN_SKU_ORDER: &str =
    "SELECT id, sku, name, price_cents, tags, attributes, version, created_at, updated_at
     FROM products ORDER BY sku";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ImportReport {
    pub rows: u64,
    pub inserted: u64,
    pub updated: u64,
    pub unchanged: u64,
}

/// Loads a CSV price list (`sku,name,price_cents,tags`, tags separated by `|`) into the
/// catalog: new SKUs are inserted, changed ones updated, the rest left alone.
///
/// The CSV is streamed into a temporary staging table with COPY, so a malformed file never
/// touches `products`, and then merged with one `MERGE` (PG 15+). All in the caller's
/// transaction: the whole file goes in, or none of it.
#[tracing::instrument(level = "debug", skip_all)]
pub async fn import_csv<S, E>(tx: &Transaction<'_>, chunks: S) -> Result<ImportReport, AppError>
where
    S: Stream<Item = Result<Bytes, E>>,
    E: Display,
{
    tx.batch_execute(
        "CREATE TEMP TABLE product_import (sku text, name text, price_cents bigint, tags text)
         ON COMMIT DROP",
    )
    .await?;

    // HEADER MATCH (PG 15+): the first line must name exactly these columns.
    let sink = tx
        .copy_in("COPY product_import (sku, name, price_cents, tags) FROM STDIN (FORMAT csv, HEADER MATCH)")
        .await?;
    let mut sink = pin!(sink);
    let mut chunks = pin!(chunks);
    while let Some(chunk) = chunks.next().await {
        let chunk = chunk.map_err(|e| AppError::Invalid {
            field: None,
            message: format!("could not read the upload: {e}"),
        })?;
        sink.send(chunk).await.map_err(input_error)?;
    }
    // Without `finish`, dropping the sink would abort the COPY.
    let rows = sink.as_mut().finish().await.map_err(input_error)?;

    // A SKU listed twice would make MERGE insert it twice (a unique violation) or update it
    // twice (a cardinality violation). Naming the SKU is kinder than either.
    let twice = tx
        .query_opt(
            "SELECT sku FROM product_import GROUP BY sku HAVING count(*) > 1 LIMIT 1",
            &[],
        )
        .await?;
    if let Some(row) = twice {
        let sku: &str = row.get(0);
        return Err(AppError::invalid(
            "sku",
            format!("the file lists {sku} more than once"),
        ));
    }

    let actions = tx
        .query(
            "MERGE INTO products AS p
             USING (
                 SELECT sku::sku AS sku, name, price_cents,
                        coalesce(string_to_array(nullif(tags, ''), '|'), '{}') AS tags
                 FROM product_import
             ) AS i
             ON p.sku = i.sku
             WHEN MATCHED AND (p.name, p.price_cents, p.tags) IS DISTINCT FROM (i.name, i.price_cents, i.tags) THEN
                 UPDATE SET name = i.name, price_cents = i.price_cents, tags = i.tags,
                            version = p.version + 1, updated_at = now()
             WHEN NOT MATCHED THEN
                 INSERT (sku, name, price_cents, tags) VALUES (i.sku, i.name, i.price_cents, i.tags)
             RETURNING merge_action()",
            &[],
        )
        .await
        .map_err(input_error)?;
    let count = |action: &str| {
        actions
            .iter()
            .filter(|row| row.get::<_, &str>(0) == action)
            .count() as u64
    };
    let (inserted, updated) = (count("INSERT"), count("UPDATE"));
    Ok(ImportReport {
        rows,
        inserted,
        updated,
        unchanged: rows - inserted - updated,
    })
}

/// Errors caused by the uploaded file are the client's to fix: report them, with the line the
/// server names. (Everywhere else a `DbError`'s text stays in the log; this one only quotes
/// the client's own data back.)
fn input_error(error: tokio_postgres::Error) -> AppError {
    // Class 22, data exception: a bad number, a malformed line, a wrong header.
    if let Some(db) = error.as_db_error()
        && db.code().code().starts_with("22")
    {
        let at = db.where_().map(|w| format!(" ({w})")).unwrap_or_default();
        return AppError::Invalid {
            field: None,
            message: format!("{}{at}", db.message()),
        };
    }
    error.into()
}
