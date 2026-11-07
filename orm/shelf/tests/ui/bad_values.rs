// Keys with a missing, wrong or repeated value.
use shelf::FromRow;

#[derive(FromRow)]
struct Product {
    #[shelf(rename)]
    a: i64,
    #[shelf(rename = 1)]
    b: i64,
    #[shelf(rename = "")]
    c: i64,
    #[shelf(skip = true)]
    d: i64,
    #[shelf(rename = "e1", rename = "e2")]
    e: i64,
    #[shelf(try_from = "not a type")]
    f: i64,
}

fn main() {}
