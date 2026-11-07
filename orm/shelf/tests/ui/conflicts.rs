// Keys that cannot go together: the error points at the one that does not fit.
use shelf::FromRow;

#[derive(FromRow)]
struct Inner {
    x: i64,
}

#[derive(FromRow)]
struct Product {
    #[shelf(skip, rename = "a")]
    a: i64,
    #[shelf(flatten, json)]
    b: Inner,
    #[shelf(json, try_from = i64)]
    c: i64,
}

fn main() {}
