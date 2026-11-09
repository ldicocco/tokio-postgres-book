#[automatically_derived]
impl ::shelf::FromRow for LegacyOrder {
    type Columns = (usize, usize, usize);
    fn resolve(
        columns: &[::shelf::__private::tokio_postgres::Column],
    ) -> ::core::result::Result<Self::Columns, ::shelf::Error> {
        ::core::result::Result::Ok((
            ::shelf::__private::column::<
                i64,
            >(columns, "LegacyOrder", "order_id", "orderId")?,
            ::shelf::__private::column::<
                i64,
            >(columns, "LegacyOrder", "unit_price_cents", "unitPriceCents")?,
            ::shelf::__private::column::<
                String,
            >(columns, "LegacyOrder", "status", "STATUS")?,
        ))
    }
    fn from_row_at(
        row: &::shelf::__private::tokio_postgres::Row,
        columns: &Self::Columns,
    ) -> ::core::result::Result<Self, ::shelf::Error> {
        ::core::result::Result::Ok(Self {
            order_id: ::shelf::__private::get::<
                i64,
            >(row, columns.0, "LegacyOrder", "order_id")?,
            unit_price_cents: ::shelf::__private::get::<
                i64,
            >(row, columns.1, "LegacyOrder", "unit_price_cents")?,
            status: ::shelf::__private::get::<
                String,
            >(row, columns.2, "LegacyOrder", "status")?,
        })
    }
}
