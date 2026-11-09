// What `Patch` refuses: a field that is not an `Option`, keys that belong on the model, and a
// patch of a versioned model without the version it was made against.
use shelf::{FromRow, Patch, Table};

#[derive(FromRow, Table)]
#[shelf(table = "products")]
struct Product {
    #[shelf(primary_key)]
    id: i64,
    sku: String,
    #[shelf(version)]
    version: i32,
}

#[derive(Patch)]
#[shelf(model = Product)]
struct NotOptions {
    sku: String,
    #[shelf(json)]
    note: Option<String>,
    #[shelf(version)]
    version: i32,
}

#[derive(Patch)]
#[shelf(model = Product)]
struct NoVersion {
    sku: Option<String>,
}

#[derive(FromRow, Table)]
#[shelf(table = "notes")]
struct Note {
    #[shelf(primary_key)]
    id: i64,
    body: String,
}

#[derive(Patch)]
#[shelf(model = Note, schema = "public")]
struct Mistakes<T> {
    #[shelf(try_from = i64)]
    body: Option<T>,
    #[shelf(version)]
    a: i32,
    #[shelf(version)]
    b: i32,
}

#[derive(Patch)]
#[shelf(model = Note)]
struct Empty {
    #[shelf(skip)]
    cached: String,
}

#[derive(Patch)]
#[shelf(model = Note)]
struct ExtraVersion {
    body: Option<String>,
    #[shelf(version)]
    version: i32,
}

fn main() {}
