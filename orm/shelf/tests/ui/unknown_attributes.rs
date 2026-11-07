// Unknown keys and values are refused where they are written, all in one build.
use shelf::FromRow;

#[derive(FromRow)]
#[shelf(table = "products")]
struct OnTheStruct {
    id: i64,
}

#[derive(FromRow)]
#[shelf(rename_all = "kebab-case")]
struct BadCase {
    #[shelf(renamed = "sku")]
    sku: String,
    #[shelf(json, default)]
    attributes: String,
}

fn main() {}
