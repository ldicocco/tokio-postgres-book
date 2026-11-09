// What `has_many` and `belongs_to` refuse: each mistake at the token that causes it.
use shelf::{FromRow, Insert, Table};

#[derive(FromRow, Table)]
#[shelf(table = "orders", has_many(OrderLine, key = order_id))]
struct NoName {
    #[shelf(primary_key)]
    id: i64,
}

#[derive(FromRow, Table)]
#[shelf(table = "orders", has_many(key = order_id, as = lines))]
struct NoModel {
    #[shelf(primary_key)]
    id: i64,
}

#[derive(FromRow, Table)]
#[shelf(table = "orders", belongs_to(Order, key = order_id, through = x))]
struct UnknownKey {
    #[shelf(primary_key)]
    id: i64,
}

// A composite key: a relation follows a single-column key.
#[derive(FromRow, Table)]
#[shelf(table = "stock", has_many(OrderLine, key = order_id, as = lines))]
struct Stock {
    #[shelf(primary_key)]
    store_id: i64,
    #[shelf(primary_key)]
    product_id: i64,
}

#[derive(FromRow, Table)]
#[shelf(table = "orders", has_many(OrderLine, key = orderid, as = lines))]
struct Order {
    #[shelf(primary_key)]
    id: i64,
}

#[derive(FromRow, Table)]
#[shelf(table = "order_lines", belongs_to(Order, key = order_id))]
struct OrderLine {
    #[shelf(primary_key)]
    id: i64,
    order_id: i64,
}

#[derive(FromRow, Table)]
#[shelf(table = "order_lines", belongs_to(Order, key = order))]
struct Skipped {
    #[shelf(primary_key)]
    id: i64,
    #[shelf(skip)]
    order: i64,
}

#[derive(FromRow, Table)]
#[shelf(table = "order_lines", belongs_to(Order, key = order_no))]
struct NoField {
    #[shelf(primary_key)]
    id: i64,
}

// The foreign key's type is not the key's.
#[derive(FromRow, Table)]
#[shelf(table = "notes", belongs_to(Order, key = order_id))]
struct Note {
    #[shelf(primary_key)]
    id: i64,
    order_id: i32,
}

#[derive(Insert)]
#[shelf(model = OrderLine, belongs_to(Order, key = order_id))]
struct NewOrderLine {
    order_id: i64,
}

fn main() {}
