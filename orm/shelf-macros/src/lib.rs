//! shelf's derive macros. Each one parses its input and hands it to `shelf-codegen`, which
//! does the work; that crate's documentation says why the two are separate.
//!
//! Use them through `shelf`, which re-exports them (`#[derive(shelf::FromRow)]`): the generated
//! code names `::shelf`, so a crate that derives needs `shelf` as a dependency anyway. The
//! attributes they read are documented on the re-exports.

use proc_macro::TokenStream;
use syn::{DeriveInput, parse_macro_input};

/// Implements `shelf::FromRow`. `attributes(shelf)` declares `#[shelf(…)]` as this derive's
/// helper attribute: without it, the compiler would reject `#[shelf(…)]` on the struct and
/// its fields before the derive ever saw them.
#[proc_macro_derive(FromRow, attributes(shelf))]
pub fn derive_from_row(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    shelf_codegen::from_row::expand(&input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}
