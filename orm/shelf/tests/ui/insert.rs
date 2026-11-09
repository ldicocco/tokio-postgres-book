// What `Insert` refuses: no model, keys that belong on the model, and a model that is not a
// `Table`, which the compiler reports at the model's name.
use shelf::{FromRow, Insert, Table};

#[derive(Insert)]
struct NoModel {
    sku: String,
}

#[derive(FromRow, Table)]
#[shelf(table = "products")]
struct Product {
    #[shelf(primary_key)]
    id: i64,
    sku: String,
}

#[derive(Insert)]
#[shelf(model = Product, table = "products")]
struct Misplaced {
    #[shelf(primary_key)]
    id: i64,
    #[shelf(skip, conflict)]
    note: String,
}

#[derive(FromRow)]
struct NotATable {
    sku: String,
}

#[derive(Insert)]
#[shelf(model = NotATable)]
struct NewThing {
    sku: String,
}

#[derive(Insert)]
#[shelf(model = Product)]
struct NewGeneric<T> {
    sku: T,
}

#[derive(Insert)]
#[shelf(model = Product)]
struct NewPair(i64, String);

fn main() {}
