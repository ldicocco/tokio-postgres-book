//! `#[derive(Patch)]` and the `version` and `updated_at` attributes against PostgreSQL: absent,
//! null and set fields, two edits of one row, a missing row, and one transaction's `now()`.

mod common;

use std::time::SystemTime;

use common::TestDb;
use shelf::{Error, FromRow, Patch, Table};
use tokio_postgres::error::SqlState;

const PRODUCTS: &str = "
    CREATE TABLE products (
        id bigint PRIMARY KEY,
        sku text NOT NULL,
        price_cents bigint NOT NULL,
        note text,
        version int NOT NULL DEFAULT 1,
        updated_at timestamptz NOT NULL DEFAULT now()
    );
    INSERT INTO products (id, sku, price_cents, note) VALUES (1, 'MUG-01', 1299, 'fragile');";

#[derive(Debug, Clone, PartialEq, FromRow, Table)]
#[shelf(table = "products")]
struct Product {
    #[shelf(primary_key)]
    id: i64,
    sku: String,
    price_cents: i64,
    note: Option<String>,
    #[shelf(version)]
    version: i32,
    #[shelf(updated_at)]
    updated_at: SystemTime,
}

#[derive(Default, Patch)]
#[shelf(model = Product)]
struct ProductPatch {
    sku: Option<String>,
    price_cents: Option<i64>,
    note: Option<Option<String>>,
    #[shelf(version)]
    version: i32,
}

#[tokio::test]
async fn a_patch_sets_what_it_has_and_leaves_the_rest() {
    let db = TestDb::new().await;
    let client = db.client_with(PRODUCTS).await;
    let before = Product::find(&client, &1).await.unwrap();

    let price = ProductPatch {
        price_cents: Some(990),
        version: 1,
        ..ProductPatch::default()
    };
    let after = price.apply(&client, &1).await.unwrap();
    assert_eq!(after.price_cents, 990);
    assert_eq!(
        (after.sku.as_str(), after.note.as_deref()),
        ("MUG-01", Some("fragile"))
    );
    assert_eq!(after.version, 2);
    assert!(after.updated_at > before.updated_at);

    // Some(None) sets a nullable column to NULL.
    let note = ProductPatch {
        note: Some(None),
        sku: Some("MUG-02".into()),
        version: 2,
        ..ProductPatch::default()
    };
    let after = note.apply(&client, &1).await.unwrap();
    assert_eq!((after.sku.as_str(), after.note), ("MUG-02", None));
    assert_eq!(after.version, 3);

    // One statement for every shape of patch.
    assert!(ProductPatch::sql().starts_with("WITH updated AS (UPDATE \"products\" SET"));
}

#[tokio::test]
async fn two_edits_of_one_version_the_second_is_stale() {
    let db = TestDb::new().await;
    let client = db.client_with(PRODUCTS).await;
    let tab = |cents| ProductPatch {
        price_cents: Some(cents),
        version: 1,
        ..ProductPatch::default()
    };
    tab(990).apply(&client, &1).await.unwrap();
    let error = tab(1100).apply(&client, &1).await.unwrap_err();
    assert!(matches!(
        error,
        Error::Stale {
            model: "Product",
            current_version: 2
        }
    ));
    assert_eq!(
        error.to_string(),
        "Product has changed since it was read: it is now at version 2"
    );
    let error = tab(1100).apply(&client, &9).await.unwrap_err();
    assert!(matches!(error, Error::NotFound { model: "Product" }));
    assert_eq!(Product::find(&client, &1).await.unwrap().price_cents, 990);
}

#[tokio::test]
async fn update_checks_and_advances_the_version_too() {
    let db = TestDb::new().await;
    let client = db.client_with(PRODUCTS).await;
    let mut first = Product::find(&client, &1).await.unwrap();
    let mut second = first.clone();

    first.note = Some("handle with care".into());
    let first = first.update(&client).await.unwrap();
    assert_eq!(first.version, 2);

    second.price_cents = 1;
    let error = second.update(&client).await.unwrap_err();
    assert!(
        matches!(
            error,
            Error::Stale {
                current_version: 2,
                ..
            }
        ),
        "{error}"
    );
}

#[tokio::test]
async fn updated_at_is_the_transactions_time() {
    let db = TestDb::new().await;
    let mut client = db.client_with(PRODUCTS).await;
    let tx = client.transaction().await.unwrap();
    let one = ProductPatch {
        price_cents: Some(1),
        version: 1,
        ..ProductPatch::default()
    }
    .apply(&tx, &1)
    .await
    .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    let two = ProductPatch {
        price_cents: Some(2),
        version: 2,
        ..ProductPatch::default()
    }
    .apply(&tx, &1)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    // `now()` is the time the transaction started, for every statement in it.
    assert_eq!(one.updated_at, two.updated_at);
}

#[tokio::test]
async fn a_domain_column_takes_a_string_in_a_patch_but_not_in_an_update() {
    let db = TestDb::new().await;
    let client = db.client_with(PRODUCTS).await;
    client
        .batch_execute(
            "CREATE DOMAIN sku_code AS text CHECK (VALUE ~ '^[A-Z0-9-]+$');
             ALTER TABLE products ALTER COLUMN sku TYPE sku_code;",
        )
        .await
        .unwrap();
    // `CASE … THEN $2 ELSE sku END` types `$2` as the domain's base type, `text`.
    ProductPatch {
        sku: Some("CUP-01".into()),
        version: 1,
        ..ProductPatch::default()
    }
    .apply(&client, &1)
    .await
    .unwrap();
    // `SET "sku" = $1` types `$1` as the domain itself, which a String does not accept.
    let product = Product::find(&client, &1).await.unwrap();
    let error = product.update(&client).await.unwrap_err();
    let Error::Db(db_error) = &error else {
        panic!("a driver error: {error}")
    };
    assert_eq!(db_error.code(), None); // refused on the client, before sending
    assert_eq!(error.to_string(), "error serializing parameter 0");

    // The domain's check still runs on a patched value.
    let error = ProductPatch {
        sku: Some("not valid".into()),
        version: 2,
        ..ProductPatch::default()
    }
    .apply(&client, &1)
    .await
    .unwrap_err();
    let Error::Db(db_error) = &error else {
        panic!("a database error: {error}")
    };
    assert_eq!(db_error.code(), Some(&SqlState::CHECK_VIOLATION));
}

/// A model without a version: a patch has no version field, and a missing row is NotFound.
#[derive(Debug, FromRow, Table)]
#[shelf(table = "notes")]
struct Note {
    #[shelf(primary_key)]
    id: i64,
    body: String,
}

#[derive(Patch)]
#[shelf(model = Note)]
struct NotePatch<'a> {
    body: Option<&'a str>,
}

#[tokio::test]
async fn an_unversioned_patch_borrowing_its_value() {
    let db = TestDb::new().await;
    let client = db
        .client_with(
            "CREATE TABLE notes (id bigint PRIMARY KEY, body text NOT NULL);
                      INSERT INTO notes VALUES (1, 'first');",
        )
        .await;
    let body = String::from("second");
    let note = NotePatch { body: Some(&body) }
        .apply(&client, &1)
        .await
        .unwrap();
    assert_eq!(note.body, "second");
    assert!(NotePatch::sql().starts_with("UPDATE \"notes\" SET"));
    let error = NotePatch { body: None }
        .apply(&client, &2)
        .await
        .unwrap_err();
    assert!(matches!(error, Error::NotFound { model: "Note" }));
}

#[cfg(feature = "deadpool")]
#[tokio::test]
async fn a_patch_on_a_pooled_connection_is_prepared_once() {
    let db = TestDb::new().await;
    db.client_with(PRODUCTS).await;
    let pool = db.pool(1);
    let object = pool.get().await.unwrap();
    for (version, cents) in [(1, 990), (2, 980), (3, 970)] {
        ProductPatch {
            price_cents: Some(cents),
            version,
            ..ProductPatch::default()
        }
        .apply(&object, &1)
        .await
        .unwrap();
    }
    assert_eq!(object.statement_cache.size(), 1);
    assert_eq!(Product::find(&object, &1).await.unwrap().version, 4);
}
