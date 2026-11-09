// A field type that tokio-postgres cannot read: the compiler's error names the field's type.
use shelf::FromRow;

#[derive(FromRow)]
struct Timing {
    started: std::time::Instant,
}

fn main() {}
