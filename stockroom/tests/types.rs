mod common;

use common::TestDb;
use stockroom::domain::{Address, Cents, OrderStatus, Quantity, Sku};
use tokio_postgres::error::SqlState;

#[tokio::test]
async fn a_sku_goes_into_the_domain_and_comes_back_as_text() {
    let db = TestDb::new().await;
    let client = db.client().await;
    let sku = Sku::parse("CHAIR-01").unwrap();

    // `$1` targets the domain column, so tokio-postgres describes it as the domain `sku`.
    let statement = client
        .prepare("INSERT INTO products (sku, name, price_cents) VALUES ($1, 'Chair', $2)")
        .await
        .unwrap();
    assert_eq!(statement.params()[0].name(), "sku");
    client
        .execute(&statement, &[&sku, &Cents(4999)])
        .await
        .unwrap();

    // A result column of a domain type is described as the base type.
    let row = client
        .query_one("SELECT sku, price_cents FROM products", &[])
        .await
        .unwrap();
    assert_eq!(row.columns()[0].type_().name(), "text");
    assert_eq!(row.get::<_, Sku>("sku"), sku);
    assert_eq!(row.get::<_, Cents>("price_cents"), Cents(4999));
}

#[tokio::test]
async fn a_plain_str_is_refused_for_a_domain_parameter_unless_cast() {
    let db = TestDb::new().await;
    let client = db.client().await;
    let refused = client
        .execute(
            "INSERT INTO products (sku, name, price_cents) VALUES ($1, 'Lamp', 1)",
            &[&"LAMP-01"],
        )
        .await
        .unwrap_err();
    assert!(
        refused.as_db_error().is_none(),
        "refused on the client, before anything was sent"
    );
    let cause = std::error::Error::source(&refused).unwrap().to_string();
    assert!(
        cause.contains("cannot convert between the Rust type `&str` and the Postgres type `sku`"),
        "{cause}"
    );
    // A cast makes the parameter `text`; the server then converts it to the domain, and checks it.
    client
        .execute(
            "INSERT INTO products (sku, name, price_cents) VALUES ($1::text, 'Lamp', 1)",
            &[&"LAMP-01"],
        )
        .await
        .unwrap();
    let bad = client
        .execute(
            "INSERT INTO products (sku, name, price_cents) VALUES ($1::text, 'Lamp', 1)",
            &[&"lamp"],
        )
        .await
        .unwrap_err();
    let db_error = bad.as_db_error().unwrap();
    assert_eq!(*db_error.code(), SqlState::CHECK_VIOLATION);
    assert_eq!(db_error.constraint(), Some("sku_format"));
}

#[tokio::test]
async fn reading_a_sku_validates_it() {
    let db = TestDb::new().await;
    let client = db.client().await;
    let row = client
        .query_one("SELECT 'not a sku'::text AS sku", &[])
        .await
        .unwrap();
    assert!(row.try_get::<_, Sku>("sku").is_err());
}

#[tokio::test]
async fn enums_composites_domains_and_arrays_of_them() {
    let db = TestDb::new().await;
    let client = db.client().await;

    let statuses = vec![OrderStatus::Paid, OrderStatus::Cancelled];
    let row = client
        .query_one("SELECT $1::order_status[] AS s", &[&statuses])
        .await
        .unwrap();
    assert_eq!(row.get::<_, Vec<OrderStatus>>("s"), statuses);

    let address = Address {
        street: "Main 1".into(),
        city: "Berlin".into(),
        postcode: "10115".into(),
        country: "DE".into(),
    };
    let row = client
        .query_one(
            "SELECT $1::address AS a, ($1::address).city AS city",
            &[&address],
        )
        .await
        .unwrap();
    assert_eq!(row.get::<_, Address>("a"), address);
    assert_eq!(row.get::<_, &str>("city"), "Berlin");

    let three = Quantity::new(3).unwrap();
    let row = client
        .query_one("SELECT $1::positive_quantity AS q", &[&three])
        .await
        .unwrap();
    assert_eq!(row.get::<_, Quantity>("q"), three);

    let zero = client
        .query_one("SELECT 0::positive_quantity", &[])
        .await
        .unwrap_err();
    assert_eq!(
        zero.as_db_error().unwrap().constraint(),
        Some("positive_quantity_check")
    );
}

/// A migration adds an enum label before the code knows it. Thanks to `allow_mismatch`,
/// reading the labels the code does know keeps working on fresh connections; only a row
/// holding the new label fails to read.
#[tokio::test]
async fn a_new_enum_label_in_the_database_breaks_only_rows_that_use_it() {
    let db = TestDb::new().await;
    db.client()
        .await
        .batch_execute("ALTER TYPE order_status ADD VALUE 'refunded'")
        .await
        .unwrap();
    let fresh = db.client().await; // loads the type with its new label
    let paid: OrderStatus = fresh
        .query_one("SELECT 'paid'::order_status", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(paid, OrderStatus::Paid);
    let refunded = fresh
        .query_one("SELECT 'refunded'::order_status", &[])
        .await
        .unwrap();
    assert!(refunded.try_get::<_, OrderStatus>(0).is_err());
}
