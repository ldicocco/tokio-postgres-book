//! stockroom-shelf: part of stockroom's data layer rewritten on shelf, behind the same HTTP API.
//! The capstone of the shelf book (`orm/book/14-capstone.md`).
//!
//! Stores, products and orders are read and written through shelf models ([`repo`]), and served
//! by this crate's handlers ([`http`]). Everything else is the `stockroom` crate's own, reused
//! unchanged: the schema and its migrations, the domain types, `AppError`, `in_transaction`,
//! the `Db` extractor, and the handlers of every route that is not ported, which the router
//! falls back to. So the service answers the whole of stockroom's API, and the ported routes
//! answer as stockroom does (the tests run one suite against both routers).

// `stockroom::db::MakeTls` is `NoTls`, which is `Copy`, without the `tls` feature, and rustls's
// connector, which is only `Clone`, with it; the code clones it so that both builds compile, as
// stockroom's does.
#![allow(clippy::clone_on_copy)]

pub mod app;
pub mod error;
pub mod http;
pub mod repo;

pub use error::app_error;
