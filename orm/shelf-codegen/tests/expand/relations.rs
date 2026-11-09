// `has_many` adds a loader for one parent and one for many; `belongs_to` the same the other way,
// with an `Option` for a nullable foreign key, and a method named after the model unless `as`
// names it.

#[derive(FromRow, Table)]
#[shelf(table = "orders", has_many(OrderLine, key = order_id, as = lines))]
pub struct Order {
    #[shelf(primary_key)]
    pub id: i64,
    pub coupon: Option<String>,
}

#[derive(FromRow, Table)]
#[shelf(table = "order_lines", belongs_to(Order, key = order_id))]
#[shelf(belongs_to(Coupon, key = gift_coupon, as = gift))]
pub struct OrderLine {
    #[shelf(primary_key)]
    pub id: i64,
    pub order_id: i64,
    pub gift_coupon: Option<String>,
}
