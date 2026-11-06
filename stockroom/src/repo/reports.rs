//! Reports: read-only questions about the whole data set.

use jiff::civil::Date;
use rust_decimal::Decimal;
use serde::Serialize;
use tokio_postgres::{Client, SimpleQueryMessage};

use crate::AppError;
use crate::domain::Sku;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Dashboard {
    pub products: i64,
    pub stores: i64,
    pub open_orders: i64,
    pub orders_today: i64,
    pub revenue_today_cents: i64,
    /// `avg()` of a `bigint` is a `numeric`: `rust_decimal` maps it exactly (serialized as a
    /// string, so no client parses it into a float).
    pub average_order_cents: Option<Decimal>,
    pub low_stock: Vec<LowStock>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LowStock {
    pub store: String,
    pub sku: Sku,
    pub available: i32,
}

/// Five independent queries on one connection. Sent together (`try_join!` polls all five
/// before any answer is back), they cost one round trip instead of five: pipelining
/// (Chapter 7). The server still runs them one after another.
#[tracing::instrument(level = "debug", skip_all)]
pub async fn dashboard(client: &Client) -> Result<Dashboard, AppError> {
    let (products, stores, open_orders, today, low) = tokio::try_join!(
        client.query_one_scalar::<i64, _>("SELECT count(*) FROM products", &[]),
        client.query_one_scalar::<i64, _>("SELECT count(*) FROM stores", &[]),
        client.query_one_scalar::<i64, _>(
            "SELECT count(*) FROM orders WHERE status IN ('pending', 'paid', 'ready')",
            &[]
        ),
        client.query_one(
            "SELECT count(*) AS orders, coalesce(sum(total_cents), 0)::bigint AS revenue,
                    round(avg(total_cents), 2) AS average
             FROM orders
             WHERE created_at >= date_trunc('day', now()) AND status <> 'cancelled'",
            &[]
        ),
        client.query(
            "SELECT s.code AS store, p.sku, st.available
             FROM stock st JOIN stores s ON s.id = st.store_id JOIN products p ON p.id = st.product_id
             WHERE st.available < 5
             ORDER BY st.available, s.code, p.sku
             LIMIT 10",
            &[]
        ),
    )?;
    let low_stock = low
        .iter()
        .map(|row| {
            Ok(LowStock {
                store: row.try_get("store")?,
                sku: row.try_get("sku")?,
                available: row.try_get("available")?,
            })
        })
        .collect::<Result<_, tokio_postgres::Error>>()?;
    Ok(Dashboard {
        products,
        stores,
        open_orders,
        orders_today: today.try_get("orders")?,
        revenue_today_cents: today.try_get("revenue")?,
        average_order_cents: today.try_get("average")?,
        low_stock,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SalesDay {
    pub day: Date,
    pub orders: i64,
    pub revenue_cents: i64,
}

/// Sales per day (UTC) between `from` and `to`, both included. `delay_ms` adds a server-side
/// `pg_sleep` first: a stand-in for a genuinely slow report, to watch timeouts and
/// cancellation at work (Chapter 12).
#[tracing::instrument(level = "debug", skip_all)]
pub async fn sales(
    client: &Client,
    from: Date,
    to: Date,
    delay_ms: u32,
) -> Result<Vec<SalesDay>, AppError> {
    if delay_ms > 0 {
        client
            .execute("SELECT pg_sleep($1)", &[&(f64::from(delay_ms) / 1000.0)])
            .await?;
    }
    let rows = client
        .query(
            "SELECT (created_at AT TIME ZONE 'UTC')::date AS day, count(*) AS orders,
                    sum(total_cents)::bigint AS revenue
             FROM orders
             WHERE created_at >= $1::date AND created_at < $2::date + 1 AND status <> 'cancelled'
             GROUP BY 1
             ORDER BY 1",
            &[&from, &to],
        )
        .await?;
    rows.iter()
        .map(|row| {
            Ok(SalesDay {
                day: row.try_get("day")?,
                orders: row.try_get("orders")?,
                revenue_cents: row.try_get("revenue")?,
            })
        })
        .collect()
}

/// The sessions on this database, from `pg_stat_activity`, through the simple query
/// protocol (Chapter 2): no parameters, several statements allowed, and every value comes
/// back as text. That suits a diagnostic page that only displays them.
#[tracing::instrument(level = "debug", skip_all)]
pub async fn sessions(
    client: &Client,
) -> Result<Vec<serde_json::Map<String, serde_json::Value>>, AppError> {
    let messages = client
        .simple_query(
            "SELECT pid, application_name, backend_type, state,
                    wait_event_type || ':' || wait_event AS waiting_on,
                    date_trunc('second', now() - query_start) AS running_for,
                    left(query, 120) AS query
             FROM pg_stat_activity
             WHERE datname = current_database()
             ORDER BY pid",
        )
        .await?;
    let mut sessions = Vec::new();
    for message in messages {
        if let SimpleQueryMessage::Row(row) = message {
            let mut session = serde_json::Map::new();
            for (i, column) in row.columns().iter().enumerate() {
                // `Option<&str>`: NULL is `None`; everything else is text, whatever its SQL type.
                session.insert(column.name().to_owned(), row.get(i).into());
            }
            sessions.push(session);
        }
    }
    Ok(sessions)
}
