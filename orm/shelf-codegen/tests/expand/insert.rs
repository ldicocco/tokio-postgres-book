// `Insert` joins its model's table and columns into the statement at compile time.
#[derive(Insert)]
#[shelf(model = Product)]
struct NewProduct<'a> {
    sku: &'a str,
    #[shelf(json)]
    attributes: Attributes,
    #[shelf(skip)]
    note: String,
}

// Fields marked `conflict` name the unique key an upsert collides on.
#[derive(Insert)]
#[shelf(model = Product, rename_all = "camelCase")]
struct ProductPrice {
    #[shelf(conflict)]
    sku: String,
    price_cents: i64,
}
