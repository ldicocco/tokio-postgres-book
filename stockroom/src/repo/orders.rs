//! Orders: placing one (reserving stock without overselling) and moving it through its states.

use std::collections::BTreeMap;

use deadpool_postgres::{GenericClient, Transaction};
use futures_util::future;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use tokio_postgres::types::{Json, ToSql};
use uuid::Uuid;

use super::stores;
use crate::AppError;
use crate::domain::{Cents, Customer, OrderStatus, Quantity, Sku};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Order {
    pub id: Uuid,
    pub store: String,
    pub status: OrderStatus,
    pub customer: Customer,
    pub total_cents: Cents,
    pub version: i32,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub lines: Vec<OrderLine>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrderLine {
    pub sku: Sku,
    pub quantity: Quantity,
    pub unit_price_cents: Cents,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NewOrder {
    /// The store's code: where the order will be collected.
    pub store: String,
    pub customer: Customer,
    pub lines: Vec<NewLine>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NewLine {
    pub sku: Sku,
    pub quantity: Quantity,
}

/// One order line, resolved against the catalog.
struct Line<'a> {
    product_id: Uuid,
    sku: &'a Sku,
    quantity: i32,
    unit_price: Cents,
}

/// Places an order: validates it, prices it from the catalog, reserves its stock and stores
/// it, all in the caller's transaction.
///
/// Reserving is one guarded `UPDATE` per line: `SET reserved = reserved + n WHERE available
/// >= n`. It is atomic on its own, at READ COMMITTED: if another order holds the row, this
/// one waits, and then re-checks the condition against the row as the other left it. No
/// lost update, no oversell, and no retries needed, unless two orders lock the same rows in
/// opposite orders and deadlock; locking in product-id order rules that out, and
/// `db::in_transaction` would retry a deadlock anyway.
#[tracing::instrument(level = "debug", skip_all, fields(store = %order.store))]
pub async fn place(tx: &Transaction<'_>, order: &NewOrder) -> Result<Order, AppError> {
    order
        .customer
        .validate()
        .map_err(|message| AppError::invalid("customer", message))?;
    if order.lines.is_empty() || order.lines.len() > 100 {
        return Err(AppError::invalid("lines", "an order has 1–100 lines"));
    }
    // One line per SKU: quantities of a repeated SKU add up.
    let mut wanted: BTreeMap<&Sku, i32> = BTreeMap::new();
    for line in &order.lines {
        *wanted.entry(&line.sku).or_default() += line.quantity.get();
    }
    if let Some((sku, _)) = wanted.iter().find(|(_, units)| **units > Quantity::MAX) {
        return Err(AppError::invalid(
            "lines",
            format!("too many units of {sku} in one order"),
        ));
    }
    // `sku = ANY($1)` compares with the domain's base type, so `$1` is `text[]`.
    let skus: Vec<&str> = wanted.keys().map(|sku| sku.as_str()).collect();

    let (store_id, rows) = tokio::try_join!(stores::id_by_code(tx, &order.store), async {
        let stmt = tx
            .prepare_cached("SELECT id, sku, price_cents FROM products WHERE sku = ANY($1)")
            .await?;
        Ok::<_, AppError>(tx.query(&stmt, &[&skus]).await?)
    })?;

    let mut lines = Vec::with_capacity(wanted.len());
    for (sku, &quantity) in &wanted {
        let Some(row) = rows
            .iter()
            .find(|row| row.get::<_, &str>("sku") == sku.as_str())
        else {
            return Err(AppError::invalid(
                "lines",
                format!("there is no product with SKU {sku}"),
            ));
        };
        lines.push(Line {
            product_id: row.try_get("id")?,
            sku,
            quantity,
            unit_price: row.try_get("price_cents")?,
        });
    }
    // The same lock order in every transaction: orders sharing products queue, never deadlock.
    lines.sort_by_key(|line| line.product_id);

    let total = lines
        .iter()
        .try_fold(Cents(0), |sum, line| {
            sum.checked_add(line.unit_price.times(line.quantity)?)
        })
        .ok_or_else(|| AppError::invalid("lines", "the order total is too large"))?;

    // Every reservation at once: pipelined, sent in this order, run one by one by the server.
    let reserve = tx
        .prepare_cached(
            "UPDATE stock SET reserved = reserved + $3
             WHERE store_id = $1 AND product_id = $2 AND available >= $3",
        )
        .await?;
    let (reserve, store_id) = (&reserve, &store_id);
    let reserved = future::try_join_all(lines.iter().map(|l| async move {
        // The parameter slice must live as long as the future that uses it: inside the
        // `async` block, not as a temporary in the closure.
        tx.execute(reserve, &[store_id, &l.product_id, &l.quantity])
            .await
    }))
    .await?;
    if let Some(short) = reserved.iter().position(|&updated| updated == 0) {
        let line = &lines[short];
        let available: i32 = tx
            .query_opt(
                "SELECT available FROM stock WHERE store_id = $1 AND product_id = $2",
                &[store_id, &line.product_id],
            )
            .await?
            .map_or(0, |row| row.get(0));
        return Err(AppError::InsufficientStock {
            sku: line.sku.clone(),
            requested: line.quantity,
            available,
        });
    }

    let id: Uuid = tx
        .query_one(
            "INSERT INTO orders (store_id, customer, total_cents) VALUES ($1, $2, $3) RETURNING id",
            &[store_id, &Json(&order.customer), &total],
        )
        .await?
        .try_get(0)?;
    // All lines in one statement: three arrays, zipped back into rows by unnest.
    let product_ids: Vec<Uuid> = lines.iter().map(|l| l.product_id).collect();
    let quantities: Vec<i32> = lines.iter().map(|l| l.quantity).collect();
    let prices: Vec<Cents> = lines.iter().map(|l| l.unit_price).collect();
    tx.execute(
        "INSERT INTO order_lines (order_id, product_id, quantity, unit_price_cents)
         SELECT $1::uuid, * FROM unnest($2::uuid[], $3::integer[], $4::bigint[])",
        &[&id, &product_ids, &quantities, &prices],
    )
    .await?;
    get(tx, id).await
}

#[tracing::instrument(level = "debug", skip_all, fields(%id))]
pub async fn get(db: &impl GenericClient, id: Uuid) -> Result<Order, AppError> {
    let head = db
        .prepare_cached(
            "SELECT o.id, s.code AS store, o.status, o.customer, o.total_cents, o.version,
                    o.created_at, o.updated_at
             FROM orders o JOIN stores s ON s.id = o.store_id
             WHERE o.id = $1",
        )
        .await?;
    let lines = db
        .prepare_cached(
            "SELECT p.sku, l.quantity, l.unit_price_cents
             FROM order_lines l JOIN products p ON p.id = l.product_id
             WHERE l.order_id = $1
             ORDER BY p.sku",
        )
        .await?;
    // Two queries, one round trip: pipelined. (The parameter slice is bound to a name
    // because both futures borrow it until they finish.)
    let params: &[&(dyn ToSql + Sync)] = &[&id];
    let (head, lines) = tokio::try_join!(db.query_opt(&head, params), db.query(&lines, params))?;
    let Some(head) = head else {
        return Err(AppError::NotFound("order"));
    };
    let lines = lines
        .iter()
        .map(|row| {
            Ok(OrderLine {
                sku: row.try_get("sku")?,
                // A domain column comes back as its base type, `integer`; the derived
                // `Quantity` accepts that too.
                quantity: row.try_get("quantity")?,
                unit_price_cents: row.try_get("unit_price_cents")?,
            })
        })
        .collect::<Result<Vec<_>, tokio_postgres::Error>>()?;
    let Json(customer): Json<Customer> = head.try_get("customer")?;
    Ok(Order {
        id: head.try_get("id")?,
        store: head.try_get("store")?,
        status: head.try_get("status")?,
        customer,
        total_cents: head.try_get("total_cents")?,
        version: head.try_get("version")?,
        created_at: head.try_get("created_at")?,
        updated_at: head.try_get("updated_at")?,
        lines,
    })
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Transitioned {
    pub previous: OrderStatus,
    pub order: Order,
}

/// Moves an order to `to`, if its state machine allows it and (when `expected_version` is
/// given) nobody changed it in the meantime; then applies what the move means for stock.
#[tracing::instrument(level = "debug", skip_all, fields(%id, %to))]
pub async fn transition(
    tx: &Transaction<'_>,
    id: Uuid,
    to: OrderStatus,
    expected_version: Option<i32>,
) -> Result<Transitioned, AppError> {
    // One statement checks the state machine and the version and makes the change.
    // `RETURNING old.status` (PG 18) reports what it was, without a separate read.
    let moved = tx
        .query_opt(
            "UPDATE orders SET status = $2, version = version + 1, updated_at = now()
             WHERE id = $1 AND status = ANY($3) AND version = coalesce($4, version)
             RETURNING old.status AS previous, store_id",
            &[
                &id,
                &to,
                &OrderStatus::allowed_before(to),
                &expected_version,
            ],
        )
        .await?;
    let Some(moved) = moved else {
        return Err(why_not(tx, id, to, expected_version).await?);
    };
    let previous: OrderStatus = moved.try_get("previous")?;
    let store_id: Uuid = moved.try_get("store_id")?;

    match to {
        OrderStatus::Cancelled => {
            // Release the reservation.
            tx.execute(
                "UPDATE stock s SET reserved = s.reserved - l.quantity
                 FROM order_lines l
                 WHERE l.order_id = $1 AND s.store_id = $2 AND s.product_id = l.product_id",
                &[&id, &store_id],
            )
            .await?;
        }
        OrderStatus::Collected => {
            // The goods leave the store: out of stock and out of the reservation, into the ledger.
            tx.execute(
                "UPDATE stock s SET on_hand = s.on_hand - l.quantity, reserved = s.reserved - l.quantity
                 FROM order_lines l
                 WHERE l.order_id = $1 AND s.store_id = $2 AND s.product_id = l.product_id",
                &[&id, &store_id],
            )
            .await?;
            tx.execute(
                "INSERT INTO stock_movements (store_id, product_id, delta, reason)
                 SELECT $2, l.product_id, -l.quantity, 'collected: order ' || l.order_id
                 FROM order_lines l WHERE l.order_id = $1",
                &[&id, &store_id],
            )
            .await?;
        }
        OrderStatus::Pending | OrderStatus::Paid | OrderStatus::Ready => {}
    }
    Ok(Transitioned {
        previous,
        order: get(tx, id).await?,
    })
}

/// Why `transition` changed nothing: no such order, a stale version, or a forbidden move.
async fn why_not(
    tx: &Transaction<'_>,
    id: Uuid,
    to: OrderStatus,
    expected_version: Option<i32>,
) -> Result<AppError, AppError> {
    let row = tx
        .query_opt("SELECT status, version FROM orders WHERE id = $1", &[&id])
        .await?;
    let Some(row) = row else {
        return Ok(AppError::NotFound("order"));
    };
    let (status, version): (OrderStatus, i32) = (row.try_get("status")?, row.try_get("version")?);
    Ok(match expected_version {
        Some(expected) if expected != version => AppError::StaleVersion { current: version },
        _ => AppError::Transition { from: status, to },
    })
}
