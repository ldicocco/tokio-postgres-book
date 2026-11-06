//! `Customer`: who placed an order, stored as `jsonb` in `orders.customer`.

use serde::{Deserialize, Serialize};

/// Read and written through `tokio_postgres::types::Json<Customer>`: serde does the work, and
/// the column stays free to grow fields without a migration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Customer {
    pub name: String,
    pub email: String,
}

impl Customer {
    /// A deliberately small check: non-empty name, something@something.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.name.trim().is_empty() || self.name.len() > 200 {
            return Err("the customer's name must be 1–200 characters");
        }
        match self.email.split_once('@') {
            Some((user, host)) if !user.is_empty() && host.contains('.') => Ok(()),
            _ => Err("the customer's email address is not valid"),
        }
    }
}
