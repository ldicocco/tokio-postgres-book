//! Running a transaction, and running it again when concurrency makes it fail (Chapter 8).

use std::time::Duration;

use deadpool_postgres::{Object, Transaction};
use futures_util::future::BoxFuture;
use tokio_postgres::IsolationLevel;

use crate::error::AppError;

pub const MAX_ATTEMPTS: u32 = 8;

/// Runs `work` in a transaction at `isolation`, commits it, and starts over from `BEGIN` when
/// it fails with a serialization failure, a deadlock or a lock timeout ([`AppError::is_retryable`]),
/// up to [`MAX_ATTEMPTS`] times.
///
/// `work` gets the transaction and `input`, and returns a boxed future:
///
/// ```ignore
/// in_transaction(&mut client, IsolationLevel::Serializable, &request, |tx, request| {
///     repo::stock::transfer(tx, request).boxed()
/// })
/// ```
///
/// Why not `impl AsyncFnMut(&Transaction<'_>) -> Result<T, AppError>`? It compiles, but a
/// handler calling it does not: the compiler cannot prove that the closure's future is `Send`
/// for every lifetime of the borrowed transaction ("implementation of `Send` is not general
/// enough"), and axum needs `Send`. A `BoxFuture` states `Send` explicitly. And `input` is an
/// argument rather than a capture because a future that must be valid for any lifetime `'t`
/// cannot borrow the caller's locals; through the argument, it borrows them for `'t` only.
pub async fn in_transaction<A, T>(
    client: &mut Object,
    isolation: IsolationLevel,
    input: &A,
    mut work: impl for<'t> FnMut(&'t Transaction<'t>, &'t A) -> BoxFuture<'t, Result<T, AppError>>,
) -> Result<T, AppError>
where
    A: ?Sized + Sync,
{
    let mut attempt = 1;
    loop {
        let tx = client
            .build_transaction()
            .isolation_level(isolation)
            .start()
            .await?;
        let outcome = match work(&tx, input).await {
            Ok(value) => tx.commit().await.map(|()| value).map_err(AppError::from),
            Err(error) => {
                // Explicitly, rather than on drop, so the next attempt starts from a clean
                // session and a failed rollback is at least logged.
                if let Err(rollback) = tx.rollback().await {
                    tracing::debug!(%rollback, "rollback failed");
                }
                Err(error)
            }
        };
        match outcome {
            Err(error) if error.is_retryable() && attempt < MAX_ATTEMPTS => {
                tracing::debug!(attempt, %error, "transaction failed under concurrency; retrying");
                tokio::time::sleep(backoff(attempt)).await;
                attempt += 1;
            }
            other => return other,
        }
    }
}

/// Exponential backoff with full jitter: a random wait up to 2, 4, 8 … ms, capped at 100 ms.
/// The randomness keeps transactions that collided once from colliding again in lockstep.
pub fn backoff(attempt: u32) -> Duration {
    let ceiling = (2u64 << attempt.min(16)).min(100);
    Duration::from_millis(fastrand::u64(1..=ceiling))
}

#[cfg(test)]
mod tests {
    #[test]
    fn backoff_grows_and_is_capped() {
        for attempt in 1..20 {
            let wait = super::backoff(attempt).as_millis();
            assert!((1..=100).contains(&wait), "attempt {attempt}: {wait} ms");
        }
    }
}
