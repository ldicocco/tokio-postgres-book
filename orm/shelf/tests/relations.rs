//! `has_many` and `belongs_to` against PostgreSQL: what each generated method returns, and that
//! a loader sends one query however many rows it loads for.

mod common;

use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};

use common::TestDb;
use shelf::{Executor, FromRow, Table};
use tokio_postgres::types::ToSql;
use tokio_postgres::{Client, CopyInSink, Row, RowStream, Statement};

const SHOP: &str = "
    CREATE TABLE stores (id bigint PRIMARY KEY, name text NOT NULL);
    CREATE TABLE coupons (code text PRIMARY KEY, percent integer NOT NULL);
    CREATE TABLE orders (
        id bigint PRIMARY KEY,
        store_id bigint NOT NULL REFERENCES stores,
        coupon text REFERENCES coupons
    );
    CREATE TABLE order_lines (
        id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
        order_id bigint NOT NULL REFERENCES orders,
        sku text NOT NULL,
        quantity integer NOT NULL
    );
    INSERT INTO stores VALUES (1, 'Berlin'), (2, 'Hamburg');
    INSERT INTO coupons VALUES ('TEN', 10);
    INSERT INTO orders VALUES (10, 1, NULL), (11, 2, 'TEN'), (12, 1, NULL);
    INSERT INTO order_lines (order_id, sku, quantity) VALUES
        (10, 'TEE-01', 1), (11, 'MUG-01', 2), (10, 'CAP-01', 3), (11, 'TEE-01', 1);";

#[derive(Debug, PartialEq, FromRow, Table)]
#[shelf(table = "stores", has_many(Order, key = store_id, as = orders))]
struct Store {
    #[shelf(primary_key)]
    id: i64,
    name: String,
}

#[derive(Debug, PartialEq, FromRow, Table)]
#[shelf(table = "orders")]
#[shelf(belongs_to(Store, key = store_id), belongs_to(Coupon, key = coupon))]
#[shelf(has_many(OrderLine, key = order_id, as = lines))]
struct Order {
    #[shelf(primary_key)]
    id: i64,
    store_id: i64,
    coupon: Option<String>,
}

#[derive(Debug, PartialEq, FromRow, Table)]
#[shelf(table = "order_lines", belongs_to(Order, key = order_id))]
struct OrderLine {
    #[shelf(primary_key, generated)]
    id: i64,
    order_id: i64,
    sku: String,
    quantity: i32,
}

#[derive(Debug, PartialEq, FromRow, Table)]
#[shelf(table = "coupons")]
struct Coupon {
    #[shelf(primary_key)]
    code: String,
    percent: i32,
}

/// A client that counts the statements it runs: the round trips a loader costs, without
/// timing anything. `Executor` is not sealed, so a test (or an application) can wrap one.
struct Counting<'c> {
    client: &'c Client,
    runs: AtomicUsize,
}

impl<'c> Counting<'c> {
    fn new(client: &'c Client) -> Self {
        Counting {
            client,
            runs: AtomicUsize::new(0),
        }
    }

    fn runs(&self) -> usize {
        self.runs.swap(0, Ordering::SeqCst)
    }

    fn count(&self) {
        self.runs.fetch_add(1, Ordering::SeqCst);
    }
}

impl Executor for Counting<'_> {
    fn prepare(
        &self,
        sql: &str,
    ) -> impl Future<Output = Result<Statement, tokio_postgres::Error>> + Send {
        self.client.prepare(sql)
    }

    fn query(
        &self,
        statement: &Statement,
        params: &[&(dyn ToSql + Sync)],
    ) -> impl Future<Output = Result<Vec<Row>, tokio_postgres::Error>> + Send {
        self.count();
        self.client.query(statement, params)
    }

    fn query_one(
        &self,
        statement: &Statement,
        params: &[&(dyn ToSql + Sync)],
    ) -> impl Future<Output = Result<Row, tokio_postgres::Error>> + Send {
        self.count();
        self.client.query_one(statement, params)
    }

    fn query_opt(
        &self,
        statement: &Statement,
        params: &[&(dyn ToSql + Sync)],
    ) -> impl Future<Output = Result<Option<Row>, tokio_postgres::Error>> + Send {
        self.count();
        self.client.query_opt(statement, params)
    }

    fn execute(
        &self,
        statement: &Statement,
        params: &[&(dyn ToSql + Sync)],
    ) -> impl Future<Output = Result<u64, tokio_postgres::Error>> + Send {
        self.count();
        self.client.execute(statement, params)
    }

    fn query_raw(
        &self,
        statement: &Statement,
        params: &[&(dyn ToSql + Sync)],
    ) -> impl Future<Output = Result<RowStream, tokio_postgres::Error>> + Send {
        self.count();
        self.client.query_raw(statement, params.iter().copied())
    }

    fn copy_in(
        &self,
        statement: &Statement,
    ) -> impl Future<Output = Result<CopyInSink<bytes::Bytes>, tokio_postgres::Error>> + Send {
        self.count();
        self.client.copy_in(statement)
    }
}

fn skus(lines: &[OrderLine]) -> Vec<&str> {
    lines.iter().map(|l| l.sku.as_str()).collect()
}

#[tokio::test]
async fn has_many_loads_one_parents_rows_or_every_parents_in_one_query() {
    let db = TestDb::new().await;
    let client = db.client_with(SHOP).await;
    let db = Counting::new(&client);

    let orders = Order::all(&db).await.unwrap();
    db.runs();
    let lines = orders[0].lines(&db).await.unwrap();
    assert_eq!(skus(&lines), ["TEE-01", "CAP-01"]); // in the lines' key order
    assert_eq!(db.runs(), 1);

    let lines = Order::load_lines(&db, &orders).await.unwrap();
    assert_eq!(db.runs(), 1);
    let per_order: Vec<Vec<&str>> = lines.iter().map(|l| skus(l)).collect();
    assert_eq!(
        per_order,
        [vec!["TEE-01", "CAP-01"], vec!["MUG-01", "TEE-01"], vec![]]
    );

    let berlin = Store::find(&db, &1).await.unwrap();
    let ids: Vec<i64> = berlin
        .orders(&db)
        .await
        .unwrap()
        .iter()
        .map(|o| o.id)
        .collect();
    assert_eq!(ids, [10, 12]);

    // Nothing to load for: no query at all.
    db.runs();
    assert!(Order::load_lines(&db, &[]).await.unwrap().is_empty());
    assert_eq!(db.runs(), 0);
}

#[tokio::test]
async fn belongs_to_loads_the_parent_and_each_parent_once() {
    let db = TestDb::new().await;
    let client = db.client_with(SHOP).await;
    let db = Counting::new(&client);

    let lines = OrderLine::all(&db).await.unwrap();
    let order = lines[0].order(&db).await.unwrap();
    assert_eq!(order.id, 10);

    db.runs();
    let orders = OrderLine::load_order(&db, &lines).await.unwrap();
    assert_eq!(db.runs(), 1);
    let mut ids: Vec<i64> = orders.keys().copied().collect();
    ids.sort();
    assert_eq!(ids, [10, 11]); // four lines, two orders
    for line in &lines {
        assert_eq!(orders[&line.order_id].id, line.order_id);
    }

    let stores = Order::load_store(&db, &Order::all(&db).await.unwrap())
        .await
        .unwrap();
    assert_eq!(stores[&2].name, "Hamburg");
}

#[tokio::test]
async fn a_nullable_foreign_key_gives_an_option() {
    let db = TestDb::new().await;
    let client = db.client_with(SHOP).await;
    let orders = Order::all(&client).await.unwrap();

    assert_eq!(orders[0].coupon(&client).await.unwrap(), None);
    let ten = orders[1].coupon(&client).await.unwrap().unwrap();
    assert_eq!((ten.code.as_str(), ten.percent), ("TEN", 10));

    let coupons = Order::load_coupon(&client, &orders).await.unwrap();
    assert_eq!(coupons.len(), 1);
    assert_eq!(coupons["TEN"].percent, 10);
    // No order has a coupon: no query, an empty map.
    let without = Order::load_coupon(&client, &orders[..1]).await.unwrap();
    assert!(without.is_empty());
}

#[tokio::test]
async fn independent_loaders_pipeline_on_one_connection() {
    let db = TestDb::new().await;
    let client = db.client_with(SHOP).await;
    let orders = Order::all(&client).await.unwrap();
    let (lines, stores) = tokio::try_join!(
        Order::load_lines(&client, &orders),
        Order::load_store(&client, &orders),
    )
    .unwrap();
    assert_eq!(lines.len(), 3);
    assert_eq!(stores.len(), 2);
}

#[tokio::test]
async fn loaders_take_any_executor() {
    let db = TestDb::new().await;
    db.client_with(SHOP).await;
    let pool = db.pool(1);
    let mut client = pool.get().await.unwrap();
    let orders = Order::all(&client).await.unwrap();
    let in_pool = Order::load_lines(&client, &orders).await.unwrap();
    let tx = client.transaction().await.unwrap();
    let in_tx = Order::load_lines(&tx, &orders).await.unwrap();
    assert_eq!(in_pool, in_tx);
    let order = in_tx[0][0].order(&tx).await.unwrap();
    assert_eq!(order, orders[0]);
    // Ended here: a transaction dropped open would hold its locks while `TestDb` drops the
    // schema, and the rollback it queues cannot run while that drop blocks this runtime.
    tx.rollback().await.unwrap();
}

/// Compiles only if the generated methods' futures are `Send` for any executor.
#[allow(dead_code)]
fn relation_futures_are_send<E: Executor>(db: &E, orders: &[Order], line: &OrderLine) {
    fn send<T: Send>(_: T) {}
    send(orders[0].lines(db));
    send(Order::load_lines(db, orders));
    send(line.order(db));
    send(orders[0].coupon(db));
    send(OrderLine::load_order(db, std::slice::from_ref(line)));
}
