//! The tests that matter most: correctness when many requests arrive at once.

mod common;

use axum::http::StatusCode;
use common::TestDb;
use futures_util::future::join_all;
use serde_json::json;

/// 20 chairs, 50 customers each wanting one, all at the same moment: exactly 20 orders.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn fifty_customers_twenty_chairs() {
    let db = TestDb::new().await;
    let app = db.app(16);
    app.store("mitte").await;
    app.product("CHAIR-01", 4999).await;
    app.stock("mitte", "CHAIR-01", 20).await;

    let attempts = (0..50).map(|_| {
        let app = app.clone();
        tokio::spawn(async move { app.order("mitte", &[("CHAIR-01", 1)]).await.status })
    });
    let statuses: Vec<StatusCode> = join_all(attempts)
        .await
        .into_iter()
        .map(Result::unwrap)
        .collect();

    let created = statuses
        .iter()
        .filter(|s| **s == StatusCode::CREATED)
        .count();
    let refused = statuses
        .iter()
        .filter(|s| **s == StatusCode::CONFLICT)
        .count();
    assert_eq!((created, refused), (20, 30), "{statuses:?}");
    assert_eq!(app.level("mitte", "CHAIR-01").await, (20, 20, 0));
}

/// Orders for two products, listing them in either order: without a fixed lock order these
/// deadlock; with it, they queue. Either way no request may fail with anything but 409.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn orders_sharing_products_do_not_deadlock() {
    let db = TestDb::new().await;
    let app = db.app(16);
    app.store("mitte").await;
    app.product("CHAIR-01", 4999).await;
    app.product("LAMP-01", 2999).await;
    app.stock("mitte", "CHAIR-01", 15).await;
    app.stock("mitte", "LAMP-01", 15).await;

    let attempts = (0..40).map(|i| {
        let app = app.clone();
        tokio::spawn(async move {
            let lines: &[(&str, i32)] = if i % 2 == 0 {
                &[("CHAIR-01", 1), ("LAMP-01", 1)]
            } else {
                &[("LAMP-01", 1), ("CHAIR-01", 1)]
            };
            app.order("mitte", lines).await.status
        })
    });
    let statuses: Vec<StatusCode> = join_all(attempts)
        .await
        .into_iter()
        .map(Result::unwrap)
        .collect();
    let created = statuses
        .iter()
        .filter(|s| **s == StatusCode::CREATED)
        .count();
    assert_eq!(created, 15, "{statuses:?}");
    assert!(
        statuses
            .iter()
            .all(|s| *s == StatusCode::CREATED || *s == StatusCode::CONFLICT),
        "{statuses:?}"
    );
    assert_eq!(app.level("mitte", "CHAIR-01").await, (15, 15, 0));
    assert_eq!(app.level("mitte", "LAMP-01").await, (15, 15, 0));
}

/// Transfers back and forth between two stores, all at once, under SERIALIZABLE: some
/// collide, PostgreSQL aborts them, `in_transaction` runs them again, and every unit is
/// accounted for at the end.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn concurrent_transfers_lose_nothing() {
    let db = TestDb::new().await;
    let app = db.app(16);
    app.store("mitte").await;
    app.store("spandau").await;
    app.product("CHAIR-01", 4999).await;
    app.stock("mitte", "CHAIR-01", 50).await;
    app.stock("spandau", "CHAIR-01", 50).await;

    let transfers = (0..40).map(|i| {
        let app = app.clone();
        tokio::spawn(async move {
            let (from, to) = if i % 2 == 0 {
                ("mitte", "spandau")
            } else {
                ("spandau", "mitte")
            };
            app.post(
                "/stock/transfers",
                json!({ "from": from, "to": to, "sku": "CHAIR-01", "quantity": 1 }),
            )
            .await
        })
    });
    for reply in join_all(transfers).await {
        let reply = reply.unwrap();
        assert_eq!(reply.status, StatusCode::OK, "{}", reply.text);
    }
    let (mitte, _, _) = app.level("mitte", "CHAIR-01").await;
    let (spandau, _, _) = app.level("spandau", "CHAIR-01").await;
    assert_eq!((mitte, spandau), (50, 50), "20 each way");

    let client = db.client().await;
    let ledger: i64 = client
        .query_one("SELECT sum(delta) FROM stock_movements", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(ledger, 100);
}
