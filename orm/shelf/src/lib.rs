//! **shelf**: a small ORM on tokio-postgres, built chapter by chapter in the book under
//! `orm/book/`.
//!
//! Phase S1a built the mapping: [`FromRow`] and its derive, with the `#[shelf(…)]` attributes
//! `rename_all`, `rename`, `skip`, `flatten`, `json` and `try_from`; tuples read by position;
//! one check per statement ([`FromRow::resolve`]); and the [`fetch_all`], [`fetch_one`] and
//! [`fetch_opt`] helpers. orm/PLAN.md §4 describes the rest of the target design.
//!
//! ```ignore
//! #[derive(Debug, shelf::FromRow)]
//! struct Product {
//!     id: i64,
//!     #[shelf(rename = "sku_code")]
//!     sku: String,
//!     tags: Option<Vec<String>>,
//! }
//!
//! let products: Vec<Product> =
//!     shelf::fetch_all(&client, "SELECT id, sku_code, tags FROM products", &[]).await?;
//! ```

mod error;
mod fetch;
mod json;
mod row;
mod tuples;

pub use error::{Error, Mapping};
pub use fetch::{fetch_all, fetch_one, fetch_opt};
pub use row::{FromColumn, FromRow, from_rows};

/// Derives [`FromRow`](trait@FromRow). Each named field is read from the column of the same
/// name, unless its `#[shelf(…)]` attributes say otherwise:
///
/// | On the struct | Meaning |
/// |---|---|
/// | `rename_all = "camelCase"` | Column names follow a convention: `snake_case` (the default), `camelCase`, `PascalCase` or `SCREAMING_SNAKE_CASE` |
///
/// | On a field | Meaning |
/// |---|---|
/// | `rename = "col"` | Read the column `col` |
/// | `skip` | Do not read; use `Default::default()` |
/// | `flatten` | A nested `FromRow` struct, read from the same row |
/// | `json` | Read a `json` or `jsonb` column through serde; an SQL `NULL` is read as JSON `null`, so an `Option<T>` field gets `None` |
/// | `try_from = S` | Read an `S`, then convert it with `TryFrom` |
///
/// Fields hold owned values (`String`, not `&str`); a struct with lifetime parameters is
/// refused. Type parameters get the bounds their fields need.
pub use shelf_macros::FromRow;

/// What generated code refers to. Not part of the API: it may change in any release.
#[doc(hidden)]
pub mod __private {
    pub use crate::json::Json;
    pub use crate::row::{column, column_at, get, get_try_from};
    pub use tokio_postgres;
}
