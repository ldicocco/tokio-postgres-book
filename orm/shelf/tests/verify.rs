//! `shelf::verify` against PostgreSQL: a model that fits its table, every kind of mismatch
//! reported at once, views and generated columns, and a table changed under a pooled connection.

mod common;

use common::TestDb;
use serde::{Deserialize, Serialize};
use shelf::{Error, FromRow, Mapping, Table, verify};

const PRODUCTS: &str = "
    CREATE TABLE products (
        id bigint PRIMARY KEY,
        sku text NOT NULL,
        price_cents bigint NOT NULL,
        stock integer NOT NULL,
        note text,
        attributes jsonb,
        doubled bigint GENERATED ALWAYS AS (price_cents * 2) VIRTUAL
    );";

#[derive(Debug, Deserialize, Serialize)]
struct Attributes {
    #[allow(dead_code)]
    color: String,
}

#[derive(Debug, FromRow, Table)]
#[shelf(table = "products")]
#[allow(dead_code)]
struct Product {
    #[shelf(primary_key)]
    id: i64,
    sku: String,
    price_cents: i64,
    #[shelf(try_from = i32)]
    stock: u32,
    note: Option<String>,
    #[shelf(json)]
    attributes: Option<Attributes>,
    doubled: Option<i64>,
    #[shelf(skip)]
    selected: bool,
}

#[tokio::test]
async fn a_model_that_fits_its_table_verifies() {
    let db = TestDb::new().await;
    let client = db.client_with(PRODUCTS).await;
    verify::<Product>(&client).await.unwrap();
}

/// The same table, read wrongly in three ways.
#[derive(Debug, FromRow, Table)]
#[shelf(table = "products")]
#[allow(dead_code)]
struct Drifted {
    #[shelf(primary_key)]
    id: i64,
    #[shelf(rename = "sku_code")]
    sku: String,
    price_cents: i32,
    note: String,
    doubled: i64,
}

#[tokio::test]
async fn every_mismatch_is_reported_at_once() {
    let db = TestDb::new().await;
    let client = db.client_with(PRODUCTS).await;
    let error = verify::<Drifted>(&client).await.unwrap_err();
    let Error::Schema { model, mismatches } = &error else {
        panic!("{error}")
    };
    assert_eq!(*model, "Drifted");
    let fields: Vec<&str> = mismatches.iter().map(|m| m.field).collect();
    assert_eq!(fields, ["sku", "price_cents", "note", "doubled"]); // in field order
    assert!(matches!(
        mismatches[0].problem,
        Mapping::MissingColumn { .. }
    ));
    assert!(matches!(mismatches[1].problem, Mapping::WrongType { .. }));
    // NULLs come from the catalog: no row is read. A generated column is nullable unless
    // declared NOT NULL.
    assert!(matches!(mismatches[2].problem, Mapping::Nullable { .. }));
    assert!(matches!(mismatches[3].problem, Mapping::Nullable { .. }));
    assert_eq!(
        error.to_string(),
        "Drifted does not match its table: \
         sku: the result has no column \"sku_code\"; \
         price_cents: column \"price_cents\" has type int8, which cannot be read into i32; \
         note: column \"note\" can be NULL in the table; read it into an Option, or make it NOT NULL; \
         doubled: column \"doubled\" can be NULL in the table; read it into an Option, or make it NOT NULL"
    );
}

#[derive(Debug, FromRow, Table)]
#[shelf(table = "product_view")]
#[allow(dead_code)]
struct ProductView {
    #[shelf(primary_key)]
    id: i64,
    sku: String,
    twice: i32,
}

#[tokio::test]
async fn a_views_columns_are_checked_for_type_but_not_for_nulls() {
    let db = TestDb::new().await;
    let client = db
        .client_with(&format!(
            "{PRODUCTS} CREATE VIEW product_view AS \
             SELECT id, sku, note, price_cents * 2 AS twice FROM products;"
        ))
        .await;
    let error = verify::<ProductView>(&client).await.unwrap_err();
    let Error::Schema { mismatches, .. } = &error else {
        panic!("{error}")
    };
    // `twice` is a bigint; a view's `id` and `sku` say nothing about NULLs, so they pass.
    assert_eq!(mismatches.len(), 1, "{error}");
    assert_eq!(mismatches[0].field, "twice");
}

#[tokio::test]
async fn a_missing_table_is_the_servers_error() {
    let db = TestDb::new().await;
    let client = db.client().await;
    let Err(Error::Db(error)) = verify::<Product>(&client).await else {
        panic!("expected a database error")
    };
    assert_eq!(
        error.code(),
        Some(&tokio_postgres::error::SqlState::UNDEFINED_TABLE)
    );
}

#[tokio::test]
async fn verify_sees_a_migration_that_a_cached_statement_would_hide() {
    let db = TestDb::new().await;
    db.client_with(PRODUCTS).await;
    let pool = db.pool(1);
    let client = pool.get().await.unwrap();
    verify::<Product>(&client).await.unwrap();
    // A model read once, so its SELECT is in the connection's statement cache.
    Product::all(&client).await.unwrap();
    client
        .batch_execute("ALTER TABLE products ALTER COLUMN sku DROP NOT NULL, DROP COLUMN note")
        .await
        .unwrap();
    let error = verify::<Product>(&client).await.unwrap_err();
    let Error::Schema { mismatches, .. } = &error else {
        panic!("{error}")
    };
    let fields: Vec<&str> = mismatches.iter().map(|m| m.field).collect();
    assert_eq!(fields, ["sku", "note"], "{error}");
}
