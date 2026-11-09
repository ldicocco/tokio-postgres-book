// What `Table` refuses, each at the token that causes it; `FromRow` reports nothing here.
use shelf::{FromRow, Table};

#[derive(FromRow, Table)]
struct NoTable {
    #[shelf(primary_key)]
    id: i64,
}

#[derive(FromRow, Table)]
#[shelf(table = "products")]
struct NoKey {
    id: i64,
}

#[derive(FromRow)]
struct Price {
    cents: i64,
}

#[derive(FromRow, Table)]
#[shelf(table = "products", schema = "")]
struct Mistakes {
    #[shelf(primary_key)]
    id: i64,
    #[shelf(flatten)]
    price: Price,
    #[shelf(skip, generated)]
    cached: i64,
}

#[derive(FromRow, Table)]
#[shelf(table = "products")]
struct Generic<T> {
    #[shelf(primary_key)]
    id: i64,
    value: T,
}

#[derive(FromRow, Table)]
#[shelf(table = "products")]
struct Versions {
    #[shelf(primary_key, version)]
    id: i64,
    #[shelf(version)]
    a: i32,
    #[shelf(version)]
    b: i32,
}

fn main() {}
