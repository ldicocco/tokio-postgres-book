//! Orders, on shelf: placing one and reading it back, from stockroom's `repo/orders.rs`.
//!
//! The request and response types are stockroom's own ([`NewOrder`], [`Order`]): an order as
//! the API shows it names its store by code and each line's product by SKU, which a row of
//! `orders` or `order_lines` does not hold. So the tables have models of their own here,
//! [`OrderRow`] and [`LineRow`], and [`get`] assembles the response from them.
//!
//! Where shelf steps aside: the reservation, `UPDATE stock … WHERE available >= $3`, keeps its
//! SQL, since its `WHERE` clause is the rule that no order oversells. Moving an order through
//! its states stays stockroom's (`transition`): one `UPDATE` checks the state machine and the
//! version, and `RETURNING old.status` reports what it was.

use std::collections::{BTreeMap, HashMap};

use deadpool_postgres::Transaction;
use futures_util::future;
use jiff::Timestamp;
use shelf::{Executor, FromRow, Insert, Table};
use stockroom::AppError;
use stockroom::domain::{Cents, Customer, OrderStatus, Quantity, Sku};
use stockroom::repo::orders::{NewOrder, Order, OrderLine};
use uuid::Uuid;

use super::products::Product;
use super::stock::Stock;
use super::stores::{self, Store};
use crate::app_error;

#[derive(Debug, Clone, FromRow, Table)]
#[shelf(table = "orders", belongs_to(Store, key = store_id))]
#[shelf(has_many(LineRow, key = order_id, as = lines))]
pub struct OrderRow {
    #[shelf(primary_key, generated)]
    pub id: Uuid,
    pub store_id: Uuid,
    pub status: OrderStatus,
    #[shelf(json)]
    pub customer: Customer,
    pub total_cents: Cents,
    #[shelf(version)]
    pub version: i32,
    #[shelf(generated)]
    pub created_at: Timestamp,
    #[shelf(updated_at)]
    pub updated_at: Timestamp,
}

/// A new order's row; the status, the version and the times are the table's defaults.
#[derive(Debug, Insert)]
#[shelf(model = OrderRow)]
pub struct NewOrderRow<'a> {
    pub store_id: Uuid,
    #[shelf(json)]
    pub customer: &'a Customer,
    pub total_cents: Cents,
}

#[derive(Debug, Clone, FromRow, Table)]
#[shelf(table = "order_lines", belongs_to(Product, key = product_id))]
pub struct LineRow {
    #[shelf(primary_key)]
    pub order_id: Uuid,
    #[shelf(primary_key)]
    pub product_id: Uuid,
    pub quantity: Quantity,
    pub unit_price_cents: Cents,
}

#[derive(Debug, Insert)]
#[shelf(model = LineRow)]
pub struct NewLineRow {
    pub order_id: Uuid,
    pub product_id: Uuid,
    /// The domain `positive_quantity`: `insert_many` sends an array of it.
    pub quantity: Quantity,
    pub unit_price_cents: Cents,
}

/// The reservation, as in stockroom: atomic at READ COMMITTED, and refused when it would take
/// more than is available.
const RESERVE: &str = "UPDATE stock SET reserved = reserved + $3
     WHERE store_id = $1 AND product_id = $2 AND available >= $3";

/// Places an order: validates it, prices it from the catalog, reserves its stock and stores
/// it, all in the caller's transaction. stockroom's `place`, with the reads and the inserts
/// through shelf and the reservation as SQL; see its documentation for why the reservation
/// cannot oversell and why the rows are locked in product-id order.
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
    let mut units: BTreeMap<&Sku, i32> = BTreeMap::new();
    for line in &order.lines {
        *units.entry(&line.sku).or_default() += line.quantity.get();
    }
    let wanted = units
        .into_iter()
        .map(|(sku, units)| match Quantity::new(units) {
            Ok(quantity) => Ok((sku, quantity)),
            Err(_) => Err(AppError::invalid(
                "lines",
                format!("too many units of {sku} in one order"),
            )),
        })
        .collect::<Result<BTreeMap<&Sku, Quantity>, AppError>>()?;

    let skus = wanted.keys().copied();
    let (store, products) = tokio::try_join!(stores::by_code(tx, &order.store), async {
        let query = Product::query().filter(Product::SKU.any(skus));
        query.all(tx).await.map_err(app_error)
    })?;

    let mut lines = Vec::with_capacity(wanted.len());
    for (sku, &quantity) in &wanted {
        let Some(product) = products.iter().find(|p| &p.sku == *sku) else {
            return Err(AppError::invalid(
                "lines",
                format!("there is no product with SKU {sku}"),
            ));
        };
        lines.push((product, quantity));
    }
    // The same lock order in every transaction: orders sharing products queue, never deadlock.
    lines.sort_by_key(|(product, _)| product.id);

    let total = lines
        .iter()
        .try_fold(Cents(0), |sum, (product, quantity)| {
            sum.checked_add(product.price_cents.times(quantity.get())?)
        })
        .ok_or_else(|| AppError::invalid("lines", "the order total is too large"))?;

    // Every reservation at once: pipelined, sent in this order, run one by one by the server.
    let reserve = tx.prepare_cached(RESERVE).await?;
    let (reserve, store_id) = (&reserve, &store.id);
    let reserved = future::try_join_all(lines.iter().map(|(product, quantity)| async move {
        let units = quantity.get();
        tx.execute(reserve, &[store_id, &product.id, &units]).await
    }))
    .await?;
    if let Some(short) = reserved.iter().position(|&updated| updated == 0) {
        let (product, quantity) = lines[short];
        let stock = Stock::find_opt(tx, &(store.id, product.id))
            .await
            .map_err(app_error)?;
        return Err(AppError::InsufficientStock {
            sku: product.sku.clone(),
            requested: quantity.get(),
            available: stock.and_then(|s| s.available).unwrap_or(0),
        });
    }

    let head = NewOrderRow {
        store_id: store.id,
        customer: &order.customer,
        total_cents: total,
    }
    .insert(tx)
    .await
    .map_err(app_error)?;
    let rows: Vec<NewLineRow> = lines
        .iter()
        .map(|(product, quantity)| NewLineRow {
            order_id: head.id,
            product_id: product.id,
            quantity: *quantity,
            unit_price_cents: product.price_cents,
        })
        .collect();
    // All lines in one statement: one array per column, zipped back into rows by unnest.
    let rows = NewLineRow::insert_many(tx, &rows)
        .await
        .map_err(app_error)?;
    let skus = products.iter().map(|p| (p.id, &p.sku)).collect();
    view(head, &store, rows, &skus)
}

/// An order with its lines. The order and its lines are read at once (pipelined), then their
/// store and products at once: two round trips, where stockroom's two joins take one.
#[tracing::instrument(level = "debug", skip_all, fields(%id))]
pub async fn get(db: &impl Executor, id: Uuid) -> Result<Order, AppError> {
    let lines = LineRow::query().filter(LineRow::ORDER_ID.eq(id));
    let (head, lines) =
        tokio::try_join!(OrderRow::find(db, &id), lines.all(db)).map_err(app_error)?;
    let (store, products) =
        tokio::try_join!(head.store(db), LineRow::load_product(db, &lines)).map_err(app_error)?;
    let skus = products.iter().map(|(id, p)| (*id, &p.sku)).collect();
    view(head, &store, lines, &skus)
}

/// The order as the API shows it: its store's code, and each line's SKU, in SKU order.
fn view(
    head: OrderRow,
    store: &Store,
    rows: Vec<LineRow>,
    skus: &HashMap<Uuid, &Sku>,
) -> Result<Order, AppError> {
    let mut lines = Vec::with_capacity(rows.len());
    for row in rows {
        let Some(sku) = skus.get(&row.product_id) else {
            return Err(AppError::Internal(format!(
                "order {} has a line for product {}, which is not loaded",
                head.id, row.product_id
            )));
        };
        lines.push(OrderLine {
            sku: (*sku).clone(),
            quantity: row.quantity,
            unit_price_cents: row.unit_price_cents,
        });
    }
    lines.sort_by(|a, b| a.sku.cmp(&b.sku));
    Ok(Order {
        id: head.id,
        store: store.code.clone(),
        status: head.status,
        customer: head.customer,
        total_cents: head.total_cents,
        version: head.version,
        created_at: head.created_at,
        updated_at: head.updated_at,
        lines,
    })
}
