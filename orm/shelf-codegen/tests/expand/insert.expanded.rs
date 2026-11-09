#[automatically_derived]
impl<'a> ::shelf::Insert for NewProduct<'a> {
    type Model = Product;
    const INSERT: &'static ::core::primitive::str = {
        const PARTS: &[&::core::primitive::str] = &[
            "INSERT INTO ",
            <Product as ::shelf::Table>::TABLE,
            " (\"sku\", \"attributes\") VALUES ($1, $2) RETURNING ",
            <Product as ::shelf::Table>::COLUMNS,
        ];
        const BYTES: [::core::primitive::u8; ::shelf::__private::total_len(PARTS)] = ::shelf::__private::join(
            PARTS,
        );
        ::shelf::__private::utf8(&BYTES)
    };
    const COLUMNS: &'static [&'static ::core::primitive::str] = &[
        "\"sku\"",
        "\"attributes\"",
    ];
    const COPY: &'static ::core::primitive::str = {
        const PARTS: &[&::core::primitive::str] = &[
            "COPY ",
            <Product as ::shelf::Table>::TABLE,
            " (\"sku\", \"attributes\") FROM STDIN (FORMAT binary)",
        ];
        const BYTES: [::core::primitive::u8; ::shelf::__private::total_len(PARTS)] = ::shelf::__private::join(
            PARTS,
        );
        ::shelf::__private::utf8(&BYTES)
    };
    fn params(
        &self,
    ) -> ::std::vec::Vec<
        &(dyn ::shelf::__private::tokio_postgres::types::ToSql + ::core::marker::Sync),
    > {
        ::std::vec::Vec::from([
            ::shelf::__private::param(&self.sku),
            ::shelf::__private::param(
                ::shelf::__private::Json::from_ref(&self.attributes),
            ),
        ])
    }
    fn arrays(
        rows: &[Self],
    ) -> ::std::vec::Vec<
        ::std::boxed::Box<
            dyn ::shelf::__private::tokio_postgres::types::ToSql + ::core::marker::Sync + ::core::marker::Send + '_,
        >,
    > {
        ::std::vec::Vec::from([
            ::shelf::__private::array(rows, |row| &row.sku),
            ::shelf::__private::array(
                rows,
                |row| ::shelf::__private::Json::from_ref(&row.attributes),
            ),
        ])
    }
}
#[automatically_derived]
impl ::shelf::Insert for ProductPrice {
    type Model = Product;
    const INSERT: &'static ::core::primitive::str = {
        const PARTS: &[&::core::primitive::str] = &[
            "INSERT INTO ",
            <Product as ::shelf::Table>::TABLE,
            " (\"sku\", \"priceCents\") VALUES ($1, $2) RETURNING ",
            <Product as ::shelf::Table>::COLUMNS,
        ];
        const BYTES: [::core::primitive::u8; ::shelf::__private::total_len(PARTS)] = ::shelf::__private::join(
            PARTS,
        );
        ::shelf::__private::utf8(&BYTES)
    };
    const COLUMNS: &'static [&'static ::core::primitive::str] = &[
        "\"sku\"",
        "\"priceCents\"",
    ];
    const COPY: &'static ::core::primitive::str = {
        const PARTS: &[&::core::primitive::str] = &[
            "COPY ",
            <Product as ::shelf::Table>::TABLE,
            " (\"sku\", \"priceCents\") FROM STDIN (FORMAT binary)",
        ];
        const BYTES: [::core::primitive::u8; ::shelf::__private::total_len(PARTS)] = ::shelf::__private::join(
            PARTS,
        );
        ::shelf::__private::utf8(&BYTES)
    };
    fn params(
        &self,
    ) -> ::std::vec::Vec<
        &(dyn ::shelf::__private::tokio_postgres::types::ToSql + ::core::marker::Sync),
    > {
        ::std::vec::Vec::from([
            ::shelf::__private::param(&self.sku),
            ::shelf::__private::param(&self.price_cents),
        ])
    }
    fn arrays(
        rows: &[Self],
    ) -> ::std::vec::Vec<
        ::std::boxed::Box<
            dyn ::shelf::__private::tokio_postgres::types::ToSql + ::core::marker::Sync + ::core::marker::Send + '_,
        >,
    > {
        ::std::vec::Vec::from([
            ::shelf::__private::array(rows, |row| &row.sku),
            ::shelf::__private::array(rows, |row| &row.price_cents),
        ])
    }
}
#[automatically_derived]
impl ::shelf::Upsert for ProductPrice {
    const UPSERT: &'static ::core::primitive::str = {
        const PARTS: &[&::core::primitive::str] = &[
            "INSERT INTO ",
            <Product as ::shelf::Table>::TABLE,
            " (\"sku\", \"priceCents\") VALUES ($1, $2) ON CONFLICT (\"sku\") DO UPDATE SET \"priceCents\" = excluded.\"priceCents\" RETURNING ",
            <Product as ::shelf::Table>::COLUMNS,
        ];
        const BYTES: [::core::primitive::u8; ::shelf::__private::total_len(PARTS)] = ::shelf::__private::join(
            PARTS,
        );
        ::shelf::__private::utf8(&BYTES)
    };
}
