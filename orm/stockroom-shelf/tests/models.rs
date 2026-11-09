//! Every model against stockroom's schema, as `serve` checks it at startup, and what it reports
//! when the schema has moved on without the models.

mod common;

use common::TestDb;
use stockroom_shelf::repo;

#[tokio::test]
async fn every_model_matches_stockroom_s_schema() {
    let db = TestDb::new().await;
    let pool = db.pool(1);
    stockroom_shelf::app::verify(&pool).await.unwrap();
}

#[tokio::test]
async fn a_schema_that_moved_on_is_reported_model_by_model() {
    let db = TestDb::new().await;
    let client = db.client().await;
    client
        .batch_execute(
            "ALTER TABLE stores RENAME COLUMN name TO title;
             ALTER TABLE products ALTER COLUMN tags DROP NOT NULL;
             ALTER TABLE order_lines ALTER COLUMN unit_price_cents TYPE numeric;",
        )
        .await
        .unwrap();
    let errors = repo::verify_models(&client).await.unwrap_err();
    let messages: Vec<String> = errors.iter().map(ToString::to_string).collect();
    assert_eq!(
        messages,
        [
            "Store does not match its table: name: the result has no column \"name\"",
            "Product does not match its table: tags: column \"tags\" can be NULL in the table; \
             read it into an Option, or make it NOT NULL",
            "LineRow does not match its table: unit_price_cents: column \"unit_price_cents\" has \
             type numeric, which cannot be read into Cents",
        ]
    );
    let pool = db.pool(1);
    let startup = stockroom_shelf::app::verify(&pool).await.unwrap_err();
    assert!(
        startup
            .to_string()
            .starts_with("the models do not match the database:\n  Store does not match"),
        "{startup}"
    );
}
