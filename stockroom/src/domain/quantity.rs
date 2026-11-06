//! `Quantity`: how many units an order line asks for, always at least one.

use postgres_types::{FromSql, ToSql};
use serde::{Deserialize, Serialize};

/// A positive number of units. In the database it is the domain `positive_quantity`, and the
/// derive below is the domain form (a one-field tuple struct with the domain's `name`): as a
/// parameter it is sent as that domain, and it reads back from the domain or from its base
/// type `integer` (a result column of a domain type is described as the base type).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, ToSql, FromSql,
)]
#[serde(try_from = "i32", into = "i32")]
#[postgres(name = "positive_quantity")]
pub struct Quantity(i32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("{0} is not a valid quantity: it must be between 1 and {max}", max = Quantity::MAX)]
pub struct InvalidQuantity(pub i32);

impl Quantity {
    /// More than this in one order line is a typo, not a click-and-collect order.
    pub const MAX: i32 = 1_000;

    pub fn new(units: i32) -> Result<Quantity, InvalidQuantity> {
        if (1..=Self::MAX).contains(&units) {
            Ok(Quantity(units))
        } else {
            Err(InvalidQuantity(units))
        }
    }

    pub fn get(self) -> i32 {
        self.0
    }
}

impl TryFrom<i32> for Quantity {
    type Error = InvalidQuantity;
    fn try_from(units: i32) -> Result<Self, Self::Error> {
        Quantity::new(units)
    }
}

impl From<Quantity> for i32 {
    fn from(q: Quantity) -> i32 {
        q.0
    }
}
