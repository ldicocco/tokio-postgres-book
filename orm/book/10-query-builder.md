# Chapter 10 — A typed query builder

> **Building an ORM on tokio-postgres** · Previous: [Chapter 9 — Partial updates and optimistic locking](09-patch-and-versions.md) · Next: [Chapter 11 — Relations without N+1](11-relations.md)

`find` and `all` cover two queries. The third most common is "these rows, filtered, sorted, a
page at a time", and its filter changes with the request: a tag if the client sent one, a price
limit if it sent that, both or neither. Writing that SQL by hand means assembling a string and a
parameter list in step, numbering each `$n` as you go, which is exactly the code where injection
bugs and off-by-one parameter errors live.

A query builder assembles it for you, from typed pieces. The derive already knows every column
and its Rust type, so it can generate a constant per column, `Product::PRICE_CENTS`, that knows
both. A filter built from it, `Product::PRICE_CENTS.gt(1_000)`, takes only an `i64`, and a
mistake is a compile error at the call. The builder numbers the parameters, keeps the values out
of the SQL, and stays small enough to read in one sitting. This chapter builds it, and marks
where it should hand over to plain SQL.

**In this chapter you will learn:**

- what a query builder should type (columns and values) and what it should leave to SQL
- how `Table` generates a `Column<M, T>` constant per field, and what `PhantomData` does there
- how a tree of conditions becomes SQL text and a parameter list that cannot drift apart
- why the query owns its values, and what that allows
- ordering, limits, `count`, `exists` and keyset pages, and how many statements they cost
- which mistakes fail to compile, and how reading the errors changed the design
- the escape hatch: raw SQL inside a filter, renumbered among the builder's parameters

---

## 10.1 What to type

Here is the destination: a function that turns a request's optional filters into a query, and
the SQL it renders for four requests.

```rust
use shelf::query::Query;
use shelf::{FromRow, Table};

#[derive(Debug, FromRow, Table)]
#[shelf(table = "products")]
#[allow(dead_code)] // the program only renders queries
struct Product {
    #[shelf(primary_key)]
    id: i64,
    sku: String,
    price_cents: i64,
    tags: Vec<String>,
}

/// A page of products, with the filters the request sent and only those.
fn page(tag: Option<&str>, max_cents: Option<i64>) -> Query<Product> {
    let mut query = Product::query().order_by(Product::SKU.asc()).limit(20);
    if let Some(tag) = tag {
        query = query.filter(Product::TAGS.contains(tag));
    }
    if let Some(max) = max_cents {
        query = query.filter(Product::PRICE_CENTS.le(max));
    }
    query
}

fn main() {
    let requests = [
        (None, None),
        (Some("sale"), None),
        (None, Some(2_000)),
        (Some("sale"), Some(2_000)),
    ];
    for (tag, max_cents) in requests {
        let query = page(tag, max_cents);
        println!("{}", query.sql());
        println!("    {:?}", query.params());
    }

    let both = page(Some("sale"), Some(2_000));
    assert!(
        both.sql()
            .ends_with(r#"WHERE "tags" @> $1 AND "price_cents" <= $2 ORDER BY "sku" LIMIT $3"#)
    );
    assert_eq!(both.params().len(), 3);
}
```

```text
SELECT "id", "sku", "price_cents", "tags" FROM "products" ORDER BY "sku" LIMIT $1
    [20]
SELECT "id", "sku", "price_cents", "tags" FROM "products" WHERE "tags" @> $1 ORDER BY "sku" LIMIT $2
    [["sale"], 20]
SELECT "id", "sku", "price_cents", "tags" FROM "products" WHERE "price_cents" <= $1 ORDER BY "sku" LIMIT $2
    [2000, 20]
SELECT "id", "sku", "price_cents", "tags" FROM "products" WHERE "tags" @> $1 AND "price_cents" <= $2 ORDER BY "sku" LIMIT $3
    [["sale"], 2000, 20]
```

Each request gets a statement that says exactly what it asks, with every value a parameter and
every `$n` matching its place in the list, including the page size. Nothing in `page` counts
parameters or writes SQL.

What the builder types is chosen by what a mistake would get wrong:

| Typed | Why |
|---|---|
| **The column** belongs to this model | `Product::query()` refuses `Order::TOTAL_CENTS`: a column of another table is an error in PostgreSQL too, but only when the query runs |
| **The value** has the column's type | `PRICE_CENTS.le(max)` takes an `i64`; a `&str` or an `i32` is a compile error, not an "error serializing parameter" at run time |
| **The operator** fits the column | `contains` exists on array columns only, `like` on text, `is_null` on nullable columns |

What it leaves to SQL is everything else: functions, joins, subqueries, aggregates, window
functions, `DISTINCT ON`. A builder that modelled all of SQL would be diesel, a much larger
project; shelf's covers the filters that requests vary, and accepts a raw SQL fragment for
anything else (§10.7). Reports and joins are better written as SQL with a `FromRow` struct, as
in Part I.

## 10.2 Column constants

`#[derive(Table)]` generates one constant per column, named after the field in capitals, in an
inherent `impl` block beside the trait impl. This program runs the derive's code generator on a
model and prints that block:

```rust
use syn::{DeriveInput, parse_quote};

fn main() {
    let input: DeriveInput = parse_quote! {
        #[shelf(table = "products")]
        pub struct Product {
            #[shelf(primary_key, generated)]
            pub id: i64,
            pub sku: String,
            pub price_cents: i64,
            note: Option<String>,
            #[shelf(json)]
            pub attributes: Attributes,
        }
    };
    let code = shelf_playground::pretty(shelf_codegen::table::expand(&input).unwrap());
    // The constants are the last item: the inherent impl after `impl Table`.
    let start = code.rfind("#[automatically_derived]").unwrap();
    println!("{}", &code[start..]);

    assert!(code.contains("pub const PRICE_CENTS: ::shelf::query::Column<Self, i64>"));
    assert!(code.contains("    const NOTE: ::shelf::query::Column<Self, Option<String>>"));
}
```

```text
#[automatically_derived]
#[allow(dead_code)]
impl Product {
    /// The column `"id"`, for the query builder.
    pub const ID: ::shelf::query::Column<Self, i64> = ::shelf::query::Column::new(
        "\"id\"",
    );
    /// The column `"sku"`, for the query builder.
    pub const SKU: ::shelf::query::Column<Self, String> = ::shelf::query::Column::new(
        "\"sku\"",
    );
    /// The column `"price_cents"`, for the query builder.
    pub const PRICE_CENTS: ::shelf::query::Column<Self, i64> = ::shelf::query::Column::new(
        "\"price_cents\"",
    );
    /// The column `"note"`, for the query builder.
    const NOTE: ::shelf::query::Column<Self, Option<String>> = ::shelf::query::Column::new(
        "\"note\"",
    );
    /// The column `"attributes"`, for the query builder.
    pub const ATTRIBUTES: ::shelf::query::Column<
        Self,
        ::shelf::__private::serde_json::Value,
    > = ::shelf::query::Column::new("\"attributes\"");
}
```

Four things are decided here:

- **The value stored is the quoted name, and nothing else.** `Column::new("\"price_cents\"")`
  is a `const fn`, so the constant is a `&'static str` in the binary, quoted at macro time like
  every name in Chapter 6's statements.
- **`T` is what the column holds on the Rust side**, which is not always the field's type. A
  `json` field reads JSON into a struct, but a condition on the column compares JSON, so its
  `T` is `serde_json::Value`; a `try_from = i32` field reads an `i32` and converts it, so its
  `T` is `i32`. A `skip` field has no column and no constant.
- **A column is as visible as its field.** `note` is private, and so is `NOTE`: a module that
  cannot read the field cannot filter by it either.
- **`#[allow(dead_code)]`**: a model has a constant per column, and most programs use a few.

`Column<M, T>` itself is a small struct with two type parameters that it never stores:

~~~rust
// orm/shelf/src/query/column.rs (excerpt)
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
    // … asc and desc (§10.5)
}
~~~

[orm/shelf/src/query/column.rs](../shelf/src/query/column.rs)

> **Rust refresher:** a struct that mentions a type parameter only in its methods' signatures
> does not compile: every parameter must be used by a field (E0392). `PhantomData<X>` is a field
> of size zero that counts as holding an `X` for the type checker, and holds nothing at run
> time. *Which* `X` matters, because the struct inherits `X`'s properties. `PhantomData<(M, T)>`
> would make `Column` `Send` only when `M` and `T` are, and a derived `Clone` would ask for
> `M: Clone`. `PhantomData<fn() -> (M, T)>` says "something that produces an `M` and a `T`": a
> function pointer is always `Send`, `Sync` and `Copy`, and it varies with `M` and `T` as they
> would. The [Rustonomicon's table](https://doc.rust-lang.org/nomicon/phantom-data.html#table-of-phantomdata-patterns)
> lists the patterns and what each implies.

A stage program shows what the marker buys: the column is the size of its name, columns of two
models are different types, and a model that is not `Send` still has `Send` columns:

```rust
use std::marker::PhantomData;
use std::rc::Rc;

/// The stage column: a name, and two types that exist for the compiler alone.
struct Column<M, T> {
    name: &'static str,
    _model: PhantomData<fn() -> (M, T)>,
}

impl<M, T> Column<M, T> {
    const fn new(name: &'static str) -> Self {
        Column {
            name,
            _model: PhantomData,
        }
    }
}

struct Product;
struct Order;
/// A model that is neither `Send` nor `Sync`: it holds an `Rc`.
#[allow(dead_code)]
struct Draft(Rc<String>);

const PRICE_CENTS: Column<Product, i64> = Column::new("\"price_cents\"");
const TOTAL_CENTS: Column<Order, i64> = Column::new("\"total_cents\"");
const TITLE: Column<Draft, String> = Column::new("\"title\"");

/// Takes a column of `Product`'s, and of no other model's.
fn product_column<T>(column: Column<Product, T>) -> &'static str {
    column.name
}

fn shareable<X: Send + Sync + 'static>(_: X) {}

fn main() {
    // The marker takes no space: a column is its name.
    assert_eq!(size_of::<Column<Product, i64>>(), size_of::<&str>());

    assert_eq!(product_column(PRICE_CENTS), "\"price_cents\"");
    // product_column(TOTAL_CENTS) is E0308: expected `Column<Product, _>`, found
    // `Column<Order, i64>`.
    assert_eq!(TOTAL_CENTS.name, "\"total_cents\"");

    // `fn() -> (Draft, String)` is Send and Sync, though `Draft` is neither.
    shareable(TITLE);
}
```

### Two constants called `VERSION`

`Table` has a constant `VERSION` (Chapter 9: the version column, if any), and a model with a
field called `version` gets a column constant `VERSION` too. Both live on `Product`, and Rust
allows it: an inherent constant shadows a trait's constant of the same name, so `Product::VERSION`
is the column, and the trait's is reached by its full name.

```rust
use shelf::{FromRow, Table};

#[derive(Debug, FromRow, Table)]
#[shelf(table = "products")]
#[allow(dead_code)] // the program only prints constants
struct Product {
    #[shelf(primary_key)]
    id: i64,
    sku: String,
    #[shelf(version)]
    version: i32,
}

fn main() {
    println!("Product::VERSION             {:?}", Product::VERSION);
    println!(
        "<Product as Table>::VERSION  {:?}",
        <Product as Table>::VERSION
    );

    assert_eq!(Product::VERSION.name(), r#""version""#);
    assert_eq!(<Product as Table>::VERSION, Some(r#""version""#));
}
```

```text
Product::VERSION             Column("\"version\"")
<Product as Table>::VERSION  Some("\"version\"")
```

This is why the code shelf generates never writes `Product::VERSION` or `Self::TABLE`: it writes
`<Product as ::shelf::Table>::VERSION`, which means the trait's constant whatever the user's
fields are called. A field named `table`, `columns` or `find` is then harmless. (shelf's generic
code may write `M::VERSION`: a type parameter has no inherent constants to shadow the trait's.)

> **Macro lens:** the derive names each constant with `format_ident!("{}", …, span =
> ident.span())`, so the name carries the field's span. That does not move every error to the
> field. Two fields that differ only in case, `sku` and a `#[shelf(rename = "SKU")] SKU`, both
> become the constant `SKU`, and rustc 1.99.1 reports the clash at the derive:
>
> ```text
> error[E0592]: duplicate definitions with name `SKU`
>  --> user/src/bin/case.rs:3:19
>   |
> 3 | #[derive(FromRow, Table)]
>   |                   ^^^^^
>   |                   |
>   |                   duplicate definitions for `SKU`
>   |                   other definition for `SKU`
> ```
>
> A diagnostic about a whole item uses the item's span, and most of the item's tokens are the
> macro's own. Rust's naming lint already warns about an upper-case field, so shelf leaves this
> rare case to the compiler.

### What a condition accepts

`PRICE_CENTS.gt(…)` must take an `i64`. The obvious signature, `gt(self, value: T)`, is too
strict: a `&String` or a `&str` for a text column would need `.clone()` or `.to_owned()` at
every call. shelf's methods take `impl Value<T>`, a trait of its own that says "a value for a
column whose values are `T`", with five impls:

~~~rust
// orm/shelf/src/query/column.rs (excerpt)
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
// …
impl<T: Clone> Value<T> for &T {
// …
impl Value<String> for &str {
// …
impl<T> Value<Option<T>> for T {
// …
impl Value<Option<String>> for &str {
~~~

[orm/shelf/src/query/column.rs](../shelf/src/query/column.rs)

The list is short on purpose, and the reason was found on the compiler, not guessed. It seems
friendly to accept an `i32` for a `bigint` column, since every `i32` fits. One impl more does
it:

~~~rust
// a stage program, compiled with rustc 1.99.1
impl Value<i64> for i32 {
    fn into_value(self) -> i64 {
        self.into()
    }
}

let price_cents: Column<i64> = Column { _value: PhantomData };
let _ = price_cents.gt(3_000_000_000);
~~~

```text
error: literal out of range for `i32`
  --> widening.rs:32:28
   |
32 |     let _ = price_cents.gt(3_000_000_000);
   |                            ^^^^^^^^^^^^^
   |
   = note: the literal `3_000_000_000` does not fit into the type `i32` whose range is `-2147483648..=2147483647`
   = help: consider using the type `u32` instead
   = note: `#[deny(overflowing_literals)]` on by default
```

Without the extra impl, the program compiles. An integer literal has no type of its own; the
compiler infers one from what it must satisfy. With a single candidate, `Value<i64> for i64`,
the literal is an `i64`. With two candidates, `i64` and `i32`, inference cannot choose, and an
ambiguous integer literal falls back to `i32`, which `3_000_000_000` does not fit. So the
friendly impl breaks every large literal on every `bigint` column. shelf converts no numbers: a
literal always takes the column's type, and an `i32` variable is a compile error you fix with
`i64::from(cents)` (§10.6).

## 10.3 Conditions

Each condition method returns an `Expr<M>`, a condition on model `M`'s rows. They are the
SQL comparisons, defined on the columns where they make sense:

| Method | SQL | On a column of type |
|---|---|---|
| `eq`, `ne`, `lt`, `le`, `gt`, `ge` | `"col" = $n`, `<>`, `<`, `<=`, `>`, `>=` | any |
| `between(low, high)` | `"col" BETWEEN $n AND $m` (both ends included) | any |
| `any(values)` | `"col" = ANY($n)`, the values as one array | any |
| `like(pattern)` | `"col" LIKE $n` | `String`, `Option<String>` |
| `is_null()`, `is_not_null()` | `"col" IS NULL`, `IS NOT NULL` | `Option<T>` |
| `contains(element)` | `"col" @> $n`, a one-element array | `Vec<E>`, `Option<Vec<E>>` |
| `a.and(b)`, `a.or(b)`, `!a` | `a AND b`, `a OR b`, `NOT (a)` | conditions on one model |

"Defined where they make sense" is literal: `like` is in an `impl<M> Column<M, String>` block,
`is_null` in an `impl<M, T> Column<M, Option<T>>` block, `contains` in an `impl<M, E> Column<M,
Vec<E>>` block. A `Column<Product, i64>` has no `like` method at all, and §10.6 shows the error.
`not` is the `!` operator, through `std::ops::Not`, because a method called `not` would trip
clippy's `should_implement_trait`.

`any` deserves a word. `"sku" = ANY($1)` with one array parameter is the same statement for two
values or two hundred, where `IN ($1, $2, …)` would be a new statement for every length (main
book [§3.8](../../book/03-rows-and-types.md#38-arrays)). Chapter 11's relation loaders are built
on it.

### A tree, rendered

Conditions combine into a tree: `and` and `or` nodes over comparisons. Turning the tree into SQL
is where hand-written query assembly goes wrong, so it is worth seeing whole. This stage program
has three kinds of node and one function that writes the text and the parameter list together:

```rust
use std::fmt::Write;
use tokio_postgres::types::ToSql;

/// A value the tree owns.
type Param = Box<dyn ToSql + Sync + Send>;

/// The stage tree: comparisons, combined with AND and OR.
enum Node {
    Compare {
        column: &'static str,
        op: &'static str,
        value: Param,
    },
    And(Vec<Node>),
    Or(Vec<Node>),
}

fn compare(
    column: &'static str,
    op: &'static str,
    value: impl ToSql + Sync + Send + 'static,
) -> Node {
    Node::Compare {
        column,
        op,
        value: Box::new(value),
    }
}

/// SQL text, and the parameters its `$n` refer to, borrowed from the tree.
struct Sql<'q> {
    text: String,
    params: Vec<&'q (dyn ToSql + Sync)>,
}

impl<'q> Sql<'q> {
    fn node(&mut self, node: &'q Node) {
        match node {
            Node::Compare { column, op, value } => {
                self.params.push(&**value);
                // The number is the list's length: text and list cannot drift apart.
                write!(self.text, "{column} {op} ${}", self.params.len()).unwrap();
            }
            Node::And(nodes) => self.join(nodes, " AND ", |n| matches!(n, Node::Or(_))),
            Node::Or(nodes) => self.join(nodes, " OR ", |n| matches!(n, Node::And(_))),
        }
    }

    /// The children of an AND or an OR; a child of the other kind goes in parentheses.
    fn join(&mut self, nodes: &'q [Node], separator: &str, nested: fn(&Node) -> bool) {
        for (i, node) in nodes.iter().enumerate() {
            if i > 0 {
                self.text.push_str(separator);
            }
            if nested(node) {
                self.text.push('(');
                self.node(node);
                self.text.push(')');
            } else {
                self.node(node);
            }
        }
    }
}

fn main() {
    // (price < 1000 OR stock > 0) AND sku <> 'MUG-01'
    let tree = Node::And(vec![
        Node::Or(vec![
            compare("\"price_cents\"", "<", 1_000_i64),
            compare("\"stock\"", ">", 0_i32),
        ]),
        compare("\"sku\"", "<>", String::from("MUG-01")),
    ]);
    let mut sql = Sql {
        text: String::from("WHERE "),
        params: Vec::new(),
    };
    sql.node(&tree);
    println!("{}", sql.text);
    println!("{:?}", sql.params);

    assert_eq!(
        sql.text,
        r#"WHERE ("price_cents" < $1 OR "stock" > $2) AND "sku" <> $3"#
    );
    assert_eq!(sql.params.len(), 3);
}
```

```text
WHERE ("price_cents" < $1 OR "stock" > $2) AND "sku" <> $3
[1000, 0, "MUG-01"]
```

Three properties make the renderer correct, and shelf's has all three:

- **One function writes both.** The `$n` is the length of the list right after the value was
  pushed, so the text cannot refer to a parameter the list does not hold, or hold one the text
  does not use. Hand-written assembly keeps a counter beside the list, and every branch that
  forgets to bump it is a bug.
- **Precedence is written out.** `AND` binds tighter than `OR` in SQL, so an `OR` inside an
  `AND` needs parentheses, and the renderer adds them exactly there. Without them, the tree above
  would mean `price < 1000 OR (stock > 0 AND sku <> 'MUG-01')`, a different query that still
  runs.
- **Rendering borrows; it does not copy.** The tree owns its values, and the parameter list
  borrows them, `&'q (dyn ToSql + Sync)`, for as long as the statement runs. Rendering a query
  twice costs two strings, not two copies of its values.

shelf's `Node` adds `BETWEEN`, `= ANY`, `IS NULL`, `NOT` and raw SQL (§10.7), and flattens as it
builds: `a.and(b).and(c)` is one `And` with three children, not two nested ones. Here are its
conditions, rendered:

```rust
use shelf::{FromRow, Table};

#[derive(Debug, FromRow, Table)]
#[shelf(table = "products")]
#[allow(dead_code)] // the program only renders queries
struct Product {
    #[shelf(primary_key)]
    id: i64,
    sku: String,
    price_cents: i64,
    tags: Vec<String>,
    note: Option<String>,
    stock: Option<i32>,
}

fn main() {
    let queries = [
        Product::query().filter(
            Product::PRICE_CENTS
                .lt(1_000)
                .or(Product::NOTE.is_not_null().and(Product::STOCK.gt(0))),
        ),
        Product::query().filter(!Product::SKU.eq("MUG-01")),
        Product::query().filter(Product::SKU.any(["TEE-01", "CAP-01"])),
        Product::query().filter(Product::PRICE_CENTS.between(1_000, 2_500)),
        Product::query().filter(
            Product::NOTE
                .like("%size%")
                .and(Product::TAGS.contains("sale")),
        ),
        Product::query()
            .filter(Product::STOCK.is_null().or(Product::STOCK.eq(0)))
            .filter(Product::PRICE_CENTS.ge(500)),
    ];
    for query in &queries {
        let sql = query.sql();
        // Print what follows the column list, which is the same every time.
        println!("{}", &sql[sql.find("FROM").unwrap()..]);
        println!("    {:?}", query.params());
    }

    let last = queries[5].sql();
    assert!(last.ends_with(
        r#"WHERE ("stock" IS NULL OR "stock" = $1) AND "price_cents" >= $2 ORDER BY "id""#
    ));
}
```

```text
FROM "products" WHERE "price_cents" < $1 OR ("note" IS NOT NULL AND "stock" > $2) ORDER BY "id"
    [1000, Some(0)]
FROM "products" WHERE NOT ("sku" = $1) ORDER BY "id"
    ["MUG-01"]
FROM "products" WHERE "sku" = ANY($1) ORDER BY "id"
    [["TEE-01", "CAP-01"]]
FROM "products" WHERE "price_cents" BETWEEN $1 AND $2 ORDER BY "id"
    [1000, 2500]
FROM "products" WHERE "note" LIKE $1 AND "tags" @> $2 ORDER BY "id"
    ["%size%", ["sale"]]
FROM "products" WHERE ("stock" IS NULL OR "stock" = $1) AND "price_cents" >= $2 ORDER BY "id"
    [Some(0), 500]
```

Three details show in the output. `STOCK.gt(0)` on an `Option<i32>` column took a plain `0`,
and sent it as `Some(0)`: a nullable column's values are `Option`s, and `impl<T> Value<Option<T>>
for T` wraps a plain value. `contains("sale")`
sent a one-element array, `["sale"]`, because `@>` compares two arrays. And two `filter` calls
are joined by `AND`, with the `OR` of the first in parentheses: each filter is a condition the
rows must meet, whatever is inside it.

> **Pitfall:** `Product::NOTE.eq(None)` compiles, since `None` is a value of the column's type,
> and renders `"note" = $1` with a NULL parameter. In SQL, `NULL = NULL` is not true but NULL,
> so the condition matches no row at all, not the rows whose note is NULL. Use `is_null()`. The
> same goes for `ne`: `"note" <> $1` is NULL for a row whose note is NULL, so that row is left
> out of both `eq(x)` and `ne(x)`. The next program checks both.

### How PostgreSQL types the parameters

The builder sends each value as a parameter and PostgreSQL decides its type, as it does for any
statement (main book [§4.3](../../book/04-prepared-statements.md#43-when-the-server-cannot-infer-a-type)).
The values must then match those types, so it is worth knowing what the builder's operators
produce, against a table with a domain:

```rust
use shelf::{FromRow, Table};

#[derive(Debug, FromRow, Table)]
#[shelf(table = "products", schema = "shelf_ch10_types")]
struct Product {
    #[shelf(primary_key, generated)]
    id: i64,
    sku: String, // a domain column, read as its base type, text
    price_cents: i64,
    tags: Vec<String>,
    note: Option<String>,
}

#[tokio::main]
async fn main() -> Result<(), shelf::Error> {
    let client = shelf_playground::connect().await?;
    client
        .batch_execute(
            "DROP SCHEMA IF EXISTS shelf_ch10_types CASCADE;
             CREATE SCHEMA shelf_ch10_types;
             CREATE DOMAIN shelf_ch10_types.sku AS text CHECK (VALUE ~ '^[A-Z]+-[0-9]+$');
             CREATE TABLE shelf_ch10_types.products (
                 id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
                 sku shelf_ch10_types.sku NOT NULL,
                 price_cents bigint NOT NULL,
                 tags text[] NOT NULL,
                 note text
             );
             INSERT INTO shelf_ch10_types.products (sku, price_cents, tags, note) VALUES
                 ('TEE-01', 2499, '{sale,cotton}', NULL), ('MUG-01', 1299, '{}', 'fragile'),
                 ('CAP-01', 1899, '{sale}', NULL);",
        )
        .await?;

    let query = Product::query()
        .filter(
            Product::SKU
                .like("TEE-%")
                .or(Product::SKU.any(["MUG-01", "CAP-01"])),
        )
        .filter(Product::PRICE_CENTS.between(1_000, 3_000))
        .filter(Product::TAGS.contains("sale"))
        .limit(10)
        .offset(0);
    let statement = client.prepare(&query.sql()).await?;
    for (i, ty) in statement.params().iter().enumerate() {
        println!("${} {}", i + 1, ty.name());
    }

    // `&str` was accepted for the domain column: comparisons type it as text.
    let found = query.all(&client).await?;
    let skus: Vec<&str> = found.iter().map(|p| p.sku.as_str()).collect();
    assert_eq!(skus, ["TEE-01", "CAP-01"]);
    let types: Vec<&str> = statement.params().iter().map(|t| t.name()).collect();
    assert_eq!(
        types,
        ["text", "_text", "int8", "int8", "_text", "int8", "int8"]
    );

    // §10.3's pitfall: `= NULL` matches nothing, and `<>` leaves the NULLs out too.
    let count = async |condition| Product::query().filter(condition).count(&client).await;
    assert_eq!(count(Product::NOTE.eq(None)).await?, 0);
    assert_eq!(count(Product::NOTE.is_null()).await?, 2);
    assert_eq!(count(Product::NOTE.eq("fragile")).await?, 1);
    assert_eq!(count(Product::NOTE.ne("fragile")).await?, 0);

    client
        .batch_execute("DROP SCHEMA shelf_ch10_types CASCADE")
        .await?;
    Ok(())
}
```

```text
$1 text
$2 _text
$3 int8
$4 int8
$5 _text
$6 int8
$7 int8
```

`LIKE` and `BETWEEN` take the column's type, `= ANY` an array of it, and `@>` the array column's
own type (`_text` is `text[]`), and `LIMIT` and `OFFSET` take `int8`, which is why `limit` and `offset` take an
`i64`. Against the domain column `sku`, both `LIKE $1` and `= ANY($2)` typed the parameter as
the base type, `text`, and `_text`: comparisons look through a domain. That is unlike an `INSERT`
or a `SET col = $n`, which type it as the domain itself and refuse a `String` (Chapter 8,
§8.2), and it is why the capstone's `Product::SKU.eq(sku)` can send stockroom's `Sku`, whose
`accepts` takes `text` as well as the domain.

## 10.4 Owned values

`Expr` and `Query` own their values, as `Box<dyn ToSql + Sync + Send>`. The alternative,
borrowing them, would save one allocation per value and put a lifetime on every query:
`Query<'a, M>`, alive only while the values it points to are. Ownership is what lets a query be
built from a request in one function and run in another, after an `.await`, or in another
task:

```rust
use shelf::query::Query;
use shelf::{FromRow, Table};

#[derive(Debug, FromRow, Table)]
#[shelf(table = "products")]
struct Product {
    #[shelf(primary_key)]
    id: i64,
    sku: String,
    tags: Vec<String>,
}

/// Builds the query from a request's parameter, which it does not keep.
fn tagged(tag: String) -> Query<Product> {
    Product::query().filter(Product::TAGS.contains(&tag))
    // `tag` is dropped here; the query holds its own copy.
}

/// Compiles only if the builder's futures are `Send` for any executor, as `tokio::spawn` and
/// axum need.
#[allow(dead_code)]
fn futures_are_send<E: shelf::Executor>(db: &E, query: &Query<Product>) {
    fn send<X: Send>(_: X) {}
    send(query.all(db));
    send(query.count(db));
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // One connection, so the spawned task sees this session's temporary table.
    let pool = shelf_playground::pool(1);
    pool.get()
        .await?
        .batch_execute(
            "CREATE TEMP TABLE products (id bigint PRIMARY KEY, sku text NOT NULL, tags text[]);
             INSERT INTO products VALUES (1, 'TEE-01', '{sale}'), (2, 'MUG-01', '{}'),
                                         (3, 'CAP-01', '{sale}');",
        )
        .await?;

    let query = tagged(String::from("sale"));
    let task = tokio::spawn({
        let pool = pool.clone();
        async move {
            let client = pool.get().await?;
            anyhow::Ok(query.all(&client).await?)
        }
    });
    let products = task.await??;
    let skus: Vec<&str> = products.iter().map(|p| p.sku.as_str()).collect();
    assert_eq!(skus, ["TEE-01", "CAP-01"]);
    Ok(())
}
```

`tokio::spawn` takes a `'static` future, so the query must own everything it carries: with
borrowed values, `tagged` could not return it at all, since `tag` dies at its end. The cost is
one small allocation per value. For scale, Chapter 5 measured a whole `resolve` of twelve fields
at 262–268 ns and a localhost round trip at 46–47 µs.

`futures_are_send` is a check that runs at compile time only. `all`, `first`, `count`, `exists`
and `stream` are inherent `async fn`s on `Query`, and an inherent `async fn` leaks its future's
auto traits: the compiler sees the body, so for a generic `E: Executor`, whose futures the trait
declares `Send` (Chapter 7, §7.2), the query's future is `Send` too. Chapter 7's caveat applies
only to `async fn` *in a trait*, where a generic caller sees the declaration and not the body.
That is also why clippy's `manual_async_fn` lint prefers the `async fn` form here.

## 10.5 Order, limit and pages

The rest of a page is ordering and bounds:

| Method | SQL | Notes |
|---|---|---|
| `order_by(Product::SKU.asc())`, `.desc()` | `ORDER BY "sku"`, `"sku" DESC` | several calls add terms; with none, the key: `ORDER BY "id"` |
| `limit(n)`, `offset(n)` | `LIMIT $n`, `OFFSET $n` | parameters, so a page size is not a new statement |
| `first()` | `LIMIT 1` | `Option<M>`, in place of any `limit` |
| `count()` | `SELECT count(*) FROM … WHERE …` | the filter alone: no order, no paging |
| `exists()` | `SELECT EXISTS (SELECT 1 FROM … WHERE …)` | the same |
| `after(column, value)` | `"col" > $n`, ordered by the column first | keyset pages |

A query without `order_by` is ordered by its key, as `Table::all` is (Chapter 6, Exercise 3: a
table scan returns rows in whatever order it finds them). The methods that run a query borrow it,
`&self`, so one filter can be counted and then paged:

```rust
use shelf::{FromRow, Table};

#[derive(Debug, FromRow, Table)]
#[shelf(table = "products")]
#[allow(dead_code)] // `tags` is only filtered on
struct Product {
    #[shelf(primary_key)]
    id: i64,
    sku: String,
    price_cents: i64,
    tags: Vec<String>,
}

#[tokio::main]
async fn main() -> Result<(), shelf::Error> {
    let client = shelf_playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE products (
                 id bigint PRIMARY KEY, sku text NOT NULL, price_cents bigint NOT NULL,
                 tags text[] NOT NULL
             );
             INSERT INTO products VALUES
                 (1, 'TEE-01', 2499, '{sale}'), (2, 'MUG-01', 1299, '{}'),
                 (3, 'CAP-01', 1899, '{sale}'), (4, 'HAT-01', 999, '{wool}'),
                 (5, 'BAG-01', 4599, '{sale}');",
        )
        .await?;

    let sale = Product::query()
        .filter(Product::TAGS.contains("sale"))
        .order_by(Product::PRICE_CENTS.desc())
        .limit(2);
    let page = sale.all(&client).await?;
    let skus: Vec<&str> = page.iter().map(|p| p.sku.as_str()).collect();
    println!("page:   {skus:?}");
    println!("count:  {}", sale.count(&client).await?);
    println!("exists: {}", sale.exists(&client).await?);
    let cheapest = Product::query()
        .order_by(Product::PRICE_CENTS.asc())
        .first(&client)
        .await?;
    println!("first:  {:?}", cheapest.as_ref().map(|p| p.sku.as_str()));

    assert_eq!(skus, ["BAG-01", "TEE-01"]);
    assert_eq!(sale.count(&client).await?, 3); // the limit does not apply
    assert!(sale.exists(&client).await?);
    assert_eq!(cheapest.map(|p| p.price_cents), Some(999));
    Ok(())
}
```

```text
page:   ["BAG-01", "TEE-01"]
count:  3
exists: true
first:  Some("HAT-01")
```

`count` returned 3 for a query limited to 2: it counts what the filter matches, which is what a
page's "3 results" needs. `exists` asks the same question and lets the server stop at the first
row.

### One statement per shape

Every distinct SQL text is a distinct prepared statement. A deadpool connection prepares each
text once and caches it (Chapter 7, §7.3), so the question for a builder is how many texts it
produces. The answer is one per *shape*, the filters present and their order, and not one per
value: values, limits and offsets are parameters. Counted on the server, through a pool of one
connection:

```rust
use shelf::query::Query;
use shelf::{FromRow, Table};

#[derive(Debug, FromRow, Table)]
#[shelf(table = "products")]
#[allow(dead_code)] // the program counts statements
struct Product {
    #[shelf(primary_key)]
    id: i64,
    sku: String,
    price_cents: i64,
    tags: Vec<String>,
}

fn page(tag: Option<&str>, max_cents: Option<i64>, limit: i64) -> Query<Product> {
    let mut query = Product::query().order_by(Product::SKU.asc()).limit(limit);
    if let Some(tag) = tag {
        query = query.filter(Product::TAGS.contains(tag));
    }
    if let Some(max) = max_cents {
        query = query.filter(Product::PRICE_CENTS.le(max));
    }
    query
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let pool = shelf_playground::pool(1);
    let client = pool.get().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE products (
                 id bigint PRIMARY KEY, sku text NOT NULL, price_cents bigint NOT NULL,
                 tags text[] NOT NULL
             );
             INSERT INTO products VALUES (1, 'TEE-01', 2499, '{sale}'), (2, 'MUG-01', 1299, '{}');",
        )
        .await?;

    let requests = [
        (None, None, 20),
        (Some("sale"), None, 20),
        (Some("wool"), None, 5),
        (None, Some(2_000), 20),
        (None, Some(1_000), 50),
        (Some("sale"), Some(5_000), 20),
        (Some("cotton"), Some(900), 1),
    ];
    for (tag, max_cents, limit) in requests {
        page(tag, max_cents, limit).all(&client).await?;
    }

    // query_typed's unnamed statement is not listed, so it does not count itself.
    let rows = client
        .query_typed("SELECT count(*) FROM pg_prepared_statements", &[])
        .await?;
    let prepared: i64 = rows[0].get(0);
    println!(
        "{} requests, {prepared} statements prepared",
        requests.len()
    );
    assert_eq!(prepared, 4);
    Ok(())
}
```

```text
7 requests, 4 statements prepared
```

Two optional filters make four shapes, and each was prepared once, whatever its values and page
size. A builder that wrote values into the SQL, or a `LIMIT 20` as text, would have prepared
seven, and a busy service would prepare without end. The number to keep in mind is the number of
shapes: `n` optional filters give up to 2ⁿ statements per pooled connection, each prepared on
first use, which is the same arithmetic as Chapter 9's design (a) (§9.2). For a handful of
filters that is cheap; for a search form with twenty optional fields, it is an argument for one
fixed statement with optional parameters, the way stockroom writes it (In stockroom, below).

### Keyset pages

`offset` reads and throws away every row before the page, so page 1,000 costs a thousand pages
of work; main book [§9.6](../../book/09-streaming.md#96-keyset-pagination-the-http-answer)
counted it with `EXPLAIN ANALYZE` (99,050 rows read for one page of 50). Keyset pagination asks
for the rows *after* the last one seen instead, which an index answers directly. `after(column,
value)` adds `"col" > $n` and makes sure the rows are ordered by that column first:

```rust
use shelf::{FromRow, Table};

#[derive(Debug, FromRow, Table)]
#[shelf(table = "products")]
struct Product {
    #[shelf(primary_key)]
    id: i64,
    sku: String,
}

#[tokio::main]
async fn main() -> Result<(), shelf::Error> {
    let client = shelf_playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE products (id bigint PRIMARY KEY, sku text NOT NULL UNIQUE);
             INSERT INTO products VALUES (1, 'TEE-01'), (2, 'MUG-01'), (3, 'CAP-01'),
                                         (4, 'HAT-01'), (5, 'BAG-01');",
        )
        .await?;

    let mut seen = Vec::new();
    let mut last: Option<String> = None;
    loop {
        let mut query = Product::query().order_by(Product::SKU.asc()).limit(2);
        if let Some(sku) = &last {
            query = query.after(Product::SKU, sku);
        }
        let page = query.all(&client).await?;
        let Some(end) = page.last() else { break };
        last = Some(end.sku.clone());
        let skus: Vec<String> = page.into_iter().map(|p| p.sku).collect();
        println!("{skus:?}");
        seen.extend(skus);
    }
    let next = Product::query()
        .order_by(Product::SKU.asc())
        .limit(2)
        .after(Product::SKU, "CAP-01");
    println!("{}", &next.sql()[next.sql().find("WHERE").unwrap()..]);

    assert_eq!(seen, ["BAG-01", "CAP-01", "HAT-01", "MUG-01", "TEE-01"]);
    assert!(
        next.sql()
            .ends_with(r#"WHERE "sku" > $1 ORDER BY "sku" LIMIT $2"#)
    );
    Ok(())
}
```

```text
["BAG-01", "CAP-01"]
["HAT-01", "MUG-01"]
["TEE-01"]
WHERE "sku" > $1 ORDER BY "sku" LIMIT $2
```

The query already ordered by `sku`, so `after` did not add a second `ORDER BY "sku"`. The loop
needed a fourth, empty page to learn that the third was the last; main book §9.6's rule, "ask for
one row more than the page", saves it. And the column must be unique: if two products shared a
SKU, the second would be skipped by `"sku" > $1`. For a column with duplicates, such as a price,
the cursor is a pair, `("price_cents", "id") > ($1, $2)`, a row comparison the builder does not
model; Exercise 3 writes it with §10.7's raw SQL.

## 10.6 What fails to compile

The point of the types is the errors, so shelf pins them with a trybuild UI test (Chapter 4,
§4.5): a file of mistakes, compiled, and its errors compared with a `.stderr` file captured from
rustc 1.99.1.

~~~rust
// orm/shelf/tests/ui/query.rs (excerpt)
#[derive(shelf::FromRow, shelf::Table)]
#[shelf(table = "products")]
struct Product {
    #[shelf(primary_key)]
    id: i64,
    sku: String,
    price_cents: i64,
    note: Option<String>,
}

#[derive(shelf::FromRow, shelf::Table)]
#[shelf(table = "orders")]
struct Order {
    #[shelf(primary_key)]
    id: i64,
    total_cents: i64,
}

use shelf::Table;

fn main() {
    let _ = Product::query().filter(Product::PRICE_CENTS.gt("1000"));

    let cents: i32 = 1000;
    let _ = Product::query().filter(Product::PRICE_CENTS.gt(cents));

    let _ = Product::query().filter(Order::TOTAL_CENTS.gt(1000));

    let _ = Product::query().filter(Product::PRICE_CENTS.like("1%"));
    let _ = Product::query().filter(Product::SKU.is_null());
    let _ = Product::query().filter(Product::NOTE.contains("x"));
}
~~~

[orm/shelf/tests/ui/query.rs](../shelf/tests/ui/query.rs)

**A value of the wrong type** is reported in `Value`'s own words, at the value:

```text
error[E0277]: `&str` cannot be compared with a column of type `i64`
  --> tests/ui/query.rs:25:61
   |
25 |     let _ = Product::query().filter(Product::PRICE_CENTS.gt("1000"));
   |                                                          -- ^^^^^^ expected a value of the column's type, `i64`
   |                                                          |
   |                                                          required by a bound introduced by this call
   |
   = help: the trait `Value<i64>` is not implemented for `&str`
   = note: a condition takes the column's own type, a reference to it, or `&str` for text; shelf converts no numbers
```

```text
error[E0277]: `i32` cannot be compared with a column of type `i64`
  --> tests/ui/query.rs:28:61
   |
28 |     let _ = Product::query().filter(Product::PRICE_CENTS.gt(cents));
   |                                                          -- ^^^^^ expected a value of the column's type, `i64`
   |                                                          |
   |                                                          required by a bound introduced by this call
   |
   = help: the trait `Value<i64>` is not implemented for `i32`
   = note: a condition takes the column's own type, a reference to it, or `&str` for text; shelf converts no numbers
```

rustc goes on to list the impls of `Value` it found (`&T`, `&str` for `String`) and the bound
on `gt`; the first lines are the ones a reader needs. The fix for the second is
`gt(i64::from(cents))`, a conversion written where it happens.

**A column of another model** is the error whose wording changed the design. The first version
of `filter` took `impl Into<Expr<M>>`, so that raw SQL could convert into a condition through a
`From<Raw<M>>` impl. A stage program with that signature shows what rustc made of an `Order`
column in a `Product` query:

```text
error[E0277]: the trait bound `Expr<Product>: From<Expr<Order>>` is not satisfied
  --> into_expr.rs:49:29
   |
49 |     let _ = products.filter(total_cents.gt(1000));
   |                      ------ ^^^^^^^^^^^^^^^^^^^^ unsatisfied trait bound
   |                      |
   |                      required by a bound introduced by this call
   |
help: the trait `From<Expr<Order>>` is not implemented for `Expr<Product>`
      but trait `From<Raw<Product>>` is implemented for it
  --> into_expr.rs:21:1
   |
21 | impl<M> From<Raw<M>> for Expr<M> {
   | ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
   = help: for that trait implementation, expected `Raw<Product>`, found `Expr<Order>`
   = note: required for `Expr<Order>` to implement `Into<Expr<Product>>`
```

Every word of it is true, and its advice points at raw SQL, which is the wrong fix. The reader's
mistake is "a condition on another model", and nothing in the error says so. shelf's `filter`
now takes `impl Condition<M>`, a trait of its own with `#[diagnostic::on_unimplemented]`
(Chapter 4, §4.5), implemented for `Expr<M>` and `Raw<M>`:

```text
error[E0277]: `Expr<Order>` is not a condition on `Product`
  --> tests/ui/query.rs:30:37
   |
30 |     let _ = Product::query().filter(Order::TOTAL_CENTS.gt(1000));
   |                              ------ ^^^^^^^^^^^^^^^^^^^^^^^^^^^ a condition on another model's rows
   |                              |
   |                              required by a bound introduced by this call
   |
   = note: a query on `Product` filters by `Product`'s own columns: `Product::COLUMN.eq(…)`
help: the trait `Condition<Product>` is not implemented for `Expr<Order>`
      but trait `Condition<Order>` is implemented for it
```

Now rustc's own help line agrees with the message: the condition is fine for `Order`, and this
query is on `Product`.

**A method the column's type does not have** is E0599, and rustc does the rest:

```text
error[E0599]: no method named `like` found for struct `Column<Product, i64>` in the current scope
  --> tests/ui/query.rs:32:58
   |
32 |     let _ = Product::query().filter(Product::PRICE_CENTS.like("1%"));
   |                                                          ^^^^
   |
help: there is a method `le` with a similar name
```

```text
error[E0599]: no method named `is_null` found for struct `Column<Product, String>` in the current scope
  --> tests/ui/query.rs:33:50
   |
33 |     let _ = Product::query().filter(Product::SKU.is_null());
   |                                                  ^^^^^^^ method not found in `Column<Product, String>`
   |
   = note: the method was found for `Column<M, Option<T>>`
```

```text
error[E0599]: no method named `contains` found for struct `Column<Product, Option<String>>` in the current scope
  --> tests/ui/query.rs:34:51
   |
34 |     let _ = Product::query().filter(Product::NOTE.contains("x"));
   |                                                   ^^^^^^^^ method not found in `Column<Product, Option<String>>`
   |
   = note: the method was found for
           - `Column<M, Option<Vec<E>>>`
           - `Column<M, Vec<E>>`
```

The `note` lines come free from defining each method on the column types it fits: rustc lists
where the method does exist. The first error's suggestion, `le`, is rustc guessing from the
spelling, and wrong here; the other two say exactly what is needed.

What the types cannot catch is everything the compiler does not see: a column whose type changed
in the database (Chapter 13 checks models against the schema at startup), and raw SQL, which is
text.

## 10.7 The escape hatch

The builder covers the filters requests vary. For everything else there are two exits.

**The whole query in SQL.** `fetch_all::<T>(&db, sql, params)` (Chapter 5) runs any statement
and maps it into any `FromRow` struct, model or not. That is the right tool for a report, a join
or a CTE, and nothing about the builder gets in its way.

**A fragment in SQL.** `Expr::raw(sql).bind(value)` is a condition written by hand, with its
values numbered from `$1`, usable wherever a condition is:

```rust
use shelf::query::Expr;
use shelf::{FromRow, Table};

#[derive(Debug, FromRow, Table)]
#[shelf(table = "products")]
struct Product {
    #[shelf(primary_key)]
    id: i64,
    sku: String,
    name: String,
    price_cents: i64,
}

#[tokio::main]
async fn main() -> Result<(), shelf::Error> {
    let client = shelf_playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE products (
                 id bigint PRIMARY KEY, sku text NOT NULL, name text NOT NULL,
                 price_cents bigint NOT NULL
             );
             INSERT INTO products VALUES (1, 'TEE-01', 'Tee', 2499), (2, 'MUG-01', 'Mug', 1299),
                                         (3, 'BAG-01', 'Bag', 4599), (4, 'HAT-01', 'Hat', 999);",
        )
        .await?;

    let query = Product::query()
        .filter(Product::PRICE_CENTS.gt(1_000))
        .filter(
            Expr::raw(r#"lower("name") = lower($1) OR "sku" LIKE $2 || '%'"#)
                .bind("TEE")
                .bind("BAG"),
        );
    let sql = query.sql();
    println!("{}", &sql[sql.find("WHERE").unwrap()..]);
    println!("    {:?}", query.params());

    let found = query.all(&client).await?;
    let skus: Vec<&str> = found.iter().map(|p| p.sku.as_str()).collect();
    assert_eq!(skus, ["TEE-01", "BAG-01"]);
    assert!(sql.contains(r#"AND (lower("name") = lower($2) OR "sku" LIKE $3 || '%')"#));
    Ok(())
}
```

```text
WHERE "price_cents" > $1 AND (lower("name") = lower($2) OR "sku" LIKE $3 || '%') ORDER BY "id"
    [1000, "TEE", "BAG"]
```

The fragment was written with `$1` and `$2`, and the query renumbered them `$2` and `$3`, after
its own `$1`. It also put the fragment in parentheses, since it cannot know the fragment's
precedence: the `OR` inside stays inside. Column names in a fragment are written quoted, as the
table has them; `Product::NAME.name()` gives the quoted name if you prefer to format it in, which
is safe because it is an identifier the derive quoted, never a value.

### Renumbering is a parser's job

Renumbering looks like a search and replace: find each `$` followed by digits, add the offset.
This stage program does exactly that, on a fragment that compares a note with a string that
happens to contain `$5`:

```rust
use shelf::query::Expr;
use shelf::{FromRow, Table};

#[derive(Debug, FromRow, Table)]
#[shelf(table = "products")]
#[allow(dead_code)] // the program only renders queries
struct Product {
    #[shelf(primary_key)]
    id: i64,
    sku: String,
    note: Option<String>,
    price_cents: i64,
}

/// The stage renumbering: every `$` followed by digits, shifted by `offset`. (ASCII only.)
fn renumber_naively(sql: &str, offset: usize) -> String {
    let bytes = sql.as_bytes();
    let (mut out, mut i) = (String::new(), 0);
    while i < bytes.len() {
        if bytes[i] == b'$' && bytes.get(i + 1).is_some_and(u8::is_ascii_digit) {
            let start = i + 1;
            i = start;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            let n: usize = sql[start..i].parse().unwrap();
            out.push_str(&format!("${}", n + offset));
        } else {
            out.push(char::from(bytes[i]));
            i += 1;
        }
    }
    out
}

fn main() {
    let fragment = r#""note" = 'was $5, now $4' AND "sku" = $1"#;

    let naive = renumber_naively(fragment, 1);
    println!("naive: {naive}");

    let query = Product::query()
        .filter(Product::PRICE_CENTS.gt(0))
        .filter(Expr::raw(fragment).bind("TEE-01"));
    let sql = query.sql();
    let shelf = &sql[sql.find("AND (").unwrap() + 5..sql.find(") ORDER").unwrap()];
    println!("shelf: {shelf}");

    assert_eq!(naive, r#""note" = 'was $6, now $5' AND "sku" = $2"#); // the string changed
    assert_eq!(shelf, r#""note" = 'was $5, now $4' AND "sku" = $2"#);
}
```

```text
naive: "note" = 'was $6, now $5' AND "sku" = $2
shelf: "note" = 'was $5, now $4' AND "sku" = $2
```

The naive version changed a string literal, so the query now looks for a note that no row has,
and runs without complaint. shelf's renderer finds placeholders with a small scanner that knows
where SQL text is not code: string literals (`'…'`, with `''` inside, and `E'…'` with backslash
escapes), quoted identifiers (`"…"`), dollar-quoted strings (`$$…$$` and `$tag$…$tag$`), both
kinds of comment (`--` to the end of the line, and `/* … */`, which nest in PostgreSQL), and
identifiers that contain a `$` (`a$1` is a name). Its unit test is the list of cases:

~~~rust
// orm/shelf/src/query/render.rs (excerpt)
    #[test]
    fn quoted_text_and_comments_are_skipped() {
        assert_eq!(numbers("'$1' = $1"), [1]);
        assert_eq!(numbers("'it''s $2' = $1"), [1]);
        assert_eq!(numbers(r"E'\'$2' = $1"), [1]);
        assert_eq!(numbers(r#""$2" = $1"#), [1]);
        assert_eq!(numbers("$$ $2 $$ = $1"), [1]);
        assert_eq!(numbers("$tag$ $2 $x$ $tag$ = $1"), [1]);
        assert_eq!(numbers("$1 -- $2\n = $3"), [1, 3]);
        assert_eq!(numbers("$1 /* $2 /* $3 */ $4 */ = $5"), [1, 5]);
        assert_eq!(numbers("a$1 = $1"), [1]);
    }
~~~

[orm/shelf/src/query/render.rs](../shelf/src/query/render.rs)

The scanner is about a hundred lines of byte matching, and shelf shows it finished rather than
building it here: every branch is one of the cases above, and the file reads top to bottom. It
is not a parser of SQL, and does not need to be. It needs to know only where a `$` is not a
placeholder, and PostgreSQL's
[lexical structure](https://www.postgresql.org/docs/18/sql-syntax-lexical.html) lists those
places.

### A placeholder without a value

A fragment that uses `$2` with one value bound would, once renumbered, silently take whatever
the statement's next parameter is: another condition's value, or the page size. shelf refuses
it where the fragment becomes a condition, with a panic that names the fragment:

<!-- check: should-panic -->
```rust
use shelf::query::Expr;
use shelf::{FromRow, Table};

#[derive(Debug, FromRow, Table)]
#[shelf(table = "products")]
#[allow(dead_code)] // the program panics before any query
struct Product {
    #[shelf(primary_key)]
    id: i64,
    sku: String,
}

fn main() {
    // `$2` has no value: in a query with other parameters, it would take one of theirs.
    let _ = Product::query()
        .limit(20)
        .filter(Expr::raw(r#""sku" = $1 OR "sku" = $2"#).bind("TEE-01"));
}
```

```text
thread 'main' (25976639) panicked at orm/playground/examples/shelf_ch10_14.rs:18:10:
raw SQL `"sku" = $1 OR "sku" = $2` refers to $2, but has 1 bound value(s)
```

A panic, not an `Err`, because this is a programming mistake in a string literal, found the
first time the line runs, like an out-of-bounds index. `#[track_caller]` on `filter` and on the
conversion puts the location at the caller's `filter` call rather than inside shelf.

### Side by side — *a filtered, sorted page*

The task: a page of products, filtered by an optional tag and an optional price limit, sorted
by SKU.

~~~rust
// diesel (illustration)
diesel::table! {
    products (id) {
        id -> Int8,
        sku -> Text,
        price_cents -> Int8,
        tags -> Array<Text>,
    }
}

#[derive(Queryable, Selectable)]
#[diesel(table_name = products)]
struct Product {
    id: i64,
    sku: String,
    price_cents: i64,
    tags: Vec<String>,
}

fn page(tag: Option<&str>, max_cents: Option<i64>) -> products::BoxedQuery<'_, Pg> {
    let mut query = products::table
        .into_boxed()
        .order(products::sku.asc())
        .limit(20);
    if let Some(tag) = tag {
        query = query.filter(products::tags.contains(vec![tag]));
    }
    if let Some(max) = max_cents {
        query = query.filter(products::price_cents.le(max));
    }
    query
}

// with diesel-async
let rows: Vec<Product> = page(Some("sale"), None)
    .select(Product::as_select())
    .load(&mut conn)
    .await?;
~~~

By hand, assembling the statement and its parameters in step, and with shelf, in one program
that checks both return the same rows for every combination:

```rust
use std::fmt::Write;

use shelf::query::Query;
use shelf::{FromRow, Table};
use tokio_postgres::Client;
use tokio_postgres::types::ToSql;

#[derive(Debug, PartialEq, FromRow, Table)]
#[shelf(table = "products")]
struct Product {
    #[shelf(primary_key)]
    id: i64,
    sku: String,
    price_cents: i64,
    tags: Vec<String>,
}

/// By hand: the text and the values grow together, numbered by the list's length.
async fn page_by_hand(
    client: &Client,
    tag: Option<&str>,
    max_cents: Option<i64>,
) -> Result<Vec<Product>, shelf::Error> {
    let mut sql = String::from("SELECT id, sku, price_cents, tags FROM products");
    let mut params: Vec<Box<dyn ToSql + Sync + Send>> = Vec::new();
    let mut conditions = Vec::new();
    if let Some(tag) = tag {
        params.push(Box::new(vec![tag.to_owned()]));
        conditions.push(format!("tags @> ${}", params.len()));
    }
    if let Some(max) = max_cents {
        params.push(Box::new(max));
        conditions.push(format!("price_cents <= ${}", params.len()));
    }
    if !conditions.is_empty() {
        write!(sql, " WHERE {}", conditions.join(" AND ")).unwrap();
    }
    params.push(Box::new(20_i64));
    write!(sql, " ORDER BY sku LIMIT ${}", params.len()).unwrap();

    let params: Vec<&(dyn ToSql + Sync)> = params.iter().map(|p| &**p as _).collect();
    shelf::fetch_all(client, &sql, &params).await
}

/// shelf: the same page, from the column constants.
fn page(tag: Option<&str>, max_cents: Option<i64>) -> Query<Product> {
    let mut query = Product::query().order_by(Product::SKU.asc()).limit(20);
    if let Some(tag) = tag {
        query = query.filter(Product::TAGS.contains(tag));
    }
    if let Some(max) = max_cents {
        query = query.filter(Product::PRICE_CENTS.le(max));
    }
    query
}

#[tokio::main]
async fn main() -> Result<(), shelf::Error> {
    let client = shelf_playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE products (
                 id bigint PRIMARY KEY, sku text NOT NULL, price_cents bigint NOT NULL,
                 tags text[] NOT NULL
             );
             INSERT INTO products VALUES
                 (1, 'TEE-01', 2499, '{sale}'), (2, 'MUG-01', 1299, '{}'),
                 (3, 'CAP-01', 1899, '{sale}'), (4, 'HAT-01', 999, '{wool}');",
        )
        .await?;

    for (tag, max_cents) in [
        (None, None),
        (Some("sale"), None),
        (None, Some(2_000)),
        (Some("sale"), Some(2_000)),
    ] {
        let by_hand = page_by_hand(&client, tag, max_cents).await?;
        let with_shelf = page(tag, max_cents).all(&client).await?;
        assert_eq!(by_hand, with_shelf);
    }
    let both = page(Some("sale"), Some(2_000)).all(&client).await?;
    assert_eq!(
        both.iter().map(|p| p.sku.as_str()).collect::<Vec<_>>(),
        ["CAP-01"]
    );
    Ok(())
}
```

| | by hand | shelf | diesel |
|---|---|---|---|
| **Same** | One statement per combination of filters, every value a `$n` parameter, the page size too | The same | The same |
| **Different** | You keep the text and the list in step, write each column name as a string, and nothing checks a value's type before the query runs | The column constants carry the model and the value type, so a wrong value or another model's column is a compile error; the `$n` and the list come from one renderer | The `table!` macro declares the schema in Rust (SQL types such as `Int8`, checked against the struct by `Selectable`), and conditions are typed by SQL type (`AsExpression<BigInt>`, which an `i32` is not); `into_boxed()` erases the query's type so that filters can be added conditionally, at the cost of a boxed, dynamically dispatched query |

diesel types far more than shelf does: joins, selected columns, aggregates and their results are
all checked by the compiler, which is what its large type-level machinery is for. shelf types
the filters that requests vary, and leaves the rest to SQL. Both reached the same rule for
numbers: diesel's `gt` takes `T: AsExpression<Self::SqlType>`, and only `i64` is an
`AsExpression<BigInt>` (diesel 2.3.14's `type_impls/primitives.rs`).

---

## In shelf

The query renders itself at each call. `render` writes the `SELECT`, each filter (an `OR` or a
raw fragment in parentheses when there are several), and the ordering and bounds, with every
value going through `Sql::param`, the one place a `$n` is written:

~~~rust
// orm/shelf/src/query/mod.rs (excerpt)
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
        // … count and exists stop here
        sql.text.push_str(" ORDER BY ");
        if self.order.is_empty() {
            sql.text.push_str(&M::KEY_COLUMNS.join(", "));
        } else {
            // … each term, with DESC where asked
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
~~~

[orm/shelf/src/query/mod.rs](../shelf/src/query/mod.rs)

~~~rust
// orm/shelf/src/query/render.rs (excerpt)
    /// Appends the next placeholder, `$n`, and its value.
    pub fn param(&mut self, value: &'q (dyn ToSql + Sync)) {
        self.params.push(value);
        write!(self.text, "${}", self.params.len()).expect("writing to a String");
    }
~~~

[orm/shelf/src/query/render.rs](../shelf/src/query/render.rs)

`M::COLUMNS`, `M::TABLE` and `M::KEY_COLUMNS` are Chapter 6's constants, quoted at macro time,
so the only text formatted at run time is identifiers the derive wrote and the numbers of the
placeholders. A raw fragment is copied in with its `$n` renumbered from the same `params.len()`.

## In stockroom

stockroom's `page` has two optional inputs, a cursor and a tag, and keeps one statement for all
four combinations by making each condition a no-op when its parameter is NULL:

~~~rust
// stockroom/src/repo/products.rs (excerpt)
pub async fn page(
    db: &impl GenericClient,
    after: Option<&Sku>,
    tag: Option<&str>,
    limit: i64,
) -> Result<Vec<Product>, AppError> {
    let stmt = db
        .prepare_cached(
            "SELECT id, sku, name, price_cents, tags, attributes, version, created_at, updated_at
             FROM products
             WHERE sku > coalesce($1, '') AND ($2::text IS NULL OR tags @> ARRAY[$2])
             ORDER BY sku
             LIMIT $3",
        )
        .await?;
    from_rows(&db.query(&stmt, &[&after, &tag, &limit]).await?)
}
~~~

[stockroom/src/repo/products.rs](../../stockroom/src/repo/products.rs)

One statement per connection, whatever the request; in exchange, every condition is in the SQL
whether or not the request uses it, and the `$2::text` cast is needed because `$2 IS NULL` alone
gives PostgreSQL no type to infer. The capstone's port writes the same page with the builder,
and pays four statements per connection for four shapes:

~~~rust
// orm/stockroom-shelf/src/repo/products.rs (excerpt)
pub async fn page(
    db: &impl Executor,
    after: Option<&Sku>,
    tag: Option<&str>,
    limit: i64,
) -> Result<Vec<Product>, AppError> {
    let mut query = Product::query().order_by(Product::SKU.asc()).limit(limit);
    if let Some(after) = after {
        query = query.after(Product::SKU, after);
    }
    if let Some(tag) = tag {
        query = query.filter(Product::TAGS.contains(tag));
    }
    query.all(db).await.map_err(app_error)
}
~~~

[orm/stockroom-shelf/src/repo/products.rs](../stockroom-shelf/src/repo/products.rs)

Both are fine for two optional filters. The fixed statement wins as the count grows, the builder
as the filters stop being "absent or equal" and start being ranges, lists and alternatives that
a fixed statement would have to anticipate.

## When not to use the builder

- **Reports, aggregates, joins, CTEs, window functions.** Write the SQL and read it with a
  `FromRow` struct (Part I). The builder selects one model's columns from one table.
- **A fixed query run often.** If a query never changes shape, a constant statement says what it
  does at a glance, and it is the same statement on every connection. `Table::find` is one.
- **Many optional filters.** Each combination is a statement on every pooled connection. Past a
  handful, prefer one statement with `($n IS NULL OR …)` conditions, as stockroom does.
- **A condition on another table.** `EXISTS (SELECT 1 FROM order_lines WHERE …)` is a raw
  fragment, or a sign that the query is a join and belongs in SQL.

## Summary

- A query builder should type what a mistake gets wrong: that a column belongs to the model, that
  a value has the column's type, and that an operator fits the column. Everything else stays SQL.
- `#[derive(Table)]` generates an inherent constant per column, `Product::PRICE_CENTS:
  Column<Product, i64>`, as visible as its field; `T` is what the column holds (`try_from`'s
  source type, `serde_json::Value` for `json`). An inherent constant shadows a trait constant, so
  the code shelf generates names `<Product as ::shelf::Table>::VERSION` in full.
- `PhantomData<fn() -> (M, T)>` makes the column carry two types at no size, and keeps it `Send`,
  `Sync` and `Copy` whatever they are.
- A condition takes `impl Value<T>`: the column's type, a reference to it, `&str` for text, a
  plain value for a nullable column. No numeric conversions: one widening impl makes integer
  literals fall back to `i32`.
- Conditions form a tree, rendered by one function that writes the `$n` and pushes the value
  together, with parentheses where `OR` sits inside `AND`. `eq(None)` matches nothing; use
  `is_null()`.
- The query owns its values (`Box<dyn ToSql + Sync + Send>`), so it can be built in one function
  and run in another task; its inherent `async fn`s are `Send` for a generic executor.
- `limit` and `offset` are parameters, so a page size is not a new statement; each shape of
  filters is one, cached once per pooled connection. `count` and `exists` ignore order and
  paging. `after` gives keyset pages on a unique column.
- Wrong values, other models' columns and missing methods are compile errors; the `Condition`
  trait exists because the `Into` signature's error pointed the wrong way.
- `Expr::raw(sql).bind(v)` is the escape hatch inside a filter, renumbered by a scanner that skips
  quoted text and comments; a placeholder without a value panics where the fragment is used.

## Exercises

1. **`ilike`.** Add a case-insensitive match, `Product::NAME.ilike("%tee%")`, without changing
   shelf.
   *Hint:* an inherent method cannot be added to another crate's type, but a trait of your own
   can be implemented for `Column<M, String>`. Its body can return a `Raw<M>`, built from the
   column's quoted `name()` and one bound value.
2. **An optional range.** Write `in_range(column, low: Option<T>, high: Option<T>) ->
   Option<Expr<M>>` for any column, which gives `BETWEEN` for two bounds, `>=` or `<=` for one,
   and nothing for none, and use it in a page.
   *Hint:* match on the pair; `Value<T>` is implemented for `T` itself.
3. **Keyset pages on a column with duplicates.** Page through products ordered by price, two at
   a time, when several products share a price.
   *Hint:* order by `price_cents` and then `id`, and filter on the pair: a row comparison,
   `("price_cents", "id") > ($1, $2)`, is a raw fragment.

### Solutions

**1.** An extension trait, implemented for text columns, that writes the fragment with the
column's quoted name:

```rust
use shelf::query::{Column, Expr, Raw};
use shelf::{FromRow, Table};

/// `ILIKE` for text columns.
trait ILike<M> {
    fn ilike(self, pattern: &str) -> Raw<M>;
}

impl<M> ILike<M> for Column<M, String> {
    fn ilike(self, pattern: &str) -> Raw<M> {
        // `name()` is an identifier the derive quoted, so formatting it in is safe.
        Expr::raw(format!("{} ILIKE $1", self.name())).bind(pattern.to_owned())
    }
}

#[derive(Debug, FromRow, Table)]
#[shelf(table = "products")]
struct Product {
    #[shelf(primary_key)]
    id: i64,
    name: String,
    price_cents: i64,
}

#[tokio::main]
async fn main() -> Result<(), shelf::Error> {
    let client = shelf_playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE products (id bigint PRIMARY KEY, name text NOT NULL,
                                         price_cents bigint NOT NULL);
             INSERT INTO products VALUES (1, 'Tee', 2499), (2, 'TEE pack', 5999), (3, 'Mug', 1299);",
        )
        .await?;
    let query = Product::query()
        .filter(Product::NAME.ilike("%tee%"))
        .filter(Product::PRICE_CENTS.lt(5_000));
    assert!(
        query
            .sql()
            .contains(r#"WHERE ("name" ILIKE $1) AND "price_cents" < $2"#)
    );
    let found = query.all(&client).await?;
    assert_eq!(
        found.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
        ["Tee"]
    );
    Ok(())
}
```

Returning `Raw<M>` is enough: it is a `Condition<M>`, so `filter`, `and` and `or` take it. The
fragment's `$1` was renumbered as the query's `$1`, since it came first.

**2.** A function over any column, building only the condition the bounds call for:

```rust
use shelf::query::{Column, Expr};
use shelf::{FromRow, Table};
use tokio_postgres::types::ToSql;

/// A range on `column`; `None` when neither bound is given.
fn in_range<M, T: ToSql + Sync + Send + 'static>(
    column: Column<M, T>,
    low: Option<T>,
    high: Option<T>,
) -> Option<Expr<M>> {
    match (low, high) {
        (Some(low), Some(high)) => Some(column.between(low, high)),
        (Some(low), None) => Some(column.ge(low)),
        (None, Some(high)) => Some(column.le(high)),
        (None, None) => None,
    }
}

#[derive(Debug, FromRow, Table)]
#[shelf(table = "products")]
struct Product {
    #[shelf(primary_key)]
    id: i64,
    sku: String,
    price_cents: i64,
}

#[tokio::main]
async fn main() -> Result<(), shelf::Error> {
    let client = shelf_playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE products (id bigint PRIMARY KEY, sku text NOT NULL,
                                         price_cents bigint NOT NULL);
             INSERT INTO products VALUES (1, 'TEE-01', 2499), (2, 'MUG-01', 1299),
                                         (3, 'HAT-01', 999), (4, 'BAG-01', 4599);",
        )
        .await?;
    for (low, high, expected) in [
        (Some(1_000), Some(3_000), vec!["TEE-01", "MUG-01"]),
        (Some(2_000), None, vec!["TEE-01", "BAG-01"]),
        (None, Some(1_000), vec!["HAT-01"]),
        (None, None, vec!["TEE-01", "MUG-01", "HAT-01", "BAG-01"]),
    ] {
        let mut query = Product::query();
        if let Some(range) = in_range(Product::PRICE_CENTS, low, high) {
            query = query.filter(range);
        }
        let found = query.all(&client).await?;
        assert_eq!(
            found.iter().map(|p| p.sku.as_str()).collect::<Vec<_>>(),
            expected
        );
    }
    Ok(())
}
```

Returning `Option<Expr<M>>` keeps the "no filter at all" case honest: no `price_cents >= NULL`,
which would match nothing (§10.3's pitfall).

**3.** The cursor is the last row's price *and* id, compared as a row value, and the order has
the same two columns:

```rust
use shelf::query::Expr;
use shelf::{FromRow, Table};

#[derive(Debug, FromRow, Table)]
#[shelf(table = "products")]
struct Product {
    #[shelf(primary_key)]
    id: i64,
    sku: String,
    price_cents: i64,
}

#[tokio::main]
async fn main() -> Result<(), shelf::Error> {
    let client = shelf_playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE products (id bigint PRIMARY KEY, sku text NOT NULL,
                                         price_cents bigint NOT NULL);
             INSERT INTO products VALUES (1, 'TEE-01', 1299), (2, 'MUG-01', 1299),
                                         (3, 'HAT-01', 999), (4, 'CAP-01', 1299),
                                         (5, 'BAG-01', 4599);",
        )
        .await?;

    let mut seen = Vec::new();
    let mut cursor: Option<(i64, i64)> = None;
    loop {
        let mut query = Product::query()
            .order_by(Product::PRICE_CENTS.asc())
            .order_by(Product::ID.asc())
            .limit(2);
        if let Some((price, id)) = cursor {
            query = query.filter(
                Expr::raw(r#"("price_cents", "id") > ($1, $2)"#)
                    .bind(price)
                    .bind(id),
            );
        }
        let page = query.all(&client).await?;
        let Some(last) = page.last() else { break };
        cursor = Some((last.price_cents, last.id));
        seen.extend(page.into_iter().map(|p| p.sku));
    }
    // The three products at 1299 all appear, in id order; none is skipped or repeated.
    assert_eq!(seen, ["HAT-01", "TEE-01", "MUG-01", "CAP-01", "BAG-01"]);
    Ok(())
}
```

`("price_cents", "id") > ($1, $2)` compares the pair as PostgreSQL compares
[row constructors](https://www.postgresql.org/docs/18/functions-comparisons.html#ROW-WISE-COMPARISON):
a higher price, or the same price and a higher id. With `after(Product::PRICE_CENTS, …)` alone,
the page that ended at `TEE-01` (1299) would have skipped `MUG-01` and `CAP-01`. An index on
`(price_cents, id)` lets the server answer each page directly, as for a single key.

---

Previous: [Chapter 9 — Partial updates and optimistic locking](09-patch-and-versions.md) · Next: [Chapter 11 — Relations without N+1](11-relations.md)
