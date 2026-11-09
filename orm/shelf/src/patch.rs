//! [`Patch`]: a partial update, written as one statement whatever fields it sets.

use std::future::Future;

use tokio_postgres::types::ToSql;

use crate::error::Error;
use crate::executor::Executor;
use crate::table::Table;
use crate::write::written;

/// A partial update of a model's row. Usually derived: `#[derive(Patch)]` with
/// `#[shelf(model = Product)]` on a struct of `Option` fields, where `None` leaves a column as
/// it is and `Some` sets it (`Option<Option<T>>` for a nullable column: `Some(None)` sets it
/// to NULL). For a model with a `version`, the patch has a `#[shelf(version)]` field: the
/// version it was made against.
///
/// Every patch of one type sends the same statement, with a flag beside each value:
/// `"col" = CASE WHEN $1 THEN $2 ELSE "col" END` (orm/PLAN.md §10, §12 #33).
pub trait Patch: Sync {
    /// The model whose row the patch changes.
    type Model: Table;

    /// The columns the patch can set, quoted, in field order.
    const COLUMNS: &'static [&'static str];

    /// The statement, built once from the model's constants ([`patch_sql`]).
    fn sql() -> &'static str;

    /// A flag and a value for each column, then the expected version, if any.
    fn params(&self) -> Vec<&(dyn ToSql + Sync)>;

    /// Applies the patch to the row whose key is `key`, and returns the row as it now is.
    /// [`Error::NotFound`] if there is no such row; [`Error::Stale`] if the model has a
    /// `version` and the row is no longer at the patch's.
    fn apply(
        &self,
        db: &impl Executor,
        key: &<Self::Model as Table>::Key,
    ) -> impl Future<Output = Result<Self::Model, Error>> + Send {
        async move {
            let mut params = self.params();
            params.extend(<Self::Model as Table>::key_params(key));
            written(db, Self::sql(), &params).await
        }
    }
}

/// The statement for a patch of `columns` (quoted) on model `M`: a flag and a value per
/// column (`$1`, `$2`, …), then the expected version if `M` has one, then the key. Generated
/// code calls it once, from a `LazyLock`: the numbers of the key's parameters depend on how
/// many the patch has, and the key's columns are the model's.
#[doc(hidden)]
pub fn patch_sql<M: Table>(columns: &[&str]) -> String {
    let mut set: Vec<String> = columns
        .iter()
        .enumerate()
        .map(|(i, c)| {
            format!(
                "{c} = CASE WHEN ${} THEN ${} ELSE {c} END",
                2 * i + 1,
                2 * i + 2
            )
        })
        .collect();
    let mut next = 2 * columns.len() + 1;
    if let Some(version) = M::VERSION {
        set.push(format!("{version} = {version} + 1"));
    }
    if let Some(updated_at) = M::UPDATED_AT {
        set.push(format!("{updated_at} = now()"));
    }
    let version_param = M::VERSION.map(|_| {
        next += 1;
        next - 1
    });
    let key_where: Vec<String> = M::KEY_COLUMNS
        .iter()
        .enumerate()
        .map(|(i, k)| format!("{k} = ${}", next + i))
        .collect();
    let key_where = key_where.join(" AND ");
    let (table, set) = (M::TABLE, set.join(", "));
    let columns = M::COLUMNS;
    match (M::VERSION, version_param) {
        (Some(version), Some(param)) => format!(
            "WITH updated AS (UPDATE {table} SET {set} WHERE {key_where} AND {version} = \
             ${param} RETURNING {columns}) SELECT {columns}, (SELECT count(*) FROM updated) \
             AS shelf_updated, (SELECT {version}::bigint FROM {table} WHERE {key_where}) AS \
             shelf_version FROM (VALUES (1)) AS one LEFT JOIN updated ON true"
        ),
        _ => format!("UPDATE {table} SET {set} WHERE {key_where} RETURNING {columns}"),
    }
}

/// Whether a patch field is set, as the flag beside its value: `&true` or `&false`, which live
/// for ever, so the parameters can still borrow from the patch alone.
#[doc(hidden)]
pub fn flag<T>(value: &Option<T>) -> &'static (dyn ToSql + Sync) {
    if value.is_some() { &true } else { &false }
}
