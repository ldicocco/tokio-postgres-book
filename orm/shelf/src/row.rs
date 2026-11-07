//! Reading rows into structs: the [`FromRow`] trait, and the helpers its derive calls.

use tokio_postgres::types::{FromSqlOwned, WasNull};
use tokio_postgres::{Column, Row};

use crate::error::{Error, Mapping};

/// A value built from one result row. Usually derived: `#[derive(FromRow)]`.
///
/// Reading happens in two steps, so that the work that depends only on the statement is done
/// once, not on every row:
///
/// 1. [`resolve`](FromRow::resolve) finds each field's column among the statement's columns
///    and checks that its type can be read into the field. A mismatch is reported here, before
///    any row is read, even when the result is empty.
/// 2. [`from_row_at`](FromRow::from_row_at) reads one row through the positions `resolve`
///    found, without looking a name up again.
///
/// [`from_row`](FromRow::from_row) does both, for a single row.
pub trait FromRow: Sized {
    /// Where this type's values are in a result, as found by `resolve`. For a derived struct,
    /// a tuple with one element per field.
    type Columns: Send + Sync + 'static;

    /// Finds and checks this type's columns in `columns`: a statement's, or a row's.
    fn resolve(columns: &[Column]) -> Result<Self::Columns, Error>;

    /// Reads `row`, whose columns `columns` was resolved from.
    fn from_row_at(row: &Row, columns: &Self::Columns) -> Result<Self, Error>;

    /// Reads a single row: `resolve`, then `from_row_at`. For many rows of one statement,
    /// resolve once instead ([`from_rows`] does).
    fn from_row(row: &Row) -> Result<Self, Error> {
        Self::from_row_at(row, &Self::resolve(row.columns())?)
    }
}

/// A value that can be read from one column: whatever tokio-postgres can decode into an owned
/// value (`FromSqlOwned`). Every such type implements it.
///
/// It exists for the error message: a field whose type cannot be read is reported in these
/// words, at the field, instead of as an unsatisfied bound on a driver trait.
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot be read from a PostgreSQL column",
    label = "tokio-postgres has no `FromSql` for this type",
    note = "use a type tokio-postgres reads, add `#[shelf(json)]` or `#[shelf(try_from = …)]`, \
            or implement `FromSql` for it"
)]
pub trait FromColumn: FromSqlOwned {}

impl<T: FromSqlOwned> FromColumn for T {}

/// Reads every row of one statement into `T`, resolving the columns once.
pub fn from_rows<T: FromRow>(rows: &[Row]) -> Result<Vec<T>, Error> {
    let Some(first) = rows.first() else {
        return Ok(Vec::new());
    };
    let columns = T::resolve(first.columns())?;
    rows.iter()
        .map(|row| T::from_row_at(row, &columns))
        .collect()
}

/// The position of the column `name` in `columns`, checked to be readable as `T`.
///
/// Generated code calls this from `resolve`; it is public for that reason only.
#[doc(hidden)]
pub fn column<T: FromColumn>(
    columns: &[Column],
    model: &'static str,
    field: &'static str,
    name: &str,
) -> Result<usize, Error> {
    let mapping = |problem| Error::Mapping {
        model,
        field,
        problem,
    };
    // Exact names only. `Row::try_get(&str)` would also try again ignoring ASCII case, but
    // PostgreSQL itself folds unquoted names to lower case, so the exact name is the rule.
    let Some(index) = columns.iter().position(|c| c.name() == name) else {
        return Err(mapping(Mapping::MissingColumn {
            column: name.to_owned(),
        }));
    };
    check::<T>(&columns[index], index).map_err(mapping)
}

/// The column at `index`, checked to be readable as `T`: for values read by position.
#[doc(hidden)]
pub fn column_at<T: FromColumn>(
    columns: &[Column],
    model: &'static str,
    field: &'static str,
    index: usize,
) -> Result<usize, Error> {
    let mapping = |problem| Error::Mapping {
        model,
        field,
        problem,
    };
    let Some(column) = columns.get(index) else {
        return Err(mapping(Mapping::MissingColumn {
            column: format!("#{index}"),
        }));
    };
    check::<T>(column, index).map_err(mapping)
}

fn check<T: FromColumn>(column: &Column, index: usize) -> Result<usize, Mapping> {
    if T::accepts(column.type_()) {
        Ok(index)
    } else {
        Err(Mapping::WrongType {
            column: column.name().to_owned(),
            sql_type: column.type_().clone(),
            rust_type: short_type_name(std::any::type_name::<T>()),
        })
    }
}

/// Reads the value at `index` (found by [`column`]) as `T`.
#[doc(hidden)]
pub fn get<T: FromColumn>(
    row: &Row,
    index: usize,
    model: &'static str,
    field: &'static str,
) -> Result<T, Error> {
    row.try_get(index)
        .map_err(|e| decode_error(row, index, model, field, e))
}

/// Reads the value at `index` as `S`, then converts it with `T::try_from`: the derive's
/// `#[shelf(try_from = S)]`.
#[doc(hidden)]
pub fn get_try_from<S, T>(
    row: &Row,
    index: usize,
    model: &'static str,
    field: &'static str,
) -> Result<T, Error>
where
    S: FromColumn,
    T: TryFrom<S>,
    T::Error: std::error::Error + Send + Sync + 'static,
{
    let value: S = get(row, index, model, field)?;
    T::try_from(value).map_err(|e| Error::Mapping {
        model,
        field,
        problem: Mapping::Decode {
            column: column_name(row, index),
            source: Box::new(e),
        },
    })
}

fn decode_error(
    row: &Row,
    index: usize,
    model: &'static str,
    field: &'static str,
    error: tokio_postgres::Error,
) -> Error {
    let column = column_name(row, index);
    let message = error.to_string();
    let problem = match error.into_source() {
        Some(source) if source.is::<WasNull>() => Mapping::Null { column },
        Some(source) => Mapping::Decode { column, source },
        // No cause: the index or the type did not fit, which `resolve` rules out unless the
        // row comes from another statement.
        None => Mapping::Decode {
            column,
            source: message.into(),
        },
    };
    Error::Mapping {
        model,
        field,
        problem,
    }
}

fn column_name(row: &Row, index: usize) -> String {
    row.columns()
        .get(index)
        .map_or_else(|| format!("#{index}"), |c| c.name().to_owned())
}

/// `type_name`'s output without module paths, for messages:
/// `core::option::Option<alloc::string::String>` becomes `Option<String>`.
fn short_type_name(full: &str) -> String {
    let mut out = String::with_capacity(full.len());
    let mut segment = String::new();
    let mut chars = full.chars().peekable();
    while let Some(c) = chars.next() {
        if c.is_alphanumeric() || c == '_' {
            segment.push(c);
        } else if c == ':' && chars.peek() == Some(&':') {
            chars.next();
            segment.clear(); // a module path: drop it
        } else {
            out.push_str(&segment);
            segment.clear();
            out.push(c);
        }
    }
    out.push_str(&segment);
    out
}

#[cfg(test)]
mod tests {
    use super::short_type_name;

    #[test]
    fn type_names_lose_their_module_paths() {
        assert_eq!(
            short_type_name("core::option::Option<alloc::string::String>"),
            "Option<String>"
        );
        assert_eq!(short_type_name("i64"), "i64");
        assert_eq!(
            short_type_name(
                "postgres_types::Json<std::collections::HashMap<alloc::string::String, i32>>"
            ),
            "Json<HashMap<String, i32>>"
        );
        assert_eq!(short_type_name("&str"), "&str");
    }
}
