#[automatically_derived]
impl ::shelf::FromRow for Line {
    type Columns = (usize, <Price as ::shelf::FromRow>::Columns, ());
    fn resolve(
        columns: &[::shelf::__private::tokio_postgres::Column],
    ) -> ::core::result::Result<Self::Columns, ::shelf::Error> {
        ::core::result::Result::Ok((
            ::shelf::__private::column::<i64>(columns, "Line", "id", "id")?,
            <Price as ::shelf::FromRow>::resolve(columns)?,
            (),
        ))
    }
    fn from_row_at(
        row: &::shelf::__private::tokio_postgres::Row,
        columns: &Self::Columns,
    ) -> ::core::result::Result<Self, ::shelf::Error> {
        ::core::result::Result::Ok(Self {
            id: ::shelf::__private::get::<i64>(row, columns.0, "Line", "id")?,
            price: <Price as ::shelf::FromRow>::from_row_at(row, &columns.1)?,
            selected: ::core::default::Default::default(),
        })
    }
}
