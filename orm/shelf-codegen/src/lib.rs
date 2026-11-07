//! What shelf's derive macros generate, as an ordinary library.
//!
//! A proc-macro crate can export nothing but macros, so the code that writes code lives here
//! instead: functions from a parsed [`syn::DeriveInput`] to a [`proc_macro2::TokenStream`].
//! `shelf-macros` is a thin shell around them. Being a plain library, this crate can be unit
//! tested, and the book's runnable examples call it directly and print what it generates
//! (orm/PLAN.md §4.1).
//!
//! - The module [`from_row`] generates `#[derive(FromRow)]`.
//! - `attrs` parses the `#[shelf(…)]` attributes, once for every derive.
//! - `case` holds the conventions of `rename_all`.
//! - `generics` adds the bounds a generic struct's fields need.
//!
//! `tests/expand/` pins down what each derive generates (see `tests/expand.rs`).

mod attrs;
mod case;
pub mod from_row;
mod generics;
