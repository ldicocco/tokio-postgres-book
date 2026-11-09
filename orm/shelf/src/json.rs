//! The wrapper that `#[shelf(json)]` reads a column through.

use std::error::Error;

use serde::Deserialize;
use tokio_postgres::types::{FromSql, Type, WasNull};

/// A `json` or `jsonb` column, deserialized with serde, in which an SQL `NULL` is read as JSON
/// `null`.
///
/// postgres-types' `Json<T>` refuses an SQL `NULL` whatever `T` is, so a field
/// `#[shelf(json)] x: Option<T>` would fail on one. Through this wrapper, `T` decides: a type
/// that deserializes from `null` (`Option<T>`, `serde_json::Value`) reads it, and any other
/// fails with `Mapping::Null`, as a NULL in any other non-`Option` field does.
///
/// Generated code reads `#[shelf(json)]` fields through it; it is public for that reason only.
#[doc(hidden)]
pub struct Json<T>(pub T);

impl<'a, T: Deserialize<'a>> FromSql<'a> for Json<T> {
    fn from_sql(ty: &Type, raw: &'a [u8]) -> Result<Self, Box<dyn Error + Sync + Send>> {
        tokio_postgres::types::Json::<T>::from_sql(ty, raw).map(|json| Json(json.0))
    }

    fn from_sql_null(_: &Type) -> Result<Self, Box<dyn Error + Sync + Send>> {
        // As for a JSON `null`; a type that refuses it makes this a NULL error.
        T::deserialize(serde_json::Value::Null)
            .map(Json)
            .map_err(|_| Box::new(WasNull) as _)
    }

    fn accepts(ty: &Type) -> bool {
        <tokio_postgres::types::Json<T> as FromSql<'a>>::accepts(ty)
    }
}
