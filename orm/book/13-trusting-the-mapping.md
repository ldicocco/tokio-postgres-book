# Chapter 13 — Trusting the mapping: schema checks and tests

> **Building an ORM on tokio-postgres** · Previous: [Chapter 12 — Bulk writes and streams](12-bulk-and-streams.md) · Next: [Chapter 14 — Capstone: stockroom on shelf](14-capstone.md)

A struct and a table are two descriptions of the same data, kept in different places, often by
different people. A migration renames a column; a column becomes nullable; a `bigint` becomes
`numeric`. Chapter 5's check catches most of those, but only when the query runs, which may be the
first request after a deploy, or a nightly job at three in the morning. One of them it catches
only when a NULL finally turns up in a row, which may be weeks later.

This chapter moves the check earlier. `shelf::verify` checks a model against its table at
startup: it prepares a statement over the table, compares its columns with the struct's fields,
asks the catalog which columns may be NULL, and reports every mismatch at once. Then it turns to
the other side of trust, the macros themselves, and the three kinds of test shelf has had since
its first derive: expansion tests, UI tests and tests against the database, with what each one
catches and what none of them can.

**In this chapter you will learn:**

- what can drift between a struct and its table, and when each kind of drift is caught
- how `verify` uses `Statement::columns()` and `pg_attribute` to check a model before it is used
- why it must bypass the statement cache
- where to run it: at startup, and in a test per model
- how to test a derive: expansion tests, UI tests, integration tests, and a hygiene test
- what compile-time checking (sqlx's `query!`, Cornucopia) adds, and what it costs

---

## 13.1 What can drift

Each change a migration can make to a table is caught at some point. The question is when:

| The table changes | Without `verify`, it is caught | With `verify` |
|---|---|---|
| A column the struct reads is renamed or dropped | when a query is prepared: `Mapping::MissingColumn` (Chapter 5) | at startup |
| Its type changes to one the field cannot read (`bigint` → `numeric`) | when a query is prepared: `Mapping::WrongType` | at startup |
| It becomes nullable, and the field is not an `Option` | **when a row is NULL**: `Mapping::Null`, perhaps weeks later | at startup, from the catalog |
| A new `NOT NULL` column without a default appears | when a `New…` struct inserts: 23502 | not checked (Exercise 2) |
| A constraint narrows (`varchar(5)`, a new `CHECK`) | when a write breaks it | not checked (Exercise 3) |

The third row is the dangerous one. A query that reads a column made nullable prepares fine, runs
fine and maps every row, until the first row whose value is NULL:

```rust
use shelf::{FromRow, Table};

#[derive(Debug, FromRow, Table)]
#[shelf(table = "products")]
struct Product {
    #[shelf(primary_key)]
    id: i64,
    sku: String,
    note: String, // was NOT NULL when the struct was written
}

#[tokio::main]
async fn main() -> Result<(), shelf::Error> {
    let client = shelf_playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE products (id bigint PRIMARY KEY, sku text NOT NULL,
                                         note text NOT NULL);
             INSERT INTO products VALUES (1, 'TEE-01', 'cotton'), (2, 'MUG-01', 'fragile');
             -- a migration: notes become optional
             ALTER TABLE products ALTER COLUMN note DROP NOT NULL;",
        )
        .await?;

    // Every check passes: the column exists, its type fits, and no row is NULL yet.
    let products = Product::all(&client).await?;
    println!("before: {} products read", products.len());

    // Weeks later, the first product without a note.
    client
        .batch_execute("INSERT INTO products VALUES (3, 'CAP-01', NULL)")
        .await?;
    let error = Product::all(&client).await.unwrap_err();
    println!("after:  {error}");
    assert!(matches!(
        error,
        shelf::Error::Mapping {
            problem: shelf::Mapping::Null { .. },
            ..
        }
    ));
    Ok(())
}
```

```text
before: 2 products read
after:  Product.note: column "note" is NULL; read it into an Option
```

Nothing about the statement could have warned earlier: a *RowDescription* carries each column's
name and type, not whether it may be NULL. That is in the catalog, and only the catalog can say it
before the first NULL does.

## 13.2 `verify`

`shelf::verify::<M>(&db)` checks model `M` against its table as the database describes it now:

```rust
use shelf::{FromRow, Table, verify};

#[derive(Debug, FromRow, Table)]
#[shelf(table = "products")]
#[allow(dead_code)] // only verified
struct Product {
    #[shelf(primary_key)]
    id: i64,
    sku: String,
    price_cents: i64,
    note: Option<String>,
    doubled: Option<i64>,
}

/// The same table, read wrongly in four ways.
#[derive(Debug, FromRow, Table)]
#[shelf(table = "products")]
#[allow(dead_code)] // only verified
struct Drifted {
    #[shelf(primary_key)]
    id: i64,
    #[shelf(rename = "sku_code")]
    sku: String,
    price_cents: i32,
    note: String,
    doubled: i64,
}

#[tokio::main]
async fn main() -> Result<(), shelf::Error> {
    let client = shelf_playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE products (
                 id bigint PRIMARY KEY,
                 sku text NOT NULL,
                 price_cents bigint NOT NULL,
                 note text,
                 doubled bigint GENERATED ALWAYS AS (price_cents * 2) VIRTUAL
             );",
        )
        .await?;

    verify::<Product>(&client).await?;
    println!("Product: ok");

    let error = verify::<Drifted>(&client).await.unwrap_err();
    let shelf::Error::Schema { model, mismatches } = &error else {
        panic!("{error}");
    };
    println!("{model}:");
    for mismatch in mismatches {
        println!("  {}: {}", mismatch.field, mismatch.problem);
    }
    let fields: Vec<&str> = mismatches.iter().map(|m| m.field).collect();
    assert_eq!(fields, ["sku", "price_cents", "note", "doubled"]); // in field order
    Ok(())
}
```

```text
Product: ok
Drifted:
  sku: the result has no column "sku_code"
  price_cents: column "price_cents" has type int8, which cannot be read into i32
  note: column "note" can be NULL in the table; read it into an Option, or make it NOT NULL
  doubled: column "doubled" can be NULL in the table; read it into an Option, or make it NOT NULL
```

Four mismatches, one report, in field order, and no row read: the table is empty. The last line is
worth a second look. `doubled` is a virtual generated column (PostgreSQL 18+) computed from a
`NOT NULL` column, so it is never NULL in practice, but the catalog says it can be, since it was
not declared `NOT NULL`, and `verify` believes the catalog. PostgreSQL 18 accepts `NOT NULL` on a
virtual column; without it, the field must be an `Option` (orm/PLAN.md §12 #40). The capstone met
this with stockroom's `stock.available` (Chapter 14).

`Error::Schema` is a variant of shelf's error, and its `Display` puts every mismatch on one line,
for a log; the program above walks `mismatches` instead, whose `problem` is the same `Mapping`
Chapter 5's check reports, `MissingColumn` and `WrongType`, plus `Nullable`, which only `verify`
can find.

### How it works

`verify` needs three things for each field: the column it reads, the type it reads it through,
and whether that type takes a NULL. The derive knows the first two at macro time and lists them
in a hidden constant of `Table`, `FIELDS`, one `Field` per column:

~~~rust
// orm/shelf-codegen/tests/expand/columns.expanded.rs (excerpt)
    const FIELDS: &'static [::shelf::__private::Field] = &[
        ::shelf::__private::Field::new::<i64>("id", "id"),
        ::shelf::__private::Field::new::<String>("sku", "sku"),
        ::shelf::__private::Field::new::<Option<String>>("note", "note"),
        ::shelf::__private::Field::new::<i32>("stock", "stock"),
        ::shelf::__private::Field::new::<
            ::shelf::__private::Json<Attributes>,
        >("attributes", "attributes"),
    ];
~~~

[orm/shelf-codegen/tests/expand/columns.expanded.rs](../shelf-codegen/tests/expand/columns.expanded.rs)

The type each field reads through is the one `FromRow` reads: `i32` for the `try_from = i32`
field `stock`, `Json<Attributes>` for the `json` field. `Field::new::<T>` is a `const fn` that
stores two function pointers, instantiated for that `T`:

~~~rust
// orm/shelf/src/verify.rs (excerpt)
pub struct Field {
    /// The field.
    pub name: &'static str,
    /// The column it reads, unquoted.
    pub column: &'static str,
    /// Finds the column and checks its type: `column::<T>` for the type the field reads.
    pub resolve: fn(&[Column], &'static str, &'static str, &str) -> Result<usize, Error>,
    /// Whether the field can take an SQL NULL from a column of this type.
    pub reads_null: fn(&Type) -> bool,
}

impl Field {
    /// The description of a field that reads its column as `T`.
    pub const fn new<T: FromColumn>(name: &'static str, column: &'static str) -> Field {
        Field {
            name,
            column,
            resolve: row::column::<T>,
            reads_null: reads_null::<T>,
        }
    }
}

fn reads_null<T: FromColumn>(ty: &Type) -> bool {
    T::from_sql_null(ty).is_ok()
}
~~~

[orm/shelf/src/verify.rs](../shelf/src/verify.rs)

`resolve` is Chapter 5's `column::<T>`: find the column by name, check `T::accepts` its type.
`reads_null` asks the field's own type whether it would take a NULL: `FromSql::from_sql_null`
succeeds for `Option<T>` and fails for everything else, and for shelf's `Json<T>` it succeeds
exactly when `T` reads a JSON `null` (Chapter 4). No list of nullable types is written anywhere;
the trait already knows.

> **Rust refresher:** `row::column::<T>` names one instantiation of a generic function, and as a
> value it is a function pointer, `fn(…) -> …`, a plain address that can live in a `const`.
> Generic code cannot be stored, but each of its instantiations can: `FIELDS` holds one
> `column::<String>` and one `column::<i64>`, chosen by the derive. That is how a table of
> fields, each with its own type, fits in one array of one struct.

Then `verify` itself, in three steps:

~~~rust
// orm/shelf/src/verify.rs (excerpt)
pub async fn verify<M: Table>(db: &impl Executor) -> Result<(), Error> {
    // `*`, not the model's columns: a statement naming a missing column fails at the first one.
    let statement = db
        .prepare_uncached(&format!("SELECT * FROM {} LIMIT 0", M::TABLE))
        .await?;
    let columns = statement.columns();
    let mut mismatches = Vec::new();
    let mut not_null = Vec::new(); // fields that cannot read NULL, and their column's index
    for (position, field) in M::FIELDS.iter().enumerate() {
        match (field.resolve)(columns, M::MODEL, field.name, field.column) {
            Ok(index) if !(field.reads_null)(columns[index].type_()) => {
                not_null.push((position, field, index));
            }
            Ok(_) => {}
            Err(Error::Mapping { problem, .. }) => mismatches.push((position, field.name, problem)),
            Err(error) => return Err(error),
        }
    }
    // … then, for the fields in `not_null`, one query to the catalog (NULLABLE)
~~~

[orm/shelf/src/verify.rs](../shelf/src/verify.rs)

1. **Describe the table.** `SELECT * FROM "products" LIMIT 0`, prepared and never run. Not the
   model's own `SELECT`: a statement that names a missing column fails to prepare at the first
   one, and `verify` wants every mismatch. `*` is safe here, though shelf never uses it to read
   (Chapter 6), because this statement is never cached: it is prepared, read and dropped.
2. **Check each field.** For each entry of `FIELDS`: find its column, check its type, and note
   the fields that cannot take a NULL. A mismatch is collected, not returned.
3. **Ask the catalog.** One query for every non-`Option` field's column:

~~~rust
// orm/shelf/src/verify.rs (excerpt)
const NULLABLE: &str = "SELECT a.attrelid, a.attnum
    FROM unnest($1::oid[], $2::int2[]) AS c (attrelid, attnum)
    JOIN pg_attribute AS a USING (attrelid, attnum)
    JOIN pg_class AS r ON r.oid = a.attrelid
    WHERE NOT a.attnotnull AND r.relkind IN ('r', 'p')";
~~~

[orm/shelf/src/verify.rs](../shelf/src/verify.rs)

A `Column` of a prepared statement knows where it comes from: `table_oid()` and `column_id()`
are the table's OID and the column's number, from the *RowDescription*, and `None` for a column
computed by an expression. Those two arrays go to `pg_attribute`, the catalog of every table's
columns, whose `attnotnull` is the `NOT NULL` flag
([PostgreSQL docs](https://www.postgresql.org/docs/18/catalog-pg-attribute.html)). The query
returns the columns that can be NULL, and each one read into a field that cannot take NULL becomes
`Mapping::Nullable`.

`relkind IN ('r', 'p')` limits that to tables, plain or partitioned. A view's columns report the
view's own OID, and a view's `attnotnull` is always false, so every column of a view would look
nullable (orm/PLAN.md §12 #40). `verify` checks a view model's names and types and leaves its
NULLs alone; a test, `a_views_columns_are_checked_for_type_but_not_for_nulls`, pins it.

### Why not the statement cache

`verify` prepares with `Executor::prepare_uncached`, a method the executor trait gained for it.
On a plain `Client` it is `prepare`; on a deadpool connection it skips the statement cache. The
reason is that a cached statement describes the table as it was when it was prepared, and
`verify`'s whole job is the table as it is now:

```rust
use shelf::{FromRow, Table, verify};

#[derive(Debug, FromRow, Table)]
#[shelf(table = "products")]
#[allow(dead_code)] // only read and verified
struct Product {
    #[shelf(primary_key)]
    id: i64,
    sku: String,
    note: Option<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let pool = shelf_playground::pool(1);
    let client = pool.get().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE products (id bigint PRIMARY KEY, sku text NOT NULL, note text);
             INSERT INTO products VALUES (1, 'TEE-01', NULL);",
        )
        .await?;
    verify::<Product>(&client).await?;
    Product::all(&client).await?; // its SELECT is now in the connection's cache

    // A migration, while the service runs.
    client
        .batch_execute("ALTER TABLE products ALTER COLUMN sku DROP NOT NULL, DROP COLUMN note")
        .await?;

    // The cached statement still describes the old table.
    let cached = Product::all(&client).await.unwrap_err();
    println!("all, cached:  {}", message(&cached));
    // verify prepares afresh, and sees both changes.
    let error = verify::<Product>(&client).await.unwrap_err();
    println!("verify:       {error}");

    let shelf::Error::Schema { mismatches, .. } = &error else {
        panic!("{error}");
    };
    let fields: Vec<&str> = mismatches.iter().map(|m| m.field).collect();
    assert_eq!(fields, ["sku", "note"]);
    Ok(())
}

/// The server's message, if the server sent one.
fn message(error: &shelf::Error) -> String {
    match error {
        shelf::Error::Db(e) => e
            .as_db_error()
            .map_or(e.to_string(), |d| d.message().into()),
        other => other.to_string(),
    }
}
```

```text
all, cached:  column "note" does not exist
verify:       Product does not match its table: sku: column "sku" can be NULL in the table; read it into an Option, or make it NOT NULL; note: the result has no column "note"
```

The cached `SELECT` can no longer run: the `ALTER TABLE` invalidated its plan, PostgreSQL analyzes
its text again on the next execution, and the column it names is gone. (Had a column been added
instead, a cached `SELECT *` would fail with "cached plan must not change result type", main book
[§4.6](../../book/04-prepared-statements.md#46-caching-statements-and-when-the-cache-goes-stale).)
The client still holds the old description, though, and that is what a cached `prepare` returns.
A `verify` through the same cache would have prepared nothing, read the old description, and
reported only what the catalog query found, the `sku` change; that is how the bug was found, by a
test, in phase S1c (orm/PLAN.md §12 #43). Preparing afresh sees the dropped column too.

> **SQL lens:** a migration that changes a table under a running service invalidates every plan
> that depends on it, on every connection. Each prepared statement is analyzed again on its next
> execution, and fails if a column it names is gone or its result would change shape; one that
> still fits runs on. The client's cached descriptions are not refreshed either way: deadpool's
> cache does not know, a fresh connection does. This is one more reason to migrate before the new
> code starts, and to start it with `verify`.

## 13.3 Where to run it

**At startup.** The capstone's `serve` command runs `verify` for every model before it accepts a
request; a model that does not fit stops the process with every mismatch in the message. A
deploy against a schema it does not match then fails its readiness check, and the old version
keeps serving (main book [§16.8](../../book/16-axum-streaming.md#168-health-and-readiness)):

~~~rust
// orm/stockroom-shelf/src/repo/mod.rs (excerpt)
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
~~~

[orm/stockroom-shelf/src/repo/mod.rs](../stockroom-shelf/src/repo/mod.rs)

The five checks are pipelined on one connection with `join!`, not `try_join!`: every model is
checked, and every error kept, so one restart shows everything that needs fixing. Each check
costs two round trips (the description, then the catalog), and pipelined, five cost about as
much as one. A startup that takes a few milliseconds longer is cheap; a deploy that fails at its
first request is not.

**In a test per model.** The same function runs in the capstone's test suite against a database
cloned from the migrated template (main book
[§17.2](../../book/17-testing.md#172-a-database-per-test-cloned-from-a-template)), so a
migration that breaks a model fails a test before it reaches a deploy. A second test changes the
schema on purpose and checks the report:

~~~rust
// orm/stockroom-shelf/tests/models.rs (excerpt)
#[tokio::test]
async fn a_schema_that_moved_on_is_reported_model_by_model() {
    let db = TestDb::new().await;
    let client = db.client().await;
    client
        .batch_execute(
            "ALTER TABLE stores RENAME COLUMN name TO title;
             ALTER TABLE products ALTER COLUMN tags DROP NOT NULL;
             ALTER TABLE order_lines ALTER COLUMN unit_price_cents TYPE numeric;",
        )
        .await
        .unwrap();
    let errors = repo::verify_models(&client).await.unwrap_err();
    let messages: Vec<String> = errors.iter().map(ToString::to_string).collect();
    assert_eq!(
        messages,
        [
            "Store does not match its table: name: the result has no column \"name\"",
            "Product does not match its table: tags: column \"tags\" can be NULL in the table; \
             read it into an Option, or make it NOT NULL",
            "LineRow does not match its table: unit_price_cents: column \"unit_price_cents\" has \
             type numeric, which cannot be read into Cents",
        ]
    );
    // …
}
~~~

[orm/stockroom-shelf/tests/models.rs](../stockroom-shelf/tests/models.rs)

The test is the cheaper of the two places: it fails in CI, before anything is deployed. Startup is
the safety net for what CI did not see: a database migrated by hand, a deploy that skipped a
migration, a staging database that drifted.

`verify` checks models, not `New…` structs, and not the queries you write by hand. An `Insert`
struct writes a subset of the columns, and the interesting question for it is the opposite one:
which required columns does it *not* write (Exercise 2). A hand-written query read with a
`FromRow` struct is checked by Chapter 5's resolve when it is prepared, which is as early as its
SQL exists.

## 13.4 Testing a derive

`verify` checks that a model fits a table. Whether the derive generates the right code for a
model in the first place is shelf's own problem, and shelf tests it four ways, because each
catches what the others cannot:

| Test | What it runs | What it catches | What it cannot catch |
|---|---|---|---|
| **Expansion** (`shelf-codegen/tests/expand.rs`) | the generator on structs in `tests/expand/*.rs`, pretty-printed, compared with `*.expanded.rs` | any change to what users get, as a reviewable diff | whether the code compiles, or works |
| **UI** (`shelf/tests/ui.rs`, trybuild) | the compiler on broken structs in `tests/ui/*.rs`, its errors compared with `*.stderr` | the message of every error, and where it points | whether correct code works |
| **Hygiene** (`shelf/tests/derive.rs`) | the derives beside `type Result = ()`, `type str = ()` and friends | generated code that names a user's item by accident | anything at run time |
| **Integration** (`shelf/tests/*.rs`, `TestDb`) | real derives, real queries, a schema per test in `shelf_test` | whether the generated code does the right thing | how an error reads, or what changed in the code |

### Expansion tests

An expansion test is a golden file: run the generator, print the result, compare it with a file
committed beside the input. The harness is fifty lines:

~~~rust
// orm/shelf-codegen/tests/expand.rs (excerpt)
#[test]
fn derives_generate_the_expected_code() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/expand");
    // …
    let bless = std::env::var_os("SHELF_BLESS").is_some();
    let mut failures = Vec::new();
    for input in &inputs {
        let actual = expanded(input);
        let expected_path = input.with_extension("expanded.rs");
        if bless {
            fs::write(&expected_path, &actual).unwrap();
            continue;
        }
        let expected = fs::read_to_string(&expected_path).unwrap_or_default();
        if actual != expected {
            failures.push(format!(
                "{} differs from {}:\n--- expected\n{expected}\n--- actual\n{actual}",
                input.display(),
                expected_path.display()
            ));
        }
    }
    // … assert that there are no failures
}
~~~

[orm/shelf-codegen/tests/expand.rs](../shelf-codegen/tests/expand.rs)

It runs in shelf-codegen, a plain library, which is why the generator lives there and not in the
proc-macro crate (Chapter 3, §3.2): a test can call `expand` like any function. The technique fits
in one program, a stage version with one input and its expected expansion, captured from the
generator:

```rust
use syn::{DeriveInput, parse_quote};

/// The expected expansion, captured from a run and committed: the "golden file".
const EXPECTED: &str = r#"#[automatically_derived]
impl ::shelf::FromRow for Price {
    type Columns = (usize, usize);
    fn resolve(
        columns: &[::shelf::__private::tokio_postgres::Column],
    ) -> ::core::result::Result<Self::Columns, ::shelf::Error> {
        ::core::result::Result::Ok((
            ::shelf::__private::column::<i64>(columns, "Price", "cents", "cents")?,
            ::shelf::__private::column::<
                String,
            >(columns, "Price", "currency", "currency")?,
        ))
    }
    fn from_row_at(
        row: &::shelf::__private::tokio_postgres::Row,
        columns: &Self::Columns,
    ) -> ::core::result::Result<Self, ::shelf::Error> {
        ::core::result::Result::Ok(Self {
            cents: ::shelf::__private::get::<i64>(row, columns.0, "Price", "cents")?,
            currency: ::shelf::__private::get::<
                String,
            >(row, columns.1, "Price", "currency")?,
        })
    }
}
"#;

fn expand(input: &DeriveInput) -> String {
    shelf_playground::pretty(shelf_codegen::from_row::expand(input).unwrap())
}

/// The lines that differ, as a reviewer reads them.
fn diff(expected: &str, actual: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (old, new) in expected.lines().zip(actual.lines()) {
        if old != new {
            out.push(format!("- {old}"));
            out.push(format!("+ {new}"));
        }
    }
    out
}

fn main() {
    let input: DeriveInput = parse_quote! {
        struct Price { cents: i64, currency: String }
    };
    assert_eq!(expand(&input), EXPECTED, "the test");

    // A change to the struct, or to the generator, shows up as a diff: the review.
    let renamed: DeriveInput = parse_quote! {
        struct Price { #[shelf(rename = "amount")] cents: i64, currency: String }
    };
    for line in diff(EXPECTED, &expand(&renamed)) {
        println!("{line}");
    }
}
```

```text
-             ::shelf::__private::column::<i64>(columns, "Price", "cents", "cents")?,
+             ::shelf::__private::column::<i64>(columns, "Price", "cents", "amount")?,
```

The diff is one line: the column name `resolve` looks for. That is the value of an expansion
test. When the generator changes deliberately, `SHELF_BLESS=1` rewrites every expected file,
and the change to what users get is a diff in the pull request, line by line, beside the change
to the generator that caused it. The cost is that the files must be *read* when blessed: a test
that is blessed without reading pins whatever the generator happens to produce.

### UI tests

A UI test pins what the user sees when they get it wrong. trybuild compiles each file in
`tests/ui/` as a small crate that must fail, and compares rustc's output with its `.stderr` file:

~~~rust
// orm/shelf/tests/ui.rs (excerpt)
#[test]
fn derive_errors() {
    trybuild::TestCases::new().compile_fail("tests/ui/*.rs");
}
~~~

[orm/shelf/tests/ui.rs](../shelf/tests/ui.rs)

Twelve files hold every error shelf's derives report, and the query builder's and relations'
type errors (Chapters 4, 10 and 11 printed them). Each `.stderr` is rustc's text, so it changes
with the toolchain: the repository pins rustc 1.99.1, and `TRYBUILD=overwrite` accepts new output
deliberately, to be read like an expansion diff. Reading them is what produced shelf's
`#[diagnostic::on_unimplemented]` messages (Chapter 4, §4.5) and the `Condition` trait (Chapter
10, §10.6): an error that was correct but pointed the wrong way showed up in a `.stderr` file
before it showed up in a user's terminal.

### The hygiene test

A derive's output is pasted into the user's module, where anything may be in scope. shelf's
generated code names everything by absolute path (`::core::result::Result`,
`::core::primitive::str`), and one test checks that it does, by deriving next to names that would
capture a careless expansion:

~~~rust
// orm/shelf/tests/derive.rs (excerpt)
// The generated code must not depend on what the user's module calls `Result`, `Ok` or
// `Default`.
#[allow(dead_code)]
type Result = ();
#[allow(dead_code, non_upper_case_globals)]
const Ok: () = ();
#[allow(dead_code)]
trait Default {}
// …nor on what it calls `str` or `u8`: generated code names `::core::primitive::str`.
#[allow(dead_code, non_camel_case_types)]
type str = ();
#[allow(dead_code, non_camel_case_types)]
type u8 = ();
~~~

[orm/shelf/tests/derive.rs](../shelf/tests/derive.rs)

If a derive ever wrote `Result<Self, Error>` instead of `::core::result::Result`, this file would
stop compiling. The test passes by compiling; its body only asserts that the impls exist.

### Integration tests

Everything else is checked against PostgreSQL: each derive and each attribute in
`shelf/tests/*.rs` (from_row, table, insert, patch, query, relations, bulk, stream, verify), with
the harness `TestDb`, which gives each test a schema of its own in the database `shelf_test`:

~~~rust
// orm/shelf/tests/common/mod.rs (excerpt)
impl TestDb {
    pub async fn new() -> TestDb {
        ensure_database();
        let schema = format!("t_{}", Uuid::now_v7().simple());
        let mut config = admin_config();
        config.dbname(DATABASE);
        let (client, task) = connect(&config).await;
        client
            .batch_execute(&format!("CREATE SCHEMA {schema}"))
            .await
            .expect("create the test's schema");
        drop(client);
        let _ = task.await;
        // Server-side settings travel in the startup packet's `options` (main book §1.4).
        config.options(format!("-c search_path={schema}"));
        TestDb { schema, config }
    }
    // … client(), pool(size), client_with(sql); Drop drops the schema
}
~~~

[orm/shelf/tests/common/mod.rs](../shelf/tests/common/mod.rs)

A schema per test is the main book's template database (§17.2) without the template: shelf has no
migrations, so each test creates the few tables it needs, and every connection it opens finds them
first on its `search_path`. Unlike temporary tables, they are visible to every connection of the
test, so a test can commit, use a pool, and check from another connection.

> **Pitfall:** a test that still holds a `Transaction` when its `TestDb` is dropped hangs. The
> drop blocks the test's runtime while `DROP SCHEMA` waits for the transaction's locks, and the
> rollback that dropping the transaction queued can never be sent, since the runtime is blocked.
> shelf's tests end every transaction explicitly (`tx.rollback().await`), and the comment in
> `tests/relations.rs` says why.

### What one suite against two routers covers

The capstone adds a fifth kind of test, which Chapter 14 shows: one scenario of HTTP requests run
against stockroom's router and against the port's, replies compared one by one. It proves that
the port answers as stockroom does, on the paths its scenarios take, and only there: a
deliberate break in the port's "orders not found" failed it, while a break in a helper that one
route had stopped calling did not (orm/PLAN.md §12 #53). Every test covers what it runs; the
coverage is the list of scenarios, and a reviewer should read it as such.

## 13.5 Compile-time checking

`verify` checks at startup. Two tools check before the program exists.

**sqlx's `query!` and `query_as!`** prepare each query against a database *while compiling*. The
macro reads `DATABASE_URL`, describes the statement, and generates code typed from the answer:
the parameters' types, the columns' types, and each column's nullability. sqlx-postgres 0.9.0
infers the last from the same catalog column `verify` reads, `pg_attribute.attnotnull`, and
patches it for outer joins with `EXPLAIN (VERBOSE, FORMAT JSON)` of the statement
(`connection/describe.rs`). Without a database at build time, the macros read saved descriptions
instead: `cargo sqlx prepare` writes one `query-<hash>.json` per query into a `.sqlx` directory,
and with `SQLX_OFFLINE=true` the build uses only those (sqlx-macros-core 0.9.0,
`query/mod.rs`).

**Cornucopia** generates a crate of typed functions from annotated SQL files, by preparing each
query against a real database at generation time (main book
[§14.6](../../book/14-data-layer.md#146-generating-the-data-layer-cornucopia)).

Both catch drift before a build exists, which is earlier than `verify` can. Both pay for it the
same way: a database at build or generation time, or saved metadata that must be regenerated after
every migration and every changed query, or the build checks a schema that no longer exists. And
both check *queries*, one by one, where shelf checks *models*: a model whose columns are checked
covers every statement the derive writes for it, and the query builder's and loaders' too, since
they select the model's columns.

| Checked when | What is checked | What it needs |
|---|---|---|
| Chapter 5's resolve: each query, as it is prepared | the columns this query returns against the struct | nothing |
| `verify`: each model, at startup or in a test | every column the model reads, its type, and NULLs from the catalog | a connection, before serving |
| sqlx's `query!`: each query, at compile time | parameters, columns, types, NULLs, of the SQL as written | a database or `.sqlx` files at build time |
| Cornucopia: each query, at generation time | the same, into generated code | a database at generation time |

shelf sits in the middle on purpose (opinion). Its derives cannot see a database while they run,
and a proc macro that connected to one would make every build depend on a server. `verify` gets
most of the safety, at the cost of finding drift at startup rather than in the editor.

### Side by side — *catching a renamed column*

The task: the column `sku` is renamed `code` by a migration, and the code still reads `sku`. When
does it find out?

~~~rust
// sqlx (illustration)
#[derive(sqlx::FromRow)]
struct Product {
    id: i64,
    sku: String,
}

// The macro prepares this against DATABASE_URL (or reads .sqlx/) while compiling: after the
// rename, the build fails here, with the server's error, before any binary exists.
let product = sqlx::query_as!(Product, "SELECT id, sku FROM products WHERE id = $1", 7_i64)
    .fetch_one(&pool)
    .await?;
~~~

By hand, the first query finds out; with shelf, `verify` finds out at startup, before any query.
One program does both, against the renamed table:

```rust
use shelf::{FromRow, Table, verify};

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
             INSERT INTO products VALUES (7, 'TEE-01');
             ALTER TABLE products RENAME COLUMN sku TO code;",
        )
        .await?;

    // shelf: at startup, before any request.
    let at_startup = verify::<Product>(&client).await.unwrap_err();
    println!("verify:          {at_startup}");

    // By hand: at the first query that reads the column, from the server.
    let by_hand = client
        .query_one("SELECT id, sku FROM products WHERE id = $1", &[&7_i64])
        .await
        .unwrap_err();
    let server = by_hand.as_db_error().expect("a server error");
    println!(
        "the first query: {} ({:?})",
        server.message(),
        server.code()
    );

    // shelf's own reads, if the service had started anyway.
    let first_read = Product::find(&client, &7).await.unwrap_err();
    assert!(matches!(first_read, shelf::Error::Db(_)));
    assert!(matches!(at_startup, shelf::Error::Schema { .. }));
    Ok(())
}
```

```text
verify:          Product does not match its table: sku: the result has no column "sku"
the first query: column "sku" does not exist (SqlState(E42703))
```

| | by hand | shelf | sqlx |
|---|---|---|---|
| **Same** | The drift is found by asking PostgreSQL to describe a statement | The same | The same |
| **Different** | Found at the first query that reads the column, which may be in production, and only for that query | Found at startup, or in a test, for every column of every verified model, NULLs included; a `New…` struct and hand-written SQL are not covered | Found at compile time, for every query written with the macros, against `DATABASE_URL` or the `.sqlx` files, which must be kept current |

The three differ only in *when*. The earlier the better, as long as "earlier" checks the database
the code will really meet: sqlx's offline files are a snapshot, and a snapshot can drift too.
`verify` checks the real database at the moment it matters, which is why the capstone runs it
even though its tests already did.

---

## In shelf

shelf's tests, as they stand at the end of Part III:

| File | Kind | What it covers |
|---|---|---|
| `shelf-codegen/tests/expand.rs` and `tests/expand/*.rs` | expansion | 11 inputs: every derive, every attribute, relations, column constants, versions |
| `shelf/tests/ui.rs` and `tests/ui/*.rs` | UI (trybuild) | 12 files: every error the derives report, the builder's and relations' type errors, a stream outliving its connection |
| `shelf/tests/derive.rs` | hygiene | the derives beside `Result`, `Ok`, `Default`, `str`, `u8`, raw identifiers |
| `shelf/tests/from_row.rs`, `table.rs`, `insert.rs`, `patch.rs`, `executor.rs` | integration | Parts I and II: mapping, reads, writes, upserts, patches, versions, executors |
| `shelf/tests/query.rs`, `relations.rs`, `bulk.rs`, `stream.rs`, `verify.rs` | integration | Part III: this chapter and the three before it |
| `shelf/examples/bench_*.rs` | benchmarks | the measured tables of Chapters 5, 11 and 12 |

The integration tests need `ADMIN_DATABASE_URL` (a server where they may create `shelf_test`);
the others need only a compiler. `cargo test -p shelf -p shelf-codegen` runs all of them.

## When not to use `verify`

- **Hand-written queries.** `verify` checks a model's table. A `FromRow` struct read from your own
  SQL is checked by Chapter 5's resolve when that SQL is prepared; to check it at startup, prepare
  the statement at startup.
- **Tables that change while the service runs**, by design (a partition attached per day): verify
  the parent, which describes every partition.
- **As the only check.** `verify` sees names, types and NULLs. Lengths, `CHECK`s, foreign keys and
  required columns are found by the writes that break them; tests that write are what catch those.

## Summary

- A struct and its table drift apart through migrations. A renamed or retyped column is caught when
  a query is prepared; a column made nullable only when a NULL row is read.
- `verify::<M>(&db)` prepares `SELECT * FROM` the table without the statement cache, checks every
  field of `Table::FIELDS` against the description, asks `pg_attribute.attnotnull` which non-`Option`
  fields' columns can be NULL, and reports every mismatch in `Error::Schema`, in field order.
- `FIELDS` holds, per field, the column and two function pointers instantiated for the type the field
  reads: `column::<T>` and `from_sql_null`.
- A view's columns are checked for names and types, not NULLs; a generated column is nullable unless
  declared `NOT NULL`.
- A cached statement describes the table as it was: after a migration that drops a column it reads,
  it fails on its next execution, and a `verify` through the cache would miss the dropped column.
- Run `verify` at startup, pipelined over every model, and in a test per model against the migrated
  schema.
- A derive is tested four ways: expansion tests (what users get, as a diff), UI tests (what errors
  say and where), a hygiene test (absolute paths), integration tests (what the code does).
- sqlx's `query!` and Cornucopia check queries before the program exists, at the cost of a database or
  saved metadata at build time; `verify` checks models against the real database at startup.

## Exercises

1. **Every model at once.** Write `verify_all(&db)` that checks a list of models, pipelined, and
   returns every error, without writing a `join!` per model.
   *Hint:* `verify` is generic, so there is no trait object of models; but each `verify::<M>(db)`
   is a future, and a `Vec` of boxed futures can hold one per model (`FutureExt::boxed`,
   `join_all`).
2. **Required columns.** Write `verify_insert::<I>(&db)` for an `Insert` struct: report the `NOT
   NULL` columns of its table that have no default, are not generated or identity columns, and that
   the struct does not write.
   *Hint:* `pg_attribute` has `attnotnull`, `atthasdef`, `attidentity` and `attgenerated`;
   `to_regclass($1)` takes the model's quoted `TABLE`; `I::COLUMNS` holds the quoted names it
   writes.
3. **What `verify` does not see.** Narrow a `text` column to `varchar(5)` under a model, and show
   that `verify` passes and the next insert of a longer value fails. What else would pass?
   *Hint:* `varchar(5)` is still read into a `String`; the length is a type modifier, which
   `accepts` does not see.

### Solutions

**1.** One boxed future per model, all polled together on one connection:

```rust
use futures_util::FutureExt;
use futures_util::future::{BoxFuture, join_all};
use shelf::{Executor, FromRow, Table, verify};

#[derive(Debug, FromRow, Table)]
#[shelf(table = "stores")]
#[allow(dead_code)] // only verified
struct Store {
    #[shelf(primary_key)]
    id: i64,
    name: String,
}

#[derive(Debug, FromRow, Table)]
#[shelf(table = "products")]
#[allow(dead_code)] // only verified
struct Product {
    #[shelf(primary_key)]
    id: i64,
    sku: String,
}

#[derive(Debug, FromRow, Table)]
#[shelf(table = "orders")]
#[allow(dead_code)] // only verified
struct Order {
    #[shelf(primary_key)]
    id: i64,
    total_cents: i64,
}

/// Every model's check, pipelined; the errors of all that failed.
async fn verify_all<E: Executor>(db: &E) -> Result<(), Vec<shelf::Error>> {
    let checks: Vec<BoxFuture<'_, Result<(), shelf::Error>>> = vec![
        verify::<Store>(db).boxed(),
        verify::<Product>(db).boxed(),
        verify::<Order>(db).boxed(),
    ];
    let errors: Vec<shelf::Error> = join_all(checks)
        .await
        .into_iter()
        .filter_map(Result::err)
        .collect();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

#[tokio::main]
async fn main() -> Result<(), shelf::Error> {
    let client = shelf_playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE stores (id bigint PRIMARY KEY, title text NOT NULL);
             CREATE TEMP TABLE products (id bigint PRIMARY KEY, sku text NOT NULL);
             CREATE TEMP TABLE orders (id bigint PRIMARY KEY, total_cents numeric NOT NULL);",
        )
        .await?;
    let errors = verify_all(&client).await.unwrap_err();
    for error in &errors {
        println!("{error}");
    }
    assert_eq!(errors.len(), 2); // stores and orders; products fits
    Ok(())
}
```

```text
Store does not match its table: name: the result has no column "name"
Order does not match its table: total_cents: column "total_cents" has type numeric, which cannot be read into i64
```

`verify::<M>` is an `async fn` generic over the model, and each instantiation's future has its
own type; `boxed()` erases them into one, `Pin<Box<dyn Future + Send>>`, so a `Vec` can hold them.
A trait object of the models themselves would not work: `Table` has associated constants, which
make it not dyn-compatible. A list of futures is the list of models.

**2.** The catalog knows which columns are required; the struct's `COLUMNS` say which it writes:

```rust
use shelf::{Executor, FromRow, Insert, Table};

#[derive(Debug, FromRow, Table)]
#[shelf(table = "products")]
#[allow(dead_code)] // only checked
struct Product {
    #[shelf(primary_key, generated)]
    id: i64,
    sku: String,
    name: String,
    price_cents: i64,
}

/// Forgets `name`, which the table requires.
#[derive(Insert)]
#[shelf(model = Product)]
#[allow(dead_code)] // only checked
struct NewProduct {
    sku: String,
    price_cents: i64,
}

/// The required columns of `I`'s table that `I` does not write, quoted.
async fn verify_insert<I: Insert>(db: &impl Executor) -> Result<Vec<String>, shelf::Error> {
    let statement = db
        .prepare(
            "SELECT attname::text FROM pg_attribute
             WHERE attrelid = to_regclass($1) AND attnum > 0 AND NOT attisdropped
               AND attnotnull AND NOT atthasdef AND attidentity = '' AND attgenerated = ''
             ORDER BY attnum",
        )
        .await?;
    let table = <I::Model as Table>::TABLE;
    let rows = db.query(&statement, &[&table]).await?;
    Ok(rows
        .iter()
        .map(|row| format!("\"{}\"", row.get::<_, String>(0).replace('"', "\"\"")))
        .filter(|column| !I::COLUMNS.contains(&column.as_str()))
        .collect())
}

#[tokio::main]
async fn main() -> Result<(), shelf::Error> {
    let client = shelf_playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE products (
                 id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
                 sku text NOT NULL, name text NOT NULL, price_cents bigint NOT NULL,
                 created_at timestamptz NOT NULL DEFAULT now(), note text
             );",
        )
        .await?;
    let missing = verify_insert::<NewProduct>(&client).await?;
    println!("NewProduct does not write: {missing:?}");
    assert_eq!(missing, [r#""name""#]);
    Ok(())
}
```

```text
NewProduct does not write: ["\"name\""]
```

The identity `id`, the defaulted `created_at` and the nullable `note` are not required; `name` is.
`to_regclass` resolves the quoted, possibly schema-qualified name the derive wrote, as the
`search_path` does, which also finds a temporary table. Comparing quoted names avoids folding case
by hand: both sides are written by the same quoting rule.

**3.** `varchar(5)` is a type modifier on `varchar`, and the description `verify` reads says
`varchar`, which a `String` accepts:

```rust
use shelf::{FromRow, Insert, Table, verify};

#[derive(Debug, FromRow, Table)]
#[shelf(table = "products")]
#[allow(dead_code)] // only checked
struct Product {
    #[shelf(primary_key, generated)]
    id: i64,
    sku: String,
}

#[derive(Insert)]
#[shelf(model = Product)]
struct NewProduct {
    sku: String,
}

#[tokio::main]
async fn main() -> Result<(), shelf::Error> {
    let client = shelf_playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE products (
                 id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY, sku text NOT NULL
             );
             ALTER TABLE products ALTER COLUMN sku TYPE varchar(5);",
        )
        .await?;

    verify::<Product>(&client).await?; // passes
    let error = NewProduct {
        sku: "TEE-001".into(),
    }
    .insert(&client)
    .await
    .unwrap_err();
    let shelf::Error::Db(error) = error else {
        panic!("{error}");
    };
    let server = error.as_db_error().expect("a server error");
    println!("{} ({:?})", server.message(), server.code());
    Ok(())
}
```

```text
value too long for type character varying(5) (SqlState(E22001))
```

Other changes that pass `verify` and fail later: a new `CHECK` constraint, a new foreign key, a new
unique index, a `numeric(10,2)` narrowed to `numeric(6,2)`, and a trigger that rejects some
writes. What they have in common is that they constrain values, not shapes, and only a write with
a value that breaks them finds out. A test suite that writes realistic values is what catches
those; `verify` is about shapes.

---

Previous: [Chapter 12 — Bulk writes and streams](12-bulk-and-streams.md) · Next: [Chapter 14 — Capstone: stockroom on shelf](14-capstone.md)
