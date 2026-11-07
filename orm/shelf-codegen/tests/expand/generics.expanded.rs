#[automatically_derived]
impl<T, M> ::shelf::FromRow for Page<T, M>
where
    T: ::shelf::FromColumn,
    M: ::shelf::FromRow,
    ::shelf::__private::Json<Vec<T>>: ::shelf::FromColumn,
{
    type Columns = (usize, usize, <M as ::shelf::FromRow>::Columns, usize);
    fn resolve(
        columns: &[::shelf::__private::tokio_postgres::Column],
    ) -> ::core::result::Result<Self::Columns, ::shelf::Error> {
        ::core::result::Result::Ok((
            ::shelf::__private::column::<i64>(columns, "Page", "total", "total")?,
            ::shelf::__private::column::<T>(columns, "Page", "item", "item")?,
            <M as ::shelf::FromRow>::resolve(columns)?,
            ::shelf::__private::column::<
                ::shelf::__private::Json<Vec<T>>,
            >(columns, "Page", "extra", "extra")?,
        ))
    }
    fn from_row_at(
        row: &::shelf::__private::tokio_postgres::Row,
        columns: &Self::Columns,
    ) -> ::core::result::Result<Self, ::shelf::Error> {
        ::core::result::Result::Ok(Self {
            total: ::shelf::__private::get::<i64>(row, columns.0, "Page", "total")?,
            item: ::shelf::__private::get::<T>(row, columns.1, "Page", "item")?,
            meta: <M as ::shelf::FromRow>::from_row_at(row, &columns.2)?,
            extra: ::shelf::__private::get::<
                ::shelf::__private::Json<Vec<T>>,
            >(row, columns.3, "Page", "extra")?
                .0,
        })
    }
}
#[automatically_derived]
impl ::shelf::FromRow for Nothing {
    type Columns = ((),);
    fn resolve(
        _columns: &[::shelf::__private::tokio_postgres::Column],
    ) -> ::core::result::Result<Self::Columns, ::shelf::Error> {
        ::core::result::Result::Ok(((),))
    }
    fn from_row_at(
        _row: &::shelf::__private::tokio_postgres::Row,
        _columns: &Self::Columns,
    ) -> ::core::result::Result<Self, ::shelf::Error> {
        ::core::result::Result::Ok(Self {
            cache: ::core::default::Default::default(),
        })
    }
}
