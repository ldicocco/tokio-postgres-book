//! Bulk writes for an [`Insert`] struct: [`Insert::insert_many`] sends a batch as one statement
//! through `unnest`, and [`Insert::copy_in`] loads it with binary COPY.
//!
//! Both learn the columns' types from the struct's own single-row `INSERT`, prepared: its
//! parameters are each column's type as PostgreSQL sees it (a domain as the domain, an enum by
//! name), so a batch accepts exactly what [`Insert::insert`] accepts. Preparing it also checks
//! the table and the columns, cleanly, before a COPY starts (orm/PLAN.md §12 #37, #38).

use std::pin::pin;

use tokio_postgres::binary_copy::BinaryCopyInWriter;
use tokio_postgres::types::{Kind, ToSql, Type};

use crate::error::Error;
use crate::executor::Executor;
use crate::fetch::fetch_all;
use crate::table::Table;
use crate::write::Insert;

/// One array parameter: a column of a batch, borrowed from its rows.
pub type Array<'r> = Box<dyn ToSql + Sync + Send + 'r>;

/// The column `field` reads from every row, as one array parameter: `Vec<&T>` is sent as
/// `T`'s array type. Generated code calls this once per column of an `Insert` struct.
#[doc(hidden)]
pub fn array<'r, R: Sync, T: ToSql + Sync + 'r>(
    rows: &'r [R],
    field: fn(&'r R) -> &'r T,
) -> Array<'r> {
    Box::new(rows.iter().map(field).collect::<Vec<&T>>())
}

/// `Insert::insert_many`: `INSERT INTO "t" ("a", "b") SELECT * FROM unnest($1::…[], $2::…[])
/// RETURNING …`, one array per column.
pub(crate) async fn insert_many<I: Insert>(
    db: &impl Executor,
    rows: &[I],
) -> Result<Vec<I::Model>, Error> {
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let insert = db.prepare(I::INSERT).await?;
    let model = <I::Model as Table>::MODEL;
    let (table, returning) = (<I::Model as Table>::TABLE, <I::Model as Table>::COLUMNS);
    if I::COLUMNS.is_empty() {
        // Nothing to write: one row of defaults per value.
        let count = rows.len() as i64;
        let sql = format!(
            "INSERT INTO {table} SELECT FROM generate_series(1, $1::bigint) RETURNING {returning}"
        );
        return fetch_all(db, &sql, &[&count]).await;
    }
    let mut casts = Vec::with_capacity(I::COLUMNS.len());
    for (i, (ty, column)) in insert.params().iter().zip(I::COLUMNS).enumerate() {
        if is_array(ty) {
            return Err(Error::Unsupported {
                model,
                message: format!(
                    "insert_many cannot send the array column {column} through unnest, which \
                     would flatten an array of arrays; use copy_in, or insert in a loop"
                ),
            });
        }
        casts.push(format!(
            "${}::{}.{}[]",
            i + 1,
            quote_ident(ty.schema()),
            quote_ident(ty.name())
        ));
    }
    let sql = format!(
        "INSERT INTO {table} ({}) SELECT * FROM unnest({}) RETURNING {returning}",
        I::COLUMNS.join(", "),
        casts.join(", ")
    );
    let arrays = I::arrays(rows);
    let params: Vec<&(dyn ToSql + Sync)> = arrays.iter().map(|a| &**a as _).collect();
    fetch_all(db, &sql, &params).await
}

/// `Insert::copy_in`: `COPY "t" ("a", "b") FROM STDIN (FORMAT binary)`, one row per value.
pub(crate) async fn copy_in<I: Insert>(db: &impl Executor, rows: &[I]) -> Result<u64, Error> {
    if I::COLUMNS.is_empty() {
        return Err(Error::Unsupported {
            model: <I::Model as Table>::MODEL,
            message: "copy_in needs a column to copy; use insert_many".to_owned(),
        });
    }
    if rows.is_empty() {
        return Ok(0);
    }
    // Preparing a COPY checks nothing, and a COPY rejected as it starts leaves the connection
    // unusable in 0.7.18 (issue #789). Preparing the INSERT checks the table and columns.
    let insert = db.prepare(I::INSERT).await?;
    let copy = db.prepare(I::COPY).await?;
    let sink = db.copy_in(&copy).await?;
    // Binary COPY trusts these types: they are the columns' own, from the server.
    let mut writer = pin!(BinaryCopyInWriter::new(sink, insert.params()));
    for row in rows {
        writer.as_mut().write(&row.params()).await?;
    }
    Ok(writer.finish().await?)
}

/// Whether a column of type `ty` holds arrays, through any domains over it.
fn is_array(ty: &Type) -> bool {
    match ty.kind() {
        Kind::Array(_) => true,
        Kind::Domain(base) => is_array(base),
        _ => false,
    }
}

/// `name` as a quoted SQL identifier, for a name learnt at run time (a type's).
fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}
