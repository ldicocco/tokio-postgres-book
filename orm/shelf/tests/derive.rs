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
// …nor on what it calls `str` or `u8`: generated code names `::core::primitive::str`.
#[allow(dead_code, non_camel_case_types)]
type str = ();
#[allow(dead_code, non_camel_case_types)]
type u8 = ();

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

// A model whose table is a keyword, with a composite key.
#[allow(dead_code)]
#[derive(shelf::FromRow, shelf::Table)]
#[shelf(table = "order", schema = "pg_temp")]
struct Order {
    #[shelf(primary_key)]
    id: i64,
    #[shelf(primary_key)]
    line: i32,
    r#type: String,
}

// A new row for it, borrowing a field, with an upsert key.
#[allow(dead_code)]
#[derive(shelf::Insert)]
#[shelf(model = Order)]
struct NewOrder<'a> {
    #[shelf(conflict)]
    id: i64,
    line: i32,
    r#type: &'a String,
}

// Relations both ways, through a foreign key that is a raw identifier and nullable.
#[allow(dead_code)]
#[derive(shelf::FromRow, shelf::Table)]
#[shelf(table = "customer", has_many(Invoice, key = r#for, as = invoices))]
struct Customer {
    #[shelf(primary_key)]
    id: i64,
}

#[allow(dead_code)]
#[derive(shelf::FromRow, shelf::Table)]
#[shelf(table = "invoice", belongs_to(Customer, key = r#for))]
struct Invoice {
    #[shelf(primary_key)]
    id: i64,
    r#for: Option<i64>,
}

fn implements_from_row<T: shelf::FromRow>() {}
fn implements_table<T: shelf::Table>() {}
fn implements_upsert<T: shelf::Upsert>() {}

#[test]
fn derived_structs_implement_from_row() {
    implements_from_row::<Product>();
    implements_from_row::<Wrapper<Product>>();
    implements_from_row::<(i64, String)>();
}

#[test]
fn derived_models_implement_table() {
    implements_table::<Order>();
    implements_upsert::<NewOrder<'static>>();
    implements_table::<Customer>();
    implements_table::<Invoice>();
    // The column constants and relation methods exist, named after the raw fields.
    assert_eq!(Invoice::FOR.name(), r#""for""#);
    let _ = Customer::load_invoices::<tokio_postgres::Client>;
    let _ = Invoice::customer::<tokio_postgres::Client>;
}
