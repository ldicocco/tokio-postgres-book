//! Conditions: a tree of comparisons, combined with `and`, `or` and `!`, plus raw SQL.

use std::marker::PhantomData;
use std::ops::Not;

use tokio_postgres::types::ToSql;

use super::render::placeholders;

/// A parameter the query owns: built in one function, sent from another, across an `.await`.
pub(crate) type Param = Box<dyn ToSql + Sync + Send>;

/// A condition on model `M`'s rows, for [`Query::filter`](super::Query::filter). Built from
/// [`Column`](super::Column)s (`Product::PRICE_CENTS.gt(1_000)`), combined with
/// [`and`](Expr::and), [`or`](Expr::or) and `!`, or written as SQL with [`Expr::raw`].
///
/// It owns its values, so a filter can be built in one place and run in another.
pub struct Expr<M> {
    pub(crate) node: Node,
    _model: PhantomData<fn() -> M>,
}

/// The tree an [`Expr`] holds. Column names are quoted; values wait as parameters until the
/// query is rendered, which numbers them.
pub(crate) enum Node {
    /// `"col" <op> $n`
    Compare {
        column: &'static str,
        op: &'static str,
        value: Param,
    },
    /// `"col" BETWEEN $n AND $m`
    Between {
        column: &'static str,
        low: Param,
        high: Param,
    },
    /// `"col" = ANY($n)`
    Any {
        column: &'static str,
        values: Param,
    },
    /// `"col" IS [NOT] NULL`
    Null {
        column: &'static str,
        negated: bool,
    },
    And(Vec<Node>),
    Or(Vec<Node>),
    Not(Box<Node>),
    /// SQL written by hand, its placeholders numbered from `$1`.
    Raw {
        sql: String,
        params: Vec<Param>,
    },
}

impl<M> Expr<M> {
    pub(crate) fn new(node: Node) -> Self {
        Expr {
            node,
            _model: PhantomData,
        }
    }

    /// Both conditions: `a AND b`.
    #[track_caller]
    pub fn and(self, other: impl Condition<M>) -> Expr<M> {
        Expr::new(match (self.node, other.into_expr().node) {
            (Node::And(mut all), Node::And(more)) => {
                all.extend(more);
                Node::And(all)
            }
            (Node::And(mut all), other) => {
                all.push(other);
                Node::And(all)
            }
            (first, Node::And(mut all)) => {
                all.insert(0, first);
                Node::And(all)
            }
            (first, other) => Node::And(vec![first, other]),
        })
    }

    /// Either condition: `a OR b`.
    #[track_caller]
    pub fn or(self, other: impl Condition<M>) -> Expr<M> {
        Expr::new(match (self.node, other.into_expr().node) {
            (Node::Or(mut all), Node::Or(more)) => {
                all.extend(more);
                Node::Or(all)
            }
            (Node::Or(mut all), other) => {
                all.push(other);
                Node::Or(all)
            }
            (first, Node::Or(mut all)) => {
                all.insert(0, first);
                Node::Or(all)
            }
            (first, other) => Node::Or(vec![first, other]),
        })
    }

    /// A condition written in SQL, the escape hatch for what the builder does not model
    /// (functions, subqueries, operators). Its placeholders are numbered from `$1`, in the
    /// order of the values given to [`Raw::bind`]; the query renumbers them to fit among its
    /// own. Write column names quoted, as they are in the table.
    ///
    /// ```ignore
    /// Product::query().filter(Expr::raw(r#"lower("sku") = lower($1)"#).bind("tee-02"))
    /// ```
    pub fn raw(sql: impl Into<String>) -> Raw<M> {
        Raw {
            sql: sql.into(),
            params: Vec::new(),
            _model: PhantomData,
        }
    }
}

/// `!condition` is `NOT (condition)`.
impl<M> Not for Expr<M> {
    type Output = Expr<M>;

    fn not(self) -> Expr<M> {
        Expr::new(Node::Not(Box::new(self.node)))
    }
}

/// A condition in SQL, with its values: [`Expr::raw`], then a [`bind`](Raw::bind) for each
/// placeholder. It is a [`Condition`] wherever one is expected.
pub struct Raw<M> {
    sql: String,
    params: Vec<Param>,
    _model: PhantomData<fn() -> M>,
}

impl<M> Raw<M> {
    /// The value for the next placeholder: the first `bind` is `$1`.
    pub fn bind(mut self, value: impl ToSql + Sync + Send + 'static) -> Self {
        self.params.push(Box::new(value));
        self
    }
}

/// What [`Query::filter`](super::Query::filter), [`Expr::and`] and [`Expr::or`] take: a
/// condition on model `M`'s rows, built from its columns ([`Expr`]) or written in SQL
/// ([`Raw`]).
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a condition on `{M}`",
    label = "a condition on another model's rows",
    note = "a query on `{M}` filters by `{M}`'s own columns: `{M}::COLUMN.eq(…)`"
)]
pub trait Condition<M> {
    /// The condition as an expression.
    fn into_expr(self) -> Expr<M>;
}

impl<M> Condition<M> for Expr<M> {
    fn into_expr(self) -> Expr<M> {
        self
    }
}

impl<M> Condition<M> for Raw<M> {
    /// # Panics
    ///
    /// If the SQL refers to a placeholder it has no value for (`$3` with two `bind`s): once the
    /// query renumbered it, it would silently take another condition's value.
    #[track_caller]
    fn into_expr(self) -> Expr<M> {
        if let Some(n) = placeholders(&self.sql)
            .map(|(_, n)| n)
            .find(|n| *n == 0 || *n > self.params.len())
        {
            panic!(
                "raw SQL `{}` refers to ${n}, but has {} bound value(s)",
                self.sql,
                self.params.len()
            );
        }
        Expr::new(Node::Raw {
            sql: self.sql,
            params: self.params,
        })
    }
}
