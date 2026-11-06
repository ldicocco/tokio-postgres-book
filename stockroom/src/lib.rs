//! stockroom: click-and-collect for a small retail chain, the capstone of the book.
//!
//! `main.rs` is the command line; everything else is here, so that the tests (and the book)
//! can reach it. The map from tokio-postgres features to files is in Chapter 18.

// `db::MakeTls` is `NoTls`, which is `Copy`, without the `tls` feature, and rustls's
// connector, which is only `Clone`, with it. The code clones it so that both builds compile;
// without the feature those clones are copies, which is all this lint objects to.
#![allow(clippy::clone_on_copy)]

pub mod app;
pub mod cli;
pub mod config;
pub mod db;
pub mod domain;
pub mod error;
pub mod http;
pub mod notify;
pub mod repo;
pub mod worker;

pub use error::AppError;
