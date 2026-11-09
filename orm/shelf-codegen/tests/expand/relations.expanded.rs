#[automatically_derived]
impl ::shelf::FromRow for Order {
    type Columns = (usize, usize);
    fn resolve(
        columns: &[::shelf::__private::tokio_postgres::Column],
    ) -> ::core::result::Result<Self::Columns, ::shelf::Error> {
        ::core::result::Result::Ok((
            ::shelf::__private::column::<i64>(columns, "Order", "id", "id")?,
            ::shelf::__private::column::<
                Option<String>,
            >(columns, "Order", "coupon", "coupon")?,
        ))
    }
    fn from_row_at(
        row: &::shelf::__private::tokio_postgres::Row,
        columns: &Self::Columns,
    ) -> ::core::result::Result<Self, ::shelf::Error> {
        ::core::result::Result::Ok(Self {
            id: ::shelf::__private::get::<i64>(row, columns.0, "Order", "id")?,
            coupon: ::shelf::__private::get::<
                Option<String>,
            >(row, columns.1, "Order", "coupon")?,
        })
    }
}
#[automatically_derived]
impl ::shelf::Table for Order {
    type Key = i64;
    const MODEL: &'static ::core::primitive::str = "Order";
    const TABLE: &'static ::core::primitive::str = "\"orders\"";
    const COLUMNS: &'static ::core::primitive::str = "\"id\", \"coupon\"";
    const FIND: &'static ::core::primitive::str = "SELECT \"id\", \"coupon\" FROM \"orders\" WHERE \"id\" = $1";
    const ALL: &'static ::core::primitive::str = "SELECT \"id\", \"coupon\" FROM \"orders\" ORDER BY \"id\"";
    const COUNT: &'static ::core::primitive::str = "SELECT count(*) FROM \"orders\"";
    const UPDATE: &'static ::core::primitive::str = "UPDATE \"orders\" SET \"coupon\" = $1 WHERE \"id\" = $2 RETURNING \"id\", \"coupon\"";
    const DELETE: &'static ::core::primitive::str = "DELETE FROM \"orders\" WHERE \"id\" = $1";
    const KEY_COLUMNS: &'static [&'static ::core::primitive::str] = &["\"id\""];
    const VERSION: ::core::option::Option<&'static ::core::primitive::str> = ::core::option::Option::None;
    const UPDATED_AT: ::core::option::Option<&'static ::core::primitive::str> = ::core::option::Option::None;
    const FIELDS: &'static [::shelf::__private::Field] = &[
        ::shelf::__private::Field::new::<i64>("id", "id"),
        ::shelf::__private::Field::new::<Option<String>>("coupon", "coupon"),
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
        ::std::vec::Vec::from([::shelf::__private::param(&self.coupon)])
    }
}
#[automatically_derived]
#[allow(dead_code)]
impl Order {
    /// The column `"id"`, for the query builder.
    pub const ID: ::shelf::query::Column<Self, i64> = ::shelf::query::Column::new(
        "\"id\"",
    );
    /// The column `"coupon"`, for the query builder.
    pub const COUPON: ::shelf::query::Column<Self, Option<String>> = ::shelf::query::Column::new(
        "\"coupon\"",
    );
    /// The `OrderLine` rows whose `order_id` holds this `Order`'s key, in their key order: one query.
    pub fn lines<'a, E: ::shelf::Executor>(
        &'a self,
        db: &'a E,
    ) -> impl ::core::future::Future<
        Output = ::core::result::Result<::std::vec::Vec<OrderLine>, ::shelf::Error>,
    > + ::core::marker::Send + 'a {
        ::shelf::__private::children(
            db,
            OrderLine::ORDER_ID,
            <Self as ::shelf::Table>::key(self),
        )
    }
    /// The `OrderLine` rows of each of `parents`, in one query (`= ANY($1)`): the `Vec` at `i` holds `parents[i]`'s.
    pub fn load_lines<'a, E: ::shelf::Executor>(
        db: &'a E,
        parents: &'a [Self],
    ) -> impl ::core::future::Future<
        Output = ::core::result::Result<
            ::std::vec::Vec<::std::vec::Vec<OrderLine>>,
            ::shelf::Error,
        >,
    > + ::core::marker::Send + 'a {
        ::shelf::__private::load_children(
            db,
            parents,
            OrderLine::ORDER_ID,
            |child: &OrderLine| { &child.order_id },
        )
    }
}
#[automatically_derived]
impl ::shelf::FromRow for OrderLine {
    type Columns = (usize, usize, usize);
    fn resolve(
        columns: &[::shelf::__private::tokio_postgres::Column],
    ) -> ::core::result::Result<Self::Columns, ::shelf::Error> {
        ::core::result::Result::Ok((
            ::shelf::__private::column::<i64>(columns, "OrderLine", "id", "id")?,
            ::shelf::__private::column::<
                i64,
            >(columns, "OrderLine", "order_id", "order_id")?,
            ::shelf::__private::column::<
                Option<String>,
            >(columns, "OrderLine", "gift_coupon", "gift_coupon")?,
        ))
    }
    fn from_row_at(
        row: &::shelf::__private::tokio_postgres::Row,
        columns: &Self::Columns,
    ) -> ::core::result::Result<Self, ::shelf::Error> {
        ::core::result::Result::Ok(Self {
            id: ::shelf::__private::get::<i64>(row, columns.0, "OrderLine", "id")?,
            order_id: ::shelf::__private::get::<
                i64,
            >(row, columns.1, "OrderLine", "order_id")?,
            gift_coupon: ::shelf::__private::get::<
                Option<String>,
            >(row, columns.2, "OrderLine", "gift_coupon")?,
        })
    }
}
#[automatically_derived]
impl ::shelf::Table for OrderLine {
    type Key = i64;
    const MODEL: &'static ::core::primitive::str = "OrderLine";
    const TABLE: &'static ::core::primitive::str = "\"order_lines\"";
    const COLUMNS: &'static ::core::primitive::str = "\"id\", \"order_id\", \"gift_coupon\"";
    const FIND: &'static ::core::primitive::str = "SELECT \"id\", \"order_id\", \"gift_coupon\" FROM \"order_lines\" WHERE \"id\" = $1";
    const ALL: &'static ::core::primitive::str = "SELECT \"id\", \"order_id\", \"gift_coupon\" FROM \"order_lines\" ORDER BY \"id\"";
    const COUNT: &'static ::core::primitive::str = "SELECT count(*) FROM \"order_lines\"";
    const UPDATE: &'static ::core::primitive::str = "UPDATE \"order_lines\" SET \"order_id\" = $1, \"gift_coupon\" = $2 WHERE \"id\" = $3 RETURNING \"id\", \"order_id\", \"gift_coupon\"";
    const DELETE: &'static ::core::primitive::str = "DELETE FROM \"order_lines\" WHERE \"id\" = $1";
    const KEY_COLUMNS: &'static [&'static ::core::primitive::str] = &["\"id\""];
    const VERSION: ::core::option::Option<&'static ::core::primitive::str> = ::core::option::Option::None;
    const UPDATED_AT: ::core::option::Option<&'static ::core::primitive::str> = ::core::option::Option::None;
    const FIELDS: &'static [::shelf::__private::Field] = &[
        ::shelf::__private::Field::new::<i64>("id", "id"),
        ::shelf::__private::Field::new::<i64>("order_id", "order_id"),
        ::shelf::__private::Field::new::<Option<String>>("gift_coupon", "gift_coupon"),
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
            ::shelf::__private::param(&self.order_id),
            ::shelf::__private::param(&self.gift_coupon),
        ])
    }
}
#[automatically_derived]
#[allow(dead_code)]
impl OrderLine {
    /// The column `"id"`, for the query builder.
    pub const ID: ::shelf::query::Column<Self, i64> = ::shelf::query::Column::new(
        "\"id\"",
    );
    /// The column `"order_id"`, for the query builder.
    pub const ORDER_ID: ::shelf::query::Column<Self, i64> = ::shelf::query::Column::new(
        "\"order_id\"",
    );
    /// The column `"gift_coupon"`, for the query builder.
    pub const GIFT_COUPON: ::shelf::query::Column<Self, Option<String>> = ::shelf::query::Column::new(
        "\"gift_coupon\"",
    );
    /// The `Order` that `order_id` points at.
    pub fn order<'a, E: ::shelf::Executor>(
        &'a self,
        db: &'a E,
    ) -> impl ::core::future::Future<
        Output = ::core::result::Result<Order, ::shelf::Error>,
    > + ::core::marker::Send + 'a {
        <Order as ::shelf::Table>::find(db, &self.order_id)
    }
    /// The `Order` rows that `children` point at, by key, in one query (`= ANY($1)`).
    pub fn load_order<'a, E: ::shelf::Executor>(
        db: &'a E,
        children: &'a [Self],
    ) -> impl ::core::future::Future<
        Output = ::core::result::Result<
            ::std::collections::HashMap<<Order as ::shelf::Table>::Key, Order>,
            ::shelf::Error,
        >,
    > + ::core::marker::Send + 'a {
        ::shelf::__private::load_parents::<
            Order,
            Self,
            _,
            E,
        >(db, children, |child: &Self| { &child.order_id })
    }
    /// The `Coupon` that `gift_coupon` points at, or `None` if it is NULL.
    pub fn gift<'a, E: ::shelf::Executor>(
        &'a self,
        db: &'a E,
    ) -> impl ::core::future::Future<
        Output = ::core::result::Result<::core::option::Option<Coupon>, ::shelf::Error>,
    > + ::core::marker::Send + 'a {
        ::shelf::__private::parent_opt::<
            Coupon,
            E,
        >(db, ::core::option::Option::as_ref(&self.gift_coupon))
    }
    /// The `Coupon` rows that `children` point at, by key, in one query (`= ANY($1)`).
    pub fn load_gift<'a, E: ::shelf::Executor>(
        db: &'a E,
        children: &'a [Self],
    ) -> impl ::core::future::Future<
        Output = ::core::result::Result<
            ::std::collections::HashMap<<Coupon as ::shelf::Table>::Key, Coupon>,
            ::shelf::Error,
        >,
    > + ::core::marker::Send + 'a {
        ::shelf::__private::load_parents::<
            Coupon,
            Self,
            _,
            E,
        >(db, children, |child: &Self| { &child.gift_coupon })
    }
}
