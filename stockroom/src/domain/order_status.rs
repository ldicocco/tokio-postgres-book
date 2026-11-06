//! `OrderStatus`: the life of a click-and-collect order, as a Postgres enum and a state machine.

use std::fmt;

use postgres_types::{FromSql, ToSql};
use serde::{Deserialize, Serialize};

/// Mirrors `CREATE TYPE order_status AS ENUM ('pending', 'paid', 'ready', 'collected',
/// 'cancelled')`. The derive checks the labels when the type is first used, so a variant
/// missing on either side is an error, not a silent mismatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSql, FromSql)]
#[serde(rename_all = "lowercase")]
#[postgres(name = "order_status", rename_all = "lowercase")]
pub enum OrderStatus {
    Pending,
    Paid,
    Ready,
    Collected,
    Cancelled,
}

impl OrderStatus {
    pub const ALL: [OrderStatus; 5] = [
        OrderStatus::Pending,
        OrderStatus::Paid,
        OrderStatus::Ready,
        OrderStatus::Collected,
        OrderStatus::Cancelled,
    ];

    /// The state machine: paid after pending, ready after paid, collected after ready;
    /// anything not finished can be cancelled; nothing leaves `collected` or `cancelled`.
    pub fn can_become(self, next: OrderStatus) -> bool {
        use OrderStatus::*;
        matches!(
            (self, next),
            (Pending, Paid)
                | (Paid, Ready)
                | (Ready, Collected)
                | (Pending | Paid | Ready, Cancelled)
        )
    }

    /// Every status an order may be in for a move to `next` to be allowed.
    pub fn allowed_before(next: OrderStatus) -> Vec<OrderStatus> {
        Self::ALL
            .into_iter()
            .filter(|s| s.can_become(next))
            .collect()
    }

    pub fn as_str(self) -> &'static str {
        match self {
            OrderStatus::Pending => "pending",
            OrderStatus::Paid => "paid",
            OrderStatus::Ready => "ready",
            OrderStatus::Collected => "collected",
            OrderStatus::Cancelled => "cancelled",
        }
    }
}

impl fmt::Display for OrderStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::OrderStatus::*;
    use super::*;

    #[test]
    fn the_happy_path_and_cancellation() {
        assert!(Pending.can_become(Paid));
        assert!(Paid.can_become(Ready));
        assert!(Ready.can_become(Collected));
        assert!(Ready.can_become(Cancelled));
        assert!(!Pending.can_become(Ready), "no skipping payment");
        assert!(
            !Collected.can_become(Cancelled),
            "a collected order is final"
        );
        assert!(!Cancelled.can_become(Pending));
    }

    #[test]
    fn allowed_before() {
        assert_eq!(
            OrderStatus::allowed_before(Cancelled),
            [Pending, Paid, Ready]
        );
        assert_eq!(OrderStatus::allowed_before(Pending), []);
    }
}
