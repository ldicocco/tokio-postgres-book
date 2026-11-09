// `rename_all` sets the convention; `rename` on a field overrides it.
#[derive(FromRow)]
#[shelf(rename_all = "camelCase")]
struct LegacyOrder {
    order_id: i64,
    unit_price_cents: i64,
    #[shelf(rename = "STATUS")]
    status: String,
}
