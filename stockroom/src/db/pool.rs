//! The connection pool (Chapter 13).

use std::time::Duration;

use deadpool_postgres::{BuildError, Manager, ManagerConfig, Pool, RecyclingMethod, Runtime};
use tokio_postgres::Config;

use super::MakeTls;

/// A pool of at most `max_size` connections.
///
/// `RecyclingMethod::Fast` hands a returned connection to the next caller after checking only
/// that it is not closed. That is right for this service because no handler leaves session
/// state behind: no `SET` (only `SET LOCAL`, which ends with its transaction), no `LISTEN`
/// (the listener has its own connection), and no temporary table that outlives a
/// transaction. A service that cannot promise that wants `RecyclingMethod::Clean`.
pub fn create(config: Config, tls: MakeTls, max_size: usize) -> Result<Pool, BuildError> {
    let manager = Manager::from_config(
        config,
        tls,
        ManagerConfig {
            recycling_method: RecyclingMethod::Fast,
        },
    );
    Pool::builder(manager)
        .max_size(max_size)
        .runtime(Runtime::Tokio1)
        // Waiting longer than this for a free connection means the service is overloaded:
        // better to answer 503 than to queue requests without bound.
        .wait_timeout(Some(Duration::from_secs(5)))
        .create_timeout(Some(Duration::from_secs(5)))
        .recycle_timeout(Some(Duration::from_secs(5)))
        .build()
}
