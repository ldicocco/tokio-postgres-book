# Chapter 3 — The first derive macro

> **Building an ORM on tokio-postgres** · Previous: [Chapter 2 — Declarative macros](02-declarative-macros.md) · Next: [Chapter 4 — Attributes, errors and generics](04-attributes.md)

A procedural macro is a Rust function that runs inside the compiler. `#[derive(FromRow)]` hands
it the tokens of your struct; it returns more tokens, an `impl` block, which the compiler
compiles as if you had written it. There is no pattern language this time: the function parses
the struct into a syntax tree, loops over its fields, looks at whatever it likes, and builds the
code it needs with ordinary Rust.

This chapter writes the first version of shelf's derive: every named field read from the
column of its name, which is Chapter 1's impl written by a program. Most of the work is not the
generator, which is twenty lines of syn and quote, but the setting around it: the kind of crate
it must live in, the crate it should live in instead, the paths that keep the generated code
working in any module, the ways to see what was generated, and how a derive reports a mistake
at the right token.

**In this chapter you will learn:**

- what a proc-macro crate is, what the compiler hands a derive, and what it takes back
- why shelf splits its macros into a code generator and a thin proc-macro shell, and what
  `proc_macro2` has to do with it
- how syn 3 parses a struct, and how quote builds tokens with interpolation and repetition
- what hygiene means for a derive: absolute paths, `__private`, raw identifiers
- three ways to see an expansion: printing it, `cargo expand`, and rust-analyzer
- how a derive reports an error as a value, pointing at the token that caused it

---

## 3.1 Proc-macro crates

A derive macro lives in a crate whose `Cargo.toml` says so:

~~~toml
# orm/shelf-macros/Cargo.toml (excerpt)
[lib]
proc-macro = true

[dependencies]
shelf-codegen = { workspace = true }
syn = { workspace = true }
~~~

`proc-macro = true` changes what the crate is. It is compiled for the machine running the
compiler (not the one your program will run on), loaded into the compiler as a plugin, and its
public functions are called during compilation. A derive is one such function, marked with the
attribute that names it:

~~~rust
// orm/shelf-macros/src/lib.rs (excerpt)
#[proc_macro_derive(FromRow, attributes(shelf))]
pub fn derive_from_row(input: TokenStream) -> TokenStream {
    // …
}
~~~

`FromRow` is the name you write in `#[derive(…)]`. `attributes(shelf)` declares `#[shelf(…)]`
as a *helper attribute* that may appear on the struct and its fields; Chapter 4 uses it. The
function takes and returns a `proc_macro::TokenStream`: a sequence of tokens (identifiers,
punctuation, literals, and groups in brackets), not text and not a syntax tree.

What exactly does a derive receive? This derive, a probe written for this chapter and not part
of shelf, turns its input into a string and adds it to the type as a constant:

~~~rust
#[proc_macro_derive(ShowInput, attributes(shelf))]
pub fn show_input(input: TokenStream) -> TokenStream {
    let text = input.to_string();
    let parsed: syn::DeriveInput = syn::parse(input).unwrap();
    let name = parsed.ident;
    quote! { impl #name { pub const INPUT: &'static str = #text; } }.into()
}
~~~

Applied to this struct, between two other derives:

~~~rust
/// A product in the catalog.
#[derive(Debug, show::ShowInput, Clone)]
#[allow(dead_code)]
struct Product {
    /// The stock-keeping unit.
    #[shelf(rename = "sku_code")]
    sku: String,
    #[cfg(any())]
    gone: i32,
    price_cents: i64,
}
~~~

`Product::INPUT` holds:

```text
/// A product in the catalog.
#[allow(dead_code)] struct Product
{
    /// The stock-keeping unit.
    #[shelf(rename = "sku_code")] sku: String, price_cents: i64,
}
```

> **Macro lens:** a derive gets the whole item: doc comments, other attributes, and the
> `#[shelf(…)]` helper attributes it will read. It does not get the `#[derive(…)]` line itself,
> nor the other derives listed in it, and the field under `#[cfg(any())]`, a condition that is
> never true, is already gone: `cfg` is evaluated before derives run. What the derive returns is
> *added after* the struct; it does not replace it. That is why `Product::INPUT` compiles: the
> struct is still there for the `impl` to attach to.

A proc-macro crate has two restrictions, and both shape shelf. It **can export nothing but
macros**. A public function next to the derive, say the code generator itself for a test to
call, is refused:

```text
error: `proc-macro` crate types currently cannot export any items other than functions tagged with `#[proc_macro]`, `#[proc_macro_derive]`, or `#[proc_macro_attribute]`
  --> show/src/lib.rs:14:1
   |
14 | pub fn expand(input: &syn::DeriveInput) -> proc_macro2::TokenStream {
   | ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
```

And **its types work only inside the compiler**. `proc_macro::TokenStream` is a handle to data
the compiler owns. In an ordinary program, `TokenStream::new()` happens to work, but parsing
code into a token stream panics:

~~~rust
extern crate proc_macro;
fn main() {
    let parsed: proc_macro::TokenStream = "struct Product;".parse().unwrap();
}
~~~

```text
thread 'main' (23461715) panicked at /rustc/58cf8f9c35b355199727627d07ae34a4e2846d67/library/proc_macro/src/bridge/client.rs:200:32:
procedural macro API is used outside of a procedural macro
```

So the function at the heart of the derive can be called by the compiler and by nothing else:
not by a unit test in another crate, not by an example in this book.

## 3.2 Three crates

That is why shelf is three crates (§0.4). `shelf-codegen` is an ordinary library that holds the
code generation, as functions from a parsed `syn::DeriveInput` to a `proc_macro2::TokenStream`.
`shelf-macros` is the proc-macro crate, and stays a shell of three lines per derive: parse the
input, call the generator, and turn an error into a compile error:

~~~rust
// orm/shelf-macros/src/lib.rs (excerpt)
use proc_macro::TokenStream;
use syn::{DeriveInput, parse_macro_input};

#[proc_macro_derive(FromRow, attributes(shelf))]
pub fn derive_from_row(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    shelf_codegen::from_row::expand(&input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}
~~~

[orm/shelf-macros/src/lib.rs](../shelf-macros/src/lib.rs)

`proc_macro2` makes the split possible. It is a crate with the same types as the compiler's
`proc_macro` (`TokenStream`, `Ident`, `Span`, `Literal`, `Group`), which work in two modes. Inside
a proc macro, they wrap the compiler's own types; anywhere else, they fall back to an
implementation of their own. syn parses `proc_macro2` tokens and quote produces them, so
`shelf-codegen` never touches `proc_macro`, and runs equally well in the compiler, in its tests,
and in this book's examples. The shell converts at the edges: `parse_macro_input!` parses the
compiler's tokens into a `DeriveInput` (or returns the parse error as a compile error), and
`.into()` turns the generator's `proc_macro2::TokenStream` back into the compiler's.

`shelf` re-exports the derive (`pub use shelf_macros::FromRow;`), so your crate depends on
`shelf` alone and writes `#[derive(shelf::FromRow)]`, or imports it with `use shelf::FromRow`,
which brings in the trait and the derive at once (§0.7).

## 3.3 Parsing with syn 3

syn parses Rust source, as tokens, into a syntax tree. A derive needs one entry point,
`DeriveInput`, which is a struct, an enum or a union with its attributes, visibility, name and
generics. This program parses a struct the way a derive receives it and walks its fields:

```rust
use quote::ToTokens;
use syn::{Data, DeriveInput, Fields, parse_quote};

fn main() {
    let input: DeriveInput = parse_quote! {
        /// A product in the catalog.
        pub struct Product {
            id: i64,
            pub sku: String,
            tags: Option<Vec<String>>,
        }
    };

    println!("name: {}", input.ident);
    println!("attributes: {}", input.attrs.len());
    let Data::Struct(data) = &input.data else {
        panic!("a struct")
    };
    let Fields::Named(fields) = &data.fields else {
        panic!("named fields")
    };
    for field in &fields.named {
        let name = field.ident.as_ref().expect("a named field");
        println!("  field {name}: {}", field.ty.to_token_stream());
    }

    assert_eq!(input.ident, "Product");
    assert_eq!(fields.named.len(), 3);
    assert_eq!(
        fields.named[2].ty.to_token_stream().to_string(),
        "Option < Vec < String > >"
    );
}
```

```text
name: Product
attributes: 1
  field id: i64
  field sku: String
  field tags: Option < Vec < String > >
```

`parse_quote!` builds a syntax tree from Rust code written in place; the book's stage programs
use it to stand in for the compiler. The doc comment is an attribute (`#[doc = "…"]`), hence
`attributes: 1`. And the type of `tags` prints with spaces between its tokens, because it *is*
tokens: `Option`, `<`, `Vec`, `<`, `String`, `>`, `>`. syn knows it is a path type with generic
arguments, but it does not know what `Option` refers to (Chapter 2's refresher).

The pieces a derive walks through:

| Type | What it holds |
|---|---|
| `DeriveInput` | `attrs`, `vis`, `ident` (the name), `generics`, and `data` |
| `Data` | `Struct(DataStruct)`, `Enum(DataEnum)` or `Union(DataUnion)` |
| `Fields` | `Named` (`{ id: i64 }`), `Unnamed` (`(i64, String)`) or `Unit` |
| `Field` | `attrs`, `vis`, `ident` (`None` in a tuple struct), `ty`, and two items new in syn 3 |

syn compiles only what you ask for. Its default features (`derive`, `parsing`, `printing`,
`clone-impls` and `proc-macro`) are all a derive needs. The workspace also enables `full`, the
whole of Rust's syntax, because `pretty` (§3.4) parses generated code into a `syn::File`, which
`full` provides.

**syn 3, not syn 2.** shelf uses syn 3.0.9. Most material online, and most derives on crates.io,
are written against syn 2, and for a derive the difference is small. `DeriveInput`, `Data`,
`Fields` and `Attribute` are unchanged, as are `parse_nested_meta` (Chapter 4) and
`split_for_impl`. `Field` changed in two places: syn 2's `mutability` became
`modifiers: FieldModifiers`, and a new `default` holds a field's default value
(`x: i32 = 5`, a language feature still unstable in Rust 1.99). Code that *reads* a field's
`attrs`, `ident` and `ty`, as shelf does, compiles against either version; code that builds or
destructures a whole `Field` does not. The rest of 3.0's breaking changes are in types,
expressions, statements, patterns, items, generics and literals; the
[3.0.0 release notes](https://github.com/dtolnay/syn/releases/tag/3.0.0) list them.

## 3.4 Generating with quote

quote's `quote!` macro is the other half: it turns Rust code written in place into a
`proc_macro2::TokenStream`, and `#name` inside it pastes in any value that implements `ToTokens`.
A repetition, `#(…)*`, pastes an iterator, one element at a time, with whatever separators the
repetition contains. That is enough for a first derive. Here is its logic as a stage program:
the generator as an ordinary function, run on a struct built with `parse_quote!`:

```rust
use proc_macro2::TokenStream;
use quote::quote;
use syn::{Data, DeriveInput, Fields, parse_quote};

/// The first `#[derive(FromRow)]`: every named field read from the column of its name, as
/// Chapter 1 wrote it by hand.
fn expand(input: &DeriveInput) -> syn::Result<TokenStream> {
    let Data::Struct(data) = &input.data else {
        return Err(syn::Error::new_spanned(
            &input.ident,
            "FromRow needs a struct",
        ));
    };
    let Fields::Named(fields) = &data.fields else {
        return Err(syn::Error::new_spanned(
            &input.ident,
            "FromRow needs named fields",
        ));
    };

    let name = &input.ident;
    let reads = fields.named.iter().map(|field| {
        let ident = field.ident.as_ref().expect("named fields have names");
        let column = ident.to_string();
        quote! { #ident: row.try_get(#column)? }
    });

    Ok(quote! {
        impl FromRow for #name {
            fn from_row(row: &Row) -> Result<Self, tokio_postgres::Error> {
                Ok(Self { #(#reads,)* })
            }
        }
    })
}

fn main() {
    let input: DeriveInput = parse_quote! {
        struct Product {
            id: i64,
            sku: String,
            tags: Option<Vec<String>>,
        }
    };
    let code = shelf_playground::pretty(expand(&input).expect("a struct with named fields"));
    println!("{code}");
    assert!(code.contains(r#"sku: row.try_get("sku")?"#));
    assert!(code.contains(r#"tags: row.try_get("tags")?"#));
}
```

```text
impl FromRow for Product {
    fn from_row(row: &Row) -> Result<Self, tokio_postgres::Error> {
        Ok(Self {
            id: row.try_get("id")?,
            sku: row.try_get("sku")?,
            tags: row.try_get("tags")?,
        })
    }
}
```

That is Chapter 1's impl, letter for letter. Each value pasted into `quote!` becomes the tokens
its type stands for:

- `#name` and `#ident` are `syn::Ident`s, pasted as identifiers.
- `#column` is a `String`, and a string is pasted as a string *literal*: `"sku"`, with the
  quotes. This is how the field's name becomes the column's name.
- `#(#reads,)*` pastes every element of the iterator `reads`, each followed by a comma.

Two helpers come up constantly. `quote::format_ident!` builds a new identifier from parts, as
`format!` builds a string (`format_ident!("{}Columns", name)` is `ProductColumns`). And
`syn::Index` pastes a number as a tuple index, which the next pitfall is about.

> **Pitfall:** a Rust integer pasted into `quote!` keeps its type as a suffix: `1usize`. That
> is right in an expression and wrong as a tuple field, which shelf's finished generator writes
> (`columns.0`, `columns.1`). The derive compiles, and its output fails to parse wherever it is
> used. `syn::Index::from(position)` pastes an unsuffixed `1`:

```rust
use quote::quote;
use syn::Index;

fn main() {
    let position: usize = 1;
    let suffixed = quote!(columns.#position);
    assert_eq!(suffixed.to_string(), "columns . 1usize");
    let error = syn::parse2::<syn::Expr>(suffixed)
        .err()
        .expect("a suffixed tuple index does not parse");
    assert_eq!(error.to_string(), "expected unsuffixed integer");

    let at = Index::from(position);
    assert_eq!(quote!(columns.#at).to_string(), "columns . 1");
}
```

The stage program printed its output with `pretty`, a helper in the shelf playground. It parses
the tokens as a file of items and formats them with prettyplease, which formats as rustfmt would
but works on a syntax tree instead of a file:

<!-- mirror: orm/playground/src/lib.rs#pretty -->
~~~rust
use proc_macro2::TokenStream;

/// Formats generated code as rustfmt would, so that an example can print what a derive writes
/// and assert on it. A derive generates items (`impl` blocks, functions, constants), and that
/// is what `tokens` must hold.
pub fn pretty(tokens: TokenStream) -> String {
    let file: syn::File = syn::parse2(tokens).expect("a derive generates items");
    prettyplease::unparse(&file)
}
~~~

`pretty` also checks the output: tokens that do not parse as items, such as the `1usize` above,
make it panic in the stage program instead of failing later in a user's build.

## 3.5 Hygiene

The stage generator's output works in Chapter 1's program, and in few others. It names
`FromRow`, `Row`, `Result`, `Ok` and `tokio_postgres` as bare paths, and the `impl` lands in
the user's module, next to their struct. If that module has no `use tokio_postgres::Row`, or its
own `type Result<T>`, or a different `FromRow`, the generated code breaks, or quietly means
something else.

A procedural macro has even less protection than Chapter 2's `macro_rules!`. Every token
`quote!` writes carries `Span::call_site()`, and the compiler resolves such tokens *as if you had
typed them next to the struct*: paths, and local variables too. (`Span::mixed_site()` gives
`macro_rules!`'s rules, where a macro's local variables are its own; shelf's generated code has
no locals that could collide, so it does not need it.) So the generated code must not depend on
any name the user's module defines:

- **Every path is absolute.** `::core::result::Result`, `::core::result::Result::Ok`,
  `::shelf::FromRow`, `::shelf::Error`. A leading `::` starts at a crate's name, which nothing in
  the user's module can shadow.
- **tokio-postgres is reached through shelf.** The generated code needs `Row` and `Column`, and
  the user's crate may not depend on tokio-postgres, or may depend on another version. So shelf
  gathers what generated code names in a hidden module, tokio-postgres included:

~~~rust
// orm/shelf/src/lib.rs (excerpt)
/// What generated code refers to. Not part of the API: it may change in any release.
#[doc(hidden)]
pub mod __private {
    // …
    pub use crate::json::Json;
    // …
    pub use crate::row::{column, column_at, get, get_try_from};
    // …
    pub use tokio_postgres;
}
~~~

[orm/shelf/src/lib.rs](../shelf/src/lib.rs)

`::shelf::__private::tokio_postgres::Row` is then the `Row` that shelf itself was compiled
against, whatever the user's crate has. The double underscore and `#[doc(hidden)]` are the usual
signal that a module is for macros only: it does not appear in the documentation, and it may
change in any release. Its helpers, `column` and `get`, keep the generated code short; Chapter 5
explains them. `Json` is shelf's wrapper for `#[shelf(json)]` fields (Chapter 4).

**Raw identifiers.** A field may be named after a keyword, with Rust's raw-identifier prefix:
`r#type`. Its identifier's text includes the prefix, so the stage generator, which uses
`ident.to_string()`, would read a column called `r#type`. syn's `IdentExt::unraw` strips it:

```rust
use syn::ext::IdentExt;
use syn::{DeriveInput, Fields, parse_quote};

fn main() {
    let input: DeriveInput = parse_quote! {
        struct Product { r#type: String }
    };
    let syn::Data::Struct(data) = &input.data else {
        panic!("a struct")
    };
    let Fields::Named(fields) = &data.fields else {
        panic!("named fields")
    };
    let ident = fields.named[0].ident.as_ref().expect("a named field");

    assert_eq!(ident.to_string(), "r#type"); // what the stage generator would read
    assert_eq!(ident.unraw().to_string(), "type"); // the column's name
}
```

The field itself is still written `r#type` in the generated initializer (`r#type: …`), where
the prefix is needed; only the column's name drops it.

**`#[automatically_derived]`** goes on the generated `impl`. The Rust Reference says it "has no
direct effect, but it may be used by tools and diagnostic lints to detect these automatically
generated implementations". The standard library's derives add it, and so does every careful
derive.

This program puts the finished derive in a hostile module: one with its own `Result`, and a
constant named `Ok`. It also reads a column named after a keyword:

```rust
mod catalog {
    // Names that would capture a careless expansion.
    #[allow(dead_code)]
    pub type Result<T> = std::result::Result<T, String>;
    #[allow(dead_code, non_upper_case_globals)]
    const Ok: () = ();

    #[derive(Debug, shelf::FromRow)]
    pub struct Product {
        pub sku: String,
        pub r#type: String,
    }
}

#[tokio::main]
async fn main() -> Result<(), shelf::Error> {
    let client = shelf_playground::connect().await?;
    client
        .batch_execute(
            "CREATE TEMP TABLE products (sku text PRIMARY KEY, type text NOT NULL);
             INSERT INTO products VALUES ('MUG-01', 'mug'), ('TEE-02', 'shirt');",
        )
        .await?;

    let products: Vec<catalog::Product> =
        shelf::fetch_all(&client, "SELECT sku, type FROM products ORDER BY sku", &[]).await?;
    assert_eq!(products[0].r#type, "mug");
    assert_eq!(products[1].sku, "TEE-02");
    Ok(())
}
```

shelf's own tests do the same with a `type Result = ();`, a `const Ok`, and a trait called
`Default`, which `#[shelf(skip)]` must not pick up (Chapter 4):

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
// … and `type str = ();`, `type u8 = ();`

#[allow(dead_code)] // only the impls matter here
#[derive(shelf::FromRow)]
struct Product {
    id: i64,
    r#type: String,
    tags: Option<Vec<String>>,
    #[shelf(skip)]
    cached: Vec<u8>,
}
~~~

[orm/shelf/tests/derive.rs](../shelf/tests/derive.rs)

## 3.6 Seeing the expansion

You will want to see what a derive wrote: to learn, to debug, and to review a change to the
generator. There are three ways.

**Print it.** Because the generator lives in an ordinary library, any program can call it and
format the result with `pretty`, as §0.7 and §3.4 did. This is the way this book uses
throughout, because it works on stable Rust with nothing to install, and because the output can
be asserted on. shelf's expansion tests do exactly this: each file in
`orm/shelf-codegen/tests/expand/` holds structs with a derive, and a test compares the
pretty-printed expansion of each with a file next to it, so that any change to the generated
code shows up as a diff in review (Chapter 13).

**Ask the compiler.** rustc can print a crate after expanding every macro in it, with the
unstable flag `-Zunpretty=expanded`. The tool [cargo-expand](https://github.com/dtolnay/cargo-expand)
wraps it (`cargo install cargo-expand`, then `cargo expand`), and formats the result with
prettyplease, the formatter `pretty` uses. An unstable flag is normally refused by a stable compiler:

```text
error: the option `Z` is only accepted on the nightly compiler
```

cargo-expand gets around that by setting `RUSTC_BOOTSTRAP=1` for the compiler it runs (its
`src/main.rs`), so it works on stable Rust. This is what the compiler itself compiled for §3.5's
`catalog::Product` (the raw command cargo-expand wraps, run the same way, before formatting):

```text
    pub struct Product {
        pub sku: String,
        pub r#type: String,
    }
    #[automatically_derived]
    impl ::core::fmt::Debug for Product {
        #[inline]
        fn fmt(&self, f: &mut ::core::fmt::Formatter) -> ::core::fmt::Result {
            ::core::fmt::Formatter::debug_struct_field2_finish(f, "Product",
                "sku", &self.sku, "type", &&self.r#type)
        }
    }
    #[automatically_derived]
    impl ::shelf::FromRow for Product {
        type Columns = (usize, usize);
        fn resolve(columns: &[::shelf::__private::tokio_postgres::Column])
            -> ::core::result::Result<Self::Columns, ::shelf::Error> {
            ::core::result::Result::Ok((::shelf::__private::column::<String>(columns,
                            "Product", "sku", "sku")?,
                    ::shelf::__private::column::<String>(columns, "Product",
                            "type", "type")?))
        }
        fn from_row_at(row: &::shelf::__private::tokio_postgres::Row,
            columns: &Self::Columns)
            -> ::core::result::Result<Self, ::shelf::Error> {
            ::core::result::Result::Ok(Self {
                    sku: ::shelf::__private::get::<String>(row, columns.0,
                            "Product", "sku")?,
                    r#type: ::shelf::__private::get::<String>(row, columns.1,
                            "Product", "type")?,
                })
        }
    }
```

The compiler's view shows what `pretty` cannot: the expansion in place, next to every other
derive. `#[derive(Debug)]`, from the standard library, follows the same rules as shelf: an
`#[automatically_derived]` impl, every path starting at `::core`, and the field `r#type`
named `"type"`, without its prefix. Expansion to text is
lossy, as cargo-expand's README warns (hygiene, for one, does not survive printing), so treat it
as a debugging aid.

**Ask the editor.** rust-analyzer's command "Expand macro recursively at caret" (in VS Code:
*rust-analyzer: Expand macro recursively at caret*) shows the expansion of the macro under the
cursor.

## 3.7 Errors as values

A derive that cannot do its job could panic, and the compiler would report it. This is a probe
derive that panics on an enum:

```text
error: proc-macro derive panicked
 --> user/src/bin/panics.rs:1:10
  |
1 | #[derive(show::ShowInput)]
  |          ^^^^^^^^^^^^^^^
  |
  = help: message: ShowInput needs a struct
```

The message survives, but the error points at the derive's name instead of the code that is
wrong, and a panic can report only one problem. So shelf's generator never panics on user input.
It returns `syn::Result<TokenStream>`, whose error is a value with a message and a *span*, the
place in the user's source it is about; several errors can be combined into one (Chapter 4).
`syn::Error::new_spanned(tokens, message)` takes the span from any tokens of the input. The shell turns the error into tokens with
`into_compile_error`, and the compiler reports them as an error at that span.

This program runs shelf's generator on two inputs it refuses, and prints the message and the
tokens the error becomes:

```rust
use syn::{DeriveInput, parse_quote};

fn main() {
    let status: DeriveInput = parse_quote! {
        enum Status { Pending, Paid }
    };
    let error = shelf_codegen::from_row::expand(&status).unwrap_err();
    println!("{error}");
    println!("{}", error.to_compile_error());
    assert!(error.to_string().contains("derive postgres_types::FromSql"));

    let pair: DeriveInput = parse_quote! {
        struct Pair(i32, i32);
    };
    let error = shelf_codegen::from_row::expand(&pair).unwrap_err();
    println!("{error}");
    assert!(error.to_string().starts_with("FromRow needs named fields"));
}
```

```text
FromRow maps a row to a struct; for a Postgres enum, derive postgres_types::FromSql instead
:: core :: compile_error ! { "FromRow maps a row to a struct; for a Postgres enum, derive postgres_types::FromSql instead" }
FromRow needs named fields: each one is read from the column of that name
```

An error becomes an invocation of the standard `compile_error!` macro, whose tokens carry the
error's span. Here, outside the compiler, the span is a placeholder. In a real build it points
at the user's code, and shelf's UI tests (Chapter 4) capture what the compiler then prints. For
an enum, a tuple struct and a union:

~~~rust
// orm/shelf/tests/ui/shapes.rs (excerpt)
#[derive(FromRow)]
enum Status {
    Pending,
    Paid,
}

#[derive(FromRow)]
struct Pair(i32, i32);

#[derive(FromRow)]
union Bits {
    int: i32,
    float: f32,
}
~~~

```text
error: FromRow maps a row to a struct; for a Postgres enum, derive postgres_types::FromSql instead
 --> tests/ui/shapes.rs:5:1
  |
5 | enum Status {
  | ^^^^

error: FromRow needs named fields: each one is read from the column of that name
  --> tests/ui/shapes.rs:11:8
   |
11 | struct Pair(i32, i32);
   |        ^^^^

error: FromRow cannot be derived for a union
  --> tests/ui/shapes.rs:14:1
   |
14 | union Bits {
   | ^^^^^
```

Each error points at the token it was built from: the `enum` keyword (`data.enum_token`), the
tuple struct's name (`input.ident`), the `union` keyword. The message says what to do instead,
where there is something to do: a Postgres enum is one column, which postgres-types' derive maps
([main book Chapter 5](../../book/05-custom-types.md)).

Spans also decide where errors in the *generated* code land, and the rule is simple: tokens the
derive writes itself carry the span of the `#[derive]`, and tokens copied from the input keep
their own. A field's type is copied from the struct into `get::<#ty>`, so if that type cannot be
read from a column, the compiler's error points at the field's type in your struct, not at the
derive. Chapter 4 uses that, and makes the message read well.

### Side by side — *attaching generated behaviour to a type*

The task: give a struct a row-mapping impl. sqlx's derive generates one that is generic over
every database sqlx supports; this is its shape for a two-field struct, from sqlx 0.9's code
generator:

~~~rust
// sqlx (illustration): the shape of what #[derive(sqlx::FromRow)] generates in sqlx 0.9
#[automatically_derived]
impl<'a, R: ::sqlx::Row> ::sqlx::FromRow<'a, R> for Product
where
    &'a ::std::primitive::str: ::sqlx::ColumnIndex<R>,
    String: ::sqlx::decode::Decode<'a, R::Database>,
    String: ::sqlx::types::Type<R::Database>,
    i64: ::sqlx::decode::Decode<'a, R::Database>,
    i64: ::sqlx::types::Type<R::Database>,
{
    fn from_row(__row: &'a R) -> ::sqlx::Result<Self> {
        let sku: String = __row.try_get("sku")?;
        let price_cents: i64 = __row.try_get("price_cents")?;
        ::std::result::Result::Ok(Product { sku, price_cents })
    }
}
~~~

By hand (Chapter 1's trait) and with shelf's derive, on the same struct, in one program:

```rust
use tokio_postgres::Row;

trait FromRow: Sized {
    fn from_row(row: &Row) -> Result<Self, tokio_postgres::Error>;
}

// By hand: the impl is yours to write, read and change.
#[derive(Debug, PartialEq)]
struct Product {
    sku: String,
    price_cents: i64,
}

impl FromRow for Product {
    fn from_row(row: &Row) -> Result<Self, tokio_postgres::Error> {
        Ok(Product {
            sku: row.try_get("sku")?,
            price_cents: row.try_get("price_cents")?,
        })
    }
}

// shelf: the impl is generated next to the struct, at compile time.
#[derive(Debug, PartialEq, shelf::FromRow)]
struct ShelfProduct {
    sku: String,
    price_cents: i64,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = shelf_playground::connect().await?;
    let sql = "SELECT 'MUG-01' AS sku, 1299::int8 AS price_cents";

    let by_hand = Product::from_row(&client.query_one(sql, &[]).await?)?;
    let with_shelf: ShelfProduct = shelf::fetch_one(&client, sql, &[]).await?;
    assert_eq!(
        (by_hand.sku, by_hand.price_cents),
        (with_shelf.sku, with_shelf.price_cents)
    );
    Ok(())
}
```

| | by hand | shelf | sqlx |
|---|---|---|---|
| **Same** | An impl of a row-mapping trait, next to the struct | The same impl, written by a derive at compile time | The same impl, written by a derive at compile time |
| **Different** | You write and maintain it, and it can do anything | For tokio-postgres's `Row` only; paths are absolute and reach tokio-postgres through `::shelf::__private`; columns are found once per statement (Chapter 5) | Generic over every sqlx database (`R: Row`), with a `Decode` and a `Type` bound per field; each field is looked up by name on every row |

---

## In shelf

The finished generator is §3.4's stage program, grown. The skeleton is the same: refuse what is
not a struct with named fields, loop over the fields, collect a piece of code per field, and
paste the pieces into one `impl` with absolute paths. What the field loop collects is more,
because `FromRow` has the two-step shape Chapter 5 explains, and the attributes of Chapter 4 are
elided here:

~~~rust
// orm/shelf-codegen/src/from_row.rs (excerpt)
/// The `impl shelf::FromRow` for `input`, or every error found in it.
pub fn expand(input: &DeriveInput) -> Result<TokenStream> {
    let fields = named_fields(input)?;
    // … the attributes are parsed here, and every error is collected (Chapter 4)

    let model = input.ident.to_string();
    let mut bounds = Bounds::new(&input.generics);
    let mut column_types = Vec::new(); // the tuple `Columns`, one element per field
    let mut resolves = Vec::new(); // `resolve`'s tuple, element by element
    let mut reads = Vec::new(); // `from_row_at`'s field initializers

    for (position, (field, attrs)) in fields.iter().enumerate() {
        let ident = field.ident.as_ref().expect("named fields have identifiers");
        // A raw identifier names the column without its prefix: `r#type` reads `type`.
        let name = ident.unraw().to_string();
        let ty = &field.ty;
        let at = Index::from(position);

        // The type `resolve` checks the column against, and the expression that reads it.
        let (checked, read) = match attrs.read() {
            // … skip, flatten, json and try_from (Chapter 4)
            Read::Column => {
                bounds.add_if_generic(&[ty], parse_quote!(#ty: ::shelf::FromColumn));
                (
                    quote!(#ty),
                    quote!(::shelf::__private::get::<#ty>(row, columns.#at, #model, #name)?),
                )
            }
        };
        // … `column` is the field's name, or what rename and rename_all make of it (Chapter 4)
        column_types.push(quote!(usize));
        resolves.push(quote! {
            ::shelf::__private::column::<#checked>(columns, #model, #name, #column)?
        });
        reads.push(quote!(#ident: #read));
    }

    // With nothing to read (every field skipped, or none at all), the parameters go unused.
    let reads_any = column_types.iter().any(|t| t.to_string() != "()");
    let (row, columns) = if reads_any {
        (quote!(row), quote!(columns))
    } else {
        (quote!(_row), quote!(_columns))
    };

    let name = &input.ident;
    let generics = bounds.apply(&input.generics);
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    // Every path is absolute (`::shelf`, `::core`): the expansion lands in the user's module,
    // where `Result` or even `shelf` may mean something else.
    Ok(quote! {
        #[automatically_derived]
        impl #impl_generics ::shelf::FromRow for #name #ty_generics #where_clause {
            type Columns = ( #(#column_types,)* );

            fn resolve(
                #columns: &[::shelf::__private::tokio_postgres::Column],
            ) -> ::core::result::Result<Self::Columns, ::shelf::Error> {
                ::core::result::Result::Ok(( #(#resolves,)* ))
            }

            fn from_row_at(
                #row: &::shelf::__private::tokio_postgres::Row,
                #columns: &Self::Columns,
            ) -> ::core::result::Result<Self, ::shelf::Error> {
                ::core::result::Result::Ok(Self { #(#reads,)* })
            }
        }
    })
}

/// The struct's named fields, or an error that says why the input cannot be mapped.
pub(crate) fn named_fields(input: &DeriveInput) -> Result<&Punctuated<Field, Token![,]>> {
    match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(fields) => Ok(&fields.named),
            _ => Err(Error::new_spanned(
                &input.ident,
                "FromRow needs named fields: each one is read from the column of that name",
            )),
        },
        Data::Enum(data) => Err(Error::new_spanned(
            data.enum_token,
            "FromRow maps a row to a struct; for a Postgres enum, derive \
             postgres_types::FromSql instead",
        )),
        Data::Union(data) => Err(Error::new_spanned(
            data.union_token,
            "FromRow cannot be derived for a union",
        )),
    }
}
~~~

[orm/shelf-codegen/src/from_row.rs](../shelf-codegen/src/from_row.rs)

Three details are new since the stage program. The loop builds three lists instead of one,
because each field contributes to `Columns`, to `resolve` and to `from_row_at`. `split_for_impl`
splits the struct's generics into the three places an `impl` needs them (`impl<T>`, `Product<T>`
and the `where` clause), with the bounds Chapter 4 adds. And a struct whose fields are all
skipped would leave `row` and `columns` unused, so the generator names them `_row` and
`_columns` then, as you would in hand-written code.

## When not to write a derive

- **For a handful of types.** Chapter 1's impls are a few lines each, and need no proc-macro
  crate, and no syn, quote and proc-macro2 to compile before your own code.
- **When a blanket impl does the job.** If every type with some trait should get the behaviour,
  `impl<T: Trait> Other for T` needs no macro at all.
- **When the generated code would be hard to predict.** A derive's value is that users can
  guess its output. If they cannot, they will read the expansion every time, and a function they
  call explicitly would serve them better.

## Summary

- A proc-macro crate (`proc-macro = true`) runs inside the compiler. A derive is a function from
  the item's tokens to new tokens, which are added after the item.
- A derive receives the whole item, attributes and doc comments included, but not the
  `#[derive]` line, and after `cfg` has been evaluated.
- A proc-macro crate can export only macros, and `proc_macro`'s types do not work outside the
  compiler. So shelf keeps code generation in `shelf-codegen`, on `proc_macro2`, with
  `shelf-macros` as a three-line shell per derive.
- syn parses tokens into `DeriveInput` → `Data` → `Fields` → `Field`; quote builds tokens with
  `#value` and `#(…)*`. A `String` pastes as a literal, a `usize` with a suffix (use
  `syn::Index` for tuple fields).
- syn 3's derive types are syn 2's, except two fields of `Field`; reading `attrs`, `ident` and
  `ty` is unchanged.
- A derive's tokens resolve at the call site: write every path absolute (`::core`, `::shelf`),
  reach tokio-postgres through `::shelf::__private`, strip `r#` with `unraw`, and mark the impl
  `#[automatically_derived]`.
- See an expansion with `pretty` (stable, assertable), `cargo expand` (the compiler's own view;
  runs on stable through `RUSTC_BOOTSTRAP`), or rust-analyzer.
- Errors are `syn::Error` values with spans, turned into `compile_error!`; tokens copied from
  the input keep their spans, so errors in generated code point at the user's code.

## Exercises

1. **The column list, in Rust.** Extend §3.4's stage generator so that it also writes
   `impl Product { pub const COLUMNS: &'static str = "id, sku, tags"; }`. Compare with Chapter 2's
   Exercise 2, which built the same string with `macro_rules!`.
   *Hint:* the generator is ordinary Rust: collect the names into a `Vec<String>` and `join`
   them, then paste the result as one literal.
2. **Make it hygienic.** Rewrite §3.4's generator so that its output compiles in any module:
   absolute paths, and `r#type` reading `type`. Assume the trait is `::shelf_book::FromRow` and
   the row type is `::tokio_postgres::Row`.
   *Hint:* every bare name in the `quote!` is a name the user's module could define.
3. **An expansion test.** Write a test, as a program with an `assert_eq!`, that pins down the
   whole output of §3.4's generator for a two-field struct.
   *Hint:* compare `pretty`'s output with the expected text in a raw string literal, `r#"…"#`.

### Solutions

**1.** In a proc macro the string is computed by ordinary Rust while the macro runs, and pasted
as one literal; `macro_rules!` had to assemble it from `stringify!` and `concat!`:

```rust
use proc_macro2::TokenStream;
use quote::quote;
use syn::{Data, DeriveInput, Fields, parse_quote};

fn expand(input: &DeriveInput) -> syn::Result<TokenStream> {
    let Data::Struct(data) = &input.data else {
        return Err(syn::Error::new_spanned(
            &input.ident,
            "FromRow needs a struct",
        ));
    };
    let Fields::Named(fields) = &data.fields else {
        return Err(syn::Error::new_spanned(
            &input.ident,
            "FromRow needs named fields",
        ));
    };

    let name = &input.ident;
    let idents: Vec<_> = fields
        .named
        .iter()
        .filter_map(|f| f.ident.as_ref())
        .collect();
    let columns: Vec<String> = idents.iter().map(|ident| ident.to_string()).collect();
    let list = columns.join(", ");

    Ok(quote! {
        impl #name {
            pub const COLUMNS: &'static str = #list;
        }

        impl FromRow for #name {
            fn from_row(row: &Row) -> Result<Self, tokio_postgres::Error> {
                Ok(Self { #(#idents: row.try_get(#columns)?,)* })
            }
        }
    })
}

fn main() {
    let input: DeriveInput = parse_quote! {
        struct Product { id: i64, sku: String, tags: Option<Vec<String>> }
    };
    let code = shelf_playground::pretty(expand(&input).unwrap());
    println!("{code}");
    assert!(code.contains(r#"pub const COLUMNS: &'static str = "id, sku, tags";"#));
}
```

The repetition `#(#idents: row.try_get(#columns)?,)*` walks two lists in step, as Chapter 2's
`$field` and `$ty` did.

**2.** Every path starts at a crate, and the column name comes from `unraw`:

```rust
use proc_macro2::TokenStream;
use quote::quote;
use syn::ext::IdentExt;
use syn::{Data, DeriveInput, Fields, parse_quote};

fn expand(input: &DeriveInput) -> syn::Result<TokenStream> {
    let Data::Struct(data) = &input.data else {
        return Err(syn::Error::new_spanned(
            &input.ident,
            "FromRow needs a struct",
        ));
    };
    let Fields::Named(fields) = &data.fields else {
        return Err(syn::Error::new_spanned(
            &input.ident,
            "FromRow needs named fields",
        ));
    };

    let name = &input.ident;
    let reads = fields.named.iter().map(|field| {
        let ident = field.ident.as_ref().expect("named fields have names");
        let column = ident.unraw().to_string();
        quote! { #ident: row.try_get(#column)? }
    });

    Ok(quote! {
        #[automatically_derived]
        impl ::shelf_book::FromRow for #name {
            fn from_row(
                row: &::tokio_postgres::Row,
            ) -> ::core::result::Result<Self, ::tokio_postgres::Error> {
                ::core::result::Result::Ok(Self { #(#reads,)* })
            }
        }
    })
}

fn main() {
    let input: DeriveInput = parse_quote! {
        struct Product { sku: String, r#type: String }
    };
    let code = shelf_playground::pretty(expand(&input).unwrap());
    println!("{code}");
    assert!(code.contains("impl ::shelf_book::FromRow for Product"));
    assert!(code.contains(r#"r#type: row.try_get("type")?"#));
    assert!(code.contains("::core::result::Result::Ok(Self"));
}
```

`row` is still a bare name, and that is fine: it is a parameter the generated function declares
itself, so the user's module cannot change what it means. This version still requires the
user's crate to depend on tokio-postgres; shelf's `__private` re-export removes that last
dependency.

**3.** The test fails, and prints both versions, as soon as the generator's output changes in
any way, which is what shelf's expansion tests are for:

```rust
use proc_macro2::TokenStream;
use quote::quote;
use syn::{Data, DeriveInput, Fields, parse_quote};

fn expand(input: &DeriveInput) -> syn::Result<TokenStream> {
    let Data::Struct(data) = &input.data else {
        return Err(syn::Error::new_spanned(
            &input.ident,
            "FromRow needs a struct",
        ));
    };
    let Fields::Named(fields) = &data.fields else {
        return Err(syn::Error::new_spanned(
            &input.ident,
            "FromRow needs named fields",
        ));
    };
    let name = &input.ident;
    let reads = fields.named.iter().map(|field| {
        let ident = field.ident.as_ref().expect("named fields have names");
        let column = ident.to_string();
        quote! { #ident: row.try_get(#column)? }
    });
    Ok(quote! {
        impl FromRow for #name {
            fn from_row(row: &Row) -> Result<Self, tokio_postgres::Error> {
                Ok(Self { #(#reads,)* })
            }
        }
    })
}

const EXPECTED: &str = r#"impl FromRow for Price {
    fn from_row(row: &Row) -> Result<Self, tokio_postgres::Error> {
        Ok(Self {
            cents: row.try_get("cents")?,
            currency: row.try_get("currency")?,
        })
    }
}
"#;

fn main() {
    let input: DeriveInput = parse_quote! {
        struct Price { cents: i64, currency: String }
    };
    let actual = shelf_playground::pretty(expand(&input).unwrap());
    assert_eq!(actual, EXPECTED, "the expansion changed:\n{actual}");
}
```

shelf's version, `orm/shelf-codegen/tests/expand.rs`, keeps the structs and their expected
expansions in files, and rewrites the expected files when run with `SHELF_BLESS=1`, so that a
deliberate change is accepted by reading a diff rather than by editing strings.

---

Previous: [Chapter 2 — Declarative macros](02-declarative-macros.md) · Next: [Chapter 4 — Attributes, errors and generics](04-attributes.md)
