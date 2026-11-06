mod common;

use std::sync::Mutex;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Method, StatusCode};
use common::{TestApp, TestDb};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use stockroom::domain::OrderStatus;
use stockroom::notify::{HubEvent, listener};
use stockroom::repo;
use stockroom::worker::outbox::{self, Deliver};
use tokio::sync::broadcast;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

async fn shop(db: &TestDb) -> TestApp {
    let app = db.app(4);
    app.store("mitte").await;
    app.product("CHAIR-01", 4999).await;
    app.stock("mitte", "CHAIR-01", 5).await;
    app
}

/// Starts the listener on the app's hub, and waits until it is listening.
async fn listen(db: &TestDb, app: &TestApp) -> (broadcast::Receiver<HubEvent>, CancellationToken) {
    let mut events = app.state.hub.subscribe();
    let shutdown = CancellationToken::new();
    tokio::spawn(listener::run(
        db.config.clone(),
        common::tls(),
        app.state.hub.clone(),
        shutdown.clone(),
    ));
    // The listener publishes `Resync` once its LISTEN has succeeded.
    let first = timeout(Duration::from_secs(5), events.recv())
        .await
        .expect("the listener connects")
        .unwrap();
    assert!(matches!(first, HubEvent::Resync));
    (events, shutdown)
}

async fn next_order_event(
    events: &mut broadcast::Receiver<HubEvent>,
) -> stockroom::notify::OrderEvent {
    loop {
        match timeout(Duration::from_secs(5), events.recv())
            .await
            .expect("an event in time")
            .unwrap()
        {
            HubEvent::Order(event) => return event,
            HubEvent::Resync => continue,
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn committed_changes_arrive_as_events_and_rolled_back_ones_do_not() {
    let db = TestDb::new().await;
    let app = shop(&db).await;
    let (mut events, shutdown) = listen(&db, &app).await;

    let placed = app.order("mitte", &[("CHAIR-01", 1)]).await;
    let id = placed.body["id"].as_str().unwrap();
    let event = next_order_event(&mut events).await;
    assert_eq!(
        (
            event.order_id.to_string().as_str(),
            event.status,
            event.previous
        ),
        (id, OrderStatus::Pending, None)
    );

    // An order that fails is rolled back, trigger and NOTIFY included: no event.
    assert_eq!(
        app.order("mitte", &[("CHAIR-01", 99)]).await.status,
        StatusCode::CONFLICT
    );
    app.json(
        Method::POST,
        &format!("/orders/{id}/transition"),
        &[],
        json!({ "to": "paid" }),
    )
    .await;
    let event = next_order_event(&mut events).await;
    assert_eq!(
        (event.status, event.previous, event.version),
        (OrderStatus::Paid, Some(OrderStatus::Pending), 2)
    );
    shutdown.cancel();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_listener_reconnects_and_asks_for_a_resync() {
    let db = TestDb::new().await;
    let app = shop(&db).await;
    let (mut events, shutdown) = listen(&db, &app).await;

    // Kill the listener's session from the server side.
    let admin = db.client().await;
    let killed: bool = admin
        .query_one(
            "SELECT pg_terminate_backend(pid) FROM pg_stat_activity
             WHERE datname = current_database() AND query LIKE 'LISTEN%'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert!(killed);
    let event = timeout(Duration::from_secs(10), events.recv())
        .await
        .expect("a reconnect in time")
        .unwrap();
    assert!(matches!(event, HubEvent::Resync));

    // And it works again.
    app.order("mitte", &[("CHAIR-01", 1)]).await;
    assert_eq!(
        next_order_event(&mut events).await.status,
        OrderStatus::Pending
    );
    shutdown.cancel();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn server_sent_events_stream_order_changes() {
    let db = TestDb::new().await;
    let app = shop(&db).await;
    let (_events, shutdown) = listen(&db, &app).await;

    let response = {
        use tower::ServiceExt;
        let request = axum::http::Request::get("/orders/events?store=mitte")
            .body(Body::empty())
            .unwrap();
        app.router.clone().oneshot(request).await.unwrap()
    };
    assert_eq!(response.headers()["content-type"], "text/event-stream");
    let mut body = response.into_body();

    app.order("mitte", &[("CHAIR-01", 1)]).await;
    let mut received = String::new();
    while !received.contains("event: order") {
        let frame = timeout(Duration::from_secs(5), body.frame())
            .await
            .expect("an SSE frame in time");
        if let Some(Ok(frame)) = frame
            && let Some(data) = frame.data_ref()
        {
            received.push_str(&String::from_utf8_lossy(data));
        }
    }
    assert!(received.contains("\"status\":\"pending\""), "{received}");

    // At shutdown the stream ends, so that graceful shutdown does not wait for it forever.
    app.state.shutdown.cancel();
    let end = timeout(Duration::from_secs(5), async {
        while let Some(frame) = body.frame().await {
            if frame.is_err() {
                break;
            }
        }
    })
    .await;
    assert!(end.is_ok(), "the SSE body ends at shutdown");
    shutdown.cancel();
}

/// Records what it delivers; fails while `fail` is set.
#[derive(Default)]
struct Recorder {
    delivered: Mutex<Vec<(String, Value)>>,
    fail: Mutex<bool>,
}

impl Deliver for &'static Recorder {
    async fn deliver(&self, topic: &str, payload: &Value) -> Result<(), String> {
        if *self.fail.lock().unwrap() {
            return Err("the mail server is down".into());
        }
        self.delivered
            .lock()
            .unwrap()
            .push((topic.to_owned(), payload.clone()));
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_outbox_delivers_each_job_once_and_retries_failures_later() {
    let db = TestDb::new().await;
    let app = shop(&db).await;
    let recorder: &'static Recorder = Box::leak(Box::default());

    *recorder.fail.lock().unwrap() = true;
    app.order("mitte", &[("CHAIR-01", 1)]).await;
    assert_eq!(outbox::drain(&app.state.pool, &recorder).await.unwrap(), 1);
    let client = db.client().await;
    let (attempts, error): (i32, String) = {
        let row = client
            .query_one("SELECT attempts, last_error FROM outbox", &[])
            .await
            .unwrap();
        (row.get(0), row.get(1))
    };
    assert_eq!((attempts, error.as_str()), (1, "the mail server is down"));
    // Not due again yet: nothing to do.
    assert_eq!(outbox::drain(&app.state.pool, &recorder).await.unwrap(), 0);

    // Make it due, and the delivery work.
    client
        .execute("UPDATE outbox SET available_at = now()", &[])
        .await
        .unwrap();
    *recorder.fail.lock().unwrap() = false;
    assert_eq!(outbox::drain(&app.state.pool, &recorder).await.unwrap(), 1);
    assert_eq!(outbox::drain(&app.state.pool, &recorder).await.unwrap(), 0);
    let delivered = recorder.delivered.lock().unwrap().clone();
    assert_eq!(delivered.len(), 1);
    assert_eq!(delivered[0].0, "order.pending");
    assert_eq!(
        repo::outbox::pending(&app.state.pool.get().await.unwrap())
            .await
            .unwrap(),
        0
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_workers_never_deliver_the_same_job() {
    let db = TestDb::new().await;
    let app = shop(&db).await;
    app.stock("mitte", "CHAIR-01", 100).await;
    for _ in 0..30 {
        app.order("mitte", &[("CHAIR-01", 1)]).await;
    }
    let recorder: &'static Recorder = Box::leak(Box::default());
    let pool = app.state.pool.clone();
    let worker = |pool: deadpool_postgres::Pool| async move {
        let mut total = 0;
        loop {
            let n = outbox::drain(&pool, &recorder).await.unwrap();
            if n == 0 {
                return total;
            }
            total += n;
        }
    };
    let (a, b) = tokio::join!(worker(pool.clone()), worker(pool.clone()));
    assert_eq!(
        a + b,
        30,
        "SKIP LOCKED: every job claimed by exactly one worker"
    );
    let mut ids: Vec<String> = recorder
        .delivered
        .lock()
        .unwrap()
        .iter()
        .map(|(_, p)| p["orderId"].as_str().unwrap().to_owned())
        .collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 30);
}

#[tokio::test]
async fn health_endpoints() {
    let db = TestDb::new().await;
    let app = db.app(2);
    let live = app.send(Method::GET, "/healthz", &[], Body::empty()).await;
    assert_eq!((live.status, live.text.as_str()), (StatusCode::OK, "ok"));
    let ready = app.get("/readyz").await;
    assert_eq!(ready.status, StatusCode::OK);
    assert_eq!(ready.body["pool"]["maxSize"], 2);
    let sessions = app.get("/admin/sessions").await;
    // The simple query protocol: every value is text, even the pid.
    assert!(
        sessions.body["sessions"][0]["pid"].is_string(),
        "{}",
        sessions.text
    );
}
