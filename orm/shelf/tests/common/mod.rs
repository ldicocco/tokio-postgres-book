//! Test support: a schema of its own for each test, in the database `shelf_test`.
//!
//! shelf has no migrations, so a test needs no template database (stockroom's harness clones
//! one). Each test creates its tables in a fresh schema, which every connection it opens finds
//! first on its `search_path`, and the schema is dropped when the [`TestDb`] is. Unlike
//! temporary tables, which one session alone can see, this lets a test commit and use several
//! connections.

#![allow(dead_code)] // each test binary uses its own subset

use std::sync::OnceLock;

use tokio_postgres::error::SqlState;
use tokio_postgres::{Client, Config, NoTls};
use uuid::Uuid;

/// The database the tests share; each test works in a schema of its own inside it.
pub const DATABASE: &str = "shelf_test";

/// The server and an account allowed to create databases: `ADMIN_DATABASE_URL`, from the
/// environment or `.env`.
fn admin_config() -> Config {
    let _ = dotenvy::dotenv();
    std::env::var("ADMIN_DATABASE_URL")
        .expect("set ADMIN_DATABASE_URL (see .env.example) to run these tests")
        .parse()
        .expect("ADMIN_DATABASE_URL must parse")
}

/// A connection whose task we can wait for, so that "closed" really means closed.
async fn connect(config: &Config) -> (Client, tokio::task::JoinHandle<()>) {
    let (client, connection) = config.connect(NoTls).await.expect("connect");
    let task = tokio::spawn(async move {
        let _ = connection.await;
    });
    (client, task)
}

/// Creates `shelf_test` if it does not exist, once per test binary.
///
/// A plain `OnceLock` and a thread of its own: each `#[tokio::test]` has its own runtime,
/// and the work must not belong to whichever test happened to come first.
fn ensure_database() {
    static DONE: OnceLock<()> = OnceLock::new();
    DONE.get_or_init(|| {
        std::thread::spawn(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("a runtime")
                .block_on(async {
                    let (admin, task) = connect(&admin_config()).await;
                    match admin
                        .batch_execute(&format!("CREATE DATABASE {DATABASE}"))
                        .await
                    {
                        Ok(()) => {}
                        // An earlier run, or another test binary at the same moment, made it.
                        Err(e)
                            if e.code() == Some(&SqlState::DUPLICATE_DATABASE)
                                || e.code() == Some(&SqlState::UNIQUE_VIOLATION) => {}
                        Err(e) => panic!("cannot create {DATABASE}: {e}"),
                    }
                    drop(admin);
                    let _ = task.await;
                })
        })
        .join()
        .expect("creating the test database failed");
    });
}

/// A schema for one test, dropped (with everything in it) when the value is.
pub struct TestDb {
    pub schema: String,
    /// Connects to `shelf_test` with `search_path` set to the schema.
    pub config: Config,
}

impl TestDb {
    pub async fn new() -> TestDb {
        ensure_database();
        let schema = format!("t_{}", Uuid::now_v7().simple());
        let mut config = admin_config();
        config.dbname(DATABASE);
        let (client, task) = connect(&config).await;
        client
            .batch_execute(&format!("CREATE SCHEMA {schema}"))
            .await
            .expect("create the test's schema");
        drop(client);
        let _ = task.await;
        // Server-side settings travel in the startup packet's `options` (main book §1.4).
        config.options(format!("-c search_path={schema}"));
        TestDb { schema, config }
    }

    /// A new connection, its tables found in this test's schema.
    pub async fn client(&self) -> Client {
        connect(&self.config).await.0
    }

    /// A new connection with `sql` already run on it: usually the test's `CREATE TABLE`s.
    pub async fn client_with(&self, sql: &str) -> Client {
        let client = self.client().await;
        client
            .batch_execute(sql)
            .await
            .expect("the test's setup SQL");
        client
    }
}

impl Drop for TestDb {
    /// `Drop` cannot be async, and this one may run inside a runtime, where blocking on a
    /// future would panic. So the schema is dropped from a thread with a runtime of its own.
    fn drop(&mut self) {
        let schema = self.schema.clone();
        let mut config = admin_config();
        config.dbname(DATABASE);
        let _ = std::thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("a runtime")
                .block_on(async move {
                    let (admin, _task) = connect(&config).await;
                    let _ = admin
                        .batch_execute(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE"))
                        .await;
                })
        })
        .join();
    }
}
