//! What a model knows about each column it reads: the query builder's constant, named after
//! the field in capitals (`Product::PRICE_CENTS: shelf::query::Column<Product, i64>`), and the
//! description `shelf::verify` checks (`Table::FIELDS`).

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::ext::IdentExt;
use syn::{Field, Type};

use crate::attrs::{FieldAttrs, Read};
use crate::sql::quote_ident;

/// The type a column's values have on the Rust side: what a condition on it takes. That is the
/// field's type, except for a `try_from` field (the type read from the column) and a `json`
/// field (JSON, whatever struct it is read into).
pub fn value_type(field: &Field, attrs: &FieldAttrs) -> TokenStream {
    match attrs.read() {
        Read::TryFrom(source) => quote!(#source),
        Read::Json => quote!(::shelf::__private::serde_json::Value),
        Read::Column | Read::Flatten | Read::Skip => {
            let ty: &Type = &field.ty;
            quote!(#ty)
        }
    }
}

/// `pub const PRICE_CENTS: Column<Self, i64> = Column::new("\"price_cents\"");`, with the
/// field's visibility: a column is as visible as the field that reads it.
pub fn constant(field: &Field, attrs: &FieldAttrs, column: &str) -> TokenStream {
    let ident = field.ident.as_ref().expect("named fields have identifiers");
    let name = format_ident!(
        "{}",
        ident.unraw().to_string().to_uppercase(),
        span = ident.span()
    );
    let vis = &field.vis;
    let ty = value_type(field, attrs);
    let quoted = quote_ident(column);
    let doc = format!(" The column `{quoted}`, for the query builder.");
    quote! {
        #[doc = #doc]
        #vis const #name: ::shelf::query::Column<Self, #ty> = ::shelf::query::Column::new(#quoted);
    }
}

/// `Field::new::<T>("field", "column")`: the field's column and the type it is read through,
/// the type `FromRow`'s `resolve` checks (`Json<T>` for `json`, the source type of `try_from`).
pub fn field(field: &Field, attrs: &FieldAttrs, column: &str) -> TokenStream {
    let ident = field.ident.as_ref().expect("named fields have identifiers");
    let name = ident.unraw().to_string();
    let ty = &field.ty;
    let read = match attrs.read() {
        Read::Json => quote!(::shelf::__private::Json<#ty>),
        Read::TryFrom(source) => quote!(#source),
        Read::Column | Read::Flatten | Read::Skip => quote!(#ty),
    };
    quote!(::shelf::__private::Field::new::<#read>(#name, #column))
}
