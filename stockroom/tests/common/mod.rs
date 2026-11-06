//! Test support (Chapter 17): a fresh database per test, and an HTTP client for the router.
//!
//! Every test gets its own database, cloned with `CREATE DATABASE … TEMPLATE` from a template
//! that was migrated once. Cloning a small database takes milliseconds, tests cannot see each
//! other's data, and they can commit, `NOTIFY` and use many connections, which tests that
//! roll back a single transaction cannot.

#![allow(dead_code)] // each test binary uses its own subset

use std::sync::OnceLock;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Method, Request, StatusCode};
use deadpool_postgres::Pool;
use http_body_util::BodyExt;
use serde_json::{Value, json};
use stockroom::config::DbArgs;
use stockroom::db::{self, MakeTls};
use stockroom::http::{AppState, RouterOptions};
use stockroom::notify::Hub;
use tokio_postgres::error::SqlState;
use tokio_postgres::{Client, Config};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;
use uuid::Uuid;

/// The server and credentials: `ADMIN_DATABASE_URL` from the environment or `.env`. The
/// account must be allowed to create databases.
fn admin_url() -> String {
    let _ = dotenvy::dotenv();
    std::env::var("ADMIN_DATABASE_URL")
        .expect("set ADMIN_DATABASE_URL (see .env.example) to run these tests")
}

fn config_for(dbname: &str) -> Config {
    let mut config = DbArgs::new(admin_url())
        .pg_config()
        .expect("ADMIN_DATABASE_URL must parse");
    config.dbname(dbname);
    config
}

pub fn tls() -> MakeTls {
    db::tls::connector(None).expect("a TLS connector")
}

/// A connection whose task we can wait for, so that "closed" really means closed.
async fn connect(config: &Config) -> (Client, tokio::task::JoinHandle<()>) {
    let (client, connection) = config.connect(tls()).await.expect("connect");
    let task = tokio::spawn(async move {
        let _ = connection.await;
    });
    (client, task)
}

/// The name of the template database for this build's migrations, created on first use.
///
/// A plain `OnceLock` and a thread of its own: each `#[tokio::test]` has its own runtime,
/// and the template must not belong to whichever test happened to come first.
fn template() -> &'static str {
    static TEMPLATE: OnceLock<String> = OnceLock::new();
    TEMPLATE.get_or_init(|| {
        std::thread::spawn(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("a runtime")
                .block_on(create_template())
        })
        .join()
        .expect("creating the template database failed")
    })
}

async fn create_template() -> String {
    let all_sql: String = db::migrate::MIGRATIONS.iter().map(|m| m.sql).collect();
    let name = format!("stockroom_template_{}", db::migrate::checksum(&all_sql));
    let (admin, task) = connect(&config_for("postgres")).await;
    // Test binaries can run at the same time: one of them builds the template.
    admin
        .execute(
            "SELECT pg_advisory_lock(hashtext('stockroom test template'))",
            &[],
        )
        .await
        .unwrap();
    let exists: bool = admin
        .query_one(
            "SELECT EXISTS (SELECT FROM pg_database WHERE datname = $1)",
            &[&name],
        )
        .await
        .unwrap()
        .get(0);
    if !exists {
        // Templates of older migrations are of no further use.
        for row in admin
            .query(
                "SELECT datname FROM pg_database WHERE datname LIKE 'stockroom\\_template\\_%'",
                &[],
            )
            .await
            .unwrap()
        {
            let old: &str = row.get(0);
            admin
                .batch_execute(&format!("DROP DATABASE \"{old}\" WITH (FORCE)"))
                .await
                .unwrap();
        }
        admin
            .batch_execute(&format!("CREATE DATABASE \"{name}\""))
            .await
            .unwrap();
        let (mut client, client_task) = connect(&config_for(&name)).await;
        db::migrate::run(&mut client)
            .await
            .expect("migrations apply to an empty database");
        drop(client);
        let _ = client_task.await; // the template must have no sessions when it is cloned
    }
    admin
        .execute(
            "SELECT pg_advisory_unlock(hashtext('stockroom test template'))",
            &[],
        )
        .await
        .unwrap();
    drop(admin);
    let _ = task.await;
    name
}

/// A database for one test, dropped when the value is.
pub struct TestDb {
    pub name: String,
    pub config: Config,
}

impl TestDb {
    pub async fn new() -> TestDb {
        let template = template();
        let name = format!("stockroom_test_{}", Uuid::now_v7().simple());
        let (admin, task) = connect(&config_for("postgres")).await;
        // A session that has just left the template can linger for a moment; cloning
        // fails with 55006 (object in use) until it is gone.
        let mut attempts = 0;
        loop {
            match admin
                .batch_execute(&format!(
                    "CREATE DATABASE \"{name}\" TEMPLATE \"{template}\""
                ))
                .await
            {
                Ok(()) => break,
                Err(e) if e.code() == Some(&SqlState::OBJECT_IN_USE) && attempts < 50 => {
                    attempts += 1;
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                Err(e) => panic!("cannot create {name}: {e}"),
            }
        }
        drop(admin);
        let _ = task.await;
        TestDb {
            config: config_for(&name),
            name,
        }
    }

    pub fn pool(&self, size: usize) -> Pool {
        db::pool::create(self.config.clone(), tls(), size).expect("a pool")
    }

    pub async fn client(&self) -> Client {
        db::connect(&self.config, &tls()).await.expect("connect")
    }

    /// The router, on a pool of `pool_size`, without the listener and the worker (tests
    /// that need them start them).
    pub fn app(&self, pool_size: usize) -> TestApp {
        self.app_with(pool_size, RouterOptions::default())
    }

    pub fn app_with(&self, pool_size: usize, options: RouterOptions) -> TestApp {
        let state = AppState {
            pool: self.pool(pool_size),
            hub: Hub::new(1024),
            tls: tls(),
            shutdown: CancellationToken::new(),
        };
        TestApp {
            router: stockroom::http::router(state.clone(), options),
            state,
        }
    }
}

impl Drop for TestDb {
    /// `Drop` cannot be async, and this one may run inside a runtime, where blocking on a
    /// future would panic. So the database is dropped from a thread with a runtime of its own.
    fn drop(&mut self) {
        let name = self.name.clone();
        let _ = std::thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("a runtime")
                .block_on(async move {
                    let (admin, _task) = connect(&config_for("postgres")).await;
                    // FORCE: the test's pool may still hold connections.
                    let _ = admin
                        .batch_execute(&format!("DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)"))
                        .await;
                })
        })
        .join();
    }
}

/// The router, called in-process with `oneshot`: no port, no HTTP client.
#[derive(Clone)]
pub struct TestApp {
    pub router: Router,
    pub state: AppState,
}

pub struct Reply {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Value,
    pub text: String,
}

impl TestApp {
    pub async fn send(
        &self,
        method: Method,
        uri: &str,
        headers: &[(&str, &str)],
        body: Body,
    ) -> Reply {
        let mut request = Request::builder().method(method).uri(uri);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let response = self
            .router
            .clone()
            .oneshot(request.body(body).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        Reply {
            status,
            headers,
            body,
            text,
        }
    }

    pub async fn get(&self, uri: &str) -> Reply {
        self.send(Method::GET, uri, &[], Body::empty()).await
    }

    pub async fn json(
        &self,
        method: Method,
        uri: &str,
        headers: &[(&str, &str)],
        body: Value,
    ) -> Reply {
        let mut all = vec![("content-type", "application/json")];
        all.extend_from_slice(headers);
        self.send(method, uri, &all, Body::from(body.to_string()))
            .await
    }

    pub async fn post(&self, uri: &str, body: Value) -> Reply {
        self.json(Method::POST, uri, &[], body).await
    }

    // Fixtures, through the API itself.

    pub async fn store(&self, code: &str) {
        let reply = self
            .post(
                "/stores",
                json!({ "code": code, "name": code, "address": {
                    "street": "Main 1", "city": "Berlin", "postcode": "10115", "country": "DE" } }),
            )
            .await;
        assert_eq!(reply.status, StatusCode::CREATED, "{}", reply.text);
    }

    pub async fn product(&self, sku: &str, price_cents: i64) {
        let reply = self
            .post(
                "/products",
                json!({ "sku": sku, "name": sku, "priceCents": price_cents }),
            )
            .await;
        assert_eq!(reply.status, StatusCode::CREATED, "{}", reply.text);
    }

    pub async fn stock(&self, store: &str, sku: &str, delta: i32) {
        let reply = self
            .post(
                &format!("/stores/{store}/stock/adjust"),
                json!({ "sku": sku, "delta": delta, "reason": "test" }),
            )
            .await;
        assert_eq!(reply.status, StatusCode::OK, "{}", reply.text);
    }

    pub async fn order(&self, store: &str, lines: &[(&str, i32)]) -> Reply {
        let lines: Vec<Value> = lines
            .iter()
            .map(|(sku, q)| json!({ "sku": sku, "quantity": q }))
            .collect();
        self.post(
            "/orders",
            json!({ "store": store, "customer": { "name": "Ada", "email": "ada@example.com" }, "lines": lines }),
        )
        .await
    }

    /// `(on_hand, reserved, available)` of one SKU in one store.
    pub async fn level(&self, store: &str, sku: &str) -> (i64, i64, i64) {
        let reply = self.get(&format!("/stores/{store}/stock")).await;
        let row = reply
            .body
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["sku"] == sku)
            .expect("a stock row");
        (
            row["onHand"].as_i64().unwrap(),
            row["reserved"].as_i64().unwrap(),
            row["available"].as_i64().unwrap(),
        )
    }
}
