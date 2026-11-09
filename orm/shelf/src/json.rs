//! The wrapper that `#[shelf(json)]` reads a column through, and writes it through.

use std::error::Error;
use std::fmt::Debug;

use bytes::BytesMut;
use serde::{Deserialize, Serialize};
use tokio_postgres::types::{FromSql, IsNull, ToSql, Type, WasNull, to_sql_checked};

/// A `json` or `jsonb` column, deserialized with serde, in which an SQL `NULL` is read as JSON
/// `null`.
///
/// postgres-types' `Json<T>` refuses an SQL `NULL` whatever `T` is, so a field
/// `#[shelf(json)] x: Option<T>` would fail on one. Through this wrapper, `T` decides: a type
/// that deserializes from `null` (`Option<T>`, `serde_json::Value`) reads it, and any other
/// fails with `Mapping::Null`, as a NULL in any other non-`Option` field does.
///
/// Generated code reads and writes `#[shelf(json)]` fields through it; it is public for that
/// reason only.
#[doc(hidden)]
#[derive(Debug)]
#[repr(transparent)]
pub struct Json<T>(pub T);

impl<T> Json<T> {
    /// `value` seen as a `Json<T>`, without moving or copying it: what lets a write list a
    /// `#[shelf(json)]` field among its parameters, which borrow from the struct.
    pub fn from_ref(value: &T) -> &Json<T> {
        // SAFETY: `Json<T>` is `#[repr(transparent)]` over `T`, so the two have the same layout,
        // and the reference keeps `value`'s lifetime.
        unsafe { &*(value as *const T).cast::<Json<T>>() }
    }
}

impl<T: Serialize + Debug> ToSql for Json<T> {
    fn to_sql(
        &self,
        ty: &Type,
        out: &mut BytesMut,
    ) -> Result<IsNull, Box<dyn Error + Sync + Send>> {
        tokio_postgres::types::Json(&self.0).to_sql(ty, out)
    }

    fn accepts(ty: &Type) -> bool {
        <tokio_postgres::types::Json<T> as ToSql>::accepts(ty)
    }

    to_sql_checked!();
}

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
