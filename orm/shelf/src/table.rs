//! [`Table`]: a [`FromRow`] type with a table and a key, and the reads its derive makes free.

use std::future::Future;

use tokio_postgres::types::ToSql;

use crate::error::Error;
use crate::executor::Executor;
use crate::fetch::{fetch_all, fetch_opt};
use crate::row::FromRow;
use crate::write::written;

/// A model: a [`FromRow`] type that is a table's row, with a key. Usually derived:
/// `#[derive(FromRow, Table)]` with `#[shelf(table = "…")]` and `#[shelf(primary_key)]`.
///
/// The derive writes every statement as a string literal at compile time, with identifiers
/// quoted and values left as `$n` parameters; the constants hold them, so the SQL a call sends
/// can be printed. Every statement names its columns: never `SELECT *`.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a shelf model",
    label = "not a `Table`",
    note = "derive it: `#[derive(FromRow, Table)]`, with `#[shelf(table = \"…\")]` on the struct and \
            `#[shelf(primary_key)]` on its key"
)]
pub trait Table: FromRow + Send + Sync {
    /// The key's type: the key column's field type, or a tuple of them for a composite key.
    type Key: Send + Sync;

    /// The struct's name, for errors.
    const MODEL: &'static str;
    /// The table, quoted, and qualified when it has a schema: `"inventory"."products"`.
    const TABLE: &'static str;
    /// The columns the model reads, quoted, in field order.
    const COLUMNS: &'static str;
    /// The row with a given key: `SELECT … WHERE "id" = $1`.
    const FIND: &'static str;
    /// Every row, ordered by the key.
    const ALL: &'static str;
    /// The number of rows: `SELECT count(*) FROM …`.
    const COUNT: &'static str;
    /// Writes every column that is neither the key nor `generated`, and reads the row back:
    /// `UPDATE … SET "sku" = $1, … WHERE "id" = $n RETURNING …`.
    const UPDATE: &'static str;
    /// `DELETE FROM … WHERE "id" = $1`.
    const DELETE: &'static str;
    /// The key's columns, quoted.
    const KEY_COLUMNS: &'static [&'static str];
    /// The `#[shelf(version)]` column, quoted: an update expects it unchanged, and adds one.
    const VERSION: Option<&'static str>;
    /// The `#[shelf(updated_at)]` column, quoted: an update sets it to `now()`.
    const UPDATED_AT: Option<&'static str>;

    /// This row's key.
    fn key(&self) -> Self::Key;

    /// `key` as statement parameters, in the order of the key's columns.
    fn key_params(key: &Self::Key) -> Vec<&(dyn ToSql + Sync)>;

    /// The values `UPDATE` sets, in order; the key's follow them.
    fn update_params(&self) -> Vec<&(dyn ToSql + Sync)>;

    /// The row whose key is `key`, or [`Error::NotFound`].
    fn find(
        db: &impl Executor,
        key: &Self::Key,
    ) -> impl Future<Output = Result<Self, Error>> + Send {
        async move {
            Self::find_opt(db, key)
                .await?
                .ok_or(Error::NotFound { model: Self::MODEL })
        }
    }

    /// The row whose key is `key`, if there is one.
    fn find_opt(
        db: &impl Executor,
        key: &Self::Key,
    ) -> impl Future<Output = Result<Option<Self>, Error>> + Send {
        async move { fetch_opt(db, Self::FIND, &Self::key_params(key)).await }
    }

    /// Every row, ordered by the key.
    fn all(db: &impl Executor) -> impl Future<Output = Result<Vec<Self>, Error>> + Send {
        async move { fetch_all(db, Self::ALL, &[]).await }
    }

    /// The number of rows.
    fn count(db: &impl Executor) -> impl Future<Output = Result<i64, Error>> + Send {
        async move {
            let statement = db.prepare(Self::COUNT).await?;
            Ok(db.query_one(&statement, &[]).await?.try_get(0)?)
        }
    }

    /// Writes this value over the row with its key, and returns the row as it now is (with
    /// whatever the database changed: a `version`, an `updated_at`, a trigger's work).
    /// [`Error::NotFound`] if no row has the key; for a model with a `version`,
    /// [`Error::Stale`] if the row is no longer at this value's version.
    fn update(&self, db: &impl Executor) -> impl Future<Output = Result<Self, Error>> + Send {
        async move {
            let key = self.key();
            let mut params = self.update_params();
            params.extend(Self::key_params(&key));
            written(db, Self::UPDATE, &params).await
        }
    }

    /// Deletes the row with this value's key; [`Error::NotFound`] if there is none.
    fn delete(&self, db: &impl Executor) -> impl Future<Output = Result<(), Error>> + Send {
        async move { Self::delete_by_key(db, &self.key()).await }
    }

    /// Deletes the row whose key is `key`; [`Error::NotFound`] if there is none.
    fn delete_by_key(
        db: &impl Executor,
        key: &Self::Key,
    ) -> impl Future<Output = Result<(), Error>> + Send {
        async move {
            let statement = db.prepare(Self::DELETE).await?;
            match db.execute(&statement, &Self::key_params(key)).await? {
                0 => Err(Error::NotFound { model: Self::MODEL }),
                _ => Ok(()),
            }
        }
    }
}

/// `value` as a statement parameter. Generated code calls this to list a key's parameters.
#[doc(hidden)]
pub fn param<T: ToSql + Sync>(value: &T) -> &(dyn ToSql + Sync) {
    value
}
