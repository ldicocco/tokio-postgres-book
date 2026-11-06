//! Stock per store: levels, adjustments and transfers between stores.

use deadpool_postgres::{GenericClient, Transaction};
use serde::{Deserialize, Serialize};
use tokio_postgres::Row;
use uuid::Uuid;

use super::{from_rows, products, stores};
use crate::AppError;
use crate::domain::{Quantity, Sku};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StockLevel {
    pub sku: Sku,
    pub on_hand: i32,
    pub reserved: i32,
    /// `on_hand - reserved`: the virtual generated column `stock.available` (PG 18).
    pub available: i32,
}

impl TryFrom<&Row> for StockLevel {
    type Error = tokio_postgres::Error;

    fn try_from(row: &Row) -> Result<Self, Self::Error> {
        Ok(StockLevel {
            sku: row.try_get("sku")?,
            on_hand: row.try_get("on_hand")?,
            reserved: row.try_get("reserved")?,
            available: row.try_get("available")?,
        })
    }
}

#[tracing::instrument(level = "debug", skip_all)]
pub async fn for_store(
    db: &impl GenericClient,
    store_id: Uuid,
) -> Result<Vec<StockLevel>, AppError> {
    let stmt = db
        .prepare_cached(
            "SELECT p.sku, s.on_hand, s.reserved, s.available
             FROM stock s JOIN products p ON p.id = s.product_id
             WHERE s.store_id = $1
             ORDER BY p.sku",
        )
        .await?;
    from_rows(&db.query(&stmt, &[&store_id]).await?)
}

#[derive(Debug, Clone, Deserialize)]
pub struct Adjustment {
    pub sku: Sku,
    /// Units received (positive) or written off (negative).
    pub delta: i32,
    pub reason: String,
}

/// Changes on-hand stock and records why in the ledger, in the caller's transaction.
/// The CHECK constraints refuse a result below zero or below what is reserved.
#[tracing::instrument(level = "debug", skip_all, fields(sku = %adjustment.sku, delta = adjustment.delta))]
pub async fn adjust(
    tx: &Transaction<'_>,
    store_id: Uuid,
    adjustment: &Adjustment,
) -> Result<StockLevel, AppError> {
    if adjustment.delta == 0 {
        return Err(AppError::invalid(
            "delta",
            "an adjustment must change something",
        ));
    }
    if adjustment.reason.trim().is_empty() {
        return Err(AppError::invalid("reason", "say why the stock changed"));
    }
    let product_id = products::id_by_sku(tx, &adjustment.sku).await?;
    let row = tx
        .query_one(
            "INSERT INTO stock (store_id, product_id, on_hand) VALUES ($1, $2, $3)
             ON CONFLICT (store_id, product_id) DO UPDATE SET on_hand = stock.on_hand + EXCLUDED.on_hand
             RETURNING on_hand, reserved, available",
            &[&store_id, &product_id, &adjustment.delta],
        )
        .await?;
    tx.execute(
        "INSERT INTO stock_movements (store_id, product_id, delta, reason) VALUES ($1, $2, $3, $4)",
        &[
            &store_id,
            &product_id,
            &adjustment.delta,
            &adjustment.reason,
        ],
    )
    .await?;
    Ok(StockLevel {
        sku: adjustment.sku.clone(),
        on_hand: row.try_get("on_hand")?,
        reserved: row.try_get("reserved")?,
        available: row.try_get("available")?,
    })
}

#[derive(Debug, Clone, Deserialize)]
pub struct Transfer {
    pub from: String,
    pub to: String,
    pub sku: Sku,
    pub quantity: Quantity,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TransferReceipt {
    pub from: StockLevel,
    pub to: StockLevel,
}

/// Moves available units from one store to another.
///
/// It reads (is enough available?) and then writes, and what it read must still hold when it
/// writes: the classic case for `SERIALIZABLE`. The caller runs it through
/// `db::in_transaction(…, IsolationLevel::Serializable, …)`, so when two transfers collide,
/// PostgreSQL aborts one with a serialization failure and the helper runs it again.
#[tracing::instrument(level = "debug", skip_all, fields(sku = %transfer.sku))]
pub async fn transfer(
    tx: &Transaction<'_>,
    transfer: &Transfer,
) -> Result<TransferReceipt, AppError> {
    if transfer.from == transfer.to {
        return Err(AppError::invalid(
            "to",
            "a transfer needs two different stores",
        ));
    }
    // Three independent lookups, pipelined on the transaction's connection.
    let (from_id, to_id, product_id) = tokio::try_join!(
        stores::id_by_code(tx, &transfer.from),
        stores::id_by_code(tx, &transfer.to),
        products::id_by_sku(tx, &transfer.sku),
    )?;
    let units = transfer.quantity.get();

    // Lock both rows first, always in the same order. Otherwise a transfer A→B and one B→A
    // each lock one row and wait for the other's: a deadlock, which PostgreSQL only notices
    // after `deadlock_timeout` (a second). In this order the second transaction just waits,
    // and when the first commits, SERIALIZABLE fails it at once ("could not serialize access
    // due to concurrent update") for `in_transaction` to retry.
    tx.execute(
        "SELECT FROM stock WHERE product_id = $1 AND store_id IN ($2, $3) ORDER BY store_id FOR UPDATE",
        &[&product_id, &from_id, &to_id],
    )
    .await?;

    let available: i32 = tx
        .query_opt(
            "SELECT available FROM stock WHERE store_id = $1 AND product_id = $2",
            &[&from_id, &product_id],
        )
        .await?
        .map_or(0, |row| row.get(0));
    if available < units {
        return Err(AppError::InsufficientStock {
            sku: transfer.sku.clone(),
            requested: units,
            available,
        });
    }

    let level = |row: Row| -> Result<StockLevel, AppError> {
        Ok(StockLevel {
            sku: transfer.sku.clone(),
            on_hand: row.try_get("on_hand")?,
            reserved: row.try_get("reserved")?,
            available: row.try_get("available")?,
        })
    };
    let from = tx
        .query_one(
            "UPDATE stock SET on_hand = on_hand - $3 WHERE store_id = $1 AND product_id = $2
             RETURNING on_hand, reserved, available",
            &[&from_id, &product_id, &units],
        )
        .await?;
    let to = tx
        .query_one(
            "INSERT INTO stock (store_id, product_id, on_hand) VALUES ($1, $2, $3)
             ON CONFLICT (store_id, product_id) DO UPDATE SET on_hand = stock.on_hand + EXCLUDED.on_hand
             RETURNING on_hand, reserved, available",
            &[&to_id, &product_id, &units],
        )
        .await?;
    tx.execute(
        "INSERT INTO stock_movements (store_id, product_id, delta, reason)
         VALUES ($1, $3, $4, 'transfer to ' || $6), ($2, $3, $5, 'transfer from ' || $7)",
        &[
            &from_id,
            &to_id,
            &product_id,
            &-units,
            &units,
            &transfer.to,
            &transfer.from,
        ],
    )
    .await?;
    Ok(TransferReceipt {
        from: level(from)?,
        to: level(to)?,
    })
}
