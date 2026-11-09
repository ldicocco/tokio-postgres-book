//! `#[derive(Insert)]` against PostgreSQL: `RETURNING` the model with its defaults, borrowed
//! and `json` fields, upserts, and the errors constraints raise.

mod common;

use std::time::SystemTime;

use common::TestDb;
use serde::{Deserialize, Serialize};
use shelf::{Error, FromRow, Insert, Table, Upsert};
use tokio_postgres::error::SqlState;

const PRODUCTS: &str = "
    CREATE TABLE products (
        id uuid PRIMARY KEY DEFAULT uuidv7(),
        sku text NOT NULL UNIQUE,
        price_cents bigint NOT NULL CHECK (price_cents >= 0),
        attributes jsonb NOT NULL DEFAULT '{}',
        created_at timestamptz NOT NULL DEFAULT now()
    );";

#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Attributes {
    colour: String,
}

#[derive(Debug, FromRow, Table)]
#[shelf(table = "products")]
struct Product {
    #[shelf(primary_key, generated)]
    id: uuid::Uuid,
    sku: String,
    price_cents: i64,
    #[shelf(json)]
    attributes: Attributes,
    #[shelf(generated)]
    created_at: SystemTime,
}

#[derive(Insert)]
#[shelf(model = Product)]
struct NewProduct<'a> {
    sku: &'a str,
    price_cents: i64,
    #[shelf(json)]
    attributes: Attributes,
}

#[tokio::test]
async fn insert_returns_the_model_with_the_database_defaults() {
    let db = TestDb::new().await;
    let client = db.client_with(PRODUCTS).await;
    let before = SystemTime::now();

    let mug = NewProduct {
        sku: "MUG-01",
        price_cents: 1299,
        attributes: Attributes {
            colour: "blue".into(),
        },
    }
    .insert(&client)
    .await
    .unwrap();
    assert_eq!((mug.sku.as_str(), mug.price_cents), ("MUG-01", 1299));
    assert_eq!(mug.attributes.colour, "blue"); // written through serde, read back
    assert_eq!(mug.id.get_version_num(), 7); // uuidv7(), PG 18
    assert!(mug.created_at >= before - std::time::Duration::from_secs(5));

    let tee = NewProduct {
        sku: "TEE-02",
        price_cents: 2499,
        attributes: Attributes::default(),
    }
    .insert(&client)
    .await
    .unwrap();
    assert!(tee.id > mug.id); // version 7 UUIDs sort by creation time
    assert_eq!(Product::find(&client, &mug.id).await.unwrap().sku, "MUG-01");
}

#[tokio::test]
async fn update_writes_a_json_field_through_serde() {
    let db = TestDb::new().await;
    let client = db.client_with(PRODUCTS).await;
    let mut mug = NewProduct {
        sku: "MUG-01",
        price_cents: 1299,
        attributes: Attributes::default(),
    }
    .insert(&client)
    .await
    .unwrap();
    mug.attributes.colour = "red".into();
    let updated = mug.update(&client).await.unwrap();
    assert_eq!(updated.attributes.colour, "red");
    let stored: serde_json::Value = client
        .query_one("SELECT attributes FROM products", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(stored, serde_json::json!({ "colour": "red" }));
}

#[derive(Insert)]
#[shelf(model = Product)]
struct ProductPrice {
    #[shelf(conflict)]
    sku: String,
    price_cents: i64,
}

#[tokio::test]
async fn upsert_inserts_then_updates_the_same_row() {
    let db = TestDb::new().await;
    let client = db.client_with(PRODUCTS).await;
    let price = |cents| ProductPrice {
        sku: "MUG-01".into(),
        price_cents: cents,
    };

    let first = price(1299).upsert(&client).await.unwrap();
    let second = price(990).upsert(&client).await.unwrap();
    assert_eq!(second.id, first.id);
    assert_eq!(second.price_cents, 990);
    assert_eq!(second.attributes, Attributes::default()); // the column's default, kept
    assert_eq!(Product::count(&client).await.unwrap(), 1);

    // A plain insert of the same key is refused by the unique constraint.
    let error = price(500).insert(&client).await.unwrap_err();
    let Error::Db(db_error) = &error else {
        panic!("a database error: {error}")
    };
    assert_eq!(db_error.code(), Some(&SqlState::UNIQUE_VIOLATION));
    assert_eq!(
        db_error.as_db_error().and_then(|e| e.constraint()),
        Some("products_sku_key")
    );
}

#[tokio::test]
async fn a_check_constraint_names_itself() {
    let db = TestDb::new().await;
    let client = db.client_with(PRODUCTS).await;
    let error = ProductPrice {
        sku: "BAD-01".into(),
        price_cents: -1,
    }
    .insert(&client)
    .await
    .unwrap_err();
    let Error::Db(db_error) = &error else {
        panic!("a database error: {error}")
    };
    assert_eq!(db_error.code(), Some(&SqlState::CHECK_VIOLATION));
    assert_eq!(
        db_error.as_db_error().and_then(|e| e.constraint()),
        Some("products_price_cents_check")
    );
}
