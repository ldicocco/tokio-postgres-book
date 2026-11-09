//! What is in the database, on shelf: stockroom's repository layer (`stockroom/src/repo/`) for
//! stores, products and orders, one module per aggregate, as there.
//!
//! Every function takes `&impl shelf::Executor`, so the same code runs on a pooled connection
//! (passed as `&object`, which keeps deadpool's statement cache) or inside a transaction. The
//! models derive their SQL; what is written out by hand is the SQL whose wording is a rule.

pub mod orders;
pub mod products;
pub mod stock;
pub mod stores;

use futures_util::join;
use shelf::{Executor, verify};

use orders::{LineRow, OrderRow};
use products::Product;
use stock::Stock;
use stores::Store;

/// Checks every model against the database as it is now, as the service starts: a model that
/// does not fit its table fails here, every mismatch at once, rather than at the first request
/// that reads it. The checks are pipelined on the one connection.
pub async fn verify_models(db: &impl Executor) -> Result<(), Vec<shelf::Error>> {
    let (stores, products, stock, orders, lines) = join!(
        verify::<Store>(db),
        verify::<Product>(db),
        verify::<Stock>(db),
        verify::<OrderRow>(db),
        verify::<LineRow>(db),
    );
    let errors: Vec<shelf::Error> = [stores, products, stock, orders, lines]
        .into_iter()
        .filter_map(Result::err)
        .collect();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}
