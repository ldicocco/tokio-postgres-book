//! The derive compiles where users write it: in another crate, with only `shelf` in scope,
//! and next to names that would capture a careless expansion.

// The generated code must not depend on what the user's module calls `Result`, `Ok` or
// `Default`.
#[allow(dead_code)]
type Result = ();
#[allow(dead_code, non_upper_case_globals)]
const Ok: () = ();
#[allow(dead_code)]
trait Default {}

#[allow(dead_code)] // only the impls matter here
#[derive(shelf::FromRow)]
struct Product {
    id: i64,
    r#type: String,
    tags: Option<Vec<String>>,
    #[shelf(skip)]
    cached: Vec<u8>,
}

#[allow(dead_code)]
#[derive(shelf::FromRow)]
struct Wrapper<T> {
    #[shelf(flatten)]
    inner: T,
}

fn implements_from_row<T: shelf::FromRow>() {}

#[test]
fn derived_structs_implement_from_row() {
    implements_from_row::<Product>();
    implements_from_row::<Wrapper<Product>>();
    implements_from_row::<(i64, String)>();
}
