//! The outbox worker: delivers the jobs the order trigger queues (Chapter 16).

use std::future::Future;
use std::time::Duration;

use deadpool_postgres::Pool;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::AppError;
use crate::notify::Hub;
use crate::repo;

/// How many jobs one transaction claims.
pub const BATCH: i64 = 20;

/// Woken by `NOTIFY outbox`; this is only the safety net for a missed wake-up.
pub const POLL_INTERVAL: Duration = Duration::from_secs(30);

/// Where a job goes: an email service, a webhook, a message broker.
pub trait Deliver: Send + Sync + 'static {
    fn deliver(
        &self,
        topic: &str,
        payload: &Value,
    ) -> impl Future<Output = Result<(), String>> + Send;
}

/// Logs each job: stockroom's stand-in for sending a "your order is ready" email.
pub struct LogDelivery;

impl Deliver for LogDelivery {
    async fn deliver(&self, topic: &str, payload: &Value) -> Result<(), String> {
        tracing::info!(topic, %payload, "delivered");
        Ok(())
    }
}

pub async fn run(pool: Pool, hub: Hub, deliver: impl Deliver, shutdown: CancellationToken) {
    loop {
        match drain(&pool, &deliver).await {
            Ok(n) if n as i64 == BATCH => continue, // a full batch: there may be more
            Ok(_) => {}
            Err(error) => tracing::warn!(%error, "outbox drain failed"),
        }
        tokio::select! {
            () = hub.outbox_woken() => {}
            () = tokio::time::sleep(POLL_INTERVAL) => {}
            () = shutdown.cancelled() => return,
        }
    }
}

/// Claims one batch of due jobs, delivers them, and records the outcome, all in one
/// transaction; returns how many it handled.
///
/// The row locks are held while delivering, which is what makes a job belong to exactly one
/// worker; it also means a slow delivery holds a pooled connection. Deliveries must have
/// short timeouts of their own.
pub async fn drain(pool: &Pool, deliver: &impl Deliver) -> Result<usize, AppError> {
    let mut client = pool.get().await?;
    let tx = client.transaction().await?;
    let jobs = repo::outbox::claim(&tx, BATCH).await?;
    for job in &jobs {
        match deliver.deliver(&job.topic, &job.payload).await {
            Ok(()) => repo::outbox::done(&tx, job.id).await?,
            Err(error) => {
                tracing::warn!(id = job.id, topic = job.topic, %error, "delivery failed; will retry");
                repo::outbox::failed(&tx, job.id, &error, retry_in(job.attempts)).await?;
            }
        }
    }
    tx.commit().await?;
    Ok(jobs.len())
}

/// 1 s, 2 s, 4 s … after each failed attempt, up to an hour.
fn retry_in(attempts: i32) -> Duration {
    Duration::from_secs(1u64 << attempts.clamp(0, 12)).min(Duration::from_secs(3600))
}
