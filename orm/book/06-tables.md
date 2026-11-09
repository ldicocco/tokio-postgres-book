# Chapter 6 — Tables: the model derive

> **Building an ORM on tokio-postgres** · Previous: [Chapter 5 — Checking a statement once, not every row](05-column-indices.md) · Next: [Chapter 7 — The executor and shelf's errors](07-executor.md)

`FromRow` reads the rows of any query, but you still write every query, and most of them are
the same few: fetch one row by its key, fetch them all, count them. The struct already knows its
columns. What it lacks is the table they come from and which of them is the key. Two attributes
supply those, and with them a second derive, `Table`, can write the everyday `SELECT`s itself.

It writes them at compile time. The macro knows every name, so it assembles each statement as a
string literal, with every identifier quoted, and leaves nothing to build at run time. The
statements are constants you can print, so the SQL a call sends is never a guess. Values never go
into those strings: they stay `$1` parameters, as they have since the main book's second
chapter. This chapter builds the derive and the four reads it makes free, and finds where SQL
written at compile time stops.

**In this chapter you will learn:**

- what a table adds to a row type: a name, a key and a column list
- how `#[derive(Table)]` reads `table`, `schema`, `primary_key` and `generated`, and what it
  refuses
- how a derive writes SQL at compile time, quoting identifiers so that a table called `order`
  works
- `find`, `find_opt`, `all` and `count`, and the SQL each one sends
- why shelf never writes `SELECT *`
- how a constant can be built from another type's constants, and where that stops

---

## 6.1 What a table adds

A `FromRow` struct describes a result: these columns, read into these fields. It does not say
where the result comes from, and Chapters 1–5 never needed it to, because you wrote the query. A
*model* is a row type that also knows where its rows live. That takes three more facts:

| Fact | What it is | For a `Product` |
|---|---|---|
| The table | where the rows live | `"products"` |
| The key | the columns that pick out one row | `"id"` |
| The column list | the columns the struct reads, in field order | `"id", "sku", "price_cents"` |

The column list is the one `FromRow` already reads; the other two are new. From the three, the
everyday statements follow mechanically: the row with a key is `SELECT` the list `FROM` the
table `WHERE` the key `= $1`, and so on. shelf's `Table` trait holds them, as constants:

~~~rust
// orm/shelf/src/table.rs (excerpt)
pub trait Table: FromRow + Send + Sync {
    /// The key's type: the key column's field type, or a tuple of them for a composite key.
    type Key: Send + Sync;

    /// The struct's name, for errors.
    const MODEL: &'static str;
    /// The table, quoted, and qualified when it has a schema: `"inventory"."products"`.
    const TABLE: &'static str;
    /// The columns the model reads, quoted, in field order.
    const COLUMNS: &'static str;
    /// The row with a given key: `SELECT … WHERE "id" = $1`.
    const FIND: &'static str;
    /// Every row, ordered by the key.
    const ALL: &'static str;
    /// The number of rows: `SELECT count(*) FROM …`.
    const COUNT: &'static str;
    // … UPDATE, DELETE, KEY_COLUMNS, VERSION, UPDATED_AT (Chapters 8 and 9), FIELDS (Chapter 13)

    /// This row's key.
    fn key(&self) -> Self::Key;

    /// `key` as statement parameters, in the order of the key's columns.
    fn key_params(key: &Self::Key) -> Vec<&(dyn ToSql + Sync)>;

    // … find, find_opt, all and count (§6.4); update and delete (Chapter 8)
}
~~~

[orm/shelf/src/table.rs](../shelf/src/table.rs)

`FromRow` is a supertrait: a model is always readable, so `find` can hand its statement to
Chapter 5's `fetch_opt::<Self>` and get a `Self` back. `Send + Sync` let a model's methods hold
it across an `.await` in a future that must be `Send`, which Chapter 7 explains. And `Key` is
whatever identifies a row: the key field's own type, or a tuple of them.

The derive writes all of it. It needs the table's name and the key's field, and the constants
can be printed without a database:

```rust
use shelf::{FromRow, Table};

#[derive(Debug, FromRow, Table)]
#[shelf(table = "products")]
#[allow(dead_code)] // the program only prints the SQL
struct Product {
    #[shelf(primary_key, generated)]
    id: i64,
    sku: String,
    price_cents: i64,
}

fn main() {
    println!("TABLE   {}", Product::TABLE);
    println!("COLUMNS {}", Product::COLUMNS);
    println!("FIND    {}", Product::FIND);
    println!("ALL     {}", Product::ALL);
    println!("COUNT   {}", Product::COUNT);

    assert_eq!(Product::TABLE, r#""products""#);
    assert_eq!(
        Product::FIND,
        r#"SELECT "id", "sku", "price_cents" FROM "products" WHERE "id" = $1"#
    );
    // The key's type is the key field's.
    let key: <Product as Table>::Key = 7_i64;
    assert_eq!(Product::key_params(&key).len(), 1);
}
```

```text
TABLE   "products"
COLUMNS "id", "sku", "price_cents"
FIND    SELECT "id", "sku", "price_cents" FROM "products" WHERE "id" = $1
ALL     SELECT "id", "sku", "price_cents" FROM "products" ORDER BY "id"
COUNT   SELECT count(*) FROM "products"
```

Every statement names its columns, quotes every identifier, and leaves the key as `$1`. Nothing
here runs at run time: `Product::FIND` is a `&'static str` in the binary, as fixed as any
literal you could write by hand, which is what makes it safe to print, to log and to assert on.

## 6.2 `#[derive(Table)]`

`Table` goes beside `FromRow` and reads the column names `FromRow` reads, `rename` and
`rename_all` included. It adds four keys of its own, and two that Chapter 9 uses:

| Attribute | On | Meaning |
|---|---|---|
| `table = "products"` | the struct | The table (required) |
| `schema = "inventory"` | the struct | Its schema, when it is not found on the `search_path` |
| `primary_key` | a field | Part of the key (required on at least one field); several make a composite key |
| `generated` | a field | The database fills it (`DEFAULT uuidv7()`, `now()`, an identity): writes leave it out |
| `version`, `updated_at` | a field | Optimistic locking and a modification time (Chapter 9) |

`generated` changes nothing about reading. A generated column is read like any other; the flag
is for Chapter 8, where `update` must not write a column the database owns, and the struct a new
row is built from leaves it out.

A key spread over several columns is a tuple of their types, in field order. stockroom's
`stock` table is keyed by store and product, and so is its model:

~~~rust
#[derive(FromRow, Table)]
#[shelf(table = "stock")]
struct Stock {
    #[shelf(primary_key)]
    store_id: i64,
    #[shelf(primary_key)]
    product_id: i64,
    on_hand: i32,
}

// type Key = (i64, i64), and FIND ends WHERE "store_id" = $1 AND "product_id" = $2
let stock = Stock::find(&client, &(store_id, product_id)).await?;
~~~

The key is owned: `key()` clones the key fields out of a row, and `find` borrows a `&(i64,
i64)`. A tuple of references would save the clone and cost a lifetime on the trait's `Key`,
which every generic caller would then have to name; for keys, which are integers and UUIDs, the
clone is free.

### Who reports a mistake

`#[shelf(…)]` is one namespace (Chapter 4, [§4.2](04-attributes.md#42-parsing-with-parse_nested_meta)):
both derives on a model parse every key, and each key has one owner that reports its mistakes.
`table`, `schema`, `primary_key` and `generated` belong to `Table`; `FromRow` passes over them,
and `Table` passes over a mistake in `rename`, which `FromRow` reports. A struct with both
derives hears about each mistake once. Unknown keys go to `FromRow`, which every model has.

What `Table` itself refuses, each at the token that causes it, is pinned by a UI test:

~~~rust
// orm/shelf/tests/ui/table.rs (excerpt)
#[derive(FromRow, Table)]
struct NoTable {
    #[shelf(primary_key)]
    id: i64,
}

#[derive(FromRow, Table)]
#[shelf(table = "products")]
struct NoKey {
    id: i64,
}

#[derive(FromRow, Table)]
#[shelf(table = "products", schema = "")]
struct Mistakes {
    #[shelf(primary_key)]
    id: i64,
    #[shelf(flatten)]
    price: Price,
    #[shelf(skip, generated)]
    cached: i64,
}

#[derive(FromRow, Table)]
#[shelf(table = "products")]
struct Generic<T> {
    #[shelf(primary_key)]
    id: i64,
    value: T,
}
~~~

[orm/shelf/tests/ui/table.rs](../shelf/tests/ui/table.rs)

```text
error: Table needs the table's name: #[shelf(table = "…")]
 --> tests/ui/table.rs:5:8
  |
5 | struct NoTable {
  |        ^^^^^^^

error: Table needs a key: mark its column, or columns, #[shelf(primary_key)]
  --> tests/ui/table.rs:12:8
   |
12 | struct NoKey {
   |        ^^^^^

error: a schema name cannot be empty
  --> tests/ui/table.rs:22:38
   |
22 | #[shelf(table = "products", schema = "")]
   |                                      ^^

error: a Table model lists its own columns, and a flattened struct's are not known here: write them out as fields, or read this struct with FromRow and SQL
  --> tests/ui/table.rs:26:13
   |
26 |     #[shelf(flatten)]
   |             ^^^^^^^

error: `generated` cannot apply to a field marked `skip`, which is not a column
  --> tests/ui/table.rs:28:19
   |
28 |     #[shelf(skip, generated)]
   |                   ^^^^^^^^^

error: a Table model cannot be generic: its SQL names one table
  --> tests/ui/table.rs:34:16
   |
34 | struct Generic<T> {
   |                ^
```

Every one of these is reported in a single build, combined as Chapter 4 combined `FromRow`'s
(§4.4). The last two need a word each.

**No type parameters.** A model's SQL names one table. `Page<T>` would send the same
`SELECT … FROM "pages"` for every `T`, and the `T` would have nothing to do with the rows. The
refusal also keeps a door open that §6.5 needs: SQL that joins another model's constants cannot
be written in a generic impl.

**No `flatten`.** `Table` writes its statements at macro time, and a macro sees only the tokens
of the item it is attached to. A `#[shelf(flatten)] price: Price` field says that some of the
columns are `Price`'s, and `Price`'s fields are in another item, which the macro never sees.
`FromRow` copes because it does not need the names: its generated `resolve` calls
`Price::resolve` at run time. `Table` needs them in a string literal.

It could be done at compile time all the same. If `FromRow` published its column list as a
constant, `Table` could join `Price`'s list into its statements with the technique of §6.5. The
catch is that a trait constant must exist for every impl, and a generic `FromRow` struct that
flattens a type parameter would have to join `T`'s list in a generic impl, which §6.5 shows the
compiler refuses. So shelf keeps the refusal, and the error says what to do instead: write the
nested struct's columns out as fields of the model, or read the nested shape with `FromRow` and
a query of your own.

## 6.3 SQL at compile time

A derive is a function from tokens to tokens, and nothing stops that function from formatting a
string. This stage program is a small `Table` derive: it reads `table` and `primary_key` with
Chapter 4's `parse_nested_meta`, quotes every name, writes three statements with `format!`, and
emits them as constants. Run on a struct whose table is called `order` and whose field is
called `type`, two words SQL reserves:

```rust
use proc_macro2::TokenStream;
use quote::quote;
use syn::ext::IdentExt;
use syn::{Data, DeriveInput, Fields, LitStr, parse_quote};

/// `name` as an SQL identifier: in double quotes, with any `"` inside it doubled.
fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// The stage `Table` derive: the table from `#[shelf(table = "…")]`, the key from
/// `#[shelf(primary_key)]`, and three statements written as string literals.
fn expand(input: &DeriveInput) -> syn::Result<TokenStream> {
    let mut table = None;
    for attr in input.attrs.iter().filter(|a| a.path().is_ident("shelf")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("table") {
                table = Some(meta.value()?.parse::<LitStr>()?.value());
                Ok(())
            } else {
                Err(meta.error("the stage knows only `table`"))
            }
        })?;
    }
    let Some(table) = table else {
        return Err(syn::Error::new_spanned(
            &input.ident,
            "Table needs #[shelf(table = \"…\")]",
        ));
    };
    let Data::Struct(data) = &input.data else {
        return Err(syn::Error::new_spanned(&input.ident, "a struct, please"));
    };
    let Fields::Named(fields) = &data.fields else {
        return Err(syn::Error::new_spanned(
            &data.fields,
            "named fields, please",
        ));
    };

    let mut columns = Vec::new();
    let mut key = Vec::new();
    for field in &fields.named {
        let mut primary_key = false;
        for attr in field.attrs.iter().filter(|a| a.path().is_ident("shelf")) {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("primary_key") {
                    primary_key = true;
                    Ok(())
                } else {
                    Err(meta.error("the stage knows only `primary_key`"))
                }
            })?;
        }
        let ident = field.ident.as_ref().expect("named fields have names");
        let column = quote_ident(&ident.unraw().to_string()); // r#type is the column "type"
        if primary_key {
            key.push(column.clone());
        }
        columns.push(column);
    }
    if key.is_empty() {
        return Err(syn::Error::new_spanned(&input.ident, "Table needs a key"));
    }

    // Each statement is formatted here, while the macro runs, and becomes a literal.
    let table = quote_ident(&table);
    let columns = columns.join(", ");
    let key_is: Vec<String> = key
        .iter()
        .enumerate()
        .map(|(i, column)| format!("{column} = ${}", i + 1))
        .collect();
    let find = format!(
        "SELECT {columns} FROM {table} WHERE {}",
        key_is.join(" AND ")
    );
    let all = format!("SELECT {columns} FROM {table} ORDER BY {}", key.join(", "));
    let count = format!("SELECT count(*) FROM {table}");

    let name = &input.ident;
    Ok(quote! {
        impl Table for #name {
            const FIND: &'static str = #find;
            const ALL: &'static str = #all;
            const COUNT: &'static str = #count;
        }
    })
}

fn main() {
    let input: DeriveInput = parse_quote! {
        #[shelf(table = "order")]
        struct Order {
            #[shelf(primary_key)]
            id: i64,
            r#type: String,
            total_cents: i64,
        }
    };
    let code = shelf_playground::pretty(expand(&input).expect("a valid model"));
    println!("{code}");
    assert!(code.contains(
        r#""SELECT \"id\", \"type\", \"total_cents\" FROM \"order\" WHERE \"id\" = $1""#
    ));

    let keyless: DeriveInput = parse_quote! {
        #[shelf(table = "order")]
        struct Order { id: i64 }
    };
    assert_eq!(
        expand(&keyless).unwrap_err().to_string(),
        "Table needs a key"
    );
}
```

```text
impl Table for Order {
    const FIND: &'static str = "SELECT \"id\", \"type\", \"total_cents\" FROM \"order\" WHERE \"id\" = $1";
    const ALL: &'static str = "SELECT \"id\", \"type\", \"total_cents\" FROM \"order\" ORDER BY \"id\"";
    const COUNT: &'static str = "SELECT count(*) FROM \"order\"";
}
```

Three things happened at macro time. `unraw` turned the raw identifier `r#type` into the column
name `type` (Chapter 3, §3.5). `quote_ident` put every name in double quotes. And `quote!`
turned each `String` into a string literal token, escaping its quotes, so the generated code
holds the finished statement.

> **Macro lens:** interpolating a `String` with `#find` does not paste its characters into the
> output as code: quote's `ToTokens` impl for `String` makes one `Literal`, a string literal
> token, escaped as Rust requires. That is why a statement full of double quotes comes out as
> one valid literal, and why no `concat!` or `format!` is left for run time. A `&str` or a
> `String` becomes a literal; an `Ident` becomes a name; a `TokenStream` is spliced in as it is.

shelf's derive does the same in more detail. The statements are written by `format!` in
`expand`, from a quoted table and column list:

~~~rust
// orm/shelf-codegen/src/table.rs (excerpt)
    let table = match &container.schema {
        Some(schema) => format!(
            "{}.{}",
            quote_ident(&schema.value.value()),
            quote_ident(&table.value.value())
        ),
        None => quote_ident(&table.value.value()),
    };
    let column_sql = column_list(&columns);
    let key_columns: Vec<String> = key.iter().map(|(_, column)| column.clone()).collect();
    let find = format!(
        "SELECT {column_sql} FROM {table} WHERE {}",
        equal_params(&key_columns, 1)
    );
    let all = format!(
        "SELECT {column_sql} FROM {table} ORDER BY {}",
        column_list(&key_columns)
    );
    let count = format!("SELECT count(*) FROM {table}");
    // …
    Ok(quote! {
        #[automatically_derived]
        impl ::shelf::Table for #name {
            type Key = #key_type;

            const MODEL: &'static ::core::primitive::str = #model;
            const TABLE: &'static ::core::primitive::str = #table;
            const COLUMNS: &'static ::core::primitive::str = #column_sql;
            const FIND: &'static ::core::primitive::str = #find;
            const ALL: &'static ::core::primitive::str = #all;
            const COUNT: &'static ::core::primitive::str = #count;
            // …
~~~

[orm/shelf-codegen/src/table.rs](../shelf-codegen/src/table.rs)

The column names in `columns` are the ones `FromRow` reads: `rename` if a field has one, else
the field's name through `rename_all`, unrawed. The helpers that quote them are in their own
module, `sql.rs`, which Chapter 8's `Insert` and Chapter 9's `Patch` share:

~~~rust
// orm/shelf-codegen/src/sql.rs (excerpt)
/// `name` as a quoted SQL identifier: `"name"`, with any `"` inside it doubled. Quoting keeps
/// the name exactly as written (PostgreSQL folds unquoted names to lower case) and lets a table
/// be called `order` or a column `type`. The main book's `quote_ident` (its Chapter 2), run by
/// the compiler.
pub fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// The columns, quoted and separated by commas: `"id", "sku"`.
pub fn column_list(columns: &[String]) -> String {
    let quoted: Vec<String> = columns.iter().map(|c| quote_ident(c)).collect();
    quoted.join(", ")
}

/// `"a" = $1 AND "b" = $2`, numbering from `first`.
pub fn equal_params(columns: &[String], first: usize) -> String {
    let terms: Vec<String> = columns
        .iter()
        .enumerate()
        .map(|(i, c)| format!("{} = ${}", quote_ident(c), first + i))
        .collect();
    terms.join(" AND ")
}
~~~

[orm/shelf-codegen/src/sql.rs](../shelf-codegen/src/sql.rs)

Quoting every name, not only the awkward ones, costs nothing and settles two problems for good.
A key word such as `order` or `type` is a name again. And case survives: PostgreSQL folds an
unquoted `unitPrice` to `unitprice`, so a table created with a quoted camel-case column can only
be reached by quoting it. This program reads such a table through the real derive, and sends
the same `SELECT` unquoted to show what quoting saved:

```rust
use shelf::{FromRow, Table};
use tokio_postgres::error::SqlState;

#[derive(Debug, FromRow, Table)]
#[shelf(table = "order", rename_all = "camelCase")]
struct Order {
    #[shelf(primary_key)]
    id: i64,
    r#type: String,
    unit_price: i64,
}

#[tokio::main]
async fn main() -> Result<(), shelf::Error> {
    let client = shelf_playground::connect().await?;
    client
        .batch_execute(
            r#"CREATE TEMP TABLE "order" (
                   id bigint PRIMARY KEY,
                   type text NOT NULL,
                   "unitPrice" bigint NOT NULL
               );
               INSERT INTO "order" VALUES (1, 'retail', 1299);"#,
        )
        .await?;

    println!("{}", Order::FIND);
    let order = Order::find(&client, &1).await?;
    assert_eq!((order.r#type.as_str(), order.unit_price), ("retail", 1299));

    // The same statement without the quotes: `order` is a key word.
    let unquoted = Order::FIND.replace('"', "");
    println!("{unquoted}");
    let error = client.query(&unquoted, &[&1_i64]).await.unwrap_err();
    println!(
        "{}",
        error.as_db_error().expect("from the server").message()
    );
    assert_eq!(error.code(), Some(&SqlState::SYNTAX_ERROR));
    Ok(())
}
```

```text
SELECT "id", "type", "unitPrice" FROM "order" WHERE "id" = $1
SELECT id, type, unitPrice FROM order WHERE id = $1
syntax error at or near "order"
```

> **Pitfall:** the derive quotes *identifiers*, which it knows at compile time, and writes
> `$n` for every *value*. Nothing a program receives at run time reaches the SQL: there is no
> code path from a request body to a statement's text. Keep it that way when you extend a
> model: a table or column name that must vary at run time is chosen from a fixed list
> ([main book §2.3](../../book/02-queries.md#identifiers-are-not-values)), and a value is
> always a parameter, never `format!`ted in.

## 6.4 Reading

With the statements written, the four reads are a few lines each. They are provided methods of
the trait, so the derive generates none of them; it generates the constants they use:

| Method | Statement | Returns |
|---|---|---|
| `find(&db, &key)` | `FIND`, through `fetch_opt` | the row, or `Error::NotFound` |
| `find_opt(&db, &key)` | `FIND`, through `fetch_opt` | `Option<Self>` |
| `all(&db)` | `ALL`, through `fetch_all` | every row, ordered by the key |
| `count(&db)` | `COUNT` | the number of rows, an `i64` |

`find` and `find_opt` send the same statement. The difference is what a missing row means:
for `find_opt` it is an answer, `None`; for `find`, which a caller uses when the row must
exist, it is an error, `NotFound`, with the model's name in it. `all` orders by the key, so two
calls on an unchanged table return the rows in the same order (without an `ORDER BY`,
PostgreSQL returns them in whatever order it reads them, which an `UPDATE` changes). This
program runs the four on a table with a single key and one with a composite key:

```rust
use shelf::{FromRow, Table};

#[derive(Debug, PartialEq, FromRow, Table)]
#[shelf(table = "products")]
struct Product {
    #[shelf(primary_key, generated)]
    id: i64,
    sku: String,
    price_cents: i64,
}

#[derive(Debug, FromRow, Table)]
#[shelf(table = "stock")]
struct Stock {
    #[shelf(primary_key)]
    store_id: i64,
    #[shelf(primary_key)]
    product_id: i64,
    on_hand: i32,
}

#[tokio::main]
async fn main() -> Result<(), shelf::Error> {
    let client = shelf_playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE products (
                 id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
                 sku text NOT NULL UNIQUE,
                 price_cents bigint NOT NULL
             );
             INSERT INTO products (sku, price_cents)
                 VALUES ('MUG-01', 1299), ('TEE-02', 2499), ('VASE-03', 3999);
             CREATE TEMP TABLE stock (
                 store_id bigint,
                 product_id bigint,
                 on_hand integer NOT NULL,
                 PRIMARY KEY (store_id, product_id)
             );
             INSERT INTO stock VALUES (1, 1, 10), (1, 2, 0), (2, 1, 4);",
        )
        .await?;

    let mug = Product::find(&client, &1).await?;
    assert_eq!(mug.sku, "MUG-01");
    assert_eq!(Product::find_opt(&client, &99).await?, None);

    let skus: Vec<String> = Product::all(&client)
        .await?
        .into_iter()
        .map(|p| p.sku)
        .collect();
    assert_eq!(skus, ["MUG-01", "TEE-02", "VASE-03"]);
    assert_eq!(Product::count(&client).await?, 3);

    // A composite key is a tuple, in field order.
    let stock = Stock::find(&client, &(2, 1)).await?;
    assert_eq!(stock.on_hand, 4);
    println!("{}", Stock::FIND);

    // A key with no row: `find` says which model it was looking for.
    let error = Product::find(&client, &99).await.unwrap_err();
    println!("{error}");
    assert!(matches!(error, shelf::Error::NotFound { model: "Product" }));
    Ok(())
}
```

```text
SELECT "store_id", "product_id", "on_hand" FROM "stock" WHERE "store_id" = $1 AND "product_id" = $2
Product not found
```

On a plain `Client` each of these calls costs two round trips, one to prepare the statement and
one to run it, which is what `query` with SQL text costs anyway; Chapter 5 used the first to
check the struct against the result. On a pooled deadpool connection the prepared statement
comes from the connection's cache after the first call, and each call costs one. Chapter 7
measures both.

### Never `SELECT *`

Every statement the derive writes lists its columns. `SELECT *` would be shorter, and `resolve`
would find the fields' columns in it all the same, so the list needs a reason. There are two.

The first is the statement cache. A prepared statement's result columns are fixed when it is
prepared. Add a column to the table and a cached `SELECT *` would now return one more, which
PostgreSQL refuses to do behind the client's back: the statement fails with SQLSTATE 0A000,
"cached plan must not change result type", until it is prepared again
([main book §4.6](../../book/04-prepared-statements.md#46-caching-statements-and-when-the-cache-goes-stale)).
A statement that names its columns returns the same columns after the migration, and keeps
working. This program caches both on one pooled connection, adds a column, and runs both again:

```rust
use shelf::{FromRow, Table};
use tokio_postgres::error::SqlState;

#[derive(Debug, FromRow, Table)]
#[shelf(table = "products")]
struct Product {
    #[shelf(primary_key)]
    id: i64,
    sku: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // One connection, so every call below uses the same session and the same cache.
    let pool = shelf_playground::pool(1);
    let db = pool.get().await?;
    db.batch_execute(
        "CREATE TEMP TABLE products (id bigint PRIMARY KEY, sku text NOT NULL);
         INSERT INTO products VALUES (1, 'MUG-01');",
    )
    .await?;

    // Both statements prepared and cached on this connection.
    let everything = db.prepare_cached("SELECT * FROM products").await?;
    db.query(&everything, &[]).await?;
    Product::find(&db, &1).await?;

    // A migration adds a column.
    db.batch_execute("ALTER TABLE products ADD COLUMN note text")
        .await?;

    let everything = db.prepare_cached("SELECT * FROM products").await?;
    let error = db.query(&everything, &[]).await.unwrap_err();
    println!(
        "SELECT *: {}",
        error.as_db_error().expect("from the server").message()
    );
    assert_eq!(error.code(), Some(&SqlState::FEATURE_NOT_SUPPORTED));

    // The model's cached statement names its columns: it still returns the same two.
    let mug = Product::find(&db, &1).await?;
    println!("Product::find: {mug:?}");
    assert_eq!(mug.sku, "MUG-01");
    Ok(())
}
```

```text
SELECT *: cached plan must not change result type
Product::find: Product { id: 1, sku: "MUG-01" }
```

The second reason is what `*` would hand `resolve`. With an explicit list the result holds
exactly the columns the struct reads; with `*` it holds every column of the table, including a
large `jsonb` document or a `bytea` the struct has no field for, which the server would still
send in every row and the client would still receive. The list costs nothing to write, because
the macro writes it.

## 6.5 Constants, and where they stop

Everything `Table` generates so far is a literal, because the macro knew every piece of it. That
stops being true in Chapter 8. A `NewProduct` struct derives `Insert` and names its model,
`#[shelf(model = Product)]`, and its statement needs the model's table and columns:
`INSERT INTO "products" (…) VALUES (…) RETURNING "id", "sku", …`. Those live in `Product`'s
constants, in another item, which the `Insert` derive cannot see. It sees the tokens
`Product`, nothing more.

The obvious tool is `concat!`, which joins literals at compile time. It takes literals only:

~~~rust
const COUNT: &str = concat!("SELECT count(*) FROM ", Product::TABLE);
~~~

```text
error: expected a literal
 --> src/bin/concat.rs:5:54
  |
5 | const COUNT: &str = concat!("SELECT count(*) FROM ", Product::TABLE);
  |                                                      ^^^^^^^^^^^^^^
  |
  = note: only literals (like `"foo"`, `-42` and `3.14`) can be passed to `concat!()`
```

`concat!` is a macro, so it runs before the compiler knows what `Product::TABLE` holds. What can
read a constant's value is const evaluation, and a `const fn` can join strings at compile time
if it is written with the loops const evaluation allows.

> **Rust refresher:** a `const` item is evaluated by the compiler, and its value is stored in
> the binary; using it costs what using a literal costs. A `const fn` is a function the compiler
> can run while evaluating one. Const evaluation allows `while` and `loop`, `if` and `match`,
> indexing, and calls to other `const fn`s. It does not allow `for` loops, which call
> `IntoIterator::into_iter` and `Iterator::next`, trait methods that are not `const` (rustc
> 1.99.1 refuses one with E0015, "cannot use `for` loop on `&[&str]` in constant functions"),
> and it does not allow allocation: the result must be a fixed-size value, such as an array
> whose length is itself a constant.

So the join takes two steps: one `const fn` computes the total length, and a second fills an
array of that length, which a third views as a `&str`. This program builds a statement of its
own from `Product`'s constants that way, and runs it:

```rust
use shelf::{FromRow, Table};

#[derive(Debug, FromRow, Table)]
#[shelf(table = "products")]
struct Product {
    #[shelf(primary_key)]
    id: i64,
    sku: String,
    price_cents: i64,
}

/// The length of `parts` joined.
const fn total_len(parts: &[&str]) -> usize {
    let mut len = 0;
    let mut i = 0;
    while i < parts.len() {
        len += parts[i].len();
        i += 1;
    }
    len
}

/// `parts` joined, as bytes; `N` is their `total_len`.
const fn join<const N: usize>(parts: &[&str]) -> [u8; N] {
    let mut out = [0; N];
    let mut at = 0;
    let mut i = 0;
    while i < parts.len() {
        let bytes = parts[i].as_bytes();
        let mut j = 0;
        while j < bytes.len() {
            out[at] = bytes[j];
            at += 1;
            j += 1;
        }
        i += 1;
    }
    out
}

/// The cheapest product: a statement of this program's, made of the model's constants.
const CHEAPEST: &str = {
    const PARTS: &[&str] = &[
        "SELECT ",
        Product::COLUMNS,
        " FROM ",
        Product::TABLE,
        " ORDER BY \"price_cents\" LIMIT 1",
    ];
    const BYTES: [u8; total_len(PARTS)] = join(PARTS);
    match std::str::from_utf8(&BYTES) {
        Ok(sql) => sql,
        Err(_) => panic!("joined &strs are UTF-8"),
    }
};

#[tokio::main]
async fn main() -> Result<(), shelf::Error> {
    println!("{CHEAPEST}");
    assert_eq!(
        CHEAPEST,
        r#"SELECT "id", "sku", "price_cents" FROM "products" ORDER BY "price_cents" LIMIT 1"#
    );

    let client = shelf_playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE products (
                 id bigint PRIMARY KEY, sku text NOT NULL, price_cents bigint NOT NULL
             );
             INSERT INTO products VALUES (1, 'VASE-03', 3999), (2, 'MUG-01', 1299);",
        )
        .await?;
    let cheapest: Product = shelf::fetch_one(&client, CHEAPEST, &[]).await?;
    assert_eq!(cheapest.sku, "MUG-01");
    Ok(())
}
```

```text
SELECT "id", "sku", "price_cents" FROM "products" ORDER BY "price_cents" LIMIT 1
```

`CHEAPEST` is as constant as `Product::FIND`: the compiler ran `total_len` and `join`, and the
binary holds the finished statement. shelf has the same three functions in `shelf/src/sql.rs`,
under `__private` for generated code to call, and Chapter 8's `Insert` derive generates exactly
this block, with its own statement's pieces around `<Product as Table>::TABLE`
([orm/shelf/src/sql.rs](../shelf/src/sql.rs)). The inner `const` items matter: an array's
length must be a constant, so `PARTS` must be one before `total_len(PARTS)` can size `BYTES`.

### Where it stops

The inner constants are also where the technique stops. An inner `const` is an item of its own,
which shares nothing with the impl around it but names, so it cannot use that impl's type
parameters. A generic model, if `Table` allowed one, would need its statement joined inside a
generic impl:

~~~rust
impl<T> Counted for Page<T> {
    const COUNT: &'static str = {
        const PARTS: &[&str] = &["SELECT count(*) FROM ", <Page<T> as Named>::TABLE];
        const BYTES: [u8; total_len(PARTS)] = join(PARTS);
        utf8(&BYTES)
    };
}
~~~

```text
error[E0401]: can't use generic parameters from outer item
  --> src/bin/generic.rs:16:65
   |
14 | impl<T> Counted for Page<T> {
   |      - type parameter from outer item
15 |     const COUNT: &'static str = {
16 |         const PARTS: &[&str] = &["SELECT count(*) FROM ", <Page<T> as Named>::TABLE];
   |               -----                                             ^ use of generic parameter from outer item
   |               |
   |               generic parameter used in this inner constant item
   |
   = note: nested items are independent from their parent item for everything except for privacy and name resolution
   = note: a `const` is a separate item from the item that contains it
help: try using a local `let` binding instead
   |
16 -         const PARTS: &[&str] = &["SELECT count(*) FROM ", <Page<T> as Named>::TABLE];
16 +         let PARTS: &[&str] = &["SELECT count(*) FROM ", <Page<T> as Named>::TABLE];
   |
```

The help does not help: a `let` binding is not a constant, so it cannot size the array. A
`static` in a generic function has the same limit, so moving the statement into a `LazyLock`
would not help either. That is one more reason `Table` refuses type parameters (§6.2), and the
reason Chapter 8's `Insert` refuses a model that mentions its own.

The other limit is computing. Joining puts finished pieces side by side; it cannot decide what a
piece says. Chapter 9's `Patch` writes an `UPDATE` whose key parameters are numbered after its
own (`WHERE "id" = $7`, with the 7 depending on how many fields the patch has and how many
columns the model's key has), and that numbering is a computation over another type's
constants. There, shelf builds the statement once, at first use, in a `LazyLock<String>`: the
case where a run-time constant is the right tool.

### Side by side — *fetching by primary key*

The task: read the product with a given id, or learn that there is none.

~~~rust
// SeaORM (illustration)
mod product {
    use sea_orm::entity::prelude::*;

    #[sea_orm::model]
    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "products")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id: i64,
        pub sku: String,
        pub price_cents: i64,
    }

    impl ActiveModelBehavior for ActiveModel {}
}

// SELECT "products"."id", "products"."sku", "products"."price_cents" FROM "products"
//     WHERE "products"."id" = $1 LIMIT $2
let product: Option<product::Model> = product::Entity::find_by_id(7).one(&db).await?;
~~~

By hand, with Chapter 5's `fetch_opt` and the SQL written out, and with the model, in one
program that checks both find the same row:

```rust
use shelf::{FromRow, Table};

#[derive(Debug, PartialEq, FromRow, Table)]
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
            "CREATE TEMP TABLE products (
                 id bigint PRIMARY KEY, sku text NOT NULL, price_cents bigint NOT NULL
             );
             INSERT INTO products VALUES (7, 'MUG-01', 1299);",
        )
        .await?;

    // By hand: the statement written out, the key passed as a parameter.
    let by_hand: Option<Product> = shelf::fetch_opt(
        &client,
        "SELECT id, sku, price_cents FROM products WHERE id = $1",
        &[&7_i64],
    )
    .await?;

    // shelf: the derive wrote the statement.
    let with_shelf = Product::find_opt(&client, &7).await?;

    assert_eq!(by_hand, with_shelf);
    assert_eq!(with_shelf.map(|p| p.sku).as_deref(), Some("MUG-01"));
    assert!(Product::find_opt(&client, &8).await?.is_none());
    Ok(())
}
```

| | by hand | shelf | SeaORM |
|---|---|---|---|
| **Same** | One `SELECT` with the key as `$1`, the columns listed, the row read into a struct | The same | The same |
| **Different** | You write the statement in each function that needs it, and keep its column list in step with the struct | The statement is a constant written at compile time from the struct, which you can print; `find` turns a missing row into `NotFound`, `find_opt` into `None` | The statement is built at run time by sea-query on every call, qualified by table name and with `LIMIT $2` added by `one()`; the model is a struct in a module with its entity, and `find_by_id` takes anything that converts into the key |

SeaORM builds its SQL from a description of the entity at run time, which is what lets its
query builder compose anything; shelf builds the fixed statements once, at compile time, and
leaves composition to Chapter 10's builder. Neither ever puts a value in the SQL.

---

## In shelf

The reads are provided methods of `Table`, written once in shelf for every model:

~~~rust
// orm/shelf/src/table.rs (excerpt)
    /// The row whose key is `key`, or [`Error::NotFound`].
    fn find(
        db: &impl Executor,
        key: &Self::Key,
    ) -> impl Future<Output = Result<Self, Error>> + Send {
        async move {
            Self::find_opt(db, key)
                .await?
                .ok_or(Error::NotFound { model: Self::MODEL })
        }
    }

    /// The row whose key is `key`, if there is one.
    fn find_opt(
        db: &impl Executor,
        key: &Self::Key,
    ) -> impl Future<Output = Result<Option<Self>, Error>> + Send {
        async move { fetch_opt(db, Self::FIND, &Self::key_params(key)).await }
    }

    /// Every row, ordered by the key.
    fn all(db: &impl Executor) -> impl Future<Output = Result<Vec<Self>, Error>> + Send {
        async move { fetch_all(db, Self::ALL, &[]).await }
    }

    // …

    /// The number of rows.
    fn count(db: &impl Executor) -> impl Future<Output = Result<i64, Error>> + Send {
        async move {
            let statement = db.prepare(Self::COUNT).await?;
            Ok(db.query_one(&statement, &[]).await?.try_get(0)?)
        }
    }
~~~

[orm/shelf/src/table.rs](../shelf/src/table.rs)

Two details. The methods take `&impl Executor`, shelf's own trait for "something that can run
a statement", which a `Client`, a `Transaction` and a deadpool connection all implement; and
they return `impl Future<Output = …> + Send` with an `async move` block inside, rather than
being `async fn`s. Chapter 7 is about both. And `key_params` is generated: for a single key it
is one parameter, `param(key)`; for a composite key, one per element of the tuple, in the order
of the key's columns, which is the order of the `$n` in `FIND`.

## When not to use `Table`

- **Reports, joins and aggregates.** A result that is not one table's rows has no key to find
  by. Write the SQL and read it with a `FromRow` struct and `fetch_all`, as in Chapters 1–5.
- **A table with no key.** A log table without a primary key can still be read with `FromRow`;
  `Table` needs a column, or columns, that pick out one row.
- **A view.** `find`, `find_opt`, `all` and `count` are `SELECT`s, so they work on a view if you
  mark a column `primary_key`. PostgreSQL does not check that the column identifies one row, so
  make sure it does; and Chapter 8's writes reach the table only through a view PostgreSQL can
  update.

## Summary

- A model is a `FromRow` struct that also knows its table, its key and its column list.
  `#[derive(Table)]` takes `table` (and `schema`) on the struct, `primary_key` on one or more
  fields, and `generated` on the columns the database fills.
- A composite key is a tuple of the key fields' types, in field order, passed as
  `&(a, b)` of owned values.
- The derive writes every statement at macro time with `format!`, quoting every identifier, and
  emits it as a string literal constant: `TABLE`, `COLUMNS`, `FIND`, `ALL`, `COUNT`. Values are
  always `$n` parameters.
- `find` (`Error::NotFound` when no row has the key), `find_opt`, `all` (ordered by the key) and
  `count` are provided methods of `Table`, built on Chapter 5's `fetch_*`.
- shelf never writes `SELECT *`: a cached `SELECT *` fails with 0A000 after a column is added,
  and `*` sends columns the struct does not read.
- `Table` refuses a struct without a table or a key, type parameters, and `flatten`, each in one
  build, at the token that causes it.
- `concat!` takes literals only. `const fn`s can join another type's constants into a constant
  statement, with inner `const` items to size the array; that cannot be done in a generic impl
  (E0401), and computing a statement, rather than joining one, needs a `LazyLock`.

## Exercises

1. **`exists`.** Write `exists(&db, &key) -> Result<bool, shelf::Error>` for any model, without
   reading the row.
   *Hint:* `SELECT EXISTS (SELECT 1 FROM … WHERE …)`. A trait with a provided method and a
   blanket impl for every `T: Table` adds it to every model; `TABLE` and `KEY_COLUMNS` are
   public constants.
2. **Several keys, one statement.** Write `find_many(&db, &[i64])` that returns every product
   whose key is in the slice, in one round trip, for a model with a single-column key.
   *Hint:* `= ANY($1)` with an array parameter
   ([main book §3.8](../../book/03-rows-and-types.md#38-arrays)). Chapter 11's relation loaders
   are built on the same statement.
3. **Why `all` orders.** Copy `Product::ALL` without its `ORDER BY`, insert three rows, update
   the first, and read them all again. In what order do they come back?
   *Hint:* an `UPDATE` in PostgreSQL writes a new version of the row; a table scan returns rows
   in the order it finds them on the page.

### Solutions

**1.** The statement can be written at run time from the model's constants: they are quoted
identifiers, not values, so formatting them in is safe. It is built on every call here; a real
version would cache it per model, as Chapter 9's `Patch` does with a `LazyLock`.

```rust
use shelf::{Executor, FromRow, Table};

/// `exists` for every model.
trait Exists: Table {
    async fn exists(db: &impl Executor, key: &Self::Key) -> Result<bool, shelf::Error> {
        let key_is: Vec<String> = Self::KEY_COLUMNS
            .iter()
            .enumerate()
            .map(|(i, column)| format!("{column} = ${}", i + 1))
            .collect();
        let sql = format!(
            "SELECT EXISTS (SELECT 1 FROM {} WHERE {})",
            Self::TABLE,
            key_is.join(" AND ")
        );
        let statement = db.prepare(&sql).await?;
        let row = db.query_one(&statement, &Self::key_params(key)).await?;
        Ok(row.try_get(0)?)
    }
}

impl<T: Table> Exists for T {}

#[derive(Debug, FromRow, Table)]
#[shelf(table = "stock")]
#[allow(dead_code)] // the program only asks whether rows exist
struct Stock {
    #[shelf(primary_key)]
    store_id: i64,
    #[shelf(primary_key)]
    product_id: i64,
    on_hand: i32,
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
             INSERT INTO stock VALUES (1, 1, 10);",
        )
        .await?;
    assert!(Stock::exists(&client, &(1, 1)).await?);
    assert!(!Stock::exists(&client, &(1, 2)).await?);
    Ok(())
}
```

The method is an `async fn` in a trait. That is fine for a program that awaits it directly; a
caller that needs the future to be `Send`, such as an axum handler generic over the executor,
would need the `impl Future + Send` form shelf uses (Chapter 7).

**2.** One statement with the keys as one array parameter:

```rust
use shelf::{Executor, FromRow, Table};

#[derive(Debug, FromRow, Table)]
#[shelf(table = "products")]
struct Product {
    #[shelf(primary_key)]
    id: i64,
    sku: String,
}

/// Every product whose id is in `ids`, in one round trip, ordered by id.
async fn find_many(db: &impl Executor, ids: &[i64]) -> Result<Vec<Product>, shelf::Error> {
    let [key] = Product::KEY_COLUMNS else {
        unreachable!("Product has a single-column key");
    };
    let sql = format!(
        "SELECT {} FROM {} WHERE {key} = ANY($1) ORDER BY {key}",
        Product::COLUMNS,
        Product::TABLE
    );
    shelf::fetch_all(db, &sql, &[&ids]).await
}

#[tokio::main]
async fn main() -> Result<(), shelf::Error> {
    let client = shelf_playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE products (id bigint PRIMARY KEY, sku text NOT NULL);
             INSERT INTO products VALUES (1, 'MUG-01'), (2, 'TEE-02'), (3, 'VASE-03');",
        )
        .await?;
    let found = find_many(&client, &[3, 1, 42]).await?;
    let skus: Vec<&str> = found.iter().map(|p| p.sku.as_str()).collect();
    assert_eq!(skus, ["MUG-01", "VASE-03"]); // 42 has no row
    Ok(())
}
```

A key with no row is simply absent from the result; whether that is an error is the caller's
decision. For a composite key the condition becomes a row comparison against two arrays,
`("store_id", "product_id") IN (SELECT * FROM unnest($1, $2))`, which is why shelf's relation
loaders support single-column keys only (Chapter 11).

**3.** The updated row comes back last:

```rust
#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = shelf_playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE products (id bigint PRIMARY KEY, sku text NOT NULL);
             INSERT INTO products VALUES (1, 'MUG-01'), (2, 'TEE-02'), (3, 'VASE-03');",
        )
        .await?;
    let unordered = "SELECT id FROM products";
    let ids = |rows: Vec<tokio_postgres::Row>| -> Vec<i64> {
        rows.iter().map(|row| row.get(0)).collect()
    };

    assert_eq!(ids(client.query(unordered, &[]).await?), [1, 2, 3]);
    client
        .execute("UPDATE products SET sku = 'MUG-01B' WHERE id = 1", &[])
        .await?;
    assert_eq!(ids(client.query(unordered, &[]).await?), [2, 3, 1]);

    // With the key's ORDER BY, as Product::ALL has it, the order does not move.
    let ordered = "SELECT id FROM products ORDER BY id";
    assert_eq!(ids(client.query(ordered, &[]).await?), [1, 2, 3]);
    Ok(())
}
```

PostgreSQL never updates a row in place: the `UPDATE` wrote a new version of row 1 after the
others on the page, and the sequential scan found it there. Nothing in SQL promises any order
without `ORDER BY`, so even this one is an accident of the storage, not a rule; a page that
fills up, a `VACUUM` or an index scan gives another. A model's `all` orders by the key so that
the order means something.

---

Previous: [Chapter 5 — Checking a statement once, not every row](05-column-indices.md) · Next: [Chapter 7 — The executor and shelf's errors](07-executor.md)
