//! Everything about reaching the database, as opposed to what is in it (`crate::repo`).

pub mod cancel;
pub mod connect;
pub mod migrate;
pub mod pool;
pub mod retry;
pub mod tls;

pub use cancel::CancelOnDrop;
pub use connect::connect;
pub use retry::in_transaction;
pub use tls::MakeTls;

use tokio_postgres::types::ToSql;

/// Parameters for the `*_raw` methods, which take an iterator rather than a slice
/// (Chapter 9): `client.query_raw(&statement, slice_iter(&[&a, &b]))`.
pub fn slice_iter<'a>(
    params: &'a [&'a (dyn ToSql + Sync)],
) -> impl ExactSizeIterator<Item = &'a dyn ToSql> + Send + Sync + 'a {
    params.iter().map(|p| *p as _)
}
