// The query builder's column constants: one per column, as visible as its field, typed by what
// the column holds (the `try_from` source, JSON for `json`), and none for a `skip` field.

#[derive(FromRow, Table)]
#[shelf(table = "products")]
pub struct Product {
    #[shelf(primary_key, generated)]
    pub id: i64,
    pub(crate) sku: String,
    note: Option<String>,
    #[shelf(try_from = i32)]
    pub stock: u32,
    #[shelf(json)]
    pub attributes: Attributes,
    #[shelf(skip)]
    pub selected: bool,
}
