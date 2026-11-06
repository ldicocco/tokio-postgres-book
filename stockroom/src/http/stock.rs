//! `/stores/{code}/stock` and `/stock/transfers`

use axum::Json;
use axum::extract::Path;
use tokio_postgres::IsolationLevel;

use super::{AppJson, Db};
use crate::repo::stock::{self, Adjustment, StockLevel, Transfer, TransferReceipt};
use crate::repo::stores;
use crate::{AppError, db};

pub async fn levels(
    Db(db): Db,
    Path(code): Path<String>,
) -> Result<Json<Vec<StockLevel>>, AppError> {
    let store_id = stores::id_by_code(&db, &code).await?;
    Ok(Json(stock::for_store(&db, store_id).await?))
}

/// A plain transaction: begin, work, commit. If anything fails, `?` returns early and the
/// dropped transaction rolls back.
pub async fn adjust(
    Db(mut db): Db,
    Path(code): Path<String>,
    AppJson(adjustment): AppJson<Adjustment>,
) -> Result<Json<StockLevel>, AppError> {
    let tx = db.transaction().await?;
    let store_id = stores::id_by_code(&tx, &code).await?;
    let level = stock::adjust(&tx, store_id, &adjustment).await?;
    tx.commit().await?;
    Ok(Json(level))
}

/// A serializable transaction that is retried when it collides with another one.
pub async fn transfer(
    Db(mut db): Db,
    AppJson(transfer): AppJson<Transfer>,
) -> Result<Json<TransferReceipt>, AppError> {
    let receipt = db::in_transaction(&mut db, IsolationLevel::Serializable, async move |tx| {
        stock::transfer(tx, &transfer).await
    })
    .await?;
    Ok(Json(receipt))
}
