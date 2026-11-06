mod common;

use std::time::{Duration, Instant};

use axum::http::{Method, StatusCode};
use common::TestDb;
use serde_json::json;
use stockroom::http::{AppState, RouterOptions};
use stockroom::notify::Hub;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn the_dashboard_adds_up() {
    let db = TestDb::new().await;
    let app = db.app(4);
    app.store("mitte").await;
    app.product("CHAIR-01", 5000).await;
    app.product("LAMP-01", 2000).await;
    app.stock("mitte", "CHAIR-01", 10).await;
    app.stock("mitte", "LAMP-01", 3).await;
    app.order("mitte", &[("CHAIR-01", 1)]).await;
    app.order("mitte", &[("CHAIR-01", 1), ("LAMP-01", 1)]).await;

    let dashboard = app.get("/reports/dashboard").await;
    assert_eq!(dashboard.status, StatusCode::OK, "{}", dashboard.text);
    let body = dashboard.body;
    assert_eq!(
        (body["products"].as_i64(), body["stores"].as_i64()),
        (Some(2), Some(1))
    );
    assert_eq!(body["openOrders"], 2);
    assert_eq!(body["ordersToday"], 2);
    assert_eq!(body["revenueTodayCents"], 12_000);
    assert_eq!(
        body["averageOrderCents"], "6000.00",
        "numeric, exactly, as a string"
    );
    assert_eq!(
        body["lowStock"],
        json!([{ "store": "mitte", "sku": "LAMP-01", "available": 2 }])
    );

    let sales = app
        .get("/reports/sales?from=2000-01-01&to=2100-01-01")
        .await;
    assert_eq!(sales.body[0]["orders"], 2);
}

async fn sleeping_queries(db: &TestDb) -> i64 {
    let client = db.client().await;
    client
        .query_one(
            "SELECT count(*) FROM pg_stat_activity
             WHERE datname = current_database() AND state = 'active' AND query LIKE 'SELECT pg_sleep%'",
            &[],
        )
        .await
        .unwrap()
        .get(0)
}

/// The request times out after 300 ms; the query would run for 5 s. Dropping the handler's
/// future must also stop the query on the server.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_abandoned_report_is_cancelled_on_the_server() {
    let db = TestDb::new().await;
    let app = db.app_with(
        2,
        RouterOptions {
            request_timeout: Duration::from_millis(300),
            ..Default::default()
        },
    );

    let started = Instant::now();
    let reply = app
        .get("/reports/sales?from=2026-01-01&to=2026-12-31&delayMs=5000")
        .await;
    assert_eq!(reply.status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(started.elapsed() < Duration::from_secs(2));

    let deadline = Instant::now() + Duration::from_secs(3);
    while sleeping_queries(&db).await > 0 {
        assert!(
            Instant::now() < deadline,
            "the query is still running on the server"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// The server's own `statement_timeout` ends a query nobody cancelled: 503 with `timeout`.
#[tokio::test]
async fn statement_timeout_is_a_503() {
    let db = TestDb::new().await;
    let mut config = db.config.clone();
    config.options("-c statement_timeout=200");
    let state = AppState {
        pool: stockroom::db::pool::create(config, common::tls(), 2).unwrap(),
        hub: Hub::new(16),
        tls: common::tls(),
        shutdown: CancellationToken::new(),
    };
    let app = common::TestApp {
        router: stockroom::http::router(state.clone(), RouterOptions::default()),
        state,
    };
    let reply = app
        .json(
            Method::GET,
            "/reports/sales?from=2026-01-01&to=2026-12-31&delayMs=2000",
            &[],
            json!(null),
        )
        .await;
    assert_eq!(reply.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(reply.body["error"], "timeout");
}
