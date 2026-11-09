//! stockroom's oversell and deadlock tests, against this crate's router: the reservation is
//! still stockroom's guarded `UPDATE`, now inside a function whose reads and inserts go
//! through shelf, so these check that nothing around it changed what it guarantees.

mod common;

use axum::http::StatusCode;
use common::{TestDb, shelf_app};
use futures_util::future::join_all;

/// 20 chairs, 50 customers each wanting one, all at the same moment: exactly 20 orders.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn fifty_customers_twenty_chairs() {
    let db = TestDb::new().await;
    let app = shelf_app(&db, 16);
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

/// Orders for two products, listing them in either order: they queue, and never deadlock,
/// because every order locks its stock rows in product-id order.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn orders_sharing_products_do_not_deadlock() {
    let db = TestDb::new().await;
    let app = shelf_app(&db, 16);
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

/// Patches without `If-Match`, all at once: each one is applied, none is refused as stale,
/// although each shelf patch checks a version (the retry in `repo::products::update`).
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn concurrent_patches_without_if_match_all_apply() {
    let db = TestDb::new().await;
    let app = shelf_app(&db, 16);
    app.product("CHAIR-01", 100).await;

    let patches = (0..30).map(|i| {
        let app = app.clone();
        tokio::spawn(async move {
            let body = serde_json::json!({ "name": format!("Chair {i}") });
            app.json(axum::http::Method::PATCH, "/products/CHAIR-01", &[], body)
                .await
                .status
        })
    });
    for status in join_all(patches).await {
        assert_eq!(status.unwrap(), StatusCode::OK);
    }
    let chair = app.get("/products/CHAIR-01").await;
    assert_eq!(chair.body["version"], 31, "one version per patch");
}
