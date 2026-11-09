//! `Executor`: shelf's calls on each of the four types it is implemented for, deadpool's
//! statement cache, and futures that are `Send` where a server needs them to be.
#![cfg(feature = "deadpool")]

mod common;

use common::TestDb;
use shelf::{Executor, FromRow, fetch_all, fetch_one};
use tokio_postgres::IsolationLevel;

const PRODUCTS: &str = "
    CREATE TABLE products (sku text PRIMARY KEY, price_cents bigint NOT NULL);
    INSERT INTO products VALUES ('MUG-01', 1299), ('TEE-02', 2499);";

const ALL: &str = "SELECT sku, price_cents FROM products ORDER BY sku";

#[derive(Debug, PartialEq, FromRow)]
struct Product {
    sku: String,
    price_cents: i64,
}

/// Generic code over any executor, as a repository function would be.
async fn skus(db: &impl Executor) -> Vec<String> {
    let products: Vec<Product> = fetch_all(db, ALL, &[]).await.unwrap();
    products.into_iter().map(|p| p.sku).collect()
}

#[tokio::test]
async fn the_same_call_runs_on_all_four_executors() {
    let db = TestDb::new().await;
    let mut client = db.client_with(PRODUCTS).await;
    let expected = vec!["MUG-01".to_owned(), "TEE-02".to_owned()];

    assert_eq!(skus(&client).await, expected);
    let tx = client.transaction().await.unwrap();
    assert_eq!(skus(&tx).await, expected);
    tx.commit().await.unwrap();

    let pool = db.pool(1);
    let mut object = pool.get().await.unwrap();
    assert_eq!(skus(&object).await, expected);
    let tx = object.transaction().await.unwrap();
    assert_eq!(skus(&tx).await, expected);
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn a_pooled_connection_prepares_each_statement_once() {
    let db = TestDb::new().await;
    db.client_with(PRODUCTS).await;
    let pool = db.pool(1);
    let mut object = pool.get().await.unwrap();
    assert_eq!(object.statement_cache.size(), 0);

    skus(&object).await;
    skus(&object).await;
    assert_eq!(object.statement_cache.size(), 1);

    // A transaction on the connection shares its cache.
    let tx = object.transaction().await.unwrap();
    skus(&tx).await;
    assert_eq!(tx.statement_cache.size(), 1);
    tx.commit().await.unwrap();

    // A plain Client has no cache: it prepares every time, and leaves nothing behind.
    let client = db.client().await;
    skus(&client).await;
    let row = client
        .query_one("SELECT count(*) FROM pg_prepared_statements", &[])
        .await
        .unwrap();
    // The count's own statement is the only one on the session.
    assert_eq!(row.get::<_, i64>(0), 1);
}

/// The main book's retry helper, reduced: the closure owns what it uses (`async move`).
async fn in_transaction<T>(
    client: &mut deadpool_postgres::Client,
    mut work: impl AsyncFnMut(&deadpool_postgres::Transaction<'_>) -> Result<T, shelf::Error>,
) -> Result<T, shelf::Error> {
    let tx = client
        .build_transaction()
        .isolation_level(IsolationLevel::Serializable)
        .start()
        .await?;
    let value = work(&tx).await?;
    tx.commit().await?;
    Ok(value)
}

#[tokio::test]
async fn shelf_calls_are_send_in_a_spawned_task_and_a_retry_helper() {
    let db = TestDb::new().await;
    db.client_with(PRODUCTS).await;
    let pool = db.pool(2);

    // `tokio::spawn` requires a `Send` future, as an axum handler does.
    let task = tokio::spawn(async move {
        let mut object = pool.get().await.unwrap();
        let sku = "TEE-02".to_owned();
        let one: Product = fetch_one(
            &object,
            "SELECT sku, price_cents FROM products WHERE sku = $1",
            &[&sku],
        )
        .await
        .unwrap();
        let all = in_transaction(&mut object, async move |tx| {
            let products: Vec<Product> = fetch_all(tx, ALL, &[]).await?;
            Ok(products.len())
        })
        .await
        .unwrap();
        (one.price_cents, all)
    });
    assert_eq!(task.await.unwrap(), (2499, 2));
}
