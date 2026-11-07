//! Running a query and reading its rows into structs.
//!
//! Each function prepares the statement first and resolves the struct's columns against it:
//! a missing column or a wrong type fails before the query runs, even if it would return no
//! rows. That costs no extra round trip: `query` with a `&str` prepares the statement anyway
//! (main book §4.1).
//!
//! They take tokio-postgres's `GenericClient`, so a `Client` or a `Transaction` will do (from
//! a deadpool connection, pass `&**object`). Phase S1b replaces it with shelf's own executor.

use tokio_postgres::GenericClient;
use tokio_postgres::types::ToSql;

use crate::error::Error;
use crate::row::FromRow;

/// Every row of `sql`, read into `T`.
pub async fn fetch_all<T: FromRow>(
    db: &impl GenericClient,
    sql: &str,
    params: &[&(dyn ToSql + Sync)],
) -> Result<Vec<T>, Error> {
    let statement = db.prepare(sql).await?;
    let columns = T::resolve(statement.columns())?;
    let rows = db.query(&statement, params).await?;
    rows.iter()
        .map(|row| T::from_row_at(row, &columns))
        .collect()
}

/// The one row of `sql`, read into `T`; fails if there are none, or several.
pub async fn fetch_one<T: FromRow>(
    db: &impl GenericClient,
    sql: &str,
    params: &[&(dyn ToSql + Sync)],
) -> Result<T, Error> {
    let statement = db.prepare(sql).await?;
    let columns = T::resolve(statement.columns())?;
    let row = db.query_one(&statement, params).await?;
    T::from_row_at(&row, &columns)
}

/// The row of `sql`, read into `T`, if there is one; fails if there are several.
pub async fn fetch_opt<T: FromRow>(
    db: &impl GenericClient,
    sql: &str,
    params: &[&(dyn ToSql + Sync)],
) -> Result<Option<T>, Error> {
    let statement = db.prepare(sql).await?;
    let columns = T::resolve(statement.columns())?;
    let row = db.query_opt(&statement, params).await?;
    row.map(|row| T::from_row_at(&row, &columns)).transpose()
}
