//! `#[derive(Table)]` against PostgreSQL: the reads it generates, keys simple and composite,
//! quoted names, a schema, and `NotFound`.

mod common;

use common::TestDb;
use shelf::{Error, FromRow, Table};

const PRODUCTS: &str = "
    CREATE TABLE products (
        id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
        sku text NOT NULL UNIQUE,
        price_cents bigint NOT NULL
    );
    INSERT INTO products (sku, price_cents) VALUES ('TEE-02', 2499), ('MUG-01', 1299);";

#[derive(Debug, PartialEq, FromRow, Table)]
#[shelf(table = "products")]
struct Product {
    #[shelf(primary_key, generated)]
    id: i64,
    sku: String,
    price_cents: i64,
    #[shelf(skip)]
    selected: bool,
}

#[tokio::test]
async fn find_find_opt_all_and_count() {
    let db = TestDb::new().await;
    let client = db.client_with(PRODUCTS).await;

    let tee = Product::find(&client, &1).await.unwrap();
    assert_eq!((tee.sku.as_str(), tee.price_cents), ("TEE-02", 2499));
    assert_eq!(tee.key(), 1);
    assert!(Product::find_opt(&client, &3).await.unwrap().is_none());

    let all = Product::all(&client).await.unwrap();
    let ids: Vec<i64> = all.iter().map(|p| p.id).collect();
    assert_eq!(ids, [1, 2]); // ordered by the key
    assert_eq!(Product::count(&client).await.unwrap(), 2);
}

#[tokio::test]
async fn a_missing_key_is_not_found() {
    let db = TestDb::new().await;
    let client = db.client_with(PRODUCTS).await;
    let error = Product::find(&client, &42).await.unwrap_err();
    assert!(matches!(error, Error::NotFound { model: "Product" }));
    assert_eq!(error.to_string(), "Product not found");
}

#[tokio::test]
async fn update_writes_every_column_but_the_key_and_generated_ones() {
    let db = TestDb::new().await;
    let client = db.client_with(PRODUCTS).await;
    let mut tee = Product::find(&client, &1).await.unwrap();
    tee.sku = "TEE-03".into();
    tee.price_cents = 1999;
    // `id` is GENERATED ALWAYS: an UPDATE that set it would fail, so `generated` leaves it out.
    let updated = tee.update(&client).await.unwrap();
    assert_eq!(
        (updated.sku.as_str(), updated.price_cents),
        ("TEE-03", 1999)
    );
    assert_eq!(Product::find(&client, &1).await.unwrap(), updated);
}

#[tokio::test]
async fn delete_then_every_write_to_the_row_is_not_found() {
    let db = TestDb::new().await;
    let client = db.client_with(PRODUCTS).await;
    let mug = Product::find(&client, &2).await.unwrap();
    mug.delete(&client).await.unwrap();
    assert_eq!(Product::count(&client).await.unwrap(), 1);

    for error in [
        mug.update(&client).await.unwrap_err(),
        mug.delete(&client).await.unwrap_err(),
        Product::delete_by_key(&client, &2).await.unwrap_err(),
    ] {
        assert!(
            matches!(error, Error::NotFound { model: "Product" }),
            "{error}"
        );
    }
}

/// Nothing but a key: `update` has nothing to write, and reads the row back.
#[derive(Debug, PartialEq, FromRow, Table)]
#[shelf(table = "tags")]
struct Tag {
    #[shelf(primary_key)]
    name: String,
}

#[tokio::test]
async fn a_model_that_is_all_key_updates_to_itself() {
    let db = TestDb::new().await;
    let client = db
        .client_with("CREATE TABLE tags (name text PRIMARY KEY); INSERT INTO tags VALUES ('sale');")
        .await;
    let tag = Tag::find(&client, &"sale".to_owned()).await.unwrap();
    assert_eq!(Tag::UPDATE, Tag::FIND);
    assert_eq!(tag.update(&client).await.unwrap(), tag);
}

#[derive(Debug, PartialEq, FromRow, Table)]
#[shelf(table = "stock")]
struct Stock {
    #[shelf(primary_key)]
    store_id: i64,
    #[shelf(primary_key)]
    product_id: i64,
    on_hand: i32,
}

#[tokio::test]
async fn a_composite_key_is_a_tuple() {
    let db = TestDb::new().await;
    let client = db
        .client_with(
            "CREATE TABLE stock (store_id bigint, product_id bigint, on_hand int NOT NULL,
                                 PRIMARY KEY (store_id, product_id));
             INSERT INTO stock VALUES (1, 1, 5), (1, 2, 7), (2, 1, 9);",
        )
        .await;
    let stock = Stock::find(&client, &(1, 2)).await.unwrap();
    assert_eq!(stock.on_hand, 7);
    assert_eq!(stock.key(), (1, 2));
    let keys: Vec<(i64, i64)> = Stock::all(&client)
        .await
        .unwrap()
        .iter()
        .map(Table::key)
        .collect();
    assert_eq!(keys, [(1, 1), (1, 2), (2, 1)]);
}

/// A table named after a key word, with camelCase columns and one called `type`, in the
/// session's temporary schema: every identifier must be quoted for this to work.
#[derive(Debug, FromRow, Table)]
#[shelf(table = "order", schema = "pg_temp", rename_all = "camelCase")]
struct Order {
    #[shelf(primary_key)]
    order_id: i64,
    r#type: String,
    total_cents: i64,
}

#[tokio::test]
async fn quoted_names_and_a_schema() {
    let db = TestDb::new().await;
    let client = db
        .client_with(
            r#"CREATE TEMP TABLE "order" ("orderId" bigint PRIMARY KEY, "type" text NOT NULL,
                                          "totalCents" bigint NOT NULL);
               INSERT INTO "order" VALUES (7, 'web', 3798);"#,
        )
        .await;
    assert_eq!(Order::TABLE, r#""pg_temp"."order""#);
    let order = Order::find(&client, &7).await.unwrap();
    assert_eq!((order.r#type.as_str(), order.total_cents), ("web", 3798));
    assert_eq!(Order::count(&client).await.unwrap(), 1);
}

#[cfg(feature = "deadpool")]
#[tokio::test]
async fn a_pooled_connection_and_its_transaction() {
    let db = TestDb::new().await;
    db.client_with(PRODUCTS).await;
    let pool = db.pool(1);
    let mut object = pool.get().await.unwrap();
    assert_eq!(Product::find(&object, &2).await.unwrap().sku, "MUG-01");
    let tx = object.transaction().await.unwrap();
    assert_eq!(Product::count(&tx).await.unwrap(), 2);
    tx.commit().await.unwrap();
    // FIND and COUNT, each prepared once on this connection.
    assert_eq!(object.statement_cache.size(), 2);
}
