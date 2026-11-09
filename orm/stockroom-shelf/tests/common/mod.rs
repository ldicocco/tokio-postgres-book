//! Test support: stockroom's own harness, and the same tests run against both routers.
//!
//! stockroom's harness (`stockroom/tests/common/mod.rs`) is included as it is: a database per
//! test, cloned from the template stockroom's migrations built, and `TestApp`, which calls a
//! router with `oneshot`. Its fields are public, so a `TestApp` can hold this crate's router
//! as well as stockroom's. [`both`] runs one scenario against each, on a database of its own,
//! and asserts that every reply is the same once what differs between two databases (the
//! generated ids, the times) is masked.

#![allow(dead_code)] // each test binary uses its own subset

#[path = "../../../../stockroom/tests/common/mod.rs"]
mod stockroom_harness;

use serde_json::Value;
use stockroom::http::RouterOptions;
pub use stockroom_harness::{Reply, TestApp, TestDb};
use uuid::Uuid;

/// This crate's router, on a pool of `pool_size`: stockroom's state, shelf's routes.
pub fn shelf_app(db: &TestDb, pool_size: usize) -> TestApp {
    let stockroom = db.app(pool_size);
    TestApp {
        router: stockroom_shelf::http::router(stockroom.state.clone(), RouterOptions::default()),
        state: stockroom.state,
    }
}

/// Runs `scenario` against stockroom's router and against this crate's, each on a fresh
/// database, and asserts that they replied the same, reply by reply. The scenario's own
/// assertions run against both.
pub async fn both(scenario: impl AsyncFn(&TestApp) -> Vec<Reply>) {
    let (stockroom_db, shelf_db) = (TestDb::new().await, TestDb::new().await);
    let stockroom = scenario(&stockroom_db.app(4)).await;
    let shelf = scenario(&shelf_app(&shelf_db, 4)).await;
    assert_eq!(
        stockroom.len(),
        shelf.len(),
        "the scenario sent as many requests"
    );
    for (i, (a, b)) in stockroom.iter().zip(&shelf).enumerate() {
        assert_eq!(
            a.status, b.status,
            "reply {i}: status\n{}\n{}",
            a.text, b.text
        );
        for header in ["etag", "content-type"] {
            assert_eq!(
                a.headers.get(header),
                b.headers.get(header),
                "reply {i}: {header}"
            );
        }
        assert_eq!(masked(a), masked(b), "reply {i}: body");
    }
}

/// A reply's body with every UUID and timestamp replaced, so that two databases' replies can
/// be compared: a JSON body as JSON, any other (NDJSON) line by line.
pub fn masked(reply: &Reply) -> Value {
    if reply.body != Value::Null || reply.text.is_empty() {
        return mask(reply.body.clone());
    }
    let lines = reply
        .text
        .lines()
        .map(|line| match serde_json::from_str(line) {
            Ok(value) => mask(value),
            Err(_) => Value::String(line.to_owned()),
        });
    Value::Array(lines.collect())
}

fn mask(value: Value) -> Value {
    match value {
        Value::String(s) if s.parse::<Uuid>().is_ok() => Value::from("<uuid>"),
        Value::String(s) if s.parse::<jiff::Timestamp>().is_ok() => Value::from("<timestamp>"),
        Value::Array(items) => Value::Array(items.into_iter().map(mask).collect()),
        Value::Object(fields) => fields.into_iter().map(|(k, v)| (k, mask(v))).collect(),
        other => other,
    }
}
