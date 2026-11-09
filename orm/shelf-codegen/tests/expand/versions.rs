// `version` and `updated_at`: an update expects the version it read and adds one, sets the
// time, and says in the same round trip whether the row was stale or missing.
#[derive(FromRow, Table)]
#[shelf(table = "products")]
struct Product {
    #[shelf(primary_key)]
    id: i64,
    price_cents: i64,
    #[shelf(version)]
    version: i32,
    #[shelf(updated_at)]
    updated_at: Timestamp,
}
