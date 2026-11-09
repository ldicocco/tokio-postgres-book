# Appendix — attribute reference, generated code and phrasebook

> **Building an ORM on tokio-postgres** · Previous: [Chapter 14 — Capstone: stockroom on shelf](14-capstone.md)

Reference material, for looking things up once the chapters have taught them: every `#[shelf(…)]`
attribute and where it may go, what each derive writes for a given struct, the calls each part
of shelf offers, how to say a shelf call in sqlx, SeaORM or diesel, a glossary, and where to read
further. Everything here is for shelf as built in this repository, on tokio-postgres 0.7.18,
syn 3.0.9 and rustc 1.99.1; the other crates' calls were checked in their sources (sqlx 0.9.0,
SeaORM 2.0.4, diesel 2.3.14, diesel-async 0.9.2) on 2026-11-09.

---

## A. Attribute reference

Every attribute is written `#[shelf(…)]`, one namespace for all four derives. Each key has one
owner, the derive that reports its mistakes (Chapter 6, "Who reports a mistake"); a struct with
`FromRow` and `Table` hears about each mistake once.

### On the struct

| Attribute | Derives | Meaning | Ch |
|---|---|---|---|
| `rename_all = "…"` | `FromRow`, `Insert`, `Patch` | Column names follow a convention: `"snake_case"` (the default), `"camelCase"`, `"PascalCase"` or `"SCREAMING_SNAKE_CASE"` | 4 |
| `table = "products"` | `Table` | The table (required) | 6 |
| `schema = "inventory"` | `Table` | Its schema, when it is not found on the `search_path` | 6 |
| `model = Product` | `Insert`, `Patch` | The model the row becomes or changes (required): its table, its columns, its key | 8, 9 |
| `has_many(Child, key = fk, as = name)` | `Table` | `parent.name(&db)` and `Parent::load_name(&db, &parents)`; `fk` is a field of `Child`; `as` is required | 11 |
| `belongs_to(Parent, key = fk)`, `belongs_to(Parent, key = fk, as = name)` | `Table` | `child.name(&db)` and `Child::load_name(&db, &children)`; `fk` is a field of this struct; the name defaults to the model's in snake case | 11 |

### On a field

| Attribute | Derives | Meaning | Ch |
|---|---|---|---|
| `rename = "col"` | `FromRow`, `Insert`, `Patch` | The column `col`; overrides `rename_all` | 4 |
| `skip` | `FromRow`, `Insert`, `Patch` | Not a column: read as `Default::default()`, never written | 4 |
| `flatten` | `FromRow` | A nested `FromRow` struct, read from the same row; refused in a `Table` | 4, 6 |
| `json` | `FromRow`, `Insert` | Read and write a `json`/`jsonb` column through serde; an SQL `NULL` reads as JSON `null`. A `Patch` refuses it: write the field as `Option<Json<T>>` or `Option<serde_json::Value>` | 4, 8, 9 |
| `try_from = S` (or `"S"`) | `FromRow` | Read an `S`, then convert with `TryFrom` | 4 |
| `primary_key` | `Table` | Part of the key (required on at least one field); several make a composite key, a tuple in field order | 6 |
| `generated` | `Table` | The database fills it (`uuidv7()`, `now()`, an identity, a generated column): `update` never writes it | 6, 8 |
| `version` | `Table`, `Patch` | On a model: optimistic locking, `WHERE … AND "version" = $n`, `SET "version" = "version" + 1`, `Error::Stale`. On a patch: the version it was made against, required exactly when the model has one | 9 |
| `updated_at` | `Table` | `SET "updated_at" = now()` on every update: the transaction's start time | 9 |
| `conflict` | `Insert` | Part of the unique key an upsert collides on; the struct then also implements `Upsert` | 8 |

### What is refused

Each at the token that causes it, all in one build:

- **Keys:** unknown or repeated keys; a value on a flag (`skip = true`); a key on the wrong
  derive's struct (`belongs_to` on an `Insert`).
- **Combinations:** `skip` with anything else; `flatten` with anything but itself; `json` with
  `try_from`; `generated` on a `skip` field.
- **`FromRow`:** an enum or a union, a tuple struct, lifetime parameters (owned values only).
- **`Table`:** no `table`, no `primary_key`, type parameters (the SQL names one table), `flatten`
  (a flattened struct's columns are not known at macro time).
- **`Insert`, `Patch`:** no `model`, type parameters. A `Patch` field that is not an `Option`; a
  `version` field when the model has none, or none when it has one (a `const` assertion, E0080).
- **Relations:** `has_many` without `as` or without a model; `has_many` on a model with a
  composite key; a `belongs_to` whose `key` is not a field of the struct, or is a `skip` field.

## B. What each derive generates

Each expansion below is quoted from shelf-codegen's expansion tests (Chapter 13, §13.4): the
generator's real output, pretty-printed, and kept true to it by the test and by this book's
excerpt check.

### `FromRow`

~~~rust
// orm/shelf-codegen/tests/expand/basic.rs (excerpt)
#[derive(FromRow)]
struct Product {
    id: i64,
    sku: String,
    r#type: String,
    tags: Option<Vec<String>>,
}
~~~

~~~rust
// orm/shelf-codegen/tests/expand/basic.expanded.rs (excerpt)
#[automatically_derived]
impl ::shelf::FromRow for Product {
    type Columns = (usize, usize, usize, usize);
    fn resolve(
        columns: &[::shelf::__private::tokio_postgres::Column],
    ) -> ::core::result::Result<Self::Columns, ::shelf::Error> {
        ::core::result::Result::Ok((
            ::shelf::__private::column::<i64>(columns, "Product", "id", "id")?,
            ::shelf::__private::column::<String>(columns, "Product", "sku", "sku")?,
            ::shelf::__private::column::<String>(columns, "Product", "type", "type")?,
            ::shelf::__private::column::<
                Option<Vec<String>>,
            >(columns, "Product", "tags", "tags")?,
        ))
    }
    fn from_row_at(
        row: &::shelf::__private::tokio_postgres::Row,
        columns: &Self::Columns,
    ) -> ::core::result::Result<Self, ::shelf::Error> {
        ::core::result::Result::Ok(Self {
            id: ::shelf::__private::get::<i64>(row, columns.0, "Product", "id")?,
            sku: ::shelf::__private::get::<String>(row, columns.1, "Product", "sku")?,
            r#type: ::shelf::__private::get::<
                String,
            >(row, columns.2, "Product", "type")?,
            tags: ::shelf::__private::get::<
                Option<Vec<String>>,
            >(row, columns.3, "Product", "tags")?,
        })
    }
}
~~~

[orm/shelf-codegen/tests/expand/basic.expanded.rs](../shelf-codegen/tests/expand/basic.expanded.rs)

`resolve` finds each column once per statement and checks its type (Chapter 5); `from_row_at` reads
each row by position. The raw identifier `r#type` reads the column `type`.

### `Table`

~~~rust
// orm/shelf-codegen/tests/expand/table.rs (excerpt)
#[derive(FromRow, Table)]
#[shelf(table = "products")]
struct Product {
    #[shelf(primary_key, generated)]
    id: i64,
    sku: String,
    r#type: String,
    #[shelf(skip)]
    selected: bool,
}
~~~

Besides `FromRow`'s impl, `Table` writes the trait impl and an inherent impl with the column
constants:

~~~rust
// orm/shelf-codegen/tests/expand/table.expanded.rs (excerpt)
#[automatically_derived]
impl ::shelf::Table for Product {
    type Key = i64;
    const MODEL: &'static ::core::primitive::str = "Product";
    const TABLE: &'static ::core::primitive::str = "\"products\"";
    const COLUMNS: &'static ::core::primitive::str = "\"id\", \"sku\", \"type\"";
    const FIND: &'static ::core::primitive::str = "SELECT \"id\", \"sku\", \"type\" FROM \"products\" WHERE \"id\" = $1";
    const ALL: &'static ::core::primitive::str = "SELECT \"id\", \"sku\", \"type\" FROM \"products\" ORDER BY \"id\"";
    const COUNT: &'static ::core::primitive::str = "SELECT count(*) FROM \"products\"";
    const UPDATE: &'static ::core::primitive::str = "UPDATE \"products\" SET \"sku\" = $1, \"type\" = $2 WHERE \"id\" = $3 RETURNING \"id\", \"sku\", \"type\"";
    const DELETE: &'static ::core::primitive::str = "DELETE FROM \"products\" WHERE \"id\" = $1";
    const KEY_COLUMNS: &'static [&'static ::core::primitive::str] = &["\"id\""];
    const VERSION: ::core::option::Option<&'static ::core::primitive::str> = ::core::option::Option::None;
    const UPDATED_AT: ::core::option::Option<&'static ::core::primitive::str> = ::core::option::Option::None;
    const FIELDS: &'static [::shelf::__private::Field] = &[
        ::shelf::__private::Field::new::<i64>("id", "id"),
        ::shelf::__private::Field::new::<String>("sku", "sku"),
        ::shelf::__private::Field::new::<String>("type", "type"),
    ];
    fn key(&self) -> Self::Key {
        ::core::clone::Clone::clone(&self.id)
    }
    fn key_params(
        key: &Self::Key,
    ) -> ::std::vec::Vec<
        &(dyn ::shelf::__private::tokio_postgres::types::ToSql + ::core::marker::Sync),
    > {
        ::std::vec::Vec::from([::shelf::__private::param(key)])
    }
    fn update_params(
        &self,
    ) -> ::std::vec::Vec<
        &(dyn ::shelf::__private::tokio_postgres::types::ToSql + ::core::marker::Sync),
    > {
        ::std::vec::Vec::from([
            ::shelf::__private::param(&self.sku),
            ::shelf::__private::param(&self.r#type),
        ])
    }
}
#[automatically_derived]
#[allow(dead_code)]
impl Product {
    /// The column `"id"`, for the query builder.
    const ID: ::shelf::query::Column<Self, i64> = ::shelf::query::Column::new("\"id\"");
    /// The column `"sku"`, for the query builder.
    const SKU: ::shelf::query::Column<Self, String> = ::shelf::query::Column::new(
        "\"sku\"",
    );
    /// The column `"type"`, for the query builder.
    const TYPE: ::shelf::query::Column<Self, String> = ::shelf::query::Column::new(
        "\"type\"",
    );
}
~~~

[orm/shelf-codegen/tests/expand/table.expanded.rs](../shelf-codegen/tests/expand/table.expanded.rs)

Every statement is a literal written at macro time, every identifier quoted (Chapter 6). `UPDATE`
writes every column that is neither key nor `generated` (Chapter 8). `FIELDS` is what `verify`
checks (Chapter 13). The constants are private because the fields are (Chapter 10).

### `Insert`

~~~rust
// orm/shelf-codegen/tests/expand/insert.rs (excerpt)
#[derive(Insert)]
#[shelf(model = Product)]
struct NewProduct<'a> {
    sku: &'a str,
    #[shelf(json)]
    attributes: Attributes,
    #[shelf(skip)]
    note: String,
}
~~~

~~~rust
// orm/shelf-codegen/tests/expand/insert.expanded.rs (excerpt)
#[automatically_derived]
impl<'a> ::shelf::Insert for NewProduct<'a> {
    type Model = Product;
    const INSERT: &'static ::core::primitive::str = {
        const PARTS: &[&::core::primitive::str] = &[
            "INSERT INTO ",
            <Product as ::shelf::Table>::TABLE,
            " (\"sku\", \"attributes\") VALUES ($1, $2) RETURNING ",
            <Product as ::shelf::Table>::COLUMNS,
        ];
        const BYTES: [::core::primitive::u8; ::shelf::__private::total_len(PARTS)] = ::shelf::__private::join(
            PARTS,
        );
        ::shelf::__private::utf8(&BYTES)
    };
    const COLUMNS: &'static [&'static ::core::primitive::str] = &[
        "\"sku\"",
        "\"attributes\"",
    ];
    const COPY: &'static ::core::primitive::str = {
        const PARTS: &[&::core::primitive::str] = &[
            "COPY ",
            <Product as ::shelf::Table>::TABLE,
            " (\"sku\", \"attributes\") FROM STDIN (FORMAT binary)",
        ];
        const BYTES: [::core::primitive::u8; ::shelf::__private::total_len(PARTS)] = ::shelf::__private::join(
            PARTS,
        );
        ::shelf::__private::utf8(&BYTES)
    };
    fn params(
        &self,
    ) -> ::std::vec::Vec<
        &(dyn ::shelf::__private::tokio_postgres::types::ToSql + ::core::marker::Sync),
    > {
        ::std::vec::Vec::from([
            ::shelf::__private::param(&self.sku),
            ::shelf::__private::param(
                ::shelf::__private::Json::from_ref(&self.attributes),
            ),
        ])
    }
    fn arrays(
        rows: &[Self],
    ) -> ::std::vec::Vec<
        ::std::boxed::Box<
            dyn ::shelf::__private::tokio_postgres::types::ToSql + ::core::marker::Sync + ::core::marker::Send + '_,
        >,
    > {
        ::std::vec::Vec::from([
            ::shelf::__private::array(rows, |row| &row.sku),
            ::shelf::__private::array(
                rows,
                |row| ::shelf::__private::Json::from_ref(&row.attributes),
            ),
        ])
    }
}
~~~

[orm/shelf-codegen/tests/expand/insert.expanded.rs](../shelf-codegen/tests/expand/insert.expanded.rs)

`INSERT` and `COPY` join the model's constants into one string at compile time, with two `const
fn`s (Chapter 6, §6.5; Chapter 8). `params` borrows from the struct, the `json` field through a
`#[repr(transparent)]` view; `arrays` gives `insert_many` one array per column (Chapter 12). With
`conflict` fields, a second impl, `Upsert`, adds `UPSERT` (`ON CONFLICT (…) DO UPDATE SET` every
other column; the same file's `ProductPrice`).

### `Patch`

~~~rust
// orm/shelf-codegen/tests/expand/patch.rs (excerpt)
#[derive(Patch)]
#[shelf(model = Product)]
struct ProductPatch {
    price_cents: Option<i64>,
    note: Option<Option<String>>,
    #[shelf(version)]
    version: i32,
}
~~~

~~~rust
// orm/shelf-codegen/tests/expand/patch.expanded.rs (excerpt)
#[automatically_derived]
impl ::shelf::Patch for ProductPatch {
    type Model = Product;
    const COLUMNS: &'static [&'static ::core::primitive::str] = &[
        "\"price_cents\"",
        "\"note\"",
    ];
    fn sql() -> &'static ::core::primitive::str {
        static SQL: ::std::sync::LazyLock<::std::string::String> = ::std::sync::LazyLock::new(||
        { ::shelf::__private::patch_sql::<Product>(&["\"price_cents\"", "\"note\""]) });
        ::std::string::String::as_str(&SQL)
    }
    fn params(
        &self,
    ) -> ::std::vec::Vec<
        &(dyn ::shelf::__private::tokio_postgres::types::ToSql + ::core::marker::Sync),
    > {
        ::std::vec::Vec::from([
            ::shelf::__private::flag(&self.price_cents),
            ::shelf::__private::param(&self.price_cents),
            ::shelf::__private::flag(&self.note),
            ::shelf::__private::param(&self.note),
            ::shelf::__private::param(&self.version),
        ])
    }
}
const _: () = {
    if <Product as ::shelf::Table>::VERSION.is_none() {
        ::core::panic!(
            "ProductPatch: the model Product has no `version` column, so the patch's #[shelf(version)] field has nothing to be checked against"
        );
    }
};
~~~

[orm/shelf-codegen/tests/expand/patch.expanded.rs](../shelf-codegen/tests/expand/patch.expanded.rs)

One statement for every patch: a flag and a value per field, `col = CASE WHEN $f THEN $v ELSE col
END`, built once at first use in a `LazyLock` because its `$n` numbers depend on the model's key
(Chapter 9). The `const` block turns a version that does not match the model into a build error.

### Every expansion test

| File | What it pins | Ch |
|---|---|---|
| `basic.rs` | `FromRow` by name, a raw identifier, an `Option` | 3, 5 |
| `rename.rs` | `rename_all`, and `rename` overriding it | 4 |
| `skip_flatten.rs` | `skip` (read as a default), `flatten` (a nested `resolve`) | 4 |
| `json_try_from.rs` | `json` through shelf's `Json<T>`, `try_from` with its source type | 4 |
| `generics.rs` | bounds only for the type parameters a field mentions | 4 |
| `table.rs` | `Table`: the statements, a composite key, a schema, a reserved word | 6 |
| `insert.rs` | `Insert` and `Upsert`: statements joined at compile time, a borrowed field, `json` | 8, 12 |
| `patch.rs` | `Patch`: flags and values, the `LazyLock`, the version assertion | 9 |
| `versions.rs` | `version` and `updated_at` in the model's `UPDATE`, `Stale` in the same round trip | 9 |
| `columns.rs` | the column constants: visibility, `try_from` and `json` types, none for `skip`; `FIELDS` | 10, 13 |
| `relations.rs` | `has_many` and `belongs_to`, a nullable foreign key, `as` | 11 |

To see the expansion of a struct of your own: run its derive's `expand` through
`shelf_playground::pretty` in a program (Chapter 3, §3.6), or `cargo expand` on a crate that uses
it.

## C. The API at a glance

| Part | Call | Returns | Ch |
|---|---|---|---|
| Mapping | `fetch_all::<T>(&db, sql, params)`, `fetch_one`, `fetch_opt` | `Vec<T>`, `T`, `Option<T>`; resolved before the query runs | 1, 5 |
| | `T::from_row(&row)`, `from_rows::<T>(&rows)` | one row; a slice, resolved once | 1, 5 |
| | tuples `(A,)` to 12 elements | by position | 2 |
| Reads | `M::find(&db, &key)`, `find_opt`, `all`, `count` | `M` or `Error::NotFound`; `Option<M>`; `Vec<M>` by key; `i64` | 6 |
| Writes | `new.insert(&db)`, `new.upsert(&db)` | `M` with its defaults | 8 |
| | `model.update(&db)`, `model.delete(&db)`, `M::delete_by_key(&db, &key)` | `M`; `()`; `NotFound` for no row, `Stale` for a version | 8, 9 |
| | `patch.apply(&db, &key)` | `M`, or `Stale`, or `NotFound` | 9 |
| Queries | `M::query().filter(…).order_by(…).limit(n).offset(n)` | a `Query<M>`: `.all`, `.first`, `.count`, `.exists`, `.stream`; `.sql()` and `.params()` | 10 |
| | `M::COL.eq(v)` … `between`, `any`, `like`, `is_null`, `contains`; `a.and(b)`, `a.or(b)`, `!a`; `.after(M::COL, v)` | conditions on `M` | 10 |
| | `Expr::raw(sql).bind(v)` | a condition in SQL, renumbered | 10 |
| Relations | `parent.children(&db)`, `Parent::load_children(&db, &parents)` | `Vec<C>`; `Vec<Vec<C>>` aligned with `parents` | 11 |
| | `child.parent(&db)`, `Child::load_parent(&db, &children)` | `P` (or `Option<P>`); `HashMap<Key, P>` | 11 |
| Bulk | `New::insert_many(&db, &rows)`, `New::copy_in(&db, &rows)` | `Vec<M>`; `u64` | 12 |
| Streams | `fetch_stream::<T>(&db, sql, params)`, `query.stream(&db)` | `ModelStream<'_, T>` | 12 |
| Checks | `verify::<M>(&db)` | `()` or `Error::Schema` with every mismatch | 13 |
| Executors | `&Client`, `&Transaction<'_>`, `&deadpool_postgres::Client`, `&deadpool_postgres::Transaction<'_>`, or your own `impl Executor` | | 7 |

`shelf::Error` has six variants: `Mapping { model, field, problem }` (with `Mapping::{MissingColumn,
WrongType, Null, Nullable, Decode}`), `NotFound { model }`, `Stale { model, current_version }`,
`Schema { model, mismatches }`, `Unsupported { model, message }`, and `Db(tokio_postgres::Error)`
(Chapter 7, §7.5). It is `#[non_exhaustive]`.

## D. Phrasebook

How a task reads in shelf and in the crates the side-by-side sections compared it with. sqlx and
SeaORM first; SeaORM 2.0.4 runs on sqlx 0.9.

| Task | shelf | sqlx 0.9 | SeaORM 2.0.4 | Ch |
|---|---|---|---|---|
| Rows into a struct | `#[derive(FromRow)]`, `fetch_all::<T>(&db, sql, params)` | `#[derive(sqlx::FromRow)]`, `query_as(sql).fetch_all(&pool)` | the entity's `Model`, from `DeriveEntityModel` | 1, 3 |
| Field attributes | `#[shelf(rename, skip, flatten, json, try_from)]` | `#[sqlx(rename, skip, flatten, json, try_from)]` | | 4 |
| Tuples | `fetch_all::<(String, i64)>` | `query_as::<_, (String, i64)>` | | 2 |
| Declare a model | `#[derive(FromRow, Table)]`, `#[shelf(table = "…")]`, `#[shelf(primary_key)]` | | `#[sea_orm::model]`, `#[sea_orm(table_name = "…")]`, `#[sea_orm(primary_key)]`, `impl ActiveModelBehavior for ActiveModel {}` | 6 |
| Find by key | `Product::find_opt(&db, &7)` | `query_as(sql).bind(7).fetch_optional(&pool)` | `product::Entity::find_by_id(7).one(&db)` | 6 |
| One function, any connection | `db: &impl shelf::Executor` | `db: impl PgExecutor<'_>` (by value; `Acquire` for two queries) | `db: &impl ConnectionTrait` | 7 |
| Insert, read back | `NewStore { … }.insert(&db)` | `query_as("INSERT … RETURNING …")` | `store::ActiveModel { code: Set(…), ..Default::default() }.insert(&db)` | 8 |
| Partial update | `#[derive(Patch)]`, `patch.apply(&db, &key)` | an `UPDATE` by hand | `update_many().set(active_model).filter(…).exec(&db)` | 9 |
| Optimistic locking | `#[shelf(version)]`, `Error::Stale` | by hand | a filter on the version, then `rows_affected` | 9 |
| A filtered page | `Product::query().filter(…).order_by(…).limit(20).all(&db)` | `QueryBuilder`, or SQL with optional parameters | `Entity::find().filter(…).order_by_asc(…)`, then `.all(&db)` | 10 |
| Children of many parents | `Order::load_lines(&db, &orders)` | `= ANY($1)` and a `HashMap`, by hand | `orders.load_many(order_line::Entity, &db)` | 11 |
| Bulk insert | `NewProduct::insert_many(&db, &rows)`, `NewProduct::copy_in(&db, &rows)` | `QueryBuilder::push_values`; `conn.copy_in_raw(sql)` | `Entity::insert_many(models).exec(&db)` | 12 |
| Stream rows | `query.stream(&db)`, `fetch_stream::<T>` | `.fetch(&pool)` | `.stream(&db)` | 12 |
| Check against the schema | `shelf::verify::<M>(&db)`, at startup | `query!`, `query_as!` at compile time (`.sqlx` offline) | | 13 |

And diesel, whose query builder Chapter 10 compared, with diesel-async for asynchronous code:

| Task | shelf | diesel 2.3.14 | Ch |
|---|---|---|---|
| The schema | the model struct and `#[shelf(table = "…")]` | `diesel::table!`, generated by `diesel print-schema` | 6 |
| A typed row | `#[derive(FromRow)]` | `#[derive(Queryable, Selectable)]` | 3 |
| A condition | `Product::PRICE_CENTS.gt(1_000)` | `products::price_cents.gt(1_000)` | 10 |
| Conditions added at run time | `query = query.filter(…)` | `.into_boxed()`, then `query = query.filter(…)` | 10 |
| Run it | `.all(&db).await` | `.load(&mut conn).await` (diesel-async) | 10 |
| Insert | `new.insert(&db)` | `diesel::insert_into(products::table).values(&new).get_result(&mut conn)` | 8 |
| Raw SQL | `fetch_all::<T>(&db, sql, params)`, `Expr::raw(sql).bind(v)` | `diesel::sql_query(sql).bind::<Text, _>(x).load(&mut conn)` | 10 |
| Children of many parents | `Order::load_lines(&db, &orders)` | `OrderLine::belonging_to(&orders).load(&mut conn)`, then `.grouped_by(&orders)` | 11 |

The differences that matter more than syntax: shelf writes its everyday SQL at compile time and
shows it as constants; it checks a model against the real database at startup rather than at
compile time; it never hides a round trip; and it is PostgreSQL only, so it uses `= ANY`, `unnest`,
binary COPY and the catalog directly (Chapter 0, §0.2).

## E. Glossary

**Batched loading.** Loading the children of many parents in one query, `"fk" = ANY($1)` with every
key in one array, then grouping in Rust: the cure for N+1 (Chapter 11).

**Column constant.** `Product::PRICE_CENTS: Column<Product, i64>`, generated by `Table` per field: a
quoted name with the model and the value type as markers (Chapter 10).

**Composite key.** A key of several columns, a tuple of the key fields' types in field order:
`Stock::find(&db, &(store_id, product_id))` (Chapter 6).

**Condition.** A filter on one model's rows: an `Expr<M>` from column methods, or a `Raw<M>` of SQL;
what `filter`, `and` and `or` accept (Chapter 10).

**Derive macro.** A procedural macro attached with `#[derive(…)]`: it receives the item, after `cfg`
evaluation and without the `#[derive]` line, and adds code beside it (Chapter 3).

**Drift.** A struct and its table no longer describing the same data, after a migration (Chapter
13).

**Executor.** shelf's trait for "something that can run a statement": a `Client`, a `Transaction`,
or a deadpool connection or transaction, each with its statement strategy (Chapter 7).

**Expansion.** The code a macro produces; for a derive, the items added after the struct. shelf
prints it with `pretty` and pins it with expansion tests (Chapters 3, 13).

**Expansion test.** A golden-file test: the generator's pretty-printed output compared with a file
committed beside its input; `SHELF_BLESS=1` rewrites the files, and their diff is the review
(Chapter 13).

**Flag (patch).** The boolean parameter beside each value of a `Patch` statement, `CASE WHEN $f THEN
$v ELSE col END`, that says whether the column is set (Chapter 9).

**Fragment specifier.** The kind of syntax a `macro_rules!` metavariable matches: `ident`, `ty`,
`expr`, `tt`, `literal` and others (Chapter 2).

**Generated (field).** A column the database fills, by default or computation; never written by
shelf's `update` (Chapters 6, 8).

**Helper attribute.** An attribute a derive declares (`attributes(shelf)`), so that the compiler
accepts it on the item and passes it to the derive (Chapter 4).

**Hygiene.** The rules that keep a macro's local names apart from the caller's. They do not cover
paths, which is why generated code names `::core::result::Result` in full (Chapters 2, 3).

**Insert struct.** A `New…` struct of the columns a new row supplies, with `model = …`: the database
fills the rest (Chapter 8).

**Keyset pagination.** Asking for the rows after the last one seen, `"col" > $n` ordered by a unique
column, rather than skipping rows with `OFFSET` (Chapter 10; main book §9.6).

**Lazy loading.** Loading a relation when a field is first read. shelf has none: every round trip is
a call with `&db` (Chapter 11).

**Loader.** A relation method that takes a slice of rows and loads what they relate to in one query:
`Order::load_lines`, `OrderLine::load_order` (Chapter 11).

**Model.** A `FromRow` struct that also knows its table, its key and its columns: `#[derive(Table)]`
(Chapter 6).

**ModelStream.** A stream of models read from a `RowStream` as rows arrive, its columns resolved
once; it borrows the executor it reads from (Chapter 12).

**N+1.** One query for a list, then one per item: a round trip per row, invisible on localhost
(Chapter 11).

**On-unimplemented message.** `#[diagnostic::on_unimplemented]` on a trait: the compiler's message
and label when a type does not implement it, stable since Rust 1.78. shelf uses it on `FromColumn`,
`Value`, `Condition`, `ForeignKey` and `Table` (Chapters 4, 10, 11).

**Orphan rule.** A trait impl must have the trait or the type local to the crate. It forbids `impl
TryFrom<&Row> for (A, B)` outside tokio-postgres (Chapter 1) and `impl From<shelf::Error> for
AppError` in the capstone (Chapter 14).

**Patch.** A struct of `Option` fields describing a partial update; `Option<Option<T>>` for a
nullable column (Chapter 9).

**PhantomData.** A zero-sized field that makes a struct carry a type parameter it does not store;
`PhantomData<fn() -> T>` keeps it `Send`, `Sync` and `Copy` whatever `T` is (Chapter 10).

**Pipelining.** Sending several requests on one connection without waiting for each answer; it
hides latency, not work (Chapter 11; main book Chapter 7).

**Raw fragment.** SQL inside a filter, `Expr::raw(sql).bind(v)`, its `$1…` renumbered among the
query's parameters (Chapter 10).

**Resolve.** shelf's step that finds each field's column and checks its type once per statement,
before the first row: `FromRow::resolve`, giving `Columns` (Chapter 5).

**Shape (of a query).** The filters present and their order, which decide its SQL text: one prepared
statement per shape, not per value (Chapter 10).

**Span.** Where a token came from in the source; errors are reported at spans, and a derive's
output keeps the spans of the tokens it copied (Chapters 3, 4).

**Stage program.** A runnable single-file version of a mechanism, such as a derive's logic as an
ordinary function, before the finished crate is shown (Chapter 0, §0.5).

**Stale.** `Error::Stale`: a write expected a version the row no longer has (Chapter 9).

**TokenStream.** The sequence of tokens a procedural macro receives and returns.
`proc_macro::TokenStream` exists only inside a macro; `proc_macro2::TokenStream` is the same thing
for any code, which is why shelf's generator lives in a library (Chapter 3).

**UI test.** A test that compiles broken code and compares the compiler's errors with a `.stderr`
file, with trybuild (Chapters 4, 13).

**Upsert.** An insert that updates the existing row on a unique-key conflict, `ON CONFLICT (…) DO
UPDATE`; derived with `conflict` fields (Chapter 8).

**Verify.** `shelf::verify::<M>(&db)`: a model checked against its table's description and the
catalog, every mismatch at once (Chapter 13).

## F. Further reading

- **Macros**: [The Little Book of Rust Macros](https://veykril.github.io/tlborm/); the Rust
  Reference on [macros by example](https://doc.rust-lang.org/reference/macros-by-example.html) and
  [procedural macros](https://doc.rust-lang.org/reference/procedural-macros.html), including
  helper attributes and the `#[diagnostic]` namespace's
  [attributes](https://doc.rust-lang.org/reference/attributes/diagnostics.html).
- **syn, quote and friends**: [syn 3.0.9](https://docs.rs/syn/3.0.9/syn/) and its
  [3.0.0 release notes](https://github.com/dtolnay/syn/releases/tag/3.0.0);
  [quote](https://docs.rs/quote/1.0.47/quote/);
  [proc-macro2](https://docs.rs/proc-macro2/1.0.107/proc_macro2/);
  [prettyplease 0.3.0](https://docs.rs/prettyplease/0.3.0/prettyplease/);
  [darling 0.24.1](https://docs.rs/darling/0.24.1/darling/), the attribute parser Chapter 4
  compared; [trybuild](https://docs.rs/trybuild/1.0.122/trybuild/);
  [cargo-expand](https://github.com/dtolnay/cargo-expand).
- **Practice**: David Tolnay's [proc-macro workshop](https://github.com/dtolnay/proc-macro-workshop),
  five derive and attribute macros to build, with tests.
- **`PhantomData` and variance**: the Rustonomicon's
  [PhantomData](https://doc.rust-lang.org/nomicon/phantom-data.html) page.
- **The other crates**: [diesel's guides](https://diesel.rs/guides/),
  [SeaORM's documentation](https://www.sea-ql.org/SeaORM/),
  [sqlx 0.9.0](https://docs.rs/sqlx/0.9.0/sqlx/), and
  [toasty](https://github.com/tokio-rs/toasty); the main book's
  [Appendix F](../../book/19-appendix.md#f-the-crates-built-on-tokio-postgres) on the crates built
  on tokio-postgres.
- **PostgreSQL**, for what shelf writes: [array functions](https://www.postgresql.org/docs/18/functions-array.html)
  (`unnest`, `= ANY`, `@>`), [row-wise comparison](https://www.postgresql.org/docs/18/functions-comparisons.html#ROW-WISE-COMPARISON),
  [`pg_attribute`](https://www.postgresql.org/docs/18/catalog-pg-attribute.html),
  [lexical structure](https://www.postgresql.org/docs/18/sql-syntax-lexical.html) (what a raw
  fragment's scanner skips), and [`COPY`](https://www.postgresql.org/docs/18/sql-copy.html).

---

Previous: [Chapter 14 — Capstone: stockroom on shelf](14-capstone.md)
