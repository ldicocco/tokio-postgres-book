#[automatically_derived]
impl ::shelf::FromRow for Product {
    type Columns = (usize, usize, usize);
    fn resolve(
        columns: &[::shelf::__private::tokio_postgres::Column],
    ) -> ::core::result::Result<Self::Columns, ::shelf::Error> {
        ::core::result::Result::Ok((
            ::shelf::__private::column::<
                ::shelf::__private::Json<Attributes>,
            >(columns, "Product", "attributes", "attributes")?,
            ::shelf::__private::column::<i64>(columns, "Product", "price", "price")?,
            ::shelf::__private::column::<String>(columns, "Product", "sku", "sku_code")?,
        ))
    }
    fn from_row_at(
        row: &::shelf::__private::tokio_postgres::Row,
        columns: &Self::Columns,
    ) -> ::core::result::Result<Self, ::shelf::Error> {
        ::core::result::Result::Ok(Self {
            attributes: ::shelf::__private::get::<
                ::shelf::__private::Json<Attributes>,
            >(row, columns.0, "Product", "attributes")?
                .0,
            price: ::shelf::__private::get_try_from::<
                i64,
                Cents,
            >(row, columns.1, "Product", "price")?,
            sku: ::shelf::__private::get_try_from::<
                String,
                Sku,
            >(row, columns.2, "Product", "sku")?,
        })
    }
}
