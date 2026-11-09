// Fields hold owned values: a struct that borrows from the row is refused.
use shelf::FromRow;

#[derive(FromRow)]
struct Product<'a> {
    sku: &'a str,
}

fn main() {}
