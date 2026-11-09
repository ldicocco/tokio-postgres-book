//! Streams of models against PostgreSQL: rows read as they arrive, a mapping checked before the
//! first one, and a connection that waits while a stream is unread.

mod common;

use std::time::Duration;

use common::TestDb;
use futures_util::{StreamExt, TryStreamExt};
use shelf::{Error, FromRow, Mapping, Table, fetch_stream};

#[derive(Debug, FromRow)]
struct Number {
    n: i64,
    label: String,
}

const NUMBERS: &str = "SELECT g AS n, 'n' || g AS label FROM generate_series(1, $1::bigint) g";

#[tokio::test]
async fn a_stream_reads_every_row_without_holding_them() {
    let db = TestDb::new().await;
    let client = db.client().await;
    let mut rows = fetch_stream::<Number>(&client, NUMBERS, &[&100_000_i64])
        .await
        .unwrap();
    let (mut count, mut sum) = (0, 0);
    while let Some(number) = rows.try_next().await.unwrap() {
        count += 1;
        sum += number.n;
        assert_eq!(number.label, format!("n{}", number.n));
    }
    assert_eq!((count, sum), (100_000, 5_000_050_000));
}

#[tokio::test]
async fn a_wrong_mapping_fails_before_the_first_row() {
    let db = TestDb::new().await;
    let client = db.client().await;
    #[derive(Debug, FromRow)]
    struct Wrong {
        #[allow(dead_code)]
        n: i32, // the column is a bigint
    }
    // Even for a result with no rows: the statement's columns are checked once.
    let error = fetch_stream::<Wrong>(&client, NUMBERS, &[&0_i64])
        .await
        .map(|_| ())
        .unwrap_err();
    assert!(
        matches!(
            &error,
            Error::Mapping {
                problem: Mapping::WrongType { .. },
                ..
            }
        ),
        "{error}"
    );
}

#[tokio::test]
async fn a_bad_row_is_an_error_in_its_place() {
    let db = TestDb::new().await;
    let client = db.client().await;
    let sql = "SELECT g AS n, CASE WHEN g = 3 THEN NULL ELSE 'n' || g END AS label
               FROM generate_series(1, 5::bigint) g";
    let items: Vec<Result<Number, Error>> = fetch_stream::<Number>(&client, sql, &[])
        .await
        .unwrap()
        .collect()
        .await;
    assert_eq!(items.len(), 5);
    assert!(items[1].is_ok() && items[3].is_ok());
    assert!(matches!(
        items[2],
        Err(Error::Mapping {
            problem: Mapping::Null { .. },
            ..
        })
    ));
}

#[derive(Debug, FromRow, Table)]
#[shelf(table = "events")]
struct Event {
    #[shelf(primary_key)]
    id: i64,
    kind: String,
}

#[tokio::test]
async fn a_query_streams_its_rows_through_any_executor() {
    let db = TestDb::new().await;
    db.client_with(
        "CREATE TABLE events (id bigint PRIMARY KEY, kind text NOT NULL);
         INSERT INTO events SELECT g, CASE WHEN g % 2 = 0 THEN 'click' ELSE 'view' END
             FROM generate_series(1, 10000) g;",
    )
    .await;
    let pool = db.pool(1);
    let mut client = pool.get().await.unwrap();
    let clicks = Event::query().filter(Event::KIND.eq("click"));
    let ids: Vec<i64> = clicks
        .stream(&client)
        .await
        .unwrap()
        .map_ok(|e| e.id)
        .try_collect()
        .await
        .unwrap();
    assert_eq!((ids.len(), ids[0], ids[4999]), (5000, 2, 10000));
    let tx = client.transaction().await.unwrap();
    let count = clicks
        .stream(&tx)
        .await
        .unwrap()
        .try_fold(0, |n, _| async move { Ok(n + 1) })
        .await
        .unwrap();
    assert_eq!(count, 5000);
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn an_unread_stream_holds_up_its_connection_until_it_is_dropped() {
    let db = TestDb::new().await;
    let client = db.client().await;
    // About 10 MB: more than the socket buffers hold, so the server waits for the client.
    let sql = "SELECT g AS n, repeat('x', 100) AS label FROM generate_series(1, 100000::bigint) g";
    let mut rows = fetch_stream::<Number>(&client, sql, &[]).await.unwrap();
    assert_eq!(rows.try_next().await.unwrap().unwrap().n, 1);
    // The next request on the connection waits behind the unread rows.
    let waiting = tokio::time::timeout(
        Duration::from_millis(300),
        client.query_one("SELECT 1", &[]),
    );
    assert!(
        waiting.await.is_err(),
        "the query should wait for the stream"
    );
    // Dropped, the stream's remaining rows are discarded, and the connection serves again.
    drop(rows);
    let one: i32 = client.query_one("SELECT 1", &[]).await.unwrap().get(0);
    assert_eq!(one, 1);
}
