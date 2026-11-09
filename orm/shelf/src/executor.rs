//! [`Executor`]: what shelf needs from a connection, implemented for the four types a program
//! has (orm/PLAN.md §4.5, §10).
//!
//! tokio-postgres's `GenericClient` and deadpool's are both sealed, and only deadpool's has a
//! statement cache, so shelf has a trait of its own. Its methods return futures that are `Send`,
//! so that shelf calls work inside axum handlers, spawned tasks and the main book's
//! `in_transaction` helper (orm/PLAN.md §12 #28).

use std::future::Future;

use tokio_postgres::types::ToSql;
use tokio_postgres::{Error, Row, Statement};

/// A connection or transaction shelf can run statements on.
///
/// Implemented for tokio-postgres's `Client` and `Transaction`, and, with the default feature
/// `deadpool`, for deadpool-postgres's `Client` (a pooled connection: pass `&object`) and
/// `Transaction`. [`prepare`](Executor::prepare) is where they differ: tokio-postgres's types
/// prepare every time, which costs the round trip that `query` with a SQL string would cost
/// anyway; deadpool's use their connection's statement cache, so a statement is prepared once
/// per connection (orm/PLAN.md §12 #29).
pub trait Executor: Sync {
    /// A prepared statement for `sql`, from the cache when the executor has one.
    fn prepare(&self, sql: &str) -> impl Future<Output = Result<Statement, Error>> + Send;

    /// Every row `statement` returns.
    fn query(
        &self,
        statement: &Statement,
        params: &[&(dyn ToSql + Sync)],
    ) -> impl Future<Output = Result<Vec<Row>, Error>> + Send;

    /// The one row `statement` returns; an error if there are none, or several.
    fn query_one(
        &self,
        statement: &Statement,
        params: &[&(dyn ToSql + Sync)],
    ) -> impl Future<Output = Result<Row, Error>> + Send;

    /// The row `statement` returns, if any; an error if there are several.
    fn query_opt(
        &self,
        statement: &Statement,
        params: &[&(dyn ToSql + Sync)],
    ) -> impl Future<Output = Result<Option<Row>, Error>> + Send;

    /// Runs `statement` and returns the number of rows it affected.
    fn execute(
        &self,
        statement: &Statement,
        params: &[&(dyn ToSql + Sync)],
    ) -> impl Future<Output = Result<u64, Error>> + Send;
}

/// Implements [`Executor`] for `$ty` by calling `$target`'s methods (which `$ty` derefs to),
/// with `$prepare` for [`Executor::prepare`].
macro_rules! executor {
    ($ty:ty, $target:ty, $prepare:ident) => {
        impl Executor for $ty {
            fn prepare(&self, sql: &str) -> impl Future<Output = Result<Statement, Error>> + Send {
                self.$prepare(sql)
            }

            fn query(
                &self,
                statement: &Statement,
                params: &[&(dyn ToSql + Sync)],
            ) -> impl Future<Output = Result<Vec<Row>, Error>> + Send {
                <$target>::query(self, statement, params)
            }

            fn query_one(
                &self,
                statement: &Statement,
                params: &[&(dyn ToSql + Sync)],
            ) -> impl Future<Output = Result<Row, Error>> + Send {
                <$target>::query_one(self, statement, params)
            }

            fn query_opt(
                &self,
                statement: &Statement,
                params: &[&(dyn ToSql + Sync)],
            ) -> impl Future<Output = Result<Option<Row>, Error>> + Send {
                <$target>::query_opt(self, statement, params)
            }

            fn execute(
                &self,
                statement: &Statement,
                params: &[&(dyn ToSql + Sync)],
            ) -> impl Future<Output = Result<u64, Error>> + Send {
                <$target>::execute(self, statement, params)
            }
        }
    };
}

executor!(tokio_postgres::Client, tokio_postgres::Client, prepare);
executor!(
    tokio_postgres::Transaction<'_>,
    tokio_postgres::Transaction<'_>,
    prepare
);
#[cfg(feature = "deadpool")]
executor!(
    deadpool_postgres::Client,
    tokio_postgres::Client,
    prepare_cached
);
#[cfg(feature = "deadpool")]
executor!(
    deadpool_postgres::Transaction<'_>,
    tokio_postgres::Transaction<'_>,
    prepare_cached
);
