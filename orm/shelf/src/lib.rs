//! **shelf**: a small ORM on tokio-postgres, built chapter by chapter in the book under
//! `orm/book/`.
//!
//! - **The mapping** (Part I): [`FromRow`] and its derive, with the `#[shelf(…)]` attributes
//!   `rename_all`, `rename`, `skip`, `flatten`, `json` and `try_from`; tuples read by position;
//!   one check per statement ([`FromRow::resolve`]); and the [`fetch_all`], [`fetch_one`] and
//!   [`fetch_opt`] helpers, for SQL you write.
//! - **The model** (Part II): the [`Executor`] trait, implemented by tokio-postgres's `Client`
//!   and `Transaction` and, with the default feature `deadpool`, by deadpool-postgres's; the
//!   [`Table`] derive (`find`, `find_opt`, `all`, `count`, `update`, `delete`), [`Insert`] and
//!   [`Upsert`], and [`Patch`], with optimistic locking through a `version` field; and
//!   [`Error`].
//! - **Queries and relations** (Part III): the [`query`] builder; `has_many` and `belongs_to`
//!   relations, whose loaders read what a slice of rows relates to in one query; bulk writes
//!   ([`Insert::insert_many`], [`Insert::copy_in`]); streams of models ([`fetch_stream`],
//!   [`ModelStream`]); and [`verify`], which checks a model against its table at startup.
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
//!
//! A model knows its table, so its everyday SQL is written for it, at compile time:
//!
//! ```ignore
//! #[derive(Debug, FromRow, Table)]
//! #[shelf(table = "orders", has_many(Line, key = order_id, as = lines))]
//! struct Order {
//!     #[shelf(primary_key)]
//!     id: i64,
//!     #[shelf(version)]
//!     version: i32,
//! }
//!
//! let order = Order::find(&db, &7).await?; // or Error::NotFound
//! let orders = Order::query().filter(Order::ID.gt(100)).limit(20).all(&db).await?;
//! let lines = Order::load_lines(&db, &orders).await?; // one query for every order
//! ```

mod bulk;
mod error;
mod executor;
mod fetch;
mod json;
mod patch;
pub mod query;
mod relations;
mod row;
mod sql;
mod stream;
mod table;
mod tuples;
mod verify;
mod write;

pub use error::{Error, Mapping, Mismatch};
pub use executor::Executor;
pub use fetch::{fetch_all, fetch_one, fetch_opt};
pub use patch::Patch;
pub use row::{FromColumn, FromRow, from_rows};
pub use stream::{ModelStream, fetch_stream};
pub use table::Table;
pub use verify::verify;
pub use write::{Insert, Upsert};

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

/// Derives [`Table`](trait@Table) for a struct that also derives [`FromRow`](trait@FromRow).
/// It reads the column names `FromRow` reads, and these attributes:
///
/// | On the struct | Meaning |
/// |---|---|
/// | `table = "products"` | The table (required) |
/// | `schema = "inventory"` | Its schema, when it is not found on the `search_path` |
///
/// | On a field | Meaning |
/// |---|---|
/// | `primary_key` | Part of the key (required on at least one field); several make a composite key, a tuple in field order |
/// | `generated` | The database fills it (`DEFAULT uuidv7()`, `now()`, an identity): writes leave it out |
/// | `version` | Optimistic locking: an update expects the row at this version, and adds one; else [`Error::Stale`] |
/// | `updated_at` | An update sets it to `now()`, the transaction's start time |
///
/// Relations go on the struct too, as many as the model has:
///
/// | Relation | Generates |
/// |---|---|
/// | `has_many(Line, key = order_id, as = lines)` | `order.lines(&db)`, a `Vec<Line>`, and `Order::load_lines(&db, &orders)`, one `Vec<Line>` per order, in the orders' order. `order_id` is a field of `Line`; `as` names the methods and is required. The model's key must be a single column |
/// | `belongs_to(Order, key = order_id)` | `line.order(&db)`, an `Order` (an `Option<Order>` when the field is an `Option`), and `Line::load_order(&db, &lines)`, a `HashMap` from key to `Order`. `order_id` is a field of this struct; `as = name` renames the methods, which default to the model's name in snake case |
///
/// Each loader sends one query, `= ANY($1)` with every key in one array.
///
/// Identifiers are quoted at compile time, so a table called `order` works. A model cannot be
/// generic, and cannot `flatten` another struct, whose columns it would have to list.
pub use shelf_macros::Table;

/// Derives [`Insert`](trait@Insert) for a struct of the columns a new row supplies, and
/// [`Upsert`](trait@Upsert) when some of its fields are marked `conflict`.
///
/// | On the struct | Meaning |
/// |---|---|
/// | `model = Product` | The model the row becomes (required): its table, and its columns, which `RETURNING` reads back |
/// | `rename_all = "…"` | As for `FromRow` |
///
/// | On a field | Meaning |
/// |---|---|
/// | `rename = "col"`, `skip` | As for `FromRow`: write the column `col`; do not write this field |
/// | `json` | Write it as `json`/`jsonb`, through serde |
/// | `conflict` | Part of the unique key an upsert collides on: `ON CONFLICT (…) DO UPDATE` sets every other column |
///
/// The statements are constants built at compile time from the model's. The struct may borrow
/// (`sku: &'a str`), but cannot have type parameters.
pub use shelf_macros::Insert;

/// Derives [`Patch`](trait@Patch) for a struct of `Option` fields: `None` leaves a column as it
/// is, `Some` sets it, and `Option<Option<T>>` lets a nullable column be set to NULL.
///
/// | On the struct | Meaning |
/// |---|---|
/// | `model = Product` | The model whose row it changes (required) |
/// | `rename_all = "…"` | As for `FromRow` |
///
/// | On a field | Meaning |
/// |---|---|
/// | `rename = "col"`, `skip` | As for `FromRow` |
/// | `version` | Not a column to set: the version the patch was made against, required exactly when the model has a `version` |
///
/// A `json` column needs no attribute here: write the field as
/// `Option<tokio_postgres::types::Json<T>>` or `Option<serde_json::Value>`.
///
/// Every patch of a type sends one statement, with a flag beside each value; it is built once,
/// at first use. The struct may borrow, but cannot have type parameters.
pub use shelf_macros::Patch;

/// What generated code refers to. Not part of the API: it may change in any release.
#[doc(hidden)]
pub mod __private {
    pub use crate::bulk::array;
    pub use crate::json::Json;
    pub use crate::patch::{flag, patch_sql};
    pub use crate::relations::{ForeignKey, children, load_children, load_parents, parent_opt};
    pub use crate::row::{column, column_at, get, get_try_from};
    pub use crate::sql::{join, total_len, utf8};
    pub use crate::table::param;
    pub use crate::verify::Field;
    pub use serde_json;
    pub use tokio_postgres;
}
