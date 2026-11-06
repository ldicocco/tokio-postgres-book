//! pgq against a real server: DATABASE_URL from the environment or `.env`.

use tokio_postgres::{Client, NoTls};

async fn connect() -> Client {
    let _ = dotenvy::dotenv();
    let url = std::env::var("DATABASE_URL").expect("set DATABASE_URL (see .env.example)");
    let (client, connection) = tokio_postgres::connect(&url, NoTls).await.unwrap();
    tokio::spawn(connection);
    client
}

#[tokio::test]
async fn several_statements_each_with_its_own_result() {
    let client = connect().await;
    let out = pgq::run(
        &client,
        "CREATE TEMP TABLE t (id int, name text);
         INSERT INTO t VALUES (1, 'chair'), (2, NULL);
         SELECT * FROM t ORDER BY id;
         SELECT * FROM t WHERE false",
    )
    .await
    .unwrap();
    assert_eq!(
        out,
        "OK, 0 affected\n\n\
         OK, 2 affected\n\n \
         id | name\n----+-------\n 1  | chair\n 2  |\n(2 rows)\n\n \
         id | name\n----+------\n(0 rows)\n\n"
    );
}

#[tokio::test]
async fn describe_names_parameter_and_column_types() {
    let client = connect().await;
    let out = pgq::describe(&client, "SELECT $1::int8 + 1 AS next, now() AS at")
        .await
        .unwrap();
    assert!(out.contains(" $1        | int8"), "{out}");
    assert!(out.contains(" next   | int8"), "{out}");
    assert!(out.contains(" at     | timestamptz"), "{out}");
}

#[tokio::test]
async fn stream_prints_rows_as_they_come() {
    let client = connect().await;
    let mut lines = Vec::new();
    let rows = pgq::stream(
        &client,
        "SELECT n, n * n AS square FROM generate_series(1, 3) n",
        |line| lines.push(line.to_owned()),
    )
    .await
    .unwrap();
    assert_eq!(rows, 3);
    assert_eq!(lines, ["n\tsquare", "1\t1", "2\t4", "3\t9"]);
}
