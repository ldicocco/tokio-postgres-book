//! minipool against a real server: DATABASE_URL from the environment or `.env`.

use std::collections::HashSet;
use std::time::Duration;

use futures_util::future::join_all;
use minipool::{Options, Pool, PoolError};
use tokio_postgres::{Client, Config, NoTls};

fn config() -> Config {
    let _ = dotenvy::dotenv();
    let url = std::env::var("DATABASE_URL").expect("set DATABASE_URL (see .env.example)");
    url.parse().unwrap()
}

fn pool(max_size: usize) -> Pool {
    Pool::new(
        config(),
        Options {
            max_size,
            wait_timeout: Duration::from_secs(5),
            verify: false,
        },
    )
}

async fn pid(client: &Client) -> i32 {
    client
        .query_one("SELECT pg_backend_pid()", &[])
        .await
        .unwrap()
        .get(0)
}

async fn admin() -> Client {
    let (client, connection) = config().connect(NoTls).await.unwrap();
    tokio::spawn(connection);
    client
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn never_more_than_max_size_connections() {
    let pool = pool(3);
    let pids = join_all((0..20).map(|_| {
        let pool = pool.clone();
        tokio::spawn(async move {
            let conn = pool.get().await.unwrap();
            let pid = pid(&conn).await;
            tokio::time::sleep(Duration::from_millis(20)).await; // hold it a while
            pid
        })
    }))
    .await;
    let distinct: HashSet<i32> = pids.into_iter().map(Result::unwrap).collect();
    assert!(distinct.len() <= 3, "{} connections", distinct.len());
    assert_eq!(
        pool.status(),
        minipool::Status {
            max_size: 3,
            in_use: 0,
            idle: distinct.len()
        }
    );
}

#[tokio::test]
async fn the_most_recently_used_connection_comes_back_first() {
    let pool = pool(4);
    let (a, b) = (pool.get().await.unwrap(), pool.get().await.unwrap());
    let (pid_a, pid_b) = (pid(&a).await, pid(&b).await);
    drop(a);
    drop(b); // b is on top of the stack now
    assert_eq!(pid(&pool.get().await.unwrap()).await, pid_b);
    assert_ne!(pid_a, pid_b);
}

/// With `verify`, a checkout costs a round trip, and a connection that died while idle is
/// caught at once, however recently it died.
#[tokio::test]
async fn verify_catches_a_connection_that_just_died() {
    let pool = Pool::new(
        config(),
        Options {
            max_size: 1,
            verify: true,
            ..Default::default()
        },
    );
    let victim = pid(&pool.get().await.unwrap()).await; // back to idle
    admin()
        .await
        .execute("SELECT pg_terminate_backend($1)", &[&victim])
        .await
        .unwrap();
    let conn = pool.get().await.unwrap();
    assert_ne!(pid(&conn).await, victim, "a fresh connection, and it works");
}

/// Without `verify`, only `is_closed()` is checked: free, but it turns true only once the
/// connection's task has seen the server go away. Right after the kill, the dead connection
/// may still be handed out (its first query then fails); a moment later it is not.
#[tokio::test]
async fn without_verify_a_dead_connection_is_skipped_once_noticed() {
    let pool = pool(1);
    let victim = pid(&pool.get().await.unwrap()).await;
    admin()
        .await
        .execute("SELECT pg_terminate_backend($1)", &[&victim])
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await; // the FATAL message arrives, the task ends
    let conn = pool.get().await.unwrap();
    assert_ne!(pid(&conn).await, victim);
}

#[tokio::test]
async fn waiting_too_long_is_an_error() {
    let pool = Pool::new(
        config(),
        Options {
            max_size: 1,
            wait_timeout: Duration::from_millis(100),
            verify: false,
        },
    );
    let _held = pool.get().await.unwrap();
    assert!(matches!(pool.get().await, Err(PoolError::Timeout)));
}

#[tokio::test]
async fn a_detached_connection_leaves_the_pool_and_frees_its_place() {
    let pool = pool(1);
    let client = pool.get().await.unwrap().detach();
    assert_eq!(
        pool.status(),
        minipool::Status {
            max_size: 1,
            in_use: 0,
            idle: 0
        }
    );
    let other = pool.get().await.unwrap();
    assert_ne!(pid(&client).await, pid(&other).await);
}

#[tokio::test]
async fn transactions_through_deref_mut() {
    let pool = pool(1);
    let mut conn = pool.get().await.unwrap();
    let tx = conn.transaction().await.unwrap();
    tx.batch_execute("CREATE TEMP TABLE t (x int); INSERT INTO t VALUES (1)")
        .await
        .unwrap();
    drop(tx); // rolled back
    let exists: bool = conn
        .query_one("SELECT to_regclass('pg_temp.t') IS NOT NULL", &[])
        .await
        .unwrap()
        .get(0);
    assert!(!exists);
}
