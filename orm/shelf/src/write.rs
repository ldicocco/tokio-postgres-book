//! Writing rows: [`Insert`] for a `New…` struct, and [`Upsert`] when it names a unique key.

use std::future::Future;

use tokio_postgres::types::ToSql;

use crate::error::Error;
use crate::executor::Executor;
use crate::fetch::{fetch_one, fetch_opt};
use crate::table::Table;

/// A row to insert into a model's table. Usually derived: `#[derive(Insert)]` with
/// `#[shelf(model = Product)]` on a struct of the columns you supply; the database fills the
/// rest (its defaults: `uuidv7()`, `now()`, an identity).
pub trait Insert: Sync {
    /// The model the row becomes: its table is where the row goes, and its columns what
    /// `RETURNING` reads back.
    type Model: Table;

    /// `INSERT INTO … (…) VALUES ($1, …) RETURNING …`, the model's columns.
    const INSERT: &'static str;

    /// The fields, as `INSERT`'s parameters, in order.
    fn params(&self) -> Vec<&(dyn ToSql + Sync)>;

    /// Inserts the row and returns it as the model, with every default filled in: one round
    /// trip.
    fn insert(
        &self,
        db: &impl Executor,
    ) -> impl Future<Output = Result<Self::Model, Error>> + Send {
        async move { fetch_one(db, Self::INSERT, &self.params()).await }
    }
}

/// An [`Insert`] that updates the existing row when it collides with a unique key: derived
/// when fields are marked `#[shelf(conflict)]`, which name that key.
pub trait Upsert: Insert {
    /// `INSERT … ON CONFLICT (…) DO UPDATE SET … = excluded.… RETURNING …`.
    const UPSERT: &'static str;

    /// Inserts the row, or updates the one with the same conflict key, and returns it.
    fn upsert(
        &self,
        db: &impl Executor,
    ) -> impl Future<Output = Result<Self::Model, Error>> + Send {
        async move { fetch_one(db, Self::UPSERT, &self.params()).await }
    }
}

/// Runs an `UPDATE … RETURNING` of model `M`, and reads the row it returns. For a model with a
/// `version`, the statement is the one that also reports why nothing changed (the derive's
/// `UPDATE`, and `Patch`'s): `shelf_updated` says whether the row was written, and
/// `shelf_version` what version it is at, NULL when there is no row (orm/PLAN.md §12 #31).
pub(crate) async fn written<M: Table>(
    db: &impl Executor,
    sql: &str,
    params: &[&(dyn ToSql + Sync)],
) -> Result<M, Error> {
    let not_found = Error::NotFound { model: M::MODEL };
    if M::VERSION.is_none() {
        return fetch_opt(db, sql, params).await?.ok_or(not_found);
    }
    let statement = db.prepare(sql).await?;
    let columns = M::resolve(statement.columns())?;
    let row = db.query_one(&statement, params).await?;
    if row.try_get::<_, i64>("shelf_updated")? == 1 {
        return M::from_row_at(&row, &columns);
    }
    match row.try_get::<_, Option<i64>>("shelf_version")? {
        Some(current_version) => Err(Error::Stale {
            model: M::MODEL,
            current_version,
        }),
        None => Err(not_found),
    }
}
