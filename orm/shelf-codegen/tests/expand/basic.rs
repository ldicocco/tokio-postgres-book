// The plain case: every field read from the column of its name. `r#type` reads `type`.
#[derive(FromRow)]
struct Product {
    id: i64,
    sku: String,
    r#type: String,
    tags: Option<Vec<String>>,
}
