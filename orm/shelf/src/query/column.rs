//! Column constants, the values they compare with, and the conditions they build.

use std::fmt;
use std::marker::PhantomData;

use tokio_postgres::types::ToSql;

use super::expr::{Expr, Node};

/// A column of model `M` whose values are `T`. `#[derive(Table)]` generates one per field,
/// named after it in capitals: `Product::PRICE_CENTS` is a `Column<Product, i64>`.
///
/// The two type parameters are what the builder checks at compile time: a condition built from
/// it holds only a `T` (`Product::PRICE_CENTS.gt("x")` does not compile), and it belongs to `M`
/// (a `Product` query refuses an `Order` column). Neither is stored: the value is the column's
/// quoted name alone.
pub struct Column<M, T> {
    name: &'static str,
    // `fn() -> …` makes the marker `Send`, `Sync` and `Copy` whatever `M` and `T` are.
    _model: PhantomData<fn() -> (M, T)>,
}

impl<M, T> Column<M, T> {
    /// The column `name`, quoted as SQL will name it (`"price_cents"`). The derive calls this;
    /// a hand-written model can too.
    pub const fn new(name: &'static str) -> Self {
        Column {
            name,
            _model: PhantomData,
        }
    }

    /// The column's name, quoted.
    pub const fn name(self) -> &'static str {
        self.name
    }

    /// Ascending order by this column, for [`Query::order_by`](super::Query::order_by).
    pub fn asc(self) -> Order<M> {
        Order::new(self.name, false)
    }

    /// Descending order by this column.
    pub fn desc(self) -> Order<M> {
        Order::new(self.name, true)
    }
}

impl<M, T: ToSql + Sync + Send + 'static> Column<M, T> {
    /// `"col" = $n`
    pub fn eq(self, value: impl Value<T>) -> Expr<M> {
        self.compare("=", value)
    }

    /// `"col" <> $n`
    pub fn ne(self, value: impl Value<T>) -> Expr<M> {
        self.compare("<>", value)
    }

    /// `"col" < $n`
    pub fn lt(self, value: impl Value<T>) -> Expr<M> {
        self.compare("<", value)
    }

    /// `"col" <= $n`
    pub fn le(self, value: impl Value<T>) -> Expr<M> {
        self.compare("<=", value)
    }

    /// `"col" > $n`
    pub fn gt(self, value: impl Value<T>) -> Expr<M> {
        self.compare(">", value)
    }

    /// `"col" >= $n`
    pub fn ge(self, value: impl Value<T>) -> Expr<M> {
        self.compare(">=", value)
    }

    /// `"col" BETWEEN $n AND $m`, both ends included.
    pub fn between(self, low: impl Value<T>, high: impl Value<T>) -> Expr<M> {
        Expr::new(Node::Between {
            column: self.name,
            low: Box::new(low.into_value()),
            high: Box::new(high.into_value()),
        })
    }

    /// `"col" = ANY($n)`: the column equals one of `values`, sent as one array parameter, so
    /// the statement is the same however many values there are (main book §3.9).
    pub fn any<V: Value<T>>(self, values: impl IntoIterator<Item = V>) -> Expr<M> {
        let values: Vec<T> = values.into_iter().map(Value::into_value).collect();
        Expr::new(Node::Any {
            column: self.name,
            values: Box::new(values),
        })
    }

    fn compare(self, op: &'static str, value: impl Value<T>) -> Expr<M> {
        Expr::new(Node::Compare {
            column: self.name,
            op,
            value: Box::new(value.into_value()),
        })
    }
}

impl<M> Column<M, String> {
    /// `"col" LIKE $n`, with `%` and `_` as wildcards.
    pub fn like(self, pattern: impl Value<String>) -> Expr<M> {
        like(self.name, pattern)
    }
}

impl<M> Column<M, Option<String>> {
    /// `"col" LIKE $n`, with `%` and `_` as wildcards. A NULL matches nothing.
    pub fn like(self, pattern: impl Value<String>) -> Expr<M> {
        like(self.name, pattern)
    }
}

fn like<M>(column: &'static str, pattern: impl Value<String>) -> Expr<M> {
    Expr::new(Node::Compare {
        column,
        op: "LIKE",
        value: Box::new(pattern.into_value()),
    })
}

impl<M, T> Column<M, Option<T>> {
    /// `"col" IS NULL`. Only a nullable column (an `Option` field) has it.
    pub fn is_null(self) -> Expr<M> {
        Expr::new(Node::Null {
            column: self.name,
            negated: false,
        })
    }

    /// `"col" IS NOT NULL`.
    pub fn is_not_null(self) -> Expr<M> {
        Expr::new(Node::Null {
            column: self.name,
            negated: true,
        })
    }
}

impl<M, E: ToSql + Sync + Send + 'static> Column<M, Vec<E>> {
    /// `"col" @> $n`: the array column contains `element`. The parameter is a one-element array,
    /// which PostgreSQL types as the column's own array type.
    pub fn contains(self, element: impl Value<E>) -> Expr<M> {
        contains(self.name, element)
    }
}

impl<M, E: ToSql + Sync + Send + 'static> Column<M, Option<Vec<E>>> {
    /// `"col" @> $n`: the array column contains `element`. A NULL array contains nothing.
    pub fn contains(self, element: impl Value<E>) -> Expr<M> {
        contains(self.name, element)
    }
}

fn contains<M, E: ToSql + Sync + Send + 'static>(
    column: &'static str,
    element: impl Value<E>,
) -> Expr<M> {
    Expr::new(Node::Compare {
        column,
        op: "@>",
        value: Box::new(vec![element.into_value()]),
    })
}

// By hand: `#[derive(Clone, Copy)]` would require `M: Copy` and `T: Copy`, which the marker
// does not need.
impl<M, T> Clone for Column<M, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<M, T> Copy for Column<M, T> {}

impl<M, T> fmt::Debug for Column<M, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Column").field(&self.name).finish()
    }
}

/// A value for a column whose values are `T`: what a condition accepts.
///
/// A `T` itself, a `&T` (cloned), `&str` for a text column, and a plain value for a nullable
/// column (`Option<T>`). Nothing else: shelf converts no numbers, so that an integer literal
/// takes the column's type (`Product::ID.gt(3_000_000_000)` on a `bigint` column), and an
/// `i32` for a `bigint` column is a compile error rather than a conversion (orm/PLAN.md §12
/// #34).
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot be compared with a column of type `{T}`",
    label = "expected a value of the column's type, `{T}`",
    note = "a condition takes the column's own type, a reference to it, or `&str` for text; \
            shelf converts no numbers"
)]
pub trait Value<T> {
    /// The value, as the column's type.
    fn into_value(self) -> T;
}

impl<T> Value<T> for T {
    fn into_value(self) -> T {
        self
    }
}

impl<T: Clone> Value<T> for &T {
    fn into_value(self) -> T {
        self.clone()
    }
}

impl Value<String> for &str {
    fn into_value(self) -> String {
        self.to_owned()
    }
}

impl<T> Value<Option<T>> for T {
    fn into_value(self) -> Option<T> {
        Some(self)
    }
}

impl Value<Option<String>> for &str {
    fn into_value(self) -> Option<String> {
        Some(self.to_owned())
    }
}

/// An `ORDER BY` term of model `M`: [`Column::asc`] or [`Column::desc`].
pub struct Order<M> {
    pub(crate) column: &'static str,
    pub(crate) descending: bool,
    _model: PhantomData<fn() -> M>,
}

impl<M> Order<M> {
    fn new(column: &'static str, descending: bool) -> Self {
        Order {
            column,
            descending,
            _model: PhantomData,
        }
    }
}

impl<M> Clone for Order<M> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<M> Copy for Order<M> {}
