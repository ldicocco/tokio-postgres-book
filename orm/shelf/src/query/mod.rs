//! The query builder: a model's rows, filtered, ordered and paged, with the SQL written for you.
//!
//! ```ignore
//! let page: Vec<Product> = Product::query()
//!     .filter(Product::PRICE_CENTS.gt(1_000).and(Product::TAGS.contains("sale")))
//!     .order_by(Product::SKU.asc())
//!     .limit(20)
//!     .all(&db)
//!     .await?;
//! ```
//!
//! What is typed is what a mistake would get wrong: the column belongs to the model, and the
//! value has the column's type. Everything else is plain SQL, rendered once per call with the
//! values as `$n` parameters, never in the text. [`Expr::raw`] takes SQL the builder does not
//! model. Each shape of query is its own statement: through a deadpool connection, each one is
//! prepared once and then cached.

mod column;
mod expr;
mod render;

use std::marker::PhantomData;

use tokio_postgres::types::ToSql;

pub use column::{Column, Order, Value};
pub use expr::{Condition, Expr, Raw};

use crate::error::Error;
use crate::executor::Executor;
use crate::fetch::{fetch_all, fetch_opt};
use crate::stream::{ModelStream, fetch_stream};
use crate::table::Table;
use expr::Node;
use render::Sql;

/// A query on model `M`'s table: [`Table::query`], then `filter`, `order_by`, `limit` and the
/// rest, then [`all`](Query::all), [`first`](Query::first), [`stream`](Query::stream),
/// [`count`](Query::count) or [`exists`](Query::exists). It owns its values, so it can be built in one function and run
/// in another.
pub struct Query<M> {
    filters: Vec<Node>,
    order: Vec<Order<M>>,
    limit: Option<i64>,
    offset: Option<i64>,
    _model: PhantomData<fn() -> M>,
}

/// What a rendering is for.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Select {
    /// The rows, ordered and paged.
    Rows { first: bool },
    /// `count(*)` of the rows that match.
    Count,
    /// Whether any row matches.
    Exists,
}

impl<M: Table> Query<M> {
    /// Every row of the table, in key order, until filtered.
    pub fn new() -> Self {
        Query {
            filters: Vec::new(),
            order: Vec::new(),
            limit: None,
            offset: None,
            _model: PhantomData,
        }
    }

    /// Keeps the rows that satisfy `condition`. Several filters must all hold (`AND`).
    #[track_caller]
    pub fn filter(mut self, condition: impl Condition<M>) -> Self {
        self.filters.push(condition.into_expr().node);
        self
    }

    /// Orders the rows by `order`, after any earlier `order_by`. Without one, rows come in key
    /// order, as [`Table::all`] returns them.
    pub fn order_by(mut self, order: Order<M>) -> Self {
        self.order.push(order);
        self
    }

    /// At most `n` rows.
    pub fn limit(mut self, n: i64) -> Self {
        self.limit = Some(n);
        self
    }

    /// Skips the first `n` rows. Each page costs more than the last, since the server still
    /// reads the rows it skips: prefer [`after`](Query::after) for deep pages.
    pub fn offset(mut self, n: i64) -> Self {
        self.offset = Some(n);
        self
    }

    /// Keyset pagination (main book §9.6): the rows after `value` in `column`'s ascending order.
    /// It adds `"col" > $n`, and orders by the column first unless an `order_by` already does.
    /// The column should be unique (a key, a SKU), or rows that share a value are skipped.
    pub fn after<T: ToSql + Sync + Send + 'static>(
        mut self,
        column: Column<M, T>,
        value: impl Value<T>,
    ) -> Self {
        if self.order.first().map(|o| o.column) != Some(column.name()) {
            self.order.insert(0, column.asc());
        }
        self.filter(column.gt(value))
    }

    /// The statement [`all`](Query::all) sends.
    pub fn sql(&self) -> String {
        self.render(Select::Rows { first: false }).text
    }

    /// The parameters of [`sql`](Query::sql), in the order of their `$n`.
    pub fn params(&self) -> Vec<&(dyn ToSql + Sync)> {
        self.render(Select::Rows { first: false }).params
    }

    /// Every row that matches.
    pub async fn all(&self, db: &impl Executor) -> Result<Vec<M>, Error> {
        let sql = self.render(Select::Rows { first: false });
        fetch_all(db, &sql.text, &sql.params).await
    }

    /// The first row that matches, if any (`LIMIT 1`, in place of any `limit`).
    pub async fn first(&self, db: &impl Executor) -> Result<Option<M>, Error> {
        let sql = self.render(Select::Rows { first: true });
        fetch_opt(db, &sql.text, &sql.params).await
    }

    /// How many rows match. Order, `limit` and `offset` do not apply.
    pub async fn count(&self, db: &impl Executor) -> Result<i64, Error> {
        let sql = self.render(Select::Count);
        let statement = db.prepare(&sql.text).await?;
        Ok(db.query_one(&statement, &sql.params).await?.try_get(0)?)
    }

    /// Whether any row matches. Order, `limit` and `offset` do not apply.
    pub async fn exists(&self, db: &impl Executor) -> Result<bool, Error> {
        let sql = self.render(Select::Exists);
        let statement = db.prepare(&sql.text).await?;
        Ok(db.query_one(&statement, &sql.params).await?.try_get(0)?)
    }

    /// Every row that matches, read as it arrives: a stream that holds one row at a time, for
    /// results too large to collect. It borrows `db`, whose connection serves no one else until
    /// the stream is finished or dropped.
    pub async fn stream<'a>(&self, db: &'a impl Executor) -> Result<ModelStream<'a, M>, Error> {
        let sql = self.render(Select::Rows { first: false });
        fetch_stream(db, &sql.text, &sql.params).await
    }

    fn render(&self, select: Select) -> Sql<'_> {
        let mut sql = Sql::new(match select {
            Select::Rows { .. } => format!("SELECT {} FROM {}", M::COLUMNS, M::TABLE),
            Select::Count => format!("SELECT count(*) FROM {}", M::TABLE),
            Select::Exists => format!("SELECT EXISTS (SELECT 1 FROM {}", M::TABLE),
        });
        for (i, filter) in self.filters.iter().enumerate() {
            sql.text.push_str(if i == 0 { " WHERE " } else { " AND " });
            // A filter that is itself an OR keeps its parentheses among the ANDs.
            if matches!(filter, Node::Or(_) | Node::Raw { .. }) && self.filters.len() > 1 {
                sql.text.push('(');
                sql.node(filter);
                sql.text.push(')');
            } else {
                sql.node(filter);
            }
        }
        let Select::Rows { first } = select else {
            if select == Select::Exists {
                sql.text.push(')');
            }
            return sql;
        };
        sql.text.push_str(" ORDER BY ");
        if self.order.is_empty() {
            sql.text.push_str(&M::KEY_COLUMNS.join(", "));
        } else {
            let terms: Vec<String> = self
                .order
                .iter()
                .map(|o| match o.descending {
                    true => format!("{} DESC", o.column),
                    false => o.column.to_owned(),
                })
                .collect();
            sql.text.push_str(&terms.join(", "));
        }
        if first {
            sql.text.push_str(" LIMIT 1");
        } else if let Some(limit) = &self.limit {
            sql.text.push_str(" LIMIT ");
            sql.param(limit);
        }
        if let Some(offset) = &self.offset {
            sql.text.push_str(" OFFSET ");
            sql.param(offset);
        }
        sql
    }
}

impl<M: Table> Default for Query<M> {
    fn default() -> Self {
        Query::new()
    }
}
