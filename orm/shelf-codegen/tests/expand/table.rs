// `Table` writes the table, the key and the reads as constants, with every identifier quoted.
#[derive(FromRow, Table)]
#[shelf(table = "products")]
struct Product {
    #[shelf(primary_key, generated)]
    id: i64,
    sku: String,
    r#type: String,
    #[shelf(skip)]
    selected: bool,
}

// A composite key is a tuple; a schema qualifies the table; `order` is a key word.
#[derive(FromRow, Table)]
#[shelf(table = "order", schema = "shop", rename_all = "camelCase")]
struct OrderLine {
    #[shelf(primary_key)]
    order_id: i64,
    #[shelf(primary_key)]
    line_no: i32,
    unit_price: i64,
}
