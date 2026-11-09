// FromRow maps a row to a struct with named fields, and to nothing else.
use shelf::FromRow;

#[derive(FromRow)]
enum Status {
    Pending,
    Paid,
}

#[derive(FromRow)]
struct Pair(i32, i32);

#[derive(FromRow)]
union Bits {
    int: i32,
    float: f32,
}

fn main() {}
