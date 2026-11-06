mod common;

use common::TestDb;
use stockroom::db::migrate::{self, MigrateError, Migration};

/// This build's migrations, plus `extra`.
fn with(extra: Migration) -> Vec<Migration> {
    let mut all: Vec<Migration> = migrate::MIGRATIONS
        .iter()
        .map(|m| Migration {
            version: m.version,
            name: m.name,
            sql: m.sql,
        })
        .collect();
    all.push(extra);
    all
}

#[tokio::test]
async fn a_migrated_database_has_nothing_left_to_do() {
    let db = TestDb::new().await;
    let mut client = db.client().await;
    assert_eq!(migrate::run(&mut client).await.unwrap(), Vec::<i32>::new());
}

#[tokio::test]
async fn a_new_migration_is_applied_once_and_an_older_build_refuses_the_schema() {
    let db = TestDb::new().await;
    let mut client = db.client().await;
    let all = with(Migration {
        version: 3,
        name: "extra",
        sql: "CREATE TABLE extra (id int)",
    });
    assert_eq!(migrate::run_with(&mut client, &all).await.unwrap(), vec![3]);
    assert_eq!(
        migrate::run_with(&mut client, &all).await.unwrap(),
        Vec::<i32>::new()
    );
    // This build does not know migration 3.
    assert!(matches!(
        migrate::run(&mut client).await,
        Err(MigrateError::Unknown { version: 3 })
    ));
}

#[tokio::test]
async fn an_edited_migration_is_refused() {
    let db = TestDb::new().await;
    let mut client = db.client().await;
    let mut edited = with(Migration {
        version: 3,
        name: "extra",
        sql: "SELECT 1",
    });
    edited[0].sql = "-- someone edited the first migration\n";
    match migrate::run_with(&mut client, &edited).await {
        Err(MigrateError::Edited { version: 1, .. }) => {}
        other => panic!("expected Edited for version 1, got {other:?}"),
    }
}

#[tokio::test]
async fn a_failed_migration_leaves_no_trace() {
    let db = TestDb::new().await;
    let mut client = db.client().await;
    let broken = with(Migration {
        version: 3,
        name: "broken",
        sql: "CREATE TABLE half (id int); SELECT 1/0",
    });
    assert!(migrate::run_with(&mut client, &broken).await.is_err());
    // DDL is transactional: the table the migration created is gone with it.
    let exists: bool = client
        .query_one("SELECT to_regclass('half') IS NOT NULL", &[])
        .await
        .unwrap()
        .get(0);
    assert!(!exists);
    let recorded: i64 = client
        .query_one("SELECT count(*) FROM schema_migrations", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(recorded, 2);
}

#[tokio::test]
async fn two_instances_migrating_at_once_apply_each_migration_once() {
    let db = TestDb::new().await;
    let (mut a, mut b) = (db.client().await, db.client().await);
    let all = with(Migration {
        version: 3,
        name: "slow",
        sql: "SELECT pg_sleep(0.2); CREATE TABLE slow (id int)",
    });
    let (from_a, from_b) = tokio::join!(
        migrate::run_with(&mut a, &all),
        migrate::run_with(&mut b, &all)
    );
    let mut applied = [from_a.unwrap(), from_b.unwrap()].concat();
    applied.sort();
    assert_eq!(
        applied,
        vec![3],
        "the advisory lock lets exactly one of them apply it"
    );
}
