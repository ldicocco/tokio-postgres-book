# Chapter 4 — Attributes, errors and generics

> **Building an ORM on tokio-postgres** · Previous: [Chapter 3 — The first derive macro](03-first-derive.md) · Next: [Chapter 5 — Checking a statement once, not every row](05-column-indices.md)

A real schema does not match your structs one to one. A column is called `sku_code` but the
field is `sku`; a legacy table uses `camelCase`; a field holds a value computed in Rust and is
not in the table at all; a `jsonb` column holds a document you want as a struct; an `int`
should become a `Grams` that refuses negative values. Each of those is one attribute on a
field, and serde has taught every Rust developer to expect them.

This chapter teaches the derive to read `#[shelf(…)]`. Parsing is the easy part. The work is
in what happens when the attributes are wrong: a misspelt key, a value where none belongs, two
attributes that contradict each other. A good derive reports all of them in one build, each at
the token that caused it, in words the user understands. The same care goes into generic
structs, which need exactly the bounds their fields require, and no others.

**In this chapter you will learn:**

- how a derive declares a helper attribute, and what happens if it forgets
- how to parse `#[shelf(…)]` with `parse_nested_meta` into a typed struct
- what `rename_all`, `rename`, `skip`, `flatten`, `json` and `try_from` generate
- how to report every mistake in one build, each at the right token
- how to test a derive's errors with trybuild, and what reading them changed in shelf
- how to give a generic struct the bounds its fields need, and why shelf refuses lifetimes
- what darling would do for you, and why shelf does without it

---

## 4.1 Helper attributes

The compiler resolves every attribute it meets. `#[allow(…)]` and `#[doc = "…"]` are built
in; `#[tokio::main]` is an attribute macro, found by its path; `#[rustfmt::skip]` and
`#[diagnostic::…]` belong to tools. `#[shelf(…)]` is none of those. It is a *helper
attribute*: a name that a derive declares, which the compiler then accepts on any item that
uses the derive, and leaves in place for the derive to read. It does nothing by itself. shelf
declares it in the shell of Chapter 3:

~~~rust
// orm/shelf-macros/src/lib.rs (excerpt)
#[proc_macro_derive(FromRow, attributes(shelf))]
~~~

What happens without the declaration? This probe derive, written for this chapter like §3.1's
`ShowInput` and not part of shelf, generates nothing and declares no helper:

~~~rust
#[proc_macro_derive(NoHelper)]
pub fn no_helper(_input: TokenStream) -> TokenStream {
    TokenStream::new()
}
~~~

Put it on a struct with a `#[shelf(skip)]` field, in a crate that depends on shelf, and the
build fails:

```text
error: cannot find attribute `shelf` in this scope
 --> user/src/bin/no_helper.rs:3:7
  |
3 |     #[shelf(skip)]
  |       ^^^^^
  |
  = note: `shelf` is in scope, but it is a crate, not an attribute
```

The derive never ran. The compiler refused the attribute while resolving names, before any
macro saw the struct, and the note says where it looked: the only `shelf` in scope is the
crate, and a crate is not an attribute. shelf met exactly this error when its hygiene test first
used `#[shelf(skip)]` (PLAN.md §12 #9). Forgetting the `#[derive(FromRow)]` itself gives the
same error, with the same note, for the same reason: a helper attribute exists only on an item
whose derive declares it.

All of shelf's derives will declare the same helper, so `#[shelf(…)]` is one namespace, shared
by `FromRow` and, from Chapter 6 on, `Table`, `Insert` and `Patch`. That decides how shelf
parses it.

## 4.2 Parsing with `parse_nested_meta`

An attribute arrives as a `syn::Attribute`: its path, `shelf`, and the tokens between the
parentheses, not yet parsed. syn does not know what `rename = "sku_code"` means to you, but it
knows the shape that most attributes share, which its documentation calls the *conventional
syntax for structured attributes*: a comma-separated list of items, each a path, optionally
followed by `= value` or by a list in parentheses of the same shape. serde's attributes follow
it, and so do shelf's.

`Attribute::parse_nested_meta` walks that list. For each item it parses the path and calls
your closure with it, as `meta.path`. The closure reads whatever follows the path:
`meta.value()?` consumes the `=` and returns the stream to parse the value from. When the
closure returns, syn expects a comma or the end of the list. This stage program is a parser for
three of shelf's keys, run on a struct built with `parse_quote!`:

```rust
use quote::ToTokens;
use syn::{Attribute, Data, DeriveInput, Fields, LitStr, Type, parse_quote};

/// The stage version of shelf's `FieldAttrs`: three keys, as typed values.
#[derive(Default)]
struct FieldAttrs {
    rename: Option<String>,
    skip: bool,
    try_from: Option<Type>,
}

fn parse(attrs: &[Attribute]) -> syn::Result<FieldAttrs> {
    let mut out = FieldAttrs::default();
    for attr in attrs.iter().filter(|attr| attr.path().is_ident("shelf")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("rename") {
                // `rename = "…"`: `value()` reads the `=`, then comes a string literal.
                let lit: LitStr = meta.value()?.parse()?;
                out.rename = Some(lit.value());
            } else if meta.path.is_ident("skip") {
                // A flag: the key is all there is.
                out.skip = true;
            } else if meta.path.is_ident("try_from") {
                // `try_from = i64`: the value is a type.
                out.try_from = Some(meta.value()?.parse()?);
            } else {
                return Err(meta.error("unknown shelf attribute"));
            }
            Ok(())
        })?;
    }
    Ok(out)
}

fn main() {
    let input: DeriveInput = parse_quote! {
        struct Product {
            #[shelf(rename = "sku_code")]
            sku: String,
            #[shelf(try_from = i64, rename = "price_cents")]
            price: Cents,
            /// Set by the user interface; not stored.
            #[shelf(skip)]
            selected: bool,
        }
    };
    let Data::Struct(data) = &input.data else {
        panic!("a struct")
    };
    let Fields::Named(fields) = &data.fields else {
        panic!("named fields")
    };
    let mut parsed = Vec::new();
    for field in &fields.named {
        let attrs = parse(&field.attrs).expect("valid attributes");
        let try_from = attrs
            .try_from
            .as_ref()
            .map(|ty| ty.to_token_stream().to_string());
        println!(
            "{}: rename {:?}, skip {}, try_from {:?}",
            field.ident.as_ref().expect("a named field"),
            attrs.rename,
            attrs.skip,
            try_from
        );
        parsed.push(attrs);
    }
    assert_eq!(parsed[0].rename.as_deref(), Some("sku_code"));
    assert_eq!(parsed[1].rename.as_deref(), Some("price_cents"));
    assert!(parsed[1].try_from.is_some());
    assert!(parsed[2].skip);

    // Three inputs the stage parser handles badly.
    let unknown: Attribute = parse_quote!(#[shelf(renamed = "sku")]);
    let Err(error) = parse(&[unknown]) else {
        panic!("an unknown key is refused")
    };
    println!("renamed = \"sku\": {error}");

    let flag_with_value: Attribute = parse_quote!(#[shelf(skip = true)]);
    let Err(error) = parse(&[flag_with_value]) else {
        panic!("a value after a flag is refused")
    };
    println!("skip = true: {error}");
    assert_eq!(error.to_string(), "expected `,`");

    let twice: Attribute = parse_quote!(#[shelf(rename = "a", rename = "b")]);
    let attrs = parse(&[twice]).expect("accepted, wrongly");
    println!("rename twice: {:?}", attrs.rename);
    assert_eq!(attrs.rename.as_deref(), Some("b"));
}
```

```text
sku: rename Some("sku_code"), skip false, try_from None
price: rename Some("price_cents"), skip false, try_from Some("i64")
selected: rename None, skip true, try_from None
renamed = "sku": unknown shelf attribute
skip = true: expected `,`
rename twice: Some("b")
```

Each kind of key reads its value its own way. `rename` parses a string literal, a `LitStr`,
and keeps its contents without the quotes. `skip` is a *flag*: there is nothing after the key
to read. `try_from` parses a `syn::Type`, because every node of syn's syntax tree can be parsed
from a stream, a type as much as a literal. The doc comment on `selected` is an attribute too
(`#[doc = "…"]`), and the filter on `shelf` passes over it. And `price` shows that one
attribute can hold several keys; two `#[shelf(…)]` attributes would parse the same.

The last three lines are what the stage parser gets wrong. An unknown key is refused, but the
message does not say which keys exist. A flag given a value, `skip = true`, fails with syn's
"expected `,`": the closure read nothing after `skip`, so syn looked for the next item and found
`=`. That is true, and no help to whoever wrote it. And a key written twice is accepted, with
the second value silently winning. shelf's parser keeps the stage's shape and fixes all three:

~~~rust
// orm/shelf-codegen/src/attrs.rs (excerpt)
/// A parsed value, with the span of the key that set it, for error messages.
pub struct Keyed<T> {
    pub span: Span,
    pub value: T,
}

// … `FieldAttrs` has an `Option<Keyed<…>>` per key: `Keyed<LitStr>` for `rename`, and so on

impl FieldAttrs {
    pub fn parse(attrs: &[Attribute], errors: &mut Errors) -> Self {
        let mut out = FieldAttrs::default();
        for attr in shelf_attrs(attrs) {
            let parsed = attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("rename") {
                    let lit: LitStr = meta.value()?.parse()?;
                    if lit.value().is_empty() {
                        return Err(Error::new_spanned(&lit, "a column name cannot be empty"));
                    }
                    once(&mut out.rename, lit, &meta)
                } else if meta.path.is_ident("skip") {
                    flag(&mut out.skip, &meta)
                    // … `flatten` and `json` the same way
                } else if meta.path.is_ident("try_from") {
                    let ty = type_value(&meta)?;
                    once(&mut out.try_from, ty, &meta)
                } else {
                    Err(unknown(
                        &meta,
                        "a field",
                        &["rename", "skip", "flatten", "json", "try_from"],
                    ))
                }
            });
            if let Err(error) = parsed {
                errors.push(error);
            }
        }
        out.check_combinations(errors);
        out
    }
    // …
}
~~~

[orm/shelf-codegen/src/attrs.rs](../shelf-codegen/src/attrs.rs)

Every value is kept as a `Keyed<T>`: the value, and the span of the key that set it, so that a
later check can point at `json` or `rename` in the user's source (§4.4). A flag is a
`Keyed<()>`: all that matters is that it was there, and where. Errors are not returned but
pushed onto `errors`, which collects them (§4.4). The rest of the work is in four small
functions:

~~~rust
// orm/shelf-codegen/src/attrs.rs (excerpt)
/// Stores a key's value, refusing a second one.
fn once<T>(slot: &mut Option<Keyed<T>>, value: T, meta: &ParseNestedMeta) -> Result<()> {
    if slot.is_some() {
        return Err(meta.error(format!("duplicate `{}`", name_of(meta))));
    }
    *slot = Some(Keyed {
        span: meta.path.span(),
        value,
    });
    Ok(())
}

/// A key without a value, such as `skip`.
fn flag(slot: &mut Option<Keyed<()>>, meta: &ParseNestedMeta) -> Result<()> {
    if meta.input.peek(Token![=]) || meta.input.peek(syn::token::Paren) {
        return Err(meta.error(format!("`{}` takes no value", name_of(meta))));
    }
    once(slot, (), meta)
}

/// A type after `=`: `try_from = i64`, or quoted as serde writes it, `try_from = "i64"`.
fn type_value(meta: &ParseNestedMeta) -> Result<Type> {
    let value = meta.value()?;
    if value.peek(LitStr) {
        let lit: LitStr = value.parse()?;
        lit.parse()
            .map_err(|_| Error::new_spanned(&lit, "expected a type, as in `try_from = i64`"))
    } else {
        value.parse()
    }
}

fn unknown(meta: &ParseNestedMeta, on: &str, known: &[&str]) -> Error {
    let known: Vec<String> = known.iter().map(|k| format!("`{k}`")).collect();
    meta.error(format!(
        "unknown shelf attribute `{}` on {on}; expected {}",
        name_of(meta),
        known.join(", ")
    ))
}
~~~

[orm/shelf-codegen/src/attrs.rs](../shelf-codegen/src/attrs.rs)

- `once` stores a value and refuses a second one.
- `flag` looks ahead before anything else (`peek` reads a token without consuming it): if an
  `=` or a parenthesis follows the key, it says that the key takes no value, instead of
  leaving syn to complain about a comma.
- `type_value` accepts a type as it is, `try_from = i64`, or quoted, `try_from = "i64"`, as
  serde writes its own `try_from`. A quoted type is parsed from the literal's contents by
  `LitStr::parse`.
- `unknown` lists the keys that do exist.

`meta.error(…)` builds a `syn::Error` that covers everything from the key to the last token the
closure has read. So a duplicate `rename = "e2"` is underlined whole, because `once` refused it
after its value was parsed, while `skip = true` underlines only `skip`, because `flag` refused
it before reading on. One of shelf's UI tests (§4.5) holds six such mistakes, and this is what
the compiler prints for them:

<!-- mirror: orm/shelf/tests/ui/bad_values.stderr -->
```text
error: expected `=`
 --> tests/ui/bad_values.rs:6:19
  |
6 |     #[shelf(rename)]
  |                   ^

error: expected string literal
 --> tests/ui/bad_values.rs:8:22
  |
8 |     #[shelf(rename = 1)]
  |                      ^

error: a column name cannot be empty
  --> tests/ui/bad_values.rs:10:22
   |
10 |     #[shelf(rename = "")]
   |                      ^^

error: `skip` takes no value
  --> tests/ui/bad_values.rs:12:13
   |
12 |     #[shelf(skip = true)]
   |             ^^^^

error: duplicate `rename`
  --> tests/ui/bad_values.rs:14:28
   |
14 |     #[shelf(rename = "e1", rename = "e2")]
   |                            ^^^^^^^^^^^^^

error: expected a type, as in `try_from = i64`
  --> tests/ui/bad_values.rs:16:24
   |
16 |     #[shelf(try_from = "not a type")]
   |                        ^^^^^^^^^^^^
```

The first two errors come from syn itself, at the token where parsing failed: after `rename`,
syn wanted an `=` and found the end of the list (so it points at the closing parenthesis), and
`LitStr` wanted a string where there was a number. The other four are shelf's.

> **Macro lens:** a proc macro on stable Rust cannot merge two spans into one
> (`Span::join` is unstable), so how does syn underline `rename = "e2"` from its first token
> to its last? It keeps both ends. `to_compile_error` writes `::core::compile_error!` with the
> first span on every token of the path, and the braces around the message with the last span,
> and the compiler underlines the whole invocation, which now runs from one to the other
> (syn 3.0.9, `error.rs`).

A key whose value is itself a list, `key(…)`, is parsed by calling `meta.parse_nested_meta`
inside the closure: it walks the inner list with the same rules. `FromRow` has no such key; the
relations of Chapter 11 will.

**One namespace, every key.** Because all of shelf's derives share `#[shelf(…)]`, each of them
will parse every key and then use the ones that concern it. An unknown key is still refused, so
that a typo cannot hide behind another derive. Today the only key on the struct is
`rename_all`, and `table`, which belongs to Chapter 6's `Table` derive, does not exist yet, so
`FromRow` refuses it (`tests/ui/unknown_attributes.rs`). Chapter 6 adds it to the shared parser,
and `FromRow` then ignores it.

## 4.3 The six attributes

What each attribute changes is easiest to see in the generated code. This program runs shelf's
generator on a struct that uses all six:

```rust
use syn::{DeriveInput, parse_quote};

fn main() {
    let input: DeriveInput = parse_quote! {
        #[shelf(rename_all = "camelCase")]
        struct Listing {
            #[shelf(rename = "sku_code")]
            sku: String,
            unit_price: i64,
            #[shelf(flatten)]
            stock: Stock,
            #[shelf(json)]
            attributes: Attributes,
            #[shelf(try_from = i32)]
            weight: Grams,
            #[shelf(skip)]
            selected: bool,
        }
    };
    let expanded = shelf_codegen::from_row::expand(&input).expect("valid attributes");
    let code = shelf_playground::pretty(expanded);
    println!("{code}");

    assert!(code.contains(r#""sku", "sku_code")"#)); // rename
    assert!(code.contains(r#""unit_price", "unitPrice")"#)); // rename_all
    assert!(code.contains("<Stock as ::shelf::FromRow>::resolve(columns)?")); // flatten
    assert!(code.contains("::shelf::__private::Json<Attributes>")); // json
    assert!(code.contains("get_try_from")); // try_from
    assert!(code.contains("selected: ::core::default::Default::default()")); // skip
}
```

```text
#[automatically_derived]
impl ::shelf::FromRow for Listing {
    type Columns = (
        usize,
        usize,
        <Stock as ::shelf::FromRow>::Columns,
        usize,
        usize,
        (),
    );
    fn resolve(
        columns: &[::shelf::__private::tokio_postgres::Column],
    ) -> ::core::result::Result<Self::Columns, ::shelf::Error> {
        ::core::result::Result::Ok((
            ::shelf::__private::column::<String>(columns, "Listing", "sku", "sku_code")?,
            ::shelf::__private::column::<
                i64,
            >(columns, "Listing", "unit_price", "unitPrice")?,
            <Stock as ::shelf::FromRow>::resolve(columns)?,
            ::shelf::__private::column::<
                ::shelf::__private::Json<Attributes>,
            >(columns, "Listing", "attributes", "attributes")?,
            ::shelf::__private::column::<i32>(columns, "Listing", "weight", "weight")?,
            (),
        ))
    }
    fn from_row_at(
        row: &::shelf::__private::tokio_postgres::Row,
        columns: &Self::Columns,
    ) -> ::core::result::Result<Self, ::shelf::Error> {
        ::core::result::Result::Ok(Self {
            sku: ::shelf::__private::get::<String>(row, columns.0, "Listing", "sku")?,
            unit_price: ::shelf::__private::get::<
                i64,
            >(row, columns.1, "Listing", "unit_price")?,
            stock: <Stock as ::shelf::FromRow>::from_row_at(row, &columns.2)?,
            attributes: ::shelf::__private::get::<
                ::shelf::__private::Json<Attributes>,
            >(row, columns.3, "Listing", "attributes")?
                .0,
            weight: ::shelf::__private::get_try_from::<
                i32,
                Grams,
            >(row, columns.4, "Listing", "weight")?,
            selected: ::core::default::Default::default(),
        })
    }
}
```

Against the plain field of Chapter 3, which `resolve` finds with `column::<T>` and
`from_row_at` reads with `get::<T>` (Chapter 5 explains the pair), each attribute changes one
thing:

| Attribute | `Columns` holds | `resolve` calls | `from_row_at` reads with |
|---|---|---|---|
| none | `usize` | `column::<T>` | `get::<T>` |
| `rename`, `rename_all` | `usize` | `column::<T>`, with another name | `get::<T>` |
| `skip` | `()` | nothing | `Default::default()` |
| `flatten` | `<T as FromRow>::Columns` | `T::resolve` | `T::from_row_at` |
| `json` | `usize` | `column::<Json<T>>` | `get::<Json<T>>`, then `.0` |
| `try_from = S` | `usize` | `column::<S>` | `get_try_from::<S, T>` |

In the generator, this is the `match` that Chapter 3's excerpt elided. `attrs.read()` says how
a field is read, once its attributes are known to be consistent (§4.4):

~~~rust
// orm/shelf-codegen/src/from_row.rs (excerpt)
// The type `resolve` checks the column against, and the expression that reads it.
let (checked, read) = match attrs.read() {
    Read::Skip => {
        bounds.add_if_generic(&[ty], parse_quote!(#ty: ::core::default::Default));
        column_types.push(quote!(()));
        resolves.push(quote!(()));
        reads.push(quote!(#ident: ::core::default::Default::default()));
        continue;
    }
    Read::Flatten => {
        bounds.add_if_generic(&[ty], parse_quote!(#ty: ::shelf::FromRow));
        column_types.push(quote!(<#ty as ::shelf::FromRow>::Columns));
        resolves.push(quote!(<#ty as ::shelf::FromRow>::resolve(columns)?));
        reads.push(quote! {
            #ident: <#ty as ::shelf::FromRow>::from_row_at(row, &columns.#at)?
        });
        continue;
    }
    // … Read::Column, as in Chapter 3
    Read::Json => {
        let json = quote!(::shelf::__private::Json<#ty>);
        bounds.add_if_generic(&[ty], parse_quote!(#json: ::shelf::FromColumn));
        (
            json.clone(),
            quote!(::shelf::__private::get::<#json>(row, columns.#at, #model, #name)?.0),
        )
    }
    Read::TryFrom(source) => {
        // … the bounds a generic struct needs (§4.6)
        (
            quote!(#source),
            quote! {
                ::shelf::__private::get_try_from::<#source, #ty>(
                    row, columns.#at, #model, #name,
                )?
            },
        )
    }
};
let column = match &attrs.rename {
    Some(rename) => rename.value.value(),
    None => container.column_for(&name),
};
~~~

[orm/shelf-codegen/src/from_row.rs](../shelf-codegen/src/from_row.rs)

### `rename` and `rename_all`

`rename` replaces the column name; `rename_all` on the struct sets a convention for every field
without one. Both work on the field's name after `unraw` (§3.5), and `rename` wins. shelf knows
four conventions, written by hand in `case.rs` rather than taken from the `heck` crate, because
four is all a PostgreSQL schema needs. `snake_case` is the default, because PostgreSQL folds
unquoted names to lower case, so most schemas are written that way. The other three turn up in
tables created with quoted names, often by tools in other languages:

~~~rust
// orm/shelf-codegen/src/case.rs (excerpt)
pub fn apply(self, field: &str) -> String {
    match self {
        Case::Snake => field.to_owned(),
        Case::ScreamingSnake => field.to_ascii_uppercase(),
        Case::Camel | Case::Pascal => {
            let mut out = String::with_capacity(field.len());
            let mut upper = self == Case::Pascal;
            for c in field.chars() {
                if c == '_' {
                    upper = !out.is_empty() || self == Case::Pascal;
                } else if upper {
                    out.extend(c.to_uppercase());
                    upper = false;
                } else {
                    out.push(c);
                }
            }
            out
        }
    }
}
~~~

[orm/shelf-codegen/src/case.rs](../shelf-codegen/src/case.rs)

The conversion runs inside the derive, so the generated code holds the finished name as a
literal (`"unitPrice"` above) and pays nothing for it at run time. Names are then matched
exactly, as PostgreSQL reports them: in SQL a camelCase column must be quoted,
`"unitPrice"`, or the server folds it to `unitprice`.

### `skip`

A skipped field is not read: it gets `::core::default::Default::default()`, with the absolute
path that kept §3.5's hostile module, with its own trait called `Default`, from capturing it.
Its element of `Columns` is `()` rather than nothing, so that every other field's element keeps
the index of its field: `columns.4` is still the fifth field's.

### `flatten`

A flattened field is a struct with its own `FromRow`, read from the same row. Its `Columns` is
nested whole in the outer tuple, its `resolve` searches the same column list, and its
`from_row_at` gets the same row with its own part of `columns`. The inner struct's derive does
its own naming: the outer struct's `rename_all` does not reach into it. Chapter 1's limitation
remains (§1.4): a struct flattened twice reads the same columns twice, and the way out is still
to alias the columns in SQL.

### `json`

`json` reads the column through `Json<T>`, postgres-types' wrapper
([main book §3.7](../../book/03-rows-and-types.md#37-json)), whose `FromSql` accepts `json` and
`jsonb` and deserializes with serde, so `T` must implement `Deserialize`. The generated code
unwraps it with `.0`. Without the attribute, the field would have to be a `Json<T>`.

> **Pitfall:** `#[shelf(json)]` on an `Option<T>` field reads `Json<Option<T>>`, and that is
> not the same as `Option<Json<T>>`. A JSON `null` in the column becomes `None`, but an SQL
> `NULL` fails the row with a `Null` error: in postgres-types 0.2.14 only `Option<T>` decodes
> NULL (its `from_sql_null` returns `None`), and here the `Option` is inside the wrapper, whose
> `from_sql_null` is the default `WasNull` error. This program reads all three cases:

```rust
use serde::Deserialize;
use shelf::{Error, FromRow, Mapping};

#[derive(Debug, Deserialize)]
struct Attributes {
    colour: String,
}

#[derive(Debug, FromRow)]
struct Listing {
    #[shelf(json)]
    attributes: Option<Attributes>,
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let client = shelf_playground::connect().await?;

    let document: Listing = shelf::fetch_one(
        &client,
        r#"SELECT '{"colour": "blue"}'::jsonb AS attributes"#,
        &[],
    )
    .await?;
    assert_eq!(
        document.attributes.map(|a| a.colour).as_deref(),
        Some("blue")
    );

    let json_null: Listing =
        shelf::fetch_one(&client, "SELECT 'null'::jsonb AS attributes", &[]).await?;
    assert!(json_null.attributes.is_none());

    let sql_null = shelf::fetch_one::<Listing>(&client, "SELECT NULL::jsonb AS attributes", &[])
        .await
        .unwrap_err();
    println!("{sql_null}");
    assert!(matches!(
        sql_null,
        Error::Mapping {
            problem: Mapping::Null { .. },
            ..
        }
    ));

    // Without the attribute, Option<Json<T>> reads an SQL NULL as None.
    let row = client.query_one("SELECT NULL::jsonb", &[]).await?;
    let attributes: Option<tokio_postgres::types::Json<Attributes>> = row.try_get(0)?;
    assert!(attributes.is_none());
    Ok(())
}
```

```text
Listing.attributes: column "attributes" is NULL; read it into an Option
```

The message's advice does not fit this case: the field already is an `Option`. For a nullable
`json` or `jsonb` column, declare the field as `Option<tokio_postgres::types::Json<T>>` without
the attribute. sqlx has a separate key for the case, `#[sqlx(json(nullable))]`.

### `try_from`

`try_from = S` reads the column as `S`, then converts with `TryFrom<S>`. The type to read is
written as a type, `try_from = i32`, or quoted as serde writes it. A conversion that fails, a
negative weight for a `Grams`, fails its row with a `Mapping::Decode` error that names the
struct, the field and the column, and carries the conversion's own error as its source. For
that, the conversion's error type must implement `std::error::Error + Send + Sync + 'static`,
and the generated code says so in its bounds.

The side by side at the end of the chapter reads all six attributes from a table, by hand and
with shelf, and shows that error next to the one hand-written code gives.

## 4.4 Every error at once

A derive that stops at its first complaint makes you rebuild once per mistake. A `syn::Error`
can hold several messages instead: `combine` appends one error to another,
`into_compile_error` turns each into its own `compile_error!`, and iterating an error yields
them one by one. shelf collects them in a small wrapper:

~~~rust
// orm/shelf-codegen/src/attrs.rs (excerpt)
/// Errors gathered while parsing, so that one build reports all of them, not only the first.
#[derive(Default)]
pub struct Errors(Option<Error>);

impl Errors {
    pub fn push(&mut self, error: Error) {
        match &mut self.0 {
            Some(all) => all.combine(error),
            None => self.0 = Some(error),
        }
    }

    pub fn finish(self) -> Result<()> {
        self.0.map_or(Ok(()), Err)
    }
}
~~~

[orm/shelf-codegen/src/attrs.rs](../shelf-codegen/src/attrs.rs)

and the generator parses everything before it gives up. This is the beginning of
`from_row::expand`, which Chapter 3 elided:

~~~rust
// orm/shelf-codegen/src/from_row.rs (excerpt)
pub fn expand(input: &DeriveInput) -> Result<TokenStream> {
    let fields = named_fields(input)?;

    let mut errors = Errors::default();
    for lifetime in input.generics.lifetimes() {
        errors.push(Error::new_spanned(
            lifetime,
            "FromRow reads owned values, so the struct cannot borrow from the row: use `String` \
             instead of `&str`, and `Vec<u8>` instead of `&[u8]`",
        ));
    }
    let container = ContainerAttrs::parse(&input.attrs, &mut errors);
    let fields: Vec<(&Field, FieldAttrs)> = fields
        .iter()
        .map(|field| (field, FieldAttrs::parse(&field.attrs, &mut errors)))
        .collect();
    errors.finish()?;
    // …
}
~~~

[orm/shelf-codegen/src/from_row.rs](../shelf-codegen/src/from_row.rs)

Only a struct of the wrong shape stops it at once (`named_fields(input)?`), because then there
are no fields to look at. Everything else is pushed: the lifetimes (§4.6), the struct's
attributes, and every field's. This program gives the generator a struct with five mistakes:

```rust
use syn::{DeriveInput, parse_quote};

fn main() {
    let input: DeriveInput = parse_quote! {
        #[shelf(rename_all = "kebab-case")]
        struct Product {
            #[shelf(renamed = "sku_code", skip = 1)]
            sku: String,
            #[shelf(skip, rename = "price_cents")]
            price: i64,
            #[shelf(json, try_from = i32)]
            weight: Grams,
        }
    };
    let error = shelf_codegen::from_row::expand(&input).expect_err("mistakes to report");
    let messages: Vec<String> = error.into_iter().map(|e| e.to_string()).collect();
    for message in &messages {
        println!("{message}");
    }
    assert_eq!(messages.len(), 4);
    assert!(messages[1].starts_with("unknown shelf attribute `renamed`"));
}
```

```text
unknown case: expected "snake_case", "camelCase", "PascalCase" or "SCREAMING_SNAKE_CASE"
unknown shelf attribute `renamed` on a field; expected `rename`, `skip`, `flatten`, `json`, `try_from`
`rename` cannot apply to a field marked `skip`, which is not read
`try_from` and `json` cannot be combined: read the JSON into a type that converts itself
```

Five mistakes, four errors. `skip = 1` is not reported, because it follows `renamed` in the same
attribute: `parse_nested_meta` returns as soon as the closure returns an error, and the rest of
that attribute is never parsed (syn 3.0.9, `meta.rs`). So the unit of collection is the
attribute: each `#[shelf(…)]` reports at most one parse error, and every attribute of every
field is parsed. Fix the first error in an attribute, and the next one shows up on the next
build.

The last two errors are of another kind. Each key was valid alone, and the combination is not.
Those checks run after a field's attributes are parsed, and point at the key that does not
fit:

~~~rust
// orm/shelf-codegen/src/attrs.rs (excerpt)
/// Refuses keys that cannot go together, pointing at the one that does not fit.
fn check_combinations(&self, errors: &mut Errors) {
    let others = [
        ("rename", self.rename.as_ref().map(|a| a.span)),
        ("flatten", self.flatten.as_ref().map(|a| a.span)),
        ("json", self.json.as_ref().map(|a| a.span)),
        ("try_from", self.try_from.as_ref().map(|a| a.span)),
    ];
    if self.skip.is_some() {
        for (name, span) in others {
            if let Some(span) = span {
                errors.push(Error::new(
                    span,
                    format!(
                        "`{name}` cannot apply to a field marked `skip`, which is not read"
                    ),
                ));
            }
        }
    } else if self.flatten.is_some() {
        // … the same, for every key but `flatten`
    } else if let (Some(_), Some(try_from)) = (&self.json, &self.try_from) {
        errors.push(Error::new(
            try_from.span,
            "`try_from` and `json` cannot be combined: read the JSON into a type that \
             converts itself",
        ));
    }
}
~~~

[orm/shelf-codegen/src/attrs.rs](../shelf-codegen/src/attrs.rs)

Each refusal has a reason the message gives. A skipped field is not read, so a `rename`, `json`
or `try_from` on it would be dead. A flattened struct reads its own columns, so a name or a
wrapper on the outer field would mean nothing. And `json` with `try_from` has two plausible
meanings (convert the JSON value, or convert after decoding), so shelf refuses it and points at
the answer serde already has: a type that converts itself while it is deserialized. The UI test
for these three, with the span of each error on the key that does not fit:

<!-- mirror: orm/shelf/tests/ui/conflicts.stderr -->
```text
error: `rename` cannot apply to a field marked `skip`, which is not read
  --> tests/ui/conflicts.rs:11:19
   |
11 |     #[shelf(skip, rename = "a")]
   |                   ^^^^^^

error: `json` cannot apply to a `flatten` field, whose struct reads its own columns
  --> tests/ui/conflicts.rs:13:22
   |
13 |     #[shelf(flatten, json)]
   |                      ^^^^

error: `try_from` and `json` cannot be combined: read the JSON into a type that converts itself
  --> tests/ui/conflicts.rs:15:19
   |
15 |     #[shelf(json, try_from = i64)]
   |                   ^^^^^^^^
```

So an error lands at one of three places (PLAN.md §12 #12): at a key, when the key is the
problem (`Keyed::span`, or `meta.error` before the value is read); at a value, when the value
is (`Error::new_spanned(&lit, …)`, as for an empty name or an unknown case); or at a field's
type, when the compiler finds that the generated code does not type-check. That last kind is
the subject of the next section.

## 4.5 UI tests

An error message is part of a derive's interface, so shelf tests it like one. trybuild compiles
each file in `tests/ui/`, expects it to fail, and compares the compiler's output with the
`.stderr` file beside it. One test function covers the whole directory:

~~~rust
// orm/shelf/tests/ui.rs (excerpt)
#[test]
fn derive_errors() {
    trybuild::TestCases::new().compile_fail("tests/ui/*.rs");
}
~~~

[orm/shelf/tests/ui.rs](../shelf/tests/ui.rs)

Each file is a small program that should not compile, with its mistakes on separate lines so
that each error has its own line number, and the `.stderr` beside it holds the compiler's
output: `conflicts.rs` holds the three fields of §4.4, and `conflicts.stderr` is what §4.4
printed. There is a pair for each kind of mistake: the shapes of §3.7, unknown keys, the bad
values of §4.2, the conflicts of §4.4, an unreadable type and a lifetime (below).

trybuild builds the files in a project of its own, which depends on shelf, and normalizes what
the compiler prints before it compares, so that the files do not depend on the machine
(trybuild 1.0.122, `normalize.rs`). Paths become relative to the crate (`tests/ui/…`). Line
numbers are kept for the test file and erased for every other file, so that editing shelf's
sources does not break every test. And a list that rustc cuts short with "and 20 others" says
"and $N others".

The first run of a new test has no `.stderr` to compare with. trybuild then fails the test and
writes what the compiler said into `wip/`, for you to read and move into place. Run with
`TRYBUILD=overwrite` and it writes the file in place, and also replaces any `.stderr` that no
longer matches:

```bash
TRYBUILD=overwrite cargo test -p shelf --test ui
git diff orm/shelf/tests/ui/
```

The expected files are rustc's words, so a new toolchain may change them without any change to
shelf; shelf's are rustc 1.99.1's. Regenerate them deliberately, and read the diff as you would
read a change to an API.

### What reading them changed

Reading those files for the first time changed shelf. For a field whose type tokio-postgres
cannot read, such as `std::time::Instant`, the error named the bound the generated code had
used: `shelf::__private::tokio_postgres::types::FromSqlOwned`, a path that no user should ever
have to see. shelf now bounds field types by a trait of its own, implemented for every type the
driver can read, and carrying a message:

~~~rust
// orm/shelf/src/row.rs (excerpt)
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot be read from a PostgreSQL column",
    label = "tokio-postgres has no `FromSql` for this type",
    note = "use a type tokio-postgres reads, add `#[shelf(json)]` or `#[shelf(try_from = …)]`, \
            or implement `FromSql` for it"
)]
pub trait FromColumn: FromSqlOwned {}

impl<T: FromSqlOwned> FromColumn for T {}
~~~

[orm/shelf/src/row.rs](../shelf/src/row.rs)

`#[diagnostic::on_unimplemented]` was stabilized in Rust 1.78.0 (the release notes:
"Stabilize the `#[diagnostic]` namespace and `#[diagnostic::on_unimplemented]` attribute").
When a type does not implement the trait, the compiler uses `message` as the error's headline,
`label` under the offending code and `note` as a note, and then adds its own explanation. It
works through the blanket impl: `Instant` fails `FromColumn` because it fails `FromSqlOwned`,
and the headline is still shelf's (PLAN.md §12 #11). For this struct

<!-- mirror: orm/shelf/tests/ui/not_readable.rs -->
~~~rust
// A field type that tokio-postgres cannot read: the compiler's error names the field's type.
use shelf::FromRow;

#[derive(FromRow)]
struct Timing {
    started: std::time::Instant,
}

fn main() {}
~~~

the compiler now says:

```text
error[E0277]: `Instant` cannot be read from a PostgreSQL column
 --> tests/ui/not_readable.rs:6:14
  |
6 |     started: std::time::Instant,
  |              ^^^^^^^^^^^^^^^^^^ tokio-postgres has no `FromSql` for this type
  |
  = help: the trait `for<'a> shelf::__private::tokio_postgres::types::FromSql<'a>` is not implemented for `Instant`
  = note: use a type tokio-postgres reads, add `#[shelf(json)]` or `#[shelf(try_from = …)]`, or implement `FromSql` for it
  = help: the following other types implement trait `shelf::__private::tokio_postgres::types::FromSql<'a>`:
            &'a [u8]
            &'a str
            Box<T>
            Box<[T]>
            Box<str>
            Cow<'a, str>
            HashMap<String, Option<String>, S>
            IpAddr
          and $N others
  = note: required for `Instant` to implement `shelf::__private::tokio_postgres::types::FromSqlOwned`
  = note: required for `Instant` to implement `FromColumn`
note: required by a bound in `shelf::__private::column`
 --> src/row.rs
  |
  | pub fn column<T: FromColumn>(
  |                  ^^^^^^^^^^ required by this bound in `column`
```

The error points at the field's type in your struct, not at the derive, because quote keeps the
spans of the tokens it copies from the input (§3.7): `column::<#ty>` carries the span of the
`std::time::Instant` you wrote.

That is the first of two errors (`tests/ui/not_readable.stderr` has both). The second is the
same, except for its last note, `required by a bound in shelf::__private::get`: the type is used
twice in the generated code, by `column` in `resolve` and by `get` in `from_row_at`, and each use
is checked. shelf accepts the repetition, because the one way to report it once loses both the
message and the position (§4.6).

## 4.6 Generics

A generic struct needs a generic impl: `impl<T, M> FromRow for Page<T, M>`, with whatever bounds
make its body compile. syn's `Generics::split_for_impl` gives the three pieces such an impl
needs, `<T, M>` after `impl`, `<T, M>` after the type's name and the `where` clause, and the
generator adds its own predicates to the `where` clause first. Which predicates? It depends on
how each field is read, so shelf adds one per field, and only for a field whose type mentions a
type parameter:

```rust
use syn::{DeriveInput, parse_quote};

fn main() {
    let input: DeriveInput = parse_quote! {
        struct Page<T, M> {
            total: i64,
            item: T,
            #[shelf(flatten)]
            meta: M,
            #[shelf(json)]
            extra: Vec<T>,
        }
    };
    let expanded = shelf_codegen::from_row::expand(&input).expect("valid attributes");
    let code = shelf_playground::pretty(expanded);
    // The impl's header: everything before its first item.
    let header: Vec<&str> = code
        .lines()
        .take_while(|line| !line.contains("type Columns"))
        .collect();
    println!("{}", header.join("\n"));

    assert!(code.contains("T: ::shelf::FromColumn,"));
    assert!(code.contains("M: ::shelf::FromRow,"));
    assert!(code.contains("::shelf::__private::Json<Vec<T>>: ::shelf::FromColumn,"));
    assert!(!code.contains("i64: ")); // no bound for a concrete type
}
```

```text
#[automatically_derived]
impl<T, M> ::shelf::FromRow for Page<T, M>
where
    T: ::shelf::FromColumn,
    M: ::shelf::FromRow,
    ::shelf::__private::Json<Vec<T>>: ::shelf::FromColumn,
{
```

`item: T` is read from a column, so `T: FromColumn`. `meta: M` is flattened, so `M: FromRow`.
`extra: Vec<T>` is read through `Json`, so its bound is on `Json<Vec<T>>`, not on `T`: for that
field alone, `T` would only have to be deserializable. And `total: i64` gets no bound.

The standard library's derives work differently: `#[derive(Clone)]` on `Page<T, M>` requires
`T: Clone` and `M: Clone`, one bound per parameter, whatever the fields are. For `FromRow` that
rule would be wrong, because the right bound depends on the attribute: a parameter bounded by
`FromColumn` could not be flattened. This program uses a generic wrapper that adds a total to
any row type, a derived struct and a tuple included:

```rust
use shelf::FromRow;

/// A row of a page, with the number of rows the whole query has.
#[derive(Debug, FromRow)]
struct Counted<T> {
    total: i64,
    #[shelf(flatten)]
    item: T,
}

#[derive(Debug, PartialEq, FromRow)]
struct Product {
    sku: String,
    price_cents: i64,
}

#[tokio::main]
async fn main() -> Result<(), shelf::Error> {
    let client = shelf_playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE products (sku text PRIMARY KEY, price_cents bigint NOT NULL);
             INSERT INTO products VALUES ('MUG-01', 1299), ('TEE-02', 2500), ('CAP-03', 1800);",
        )
        .await?;

    // count(*) OVER () is computed before LIMIT: every row carries the full count.
    let page: Vec<Counted<Product>> = shelf::fetch_all(
        &client,
        "SELECT sku, price_cents, count(*) OVER () AS total FROM products ORDER BY sku LIMIT 2",
        &[],
    )
    .await?;
    assert_eq!(page.len(), 2);
    assert_eq!(page[0].total, 3);
    assert_eq!(
        page[0].item,
        Product {
            sku: "CAP-03".into(),
            price_cents: 1800
        }
    );

    // A tuple is a FromRow too; it reads by position, the first column here.
    let skus: Vec<Counted<(String,)>> = shelf::fetch_all(
        &client,
        "SELECT sku, count(*) OVER () AS total FROM products ORDER BY sku",
        &[],
    )
    .await?;
    assert_eq!(skus[2].item.0, "TEE-02");
    Ok(())
}
```

`Counted<Product>` needs `Product: FromRow`, which holds; `T: FromColumn`, the `Clone`-style
bound, would have refused it.

The code that decides is short. `mentions_param` walks a type's tokens looking for the name of a
type parameter, inside groups too, so `Vec<T>` and `Option<(T, i64)>` count:

~~~rust
// orm/shelf-codegen/src/generics.rs (excerpt)
/// Adds `predicate` if any of `types` mentions a type parameter. A bound on a concrete
/// type would be checked where the derive is, far from the field, so it is left out: the
/// generated code reports a concrete type that does not fit at the field itself.
pub fn add_if_generic(&mut self, types: &[&Type], predicate: WherePredicate) {
    if types.iter().any(|ty| self.mentions_param(ty)) {
        self.predicates.push(predicate);
    }
}

// … `apply` adds the predicates to a clone of the struct's `where` clause
fn mentions(tokens: TokenStream, params: &[Ident]) -> bool {
    tokens.into_iter().any(|tree| match tree {
        TokenTree::Ident(ident) => params.contains(&ident),
        TokenTree::Group(group) => mentions(group.stream(), params),
        _ => false,
    })
}
~~~

[orm/shelf-codegen/src/generics.rs](../shelf-codegen/src/generics.rs)

The check works on tokens, not on meaning, so it can be fooled: a concrete type that happens to
share a parameter's name, say a path ending in `::T`, would get a bound too. That costs nothing
when the bound holds, and darling's `usage` module (§4.7) is the precise alternative.

### Why no bounds on concrete types

Why not bound every field's type, `std::time::Instant: FromColumn` included, so that §4.5's
error appears once instead of twice? This probe derive, written for this chapter, does that for
a struct's first field, in an empty impl of a marker trait that is there only to carry the
`where` clause:

~~~rust
#[proc_macro_derive(BoundFirst)]
pub fn bound_first(input: TokenStream) -> TokenStream {
    // … `ty`, the first field's type, and `name`, the struct's
    quote! {
        impl ::core::marker::Unpin for #name where #ty: ::shelf::FromColumn {}
    }
    .into()
}
~~~

On §4.5's `Timing`, rustc 1.99.1 reports one error, as hoped, and loses both things §4.5 gained:

```text
error[E0277]: the trait bound `Instant: shelf::__private::tokio_postgres::types::FromSqlOwned` is not satisfied
 --> user/src/bin/bound_first.rs:2:10
  |
2 | #[derive(show::BoundFirst)]
  |          ^^^^^^^^^^^^^^^^ the trait `for<'a> shelf::__private::tokio_postgres::types::FromSql<'a>` is not implemented for `Instant`
  |
  = help: the following other types implement trait `shelf::__private::tokio_postgres::types::FromSql<'a>`:
            &'a [u8]
            &'a str
            Box<T>
            Box<[T]>
            Box<str>
            Cow<'a, str>
            HashMap<String, Option<String>, S>
            IpAddr
          and 20 others
  = note: required for `Instant` to implement `shelf::__private::tokio_postgres::types::FromSqlOwned`
  = note: this error originates in the derive macro `show::BoundFirst` (in Nightly builds, run with -Z macro-backtrace for more info)
```

The error points at the derive, not at the field, although `#ty` carries the field's span. And
it is in the driver's words: to accept the `where` clause, the compiler first needs `Instant` to
meet `FromColumn`'s supertrait, `FromSqlOwned`, and it reports that failure, without
`FromColumn`'s message. Two errors at the field that say what to do are better than one at the
derive that does not, so shelf bounds only what is generic, and leaves concrete types to be
checked where the generated code uses them.

### Lifetimes are refused

A struct that borrows from the row, `struct Product<'a> { sku: &'a str }`, is refused, with an
error at the lifetime that says what to use instead:

<!-- mirror: orm/shelf/tests/ui/lifetime.stderr -->
```text
error: FromRow reads owned values, so the struct cannot borrow from the row: use `String` instead of `&str`, and `Vec<u8>` instead of `&[u8]`
 --> tests/ui/lifetime.rs:5:16
  |
5 | struct Product<'a> {
  |                ^^
```

Borrowing would save copying each string out of the row's buffer
([main book §3.4](../../book/03-rows-and-types.md#34-borrowing-from-a-row)). But `FromRow`
returns `Self` with no lifetime tied to the row, and making it `FromRow<'r>` would spread that
lifetime to every function that maps rows: `fetch_all` could no longer return the values,
because the rows they borrow from would be dropped as it returns (Exercise 3). shelf keeps the
trait simple and maps owned values. The generator does not try to tell a harmless lifetime from
a borrow: every lifetime parameter is refused, each with its own error.

## 4.7 What darling would save

[darling](https://docs.rs/darling/0.24.1/darling/) is the crate most derives use for this
chapter's work. Version 0.24.1 is built on syn 3. You declare the attributes as a struct and
derive a parser for it, much as serde derives a deserializer. This is shelf's parser in
darling, written for this chapter and compiled against darling 0.24.1 in a probe:

~~~rust
// darling 0.24.1 (illustration): shelf's attributes as darling receivers
use darling::util::{Flag, SpannedValue};
use darling::{FromDeriveInput, FromField, FromMeta, ast};

#[derive(FromDeriveInput)]
#[darling(attributes(shelf), supports(struct_named))]
struct Input {
    data: ast::Data<(), FieldAttrs>,
    rename_all: Option<Case>,
}

#[derive(FromMeta)]
enum Case {
    #[darling(rename = "snake_case")]
    Snake,
    #[darling(rename = "camelCase")]
    Camel,
    // … PascalCase and SCREAMING_SNAKE_CASE the same way
}

#[derive(FromField)]
#[darling(attributes(shelf), and_then = FieldAttrs::check)]
struct FieldAttrs {
    rename: Option<SpannedValue<String>>,
    skip: Flag,
    flatten: Flag,
    json: Flag,
    try_from: Option<SpannedValue<syn::Type>>,
}

impl FieldAttrs {
    /// The combinations shelf refuses: darling cannot know them.
    fn check(self) -> darling::Result<Self> {
        let mut errors = darling::Error::accumulator();
        if self.json.is_present()
            && let Some(try_from) = &self.try_from
        {
            errors.push(
                darling::Error::custom("`try_from` and `json` cannot be combined")
                    .with_span(&try_from.span()),
            );
        }
        // … the other combinations, the same way
        errors.finish_with(self)
    }
}

// In the derive: Input::from_derive_input(&input), and on failure, errors.write_errors().
~~~

On a struct with the mistakes of shelf's UI tests, plus one attribute with two, this is what
the probe's build reports:

```text
error: Unknown value: `kebab-case`. Available values: `PascalCase`, `SCREAMING_SNAKE_CASE`, `camelCase`, `snake_case`
 --> userd/src/bin/darling.rs:4:22
  |
4 | #[shelf(rename_all = "kebab-case")]
  |                      ^^^^^^^^^^^^

error: Unknown field: `renamed`. Did you mean `rename`?
 --> userd/src/bin/darling.rs:6:13
  |
6 |     #[shelf(renamed = "sku")]
  |             ^^^^^^^

error: `rename` cannot apply to a field marked `skip`
 --> userd/src/bin/darling.rs:8:28
  |
8 |     #[shelf(skip, rename = "a")]
  |                            ^^^

error: `try_from` and `json` cannot be combined
  --> userd/src/bin/darling.rs:10:30
   |
10 |     #[shelf(json, try_from = i64)]
   |                              ^^^

error: Duplicate field `rename`
  --> userd/src/bin/darling.rs:12:28
   |
12 |     #[shelf(rename = "e1", rename = "e2")]
   |                            ^^^^^^

error: Unexpected type `bool`
  --> userd/src/bin/darling.rs:14:20
   |
14 |     #[shelf(skip = true)]
   |                    ^^^^

error: Unexpected type `int`
  --> userd/src/bin/darling.rs:16:22
   |
16 |     #[shelf(rename = 1)]
   |                      ^

error: Unknown field: `renamed`. Did you mean `rename`?
  --> userd/src/bin/darling.rs:24:13
   |
24 |     #[shelf(renamed = "x", skip = 1, json)]
   |             ^^^^^^^

error: Unexpected type `int`
  --> userd/src/bin/darling.rs:24:35
   |
24 |     #[shelf(renamed = "x", skip = 1, json)]
   |                                   ^
```

What darling does for you:

- **Unknown keys, with a suggestion.** "Did you mean `rename`?" comes from comparing the
  unknown name with the known ones (the default `suggestions` feature).
- **Duplicates, wrong kinds of value, and enum values**, each with its message and the list of
  what would have been accepted.
- **Every error in an attribute.** darling goes on after an error inside one attribute, so the
  attribute on line 24 gets two errors (`renamed` and `skip = 1`) where shelf's parser would
  report one.
- **Spans.** `SpannedValue<T>` and `Flag` keep where each value came from, as shelf's `Keyed`
  does; the span is the value's rather than the key's.
- **`darling::usage`**, which finds where a struct's type parameters are used, the precise
  version of §4.6's token check.

What it does not do: shelf's combinations are shelf's rules, so they are written by hand in
`and_then` either way. The wording of its own messages is darling's ("Unexpected type `bool`"
where shelf says "`skip` takes no value"). And it is five more crates to compile before your
derive: darling, darling_core, darling_macro, ident_case and, for the suggestions, strsim.

shelf parses by hand because this book is about the mechanism: `attrs.rs` is about 250 lines,
and every one of them is something darling would otherwise be doing for you. In a derive of
your own with more than a few keys, darling is the better choice. Either way, once you have
written the parser yourself, darling no longer looks like magic.

### Side by side — *attribute-driven mapping*

The task: read a legacy table whose names and types do not match the struct. Its columns were
created with quoted camelCase names, except `sku_code`, added later; two of them belong in a
nested struct; a `jsonb` column holds a document; an `int` must become a `Grams` that refuses
negative values; and one field is not in the table at all. sqlx's derive takes nearly the same
attributes:

~~~rust
// sqlx (illustration): the same struct with sqlx 0.9's #[sqlx(…)] attributes
#[derive(sqlx::FromRow)]
#[sqlx(rename_all = "camelCase")]
struct Listing {
    #[sqlx(rename = "sku_code")]
    sku: String,
    unit_price: i64,
    #[sqlx(flatten)]
    stock: Stock,
    #[sqlx(json)]
    attributes: Attributes,
    #[sqlx(try_from = "i32")]
    weight: Grams,
    #[sqlx(skip)]
    selected: bool,
}
~~~

By hand and with shelf, on the same table, in one program:

```rust
use std::error::Error;

use serde::Deserialize;
use shelf::FromRow;
use tokio_postgres::Row;
use tokio_postgres::types::Json;

/// A document in a `jsonb` column.
#[derive(Debug, PartialEq, Deserialize)]
struct Attributes {
    colour: String,
    ml: Option<u32>,
}

/// A weight that is never negative, although the column would allow it.
#[derive(Debug, PartialEq)]
struct Grams(u32);

impl TryFrom<i32> for Grams {
    type Error = std::num::TryFromIntError;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        u32::try_from(value).map(Grams)
    }
}

/// Two columns of the same row, in a struct of their own.
#[derive(Debug, PartialEq, FromRow)]
#[shelf(rename_all = "camelCase")]
struct Stock {
    on_hand: i32,
    reserved: i32,
}

// By hand: every name, wrapper and conversion, written out in the body.
struct HandListing {
    sku: String,
    unit_price: i64,
    stock: Stock,
    attributes: Attributes,
    weight: Grams,
    selected: bool,
}

impl HandListing {
    fn from_row(row: &Row) -> Result<Self, Box<dyn Error + Send + Sync>> {
        Ok(HandListing {
            sku: row.try_get("sku_code")?,
            unit_price: row.try_get("unitPrice")?,
            stock: Stock {
                on_hand: row.try_get("onHand")?,
                reserved: row.try_get("reserved")?,
            },
            attributes: row.try_get::<_, Json<Attributes>>("attributes")?.0,
            weight: Grams::try_from(row.try_get::<_, i32>("weight")?)?,
            selected: false,
        })
    }
}

// shelf: the same mapping, as attributes.
#[derive(Debug, FromRow)]
#[shelf(rename_all = "camelCase")]
struct Listing {
    #[shelf(rename = "sku_code")]
    sku: String,
    unit_price: i64,
    #[shelf(flatten)]
    stock: Stock,
    #[shelf(json)]
    attributes: Attributes,
    #[shelf(try_from = i32)]
    weight: Grams,
    #[shelf(skip)]
    selected: bool,
}

const SELECT: &str = r#"SELECT sku_code, "unitPrice", "onHand", reserved, attributes, weight
                          FROM listings ORDER BY sku_code"#;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let client = shelf_playground::connect().await?;
    client
        .batch_execute(
            r#"CREATE TEMP TABLE listings (
                   sku_code    text PRIMARY KEY,
                   "unitPrice" bigint NOT NULL,
                   "onHand"    int NOT NULL,
                   reserved    int NOT NULL,
                   attributes  jsonb NOT NULL,
                   weight      int NOT NULL
               );
               INSERT INTO listings VALUES
                   ('MUG-01', 1299, 40, 3, '{"colour": "blue", "ml": 350}', 410),
                   ('TEE-02', 2500, 12, 0, '{"colour": "black"}', 180);"#,
        )
        .await?;

    let rows = client.query(SELECT, &[]).await?;
    let by_hand = rows
        .iter()
        .map(HandListing::from_row)
        .collect::<Result<Vec<_>, _>>()?;
    let with_shelf: Vec<Listing> = shelf::fetch_all(&client, SELECT, &[]).await?;
    for (hand, shelf) in by_hand.iter().zip(&with_shelf) {
        assert_eq!(hand.sku, shelf.sku);
        assert_eq!(hand.unit_price, shelf.unit_price);
        assert_eq!(hand.stock, shelf.stock);
        assert_eq!(hand.attributes, shelf.attributes);
        assert_eq!(hand.weight, shelf.weight);
        assert_eq!(hand.selected, shelf.selected);
    }
    assert_eq!(with_shelf[0].stock.on_hand, 40);
    assert_eq!(with_shelf[1].attributes.ml, None);

    // A weight the conversion refuses: the same failure, reported both ways.
    client
        .execute(
            "UPDATE listings SET weight = -5 WHERE sku_code = 'TEE-02'",
            &[],
        )
        .await?;
    let rows = client.query(SELECT, &[]).await?;
    let Err(hand_error) = rows
        .iter()
        .map(HandListing::from_row)
        .collect::<Result<Vec<_>, _>>()
    else {
        panic!("a negative weight is refused")
    };
    let Err(shelf_error) = shelf::fetch_all::<Listing>(&client, SELECT, &[]).await else {
        panic!("a negative weight is refused")
    };
    println!("by hand: {hand_error}");
    println!("shelf:   {shelf_error}");
    assert!(matches!(
        shelf_error,
        shelf::Error::Mapping {
            field: "weight",
            ..
        }
    ));
    Ok(())
}
```

```text
by hand: number too small to fit in target type
shelf:   Listing.weight: column "weight": number too small to fit in target type
```

The message after shelf's colon is the conversion's own, from the standard library's
`TryFromIntError`, captured rather than remembered: shelf's tests once expected a different
wording, which an older Rust had used (PLAN.md §12 #10).

| | by hand | shelf | sqlx |
|---|---|---|---|
| **Same** | The column's name, the nested struct, the JSON wrapper and the conversion, each stated once per field | The same, as attributes on the fields | The same, as attributes with the same names (`rename_all`, `rename`, `flatten`, `json`, `try_from`, `skip`) |
| **Different** | Any logic at all, in the function body; an error is whatever the failing call returned, without the field or the column | Unknown keys, duplicates and contradictions are compile errors at the token, all in one build; a failed conversion names the struct, the field and the column; `try_from` takes a type or a quoted type | `try_from` takes only a quoted type; `json(nullable)` and a `default` key exist; it has no branch for an unknown field key, and `unwrap`s the result of parsing field attributes, so a mistake there stops the derive with a panic (§3.7; sqlx-macros-core 0.9.0's source) |

---

## In shelf

Parsing and generating meet at one enum. Once a field's attributes are known to be consistent,
`read` reduces them to the single way the field is read, and the generator matches on that
(§4.3) instead of on the combination of keys:

~~~rust
// orm/shelf-codegen/src/attrs.rs (excerpt)
/// How a field is read, once its attributes are known to be consistent.
pub enum Read<'a> {
    /// From one column, by its own `FromSql`.
    Column,
    /// From one column, through `Json<T>`.
    Json,
    /// From one column as the given type, then `TryFrom`.
    TryFrom(&'a Type),
    /// As a nested `FromRow`, from the same row.
    Flatten,
    /// Not at all: `Default::default()`.
    Skip,
}
~~~

[orm/shelf-codegen/src/attrs.rs](../shelf-codegen/src/attrs.rs)

`FieldAttrs::read` returns the first that applies, in the order `skip`, `flatten`, `json`,
`try_from`, and `Column` otherwise; after `check_combinations`, at most one of them can. `rename`
is not part of `Read`, because it changes the column's name, not how its value is read.

Adding an attribute to shelf touches these places, which is also a map of the chapter:

1. **`attrs.rs`**: the key in `FieldAttrs` (or `ContainerAttrs`), a branch in `parse`, and any
   combination it cannot join in `check_combinations`.
2. **`from_row.rs`**: what the field generates, usually a new `Read` arm, with the bound
   `add_if_generic` should add for a generic field (`generics.rs`).
3. **An expansion test**, `orm/shelf-codegen/tests/expand/NAME.rs`, which pins down the code
   it generates (Chapter 13).
4. **An integration test** in `orm/shelf/tests/from_row.rs`, against PostgreSQL.
5. **A UI test** for each error it can report, in `orm/shelf/tests/ui/`.
6. **The attribute table** in the documentation of `shelf::FromRow`
   ([orm/shelf/src/lib.rs](../shelf/src/lib.rs)).

## When not to use an attribute

- **When the SQL can do it.** `SELECT sku_code AS sku` needs no `rename`, and keeps the
  mapping where the query is.
- **When only one query needs it.** An attribute applies to every query that reads the struct:
  a `rename` for one report's alias binds the struct to that name everywhere. Alias in that
  query instead.
- **When the conversion is business logic.** `try_from` is for conversions that only check and
  wrap, such as a non-negative amount or a validated code. If the conversion needs other
  fields, or a lookup, read the raw values and build the type in a constructor.

## Summary

- A derive declares its helper attributes, `attributes(shelf)`. Without that, or without the
  `#[derive]` on the struct, the compiler refuses `#[shelf(…)]` before any macro runs, with a
  note that `shelf` is a crate.
- `Attribute::parse_nested_meta` walks the conventional `key`, `key = value`, `key(…)` syntax;
  the closure reads each key's value with `meta.value()?` and a parse of the right type
  (`LitStr`, `Type`), or nothing for a flag.
- shelf's parser keeps each value with the span of its key (`Keyed`), refuses duplicates, flags
  with values and unknown keys (listing the known ones), and accepts a type quoted or not.
- `rename_all`, `rename`, `skip`, `flatten`, `json` and `try_from` each change one element of
  `Columns`, one call in `resolve` and one read in `from_row_at`. `#[shelf(json)]` on an
  `Option<T>` decodes a JSON `null`, but not an SQL `NULL`.
- Errors are collected with `syn::Error::combine` and reported together. `parse_nested_meta`
  stops at the first error in an attribute, so each attribute reports at most one parse error;
  combinations that cannot go together are refused at the key that does not fit.
- trybuild's UI tests pin down the compiler's output, normalized; regenerate them with
  `TRYBUILD=overwrite` and read the diff. Reading them gave shelf `FromColumn`, whose
  `#[diagnostic::on_unimplemented]` (Rust 1.78+) puts shelf's words on a type error.
- A generic struct gets one bound per field whose type mentions a type parameter, chosen by how
  the field is read; concrete types get none, so their errors keep shelf's message. Lifetime
  parameters are refused.
- darling would parse the same attributes from a declared struct, with better errors for free;
  shelf parses by hand to show the mechanism.

## Exercises

1. **A default for a missing column.** Design `#[shelf(default)]`: read the column if the
   result has it, and use `Default::default()` if it does not (sqlx has the same key). Write,
   by hand, the `impl FromRow` the derive should generate for a struct with one such field, and
   check it against a query with the column and one without.
   *Hint:* `resolve` cannot return an index for a column that is not there; its element of
   `Columns` can be an `Option<usize>`.
2. **Two fields, one column.** Write a check, as a stage function on a `DeriveInput`, that
   refuses two fields that read the same column, and reports the second at its field. Then
   decide whether shelf should have it.
   *Hint:* collect each column name in a `HashMap` with the field that claimed it first, and
   combine the errors.
3. **Borrowing from the row.** Suppose `FromRow` became `FromRow<'r>`, with
   `fn from_row(row: &'r Row) -> Result<Self, Error>`, so that a field could be a `&'r str`.
   What goes wrong in `fetch_all`?
   *Hint:* who owns the rows, and when are they dropped?

### Solutions

**1.** The field's element of `Columns` becomes `Option<usize>`: `resolve` records whether the
column is there, and `from_row_at` reads it or falls back. This is the impl the derive should
generate, written by hand without the type checks that `column` would add:

```rust
use shelf::{FromRow, Mapping};
use tokio_postgres::{Column, Row};

/// A product whose `note` column only some queries select: `#[shelf(default)]` on `note`.
#[derive(Debug, PartialEq)]
struct Product {
    sku: String,
    note: String,
}

impl FromRow for Product {
    /// `None` when the result has no `note` column.
    type Columns = (usize, Option<usize>);

    fn resolve(columns: &[Column]) -> Result<Self::Columns, shelf::Error> {
        let find = |name: &str| columns.iter().position(|c| c.name() == name);
        let sku = find("sku").ok_or_else(|| shelf::Error::Mapping {
            model: "Product",
            field: "sku",
            problem: Mapping::MissingColumn {
                column: "sku".into(),
            },
        })?;
        Ok((sku, find("note")))
    }

    fn from_row_at(row: &Row, columns: &Self::Columns) -> Result<Self, shelf::Error> {
        Ok(Product {
            sku: row.try_get(columns.0)?,
            note: match columns.1 {
                Some(at) => row.try_get(at)?,
                None => Default::default(),
            },
        })
    }
}

#[tokio::main]
async fn main() -> Result<(), shelf::Error> {
    let client = shelf_playground::connect().await?;

    let with_note: Product =
        shelf::fetch_one(&client, "SELECT 'MUG-01' AS sku, 'fragile' AS note", &[]).await?;
    assert_eq!(with_note.note, "fragile");

    let without: Product = shelf::fetch_one(&client, "SELECT 'MUG-01' AS sku", &[]).await?;
    assert_eq!(without.note, "");
    Ok(())
}
```

In the derive, `default` is a flag in `FieldAttrs`, orthogonal to `Read`, like `rename`: a
defaulted field can still be read through `json` or `try_from`, so it should not become a
`Read` arm of its own. It cannot go with `skip` or `flatten`. The generator then wraps the
field's `resolve` call and its read, and `__private` needs a version of `column` that returns
`Ok(None)` for a missing column but still checks the type of one that is present.

**2.** The check needs each field's column name, so it reads `rename` (only `rename`, for
brevity: a full version would apply `rename_all` too):

```rust
use std::collections::HashMap;

use syn::{Data, DeriveInput, Error, Field, Fields, Ident, LitStr, parse_quote};

/// The column a field reads: its `rename`, or its own name.
fn column_of(field: &Field) -> syn::Result<String> {
    let mut column = field.ident.as_ref().expect("a named field").to_string();
    for attr in field.attrs.iter().filter(|a| a.path().is_ident("shelf")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("rename") {
                column = meta.value()?.parse::<LitStr>()?.value();
            }
            Ok(())
        })?;
    }
    Ok(column)
}

/// Refuses a second field that reads a column another field already reads.
fn check_unique(input: &DeriveInput) -> syn::Result<()> {
    let Data::Struct(data) = &input.data else {
        return Ok(());
    };
    let Fields::Named(fields) = &data.fields else {
        return Ok(());
    };
    let mut first_reader: HashMap<String, &Ident> = HashMap::new();
    let mut errors: Option<Error> = None;
    for field in &fields.named {
        let ident = field.ident.as_ref().expect("a named field");
        let column = column_of(field)?;
        if let Some(first) = first_reader.get(&column) {
            let error = Error::new_spanned(
                ident,
                format!("`{first}` already reads the column {column:?}"),
            );
            match &mut errors {
                Some(all) => all.combine(error),
                None => errors = Some(error),
            }
        } else {
            first_reader.insert(column, ident);
        }
    }
    errors.map_or(Ok(()), Err)
}

fn main() {
    let input: DeriveInput = parse_quote! {
        struct Product {
            sku: String,
            #[shelf(rename = "sku")]
            code: String,
            price_cents: i64,
        }
    };
    let error = check_unique(&input).expect_err("two fields read sku");
    println!("{error}");
    assert_eq!(error.to_string(), "`sku` already reads the column \"sku\"");

    let fine: DeriveInput = parse_quote! {
        struct Product { sku: String, #[shelf(rename = "sku_code")] code: String }
    };
    assert!(check_unique(&fine).is_ok());
}
```

shelf does not have this check, on purpose: reading one column twice is odd, but not wrong. A
struct may want a `jsonb` column both as a typed value and as a raw `serde_json::Value`, and
`resolve` finds the same index for both without complaint.

**3.** `fetch_all` runs the query, which gives it a `Vec<Row>` that it owns, and maps every row.
Each value would borrow from its row, so the values could not outlive the vector, and the
vector is dropped when `fetch_all` returns: the borrow checker refuses to return them. The
caller would have to own the rows instead, calling the query and then the mapping, two steps
every time, with the rows kept alive beside the values. Every helper built on `FromRow`
(`fetch_one`, and later the model's `find` and `all`) would inherit the same lifetime and the
same two steps. The main book shows where borrowing does pay: a loop over rows that only looks
at each value briefly ([main book §3.4](../../book/03-rows-and-types.md#34-borrowing-from-a-row)).
shelf maps into owned values, and leaves that loop to hand-written code.

---

Previous: [Chapter 3 — The first derive macro](03-first-derive.md) · Next: [Chapter 5 — Checking a statement once, not every row](05-column-indices.md)
