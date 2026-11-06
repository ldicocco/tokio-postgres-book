//! The domain's value types and how each one maps to PostgreSQL (Chapter 5).
//!
//! | Rust | PostgreSQL | How |
//! |---|---|---|
//! | [`Sku`] | domain `sku` over `text` | `ToSql`/`FromSql` written by hand |
//! | [`Cents`] | `bigint` | derived, `#[postgres(transparent)]` |
//! | [`Quantity`] | domain `positive_quantity` over `integer` | derived domain |
//! | [`OrderStatus`] | enum `order_status` | derived enum |
//! | [`Address`] | composite type `address` | derived composite |
//! | [`Customer`] | `jsonb` | serde, through `tokio_postgres::types::Json` |

mod address;
mod cents;
mod customer;
mod order_status;
mod quantity;
mod sku;

pub use address::Address;
pub use cents::Cents;
pub use customer::Customer;
pub use order_status::OrderStatus;
pub use quantity::{InvalidQuantity, Quantity};
pub use sku::{InvalidSku, Sku};
