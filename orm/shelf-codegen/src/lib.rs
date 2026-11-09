//! What shelf's derive macros generate, as an ordinary library.
//!
//! A proc-macro crate can export nothing but macros, so the code that writes code lives here
//! instead: functions from a parsed [`syn::DeriveInput`] to a [`proc_macro2::TokenStream`].
//! `shelf-macros` is a thin shell around them. Being a plain library, this crate can be unit
//! tested, and the book's runnable examples call it directly and print what it generates
//! (orm/PLAN.md §4.1).
//!
//! - The module [`from_row`] generates `#[derive(FromRow)]`, [`table`] `#[derive(Table)]`,
//!   [`insert`] `#[derive(Insert)]` and [`patch`] `#[derive(Patch)]`.
//! - `attrs` parses the `#[shelf(…)]` attributes, for every derive, and decides which derive
//!   reports which mistake.
//! - `case` holds the conventions of `rename_all`.
//! - `columns` writes the query builder's column constants, which `Table` adds to a model.
//! - `generics` adds the bounds a generic struct's fields need.
//! - `relations` writes the methods of `has_many` and `belongs_to`, which `Table` adds.
//! - `sql` quotes identifiers and writes the pieces of SQL the derives share.
//!
//! `tests/expand/` pins down what each derive generates (see `tests/expand.rs`).

mod attrs;
mod case;
mod columns;
pub mod from_row;
mod generics;
pub mod insert;
pub mod patch;
mod relations;
mod sql;
pub mod table;
