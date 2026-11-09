# Chapter 2 — Declarative macros

> **Building an ORM on tokio-postgres** · Previous: [Chapter 1 — Mapping rows by hand](01-mapping-by-hand.md) · Next: [Chapter 3 — The first derive macro](03-first-derive.md)

Chapter 1's `FromRow` impl is mechanical: the same line for every field, with the field's name
in two places, once as a field and once as a string. Mechanical code is what macros are for,
and Rust has two kinds. This chapter uses the simpler one, `macro_rules!`, which matches the
tokens it is given against patterns and pastes the pieces it matched into a template.

It goes surprisingly far. A `model!` macro can declare a struct and write its `FromRow` impl in
one go, and one invocation can write `FromRow` for tuples of every size from one to twelve,
which shelf keeps to this day. Then it stops: as soon as a field needs an attribute (read it
from another column, skip it, decode it as JSON), the patterns turn into a puzzle, and the
errors point at the macro's rules instead of at your mistake. That is the case for the next
chapter's procedural macros.

**In this chapter you will learn:**

- how `macro_rules!` matches tokens: rules, fragment specifiers and repetitions
- how to write a `model!` macro that declares a struct and its `FromRow` impl together
- how one macro invocation writes `FromRow` for tuples of one to twelve elements, and why the
  positions are matched as `tt`
- what hygiene protects in a macro, what it does not, and what `$crate` is for
- where declarative macros stop, and why shelf's derives are procedural

---

## 2.1 Matching tokens

A `macro_rules!` macro is a list of rules. Each rule is a *matcher*, a pattern over tokens, and
a *transcriber*, the code to produce when the pattern matches. The compiler tries the rules in
order and expands the first one that matches. Here is a macro with one rule, which turns a list
of column names into the text of a `SELECT` list, at compile time:

```rust
/// Joins column names into a `SELECT` list: `columns!(id, sku)` is `"id, sku"`.
macro_rules! columns {
    ($first:ident $(, $rest:ident)* $(,)?) => {
        concat!(stringify!($first) $(, ", ", stringify!($rest))*)
    };
}

const PRODUCT_COLUMNS: &str = columns!(id, sku, price_cents);

fn main() {
    assert_eq!(PRODUCT_COLUMNS, "id, sku, price_cents");
    assert_eq!(columns!(id), "id");
    assert_eq!(columns!(id, sku,), "id, sku"); // a trailing comma is allowed
}
```

The matcher reads like this:

- `$first:ident` matches one identifier and calls it `$first`. The part after the colon is a
  *fragment specifier*: it says what kind of syntax the metavariable accepts.
- `$(, $rest:ident)*` is a *repetition*: a comma followed by an identifier, zero or more times
  (`*`). Each identifier it matches is one `$rest`.
- `$(,)?` matches an optional trailing comma (`?` is zero or one).

The transcriber uses the same notation. `$(, ", ", stringify!($rest))*` repeats its contents
once for every `$rest` that was matched. `stringify!` turns tokens into a string literal, and
`concat!` joins literals into one: both run during expansion, so `PRODUCT_COLUMNS` is a plain
`&'static str` literal and the program does no work to build it. The first element is matched
on its own because the separator goes *between* elements: there is no way to say "a comma before
every element but the first" in one repetition.

The fragment specifiers this book uses:

| Specifier | Matches | Example |
|---|---|---|
| `ident` | an identifier | `price_cents`, `Product` |
| `ty` | a type | `Option<Vec<String>>` |
| `expr` | an expression | `row.len() + 1` |
| `literal` | a literal | `"sku"`, `0` |
| `vis` | a visibility, possibly empty | `pub`, `pub(crate)`, or nothing |
| `meta` | the inside of an attribute | `derive(Debug, PartialEq)` |
| `tt` | one token tree: a single token, or a group in `()`, `[]` or `{}` | `0`, `(a, b)` |

The Rust Reference's
[Macros By Example](https://doc.rust-lang.org/reference/macros-by-example.html) lists the
others (`path`, `pat`, `pat_param`, `block`, `item`, `lifetime`, `stmt`, `expr_2021`) and the
rules about which tokens may
follow each one.

> **Rust refresher:** macros expand before the compiler checks types, or even knows what most
> names refer to. A macro sees `Option<Vec<String>>` as a handful of tokens; the `ty` specifier
> checks that they *form* a type, but the macro cannot ask whether that type is an `Option`, or
> whether it implements `FromSql`. This is true of every macro in Rust, procedural ones
> included. shelf's derive cannot know that your `type Tags = Option<Vec<String>>` is an
> `Option` either. It does not need to: it generates code that only compiles if the types fit,
> and lets the compiler check.

## 2.2 A `model!` macro

A struct definition is tokens too. A macro that accepts one can paste it back unchanged and add
the `FromRow` impl after it, writing one `try_get` per field with `stringify!` for the column
name:

```rust
use tokio_postgres::Row;

trait FromRow: Sized {
    fn from_row(row: &Row) -> Result<Self, tokio_postgres::Error>;
}

/// Declares a struct, and implements `FromRow` for it: each field is read from the column of
/// its name.
macro_rules! model {
    (
        $(#[$meta:meta])*
        $vis:vis struct $name:ident {
            $($field_vis:vis $field:ident : $ty:ty),* $(,)?
        }
    ) => {
        $(#[$meta])*
        $vis struct $name {
            $($field_vis $field: $ty),*
        }

        impl FromRow for $name {
            fn from_row(row: &Row) -> Result<Self, tokio_postgres::Error> {
                Ok(Self {
                    $($field: row.try_get(stringify!($field))?),*
                })
            }
        }
    };
}

model! {
    #[derive(Debug, PartialEq)]
    pub struct Product {
        pub id: i64,
        pub sku: String,
        pub price_cents: i64,
        pub tags: Option<Vec<String>>,
    }
}

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = shelf_playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE products (
                 id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
                 sku text NOT NULL,
                 price_cents bigint NOT NULL,
                 tags text[]
             );
             INSERT INTO products (sku, price_cents, tags)
             VALUES ('MUG-01', 1299, '{kitchen}'), ('TEE-02', 2499, NULL);",
        )
        .await?;

    let products = client
        .query(
            "SELECT id, sku, price_cents, tags FROM products ORDER BY id",
            &[],
        )
        .await?
        .iter()
        .map(Product::from_row)
        .collect::<Result<Vec<_>, _>>()?;

    assert_eq!(products[0].sku, "MUG-01");
    assert_eq!(products[0].tags, Some(vec!["kitchen".to_string()]));
    assert_eq!(products[1].price_cents, 2499);
    assert_eq!(products[1].tags, None);
    Ok(())
}
```

Three pieces of the matcher do the work:

- `$(#[$meta:meta])*` accepts any number of attributes on the struct and pastes them back, so
  `#[derive(Debug, PartialEq)]` still applies. Without it, `model!` would refuse every
  attribute.
- `$vis:vis` and `$field_vis:vis` accept `pub`, `pub(crate)` or nothing at all, so the macro
  does not decide the struct's visibility for you.
- The field repetition, `$($field_vis:vis $field:ident : $ty:ty),*`, gives the transcriber two
  lists that line up, `$field` and `$ty`, and it uses them twice: once to declare the fields and
  once, `$field` alone, to read them. The field names are written once, by you, and the column
  names come from them.

The struct's fields are now named once instead of twice. The cost is that the struct lives
inside a macro call, which is no longer ordinary Rust to your tools. rustfmt leaves the inside of
`model! { … }` alone, so its formatting is yours to keep, and a mistake inside it is reported
against the macro's rules, as §2.5 shows. That cost is the reason shelf's mapping is a derive: a
derive leaves your struct an ordinary struct, and adds the impl next to it.

## 2.3 Tuples, one line per arity

A query that returns two columns does not always deserve a struct. Chapter 1 implemented
`FromRow` for `(A, B)` by hand, reading by position; shelf has the same for tuples of one to
twelve elements, and a single macro writes all twelve impls. Each line of the invocation is one
tuple size, with each element's type parameter next to its position:

```rust
use tokio_postgres::Row;
use tokio_postgres::types::FromSqlOwned;

trait FromRow: Sized {
    fn from_row(row: &Row) -> Result<Self, tokio_postgres::Error>;
}

/// `FromRow` for tuples, read by position: one impl per line of the invocation.
macro_rules! tuple_impls {
    ($( ($($T:ident $i:tt),+) )+) => {
        $(
            impl<$($T: FromSqlOwned),+> FromRow for ($($T,)+) {
                fn from_row(row: &Row) -> Result<Self, tokio_postgres::Error> {
                    Ok(($(row.try_get::<_, $T>($i)?,)+))
                }
            }
        )+
    };
}

tuple_impls! {
    (A 0)
    (A 0, B 1)
    (A 0, B 1, C 2)
    (A 0, B 1, C 2, D 3)
}

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = shelf_playground::connect().await?;
    let row = client
        .query_one(
            "SELECT 'MUG-01' AS sku, 1299::int8 AS price_cents, true AS active, 3 AS stock",
            &[],
        )
        .await?;

    let (sku,) = <(String,)>::from_row(&row)?;
    let pair = <(String, i64)>::from_row(&row)?;
    let all = <(String, i64, bool, i32)>::from_row(&row)?;
    assert_eq!(sku, "MUG-01");
    assert_eq!(pair, ("MUG-01".to_string(), 1299));
    assert_eq!(all, ("MUG-01".to_string(), 1299, true, 3));
    Ok(())
}
```

The matcher has a repetition inside a repetition. The outer one, `$( (…) )+`, matches each
parenthesized line; the inner one, `$($T:ident $i:tt),+`, matches the elements of a line. In the
transcriber, `$( impl … )+` repeats per line, and inside it `$($T: FromSqlOwned),+` and
`($($T,)+)` repeat per element. The second one writes a comma after *every* element, including
the last, which is not just tidy: a one-element tuple is spelled `(A,)`, and `(A)` would be `A`
in parentheses.

Why is the position matched as a `tt`, a single token tree, and not as a `literal`? Here either
would do, because `$i` is pasted into expression positions: `row.try_get::<_, $T>($i)`. But the
two are pasted differently. A `tt` goes in as the token it was. A `literal` or an `expr` goes in
as one sealed piece of syntax, which some positions refuse. A tuple field is one of them:

~~~rust
macro_rules! field_tt { ($t:expr, $i:tt) => { $t.$i }; }
macro_rules! field_lit { ($t:expr, $i:literal) => { $t.$i }; }
fn main() {
    let t = (10, 20);
    println!("{}", field_tt!(t, 1));
    println!("{}", field_lit!(t, 1));
}
~~~

`field_tt!(t, 1)` becomes `t.1`. `field_lit!(t, 1)` does not compile:

```text
error: unexpected token: `literal` metavariable
 --> literal.rs:2:56
  |
2 | macro_rules! field_lit { ($t:expr, $i:literal) => { $t.$i }; }
  |                                                        ^^
...
6 |     println!("{}", field_lit!(t, 1));
  |                    ---------------- in this macro invocation
```

So `tt` is the habit for a position: it works everywhere a number does. Exercise 3 needs that.

Writing the positions out by hand looks like something the macro should count for itself.
Nightly Rust has *metavariable expressions*, whose `${index()}` gives the position inside a
repetition, but in rustc 1.99.1 they are still unstable (error E0658, tracking issue
[#83527](https://github.com/rust-lang/rust/issues/83527)). A recursive macro can avoid
repeating the lines instead (Exercise 1), but the flat list is the easiest to read and to
check, so shelf keeps it.

## 2.4 Hygiene, and `$crate`

A macro's output lands in the caller's code, among the caller's names. Two questions follow:
can the macro's own variables collide with the caller's, and do the names the macro writes mean
what its author intended? `macro_rules!` answers them differently, and this program shows both
answers:

```rust
mod text {
    /// Pads a cell of a plain-text table to eight characters.
    pub fn cell(value: &str) -> String {
        format!("{value:<8}|")
    }

    /// One line of a plain-text table, from any number of cells.
    #[macro_export]
    macro_rules! table_line {
        ($($value:expr),+ $(,)?) => {{
            let mut line = ::std::string::String::new();
            $( line.push_str(&$crate::text::cell($value)); )+
            line
        }};
    }
}

// A name the macro must not depend on: this crate's `String` is not std's.
#[allow(dead_code)]
struct String;

fn main() {
    let line = "MUG-01";
    let header = table_line!("sku", "price");
    let row = table_line!(line, "1299"); // the caller's `line`, not the macro's
    assert_eq!(header, "sku     |price   |");
    assert_eq!(row, "MUG-01  |1299    |");
    assert_eq!(line, "MUG-01");
}
```

**Local variables are hygienic.** The macro declares `let mut line`, and the caller passes in
its own `line`. Inside the expansion both are in scope, with the same name, yet `$value` still
means the caller's `&str` and the macro's `push_str` still targets its own `String`. The
compiler remembers where each identifier was written: a variable declared by the macro is
visible only to the macro's tokens, and the caller's tokens see only the caller's variables.

**Paths are not.** Every other name in the expansion, types, functions, traits, constants, is
looked up where the macro is *called*. That is why `table_line!` writes
`::std::string::String::new()`: in `main`'s module, `String` is the caller's unit struct, and
`String::new()` would not compile. A leading `::` means that the first segment names a crate
(`std`, `core`, `shelf`), never an item of the caller's module, so nothing the caller declares can
shadow it. The Reference calls this mix *mixed-site hygiene*.

`model!` from §2.2 is not that careful. It writes `Result<Self, tokio_postgres::Error>`, and an
application with the common alias `type Result<T> = std::result::Result<T, AppError>` in scope
breaks it:

~~~rust
mod app {
    use super::{FromRow, Row};

    type Result<T> = std::result::Result<T, String>;

    model! {
        pub struct Product {
            pub sku: String,
        }
    }
}
~~~

```text
error[E0107]: type alias takes 1 generic argument but 2 generic arguments were supplied
  --> src/bin/model_alias.rs:20:39
   |
20 |               fn from_row(row: &Row) -> Result<Self, tokio_postgres::Error> {
   |                                         ^^^^^^     ----------------------- help: remove the unnecessary generic argument
   |                                         |
   |                                         expected 1 generic argument
...
34 | /     model! {
35 | |         pub struct Product {
36 | |             pub sku: String,
37 | |         }
38 | |     }
   | |_____- in this macro invocation
   |
note: type alias defined here, with 1 generic parameter: `T`
  --> src/bin/model_alias.rs:32:10
   |
32 |     type Result<T> = std::result::Result<T, String>;
   |          ^^^^^^ -
   = note: this error originates in the macro `model` (in Nightly builds, run with -Z macro-backtrace for more info)
```

The fix is the same as in `table_line!`: absolute paths for everything the macro names,
`::core::result::Result` and `::core::result::Result::Ok`, never `Result` and `Ok`. Notice also
that `app` had to import `FromRow` and `Row` for `model!` to work at all, because the expansion
names them as bare identifiers. A careful `model!` would not ask that of its caller.

**`$crate` names the macro's own crate.** `cell` is a function in this program, so `table_line!`
could have written `crate::text::cell`. But a macro exported from a library (`#[macro_export]`)
runs in other crates, where `crate` means the *caller's* crate. `$crate` expands to the crate
that defined the macro, wherever it is called, and it keeps working when the caller renames the
dependency in `Cargo.toml`. It is how a library's macro reaches the library's helpers.

> **Macro lens:** `$crate` exists only in `macro_rules!`. A procedural macro is a crate of its
> own, and the tokens it returns have no way to say "the crate that re-exports me", so shelf's
> generated code writes `::shelf::…` and assumes the dependency is called `shelf`. serde does the
> same, and offers `#[serde(crate = "…")]` for the rare caller who renamed it. Chapter 3 returns
> to the rest of a derive's hygiene: absolute paths everywhere, and the hidden module
> `::shelf::__private` that spares your crate a direct dependency on tokio-postgres.

## 2.5 Where declarative macros stop

`model!` maps each field to the column of its name. Real models need more, and the first thing
they need is an attribute: this field reads the column `sku_code`. The natural syntax is an
attribute on the field. `model!` does not accept it:

~~~rust
model! {
    pub struct Product {
        pub id: i64,
        #[col = "sku_code"]
        pub sku: String,
    }
}
~~~

```text
error: no rules expected `#`
  --> src/bin/model_attr.rs:32:9
   |
 7 | macro_rules! model {
   | ------------------ when calling this macro
...
32 |         #[col = "sku_code"]
   |         ^ no rules expected this token in macro call
   |
note: while trying to match meta-variable `$field_vis:vis`
  --> src/bin/model_attr.rs:11:15
   |
11 |             $($field_vis:vis $field:ident : $ty:ty),* $(,)?
   |               ^^^^^^^^^^^^^^
```

Accepting the attribute is easy: add `$(#[$fattr:meta])*` before each field, as for the struct.
*Acting* on it is not. A matcher cannot say "a `col = "…"` attribute if there is one, otherwise
the field's name", because the transcriber has no `if`. The way out is a rule per kind of field,
and a macro that walks the fields one at a time, calling itself on the rest with what it has
built so far: a *tt-muncher*. Its shape, not compiled:

~~~rust
// A tt-muncher for model!'s fields (illustration): one field consumed per step.
(@fields [$($done:tt)*] #[col = $col:literal] $field:ident : $ty:ty, $($rest:tt)*) => {
    model!(@fields [$($done)* $field: row.try_get($col)?,] $($rest)*)
};
(@fields [$($done:tt)*] $field:ident : $ty:ty, $($rest:tt)*) => {
    model!(@fields [$($done)* $field: row.try_get(stringify!($field))?,] $($rest)*)
};
(@fields [$($done:tt)*]) => { /* write the impl from everything in $done */ };
~~~

That handles one attribute. Each new one (`skip`, `json`, `flatten`, `try_from`) adds rules,
combinations of them multiply the rules, and an attribute that is misspelt does not match any
rule, so the error is "no rules expected" at the `#`, with no word about what was wrong.

Generics go the same way:

~~~rust
model! {
    pub struct Page<T> {
        pub items: Vec<T>,
    }
}
~~~

```text
error: no rules expected `<`
  --> src/bin/model_attr.rs:38:20
   |
 7 | macro_rules! model {
   | ------------------ when calling this macro
...
38 |     pub struct Page<T> {
   |                    ^ no rules expected this token in macro call
   |
note: while trying to match `{`
  --> src/bin/model_attr.rs:10:37
   |
10 |         $vis:vis struct $name:ident {
   |                                     ^
```

Matching `<T>` is possible. Matching `<'a, T: Clone, const N: usize>` and a `where` clause, and
then adding the bound each field needs, is a parser for Rust's generics written in
`macro_rules!`.

Each of these problems has the same root: `macro_rules!` matches tokens against fixed patterns,
and a struct is not a fixed pattern. What the job needs is a macro that receives the struct as
*data*, already parsed, with a list of fields it can loop over, attributes it can inspect with
ordinary `if`s, generics it can modify, and a way to report a mistake at the exact token that
caused it, in its own words. That is a procedural macro, and syn is the parser. Chapter 3
writes one.

### Side by side — *generating impls for many types*

The task: `FromRow` for tuples, read by position. sqlx has the same impls, written by a macro
with one invocation per tuple size:

~~~rust
// sqlx (illustration), from sqlx-core 0.9.0's src/from_row.rs, abridged
macro_rules! impl_from_row_for_tuple {
    ($( ($idx:tt) -> $T:ident );+;) => {
        impl<'r, R, $($T,)+> FromRow<'r, R> for ($($T,)+)
        where
            R: Row,
            usize: crate::column::ColumnIndex<R>,
            $($T: crate::decode::Decode<'r, R::Database> + crate::types::Type<R::Database>,)+
        {
            #[inline]
            fn from_row(row: &'r R) -> Result<Self, Error> {
                Ok(($(row.try_get($idx as usize)?,)+))
            }
        }
    };
}

impl_from_row_for_tuple!(
    (0) -> T1;
    (1) -> T2;
);
// … one invocation per size, up to sixteen elements

let pairs: Vec<(String, i64)> = sqlx::query_as("SELECT sku, price_cents FROM products")
    .fetch_all(&pool)
    .await?;
~~~

By hand, two sizes written out, and shelf's own tuples, in one program:

```rust
use tokio_postgres::Row;
use tokio_postgres::types::FromSqlOwned;

trait FromRow: Sized {
    fn from_row(row: &Row) -> Result<Self, tokio_postgres::Error>;
}

// By hand: one impl per size, each written out.
impl<A: FromSqlOwned> FromRow for (A,) {
    fn from_row(row: &Row) -> Result<Self, tokio_postgres::Error> {
        Ok((row.try_get(0)?,))
    }
}

impl<A: FromSqlOwned, B: FromSqlOwned> FromRow for (A, B) {
    fn from_row(row: &Row) -> Result<Self, tokio_postgres::Error> {
        Ok((row.try_get(0)?, row.try_get(1)?))
    }
}

const SQL: &str = "SELECT sku, price_cents FROM products ORDER BY sku";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = shelf_playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE products (sku text PRIMARY KEY, price_cents bigint NOT NULL);
             INSERT INTO products VALUES ('MUG-01', 1299), ('TEE-02', 2499);",
        )
        .await?;

    let by_hand = client
        .query(SQL, &[])
        .await?
        .iter()
        .map(<(String, i64)>::from_row)
        .collect::<Result<Vec<_>, _>>()?;

    // shelf: the impls come from one macro invocation in shelf/src/tuples.rs.
    let with_shelf: Vec<(String, i64)> = shelf::fetch_all(&client, SQL, &[]).await?;
    assert_eq!(by_hand, with_shelf);

    // A wrong element type: the error names the position and the column.
    let error = shelf::fetch_all::<(String, i32)>(&client, SQL, &[])
        .await
        .unwrap_err();
    println!("{error}");
    assert_eq!(
        error.to_string(),
        r#"tuple.1: column "price_cents" has type int8, which cannot be read into i32"#
    );
    Ok(())
}
```

```text
tuple.1: column "price_cents" has type int8, which cannot be read into i32
```

| | by hand | shelf | sqlx |
|---|---|---|---|
| **Same** | One impl per tuple size, reading by position | The same impls, written by a macro | The same impls, written by a macro |
| **Different** | Every impl typed out; tokio-postgres's error names only the column | Twelve sizes from one invocation, a line each; errors name the position (`tuple.1`) and the column, and are found once per statement | Sixteen sizes, one invocation each; generic over sqlx's databases |

---

## In shelf

shelf's tuple impls are §2.3's macro, with two differences. `FromRow` has the two methods
Chapter 5 introduces, `resolve` (check the statement's columns once) and `from_row_at` (read a
row), and the positions are pasted a second time through `stringify!`, so that an error can
name the element (`"1"` in `tuple.1` above):

~~~rust
// orm/shelf/src/tuples.rs (excerpt)
macro_rules! tuple_impls {
    ($( ($($T:ident $i:tt),+) )+) => {
        $(
            impl<$($T: FromColumn),+> FromRow for ($($T,)+) {
                type Columns = ();

                fn resolve(columns: &[Column]) -> Result<(), Error> {
                    $( column_at::<$T>(columns, "tuple", stringify!($i), $i)?; )+
                    Ok(())
                }

                fn from_row_at(row: &Row, _: &()) -> Result<Self, Error> {
                    Ok(($( get::<$T>(row, $i, "tuple", stringify!($i))?, )+))
                }
            }
        )+
    };
}

tuple_impls! {
    (A 0)
    (A 0, B 1)
    (A 0, B 1, C 2)
    // … one line per size, up to twelve elements
}
~~~

[orm/shelf/src/tuples.rs](../shelf/src/tuples.rs)

The bound is `FromColumn`, shelf's own name for tokio-postgres's `FromSqlOwned`, which exists to
make one compiler error read better (Chapter 4). A tuple's `Columns` is `()`: its positions are
fixed, so there is nothing to look up, and `resolve` only checks each column's type. This macro
lives inside shelf and is not exported, so it can name `FromRow`, `Column` and `get` directly:
its expansion is always in `tuples.rs`, where those names are imported. A macro for other
crates would need the absolute paths of §2.4.

## When not to use `macro_rules!`

- **When a generic impl does the job.** shelf's `FromColumn` has a single impl,
  `impl<T: FromSqlOwned> FromColumn for T`, which covers every type at once with no macro. A
  macro is for impls that differ in *shape*, like tuples of different sizes.
- **When a function does the job.** A function is type-checked where it is written, and its
  errors point at it. Reach for a macro when you need code that a function cannot express: new
  items, new fields, a variable number of arguments.
- **When the input needs attributes, generics or good error messages.** That is a procedural
  macro's job (Chapter 3).

## Summary

- `macro_rules!` matches tokens against rules, in order, and pastes what it matched into a
  template. Fragment specifiers (`ident`, `ty`, `expr`, `tt`, `vis`, `meta`, `literal`) say
  what each metavariable accepts; `*`, `+` and `?` repeat.
- Macros run before type checking: they see tokens, never types, and that holds for procedural
  macros too.
- A `model!` macro can declare a struct and write its `FromRow` impl, so field names are written
  once. The struct then lives inside a macro call, which tools handle less well.
- One invocation writes `FromRow` for tuples of one to twelve elements, a line per size. Match
  positions as `tt`: a captured `literal` cannot be pasted as a tuple index. `${index()}` is
  still unstable in rustc 1.99.1.
- Local variables in a macro are hygienic; paths are resolved at the call site, so a macro for
  other people's code writes `::core::result::Result`. `$crate` names the macro's own crate.
- Field attributes, generics and readable errors are beyond what `macro_rules!` does well. A
  procedural macro gets the struct as parsed data.

## Exercises

1. **One line instead of twelve.** Rewrite §2.3's `tuple_impls!` so that a single invocation,
   `tuple_impls!([] A 0, B 1, C 2, D 3)`, writes all four impls.
   *Hint:* keep the elements already handled in the brackets, an *accumulator*. Each step moves
   one element from the list into the accumulator, writes the impl for the accumulator, and calls
   the macro again with the rest. Peeling elements off the front without an accumulator gets the
   positions wrong: the rest would start at 1.
2. **The column list too.** Make `model!` also generate a constant `Product::COLUMNS` holding
   `"id, sku, price_cents, tags"`, and use it to write the query.
   *Hint:* a macro can call another macro: §2.1's `columns!` is half the work.
3. **Tuples as parameters.** Write a trait `Params` with
   `fn params(&self) -> Vec<&(dyn ToSql + Sync)>`, implement it for tuples of one to three
   elements with a macro, and insert a row with `("VASE-03", 3999_i64).params()`.
   *Hint:* a tuple's fields are `self.0`, `self.1`, and so on, which is where §2.3's `tt` earns
   its keep.

### Solutions

**1.** Each step writes one impl and recurses, so the invocation is one line. The cost is a
macro that is harder to read than the flat list, and a recursion depth, one level per element,
that the compiler limits (128 levels by default):

```rust
use tokio_postgres::Row;
use tokio_postgres::types::FromSqlOwned;

trait FromRow: Sized {
    fn from_row(row: &Row) -> Result<Self, tokio_postgres::Error>;
}

macro_rules! tuple_impls {
    // Nothing left to move: done.
    ([$($T:ident $i:tt),*]) => {};
    // Move the next element into the accumulator, write its impl, and go on with the rest.
    ([$($T:ident $i:tt),*] $U:ident $j:tt $(, $rest_t:ident $rest_i:tt)*) => {
        impl<$($T: FromSqlOwned,)* $U: FromSqlOwned> FromRow for ($($T,)* $U,) {
            fn from_row(row: &Row) -> Result<Self, tokio_postgres::Error> {
                Ok(($(row.try_get::<_, $T>($i)?,)* row.try_get::<_, $U>($j)?,))
            }
        }
        tuple_impls!([$($T $i,)* $U $j] $($rest_t $rest_i),*);
    };
}

tuple_impls!([] A 0, B 1, C 2, D 3);

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = shelf_playground::connect().await?;
    let row = client
        .query_one("SELECT 'MUG-01' AS sku, 1299::int8, true, 3", &[])
        .await?;
    assert_eq!(<(String,)>::from_row(&row)?, ("MUG-01".to_string(),));
    assert_eq!(
        <(String, i64, bool, i32)>::from_row(&row)?,
        ("MUG-01".to_string(), 1299, true, 3)
    );
    Ok(())
}
```

**2.** `model!` hands its field names to `columns!`, which joins them at compile time:

```rust
use tokio_postgres::Row;

trait FromRow: Sized {
    fn from_row(row: &Row) -> Result<Self, tokio_postgres::Error>;
}

macro_rules! columns {
    ($first:ident $(, $rest:ident)* $(,)?) => {
        concat!(stringify!($first) $(, ", ", stringify!($rest))*)
    };
}

macro_rules! model {
    (
        $(#[$meta:meta])*
        $vis:vis struct $name:ident {
            $($field_vis:vis $field:ident : $ty:ty),* $(,)?
        }
    ) => {
        $(#[$meta])*
        $vis struct $name {
            $($field_vis $field: $ty),*
        }

        impl $name {
            /// The columns `from_row` reads, as a `SELECT` list.
            const COLUMNS: &'static str = columns!($($field),*);
        }

        impl FromRow for $name {
            fn from_row(row: &Row) -> Result<Self, tokio_postgres::Error> {
                Ok(Self {
                    $($field: row.try_get(stringify!($field))?),*
                })
            }
        }
    };
}

model! {
    #[derive(Debug)]
    struct Product {
        id: i64,
        sku: String,
        price_cents: i64,
        tags: Option<Vec<String>>,
    }
}

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    assert_eq!(Product::COLUMNS, "id, sku, price_cents, tags");

    let client = shelf_playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE products (id bigint, sku text, price_cents bigint, tags text[]);
             INSERT INTO products VALUES (1, 'MUG-01', 1299, NULL);",
        )
        .await?;
    // COLUMNS holds identifiers the macro wrote from the struct, never a value, so it can
    // go into the SQL text; values still travel as $n parameters.
    let sql = format!("SELECT {} FROM products WHERE id = $1", Product::COLUMNS);
    let product = Product::from_row(&client.query_one(&sql, &[&1_i64]).await?)?;
    assert_eq!(product.sku, "MUG-01");
    assert_eq!(
        (product.id, product.price_cents, product.tags),
        (1, 1299, None)
    );
    Ok(())
}
```

The `format!` runs on every call. Chapter 6 builds such statements once, at compile time, from
constants the derive writes, and quotes each identifier on the way.

**3.** The impls use `self.$i`, which compiles only because `$i` is a `tt`:

```rust
use tokio_postgres::types::ToSql;

/// A tuple's values, as the parameter list `query` and `execute` take.
trait Params {
    fn params(&self) -> Vec<&(dyn ToSql + Sync)>;
}

macro_rules! params_impls {
    ($( ($($T:ident $i:tt),+) )+) => {
        $(
            impl<$($T: ToSql + Sync),+> Params for ($($T,)+) {
                fn params(&self) -> Vec<&(dyn ToSql + Sync)> {
                    vec![$(&self.$i as &(dyn ToSql + Sync)),+]
                }
            }
        )+
    };
}

params_impls! {
    (A 0)
    (A 0, B 1)
    (A 0, B 1, C 2)
}

#[tokio::main]
async fn main() -> Result<(), tokio_postgres::Error> {
    let client = shelf_playground::connect().await?;
    client
        .batch_execute("CREATE TEMP TABLE products (sku text, price_cents bigint)")
        .await?;
    let inserted = client
        .execute(
            "INSERT INTO products VALUES ($1, $2)",
            &("VASE-03", 3999_i64).params(),
        )
        .await?;
    assert_eq!(inserted, 1);
    let price: i64 = client
        .query_one_scalar(
            "SELECT price_cents FROM products WHERE sku = $1",
            &("VASE-03",).params(),
        )
        .await?;
    assert_eq!(price, 3999);
    Ok(())
}
```

With `$i:literal` instead, `self.$i` is refused with the error shown in §2.3.

---

Previous: [Chapter 1 — Mapping rows by hand](01-mapping-by-hand.md) · Next: [Chapter 3 — The first derive macro](03-first-derive.md)
