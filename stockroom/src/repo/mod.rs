//! What is in the database (Chapter 14): one module per aggregate, plain async functions.
//!
//! Every function takes `&impl GenericClient` (deadpool's: like tokio-postgres's, plus
//! `prepare_cached`), so the same code runs on a pooled connection or inside a transaction;
//! or it takes a `&Transaction` when it only makes sense inside one. SQL lives here and
//! nowhere else, written out in full so that what runs is what you read.

pub mod orders;
pub mod outbox;
pub mod products;
pub mod reports;
pub mod stock;
pub mod stores;

use tokio_postgres::Row;

use crate::AppError;

/// Rows into values, for any type that can be built from a `&Row`.
pub fn from_rows<T>(rows: &[Row]) -> Result<Vec<T>, AppError>
where
    for<'r> T: TryFrom<&'r Row, Error = tokio_postgres::Error>,
{
    rows.iter()
        .map(|row| T::try_from(row).map_err(AppError::from))
        .collect()
}
