//! [`Executor`]: what shelf needs from a connection, implemented for the four types a program
//! has (orm/PLAN.md §4.5, §10).
//!
//! tokio-postgres's `GenericClient` and deadpool's are both sealed, and only deadpool's has a
//! statement cache, so shelf has a trait of its own. Its methods return futures that are `Send`,
//! so that shelf calls work inside axum handlers, spawned tasks and the main book's
//! `in_transaction` helper (orm/PLAN.md §12 #28).

use std::future::Future;

use bytes::Bytes;
use tokio_postgres::types::ToSql;
use tokio_postgres::{CopyInSink, Error, Row, RowStream, Statement};

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

    /// A prepared statement for `sql`, never from a cache: the server's description now.
    /// [`verify`](crate::verify) needs it, since a cached statement describes a table as it was
    /// when it was prepared. By default, [`prepare`](Executor::prepare).
    fn prepare_uncached(&self, sql: &str) -> impl Future<Output = Result<Statement, Error>> + Send {
        self.prepare(sql)
    }

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

    /// Starts `statement` and returns its rows as they arrive, for streams. The `RowStream` does
    /// not borrow the connection, which serves no one else until the stream is finished or
    /// dropped.
    fn query_raw(
        &self,
        statement: &Statement,
        params: &[&(dyn ToSql + Sync)],
    ) -> impl Future<Output = Result<RowStream, Error>> + Send;

    /// Starts a `COPY … FROM STDIN` and returns the sink its data goes into. A COPY the server
    /// rejects as it starts leaves a tokio-postgres 0.7.18 connection unusable (issue #789), so
    /// shelf checks the table and columns first (orm/PLAN.md §12 #38).
    fn copy_in(
        &self,
        statement: &Statement,
    ) -> impl Future<Output = Result<CopyInSink<Bytes>, Error>> + Send;
}

/// Implements [`Executor`] for `$ty` by calling `$target`'s methods (which `$ty` derefs to),
/// with `$prepare` for [`Executor::prepare`].
macro_rules! executor {
    ($ty:ty, $target:ty, $prepare:ident) => {
        impl Executor for $ty {
            fn prepare(&self, sql: &str) -> impl Future<Output = Result<Statement, Error>> + Send {
                self.$prepare(sql)
            }

            fn prepare_uncached(
                &self,
                sql: &str,
            ) -> impl Future<Output = Result<Statement, Error>> + Send {
                <$target>::prepare(self, sql)
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

            fn query_raw(
                &self,
                statement: &Statement,
                params: &[&(dyn ToSql + Sync)],
            ) -> impl Future<Output = Result<RowStream, Error>> + Send {
                <$target>::query_raw(self, statement, params.iter().copied())
            }

            fn copy_in(
                &self,
                statement: &Statement,
            ) -> impl Future<Output = Result<CopyInSink<Bytes>, Error>> + Send {
                <$target>::copy_in(self, statement)
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
