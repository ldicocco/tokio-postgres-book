//! `Address`: a store's postal address, the composite type `address`.

use postgres_types::{FromSql, ToSql};
use serde::{Deserialize, Serialize};

/// Mirrors `CREATE TYPE address AS (street text, city text, postcode text, country char(2))`.
/// The derive matches fields by name; `country` reads from `char(2)` (`bpchar`) as a `String`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSql, FromSql)]
#[postgres(name = "address")]
pub struct Address {
    pub street: String,
    pub city: String,
    pub postcode: String,
    pub country: String,
}
