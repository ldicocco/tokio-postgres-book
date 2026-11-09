#[automatically_derived]
impl ::shelf::FromRow for Product {
    type Columns = (usize, usize, usize, ());
    fn resolve(
        columns: &[::shelf::__private::tokio_postgres::Column],
    ) -> ::core::result::Result<Self::Columns, ::shelf::Error> {
        ::core::result::Result::Ok((
            ::shelf::__private::column::<i64>(columns, "Product", "id", "id")?,
            ::shelf::__private::column::<String>(columns, "Product", "sku", "sku")?,
            ::shelf::__private::column::<String>(columns, "Product", "type", "type")?,
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
            r#type: ::shelf::__private::get::<
                String,
            >(row, columns.2, "Product", "type")?,
            selected: ::core::default::Default::default(),
        })
    }
}
#[automatically_derived]
impl ::shelf::Table for Product {
    type Key = i64;
    const MODEL: &'static ::core::primitive::str = "Product";
    const TABLE: &'static ::core::primitive::str = "\"products\"";
    const COLUMNS: &'static ::core::primitive::str = "\"id\", \"sku\", \"type\"";
    const FIND: &'static ::core::primitive::str = "SELECT \"id\", \"sku\", \"type\" FROM \"products\" WHERE \"id\" = $1";
    const ALL: &'static ::core::primitive::str = "SELECT \"id\", \"sku\", \"type\" FROM \"products\" ORDER BY \"id\"";
    const COUNT: &'static ::core::primitive::str = "SELECT count(*) FROM \"products\"";
    const UPDATE: &'static ::core::primitive::str = "UPDATE \"products\" SET \"sku\" = $1, \"type\" = $2 WHERE \"id\" = $3 RETURNING \"id\", \"sku\", \"type\"";
    const DELETE: &'static ::core::primitive::str = "DELETE FROM \"products\" WHERE \"id\" = $1";
    const KEY_COLUMNS: &'static [&'static ::core::primitive::str] = &["\"id\""];
    const VERSION: ::core::option::Option<&'static ::core::primitive::str> = ::core::option::Option::None;
    const UPDATED_AT: ::core::option::Option<&'static ::core::primitive::str> = ::core::option::Option::None;
    const FIELDS: &'static [::shelf::__private::Field] = &[
        ::shelf::__private::Field::new::<i64>("id", "id"),
        ::shelf::__private::Field::new::<String>("sku", "sku"),
        ::shelf::__private::Field::new::<String>("type", "type"),
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
            ::shelf::__private::param(&self.r#type),
        ])
    }
}
#[automatically_derived]
#[allow(dead_code)]
impl Product {
    /// The column `"id"`, for the query builder.
    const ID: ::shelf::query::Column<Self, i64> = ::shelf::query::Column::new("\"id\"");
    /// The column `"sku"`, for the query builder.
    const SKU: ::shelf::query::Column<Self, String> = ::shelf::query::Column::new(
        "\"sku\"",
    );
    /// The column `"type"`, for the query builder.
    const TYPE: ::shelf::query::Column<Self, String> = ::shelf::query::Column::new(
        "\"type\"",
    );
}
#[automatically_derived]
impl ::shelf::FromRow for OrderLine {
    type Columns = (usize, usize, usize);
    fn resolve(
        columns: &[::shelf::__private::tokio_postgres::Column],
    ) -> ::core::result::Result<Self::Columns, ::shelf::Error> {
        ::core::result::Result::Ok((
            ::shelf::__private::column::<
                i64,
            >(columns, "OrderLine", "order_id", "orderId")?,
            ::shelf::__private::column::<
                i32,
            >(columns, "OrderLine", "line_no", "lineNo")?,
            ::shelf::__private::column::<
                i64,
            >(columns, "OrderLine", "unit_price", "unitPrice")?,
        ))
    }
    fn from_row_at(
        row: &::shelf::__private::tokio_postgres::Row,
        columns: &Self::Columns,
    ) -> ::core::result::Result<Self, ::shelf::Error> {
        ::core::result::Result::Ok(Self {
            order_id: ::shelf::__private::get::<
                i64,
            >(row, columns.0, "OrderLine", "order_id")?,
            line_no: ::shelf::__private::get::<
                i32,
            >(row, columns.1, "OrderLine", "line_no")?,
            unit_price: ::shelf::__private::get::<
                i64,
            >(row, columns.2, "OrderLine", "unit_price")?,
        })
    }
}
#[automatically_derived]
impl ::shelf::Table for OrderLine {
    type Key = (i64, i32);
    const MODEL: &'static ::core::primitive::str = "OrderLine";
    const TABLE: &'static ::core::primitive::str = "\"shop\".\"order\"";
    const COLUMNS: &'static ::core::primitive::str = "\"orderId\", \"lineNo\", \"unitPrice\"";
    const FIND: &'static ::core::primitive::str = "SELECT \"orderId\", \"lineNo\", \"unitPrice\" FROM \"shop\".\"order\" WHERE \"orderId\" = $1 AND \"lineNo\" = $2";
    const ALL: &'static ::core::primitive::str = "SELECT \"orderId\", \"lineNo\", \"unitPrice\" FROM \"shop\".\"order\" ORDER BY \"orderId\", \"lineNo\"";
    const COUNT: &'static ::core::primitive::str = "SELECT count(*) FROM \"shop\".\"order\"";
    const UPDATE: &'static ::core::primitive::str = "UPDATE \"shop\".\"order\" SET \"unitPrice\" = $1 WHERE \"orderId\" = $2 AND \"lineNo\" = $3 RETURNING \"orderId\", \"lineNo\", \"unitPrice\"";
    const DELETE: &'static ::core::primitive::str = "DELETE FROM \"shop\".\"order\" WHERE \"orderId\" = $1 AND \"lineNo\" = $2";
    const KEY_COLUMNS: &'static [&'static ::core::primitive::str] = &[
        "\"orderId\"",
        "\"lineNo\"",
    ];
    const VERSION: ::core::option::Option<&'static ::core::primitive::str> = ::core::option::Option::None;
    const UPDATED_AT: ::core::option::Option<&'static ::core::primitive::str> = ::core::option::Option::None;
    const FIELDS: &'static [::shelf::__private::Field] = &[
        ::shelf::__private::Field::new::<i64>("order_id", "orderId"),
        ::shelf::__private::Field::new::<i32>("line_no", "lineNo"),
        ::shelf::__private::Field::new::<i64>("unit_price", "unitPrice"),
    ];
    fn key(&self) -> Self::Key {
        (
            ::core::clone::Clone::clone(&self.order_id),
            ::core::clone::Clone::clone(&self.line_no),
        )
    }
    fn key_params(
        key: &Self::Key,
    ) -> ::std::vec::Vec<
        &(dyn ::shelf::__private::tokio_postgres::types::ToSql + ::core::marker::Sync),
    > {
        ::std::vec::Vec::from([
            ::shelf::__private::param(&key.0),
            ::shelf::__private::param(&key.1),
        ])
    }
    fn update_params(
        &self,
    ) -> ::std::vec::Vec<
        &(dyn ::shelf::__private::tokio_postgres::types::ToSql + ::core::marker::Sync),
    > {
        ::std::vec::Vec::from([::shelf::__private::param(&self.unit_price)])
    }
}
#[automatically_derived]
#[allow(dead_code)]
impl OrderLine {
    /// The column `"orderId"`, for the query builder.
    const ORDER_ID: ::shelf::query::Column<Self, i64> = ::shelf::query::Column::new(
        "\"orderId\"",
    );
    /// The column `"lineNo"`, for the query builder.
    const LINE_NO: ::shelf::query::Column<Self, i32> = ::shelf::query::Column::new(
        "\"lineNo\"",
    );
    /// The column `"unitPrice"`, for the query builder.
    const UNIT_PRICE: ::shelf::query::Column<Self, i64> = ::shelf::query::Column::new(
        "\"unitPrice\"",
    );
}
