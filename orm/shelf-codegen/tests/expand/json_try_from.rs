// `json` reads through `Json<T>`; `try_from` reads one type and converts it.
#[derive(FromRow)]
struct Product {
    #[shelf(json)]
    attributes: Attributes,
    #[shelf(try_from = i64)]
    price: Cents,
    #[shelf(try_from = "String", rename = "sku_code")]
    sku: Sku,
}
