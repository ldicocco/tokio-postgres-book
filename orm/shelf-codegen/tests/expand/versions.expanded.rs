#[automatically_derived]
impl ::shelf::FromRow for Product {
    type Columns = (usize, usize, usize, usize);
    fn resolve(
        columns: &[::shelf::__private::tokio_postgres::Column],
    ) -> ::core::result::Result<Self::Columns, ::shelf::Error> {
        ::core::result::Result::Ok((
            ::shelf::__private::column::<i64>(columns, "Product", "id", "id")?,
            ::shelf::__private::column::<
                i64,
            >(columns, "Product", "price_cents", "price_cents")?,
            ::shelf::__private::column::<i32>(columns, "Product", "version", "version")?,
            ::shelf::__private::column::<
                Timestamp,
            >(columns, "Product", "updated_at", "updated_at")?,
        ))
    }
    fn from_row_at(
        row: &::shelf::__private::tokio_postgres::Row,
        columns: &Self::Columns,
    ) -> ::core::result::Result<Self, ::shelf::Error> {
        ::core::result::Result::Ok(Self {
            id: ::shelf::__private::get::<i64>(row, columns.0, "Product", "id")?,
            price_cents: ::shelf::__private::get::<
                i64,
            >(row, columns.1, "Product", "price_cents")?,
            version: ::shelf::__private::get::<
                i32,
            >(row, columns.2, "Product", "version")?,
            updated_at: ::shelf::__private::get::<
                Timestamp,
            >(row, columns.3, "Product", "updated_at")?,
        })
    }
}
#[automatically_derived]
impl ::shelf::Table for Product {
    type Key = i64;
    const MODEL: &'static ::core::primitive::str = "Product";
    const TABLE: &'static ::core::primitive::str = "\"products\"";
    const COLUMNS: &'static ::core::primitive::str = "\"id\", \"price_cents\", \"version\", \"updated_at\"";
    const FIND: &'static ::core::primitive::str = "SELECT \"id\", \"price_cents\", \"version\", \"updated_at\" FROM \"products\" WHERE \"id\" = $1";
    const ALL: &'static ::core::primitive::str = "SELECT \"id\", \"price_cents\", \"version\", \"updated_at\" FROM \"products\" ORDER BY \"id\"";
    const COUNT: &'static ::core::primitive::str = "SELECT count(*) FROM \"products\"";
    const UPDATE: &'static ::core::primitive::str = "WITH updated AS (UPDATE \"products\" SET \"price_cents\" = $1, \"version\" = \"version\" + 1, \"updated_at\" = now() WHERE \"id\" = $3 AND \"version\" = $2 RETURNING \"id\", \"price_cents\", \"version\", \"updated_at\") SELECT \"id\", \"price_cents\", \"version\", \"updated_at\", (SELECT count(*) FROM updated) AS shelf_updated, (SELECT \"version\"::bigint FROM \"products\" WHERE \"id\" = $3) AS shelf_version FROM (VALUES (1)) AS one LEFT JOIN updated ON true";
    const DELETE: &'static ::core::primitive::str = "DELETE FROM \"products\" WHERE \"id\" = $1";
    const KEY_COLUMNS: &'static [&'static ::core::primitive::str] = &["\"id\""];
    const VERSION: ::core::option::Option<&'static ::core::primitive::str> = ::core::option::Option::Some(
        "\"version\"",
    );
    const UPDATED_AT: ::core::option::Option<&'static ::core::primitive::str> = ::core::option::Option::Some(
        "\"updated_at\"",
    );
    const FIELDS: &'static [::shelf::__private::Field] = &[
        ::shelf::__private::Field::new::<i64>("id", "id"),
        ::shelf::__private::Field::new::<i64>("price_cents", "price_cents"),
        ::shelf::__private::Field::new::<i32>("version", "version"),
        ::shelf::__private::Field::new::<Timestamp>("updated_at", "updated_at"),
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
            ::shelf::__private::param(&self.price_cents),
            ::shelf::__private::param(&self.version),
        ])
    }
}
#[automatically_derived]
#[allow(dead_code)]
impl Product {
    /// The column `"id"`, for the query builder.
    const ID: ::shelf::query::Column<Self, i64> = ::shelf::query::Column::new("\"id\"");
    /// The column `"price_cents"`, for the query builder.
    const PRICE_CENTS: ::shelf::query::Column<Self, i64> = ::shelf::query::Column::new(
        "\"price_cents\"",
    );
    /// The column `"version"`, for the query builder.
    const VERSION: ::shelf::query::Column<Self, i32> = ::shelf::query::Column::new(
        "\"version\"",
    );
    /// The column `"updated_at"`, for the query builder.
    const UPDATED_AT: ::shelf::query::Column<Self, Timestamp> = ::shelf::query::Column::new(
        "\"updated_at\"",
    );
}
