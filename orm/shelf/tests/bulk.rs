//! `insert_many` and `copy_in` against PostgreSQL: a domain, an enum, JSON, an array column,
//! borrowed fields, the errors, and the connection still usable after them.

mod common;

use common::TestDb;
use postgres_types::{FromSql, ToSql};
use serde::{Deserialize, Serialize};
use shelf::{Error, FromRow, Insert, Table};

const SCHEMA: &str = "
    CREATE DOMAIN sku AS text CHECK (VALUE ~ '^[A-Z]+-[0-9]+$');
    CREATE TYPE status AS ENUM ('draft', 'live');
    CREATE TABLE products (
        id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
        sku sku NOT NULL UNIQUE,
        name text NOT NULL,
        status status NOT NULL DEFAULT 'draft',
        price_cents bigint NOT NULL,
        attributes jsonb NOT NULL DEFAULT '{}',
        tags text[] NOT NULL DEFAULT '{}'
    );";

#[derive(Debug, Clone, PartialEq, ToSql, FromSql)]
#[postgres(name = "sku")]
struct Sku(String);

#[derive(Debug, Clone, Copy, PartialEq, ToSql, FromSql)]
#[postgres(name = "status")]
enum Status {
    #[postgres(name = "draft")]
    Draft,
    #[postgres(name = "live")]
    Live,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Attributes {
    #[serde(default)] // the column's default is `{}`
    color: String,
}

#[derive(Debug, PartialEq, FromRow, Table)]
#[shelf(table = "products")]
struct Product {
    #[shelf(primary_key, generated)]
    id: i64,
    sku: Sku,
    name: String,
    status: Status,
    price_cents: i64,
    #[shelf(json)]
    attributes: Attributes,
    tags: Vec<String>,
}

/// A new product, borrowing its name; `tags` is left to its default.
#[derive(Insert)]
#[shelf(model = Product)]
struct NewProduct<'a> {
    sku: Sku,
    name: &'a str,
    status: Status,
    price_cents: i64,
    #[shelf(json)]
    attributes: Attributes,
}

/// With an array column: `unnest` cannot take it, COPY can.
#[derive(Insert)]
#[shelf(model = Product)]
struct NewTagged {
    sku: Sku,
    name: String,
    price_cents: i64,
    tags: Vec<String>,
}

fn names(n: usize) -> Vec<String> {
    (0..n).map(|i| format!("Product {i}")).collect()
}

fn batch(names: &[String]) -> Vec<NewProduct<'_>> {
    names
        .iter()
        .enumerate()
        .map(|(i, name)| NewProduct {
            sku: Sku(format!("SKU-{i}")),
            name,
            status: if i % 2 == 0 {
                Status::Live
            } else {
                Status::Draft
            },
            price_cents: i as i64 * 100,
            attributes: Attributes {
                color: format!("c{}", i % 3),
            },
        })
        .collect()
}

#[tokio::test]
async fn insert_many_writes_a_batch_in_one_statement_and_returns_it() {
    let db = TestDb::new().await;
    let client = db.client_with(SCHEMA).await;
    let names = names(1000);
    let rows = batch(&names);
    let products = NewProduct::insert_many(&client, &rows).await.unwrap();
    assert_eq!(products.len(), 1000);
    // The rows come back as inserted: keys generated in order, every field as written.
    assert!(products.windows(2).all(|w| w[0].id < w[1].id));
    let last = &products[999];
    assert_eq!(last.sku, Sku("SKU-999".into()));
    assert_eq!(last.name, "Product 999");
    assert_eq!(
        (
            last.status,
            last.price_cents,
            last.attributes.color.as_str()
        ),
        (Status::Draft, 99_900, "c0")
    );
    assert!(last.tags.is_empty()); // the column's default
    assert_eq!(Product::count(&client).await.unwrap(), 1000);
    assert!(
        NewProduct::insert_many(&client, &[])
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn copy_in_loads_a_batch_and_counts_it() {
    let db = TestDb::new().await;
    let client = db.client_with(SCHEMA).await;
    let names = names(1000);
    assert_eq!(
        NewProduct::copy_in(&client, &batch(&names)).await.unwrap(),
        1000
    );
    let tagged: Vec<NewTagged> = (0..3)
        .map(|i| NewTagged {
            sku: Sku(format!("TAG-{i}")),
            name: format!("Tagged {i}"),
            price_cents: 100,
            tags: vec!["sale".into(), format!("t{i}")],
        })
        .collect();
    assert_eq!(NewTagged::copy_in(&client, &tagged).await.unwrap(), 3);
    let tag = Product::query()
        .filter(Product::TAGS.contains("t2"))
        .first(&client)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(tag.tags, ["sale", "t2"]);
    assert_eq!(NewProduct::copy_in(&client, &[]).await.unwrap(), 0);
}

#[tokio::test]
async fn an_array_column_cannot_go_through_unnest() {
    let db = TestDb::new().await;
    let client = db.client_with(SCHEMA).await;
    let rows = [NewTagged {
        sku: Sku("TAG-1".into()),
        name: "Tagged".into(),
        price_cents: 100,
        tags: vec!["sale".into()],
    }];
    let error = NewTagged::insert_many(&client, &rows).await.unwrap_err();
    assert!(matches!(
        error,
        Error::Unsupported {
            model: "Product",
            ..
        }
    ));
    assert_eq!(
        error.to_string(),
        "Product: insert_many cannot send the array column \"tags\" through unnest, which \
         would flatten an array of arrays; use copy_in, or insert in a loop"
    );
}

#[tokio::test]
async fn a_bad_row_fails_the_batch_and_the_connection_lives() {
    let db = TestDb::new().await;
    let client = db.client_with(SCHEMA).await;
    let names = names(3);
    let mut rows = batch(&names);
    rows[2].sku = Sku("not a sku".into());
    for error in [
        NewProduct::insert_many(&client, &rows)
            .await
            .map(|_| ())
            .unwrap_err(),
        NewProduct::copy_in(&client, &rows)
            .await
            .map(|_| ())
            .unwrap_err(),
    ] {
        let Error::Db(error) = error else {
            panic!("{error}")
        };
        assert_eq!(
            error.code(),
            Some(&tokio_postgres::error::SqlState::CHECK_VIOLATION)
        );
    }
    // Nothing was written, and the connection still works.
    assert_eq!(Product::count(&client).await.unwrap(), 0);
}

#[tokio::test]
async fn a_missing_column_is_caught_before_the_copy_starts() {
    let db = TestDb::new().await;
    let client = db.client_with(SCHEMA).await;
    client
        .batch_execute("ALTER TABLE products DROP COLUMN attributes")
        .await
        .unwrap();
    let names = names(2);
    let error = NewProduct::copy_in(&client, &batch(&names))
        .await
        .unwrap_err();
    let Error::Db(error) = error else {
        panic!("{error}")
    };
    assert_eq!(
        error.as_db_error().unwrap().message(),
        "column \"attributes\" of relation \"products\" does not exist"
    );
    // A COPY rejected as it starts would have left the connection unusable (issue #789).
    assert_eq!(Product::count(&client).await.unwrap(), 0);
}

/// Every column has a default: a row of nothing.
#[derive(Debug, PartialEq, FromRow, Table)]
#[shelf(table = "events")]
struct Event {
    #[shelf(primary_key, generated)]
    id: i64,
    kind: String,
}

#[derive(Insert)]
#[shelf(model = Event)]
struct NewEvent {}

#[tokio::test]
async fn a_struct_with_no_columns_inserts_defaults() {
    let db = TestDb::new().await;
    let client = db
        .client_with(
            "CREATE TABLE events (id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY, \
             kind text NOT NULL DEFAULT 'ping')",
        )
        .await;
    let events = NewEvent::insert_many(&client, &[NewEvent {}, NewEvent {}])
        .await
        .unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[1].kind, "ping");
    let error = NewEvent::copy_in(&client, &[NewEvent {}])
        .await
        .unwrap_err();
    assert!(matches!(error, Error::Unsupported { model: "Event", .. }));
}

#[tokio::test]
async fn bulk_writes_take_a_pooled_connection_and_a_transaction() {
    let db = TestDb::new().await;
    db.client_with(SCHEMA).await;
    let pool = db.pool(1);
    let mut client = pool.get().await.unwrap();
    let names = names(10);
    let rows = batch(&names);
    let tx = client.transaction().await.unwrap();
    NewProduct::insert_many(&tx, &rows[..5]).await.unwrap();
    NewProduct::copy_in(&tx, &rows[5..]).await.unwrap();
    assert_eq!(Product::count(&tx).await.unwrap(), 10);
    tx.rollback().await.unwrap();
    assert_eq!(Product::count(&client).await.unwrap(), 0);
}
