# Chapter 11 — Relations without N+1

> **Building an ORM on tokio-postgres** · Previous: [Chapter 10 — A typed query builder](10-query-builder.md) · Next: [Chapter 12 — Bulk writes and streams](12-bulk-and-streams.md)

An order has lines. Showing a page of fifty orders with their lines is the textbook case where an
ORM is either a great help or a disaster. The disaster is the obvious loop: fetch the orders,
then, for each one, fetch its lines. That is fifty-one queries, and over a network each one
costs a round trip, so the page that took a few milliseconds on localhost takes more than a
second in production. It is called the N+1 problem, and the cure is older than ORMs: fetch all
the lines of all fifty orders in one query, and sort them into their orders in Rust.

shelf makes the cure the easy path. A `has_many` attribute on `Order` generates a loader that
takes a slice of orders and returns their lines grouped by order, in one query with
`"order_id" = ANY($1)`. This chapter declares relations, measures the loop and the loader
through lagproxy, looks at the two other answers, pipelining and a join, and explains why shelf
has no lazy loading at all.

**In this chapter you will learn:**

- how `has_many` and `belongs_to` declare a relation, and the methods they generate
- how a derive writes code that names another struct's fields, and where its errors land
- what the N+1 problem costs, counted with an executor of your own and timed through lagproxy
- how batched loading with `= ANY($1)` fetches every child in one round trip
- how to pipeline independent loads on one connection, and what pipelining cannot hide
- when a join is the better query, and how to read one
- why shelf has no lazy loading

---

## 11.1 Declaring relations

A relation is two attributes on the struct, one per direction, each naming the other model and
the field that holds the foreign key:

~~~rust
#[derive(FromRow, Table)]
#[shelf(table = "orders", has_many(OrderLine, key = order_id, as = lines))]
struct Order { /* … */ }

#[derive(FromRow, Table)]
#[shelf(table = "order_lines", belongs_to(Order, key = order_id))]
struct OrderLine { /* …, */ order_id: i64 }
~~~

In both, `key` names the same field, `order_lines.order_id`. For `belongs_to` it is a field of
the struct being derived; for `has_many` it is a field of the *other* struct. `as` names the
generated method. `has_many` requires it, because the method's name is a plural and shelf does
not guess plurals (`lines`, but `people`, `categories`, `data`); `belongs_to` defaults to the
model's name in snake case (`OrderLine` gives `order_line`), and takes `as` to choose another. A
model may have several relations, in one attribute or several.

Each relation generates two methods, one for a row and one for a slice of rows:

| On `Order` | Generates | Returns |
|---|---|---|
| `has_many(OrderLine, key = order_id, as = lines)` | `order.lines(&db)` | `Vec<OrderLine>`, in the lines' key order |
| | `Order::load_lines(&db, &orders)` | `Vec<Vec<OrderLine>>`, one per order, aligned with `orders` |
| `belongs_to(Store, key = store_id)` | `order.store(&db)` | `Store` (`Error::NotFound` if there is none) |
| | `Order::load_store(&db, &orders)` | `HashMap<i64, Store>`, by key |
| `belongs_to(Coupon, key = coupon)`, with `coupon: Option<String>` | `order.coupon(&db)` | `Option<Coupon>`, `None` for a NULL |
| | `Order::load_coupon(&db, &orders)` | `HashMap<String, Coupon>` |

The two directions return different shapes on purpose. A `has_many` loader's parents are the
slice you passed, each with its own children, so the result lines up with it: `orders[i]`'s lines
are at `lines[i]`, and the two zip. A `belongs_to` loader's parents are shared: fifty lines may
belong to three orders, so each parent is loaded once and returned in a map by key, which each
child looks up. A nullable foreign key, a field written `Option<…>`, makes the single-row method
return an `Option`, and the loader skip the NULLs.

All of it at work, on four small tables:

```rust
use shelf::{FromRow, Table};

#[derive(Debug, FromRow, Table)]
#[shelf(table = "stores")]
struct Store {
    #[shelf(primary_key)]
    id: i64,
    name: String,
}

#[derive(Debug, FromRow, Table)]
#[shelf(table = "coupons")]
struct Coupon {
    #[shelf(primary_key)]
    code: String,
    percent: i32,
}

#[derive(Debug, FromRow, Table)]
#[shelf(table = "orders", has_many(OrderLine, key = order_id, as = lines))]
#[shelf(belongs_to(Store, key = store_id), belongs_to(Coupon, key = coupon))]
struct Order {
    #[shelf(primary_key)]
    id: i64,
    store_id: i64,
    coupon: Option<String>,
}

#[derive(Debug, FromRow, Table)]
#[shelf(table = "order_lines", belongs_to(Order, key = order_id))]
struct OrderLine {
    #[shelf(primary_key, generated)]
    id: i64,
    order_id: i64,
    sku: String,
    quantity: i32,
}

#[tokio::main]
async fn main() -> Result<(), shelf::Error> {
    let client = shelf_playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE stores (id bigint PRIMARY KEY, name text NOT NULL);
             CREATE TEMP TABLE coupons (code text PRIMARY KEY, percent integer NOT NULL);
             CREATE TEMP TABLE orders (
                 id bigint PRIMARY KEY,
                 store_id bigint NOT NULL REFERENCES stores,
                 coupon text REFERENCES coupons
             );
             CREATE TEMP TABLE order_lines (
                 id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
                 order_id bigint NOT NULL REFERENCES orders,
                 sku text NOT NULL,
                 quantity integer NOT NULL
             );
             INSERT INTO stores VALUES (1, 'Berlin'), (2, 'Hamburg');
             INSERT INTO coupons VALUES ('TEN', 10);
             INSERT INTO orders VALUES (10, 1, NULL), (11, 2, 'TEN'), (12, 1, NULL);
             INSERT INTO order_lines (order_id, sku, quantity) VALUES
                 (10, 'TEE-01', 1), (11, 'MUG-01', 2), (10, 'CAP-01', 3), (11, 'TEE-01', 1);",
        )
        .await?;

    let orders = Order::all(&client).await?;
    // One query for the lines of every order, one for the stores they point at.
    let lines = Order::load_lines(&client, &orders).await?;
    let stores = Order::load_store(&client, &orders).await?;
    for (order, lines) in orders.iter().zip(&lines) {
        let items: Vec<String> = lines
            .iter()
            .map(|l| format!("{}×{}", l.sku, l.quantity))
            .collect();
        let store = &stores[&order.store_id].name;
        println!("order {} ({store}): {items:?}", order.id);
    }

    // One row's relations, one query each.
    let first = orders[0].lines(&client).await?;
    let coupon = orders[1].coupon(&client).await?;
    let none = orders[0].coupon(&client).await?;
    let order = first[0].order(&client).await?;
    println!("order 11's coupon: {coupon:?}");

    assert_eq!(lines.len(), orders.len());
    assert!(lines[2].is_empty()); // order 12 has no lines, and keeps its place
    assert_eq!(stores.len(), 2);
    assert_eq!(first.len(), 2);
    assert_eq!(coupon.map(|c| c.percent), Some(10));
    assert!(none.is_none());
    assert_eq!(order.id, 10);

    let all_lines = OrderLine::all(&client).await?;
    let their_orders = OrderLine::load_order(&client, &all_lines).await?;
    assert_eq!(their_orders.len(), 2); // four lines, two orders, each loaded once
    Ok(())
}
```

```text
order 10 (Berlin): ["TEE-01×1", "CAP-01×3"]
order 11 (Hamburg): ["MUG-01×2", "TEE-01×1"]
order 12 (Berlin): []
order 11's coupon: Some(Coupon { code: "TEN", percent: 10 })
```

`coupon` is both a field and a method of `Order`: `order.coupon` is the code, `order.coupon(&db)`
the row. Rust keeps fields and methods apart, so the derive can name the method after the model
as `belongs_to` promises, even when the foreign key field has the same name.

### What the derive writes

The generated methods are short, because the work is in shelf's loaders (§11.3). This program
runs the derive's code generator on `Order` and prints the two methods `has_many` adds:

```rust
use syn::{DeriveInput, parse_quote};

fn main() {
    let input: DeriveInput = parse_quote! {
        #[shelf(table = "orders", has_many(OrderLine, key = order_id, as = lines))]
        pub struct Order {
            #[shelf(primary_key)]
            pub id: i64,
            pub store_id: i64,
        }
    };
    let code = shelf_playground::pretty(shelf_codegen::table::expand(&input).unwrap());
    let start = code.find("    /// The `OrderLine` rows whose").unwrap();
    println!("{}", &code[start..]);

    assert!(code.contains("::shelf::__private::children("));
    assert!(code.contains("OrderLine::ORDER_ID,"));
    assert!(code.contains("|child: &OrderLine| { &child.order_id }"));
}
```

```text
    /// The `OrderLine` rows whose `order_id` holds this `Order`'s key, in their key order: one query.
    pub fn lines<'a, E: ::shelf::Executor>(
        &'a self,
        db: &'a E,
    ) -> impl ::core::future::Future<
        Output = ::core::result::Result<::std::vec::Vec<OrderLine>, ::shelf::Error>,
    > + ::core::marker::Send + 'a {
        ::shelf::__private::children(
            db,
            OrderLine::ORDER_ID,
            <Self as ::shelf::Table>::key(self),
        )
    }
    /// The `OrderLine` rows of each of `parents`, in one query (`= ANY($1)`): the `Vec` at `i` holds `parents[i]`'s.
    pub fn load_lines<'a, E: ::shelf::Executor>(
        db: &'a E,
        parents: &'a [Self],
    ) -> impl ::core::future::Future<
        Output = ::core::result::Result<
            ::std::vec::Vec<::std::vec::Vec<OrderLine>>,
            ::shelf::Error,
        >,
    > + ::core::marker::Send + 'a {
        ::shelf::__private::load_children(
            db,
            parents,
            OrderLine::ORDER_ID,
            |child: &OrderLine| { &child.order_id },
        )
    }
}
```

Each method hands a loader three things the derive cannot compute itself: the child's column
constant, `OrderLine::ORDER_ID` (Chapter 10), for the query; a closure that reads a child's
foreign key, `|child: &OrderLine| &child.order_id`, for the grouping; and the parent's key. The
return type is written out as `impl Future<…> + Send + 'a`, as everywhere in shelf (Chapter 7):
the future borrows `self` and `db` for `'a`, and is `Send` whatever the executor. `belongs_to`
is shorter still: `line.order(&db)` is `<Order as Table>::find(db, &self.order_id)`, Chapter
6's `find` with the foreign key.

> **Macro lens:** `OrderLine::ORDER_ID` and `child.order_id` name things in *another* struct,
> which this derive never sees: a derive receives only the item it is attached to (Chapter 3,
> §3.1). So the macro cannot check that `OrderLine` has a field `order_id`. It writes code that
> assumes so and lets the compiler check it, after expansion, when both structs exist. For that
> check to point somewhere useful, the calls are built with `quote_spanned!{key.span()=> …}`:
> every token of them carries the span of the `order_id` written in the attribute, so an error in
> the generated call is reported there, and not at `#[derive(Table)]`.

The UI test `tests/ui/relations.rs` shows where that puts the errors. A typo in `key`, on a
`has_many`:

~~~rust
// orm/shelf/tests/ui/relations.rs (excerpt)
#[derive(FromRow, Table)]
#[shelf(table = "orders", has_many(OrderLine, key = orderid, as = lines))]
struct Order {
    #[shelf(primary_key)]
    id: i64,
}

#[derive(FromRow, Table)]
#[shelf(table = "order_lines", belongs_to(Order, key = order_id))]
struct OrderLine {
    #[shelf(primary_key)]
    id: i64,
    order_id: i64,
}
~~~

[orm/shelf/tests/ui/relations.rs](../shelf/tests/ui/relations.rs)

```text
error[E0599]: no associated function or constant named `ORDERID` found for struct `OrderLine` in the current scope
  --> tests/ui/relations.rs:36:53
   |
36 | #[shelf(table = "orders", has_many(OrderLine, key = orderid, as = lines))]
   |                                                     ^^^^^^^ associated function or constant not found in `OrderLine`
...
44 | struct OrderLine {
   | ---------------- associated function or constant `ORDERID` not found for this struct
   |
help: there is an associated constant `ORDER_ID` with a similar name
   |
36 - #[shelf(table = "orders", has_many(OrderLine, key = orderid, as = lines))]
36 + #[shelf(table = "orders", has_many(OrderLine, key = ORDER_ID, as = lines))]
   |

error[E0609]: no field `orderid` on type `&OrderLine`
  --> tests/ui/relations.rs:36:53
   |
36 | #[shelf(table = "orders", has_many(OrderLine, key = orderid, as = lines))]
   |                                                     ^^^^^^^ unknown field
   |
help: a field with a similar name exists
   |
36 | #[shelf(table = "orders", has_many(OrderLine, key = order_id, as = lines))]
   |                                                          +
```

Both errors point at the attribute's `orderid`. The second is the better one: rustc compared the
name with `OrderLine`'s fields and suggests `order_id`. The first suggests `ORDER_ID`, the
constant's name, which would not be the right fix in the attribute; it is rustc reasoning about
the generated code, as it must, and the second error says what to write.

A foreign key of the wrong type, an `i32` field pointing at an `i64` key, is caught by the same
mechanism, and by a trait of shelf's own with `#[diagnostic::on_unimplemented]`:

```text
error[E0308]: mismatched types
  --> tests/ui/relations.rs:68:50
   |
68 | #[shelf(table = "notes", belongs_to(Order, key = order_id))]
   |                                                  ^^^^^^^^
   |                                                  |
   |                                                  expected `&i64`, found `&i32`
   |                                                  arguments to this function are incorrect
   |
   = note: expected reference `&i64`
              found reference `&i32`

error[E0277]: a foreign key of type `i32` cannot refer to a key of type `i64`
  --> tests/ui/relations.rs:68:50
   |
68 | #[shelf(table = "notes", belongs_to(Order, key = order_id))]
   |                                                  ^^^^^^^^ the foreign key's field
   |
   = help: the trait `shelf::__private::ForeignKey<i64>` is not implemented for `i32`
   = note: a foreign key has the type of the key it refers to, or an `Option` of it
```

The first comes from `order()`, which calls `find(db, &self.order_id)`; the second from
`load_order()`, whose loader requires the field to implement `ForeignKey<i64>`, which `i64` and
`Option<i64>` do. One more copy of the second is reported at `#[derive(…, Table)]`: it comes from
the method's return type, which is written once for the whole method and carries the derive's
span. The `.stderr` file keeps all three, so a change in where they land shows up in review.

What the derive *can* check, it checks itself, at the attribute's tokens: `has_many` without
`as` or without a model, an unknown key, a `belongs_to` whose `key` is not a field of the struct
or is a `skip` field, a relation on an `Insert` struct, and a `has_many` on a model with a
composite key:

```text
error: `has_many` needs `as = …`, the name of the method it generates (shelf does not guess plurals): has_many(OrderLine, key = order_id, as = lines)
 --> tests/ui/relations.rs:5:27
  |
5 | #[shelf(table = "orders", has_many(OrderLine, key = order_id))]
  |                           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^

error: `has_many` loads by this model's key, which must be one column: a relation follows a single-column key
  --> tests/ui/relations.rs:27:26
   |
27 | #[shelf(table = "stock", has_many(OrderLine, key = order_id, as = lines))]
   |                          ^^^^^^^^

error: `belongs_to`'s key is a field of `NoField` that holds the foreign key; it has no field `order_no` (NoField points at `Order`'s key)
  --> tests/ui/relations.rs:60:56
   |
60 | #[shelf(table = "order_lines", belongs_to(Order, key = order_no))]
   |                                                        ^^^^^^^^
```

Parsing `as = lines` takes no special care, although `as` is a Rust keyword. Chapter 4's
`parse_nested_meta` reads each key as a path, and syn 3.0.9 reads that path's segments with
`Ident::parse_any` (`meta.rs`, `parse_meta_path`), which accepts keywords; `inner.path.is_ident("as")`
then matches it like any other key. serde's `#[serde(crate = "…")]` is parsed the same way.

## 11.2 N+1, counted and timed

The loop is easy to write and correct, which is why it is everywhere:

~~~rust
for order in &orders {
    let lines = order.lines(&db).await?; // one query per order
    // …
}
~~~

Its cost is in the number of statements, and the number is easy to count with an executor of
your own. `Executor` is not sealed (Chapter 7, §7.2), so a wrapper around a `Client` can count
the statements it runs and keep the SQL it prepares, and every shelf call works through it:

```rust
use std::future::Future;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use bytes::Bytes;
use shelf::{Executor, FromRow, Table};
use tokio_postgres::types::ToSql;
use tokio_postgres::{Client, CopyInSink, Error, Row, RowStream, Statement};

/// A client that counts the statements it runs and records the SQL it prepares.
struct Recording<'c> {
    client: &'c Client,
    runs: AtomicUsize,
    prepared: Mutex<Vec<String>>,
}

impl<'c> Recording<'c> {
    fn new(client: &'c Client) -> Self {
        Recording {
            client,
            runs: AtomicUsize::new(0),
            prepared: Mutex::new(Vec::new()),
        }
    }

    /// The statements run since the last call.
    fn runs(&self) -> usize {
        self.runs.swap(0, Ordering::SeqCst)
    }

    fn run(&self) {
        self.runs.fetch_add(1, Ordering::SeqCst);
    }
}

impl Executor for Recording<'_> {
    fn prepare(&self, sql: &str) -> impl Future<Output = Result<Statement, Error>> + Send {
        self.prepared.lock().unwrap().push(sql.to_owned());
        self.client.prepare(sql)
    }

    fn query(
        &self,
        statement: &Statement,
        params: &[&(dyn ToSql + Sync)],
    ) -> impl Future<Output = Result<Vec<Row>, Error>> + Send {
        self.run();
        self.client.query(statement, params)
    }

    fn query_one(
        &self,
        statement: &Statement,
        params: &[&(dyn ToSql + Sync)],
    ) -> impl Future<Output = Result<Row, Error>> + Send {
        self.run();
        self.client.query_one(statement, params)
    }

    fn query_opt(
        &self,
        statement: &Statement,
        params: &[&(dyn ToSql + Sync)],
    ) -> impl Future<Output = Result<Option<Row>, Error>> + Send {
        self.run();
        self.client.query_opt(statement, params)
    }

    fn execute(
        &self,
        statement: &Statement,
        params: &[&(dyn ToSql + Sync)],
    ) -> impl Future<Output = Result<u64, Error>> + Send {
        self.run();
        self.client.execute(statement, params)
    }

    fn query_raw(
        &self,
        statement: &Statement,
        params: &[&(dyn ToSql + Sync)],
    ) -> impl Future<Output = Result<RowStream, Error>> + Send {
        self.run();
        self.client.query_raw(statement, params.iter().copied())
    }

    fn copy_in(
        &self,
        statement: &Statement,
    ) -> impl Future<Output = Result<CopyInSink<Bytes>, Error>> + Send {
        self.run();
        self.client.copy_in(statement)
    }
}

#[derive(Debug, FromRow, Table)]
#[shelf(table = "orders", has_many(OrderLine, key = order_id, as = lines))]
struct Order {
    #[shelf(primary_key)]
    id: i64,
}

#[derive(Debug, PartialEq, FromRow, Table)]
#[shelf(table = "order_lines")]
#[allow(dead_code)] // only loaded
struct OrderLine {
    #[shelf(primary_key)]
    id: i64,
    order_id: i64,
    sku: String,
}

#[tokio::main]
async fn main() -> Result<(), shelf::Error> {
    let client = shelf_playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE orders (id bigint PRIMARY KEY);
             CREATE TEMP TABLE order_lines (
                 id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
                 order_id bigint NOT NULL, sku text NOT NULL
             );
             INSERT INTO orders SELECT generate_series(1, 50);
             INSERT INTO order_lines (order_id, sku)
                 SELECT o, 'SKU-' || l FROM generate_series(1, 50) o, generate_series(1, 3) l;",
        )
        .await?;
    let db = Recording::new(&client);

    // The loop: the orders, then each order's lines.
    let orders = Order::all(&db).await?;
    let mut looped = Vec::new();
    for order in &orders {
        looped.push(order.lines(&db).await?);
    }
    let loop_runs = db.runs();

    // The loader: the orders, then every order's lines at once.
    let orders = Order::all(&db).await?;
    let batched = Order::load_lines(&db, &orders).await?;
    let loader_runs = db.runs();

    println!(
        "the loop:   {loop_runs} statements for {} orders",
        orders.len()
    );
    println!("the loader: {loader_runs} statements");
    println!("{}", db.prepared.lock().unwrap().last().unwrap());

    assert_eq!(loop_runs, 1 + orders.len());
    assert_eq!(loader_runs, 2);
    assert_eq!(looped, batched); // the same lines, grouped the same way
    Ok(())
}
```

```text
the loop:   51 statements for 50 orders
the loader: 2 statements
SELECT "id", "order_id", "sku" FROM "order_lines" WHERE "order_id" = ANY($1) ORDER BY "id"
```

Fifty-one statements against two, for the same result: `assert_eq!(looped, batched)` compares the
fifty `Vec`s of lines. The last statement prepared is the loader's, a query through Chapter 10's
builder, with every order's key in one array parameter.

### What the count costs

On localhost a statement costs tens of microseconds, and the loop is fast enough to go unnoticed
in development. Through lagproxy, which adds 5 ms each way as a network would, each statement
costs at least one round trip, and a plain `Client` prepares before it runs, which costs a
second (Chapter 7, §7.3). This program times the loop, the same loop pipelined, and the loader:

<!-- check: needs=lagproxy -->
```rust
use std::time::Instant;

use futures_util::future::try_join_all;
use shelf::{FromRow, Table};

#[derive(Debug, FromRow, Table)]
#[shelf(table = "orders", has_many(OrderLine, key = order_id, as = lines))]
struct Order {
    #[shelf(primary_key)]
    id: i64,
}

#[derive(Debug, PartialEq, FromRow, Table)]
#[shelf(table = "order_lines")]
#[allow(dead_code)] // only loaded
struct OrderLine {
    #[shelf(primary_key)]
    id: i64,
    order_id: i64,
    sku: String,
}

#[tokio::main]
async fn main() -> Result<(), shelf::Error> {
    // Through lagproxy: every round trip costs at least twice the delay.
    let client = shelf_playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE orders (id bigint PRIMARY KEY);
             CREATE TEMP TABLE order_lines (
                 id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
                 order_id bigint NOT NULL, sku text NOT NULL
             );
             INSERT INTO orders SELECT generate_series(1, 50);
             INSERT INTO order_lines (order_id, sku)
                 SELECT o, 'SKU-' || l FROM generate_series(1, 50) o, generate_series(1, 3) l;",
        )
        .await?;
    let orders = Order::all(&client).await?;

    let started = Instant::now();
    let mut looped = Vec::new();
    for order in &orders {
        looped.push(order.lines(&client).await?);
    }
    let one_by_one = started.elapsed();

    let started = Instant::now();
    let pipelined = try_join_all(orders.iter().map(|order| order.lines(&client))).await?;
    let all_at_once = started.elapsed();

    let started = Instant::now();
    let batched = Order::load_lines(&client, &orders).await?;
    let loader = started.elapsed();

    println!("the loop             {one_by_one:>9.1?}");
    println!("the loop, pipelined  {all_at_once:>9.1?}");
    println!("load_lines           {loader:>9.1?}");

    assert_eq!(looped, batched);
    assert_eq!(pipelined, batched);
    assert!(
        loader * 10 < one_by_one,
        "two round trips against a hundred"
    );
    assert!(
        all_at_once * 10 < one_by_one,
        "pipelining hides the latency"
    );
    Ok(())
}
```

```text
the loop                  1.5s
the loop, pipelined     30.5ms
load_lines              30.9ms
```

Three runs (a debug build, lagproxy at 5 ms each way, PostgreSQL 18.6 on the same machine) gave
1.5 s for the loop each time, 29.0–30.5 ms for the pipelined loop and 28.6–30.9 ms for the
loader. The loop waited for a hundred round trips, two per order; the loader waited for two,
prepare and query. The benchmark in shelf's repository runs the same comparison on a hundred orders with
three lines each, in a release build, also on a pooled connection (whose statement cache saves
the prepare), and directly against the server:

~~~bash
cargo run -p lagproxy -- --delay-ms 5          # :6543 → :5432, +5 ms each way
DATABASE_URL=postgres://…@localhost:6543/tpbook \
    cargo run -p shelf --release --example bench_relations
~~~

| 100 orders × 3 lines | Round trips | Through lagproxy (5 ms each way) | On localhost |
|---|---|---|---|
| the loop, a `Client` | ≈ 200 | 2,732–2,753 ms | 10.6–12.6 ms |
| the loop, pooled | ≈ 100 | 1,348–1,366 ms | 5.0–5.5 ms |
| the loop pipelined (`try_join_all`), a `Client` | 2–3 | 32–35 ms | 3.3–3.5 ms |
| `load_lines`, a `Client` | 2 | 26.7–28.5 ms | 0.4–0.6 ms |
| `load_lines`, pooled | 1 | 14.0–14.6 ms | 0.2 ms |

(Median of three per strategy, ranges over three runs; rustc 1.99.1, Apple Silicon, PostgreSQL
18.6 on the same machine; a cached `find` took 13.3–13.8 ms through the proxy.)

The loop is not slow because PostgreSQL is slow: on localhost it took 5 to 13 milliseconds. It is
slow because it waits for the network a hundred or two hundred times, and the network in
production is not localhost. A page that renders in 12 ms on a laptop takes 2.7 s with 5 ms of
latency each way. That is the whole N+1 problem, and the reason it survives code review: nothing
in the loop looks expensive.

## 11.3 Batched loading

The loader sends one statement, whatever the number of parents: the children whose foreign key
is in the list of the parents' keys, the list sent as one array parameter. Then it groups the
children by their foreign key in a `HashMap`, and returns them in the parents' order:

~~~rust
// orm/shelf/src/relations.rs (excerpt)
pub fn load_children<'a, P, C, T, F, E>(
    db: &'a E,
    parents: &'a [P],
    column: Column<C, T>,
    fk: fn(&C) -> &F,
) -> impl Future<Output = Result<Vec<Vec<C>>, Error>> + Send
where
    P: Table,
    P::Key: Value<T> + Hash + Eq + Clone,
    C: Table,
    T: ToSql + Sync + Send + 'static,
    F: ForeignKey<P::Key>,
    E: Executor,
{
    let keys: Vec<P::Key> = parents.iter().map(P::key).collect();
    async move {
        if parents.is_empty() {
            return Ok(Vec::new());
        }
        let children = C::query().filter(column.any(keys)).all(db).await?;
        let mut groups: HashMap<P::Key, Vec<C>> = HashMap::new();
        for child in children {
            if let Some(key) = fk(&child).key() {
                groups.entry(key.clone()).or_default().push(child);
            }
        }
        Ok(parents
            .iter()
            .map(|parent| groups.remove(&parent.key()).unwrap_or_default())
            .collect())
    }
}
~~~

[orm/shelf/src/relations.rs](../shelf/src/relations.rs)

The details each settle a question:

- **`= ANY($1)`, not `IN ($1, $2, …)`.** One array parameter makes one statement for any number
  of parents, so a pooled connection prepares it once (main book
  [§3.8](../../book/03-rows-and-types.md#38-arrays)). `IN` with a placeholder per key would be a
  new statement for every page size.
- **No parents, no query.** An empty slice returns at once. `= ANY('{}')` would be valid SQL and
  return nothing, at the cost of a round trip.
- **The bounds say what the grouping needs.** The key must be hashable and comparable
  (`Hash + Eq`), and cloned into the map (`Clone`); keys are integers, UUIDs and short strings,
  for which that is cheap. `P::Key: Value<T>` says the parent's key fits the child's column, the
  check that produced §11.1's `ForeignKey` error.
- **`ForeignKey` reads `i64` and `Option<i64>` alike.** It is implemented for `K` and for
  `Option<K>`, returning `Option<&K>`, so a child whose nullable foreign key is NULL belongs to
  no parent and is skipped.
- **A parent without children gets an empty `Vec`**, in its place: `unwrap_or_default()`. The
  result always has as many entries as `parents`, which is what makes `zip` safe.
- **The children keep their key order** within each group, since the query orders by the child's
  key and the grouping preserves order. Exercise 2 orders them by something else.

`load_parents`, the `belongs_to` loader, is the mirror image: it collects the children's foreign
keys into a `HashSet` (fifty lines, three orders, three keys), sends `"id" = ANY($1)` for them,
and returns the parents in a `HashMap` by key.

**Single-column keys only.** A loader compares one column with one array. A composite key, such
as stockroom's `stock (store_id, product_id)`, would need a row comparison against two arrays,
`("store_id", "product_id") IN (SELECT * FROM unnest($1, $2))`, and keys that are tuples, which
the builder does not model. shelf refuses `has_many` on a model with a composite key at the
attribute (§11.1), and Exercise 3 writes such a loader by hand.

## 11.4 Pipelining

tokio-postgres pipelines: requests sent on one connection without waiting go out back to back,
and their answers come back in order (main book
[Chapter 7](../../book/07-pipelining.md)). Two loads that do not depend on each other, the lines
and the stores of a page of orders, can therefore share their round trips:

<!-- check: needs=lagproxy -->
```rust
use std::time::Instant;

use shelf::{FromRow, Table};

#[derive(Debug, FromRow, Table)]
#[shelf(table = "stores")]
struct Store {
    #[shelf(primary_key)]
    id: i64,
    name: String,
}

#[derive(Debug, FromRow, Table)]
#[shelf(table = "orders", has_many(OrderLine, key = order_id, as = lines))]
#[shelf(belongs_to(Store, key = store_id))]
struct Order {
    #[shelf(primary_key)]
    id: i64,
    store_id: i64,
}

#[derive(Debug, FromRow, Table)]
#[shelf(table = "order_lines")]
#[allow(dead_code)] // only loaded
struct OrderLine {
    #[shelf(primary_key)]
    id: i64,
    order_id: i64,
    sku: String,
}

#[tokio::main]
async fn main() -> Result<(), shelf::Error> {
    let client = shelf_playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE stores (id bigint PRIMARY KEY, name text NOT NULL);
             CREATE TEMP TABLE orders (id bigint PRIMARY KEY, store_id bigint NOT NULL);
             CREATE TEMP TABLE order_lines (
                 id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
                 order_id bigint NOT NULL, sku text NOT NULL
             );
             INSERT INTO stores VALUES (1, 'Berlin'), (2, 'Hamburg');
             INSERT INTO orders SELECT i, 1 + i % 2 FROM generate_series(1, 50) i;
             INSERT INTO order_lines (order_id, sku)
                 SELECT o, 'SKU-' || l FROM generate_series(1, 50) o, generate_series(1, 3) l;",
        )
        .await?;
    let orders = Order::all(&client).await?;

    let started = Instant::now();
    let lines = Order::load_lines(&client, &orders).await?;
    let stores = Order::load_store(&client, &orders).await?;
    let in_sequence = started.elapsed();

    let started = Instant::now();
    let (lines_again, stores_again) = tokio::try_join!(
        Order::load_lines(&client, &orders),
        Order::load_store(&client, &orders),
    )?;
    let pipelined = started.elapsed();

    println!("one after the other  {in_sequence:>8.1?}");
    println!("pipelined            {pipelined:>8.1?}");

    assert_eq!(lines.len(), lines_again.len());
    assert_eq!(stores.len(), stores_again.len());
    assert!(
        pipelined < in_sequence * 3 / 4,
        "two round trips against four"
    );
    Ok(())
}
```

```text
one after the other    58.6ms
pipelined              28.5ms
```

Three runs gave 52.7–58.6 ms one after the other and 28.5–28.7 ms pipelined. Four round trips
became two: both prepares went out together, then both queries. Nothing about
the loaders had to change. They take `&db`, so two of them can borrow one connection at once,
and the same holds for a `Transaction` and a pooled connection, whose statements are then also
cached. The capstone's `orders::get` does exactly this (In stockroom, below).

Pipelining also rescues the N+1 loop, up to a point: in §11.2, `try_join_all` of fifty
`order.lines(&db)` calls took 29.0–30.5 ms, as fast as the loader through the proxy. It hides the *latency* of fifty
queries, since all of them travel together, but not their *work*: the server still parses, plans
and runs fifty statements, and the client still decodes fifty results. The benchmark's localhost
column shows that work when the network is gone: 3.3–3.5 ms for the pipelined loop against 0.2 ms
for one pooled `load_lines`, 15 times as much. Pipelining is the right tool for independent
queries; batching is the right tool for the same query repeated.

## 11.5 Joins

The third answer is SQL's own: a join returns orders and lines in one statement. Its cost is
duplication: a join row holds an order's columns *and* a line's, so an order with three lines
comes back three times, and an order with none disappears unless the join is a `LEFT JOIN`. shelf
reads such rows with a `FromRow` struct that flattens the parent (Chapter 4, §4.3); the grouping
is yours to write.

The other way is to aggregate the children in SQL, one row per parent: `json_agg` builds a JSON
array of the lines, which `#[shelf(json)]` reads into a `Vec`. Both, side by side:

```rust
use serde::Deserialize;
use shelf::FromRow;

/// The order's columns in a join row; its `id` is aliased, since the line has one too.
#[derive(Debug, PartialEq, FromRow)]
struct OrderHead {
    #[shelf(rename = "order_id")]
    id: i64,
    store_id: i64,
}

/// One join row: an order, repeated for each of its lines.
#[derive(Debug, FromRow)]
struct LineRow {
    #[shelf(flatten)]
    order: OrderHead,
    sku: String,
    quantity: i32,
}

#[derive(Debug, PartialEq, Deserialize)]
struct Line {
    sku: String,
    quantity: i32,
}

/// One row per order, its lines aggregated into JSON.
#[derive(Debug, FromRow)]
struct OrderWithLines {
    id: i64,
    store_id: i64,
    #[shelf(json)]
    lines: Vec<Line>,
}

#[tokio::main]
async fn main() -> Result<(), shelf::Error> {
    let client = shelf_playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE orders (id bigint PRIMARY KEY, store_id bigint NOT NULL);
             CREATE TEMP TABLE order_lines (
                 id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
                 order_id bigint NOT NULL REFERENCES orders,
                 sku text NOT NULL, quantity integer NOT NULL
             );
             INSERT INTO orders VALUES (10, 1), (11, 2), (12, 1);
             INSERT INTO order_lines (order_id, sku, quantity) VALUES
                 (10, 'TEE-01', 1), (11, 'MUG-01', 2), (10, 'CAP-01', 3), (11, 'TEE-01', 1);",
        )
        .await?;

    // A join: one row per line, the order's columns repeated; order 12 has no line, and no row.
    let rows: Vec<LineRow> = shelf::fetch_all(
        &client,
        "SELECT o.id AS order_id, o.store_id, l.sku, l.quantity
         FROM orders o JOIN order_lines l ON l.order_id = o.id
         ORDER BY o.id, l.id",
        &[],
    )
    .await?;
    println!("join:     {} rows", rows.len());
    for row in &rows {
        println!("    {:?} {} {}", row.order, row.sku, row.quantity);
    }

    // An aggregate: one row per order, its lines in a JSON array; a LEFT JOIN keeps order 12.
    let orders: Vec<OrderWithLines> = shelf::fetch_all(
        &client,
        "SELECT o.id, o.store_id,
                coalesce(json_agg(json_build_object('sku', l.sku, 'quantity', l.quantity)
                                  ORDER BY l.id) FILTER (WHERE l.id IS NOT NULL), '[]') AS lines
         FROM orders o LEFT JOIN order_lines l ON l.order_id = o.id
         GROUP BY o.id ORDER BY o.id",
        &[],
    )
    .await?;
    println!("json_agg: {} rows", orders.len());
    for order in &orders {
        println!("    {} {} {:?}", order.id, order.store_id, order.lines);
    }

    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0].order, rows[1].order); // order 10, twice
    assert_eq!(orders.len(), 3);
    assert!(orders[2].lines.is_empty());
    assert_eq!(
        orders[0].lines[1],
        Line {
            sku: "CAP-01".into(),
            quantity: 3
        }
    );
    Ok(())
}
```

```text
join:     4 rows
    OrderHead { id: 10, store_id: 1 } TEE-01 1
    OrderHead { id: 10, store_id: 1 } CAP-01 3
    OrderHead { id: 11, store_id: 2 } MUG-01 2
    OrderHead { id: 11, store_id: 2 } TEE-01 1
json_agg: 3 rows
    10 1 [Line { sku: "TEE-01", quantity: 1 }, Line { sku: "CAP-01", quantity: 3 }]
    11 2 [Line { sku: "MUG-01", quantity: 2 }, Line { sku: "TEE-01", quantity: 1 }]
    12 1 []
```

The join sent each order once per line; with wide parent rows (a `jsonb` customer, an address)
and many children, that duplication is most of the bytes on the wire. The `json_agg` version
sends each order once and every line once, at the price of JSON: the lines arrive as text, are
parsed by serde, and lose their PostgreSQL types (a `numeric` becomes a JSON number, a
`timestamptz` a string). `FILTER (WHERE l.id IS NOT NULL)` and `coalesce(…, '[]')` are what make
an order without lines an empty array instead of `[null]`. An `array_agg` of a composite type
keeps the types, if the composite has a `FromSql` (postgres-types' derive); it is more setup for
the same shape.

When does a join win? When the query needs both tables to decide *which* rows: "orders that
contain a TEE-01", "lines of orders from Berlin", sorted by a column of the other table. A loader
cannot filter parents by their children. And when one round trip matters more than duplicated
bytes: the main book's stockroom reads an order and its lines with two joined statements,
pipelined, in one round trip, where shelf's port takes two (In stockroom, below). For a page of
parents with their children, the loader is simpler, sends no duplicates, and keeps every type.

## 11.6 Why no lazy loading

Many ORMs load relations lazily: `order.lines` is a property that runs a query the first time it
is read. shelf has no such thing, and in async Rust it would cost more than it does elsewhere:

- **A hidden connection.** To load on access, the model must hold a way to reach the database: a
  pool handle in every row, or a global. A model is then no longer plain data that can be sent
  over a channel, serialized or compared, and which connection (inside which transaction) the
  load uses is decided far from the call.
- **A hidden `.await`.** Reading a field cannot await, so the lazy property becomes an `async`
  getter, `order.lines().await`, and at that point it is a method call that makes a round trip:
  which is what `order.lines(&db)` already is, except that shelf's version says *where* the
  round trip goes.
- **A hidden round trip.** The N+1 loop is invisible when each iteration reads a property. Every
  N+1 problem in §11.2 was a loop of explicit calls; a lazy property makes the same loop look like
  field access, and the profiler is the only one who notices.

So shelf's rule is that every round trip is a call with `&db` in it (opinion). A loop that calls
`order.lines(&db)` is visibly a loop of queries; `Order::load_lines(&db, &orders)` is visibly one.

### Side by side — *orders with their lines*

The task: a page of orders, each with its lines, in two statements however many orders there are.

~~~rust
// SeaORM (illustration)
mod order {
    use sea_orm::entity::prelude::*;

    #[sea_orm::model]
    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "orders")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id: i64,
        pub store_id: i64,
        #[sea_orm(has_many)]
        pub lines: HasMany<super::order_line::Entity>,
    }

    impl ActiveModelBehavior for ActiveModel {}
}

mod order_line {
    use sea_orm::entity::prelude::*;

    #[sea_orm::model]
    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "order_lines")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id: i64,
        pub order_id: i64,
        pub sku: String,
        #[sea_orm(belongs_to, from = "order_id", to = "id")]
        pub order: BelongsTo<super::order::Entity>,
    }

    impl ActiveModelBehavior for ActiveModel {}
}

use sea_orm::LoaderTrait;

let orders = order::Entity::find().all(&db).await?;
// One query: WHERE (order_id) IN ((…), (…), …), one bound value per distinct order.
let lines: Vec<Vec<order_line::Model>> = orders.load_many(order_line::Entity, &db).await?;
~~~

By hand, with the grouping written out, and with shelf's loader, in one program that checks both
give the same groups:

```rust
use std::collections::HashMap;

use shelf::{FromRow, Table};
use tokio_postgres::Client;

#[derive(Debug, FromRow, Table)]
#[shelf(table = "orders", has_many(OrderLine, key = order_id, as = lines))]
struct Order {
    #[shelf(primary_key)]
    id: i64,
    store_id: i64,
}

#[derive(Debug, PartialEq, FromRow, Table)]
#[shelf(table = "order_lines")]
struct OrderLine {
    #[shelf(primary_key)]
    id: i64,
    order_id: i64,
    sku: String,
}

/// By hand: the keys as one array, then the lines grouped by order, in the orders' order.
async fn lines_by_hand(
    client: &Client,
    orders: &[Order],
) -> Result<Vec<Vec<OrderLine>>, shelf::Error> {
    let ids: Vec<i64> = orders.iter().map(|o| o.id).collect();
    let lines: Vec<OrderLine> = shelf::fetch_all(
        client,
        "SELECT id, order_id, sku FROM order_lines WHERE order_id = ANY($1) ORDER BY id",
        &[&ids],
    )
    .await?;
    let mut groups: HashMap<i64, Vec<OrderLine>> = HashMap::new();
    for line in lines {
        groups.entry(line.order_id).or_default().push(line);
    }
    Ok(orders
        .iter()
        .map(|o| groups.remove(&o.id).unwrap_or_default())
        .collect())
}

#[tokio::main]
async fn main() -> Result<(), shelf::Error> {
    let client = shelf_playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE orders (id bigint PRIMARY KEY, store_id bigint NOT NULL);
             CREATE TEMP TABLE order_lines (
                 id bigint PRIMARY KEY, order_id bigint NOT NULL, sku text NOT NULL
             );
             INSERT INTO orders VALUES (10, 1), (11, 2), (12, 1);
             INSERT INTO order_lines VALUES
                 (1, 10, 'TEE-01'), (2, 11, 'MUG-01'), (3, 10, 'CAP-01'), (4, 11, 'TEE-01');",
        )
        .await?;
    let orders = Order::query()
        .filter(Order::STORE_ID.eq(1))
        .all(&client)
        .await?;

    let by_hand = lines_by_hand(&client, &orders).await?;
    let with_shelf = Order::load_lines(&client, &orders).await?;

    assert_eq!(by_hand, with_shelf);
    assert_eq!(with_shelf.len(), 2); // orders 10 and 12
    assert_eq!(with_shelf[0].len(), 2);
    assert!(with_shelf[1].is_empty());
    Ok(())
}
```

| | by hand | shelf | SeaORM |
|---|---|---|---|
| **Same** | Two statements whatever the number of orders: the parents, then every child at once, grouped by parent in Rust, one `Vec` per parent in the parents' order | The same | The same |
| **Different** | You write the grouping, the key list and the SQL, and keep its column list in step with the struct | `has_many` generates `load_lines` from the attribute; the query is `= ANY($1)` with one array, one statement for any page size; a mistake in `key` is a compile error at the attribute | The relation is a field of the model (`HasMany<…>`), and `load_many` is a method on any `Vec` of models; the query is a tuple `IN` with one bound value per distinct key (`loader.rs`, `prepare_condition_with_save_as`), which supports composite keys and gives a new statement for each number of keys; SeaORM 2's entity loader also joins one-to-one relations into the parent's query |

SeaORM handles composite keys, and many-to-many relations through a junction table, which shelf
does not; shelf's single array parameter keeps one statement per relation. Both, like the
hand-written version, return the children in the shape of the parents you passed.

---

## In shelf

The generator is one function per direction. `has_many` checks nothing it cannot see: it builds
the names it needs from the attribute (`load_` and the method's name; the child's constant from
the key's name in capitals) and spans the calls on the key:

~~~rust
// orm/shelf-codegen/src/relations.rs (excerpt)
fn has_many(input: &DeriveInput, relation: &Relation) -> TokenStream {
    let Relation {
        model: child,
        key,
        name,
        ..
    } = relation;
    let name = name
        .as_ref()
        .expect("has_many requires `as`, checked when parsed");
    let load = format_ident!("load_{}", name.unraw());
    // The child's column constant, spanned on `key` so that a wrong name is reported there.
    let column = format_ident!(
        "{}",
        key.unraw().to_string().to_uppercase(),
        span = key.span()
    );
    // …
    // The calls carry the span of `key = …`: a type that does not fit is reported there.
    let one_body = quote_spanned! {key.span()=>
        ::shelf::__private::children(db, #child::#column, <Self as ::shelf::Table>::key(self))
    };
    let many_body = quote_spanned! {key.span()=>
        ::shelf::__private::load_children(db, parents, #child::#column, |child: &#child| {
            &child.#key
        })
    };
    quote! {
        #[doc = #one_doc]
        #vis fn #name<'a, E: ::shelf::Executor>(&'a self, db: &'a E) -> #one {
            #one_body
        }

        #[doc = #many_doc]
        #vis fn #load<'a, E: ::shelf::Executor>(db: &'a E, parents: &'a [Self]) -> #many {
            #many_body
        }
    }
}
~~~

[orm/shelf-codegen/src/relations.rs](../shelf-codegen/src/relations.rs)

`belongs_to` decides one thing from tokens: whether the foreign key field is written `Option<…>`,
which picks `find` or `parent_opt` and the return type. A derive sees tokens, not types, so a
type alias of `Option` is not recognized; the field must be written `Option<…>` for a nullable
key.

## In stockroom

stockroom's `orders::get` reads one order and its lines with two joined statements, pipelined:
one round trip.

~~~rust
// stockroom/src/repo/orders.rs (excerpt)
pub async fn get(db: &impl GenericClient, id: Uuid) -> Result<Order, AppError> {
    let head = db
        .prepare_cached(
            "SELECT o.id, s.code AS store, o.status, o.customer, o.total_cents, o.version,
                    o.created_at, o.updated_at
             FROM orders o JOIN stores s ON s.id = o.store_id
             WHERE o.id = $1",
        )
        .await?;
    let lines = db
        .prepare_cached(
            "SELECT p.sku, l.quantity, l.unit_price_cents
             FROM order_lines l JOIN products p ON p.id = l.product_id
             WHERE l.order_id = $1
             ORDER BY p.sku",
        )
        .await?;
    // Two queries, one round trip: pipelined. (The parameter slice is bound to a name
    // because both futures borrow it until they finish.)
    let params: &[&(dyn ToSql + Sync)] = &[&id];
    let (head, lines) = tokio::try_join!(db.query_opt(&head, params), db.query(&lines, params))?;
    // …
}
~~~

[stockroom/src/repo/orders.rs](../../stockroom/src/repo/orders.rs)

The capstone's port has no joins: it reads the order and its lines pipelined, then the store and
the lines' products pipelined, with a `belongs_to` method and a `belongs_to` loader:

~~~rust
// orm/stockroom-shelf/src/repo/orders.rs (excerpt)
pub async fn get(db: &impl Executor, id: Uuid) -> Result<Order, AppError> {
    let lines = LineRow::query().filter(LineRow::ORDER_ID.eq(id));
    let (head, lines) =
        tokio::try_join!(OrderRow::find(db, &id), lines.all(db)).map_err(app_error)?;
    let (store, products) =
        tokio::try_join!(head.store(db), LineRow::load_product(db, &lines)).map_err(app_error)?;
    let skus = products.iter().map(|(id, p)| (*id, &p.sku)).collect();
    view(head, &store, lines, &skus)
}
~~~

[orm/stockroom-shelf/src/repo/orders.rs](../stockroom-shelf/src/repo/orders.rs)

Two round trips against one: through lagproxy, 28.4–29.9 ms against 13.2–14.8 ms (orm/PLAN.md
§12 #54, measured in Chapter 14). Calling the relation methods one after another would take
three. This is §11.5's trade in a real handler: the join puts the store's code and the products'
SKUs in the rows that need them, and the loaders cannot, because what a line needs from its
product is decided by the line. For one order, the hand-written joins win on round trips; for a
page of orders, the port's loaders would cost the same two round trips, and the joins would
duplicate every order's columns.

## When not to use relation loaders

- **A report across several tables.** Totals per store, the best-selling SKUs: one SQL query
  with joins and `GROUP BY`, read with a `FromRow` struct.
- **Filtering parents by their children.** "Orders containing a TEE-01" is a join or an
  `EXISTS`, written in SQL or as a raw fragment (Chapter 10, §10.7).
- **A deep tree.** Categories of categories, a bill of materials: a recursive CTE fetches the
  whole tree in one statement, where loaders would take one round trip per level.
- **One row and everything it needs, where one round trip matters.** Pipelined joins, as in
  stockroom's `orders::get`.

## Summary

- `has_many(Child, key = fk, as = name)` and `belongs_to(Parent, key = fk[, as = name])`
  declare a relation; `key` names the foreign key field, on the child for both. `has_many` needs
  `as`, since shelf does not guess plurals.
- Each relation generates a method for one row (`order.lines(&db)`, `line.order(&db)`) and a
  loader for a slice (`Order::load_lines` → `Vec<Vec<OrderLine>>` aligned with the slice,
  `OrderLine::load_order` → `HashMap` by key). A foreign key written `Option<…>` makes the
  single-row method return an `Option`.
- The derive cannot see the other struct, so it writes code that names its constant and field
  and lets the compiler check them; `quote_spanned!` on the key's span puts the errors in the
  attribute.
- The N+1 loop sends one statement per parent: 51 for 50 orders. Through 5 ms of latency each
  way, it took 1.5 s against 29–31 ms for the loader; on localhost the same loop costs a few
  milliseconds, which is why it goes unnoticed.
- A loader is one query, `= ANY($1)` with every key in one array, grouped by foreign key in a
  `HashMap`; no parents, no query. Relations follow single-column keys.
- Independent loaders pipeline on one connection with `try_join!`. Pipelining the N+1 loop hides
  its latency but not its work.
- A join duplicates parent columns and needs grouping; `json_agg` returns one row per parent at
  the cost of JSON. A join wins when the query filters or sorts across tables.
- shelf has no lazy loading: every round trip is a call with `&db` in it.

## Exercises

1. **Two levels.** Load a page of orders, their lines, and the lines' products, in three
   statements.
   *Hint:* the second loader takes the first one's children: flatten the `Vec<Vec<OrderLine>>`
   into one slice for `OrderLine::load_product`.
2. **Lines in SKU order.** Return each order's lines ordered by SKU rather than by key, still in
   one query.
   *Hint:* order in SQL, group in Rust: the builder with `= ANY` and `order_by`, then a `HashMap`
   whose `Vec`s keep the order they were filled in.
3. **A composite foreign key.** stockroom's `stock` table is keyed by `(store_id, product_id)`,
   and `stock_movements` points at it with the same two columns. Load the movements of a slice
   of stock rows in one query.
   *Hint:* send the two halves of the keys as two arrays and compare the pair:
   `("store_id", "product_id") IN (SELECT * FROM unnest($1::int8[], $2::int8[]))`, as a raw
   fragment; group by the pair, a tuple, which is `Hash + Eq`.

### Solutions

**1.** Each level is one loader; the second needs the first one's results, so they run in
sequence:

```rust
use shelf::{FromRow, Table};

#[derive(Debug, FromRow, Table)]
#[shelf(table = "orders", has_many(OrderLine, key = order_id, as = lines))]
struct Order {
    #[shelf(primary_key)]
    id: i64,
}

#[derive(Debug, Clone, FromRow, Table)]
#[shelf(table = "order_lines", belongs_to(Product, key = product_id))]
struct OrderLine {
    #[shelf(primary_key)]
    id: i64,
    order_id: i64,
    product_id: i64,
    quantity: i32,
}

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
            "CREATE TEMP TABLE products (id bigint PRIMARY KEY, sku text NOT NULL);
             CREATE TEMP TABLE orders (id bigint PRIMARY KEY);
             CREATE TEMP TABLE order_lines (
                 id bigint PRIMARY KEY, order_id bigint NOT NULL, product_id bigint NOT NULL,
                 quantity integer NOT NULL
             );
             INSERT INTO products VALUES (1, 'TEE-01'), (2, 'MUG-01'), (3, 'CAP-01');
             INSERT INTO orders VALUES (10), (11);
             INSERT INTO order_lines VALUES (1, 10, 1, 1), (2, 11, 2, 2), (3, 10, 3, 3),
                                            (4, 11, 1, 1);",
        )
        .await?;

    let orders = Order::all(&client).await?;
    let lines = Order::load_lines(&client, &orders).await?;
    let every_line = lines.concat(); // one slice, for the second level
    let products = OrderLine::load_product(&client, &every_line).await?;

    for (order, lines) in orders.iter().zip(&lines) {
        let items: Vec<String> = lines
            .iter()
            .map(|l| format!("{}×{}", products[&l.product_id].sku, l.quantity))
            .collect();
        println!("order {}: {items:?}", order.id);
    }
    assert_eq!(products.len(), 3); // TEE-01 is in both orders, and loaded once
    assert_eq!(products[&lines[1][0].product_id].sku, "MUG-01");
    Ok(())
}
```

```text
order 10: ["TEE-01×1", "CAP-01×3"]
order 11: ["MUG-01×2", "TEE-01×1"]
```

`concat` clones the lines into one `Vec`, which is why `OrderLine` derives `Clone`; a loader that
took an iterator of references would avoid it. Three statements for any number of orders: the
orders, every line, every product.

**2.** The loader's query, ordered by SKU, then the grouping, which keeps the order in which each
`Vec` was filled:

```rust
use std::collections::HashMap;

use shelf::{FromRow, Table};

#[derive(Debug, FromRow, Table)]
#[shelf(table = "orders")]
struct Order {
    #[shelf(primary_key)]
    id: i64,
}

#[derive(Debug, FromRow, Table)]
#[shelf(table = "order_lines")]
struct OrderLine {
    #[shelf(primary_key)]
    id: i64,
    order_id: i64,
    sku: String,
}

/// Each order's lines, in SKU order, in one query.
async fn lines_by_sku(
    db: &impl shelf::Executor,
    orders: &[Order],
) -> Result<Vec<Vec<OrderLine>>, shelf::Error> {
    let ids: Vec<i64> = orders.iter().map(|o| o.id).collect();
    let lines = OrderLine::query()
        .filter(OrderLine::ORDER_ID.any(ids))
        .order_by(OrderLine::SKU.asc())
        .all(db)
        .await?;
    let mut groups: HashMap<i64, Vec<OrderLine>> = HashMap::new();
    for line in lines {
        groups.entry(line.order_id).or_default().push(line);
    }
    Ok(orders
        .iter()
        .map(|o| groups.remove(&o.id).unwrap_or_default())
        .collect())
}

#[tokio::main]
async fn main() -> Result<(), shelf::Error> {
    let client = shelf_playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE orders (id bigint PRIMARY KEY);
             CREATE TEMP TABLE order_lines (
                 id bigint PRIMARY KEY, order_id bigint NOT NULL, sku text NOT NULL
             );
             INSERT INTO orders VALUES (10), (11);
             INSERT INTO order_lines VALUES (1, 10, 'TEE-01'), (2, 11, 'MUG-01'),
                                            (3, 10, 'CAP-01'), (4, 10, 'BAG-01');",
        )
        .await?;
    let orders = Order::all(&client).await?;
    let lines = lines_by_sku(&client, &orders).await?;
    let skus: Vec<&str> = lines[0].iter().map(|l| l.sku.as_str()).collect();
    assert_eq!(skus, ["BAG-01", "CAP-01", "TEE-01"]);
    assert_eq!(lines[0][0].id, 4);
    assert_eq!(lines[1].len(), 1);
    Ok(())
}
```

Sorting each group in Rust afterwards would work as well; ordering in SQL lets an index on
`(order_id, sku)` do it, and keeps the function the same shape as shelf's loader.

**3.** The keys travel as two arrays of equal length, which `unnest` turns back into pairs; the
grouping key is the pair:

```rust
use std::collections::HashMap;

use shelf::query::Expr;
use shelf::{FromRow, Table};

#[derive(Debug, FromRow, Table)]
#[shelf(table = "stock")]
struct Stock {
    #[shelf(primary_key)]
    store_id: i64,
    #[shelf(primary_key)]
    product_id: i64,
    on_hand: i32,
}

#[derive(Debug, FromRow, Table)]
#[shelf(table = "stock_movements")]
struct Movement {
    #[shelf(primary_key)]
    id: i64,
    store_id: i64,
    product_id: i64,
    delta: i32,
}

/// The movements of each stock row, in one query, aligned with `stock`.
async fn load_movements(
    db: &impl shelf::Executor,
    stock: &[Stock],
) -> Result<Vec<Vec<Movement>>, shelf::Error> {
    let stores: Vec<i64> = stock.iter().map(|s| s.store_id).collect();
    let products: Vec<i64> = stock.iter().map(|s| s.product_id).collect();
    let movements = Movement::query()
        .filter(
            Expr::raw(
                r#"("store_id", "product_id") IN (SELECT * FROM unnest($1::int8[], $2::int8[]))"#,
            )
            .bind(stores)
            .bind(products),
        )
        .all(db)
        .await?;
    let mut groups: HashMap<(i64, i64), Vec<Movement>> = HashMap::new();
    for movement in movements {
        let key = (movement.store_id, movement.product_id);
        groups.entry(key).or_default().push(movement);
    }
    Ok(stock
        .iter()
        .map(|s| groups.remove(&s.key()).unwrap_or_default())
        .collect())
}

#[tokio::main]
async fn main() -> Result<(), shelf::Error> {
    let client = shelf_playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE stock (
                 store_id bigint, product_id bigint, on_hand integer NOT NULL,
                 PRIMARY KEY (store_id, product_id)
             );
             CREATE TEMP TABLE stock_movements (
                 id bigint PRIMARY KEY, store_id bigint NOT NULL, product_id bigint NOT NULL,
                 delta integer NOT NULL,
                 FOREIGN KEY (store_id, product_id) REFERENCES stock
             );
             INSERT INTO stock VALUES (1, 1, 7), (1, 2, 3), (2, 1, 0);
             INSERT INTO stock_movements VALUES
                 (1, 1, 1, 10), (2, 1, 2, 3), (3, 1, 1, -3), (4, 2, 1, 5), (5, 2, 1, -5);",
        )
        .await?;
    let stock = Stock::all(&client).await?;
    let movements = load_movements(&client, &stock).await?;
    for (row, moves) in stock.iter().zip(&movements) {
        let sum: i32 = moves.iter().map(|m| m.delta).sum();
        assert_eq!(sum, row.on_hand); // every movement found its stock row
    }
    assert_eq!(
        movements.iter().map(Vec::len).collect::<Vec<_>>(),
        [2, 1, 2]
    );
    Ok(())
}
```

`Stock::key()` is the generated key, `(i64, i64)`, the same tuple the grouping builds from a
movement, so the lookup needs no conversion. The casts, `$1::int8[]`, are needed because
`unnest`'s argument type cannot be inferred from a bare parameter. A version for shelf would
generate this per composite relation, with a tuple of arrays built by the derive: Chapter 12's
`insert_many` builds one array per column the same way.

---

Previous: [Chapter 10 — A typed query builder](10-query-builder.md) · Next: [Chapter 12 — Bulk writes and streams](12-bulk-and-streams.md)
