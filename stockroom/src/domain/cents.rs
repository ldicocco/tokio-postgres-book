//! `Cents`: money as a whole number of cents, stored as `bigint`.

use std::fmt;
use std::iter::Sum;

use postgres_types::{FromSql, ToSql};
use serde::{Deserialize, Serialize};

/// An amount of money in cents. `transparent` makes it travel exactly like its `i64`, so it
/// can be used wherever the database has a `bigint`, arrays (`bigint[]`) included.
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    ToSql,
    FromSql,
)]
#[serde(transparent)]
#[postgres(transparent)]
pub struct Cents(pub i64);

impl Cents {
    /// `self × quantity`, or `None` on overflow.
    pub fn times(self, quantity: i32) -> Option<Cents> {
        self.0.checked_mul(i64::from(quantity)).map(Cents)
    }

    pub fn checked_add(self, other: Cents) -> Option<Cents> {
        self.0.checked_add(other.0).map(Cents)
    }
}

impl Sum for Cents {
    fn sum<I: Iterator<Item = Cents>>(iter: I) -> Cents {
        Cents(iter.map(|c| c.0).sum())
    }
}

impl fmt::Display for Cents {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sign = if self.0 < 0 { "-" } else { "" };
        write!(
            f,
            "{sign}{}.{:02}",
            (self.0 / 100).abs(),
            (self.0 % 100).abs()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arithmetic_and_display() {
        assert_eq!(Cents(4999).times(3), Some(Cents(14997)));
        assert_eq!(Cents(i64::MAX).times(2), None);
        assert_eq!([Cents(1), Cents(2)].into_iter().sum::<Cents>(), Cents(3));
        assert_eq!(Cents(4999).to_string(), "49.99");
        assert_eq!(Cents(-5).to_string(), "-0.05");
    }
}
