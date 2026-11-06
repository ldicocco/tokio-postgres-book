//! The outbox: jobs written in the same transaction as the change they report (Chapter 16).

use std::time::Duration;

use deadpool_postgres::{GenericClient, Transaction};
use serde_json::Value;

use crate::AppError;

#[derive(Debug, Clone)]
pub struct Job {
    pub id: i64,
    pub topic: String,
    pub payload: Value,
    pub attempts: i32,
}

/// Claims up to `limit` due jobs for the caller's transaction.
///
/// `FOR UPDATE SKIP LOCKED` is what lets several workers share the table: each row a worker
/// claims is locked until its transaction ends, and the other workers skip locked rows
/// instead of waiting for them. A worker that dies mid-batch releases its rows with its
/// connection, and another one picks them up: at-least-once delivery.
pub async fn claim(tx: &Transaction<'_>, limit: i64) -> Result<Vec<Job>, AppError> {
    let stmt = tx
        .prepare_cached(
            "SELECT id, topic, payload, attempts FROM outbox
             WHERE done_at IS NULL AND available_at <= now()
             ORDER BY id
             LIMIT $1
             FOR UPDATE SKIP LOCKED",
        )
        .await?;
    tx.query(&stmt, &[&limit])
        .await?
        .iter()
        .map(|row| {
            Ok(Job {
                id: row.try_get("id")?,
                topic: row.try_get("topic")?,
                payload: row.try_get("payload")?,
                attempts: row.try_get("attempts")?,
            })
        })
        .collect()
}

pub async fn done(tx: &Transaction<'_>, id: i64) -> Result<(), AppError> {
    tx.execute(
        "UPDATE outbox SET done_at = now(), attempts = attempts + 1, last_error = NULL WHERE id = $1",
        &[&id],
    )
    .await?;
    Ok(())
}

pub async fn failed(
    tx: &Transaction<'_>,
    id: i64,
    error: &str,
    retry_in: Duration,
) -> Result<(), AppError> {
    tx.execute(
        "UPDATE outbox SET attempts = attempts + 1, last_error = $2,
                           available_at = now() + make_interval(secs => $3)
         WHERE id = $1",
        &[&id, &error, &retry_in.as_secs_f64()],
    )
    .await?;
    Ok(())
}

/// How many jobs are waiting.
pub async fn pending(db: &impl GenericClient) -> Result<i64, AppError> {
    let row = db
        .query_one("SELECT count(*) FROM outbox WHERE done_at IS NULL", &[])
        .await?;
    Ok(row.try_get(0)?)
}
