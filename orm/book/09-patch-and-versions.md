# Chapter 9 — Partial updates and optimistic locking

> **Building an ORM on tokio-postgres** · Previous: [Chapter 8 — Writing rows: insert, update, delete, upsert](08-writing.md) · Next: [Chapter 10 — A typed query builder](10-query-builder.md)

An admin edits a product's price and nothing else. The request body is `{"price_cents": 990}`,
and the update should touch that column only: another admin's change to the name, made a second
earlier, must survive. Chapter 8's `update` writes every column, so it would undo that change, as
its Exercise 2 showed. A partial update needs to know which fields were sent, and JSON has a
third state that `Option` cannot express: a field sent as `null`, which means "set it to NULL", is
not a field that was left out.

Then there is the edit that arrives late. Two tabs load version 7 of the product; both save.
Without a check, the second save silently overwrites the first. With a version column, the second
`UPDATE` finds no row at version 7, and the application can say so. This chapter builds both:
`#[derive(Patch)]`, in the one of three designs that measurement picked, and a `version`
attribute that turns a lost update into an error.

**In this chapter you will learn:**

- how to tell "absent" from "null" in a request body, with `Option<Option<T>>` and serde
- three ways to write a partial `UPDATE`, and what each costs in statements and round trips
- how `#[derive(Patch)]` generates the one shelf chose, and why its statement lives in a
  `LazyLock`
- how a version column turns a lost update into `Error::Stale`, told apart from `NotFound` in the
  same round trip
- what `updated_at = now()` records: the transaction's start, not the statement's
- how ETags and `If-Match` fit on top, in an axum handler

---

## 9.1 Absent or null

A partial update's fields have one more state than a model's:

| Column | Field type | Absent | `null` | A value |
|---|---|---|---|---|
| `price_cents bigint NOT NULL` | `Option<i64>` | `None`: leave it | `None` too | `Some(990)` |
| `note text` (nullable) | `Option<Option<String>>` | `None`: leave it | `Some(None)`: set NULL | `Some(Some("…"))` |

For a column that cannot be NULL, `Option<T>` is enough: `None` leaves the column as it is.
For a nullable column, the outer `Option` says whether to touch the column, the inner one what
to put there. Rust has the type; the catch is serde, which reads a JSON `null` into an
`Option<Option<T>>` as the outer `None`, the same as a missing field. A one-line deserializer
fixes that: it reads the field as an `Option<T>` and wraps whatever it gets in `Some`, so a
`null` becomes `Some(None)`; `#[serde(default)]` keeps a missing field `None`, since the
deserializer then never runs:

```rust
use serde::{Deserialize, Deserializer};

/// A field that may be absent, `null` or a value: `null` becomes `Some(None)`. With
/// `#[serde(default)]`, an absent field stays `None`.
fn double_option<'de, T, D>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

/// serde's own reading of `Option<Option<T>>`.
#[derive(Debug, PartialEq, Deserialize)]
struct Naive {
    note: Option<Option<String>>,
}

#[derive(Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProductPatch {
    price_cents: Option<i64>,
    #[serde(default, deserialize_with = "double_option")]
    note: Option<Option<String>>,
}

fn main() {
    let read = |json| serde_json::from_str::<ProductPatch>(json).unwrap();
    let naive = |json| serde_json::from_str::<Naive>(json).unwrap();

    for body in [r#"{}"#, r#"{"note": null}"#, r#"{"note": "fragile"}"#] {
        println!(
            "{body:<22} naive {:?}, patch {:?}",
            naive(body).note,
            read(body).note
        );
    }
    assert_eq!(
        naive(r#"{"note": null}"#),
        naive(r#"{}"#),
        "serde cannot tell them apart"
    );
    assert_eq!(read(r#"{}"#).note, None);
    assert_eq!(read(r#"{"note": null}"#).note, Some(None));
    assert_eq!(
        read(r#"{"note": "fragile"}"#).note,
        Some(Some("fragile".into()))
    );

    // A null for a column that cannot be NULL reads as `None`: "leave it".
    assert_eq!(read(r#"{"price_cents": null}"#).price_cents, None);
    // And a misspelt field is refused rather than ignored.
    assert!(serde_json::from_str::<ProductPatch>(r#"{"price": 990}"#).is_err());
}
```

```text
{}                     naive None, patch None
{"note": null}         naive None, patch Some(None)
{"note": "fragile"}    naive Some(Some("fragile")), patch Some(Some("fragile"))
```

The function is what serde_with 3.24 ships as `serde_with::rust::double_option` (with a
serializer beside it), used as `#[serde(default, with = "::serde_with::rust::double_option")]`;
for one function, a crate is optional. `deny_unknown_fields` matters more in a patch than
anywhere else: without it, `{"price": 990}` deserializes to an empty patch, which succeeds and
changes nothing, and the client is told it worked. A `null` sent for a column that cannot be
NULL is read as "leave it"; refusing it instead takes a deserializer of its own, which reads the
field as a `T` and so fails on `null`.

## 9.2 Three designs

There are three ways to send a partial update, and each gives something up. The first builds the
`SET` list at run time from the fields that are present: the SQL is exactly what you would write
by hand, but every combination of fields is a different statement, prepared and cached
separately. The second sends every column, and writes `col = coalesce($n, col)`: one statement
for every patch, but `coalesce` cannot tell a NULL you want from a value you did not send, so it
can never set a column to NULL. The third sends a flag beside each value, `col = CASE WHEN $f
THEN $v ELSE col END`: one statement, NULLs included, at the price of twice the parameters.

| Design | Statement for `price_cents` only | Statements per model | Can set NULL |
|---|---|---|---|
| (a) a `SET` list per shape | `UPDATE … SET price_cents = $1 WHERE id = $2` | up to 2ⁿ − 1 for n fields | yes |
| (b) `coalesce` | `UPDATE … SET name = coalesce($1, name), price_cents = coalesce($2, price_cents), … WHERE id = $n` | 1 | no |
| (c) a flag per field | `UPDATE … SET name = CASE WHEN $1 THEN $2 ELSE name END, … WHERE id = $n` | 1 | yes |

This program runs all three on one pooled connection, through lagproxy, with fourteen patches:
every non-empty combination of three fields, once setting the nullable `note` to a value and once
to NULL. It counts the statements each design leaves in deadpool's cache and the NULLs it
managed to write, and times the patches:

<!-- check: needs=lagproxy -->
```rust
use std::time::{Duration, Instant};
use tokio_postgres::types::ToSql;

/// A patch of three columns, one nullable: `Some(None)` sets `note` to NULL.
struct Patch {
    name: Option<String>,
    price_cents: Option<i64>,
    note: Option<Option<String>>,
}

/// Every non-empty shape of the three fields, twice: `note` set to a value, then to NULL.
fn patches() -> Vec<Patch> {
    let mut all = Vec::new();
    for shape in 1..8 {
        for null in [false, true] {
            all.push(Patch {
                name: (shape & 1 != 0).then(|| "Blue mug".to_string()),
                price_cents: (shape & 2 != 0).then_some(990),
                note: (shape & 4 != 0).then(|| (!null).then(|| "fragile".to_string())),
            });
        }
    }
    all
}

const ID: i64 = 1;

/// (a) The `SET` list of the fields that are present: one statement per shape.
fn set_list(patch: &Patch) -> (String, Vec<&(dyn ToSql + Sync)>) {
    let mut set = Vec::new();
    let mut params: Vec<&(dyn ToSql + Sync)> = Vec::new();
    if let Some(name) = &patch.name {
        params.push(name);
        set.push(format!("name = ${}", params.len()));
    }
    if let Some(price_cents) = &patch.price_cents {
        params.push(price_cents);
        set.push(format!("price_cents = ${}", params.len()));
    }
    if let Some(note) = &patch.note {
        params.push(note);
        set.push(format!("note = ${}", params.len()));
    }
    params.push(&ID);
    let sql = format!(
        "UPDATE products SET {} WHERE id = ${}",
        set.join(", "),
        params.len()
    );
    (sql, params)
}

/// (b) Every column, `coalesce`d: NULL means "leave it", so NULL cannot be written.
const COALESCE: &str = "UPDATE products SET
    name = coalesce($1, name),
    price_cents = coalesce($2, price_cents),
    note = coalesce($3, note)
    WHERE id = $4";

/// (c) Every column, with a flag that says whether to set it.
const FLAGS: &str = "UPDATE products SET
    name = CASE WHEN $1 THEN $2 ELSE name END,
    price_cents = CASE WHEN $3 THEN $4 ELSE price_cents END,
    note = CASE WHEN $5 THEN $6 ELSE note END
    WHERE id = $7";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let pool = shelf_playground::pool(1);
    let db = pool.get().await?;
    db.batch_execute(
        "CREATE TEMP TABLE products (
             id bigint PRIMARY KEY, name text NOT NULL, price_cents bigint NOT NULL, note text
         );
         INSERT INTO products VALUES (1, 'Mug', 1299, 'new');",
    )
    .await?;

    let mut results = Vec::new();
    for design in ["(a) SET list", "(b) coalesce", "(c) flags"] {
        let cached_before = db.statement_cache.size();
        let (mut nulls_wanted, mut nulls_written) = (0, 0);
        let mut elapsed = Duration::ZERO;
        for patch in patches() {
            db.batch_execute("UPDATE products SET name = 'Mug', price_cents = 1299, note = 'new'")
                .await?;
            let started = Instant::now();
            match design {
                "(a) SET list" => {
                    let (sql, params) = set_list(&patch);
                    let statement = db.prepare_cached(&sql).await?;
                    db.execute(&statement, &params).await?;
                }
                "(b) coalesce" => {
                    let note = patch.note.clone().flatten(); // absent and null: both None
                    let statement = db.prepare_cached(COALESCE).await?;
                    db.execute(&statement, &[&patch.name, &patch.price_cents, &note, &ID])
                        .await?;
                }
                _ => {
                    let flags = [
                        patch.name.is_some(),
                        patch.price_cents.is_some(),
                        patch.note.is_some(),
                    ];
                    let statement = db.prepare_cached(FLAGS).await?;
                    let params: [&(dyn ToSql + Sync); 7] = [
                        &flags[0],
                        &patch.name,
                        &flags[1],
                        &patch.price_cents,
                        &flags[2],
                        &patch.note,
                        &ID,
                    ];
                    db.execute(&statement, &params).await?;
                }
            }
            elapsed += started.elapsed();
            if patch.note == Some(None) {
                nulls_wanted += 1;
                let note: Option<String> = db
                    .query_one("SELECT note FROM products WHERE id = 1", &[])
                    .await?
                    .get(0);
                nulls_written += i32::from(note.is_none());
            }
        }
        let statements = db.statement_cache.size() - cached_before;
        let per_patch = elapsed / patches().len() as u32;
        println!(
            "{design:<13} {statements} statement(s), {nulls_written} of {nulls_wanted} NULLs written, {per_patch:.1?} a patch"
        );
        results.push((statements, nulls_written, per_patch));
    }

    let [
        (a_statements, a_nulls, a_time),
        (b_statements, b_nulls, _),
        (c_statements, c_nulls, c_time),
    ] = results[..]
    else {
        unreachable!("three designs");
    };
    assert_eq!((a_statements, b_statements, c_statements), (7, 1, 1));
    assert_eq!((a_nulls, b_nulls, c_nulls), (4, 0, 4));
    assert!(a_time > c_time * 6 / 5, "each new shape costs a prepare");
    Ok(())
}
```

```text
(a) SET list  7 statement(s), 4 of 4 NULLs written, 22.1ms a patch
(b) coalesce  1 statement(s), 0 of 4 NULLs written, 15.6ms a patch
(c) flags     1 statement(s), 4 of 4 NULLs written, 15.8ms a patch
```

Three runs (a debug build, lagproxy at 5 ms each way) gave 22.0–22.7 ms a patch for (a),
15.6–16.4 ms for (b) and 15.5–15.8 ms for (c): half of (a)'s fourteen patches are a shape it has
not seen, and each of those costs a prepare. The measurement that chose shelf's design (PLAN.md
§12, finding 33) ran longer, 70 patches cycling through the seven shapes, 20 of them writing
NULL, three runs in a release build through lagproxy at 5 ms each way, so that (a)'s prepares
were spread thinner:

| Design | Statements cached | NULLs written | Per patch |
|---|---|---|---|
| (a) a `SET` list per shape | 7 | 20 of 20 | 15.0–15.8 ms |
| (b) `coalesce` | 1 | 0 of 20 | 13.8–14.6 ms |
| (c) a flag per field | 1 | 20 of 20 | 13.9–14.7 ms |

(b) is out: a patch API that cannot clear a field is a bug report waiting to happen. Between (a)
and (c), the numbers say that a flag per field costs nothing measurable in time, while a `SET`
list per shape costs a prepare per new shape on every pooled connection, and a cache that grows
with the number of shapes in use: with n patchable fields, up to 2ⁿ − 1 statements, on each
connection of the pool. shelf uses (c).

Design (c) has a catch that had to be checked: it sends a typed value for every field, even the
fields a patch leaves alone. An unset `Option<i64>` is sent as a NULL of type `int8` with its flag
false, and the server never looks at it. That held for every column type tried (text, integers,
`bool`, `float8`, `jsonb`, `text[]`, `timestamptz`, `uuid`, an enum and a domain), and the
domain had a bonus: in a `CASE` or a `coalesce`, PostgreSQL types the parameter as the domain's
base type, so a patch can write a `String` to a `sku` column that Chapter 8's `INSERT` and
`update` refuse it for. The domain's check still runs when the value is assigned (PLAN.md §12,
finding 30, and shelf's test `a_domain_column_takes_a_string_in_a_patch_but_not_in_an_update`).

## 9.3 `#[derive(Patch)]`

A patch is a struct of `Option` fields that names its model. It usually derives `Deserialize`
too, so that the request body is the patch:

~~~rust
#[derive(Debug, Default, Deserialize, Patch)]
#[serde(deny_unknown_fields)]
#[shelf(model = Product)]
struct ProductPatch {
    name: Option<String>,
    price_cents: Option<i64>,
    #[serde(default, deserialize_with = "double_option")]
    note: Option<Option<String>>,
}
~~~

The derive generates design (c). Running it on a two-field patch shows what it emits:

```rust
use syn::{DeriveInput, parse_quote};

fn main() {
    let input: DeriveInput = parse_quote! {
        #[shelf(model = Product)]
        struct ProductPatch {
            price_cents: Option<i64>,
            note: Option<Option<String>>,
        }
    };
    let code = shelf_playground::pretty(shelf_codegen::patch::expand(&input).unwrap());
    println!("{code}");
    assert!(code.contains("::shelf::__private::flag(&self.price_cents)"));
    assert!(code.contains("static SQL: ::std::sync::LazyLock<::std::string::String>"));
}
```

```text
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
        ])
    }
}
const _: () = {
    if <Product as ::shelf::Table>::VERSION.is_some() {
        ::core::panic!(
            "ProductPatch: the model Product has a `version` column, so the patch needs a #[shelf(version)] field holding the version it was made against"
        );
    }
};
```

Three things are generated. `COLUMNS` lists the columns the patch can set, quoted at macro time.
`params()` lists a flag and a value per field, in the order of the `$n` in the statement, each
borrowed from the patch: `flag` returns a `&'static` reference to `true` or `false`, so the
flags need no storage of their own. And `sql()` returns the statement, which is not a constant.

### Why a `LazyLock`

The statement is `UPDATE "products" SET "price_cents" = CASE WHEN $1 THEN $2 ELSE
"price_cents" END, … WHERE "id" = $5`. The derive knows its own columns, but the table, the
key's columns and whether the model has a version and an `updated_at` are the model's constants,
and the key's placeholder numbers depend on how many parameters the patch has before them. That
is the computing §6.5 said joining cannot do: the `$5` is a number derived from another type's
constants. So the derive emits a `static` `LazyLock`, which calls shelf's `patch_sql::<Product>`
the first time `sql()` is called, and keeps the `String` for the rest of the program.

> **Rust refresher:** `std::sync::LazyLock<T>` (stable since Rust 1.80.0) holds a value that is
> computed on first access, by the closure given to `LazyLock::new`, and is the same value for
> every access after, from every thread: the first caller runs the closure while any others
> wait. A `static` must be initialised by a constant expression, and `LazyLock::new(closure)`
> is one, which is what lets a `static` hold something built at run time. Before 1.80 the
> `once_cell` and `lazy_static` crates did this job. Here, each patch type's `sql()` has its own
> `static`, since each impl is a separate function; a `static` inside a generic function is one
> item shared by every instantiation, and cannot name the function's type parameters (§6.5),
> which is one reason `Patch`, like `Table` and `Insert`, refuses type parameters.

`patch_sql` is ordinary run-time code, the counterpart of Chapter 6's `format!`s, run once per
patch type:

~~~rust
// orm/shelf/src/patch.rs (excerpt)
pub fn patch_sql<M: Table>(columns: &[&str]) -> String {
    let mut set: Vec<String> = columns
        .iter()
        .enumerate()
        .map(|(i, c)| {
            format!(
                "{c} = CASE WHEN ${} THEN ${} ELSE {c} END",
                2 * i + 1,
                2 * i + 2
            )
        })
        .collect();
    let mut next = 2 * columns.len() + 1;
    if let Some(version) = M::VERSION {
        set.push(format!("{version} = {version} + 1"));
    }
    if let Some(updated_at) = M::UPDATED_AT {
        set.push(format!("{updated_at} = now()"));
    }
    // … the version's parameter, then the key's: `"id" = $n` from M::KEY_COLUMNS
    let key_where = key_where.join(" AND ");
    let (table, set) = (M::TABLE, set.join(", "));
    let columns = M::COLUMNS;
    match (M::VERSION, version_param) {
        // … with a version: §9.4's statement
        _ => format!("UPDATE {table} SET {set} WHERE {key_where} RETURNING {columns}"),
    }
}
~~~

[orm/shelf/src/patch.rs](../shelf/src/patch.rs)

`apply(&db, &key)` appends the key's parameters to `params()` and sends `sql()` through the same
`written` helper as Chapter 8's `update`, so a patch returns the row as it now is, and a key with
no row is `NotFound`. This program deserializes three request bodies into one patch type and
applies each:

```rust
use serde::{Deserialize, Deserializer};
use shelf::{FromRow, Patch, Table};

fn double_option<'de, T, D>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

#[derive(Debug, FromRow, Table)]
#[shelf(table = "products")]
struct Product {
    #[shelf(primary_key)]
    id: i64,
    name: String,
    price_cents: i64,
    note: Option<String>,
}

#[derive(Debug, Deserialize, Patch)]
#[serde(deny_unknown_fields)]
#[shelf(model = Product)]
struct ProductPatch {
    name: Option<String>,
    price_cents: Option<i64>,
    #[serde(default, deserialize_with = "double_option")]
    note: Option<Option<String>>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = shelf_playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE products (
                 id bigint PRIMARY KEY, name text NOT NULL, price_cents bigint NOT NULL, note text
             );
             INSERT INTO products VALUES (1, 'Mug', 1299, 'fragile');",
        )
        .await?;
    println!("{}", ProductPatch::sql());

    for body in [
        r#"{"price_cents": 990}"#,
        r#"{"note": null}"#,
        r#"{"name": "Blue mug", "note": "chipped"}"#,
    ] {
        let patch: ProductPatch = serde_json::from_str(body)?;
        let product = patch.apply(&client, &1).await?;
        println!(
            "{body:<40} -> {} | {} | {:?}",
            product.name, product.price_cents, product.note
        );
    }
    let product = Product::find(&client, &1).await?;
    assert_eq!(
        (
            product.name.as_str(),
            product.price_cents,
            product.note.as_deref()
        ),
        ("Blue mug", 990, Some("chipped"))
    );

    let missing = ProductPatch {
        name: None,
        price_cents: Some(1),
        note: None,
    };
    let error = missing.apply(&client, &2).await.unwrap_err();
    assert!(matches!(error, shelf::Error::NotFound { model: "Product" }));
    Ok(())
}
```

```text
UPDATE "products" SET "name" = CASE WHEN $1 THEN $2 ELSE "name" END, "price_cents" = CASE WHEN $3 THEN $4 ELSE "price_cents" END, "note" = CASE WHEN $5 THEN $6 ELSE "note" END WHERE "id" = $7 RETURNING "id", "name", "price_cents", "note"
{"price_cents": 990}                     -> Mug | 990 | Some("fragile")
{"note": null}                           -> Mug | 990 | None
{"name": "Blue mug", "note": "chipped"}  -> Blue mug | 990 | Some("chipped")
```

Each body touched only what it named, and the `null` cleared the note. One statement served all
three, prepared on each call on this plain `Client`, and once per connection on a pooled one.

### What `Patch` refuses

The UI test pins what the derive reports:

~~~rust
// orm/shelf/tests/ui/patch.rs (excerpt)
#[derive(FromRow, Table)]
#[shelf(table = "products")]
struct Product {
    #[shelf(primary_key)]
    id: i64,
    sku: String,
    #[shelf(version)]
    version: i32,
}

#[derive(Patch)]
#[shelf(model = Product)]
struct NotOptions {
    sku: String,
    #[shelf(json)]
    note: Option<String>,
    #[shelf(version)]
    version: i32,
}

#[derive(Patch)]
#[shelf(model = Product)]
struct NoVersion {
    sku: Option<String>,
}
~~~

[orm/shelf/tests/ui/patch.rs](../shelf/tests/ui/patch.rs)

```text
error: a Patch field is an `Option`: `None` leaves the column as it is and `Some` sets it (`Option<Option<T>>` for a nullable column, whose `Some(None)` sets NULL)
  --> tests/ui/patch.rs:18:10
   |
18 |     sku: String,
   |          ^^^^^^

error: a Patch writes each value as it is, so `json` has no meaning here: for a json column, write the field as `Option<tokio_postgres::types::Json<T>>` or `Option<serde_json::Value>`
  --> tests/ui/patch.rs:19:13
   |
19 |     #[shelf(json)]
   |             ^^^^

error[E0080]: evaluation panicked: NoVersion: the model Product has a `version` column, so the patch needs a #[shelf(version)] field holding the version it was made against
  --> tests/ui/patch.rs:25:10
   |
25 | #[derive(Patch)]
   |          ^^^^^ evaluation of `_` failed here
```

A `json` field on a patch needs no attribute: a field of type
`Option<tokio_postgres::types::Json<T>>`, or `Option<serde_json::Value>`, writes JSON through its
own `ToSql`, which is what the attribute does on a model (shelf's test
`a_json_column_is_patched_through_json`; the capstone's `ProductPatch` has an
`Option<Value>`).

> **Macro lens:** the last error is a rule that spans two types: a patch of a model with a
> `version` must carry the version it was made against, and a patch of a model without one must
> not. The `Patch` derive cannot see the model's fields, but the compiler can evaluate the
> model's constant, so the derive emits `const _: () = { if <Product as Table>::VERSION.is_none()
> { panic!("…") } };` (printed above, with `is_some` for a patch without a version field). A
> panic during constant evaluation is a compile error, E0080, with the panic's message, and the
> derive points it at `Patch` in the `#[derive]`, through the span of the code it generates. It
> needs `Option::is_none` to be a `const fn`, which it is.

## 9.4 Version columns

A version column makes every write say which version of the row it was made against. Mark the
model's field `#[shelf(version)]`, and three things change:

- `update` and every patch add `"version" = "version" + 1` to their `SET`;
- they write only `WHERE "id" = $n AND "version" = $m`, with the version the caller holds;
- the patch type needs a `#[shelf(version)]` field, the version it was made against.

When the row is not at that version, the `UPDATE` changes nothing. Zero rows then has two
causes: the row is gone (`NotFound`), or someone else wrote it first (`Stale`). They need
different answers, a 404 and a 412, and a 412 is more useful with the current version in it, so
that the client can fetch the row again and retry. stockroom tells them apart with a second
query when the update finds nothing. shelf does it in the same statement:

~~~sql
WITH updated AS (
    UPDATE "products" SET … , "version" = "version" + 1
    WHERE "id" = $6 AND "version" = $5
    RETURNING "id", "name", …
)
SELECT "id", "name", …,
       (SELECT count(*) FROM updated) AS shelf_updated,
       (SELECT "version"::bigint FROM "products" WHERE "id" = $6) AS shelf_version
FROM (VALUES (1)) AS one LEFT JOIN updated ON true
~~~

The statement always returns exactly one row: `VALUES (1)` provides it, and the `LEFT JOIN`
fills in the updated row's columns when there is one, and NULLs when there is not.
`shelf_updated` says which, and `shelf_version` is the row's version as the statement found it,
NULL when there is no row. So:

| `shelf_updated` | `shelf_version` | Result |
|---|---|---|
| 1 | the old version | the updated row |
| 0 | a version | `Error::Stale { current_version }` |
| 0 | NULL | `Error::NotFound` |

> **SQL lens:** every part of a statement with a data-modifying `WITH` sees the same snapshot,
> taken before any of it ran, so the outer `SELECT … FROM "products"` does not see the
> `UPDATE`'s new row version; that is why `shelf_version` is the old version after a success
> ([WITH queries, data-modifying statements](https://www.postgresql.org/docs/18/queries-with.html#QUERIES-WITH-MODIFYING)).
> It is also why the `RETURNING` of the `UPDATE`, not a second read of the table, supplies the
> row: only `RETURNING` has the new values.

`written`, in shelf, reads that row: the model from its columns when `shelf_updated` is 1, else
`Stale` or `NotFound` from `shelf_version`.

~~~rust
// orm/shelf/src/write.rs (excerpt)
pub(crate) async fn written<M: Table>(
    db: &impl Executor,
    sql: &str,
    params: &[&(dyn ToSql + Sync)],
) -> Result<M, Error> {
    let not_found = Error::NotFound { model: M::MODEL };
    if M::VERSION.is_none() {
        return fetch_opt(db, sql, params).await?.ok_or(not_found);
    }
    let statement = db.prepare(sql).await?;
    let columns = M::resolve(statement.columns())?;
    let row = db.query_one(&statement, params).await?;
    if row.try_get::<_, i64>("shelf_updated")? == 1 {
        return M::from_row_at(&row, &columns);
    }
    match row.try_get::<_, Option<i64>>("shelf_version")? {
        Some(current_version) => Err(Error::Stale {
            model: M::MODEL,
            current_version,
        }),
        None => Err(not_found),
    }
}
~~~

[orm/shelf/src/write.rs](../shelf/src/write.rs)

The model is resolved against the statement's columns as always; the two extra columns are
simply not fields of it. Two tabs, then, each holding version 1:

```rust
use shelf::{FromRow, Patch, Table};

#[derive(Debug, Clone, FromRow, Table)]
#[shelf(table = "products")]
struct Product {
    #[shelf(primary_key)]
    id: i64,
    name: String,
    price_cents: i64,
    #[shelf(version)]
    version: i32,
}

#[derive(Debug, Default, Patch)]
#[shelf(model = Product)]
struct ProductPatch {
    name: Option<String>,
    price_cents: Option<i64>,
    #[shelf(version)]
    version: i32,
}

#[tokio::main]
async fn main() -> Result<(), shelf::Error> {
    let client = shelf_playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE products (
                 id bigint PRIMARY KEY, name text NOT NULL, price_cents bigint NOT NULL,
                 version integer NOT NULL DEFAULT 1
             );
             INSERT INTO products (id, name, price_cents) VALUES (1, 'Mug', 1299);",
        )
        .await?;

    // Both tabs load the product at version 1.
    let tab_a = Product::find(&client, &1).await?;
    let tab_b = Product::find(&client, &1).await?;

    let saved = ProductPatch {
        price_cents: Some(990),
        version: tab_a.version,
        ..ProductPatch::default()
    }
    .apply(&client, &1)
    .await?;
    assert_eq!(saved.version, 2);

    // Tab B saves against version 1, which is gone.
    let error = ProductPatch {
        name: Some("Blue mug".into()),
        version: tab_b.version,
        ..ProductPatch::default()
    }
    .apply(&client, &1)
    .await
    .unwrap_err();
    println!("{error}");
    let shelf::Error::Stale {
        current_version, ..
    } = error
    else {
        panic!("stale");
    };

    // Tab B reads again, sees the new price, and saves at the current version.
    let fresh = Product::find(&client, &1).await?;
    assert_eq!(
        (i64::from(fresh.version), fresh.price_cents),
        (current_version, 990)
    );
    let saved = ProductPatch {
        name: Some("Blue mug".into()),
        version: fresh.version,
        ..ProductPatch::default()
    }
    .apply(&client, &1)
    .await?;
    assert_eq!(
        (saved.name.as_str(), saved.price_cents, saved.version),
        ("Blue mug", 990, 3)
    );

    // Chapter 8's `update` checks the version too: tab B's stale copy is refused.
    let mut stale_copy = tab_b.clone();
    stale_copy.price_cents = 1;
    assert!(matches!(
        stale_copy.update(&client).await,
        Err(shelf::Error::Stale {
            current_version: 3,
            ..
        })
    ));
    // And a row that does not exist is NotFound, from the same statement.
    let error = ProductPatch {
        version: 1,
        price_cents: Some(1),
        ..ProductPatch::default()
    }
    .apply(&client, &2)
    .await
    .unwrap_err();
    assert!(matches!(error, shelf::Error::NotFound { .. }));
    Ok(())
}
```

```text
Product has changed since it was read: it is now at version 2
```

The loss of Chapter 8's Exercise 2 is now an error, and the error carries what the client needs
to recover. For a model with a version, `update` runs the same kind of statement, so a stale copy
of the whole model is refused as well; and since `update`'s `SET` comes from the copy's fields,
the version is the only guard it has against overwriting a newer row.

## 9.5 `updated_at`

`#[shelf(updated_at)]` on a timestamp field adds `"updated_at" = now()` to every update and every
patch. The field is read like any other, so the row a write returns has the new time in it.

> **SQL lens:** `now()` is the start time of the current *transaction*, not of the statement,
> so its value does not change while the transaction lasts, and PostgreSQL's documentation calls
> that a feature; `clock_timestamp()` is the one that changes from call to call
> ([Current Date/Time](https://www.postgresql.org/docs/18/functions-datetime.html#FUNCTIONS-DATETIME-CURRENT)).
> So two writes in one transaction record the same instant, which is usually what you want: the
> rows a transaction changed all say when it happened. Outside a transaction, each statement is
> a transaction of its own, and each gets its own time.

```rust
use jiff::Timestamp;
use shelf::{FromRow, Patch, Table};
use std::time::Duration;

#[derive(Debug, FromRow, Table)]
#[shelf(table = "products")]
struct Product {
    #[shelf(primary_key)]
    id: i64,
    price_cents: i64,
    #[shelf(updated_at)]
    updated_at: Timestamp,
}

#[derive(Debug, Patch)]
#[shelf(model = Product)]
struct ProductPatch {
    price_cents: Option<i64>,
}

fn price(price_cents: i64) -> ProductPatch {
    ProductPatch {
        price_cents: Some(price_cents),
    }
}

#[tokio::main]
async fn main() -> Result<(), shelf::Error> {
    let mut client = shelf_playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE products (
                 id bigint PRIMARY KEY, price_cents bigint NOT NULL,
                 updated_at timestamptz NOT NULL DEFAULT now()
             );
             INSERT INTO products (id, price_cents) VALUES (1, 1299);",
        )
        .await?;

    // Two patches in one transaction, 20 ms apart: one instant.
    let tx = client.transaction().await?;
    let first = price(990).apply(&tx, &1).await?;
    tokio::time::sleep(Duration::from_millis(20)).await;
    let second = price(980).apply(&tx, &1).await?;
    tx.commit().await?;
    assert_eq!(first.updated_at, second.updated_at);

    // The same two outside a transaction: each statement has its own time.
    let first = price(970).apply(&client, &1).await?;
    tokio::time::sleep(Duration::from_millis(20)).await;
    let second = price(960).apply(&client, &1).await?;
    assert!(
        second.updated_at.duration_since(first.updated_at) >= jiff::SignedDuration::from_millis(20)
    );
    Ok(())
}
```

## 9.6 ETags and `If-Match` in axum

HTTP has the version check built in. A `GET` returns the version as an `ETag` header; a client
that wants to change the resource sends the tag back in `If-Match`; and the server answers 412
Precondition Failed when the resource no longer has that tag
([main book Chapter 15](../../book/15-axum-api.md)). With a version column, the tag is the
version, and `If-Match` is the patch's `version` field. This program serves `GET` and `PATCH` for
products with axum, and drives the router through `oneshot` (the playground's `call`), without a
port:

```rust
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, Request, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;
use shelf::{FromRow, Patch, Table};
use std::sync::Arc;

#[derive(Debug, Serialize, FromRow, Table)]
#[shelf(table = "products")]
struct Product {
    #[shelf(primary_key)]
    id: i64,
    name: String,
    price_cents: i64,
    #[shelf(version)]
    version: i32,
}

#[derive(Debug, Deserialize, Patch)]
#[serde(deny_unknown_fields)]
#[shelf(model = Product)]
struct ProductPatch {
    name: Option<String>,
    price_cents: Option<i64>,
    /// Not in the body: the handler sets it from If-Match.
    #[serde(skip)]
    #[shelf(version)]
    version: i32,
}

type Db = Arc<tokio_postgres::Client>;

/// A product, with its version as the ETag.
fn product(product: Product) -> Response {
    let etag = format!("\"{}\"", product.version);
    ([(header::ETAG, etag)], Json(product)).into_response()
}

/// shelf's errors as responses: what the client can act on, and nothing else.
fn failure(error: shelf::Error) -> Response {
    let (status, body) = match error {
        shelf::Error::NotFound { .. } => (StatusCode::NOT_FOUND, json!({"error": "not found"})),
        shelf::Error::Stale {
            current_version, ..
        } => (
            StatusCode::PRECONDITION_FAILED,
            json!({"error": "changed since you read it", "currentVersion": current_version}),
        ),
        _ => (
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({"error": "internal error"}),
        ),
    };
    (status, Json(body)).into_response()
}

async fn show(State(db): State<Db>, Path(id): Path<i64>) -> Response {
    match Product::find(&*db, &id).await {
        Ok(found) => product(found),
        Err(error) => failure(error),
    }
}

async fn edit(
    State(db): State<Db>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Json(mut patch): Json<ProductPatch>,
) -> Response {
    // If-Match: "3" is the version the client read.
    let read = headers
        .get(header::IF_MATCH)
        .and_then(|value| value.to_str().ok())
        .and_then(|tag| tag.trim_matches('"').parse().ok());
    let Some(version) = read else {
        let body = json!({"error": "send If-Match with the ETag you read"});
        return (StatusCode::PRECONDITION_REQUIRED, Json(body)).into_response();
    };
    patch.version = version;
    match patch.apply(&*db, &id).await {
        Ok(saved) => product(saved),
        Err(error) => failure(error),
    }
}

fn patch(uri: &str, if_match: Option<&str>, body: &str) -> Request<Body> {
    let mut request = Request::patch(uri).header(header::CONTENT_TYPE, "application/json");
    if let Some(tag) = if_match {
        request = request.header(header::IF_MATCH, tag);
    }
    request.body(Body::from(body.to_owned())).unwrap()
}

#[tokio::main]
async fn main() -> Result<(), shelf::Error> {
    let client = shelf_playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE products (
                 id bigint PRIMARY KEY, name text NOT NULL, price_cents bigint NOT NULL,
                 version integer NOT NULL DEFAULT 1
             );
             INSERT INTO products (id, name, price_cents) VALUES (1, 'Mug', 1299);",
        )
        .await?;
    let app = Router::new()
        .route("/products/{id}", get(show).patch(edit))
        .with_state(Arc::new(client));

    let requests = [
        Request::get("/products/1").body(Body::empty()).unwrap(),
        patch("/products/1", Some("\"1\""), r#"{"price_cents": 990}"#),
        patch("/products/1", Some("\"1\""), r#"{"name": "Blue mug"}"#),
        patch("/products/1", None, r#"{"name": "Blue mug"}"#),
        patch("/products/7", Some("\"1\""), r#"{"name": "Vase"}"#),
    ];
    let mut statuses = Vec::new();
    for request in requests {
        let line = format!("{} {}", request.method(), request.uri());
        let (status, headers, body) = shelf_playground::call(&app, request).await;
        let etag = headers
            .get(header::ETAG)
            .map(|v| v.to_str().unwrap().to_owned());
        println!(
            "{line:<18} {} {:<5} {body}",
            status.as_u16(),
            etag.unwrap_or_default()
        );
        statuses.push(status.as_u16());
    }
    assert_eq!(statuses, [200, 200, 412, 428, 404]);
    Ok(())
}
```

```text
GET /products/1    200 "1"   {"id":1,"name":"Mug","price_cents":1299,"version":1}
PATCH /products/1  200 "2"   {"id":1,"name":"Mug","price_cents":990,"version":2}
PATCH /products/1  412       {"currentVersion":2,"error":"changed since you read it"}
PATCH /products/1  428       {"error":"send If-Match with the ETag you read"}
PATCH /products/7  404       {"error":"not found"}
```

The handler parses `If-Match` itself and requires it: a `PATCH` without one is answered 428
Precondition Required, which tells the client that this API does not accept blind writes.
stockroom is more lenient: without `If-Match` it applies the patch at whatever version the row
has (`version = coalesce($2, version)`). A shelf patch always checks a version, so the capstone
gets stockroom's behaviour by reading the row first and retrying on `Stale`:

~~~rust
// orm/stockroom-shelf/src/repo/products.rs (excerpt)
pub async fn update(
    db: &impl Executor,
    sku: &Sku,
    expected_version: Option<i32>,
    mut patch: ProductPatch,
) -> Result<Product, AppError> {
    loop {
        let product = by_sku(db, sku).await?;
        patch.version = expected_version.unwrap_or(product.version);
        match patch.apply(db, &product.id).await {
            Err(shelf::Error::Stale { .. }) if expected_version.is_none() => continue,
            result => return result.map_err(app_error),
        }
    }
}
~~~

[orm/stockroom-shelf/src/repo/products.rs](../stockroom-shelf/src/repo/products.rs)

Each retry means another write succeeded in between, so the loop always makes progress; the
capstone's test sends 30 patches without `If-Match` at once, and the row ends at version 31. The
price is a read before every patch: two round trips where stockroom's `coalesce` needs one
(PLAN.md §12, findings 51 and 54). The route names a product by SKU and `apply` takes the key,
so the capstone reads first anyway.

### Side by side — *an edit from two admin tabs*

The task: two tabs hold version 1 of a product; one saves a new price, then the other saves a new
name against version 1, and must be told the product changed.

~~~rust
// SeaORM (illustration)
// Only the `Set` fields are written: a partial update by construction.
let changes = product::ActiveModel {
    name: Set("Blue mug".to_owned()),
    ..Default::default() // every other field NotSet
};
// No version attribute in SeaORM 2.0.4: the check is a filter, and the count says the rest.
let result = product::Entity::update_many()
    .set(changes)
    .col_expr(product::COLUMN.version, product::COLUMN.version.add(1))
    .filter(product::COLUMN.id.eq(1))
    .filter(product::COLUMN.version.eq(1))
    .exec(&db)
    .await?;
if result.rows_affected == 0 {
    // stale or gone: a second query to find out which, and no row returned either way
}
~~~

By hand, as stockroom's `products::update` writes it (a `coalesce` per field, then a second
query when nothing was updated; stockroom also `coalesce`s the version, which this program
requires), and with shelf, in one program that runs the same
two tabs through both:

```rust
use shelf::{FromRow, Patch, Table};
use tokio_postgres::Client;

#[derive(Debug, FromRow, Table)]
#[shelf(table = "products")]
struct Product {
    #[shelf(primary_key)]
    id: i64,
    name: String,
    price_cents: i64,
    #[shelf(version)]
    version: i32,
}

#[derive(Debug, Default, Patch)]
#[shelf(model = Product)]
struct ProductPatch {
    name: Option<String>,
    price_cents: Option<i64>,
    #[shelf(version)]
    version: i32,
}

/// By hand: stockroom's shape. One statement when it works, two when it does not.
async fn update_by_hand(
    db: &Client,
    id: i64,
    expected_version: i32,
    name: Option<&str>,
    price_cents: Option<i64>,
) -> Result<Product, shelf::Error> {
    let updated = db
        .query_opt(
            "UPDATE products SET
                 name = coalesce($3, name),
                 price_cents = coalesce($4, price_cents),
                 version = version + 1
             WHERE id = $1 AND version = $2
             RETURNING id, name, price_cents, version",
            &[&id, &expected_version, &name, &price_cents],
        )
        .await?;
    if let Some(row) = updated {
        return Product::from_row(&row);
    }
    // Nothing updated: gone, or changed by someone else first.
    let current = db
        .query_opt("SELECT version FROM products WHERE id = $1", &[&id])
        .await?;
    Err(match current {
        None => shelf::Error::NotFound { model: "Product" },
        Some(row) => shelf::Error::Stale {
            model: "Product",
            current_version: row.try_get::<_, i32>(0)?.into(),
        },
    })
}

#[tokio::main]
async fn main() -> Result<(), shelf::Error> {
    let client = shelf_playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE products (
                 id bigint PRIMARY KEY, name text NOT NULL, price_cents bigint NOT NULL,
                 version integer NOT NULL DEFAULT 1
             );
             INSERT INTO products (id, name, price_cents) VALUES (1, 'Mug', 1299), (2, 'Mug', 1299);",
        )
        .await?;

    // By hand, on product 1.
    update_by_hand(&client, 1, 1, None, Some(990)).await?;
    let by_hand = update_by_hand(&client, 1, 1, Some("Blue mug"), None)
        .await
        .unwrap_err();

    // shelf, on product 2.
    ProductPatch {
        price_cents: Some(990),
        version: 1,
        ..ProductPatch::default()
    }
    .apply(&client, &2)
    .await?;
    let with_shelf = ProductPatch {
        name: Some("Blue mug".into()),
        version: 1,
        ..ProductPatch::default()
    }
    .apply(&client, &2)
    .await
    .unwrap_err();

    for error in [&by_hand, &with_shelf] {
        assert!(matches!(
            error,
            shelf::Error::Stale {
                current_version: 2,
                ..
            }
        ));
    }
    let rows = Product::all(&client).await?;
    assert!(
        rows.iter()
            .all(|p| (p.name.as_str(), p.price_cents) == ("Mug", 990))
    );
    Ok(())
}
```

| | by hand | shelf | SeaORM |
|---|---|---|---|
| **Same** | One `UPDATE` that writes only what was sent and checks the version in its `WHERE`; the second tab's save changes nothing | The same | The same |
| **Different** | `coalesce` per field, so a field can never be set to NULL; telling stale from gone takes a second query, and the version to report comes from it | A flag per field, so NULL can be written; one statement for every patch of the type, which also returns the current version or says the row is gone; `Stale` and `NotFound` are errors with what a 412 or 404 needs | Partial by construction (`Set` fields only), but each combination of fields renders a different statement; no version support, so the check is a filter you add, and zero rows affected tells you nothing more |

---

## In shelf

The patch's parameters borrow from the patch, and the flags borrow from nowhere:

~~~rust
// orm/shelf/src/patch.rs (excerpt)
/// Whether a patch field is set, as the flag beside its value: `&true` or `&false`, which live
/// for ever, so the parameters can still borrow from the patch alone.
#[doc(hidden)]
pub fn flag<T>(value: &Option<T>) -> &'static (dyn ToSql + Sync) {
    if value.is_some() { &true } else { &false }
}
~~~

[orm/shelf/src/patch.rs](../shelf/src/patch.rs)

`&true` is a reference to a constant, which Rust promotes to a `'static` value, so `flag` can
return a reference without anything to borrow it from. The value beside each flag is the field
itself, `param(&self.price_cents)`, an `&Option<i64>`: tokio-postgres's `ToSql` for `Option<T>`
sends `None` as a NULL of `T`'s type, and for `Option<Option<T>>`, both `None` and `Some(None)`
are NULLs, the flag deciding which of them is written.

## When not to use `Patch`

- **A business operation.** An order's status transition (stockroom's
  `POST /orders/{id}/transition`) has rules about which changes are allowed from which state; a
  patch that can set `status` to anything bypasses them. Write a function with its own statement.
- **Counters and expressions.** `stock = stock - 1` depends on the row's current value; a patch
  writes a value from the client.
- **A field whose change must always be checked.** A patch writes whatever fields are present.
  If changing the price needs a permission that changing the name does not, check which fields
  are `Some` before applying it (Exercise 2's `changed` is the list).

## Summary

- A partial update needs three states per nullable field: absent, `null` and a value, which is
  `Option<Option<T>>`. serde needs a one-line `deserialize_with` (or serde_with's
  `double_option`) and `#[serde(default)]` to read `null` as `Some(None)`.
  `deny_unknown_fields` keeps a misspelt field from becoming an empty patch.
- Three designs: (a) a `SET` list per shape, up to 2ⁿ − 1 statements per connection; (b)
  `coalesce`, one statement that cannot write NULL; (c) a flag per field, one statement with NULLs.
  Measured through lagproxy, (c) costs no more per patch than (b), and (a) a prepare per new
  shape. shelf uses (c).
- `#[derive(Patch)]` on a struct of `Option` fields with `#[shelf(model = …)]` generates the
  columns, a flag and a value per field, and the statement, built once in a `LazyLock` by
  `patch_sql`, because its placeholder numbers depend on the model's key.
- `#[shelf(version)]` on the model adds `version = version + 1` and `AND version = $n` to every
  update and patch; the patch carries the version it was made against, which a `const` assertion
  enforces at compile time.
- One statement tells success, `Stale { current_version }` and `NotFound` apart: a data-modifying
  `WITH`, a `LEFT JOIN` to one row, and a subquery that sees the version from before the update.
- `#[shelf(updated_at)]` sets `now()`, the transaction's start time: writes in one transaction
  share it.
- In HTTP, the version is the `ETag` and `If-Match` is the patch's version; `Stale` is a 412 with
  the current version. Patching "at any version" takes a read and a retry on `Stale`.

## Exercises

1. **An empty patch.** Apply a patch with every field `None` to a versioned model. What happens to
   the row? Write `apply_if_any`, which sends nothing for an empty patch.
   *Hint:* look at the `SET` list `patch_sql` builds when no flag is true. Decide what to return
   when nothing is sent.
2. **What changed.** Give `ProductPatch` a method `changed(&self) -> Vec<&'static str>` that lists
   the columns a patch sets, for an audit log or a permission check.
   *Hint:* `Patch::COLUMNS` is in field order; pair it with each field's `is_some()`.
3. **A `bigint` version.** Declare the version column `bigint` and the model's field `i64`. Does
   everything still work, `Stale` included?
   *Hint:* look at the type of `shelf_version` in the statement, and of `current_version` in
   `Error::Stale`.

### Solutions

**1.** The empty patch still writes: every `CASE` keeps its column, but `version = version + 1`
(and `updated_at = now()`, if the model has one) is in the `SET` list regardless, so the row moves
to the next version, and every other client's copy becomes stale. Checking before the round trip
avoids that:

```rust
use shelf::{Executor, FromRow, Patch, Table};

#[derive(Debug, FromRow, Table)]
#[shelf(table = "products")]
struct Product {
    #[shelf(primary_key)]
    id: i64,
    name: String,
    price_cents: i64,
    #[shelf(version)]
    version: i32,
}

#[derive(Debug, Default, Patch)]
#[shelf(model = Product)]
struct ProductPatch {
    name: Option<String>,
    price_cents: Option<i64>,
    #[shelf(version)]
    version: i32,
}

impl ProductPatch {
    fn is_empty(&self) -> bool {
        self.name.is_none() && self.price_cents.is_none()
    }

    /// Applies the patch if it sets anything; `None`, and nothing sent, if it does not.
    async fn apply_if_any(
        &self,
        db: &impl Executor,
        id: i64,
    ) -> Result<Option<Product>, shelf::Error> {
        if self.is_empty() {
            return Ok(None);
        }
        self.apply(db, &id).await.map(Some)
    }
}

#[tokio::main]
async fn main() -> Result<(), shelf::Error> {
    let client = shelf_playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE products (
                 id bigint PRIMARY KEY, name text NOT NULL, price_cents bigint NOT NULL,
                 version integer NOT NULL DEFAULT 1
             );
             INSERT INTO products (id, name, price_cents) VALUES (1, 'Mug', 1299);",
        )
        .await?;
    let empty = ProductPatch {
        version: 1,
        ..ProductPatch::default()
    };

    // Through `apply`: nothing set, and still a new version.
    let after = empty.apply(&client, &1).await?;
    assert_eq!((after.name.as_str(), after.version), ("Mug", 2));

    // Through `apply_if_any`: nothing sent, the version unchanged.
    let empty = ProductPatch {
        version: 2,
        ..ProductPatch::default()
    };
    assert!(empty.apply_if_any(&client, 1).await?.is_none());
    assert_eq!(Product::find(&client, &1).await?.version, 2);
    Ok(())
}
```

`apply_if_any` returns `None` rather than the row because it never read the row, and it does not
check the version either: an empty patch against a stale version is not an error here. A handler
that must answer an empty `PATCH` with the resource would `find` it, and compare the version
itself. The derive could generate `is_empty` from the same fields as `params`; it is written by
hand here.

**2.** `COLUMNS` and the fields are in the same order, so a zip pairs them:

```rust
use shelf::{FromRow, Patch, Table};

#[derive(Debug, FromRow, Table)]
#[shelf(table = "products")]
#[allow(dead_code)]
struct Product {
    #[shelf(primary_key)]
    id: i64,
    name: String,
    price_cents: i64,
    note: Option<String>,
}

#[derive(Debug, Default, Patch)]
#[shelf(model = Product)]
struct ProductPatch {
    name: Option<String>,
    price_cents: Option<i64>,
    note: Option<Option<String>>,
}

impl ProductPatch {
    /// The columns this patch sets, quoted, in field order.
    fn changed(&self) -> Vec<&'static str> {
        let set = [
            self.name.is_some(),
            self.price_cents.is_some(),
            self.note.is_some(),
        ];
        Self::COLUMNS
            .iter()
            .zip(set)
            .filter_map(|(&column, set)| set.then_some(column))
            .collect()
    }
}

fn main() {
    let patch = ProductPatch {
        price_cents: Some(990),
        note: Some(None),
        ..ProductPatch::default()
    };
    println!("{:?}", patch.changed());
    assert_eq!(patch.changed(), [r#""price_cents""#, r#""note""#]);
}
```

```text
["\"price_cents\"", "\"note\""]
```

The names are quoted, as `COLUMNS` holds them; an audit log would strip the quotes, or the derive
could generate a second list of plain names. `Some(None)` counts as a change: the patch sets
`note` to NULL.

**3.** It works. The statement casts the version it reports, `"version"::bigint AS shelf_version`,
and `Error::Stale` carries an `i64`, so an `int4` and an `int8` version are read the same way;
`version + 1` is computed in the column's own type, and the patch's `version` field is typed by
the server as the column, here `int8`:

```rust
use shelf::{FromRow, Patch, Table};

#[derive(Debug, FromRow, Table)]
#[shelf(table = "products")]
struct Product {
    #[shelf(primary_key)]
    id: i64,
    price_cents: i64,
    #[shelf(version)]
    version: i64,
}

#[derive(Debug, Patch)]
#[shelf(model = Product)]
struct ProductPatch {
    price_cents: Option<i64>,
    #[shelf(version)]
    version: i64,
}

#[tokio::main]
async fn main() -> Result<(), shelf::Error> {
    let client = shelf_playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE products (
                 id bigint PRIMARY KEY, price_cents bigint NOT NULL,
                 version bigint NOT NULL DEFAULT 5000000000
             );
             INSERT INTO products (id, price_cents) VALUES (1, 1299);",
        )
        .await?;
    let patch = |version| ProductPatch {
        price_cents: Some(990),
        version,
    };
    let saved = patch(5_000_000_000).apply(&client, &1).await?;
    assert_eq!(saved.version, 5_000_000_001);
    let error = patch(5_000_000_000).apply(&client, &1).await.unwrap_err();
    assert!(matches!(
        error,
        shelf::Error::Stale {
            current_version: 5_000_000_001,
            ..
        }
    ));
    Ok(())
}
```

A version past `i32::MAX` is reported intact. The capstone's `app_error` is the place where an
`i64` would not fit: stockroom's `StaleVersion` holds an `i32`, so it converts with `try_from` and
reports an internal error for a version it cannot represent (Chapter 7, §7.5).

---

Previous: [Chapter 8 — Writing rows: insert, update, delete, upsert](08-writing.md) · Next: [Chapter 10 — A typed query builder](10-query-builder.md)
