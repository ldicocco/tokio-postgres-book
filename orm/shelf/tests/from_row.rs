//! `FromRow` against PostgreSQL: each attribute, tuples, generics, the fetch helpers, and the
//! errors, including the check that runs before a query does.

mod common;

use common::TestDb;
use serde::Deserialize;
use shelf::{Error, FromRow, Mapping, fetch_all, fetch_one, fetch_opt};

const PRODUCTS: &str = "
    CREATE TABLE products (
        id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
        sku text NOT NULL UNIQUE,
        type text NOT NULL,
        price_cents bigint NOT NULL,
        tags text[],
        attributes jsonb NOT NULL DEFAULT '{}'
    );
    INSERT INTO products (sku, type, price_cents, tags, attributes) VALUES
        ('MUG-01', 'kitchen', 1299, '{blue,sale}', '{\"colour\": \"blue\", \"ml\": 350}'),
        ('TEE-02', 'clothing', 2499, NULL, '{\"colour\": \"white\"}');";

#[derive(Debug, PartialEq, FromRow)]
struct Product {
    id: i64,
    sku: String,
    r#type: String,
    price_cents: i64,
    tags: Option<Vec<String>>,
}

#[tokio::test]
async fn fields_are_read_by_name_whatever_the_column_order() {
    let db = TestDb::new().await;
    let client = db.client_with(PRODUCTS).await;
    let products: Vec<Product> = fetch_all(
        &client,
        "SELECT tags, price_cents, type, sku, id FROM products ORDER BY id",
        &[],
    )
    .await
    .unwrap();
    assert_eq!(products.len(), 2);
    assert_eq!(products[0].sku, "MUG-01");
    assert_eq!(products[0].r#type, "kitchen"); // `r#type` reads the column `type`
    assert_eq!(products[0].tags, Some(vec!["blue".into(), "sale".into()]));
    assert_eq!(products[1].tags, None); // NULL into an Option
    assert_eq!(products[1].price_cents, 2499);
}

#[tokio::test]
async fn rename_all_sets_the_convention_and_rename_overrides_it() {
    #[derive(Debug, FromRow)]
    #[shelf(rename_all = "camelCase")]
    struct LegacyOrder {
        order_id: i64,
        unit_price_cents: i64,
        #[shelf(rename = "STATUS")]
        status: String,
    }

    let db = TestDb::new().await;
    let client = db
        .client_with(
            r#"CREATE TABLE legacy ("orderId" bigint, "unitPriceCents" bigint, "STATUS" text);
               INSERT INTO legacy VALUES (7, 990, 'paid');"#,
        )
        .await;
    let order: LegacyOrder = fetch_one(&client, "SELECT * FROM legacy", &[])
        .await
        .unwrap();
    assert_eq!(
        (
            order.order_id,
            order.unit_price_cents,
            order.status.as_str()
        ),
        (7, 990, "paid")
    );
}

#[tokio::test]
async fn flatten_reads_a_nested_struct_and_skip_reads_nothing() {
    #[derive(Debug, PartialEq, FromRow)]
    struct Price {
        price_cents: i64,
        currency: String,
    }

    #[derive(Debug, FromRow)]
    struct Line {
        sku: String,
        #[shelf(flatten)]
        price: Price,
        #[shelf(skip)]
        selected: bool,
    }

    let db = TestDb::new().await;
    let client = db.client_with(PRODUCTS).await;
    let line: Line = fetch_one(
        &client,
        "SELECT sku, price_cents, 'EUR' AS currency FROM products WHERE sku = $1",
        &[&"MUG-01"],
    )
    .await
    .unwrap();
    assert_eq!(line.sku, "MUG-01");
    assert_eq!(
        line.price,
        Price {
            price_cents: 1299,
            currency: "EUR".into()
        }
    );
    assert!(!line.selected); // Default::default()
}

#[tokio::test]
async fn json_reads_a_jsonb_column_through_serde() {
    #[derive(Debug, PartialEq, Deserialize)]
    struct Attributes {
        colour: String,
        ml: Option<u32>,
    }

    #[derive(Debug, FromRow)]
    struct WithAttributes {
        sku: String,
        #[shelf(json)]
        attributes: Attributes,
    }

    let db = TestDb::new().await;
    let client = db.client_with(PRODUCTS).await;
    let rows: Vec<WithAttributes> = fetch_all(
        &client,
        "SELECT sku, attributes FROM products ORDER BY id",
        &[],
    )
    .await
    .unwrap();
    assert_eq!(rows[0].sku, "MUG-01");
    assert_eq!(
        rows[0].attributes,
        Attributes {
            colour: "blue".into(),
            ml: Some(350)
        }
    );
    assert_eq!(rows[1].attributes.ml, None);
}

#[tokio::test]
async fn json_reads_an_sql_null_as_json_null() {
    #[derive(Debug, PartialEq, Deserialize)]
    struct Attributes {
        colour: String,
    }

    #[derive(Debug, FromRow)]
    struct Optional {
        #[shelf(json)]
        attributes: Option<Attributes>,
    }

    #[derive(Debug, FromRow)]
    struct Required {
        #[shelf(json)]
        attributes: Attributes,
    }

    let db = TestDb::new().await;
    let client = db.client().await;
    let document = r#"SELECT '{"colour": "blue"}'::jsonb AS attributes"#;
    let json_null = "SELECT 'null'::jsonb AS attributes";
    let sql_null = "SELECT NULL::jsonb AS attributes";

    // An Option reads a document as Some, and either kind of null as None.
    let blue = Some(Attributes {
        colour: "blue".into(),
    });
    for (sql, expected) in [(document, blue), (json_null, None), (sql_null, None)] {
        let row: Optional = fetch_one(&client, sql, &[]).await.unwrap();
        assert_eq!(row.attributes, expected, "{sql}");
    }

    // A type that refuses JSON null refuses an SQL NULL too, as a NULL error.
    let row: Required = fetch_one(&client, document, &[]).await.unwrap();
    assert_eq!(row.attributes.colour, "blue");
    let error = fetch_one::<Required>(&client, sql_null, &[])
        .await
        .unwrap_err();
    assert!(matches!(
        &error,
        Error::Mapping {
            problem: Mapping::Null { .. },
            ..
        }
    ));
    assert_eq!(
        error.to_string(),
        "Required.attributes: column \"attributes\" is NULL; read it into an Option"
    );
}

/// Cents that are never negative: the conversion refuses what the column allows.
#[derive(Debug, PartialEq)]
struct Cents(u64);

impl TryFrom<i64> for Cents {
    type Error = std::num::TryFromIntError;
    fn try_from(value: i64) -> Result<Self, Self::Error> {
        u64::try_from(value).map(Cents)
    }
}

#[derive(Debug, FromRow)]
struct Priced {
    #[shelf(try_from = i64, rename = "price_cents")]
    price: Cents,
}

#[tokio::test]
async fn try_from_reads_one_type_and_converts_it() {
    let db = TestDb::new().await;
    let client = db.client_with(PRODUCTS).await;
    let priced: Priced = fetch_one(
        &client,
        "SELECT price_cents FROM products WHERE sku = 'MUG-01'",
        &[],
    )
    .await
    .unwrap();
    assert_eq!(priced.price, Cents(1299));

    // A value the conversion refuses fails that row, naming the field and the column.
    let error = fetch_one::<Priced>(&client, "SELECT -5::bigint AS price_cents", &[])
        .await
        .unwrap_err();
    assert!(matches!(
        &error,
        Error::Mapping {
            problem: Mapping::Decode { .. },
            ..
        }
    ));
    assert_eq!(
        error.to_string(),
        "Priced.price: column \"price_cents\": number too small to fit in target type"
    );
}

#[tokio::test]
async fn type_parameters_get_the_bounds_their_fields_need() {
    #[derive(Debug, FromRow)]
    struct Counted<T> {
        n: i64,
        #[shelf(flatten)]
        item: T,
    }

    #[derive(Debug, FromRow)]
    struct Sku {
        sku: String,
    }

    let db = TestDb::new().await;
    let client = db.client_with(PRODUCTS).await;
    let rows: Vec<Counted<Sku>> = fetch_all(
        &client,
        "SELECT sku, count(*) OVER () AS n FROM products ORDER BY sku",
        &[],
    )
    .await
    .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!((rows[0].n, rows[0].item.sku.as_str()), (2, "MUG-01"));
}

#[tokio::test]
async fn tuples_are_read_by_position() {
    let db = TestDb::new().await;
    let client = db.client_with(PRODUCTS).await;
    let pairs: Vec<(String, i64)> = fetch_all(
        &client,
        "SELECT sku, price_cents FROM products ORDER BY id",
        &[],
    )
    .await
    .unwrap();
    assert_eq!(pairs, [("MUG-01".into(), 1299), ("TEE-02".into(), 2499)]);

    let error = fetch_all::<(String, i64)>(&client, "SELECT sku FROM products", &[])
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "tuple.1: the result has no column \"#1\""
    );
}

#[tokio::test]
async fn fetch_opt_and_fetch_one_count_their_rows() {
    let db = TestDb::new().await;
    let client = db.client_with(PRODUCTS).await;
    let sql = "SELECT * FROM products WHERE sku = $1";
    let none: Option<Product> = fetch_opt(&client, sql, &[&"NOPE"]).await.unwrap();
    assert_eq!(none, None);
    let some: Option<Product> = fetch_opt(&client, sql, &[&"TEE-02"]).await.unwrap();
    assert_eq!(some.unwrap().sku, "TEE-02");
    let error = fetch_one::<Product>(&client, sql, &[&"NOPE"])
        .await
        .unwrap_err();
    assert!(matches!(error, Error::Db(_)), "{error:?}");
}

#[tokio::test]
async fn a_transaction_works_too_and_from_rows_reads_rows_already_fetched() {
    let db = TestDb::new().await;
    let mut client = db.client_with(PRODUCTS).await;
    let tx = client.transaction().await.unwrap();
    tx.execute(
        "UPDATE products SET price_cents = price_cents + 1 WHERE sku = 'MUG-01'",
        &[],
    )
    .await
    .unwrap();
    let products: Vec<Product> = fetch_all(&tx, "SELECT * FROM products ORDER BY id", &[])
        .await
        .unwrap();
    assert_eq!(products[0].price_cents, 1300); // the transaction's own write

    let rows = tx
        .query("SELECT * FROM products ORDER BY id", &[])
        .await
        .unwrap();
    let again: Vec<Product> = shelf::from_rows(&rows).unwrap();
    assert_eq!(again, products);
    tx.rollback().await.unwrap();
}

// ── Errors ───────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn a_missing_column_fails_before_the_query_runs() {
    let db = TestDb::new().await;
    let client = db.client_with(PRODUCTS).await;
    // The INSERT would add a row; the check fails on the prepared statement, before it runs.
    let error = fetch_all::<Product>(
        &client,
        "INSERT INTO products (sku, type, price_cents) VALUES ('NEW-03', 'x', 1)
         RETURNING id, sku, price_cents, tags",
        &[],
    )
    .await
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Product.type: the result has no column \"type\""
    );
    let count: i64 = client
        .query_one("SELECT count(*) FROM products", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(count, 2, "the INSERT did not run");
}

#[tokio::test]
async fn a_wrong_type_is_reported_even_for_an_empty_result() {
    let db = TestDb::new().await;
    let client = db.client_with(PRODUCTS).await;
    let error = fetch_all::<Product>(
        &client,
        "SELECT id, sku, type, price_cents::text AS price_cents, tags FROM products WHERE false",
        &[],
    )
    .await
    .unwrap_err();
    match &error {
        Error::Mapping {
            model: "Product",
            field: "price_cents",
            problem:
                Mapping::WrongType {
                    sql_type,
                    rust_type,
                    ..
                },
        } => {
            assert_eq!(sql_type, &tokio_postgres::types::Type::TEXT);
            assert_eq!(rust_type, "i64");
        }
        other => panic!("unexpected {other:?}"),
    }
    assert_eq!(
        error.to_string(),
        "Product.price_cents: column \"price_cents\" has type text, which cannot be read into i64"
    );
}

#[tokio::test]
async fn a_null_in_a_field_that_is_not_an_option_names_the_column() {
    #[derive(Debug, FromRow)]
    struct Tagged {
        #[allow(dead_code)]
        tags: Vec<String>,
    }

    let db = TestDb::new().await;
    let client = db.client_with(PRODUCTS).await;
    let error = fetch_all::<Tagged>(&client, "SELECT tags FROM products ORDER BY id", &[])
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Tagged.tags: column \"tags\" is NULL; read it into an Option"
    );
}

#[tokio::test]
async fn names_match_exactly_unlike_row_try_get() {
    #[derive(Debug, FromRow)]
    struct Upper {
        #[allow(dead_code)]
        sku: String,
    }

    let db = TestDb::new().await;
    let client = db.client().await;
    let row = client
        .query_one(r#"SELECT 'MUG-01' AS "SKU""#, &[])
        .await
        .unwrap();
    // tokio-postgres falls back to an ASCII case-insensitive match...
    assert_eq!(row.try_get::<_, String>("sku").unwrap(), "MUG-01");
    // ...shelf does not: PostgreSQL folds unquoted names itself, so a quoted "SKU" is another
    // column.
    let error = Upper::from_row(&row).unwrap_err();
    assert_eq!(
        error.to_string(),
        "Upper.sku: the result has no column \"sku\""
    );
}
