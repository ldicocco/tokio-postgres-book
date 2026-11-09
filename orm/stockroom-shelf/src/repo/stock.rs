//! Stock per store, as far as orders need it: one row by its composite key.
//!
//! Levels, adjustments and transfers stay stockroom's (`repo/stock.rs`): an adjustment adds to
//! `on_hand` (`on_hand = stock.on_hand + EXCLUDED.on_hand`), which an upsert that sets columns
//! cannot say, and a transfer is a serializable read-then-write whose statements are the point.

use shelf::{FromRow, Table};
use uuid::Uuid;

#[derive(Debug, Clone, FromRow, Table)]
#[shelf(table = "stock")]
pub struct Stock {
    #[shelf(primary_key)]
    pub store_id: Uuid,
    #[shelf(primary_key)]
    pub product_id: Uuid,
    pub on_hand: i32,
    pub reserved: i32,
    /// `on_hand - reserved`, the virtual generated column (PG 18): never written. It cannot be
    /// NULL, but the catalog does not say so (a generated column is nullable unless declared
    /// `NOT NULL`), so `verify` would refuse an `i32` here.
    #[shelf(generated)]
    pub available: Option<i32>,
}
