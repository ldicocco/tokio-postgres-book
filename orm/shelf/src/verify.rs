//! [`verify`]: a model checked against its table before it is used, at startup or in a test,
//! with every mismatch reported at once.
//!
//! A query checks its mapping when it is prepared (Chapter 5), which may be the first request
//! after a deploy. `verify` does that check early, for every field, and adds what a statement
//! cannot say: whether a column can hold NULL, which the catalog knows for a table's own
//! columns (orm/PLAN.md §12 #40).

use std::collections::HashSet;

use tokio_postgres::Column;
use tokio_postgres::types::Type;

use crate::error::{Error, Mapping, Mismatch};
use crate::executor::Executor;
use crate::row::{self, FromColumn};
use crate::table::Table;

/// A field of a model as `verify` checks it: its column, and the type it reads through.
/// `#[derive(Table)]` lists them in `Table::FIELDS`.
#[doc(hidden)]
pub struct Field {
    /// The field.
    pub name: &'static str,
    /// The column it reads, unquoted.
    pub column: &'static str,
    /// Finds the column and checks its type: `column::<T>` for the type the field reads.
    pub resolve: fn(&[Column], &'static str, &'static str, &str) -> Result<usize, Error>,
    /// Whether the field can take an SQL NULL from a column of this type.
    pub reads_null: fn(&Type) -> bool,
}

impl Field {
    /// The description of a field that reads its column as `T`.
    pub const fn new<T: FromColumn>(name: &'static str, column: &'static str) -> Field {
        Field {
            name,
            column,
            resolve: row::column::<T>,
            reads_null: reads_null::<T>,
        }
    }
}

fn reads_null<T: FromColumn>(ty: &Type) -> bool {
    T::from_sql_null(ty).is_ok()
}

/// The columns of `$1` (table OIDs) and `$2` (column numbers) that can hold NULL. Only a table's
/// own columns count: a view reports its own OID, where every column looks nullable.
const NULLABLE: &str = "SELECT a.attrelid, a.attnum
    FROM unnest($1::oid[], $2::int2[]) AS c (attrelid, attnum)
    JOIN pg_attribute AS a USING (attrelid, attnum)
    JOIN pg_class AS r ON r.oid = a.attrelid
    WHERE NOT a.attnotnull AND r.relkind IN ('r', 'p')";

/// Checks model `M` against its table, as the database describes it now: every field's column
/// exists, has a type the field can read, and, if the field is not an `Option`, cannot hold
/// NULL. Every mismatch is reported at once, in [`Error::Schema`].
///
/// It prepares `SELECT * FROM` the table, which reads no rows, without the executor's statement
/// cache: a cached statement describes the table as it was. A column of a view, or one computed
/// by an expression, has no `NOT NULL` to read, and is not checked for NULLs.
pub async fn verify<M: Table>(db: &impl Executor) -> Result<(), Error> {
    // `*`, not the model's columns: a statement naming a missing column fails at the first one.
    let statement = db
        .prepare_uncached(&format!("SELECT * FROM {} LIMIT 0", M::TABLE))
        .await?;
    let columns = statement.columns();
    let mut mismatches = Vec::new();
    let mut not_null = Vec::new(); // fields that cannot read NULL, and their column's index
    for (position, field) in M::FIELDS.iter().enumerate() {
        match (field.resolve)(columns, M::MODEL, field.name, field.column) {
            Ok(index) if !(field.reads_null)(columns[index].type_()) => {
                not_null.push((position, field, index));
            }
            Ok(_) => {}
            Err(Error::Mapping { problem, .. }) => mismatches.push((position, field.name, problem)),
            Err(error) => return Err(error),
        }
    }
    let (tables, numbers): (Vec<u32>, Vec<i16>) = not_null
        .iter()
        .filter_map(|(_, _, i)| Some((columns[*i].table_oid()?, columns[*i].column_id()?)))
        .unzip();
    if !tables.is_empty() {
        let statement = db.prepare(NULLABLE).await?;
        let nullable: HashSet<(u32, i16)> = db
            .query(&statement, &[&tables, &numbers])
            .await?
            .iter()
            .map(|row| (row.get(0), row.get(1)))
            .collect();
        for (position, field, index) in not_null {
            let column = &columns[index];
            if let (Some(table), Some(number)) = (column.table_oid(), column.column_id())
                && nullable.contains(&(table, number))
            {
                let problem = Mapping::Nullable {
                    column: column.name().to_owned(),
                };
                mismatches.push((position, field.name, problem));
            }
        }
    }
    if mismatches.is_empty() {
        return Ok(());
    }
    mismatches.sort_by_key(|(position, _, _)| *position);
    Err(Error::Schema {
        model: M::MODEL,
        mismatches: mismatches
            .into_iter()
            .map(|(_, field, problem)| Mismatch { field, problem })
            .collect(),
    })
}
