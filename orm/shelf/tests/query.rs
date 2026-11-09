//! The query builder against PostgreSQL: the SQL each filter renders, its parameters, and the
//! rows it returns.

mod common;

use common::TestDb;
use shelf::query::{Expr, Query};
use shelf::{FromRow, Table};

const PRODUCTS: &str = "
    CREATE DOMAIN sku AS text CHECK (VALUE ~ '^[A-Z]+-[0-9]+$');
    CREATE TABLE products (
        id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
        sku sku NOT NULL UNIQUE,
        name text NOT NULL,
        price_cents bigint NOT NULL,
        tags text[] NOT NULL DEFAULT '{}',
        note text,
        stock integer
    );
    INSERT INTO products (sku, name, price_cents, tags, note, stock) VALUES
        ('TEE-01', 'Tee', 2499, '{sale,cotton}', NULL, 10),
        ('MUG-01', 'Mug', 1299, '{}', 'fragile', NULL),
        ('CAP-01', 'Cap', 1899, '{sale}', NULL, 0),
        ('HAT-01', 'Hat', 999, '{wool}', 'one size', 3),
        ('BAG-01', 'Bag', 4599, '{sale,leather}', NULL, 1);";

#[derive(Debug, PartialEq, FromRow, Table)]
#[shelf(table = "products")]
struct Product {
    #[shelf(primary_key, generated)]
    id: i64,
    sku: String,
    name: String,
    price_cents: i64,
    tags: Vec<String>,
    note: Option<String>,
    stock: Option<i32>,
}

fn skus(products: &[Product]) -> Vec<&str> {
    products.iter().map(|p| p.sku.as_str()).collect()
}

#[test]
fn a_query_renders_one_statement_with_numbered_parameters() {
    let query = Product::query()
        .filter(
            Product::PRICE_CENTS
                .gt(1_000)
                .and(Product::TAGS.contains("sale")),
        )
        .order_by(Product::SKU.asc())
        .limit(20);
    assert_eq!(
        query.sql(),
        r#"SELECT "id", "sku", "name", "price_cents", "tags", "note", "stock" FROM "products" WHERE "price_cents" > $1 AND "tags" @> $2 ORDER BY "sku" LIMIT $3"#
    );
    assert_eq!(format!("{:?}", query.params()), r#"[1000, ["sale"], 20]"#);
}

#[test]
fn or_and_not_keep_their_meaning() {
    let query = Product::query()
        .filter(
            Product::PRICE_CENTS
                .lt(1_000)
                .or(Product::NOTE.is_not_null().and(Product::STOCK.gt(0))),
        )
        .filter(!Product::SKU.eq("MUG-01"));
    assert!(
        query.sql().ends_with(
            r#"WHERE ("price_cents" < $1 OR ("note" IS NOT NULL AND "stock" > $2)) AND NOT ("sku" = $3) ORDER BY "id""#
        ),
        "{}",
        query.sql()
    );
    let query = Product::query().filter(
        Product::STOCK
            .is_null()
            .or(Product::STOCK.eq(0))
            .and(Product::NAME.like("%a%")),
    );
    assert!(
        query.sql().ends_with(
            r#"WHERE ("stock" IS NULL OR "stock" = $1) AND "name" LIKE $2 ORDER BY "id""#
        ),
        "{}",
        query.sql()
    );
}

#[tokio::test]
async fn filters_return_the_rows_they_describe() {
    let db = TestDb::new().await;
    let client = db.client_with(PRODUCTS).await;

    let sale = Product::query()
        .filter(
            Product::PRICE_CENTS
                .gt(1_000)
                .and(Product::TAGS.contains("sale")),
        )
        .order_by(Product::SKU.asc())
        .all(&client)
        .await
        .unwrap();
    assert_eq!(skus(&sale), ["BAG-01", "CAP-01", "TEE-01"]);

    // `&str` for a domain column: a comparison types the parameter as the base type, text.
    let mug = Product::query()
        .filter(Product::SKU.eq("MUG-01"))
        .first(&client)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(mug.note.as_deref(), Some("fragile"));

    let picked = Product::query()
        .filter(Product::SKU.any(["HAT-01", "CAP-01", "NONE-01"]))
        .all(&client)
        .await
        .unwrap();
    assert_eq!(skus(&picked), ["CAP-01", "HAT-01"]); // key order

    let mid = Product::query()
        .filter(Product::PRICE_CENTS.between(1_000, 2_500))
        .order_by(Product::PRICE_CENTS.desc())
        .all(&client)
        .await
        .unwrap();
    assert_eq!(skus(&mid), ["TEE-01", "CAP-01", "MUG-01"]);

    let unknown_stock = Product::query()
        .filter(Product::STOCK.is_null().or(Product::STOCK.eq(0)))
        .all(&client)
        .await
        .unwrap();
    assert_eq!(skus(&unknown_stock), ["MUG-01", "CAP-01"]);

    let noted = Product::query()
        .filter(Product::NOTE.like("%size%"))
        .all(&client)
        .await
        .unwrap();
    assert_eq!(skus(&noted), ["HAT-01"]);

    let not_sale = Product::query()
        .filter(!Product::TAGS.contains("sale"))
        .all(&client)
        .await
        .unwrap();
    assert_eq!(skus(&not_sale), ["MUG-01", "HAT-01"]);
}

#[tokio::test]
async fn count_and_exists_use_the_filter_alone() {
    let db = TestDb::new().await;
    let client = db.client_with(PRODUCTS).await;
    let sale = Product::query()
        .filter(Product::TAGS.contains("sale"))
        .order_by(Product::SKU.asc())
        .limit(1);
    assert_eq!(sale.count(&client).await.unwrap(), 3);
    assert!(sale.exists(&client).await.unwrap());
    let none = Product::query().filter(Product::PRICE_CENTS.gt(1_000_000));
    assert_eq!(none.count(&client).await.unwrap(), 0);
    assert!(!none.exists(&client).await.unwrap());
    assert!(none.first(&client).await.unwrap().is_none());
    // No filter at all: the whole table, in key order, as `all` returns it.
    let all = Product::query().all(&client).await.unwrap();
    assert_eq!(all, Product::all(&client).await.unwrap());
}

#[tokio::test]
async fn keyset_pages_walk_the_table_in_order() {
    let db = TestDb::new().await;
    let client = db.client_with(PRODUCTS).await;
    let mut seen = Vec::new();
    let mut last: Option<String> = None;
    loop {
        let mut query = Product::query().limit(2);
        if let Some(sku) = &last {
            query = query.after(Product::SKU, sku);
        } else {
            query = query.order_by(Product::SKU.asc());
        }
        let page = query.all(&client).await.unwrap();
        let Some(end) = page.last() else { break };
        last = Some(end.sku.clone());
        seen.extend(page.into_iter().map(|p| p.sku));
    }
    assert_eq!(seen, ["BAG-01", "CAP-01", "HAT-01", "MUG-01", "TEE-01"]);
    // `after` orders by its column itself, and only once.
    let query = Product::query().after(Product::SKU, "CAP-01");
    assert!(query.sql().ends_with(r#"WHERE "sku" > $1 ORDER BY "sku""#));
    let query = Product::query()
        .order_by(Product::SKU.asc())
        .after(Product::SKU, "CAP-01");
    assert!(query.sql().ends_with(r#"ORDER BY "sku""#));
    // OFFSET pages are parameters too.
    let page = Product::query().limit(2).offset(4);
    assert!(page.sql().ends_with(r#"ORDER BY "id" LIMIT $1 OFFSET $2"#));
    assert_eq!(skus(&page.all(&client).await.unwrap()), ["BAG-01"]);
}

#[tokio::test]
async fn raw_sql_is_renumbered_among_the_builders_parameters() {
    let db = TestDb::new().await;
    let client = db.client_with(PRODUCTS).await;
    let query = Product::query()
        .filter(Product::PRICE_CENTS.gt(1_000))
        .filter(
            Expr::raw(r#"lower("name") = lower($1) OR "sku" LIKE $2 || '%'"#)
                .bind("TEE")
                .bind("BAG"),
        );
    assert!(
        query.sql().ends_with(
            r#"WHERE "price_cents" > $1 AND (lower("name") = lower($2) OR "sku" LIKE $3 || '%') ORDER BY "id""#
        ),
        "{}",
        query.sql()
    );
    assert_eq!(
        skus(&query.all(&client).await.unwrap()),
        ["TEE-01", "BAG-01"]
    );
}

#[test]
#[should_panic(expected = "refers to $2, but has 1 bound value(s)")]
fn raw_sql_without_a_value_for_its_placeholder_panics() {
    let _ = Product::query().filter(Expr::raw("x = $1 OR y = $2").bind(1_i64));
}

#[tokio::test]
async fn a_query_is_built_in_one_task_and_run_in_another_through_a_pool() {
    let db = TestDb::new().await;
    db.client_with(PRODUCTS).await;
    let pool = db.pool(2);
    let query: Query<Product> = Product::query().filter(Product::TAGS.contains("sale"));
    let count = tokio::spawn(async move {
        let client = pool.get().await.unwrap();
        query.count(&client).await.unwrap()
    })
    .await
    .unwrap();
    assert_eq!(count, 3);
}

/// Compiles only if the builder's futures are `Send` for any executor, as `tokio::spawn` and
/// axum need (orm/PLAN.md §12 #28): inherent `async fn`s leak their auto traits.
#[allow(dead_code)]
fn query_futures_are_send<E: shelf::Executor>(db: &E) {
    fn send<T: Send>(_: T) {}
    let query = Product::query().filter(Product::TAGS.contains("sale"));
    send(query.all(db));
    send(query.first(db));
    send(query.count(db));
    send(query.exists(db));
}
