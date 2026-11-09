//! Streams of rows read into structs as they arrive, for results too large to hold at once.
//!
//! The columns are resolved once, from the prepared statement, before the first row, as
//! [`fetch_all`](crate::fetch_all) does; each row is then read as it comes off the socket.
//! The stream borrows the executor it reads from: while it is unfinished, its connection
//! serves no one else (each request's response channel holds one message, so an unread
//! stream stalls every later request on the connection), and the borrow keeps a pooled
//! connection from going back to the pool with rows still on the way (main book Chapter 9,
//! root PLAN.md §12 #13).

use std::marker::PhantomData;
use std::pin::Pin;
use std::task::{Context, Poll, ready};

use futures_util::Stream;
use tokio_postgres::RowStream;
use tokio_postgres::types::ToSql;

use crate::error::Error;
use crate::executor::Executor;
use crate::row::FromRow;

/// The rows of a statement, each read into `T` as it arrives: a [`Stream`] of
/// `Result<T, Error>`. Made by [`fetch_stream`] or [`Query::stream`](crate::query::Query::stream).
///
/// `'a` is the borrow of the executor: the stream cannot outlive the connection it reads from.
/// Finish it, or drop it, before using the connection for anything else.
pub struct ModelStream<'a, T: FromRow> {
    rows: Pin<Box<RowStream>>,
    columns: T::Columns,
    _db: PhantomData<&'a ()>,
}

// The columns are never pinned: only `rows` is polled in place, and it is boxed.
impl<T: FromRow> Unpin for ModelStream<'_, T> {}

impl<T: FromRow> Stream for ModelStream<'_, T> {
    type Item = Result<T, Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        Poll::Ready(match ready!(this.rows.as_mut().poll_next(cx)) {
            Some(Ok(row)) => Some(T::from_row_at(&row, &this.columns)),
            Some(Err(error)) => Some(Err(Error::Db(error))),
            None => None,
        })
    }
}

/// The rows of `sql`, read into `T` one at a time, as they arrive. A missing column or a wrong
/// type fails here, before the query runs.
pub async fn fetch_stream<'a, T: FromRow>(
    db: &'a impl Executor,
    sql: &str,
    params: &[&(dyn ToSql + Sync)],
) -> Result<ModelStream<'a, T>, Error> {
    let statement = db.prepare(sql).await?;
    let columns = T::resolve(statement.columns())?;
    let rows = db.query_raw(&statement, params).await?;
    Ok(ModelStream {
        rows: Box::pin(rows),
        columns,
        _db: PhantomData,
    })
}
