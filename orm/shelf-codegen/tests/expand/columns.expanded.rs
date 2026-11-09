#[automatically_derived]
impl ::shelf::FromRow for Product {
    type Columns = (usize, usize, usize, usize, usize, ());
    fn resolve(
        columns: &[::shelf::__private::tokio_postgres::Column],
    ) -> ::core::result::Result<Self::Columns, ::shelf::Error> {
        ::core::result::Result::Ok((
            ::shelf::__private::column::<i64>(columns, "Product", "id", "id")?,
            ::shelf::__private::column::<String>(columns, "Product", "sku", "sku")?,
            ::shelf::__private::column::<
                Option<String>,
            >(columns, "Product", "note", "note")?,
            ::shelf::__private::column::<i32>(columns, "Product", "stock", "stock")?,
            ::shelf::__private::column::<
                ::shelf::__private::Json<Attributes>,
            >(columns, "Product", "attributes", "attributes")?,
            (),
        ))
    }
    fn from_row_at(
        row: &::shelf::__private::tokio_postgres::Row,
        columns: &Self::Columns,
    ) -> ::core::result::Result<Self, ::shelf::Error> {
        ::core::result::Result::Ok(Self {
            id: ::shelf::__private::get::<i64>(row, columns.0, "Product", "id")?,
            sku: ::shelf::__private::get::<String>(row, columns.1, "Product", "sku")?,
            note: ::shelf::__private::get::<
                Option<String>,
            >(row, columns.2, "Product", "note")?,
            stock: ::shelf::__private::get_try_from::<
                i32,
                u32,
            >(row, columns.3, "Product", "stock")?,
            attributes: ::shelf::__private::get::<
                ::shelf::__private::Json<Attributes>,
            >(row, columns.4, "Product", "attributes")?
                .0,
            selected: ::core::default::Default::default(),
        })
    }
}
#[automatically_derived]
impl ::shelf::Table for Product {
    type Key = i64;
    const MODEL: &'static ::core::primitive::str = "Product";
    const TABLE: &'static ::core::primitive::str = "\"products\"";
    const COLUMNS: &'static ::core::primitive::str = "\"id\", \"sku\", \"note\", \"stock\", \"attributes\"";
    const FIND: &'static ::core::primitive::str = "SELECT \"id\", \"sku\", \"note\", \"stock\", \"attributes\" FROM \"products\" WHERE \"id\" = $1";
    const ALL: &'static ::core::primitive::str = "SELECT \"id\", \"sku\", \"note\", \"stock\", \"attributes\" FROM \"products\" ORDER BY \"id\"";
    const COUNT: &'static ::core::primitive::str = "SELECT count(*) FROM \"products\"";
    const UPDATE: &'static ::core::primitive::str = "UPDATE \"products\" SET \"sku\" = $1, \"note\" = $2, \"stock\" = $3, \"attributes\" = $4 WHERE \"id\" = $5 RETURNING \"id\", \"sku\", \"note\", \"stock\", \"attributes\"";
    const DELETE: &'static ::core::primitive::str = "DELETE FROM \"products\" WHERE \"id\" = $1";
    const KEY_COLUMNS: &'static [&'static ::core::primitive::str] = &["\"id\""];
    const VERSION: ::core::option::Option<&'static ::core::primitive::str> = ::core::option::Option::None;
    const UPDATED_AT: ::core::option::Option<&'static ::core::primitive::str> = ::core::option::Option::None;
    const FIELDS: &'static [::shelf::__private::Field] = &[
        ::shelf::__private::Field::new::<i64>("id", "id"),
        ::shelf::__private::Field::new::<String>("sku", "sku"),
        ::shelf::__private::Field::new::<Option<String>>("note", "note"),
        ::shelf::__private::Field::new::<i32>("stock", "stock"),
        ::shelf::__private::Field::new::<
            ::shelf::__private::Json<Attributes>,
        >("attributes", "attributes"),
    ];
    fn key(&self) -> Self::Key {
        ::core::clone::Clone::clone(&self.id)
    }
    fn key_params(
        key: &Self::Key,
    ) -> ::std::vec::Vec<
        &(dyn ::shelf::__private::tokio_postgres::types::ToSql + ::core::marker::Sync),
    > {
        ::std::vec::Vec::from([::shelf::__private::param(key)])
    }
    fn update_params(
        &self,
    ) -> ::std::vec::Vec<
        &(dyn ::shelf::__private::tokio_postgres::types::ToSql + ::core::marker::Sync),
    > {
        ::std::vec::Vec::from([
            ::shelf::__private::param(&self.sku),
            ::shelf::__private::param(&self.note),
            ::shelf::__private::param(&self.stock),
            ::shelf::__private::param(
                ::shelf::__private::Json::from_ref(&self.attributes),
            ),
        ])
    }
}
#[automatically_derived]
#[allow(dead_code)]
impl Product {
    /// The column `"id"`, for the query builder.
    pub const ID: ::shelf::query::Column<Self, i64> = ::shelf::query::Column::new(
        "\"id\"",
    );
    /// The column `"sku"`, for the query builder.
    pub(crate) const SKU: ::shelf::query::Column<Self, String> = ::shelf::query::Column::new(
        "\"sku\"",
    );
    /// The column `"note"`, for the query builder.
    const NOTE: ::shelf::query::Column<Self, Option<String>> = ::shelf::query::Column::new(
        "\"note\"",
    );
    /// The column `"stock"`, for the query builder.
    pub const STOCK: ::shelf::query::Column<Self, i32> = ::shelf::query::Column::new(
        "\"stock\"",
    );
    /// The column `"attributes"`, for the query builder.
    pub const ATTRIBUTES: ::shelf::query::Column<
        Self,
        ::shelf::__private::serde_json::Value,
    > = ::shelf::query::Column::new("\"attributes\"");
}
