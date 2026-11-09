//! The harness itself: each test sees its own schema, and the schema goes when the test ends.

mod common;

use common::TestDb;

#[tokio::test]
async fn a_test_works_in_its_own_schema_which_is_dropped_afterwards() {
    let db = TestDb::new().await;
    let client = db
        .client_with("CREATE TABLE things (id int PRIMARY KEY)")
        .await;
    let schema: String = client
        .query_one("SELECT current_schema()", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(schema, db.schema);
    // A second connection finds the same table: the schema is not tied to a session.
    let other = db.client().await;
    other
        .execute("INSERT INTO things VALUES (1)", &[])
        .await
        .unwrap();
    let name = db.schema.clone();
    drop((client, other));
    drop(db);

    let check = TestDb::new().await;
    let left: i64 = check
        .client()
        .await
        .query_one(
            "SELECT count(*) FROM pg_namespace WHERE nspname = $1",
            &[&name],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(left, 0, "the first test's schema was dropped");
}
