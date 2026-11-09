// The query builder's type errors: a value of the wrong type, a column of another model, and
// a method the column's type does not have.

#[derive(shelf::FromRow, shelf::Table)]
#[shelf(table = "products")]
struct Product {
    #[shelf(primary_key)]
    id: i64,
    sku: String,
    price_cents: i64,
    note: Option<String>,
}

#[derive(shelf::FromRow, shelf::Table)]
#[shelf(table = "orders")]
struct Order {
    #[shelf(primary_key)]
    id: i64,
    total_cents: i64,
}

use shelf::Table;

fn main() {
    let _ = Product::query().filter(Product::PRICE_CENTS.gt("1000"));

    let cents: i32 = 1000;
    let _ = Product::query().filter(Product::PRICE_CENTS.gt(cents));

    let _ = Product::query().filter(Order::TOTAL_CENTS.gt(1000));

    let _ = Product::query().filter(Product::PRICE_CENTS.like("1%"));
    let _ = Product::query().filter(Product::SKU.is_null());
    let _ = Product::query().filter(Product::NOTE.contains("x"));
}
