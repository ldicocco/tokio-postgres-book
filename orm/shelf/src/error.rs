//! shelf's error type.

use tokio_postgres::types::Type;

/// What can go wrong in shelf: a row that does not fit its struct, or anything the database
/// or the driver reports.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A result did not fit the struct it was read into. Reported once per statement, before
    /// the first row is read, when a column is missing or has the wrong type; and for a row
    /// whose value cannot be decoded (a NULL in a non-`Option` field, a failed `try_from`).
    #[error("{model}.{field}: {problem}")]
    Mapping {
        /// The struct (or `tuple`).
        model: &'static str,
        /// The field (or the tuple position).
        field: &'static str,
        #[source]
        problem: Mapping,
    },

    /// No row has the key a call asked for: `find`, or a write to a row that is gone.
    #[error("{model} not found")]
    NotFound {
        /// The model.
        model: &'static str,
    },

    /// A versioned update found the row at another version than it expected: someone else
    /// changed it since it was read.
    #[error("{model} has changed since it was read: it is now at version {current_version}")]
    Stale {
        /// The model.
        model: &'static str,
        /// The row's version now, to report (in an `ETag`, say) or to read again.
        current_version: i64,
    },

    /// [`verify`](crate::verify) found that a model does not match its table: every field that
    /// would fail to read, at once.
    #[error("{model} does not match its table: {}", Mismatch::list(.mismatches))]
    Schema {
        /// The model.
        model: &'static str,
        /// What does not fit, field by field, in field order.
        mismatches: Vec<Mismatch>,
    },

    /// A call shelf cannot make for this model: `insert_many` with an array column, which
    /// `unnest` would flatten, or a COPY of a row with no columns.
    #[error("{model}: {message}")]
    Unsupported {
        /// The model.
        model: &'static str,
        /// What cannot be done, and what to do instead.
        message: String,
    },

    /// The database, the connection, or the driver.
    #[error(transparent)]
    Db(#[from] tokio_postgres::Error),
}

/// Why a result did not fit: the detail of [`Error::Mapping`].
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Mapping {
    /// The result has no column of that name. Names are matched exactly, as PostgreSQL
    /// reports them (unquoted identifiers are folded to lower case).
    #[error("the result has no column {column:?}")]
    MissingColumn { column: String },

    /// The column's type cannot be read into the field's.
    #[error("column {column:?} has type {sql_type}, which cannot be read into {rust_type}")]
    WrongType {
        column: String,
        sql_type: Type,
        rust_type: String,
    },

    /// The column is NULL, and the field is not an `Option`.
    #[error("column {column:?} is NULL; read it into an Option")]
    Null { column: String },

    /// The table's column can hold NULL, and the field cannot take one: found by
    /// [`verify`](crate::verify), from the catalog, before any row is NULL.
    #[error(
        "column {column:?} can be NULL in the table; read it into an Option, or make it NOT NULL"
    )]
    Nullable { column: String },

    /// The value could not be decoded, or a `try_from` conversion refused it.
    #[error("column {column:?}: {source}")]
    Decode {
        column: String,
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

/// One field of a model that does not fit its table: part of [`Error::Schema`].
#[derive(Debug)]
pub struct Mismatch {
    /// The field.
    pub field: &'static str,
    /// Why it does not fit.
    pub problem: Mapping,
}

impl Mismatch {
    fn list(mismatches: &[Mismatch]) -> String {
        let all: Vec<String> = mismatches
            .iter()
            .map(|m| format!("{}: {}", m.field, m.problem))
            .collect();
        all.join("; ")
    }
}
