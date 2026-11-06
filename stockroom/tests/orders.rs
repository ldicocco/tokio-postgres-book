mod common;

use axum::http::StatusCode;
use common::{TestApp, TestDb};
use serde_json::{Value, json};

async fn shop(db: &TestDb) -> TestApp {
    let app = db.app(4);
    app.store("mitte").await;
    app.product("CHAIR-01", 4999).await;
    app.product("LAMP-01", 2999).await;
    app.stock("mitte", "CHAIR-01", 5).await;
    app.stock("mitte", "LAMP-01", 2).await;
    app
}

async fn transition(app: &TestApp, id: &str, to: &str, if_match: Option<&str>) -> common::Reply {
    let headers: Vec<(&str, &str)> = if_match.map(|v| ("if-match", v)).into_iter().collect();
    app.json(
        axum::http::Method::POST,
        &format!("/orders/{id}/transition"),
        &headers,
        json!({ "to": to }),
    )
    .await
}

#[tokio::test]
async fn placing_an_order_prices_it_and_reserves_its_stock() {
    let db = TestDb::new().await;
    let app = shop(&db).await;

    // The same SKU twice is one line of 3.
    let placed = app
        .order("mitte", &[("CHAIR-01", 2), ("LAMP-01", 1), ("CHAIR-01", 1)])
        .await;
    assert_eq!(placed.status, StatusCode::CREATED, "{}", placed.text);
    assert_eq!(placed.body["status"], "pending");
    assert_eq!(placed.body["totalCents"], 3 * 4999 + 2999);
    assert_eq!(
        placed.body["lines"],
        json!([
            { "sku": "CHAIR-01", "quantity": 3, "unitPriceCents": 4999 },
            { "sku": "LAMP-01", "quantity": 1, "unitPriceCents": 2999 }
        ])
    );
    assert_eq!(app.level("mitte", "CHAIR-01").await, (5, 3, 2));

    let id = placed.body["id"].as_str().unwrap();
    let read = app.get(&format!("/orders/{id}")).await;
    assert_eq!(
        read.body["customer"],
        json!({ "name": "Ada", "email": "ada@example.com" })
    );
    assert_eq!(read.headers["etag"], "\"1\"");
}

#[tokio::test]
async fn an_order_for_more_than_is_available_reserves_nothing() {
    let db = TestDb::new().await;
    let app = shop(&db).await;
    let refused = app.order("mitte", &[("CHAIR-01", 1), ("LAMP-01", 3)]).await;
    assert_eq!(refused.status, StatusCode::CONFLICT);
    assert_eq!(
        refused.body,
        json!({ "error": "insufficient_stock", "sku": "LAMP-01", "requested": 3, "available": 2,
                "message": "not enough stock of LAMP-01: 3 requested, 2 available" })
    );
    // The chair's reservation was rolled back with the rest.
    assert_eq!(app.level("mitte", "CHAIR-01").await, (5, 0, 5));

    assert_eq!(
        app.order("mitte", &[("NOPE-01", 1)]).await.status,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        app.order("nowhere", &[("CHAIR-01", 1)]).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.order("mitte", &[("CHAIR-01", 0)]).await.status,
        StatusCode::UNPROCESSABLE_ENTITY
    );
}

#[tokio::test]
async fn an_order_moves_through_its_states_and_stock_follows() {
    let db = TestDb::new().await;
    let app = shop(&db).await;
    let placed = app.order("mitte", &[("CHAIR-01", 2)]).await;
    let id = placed.body["id"].as_str().unwrap().to_owned();

    let skipped = transition(&app, &id, "ready", None).await;
    assert_eq!(skipped.status, StatusCode::CONFLICT);
    assert_eq!(skipped.body["error"], "invalid_transition");

    let paid = transition(&app, &id, "paid", Some("\"1\"")).await;
    assert_eq!(paid.status, StatusCode::OK, "{}", paid.text);
    assert_eq!(paid.body["previous"], "pending"); // RETURNING old.status
    assert_eq!(paid.body["order"]["version"], 2);

    let stale = transition(&app, &id, "ready", Some("\"1\"")).await;
    assert_eq!(stale.status, StatusCode::PRECONDITION_FAILED);

    assert_eq!(
        transition(&app, &id, "ready", Some("\"2\"")).await.status,
        StatusCode::OK
    );
    assert_eq!(app.level("mitte", "CHAIR-01").await, (5, 2, 3));
    assert_eq!(
        transition(&app, &id, "collected", None).await.status,
        StatusCode::OK
    );
    // Collected: out of the store and out of the reservation.
    assert_eq!(app.level("mitte", "CHAIR-01").await, (3, 0, 3));
    assert_eq!(
        transition(&app, &id, "cancelled", None).await.status,
        StatusCode::CONFLICT
    );

    let client = db.client().await;
    let ledger: i64 = client
        .query_one("SELECT sum(delta) FROM stock_movements m JOIN products p ON p.id = m.product_id WHERE p.sku = 'CHAIR-01'", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(ledger, 3, "+5 delivered, -2 collected");
}

#[tokio::test]
async fn cancelling_releases_the_reservation() {
    let db = TestDb::new().await;
    let app = shop(&db).await;
    let placed = app.order("mitte", &[("LAMP-01", 2)]).await;
    let id = placed.body["id"].as_str().unwrap().to_owned();
    assert_eq!(app.level("mitte", "LAMP-01").await, (2, 2, 0));
    assert_eq!(
        transition(&app, &id, "paid", None).await.status,
        StatusCode::OK
    );
    let cancelled = transition(&app, &id, "cancelled", None).await;
    assert_eq!(cancelled.body["previous"], "paid");
    assert_eq!(app.level("mitte", "LAMP-01").await, (2, 0, 2));
}

#[tokio::test]
async fn unknown_orders_are_not_found() {
    let db = TestDb::new().await;
    let app = db.app(2);
    let id = "01a1b1ee-0000-7000-8000-000000000000";
    assert_eq!(
        app.get(&format!("/orders/{id}")).await.status,
        StatusCode::NOT_FOUND
    );
    let reply = transition(&app, id, "paid", None).await;
    assert_eq!(reply.status, StatusCode::NOT_FOUND);
    assert_eq!(reply.body["error"], Value::from("not_found"));
}
