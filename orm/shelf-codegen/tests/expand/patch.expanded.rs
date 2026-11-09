#[automatically_derived]
impl ::shelf::Patch for ProductPatch {
    type Model = Product;
    const COLUMNS: &'static [&'static ::core::primitive::str] = &[
        "\"price_cents\"",
        "\"note\"",
    ];
    fn sql() -> &'static ::core::primitive::str {
        static SQL: ::std::sync::LazyLock<::std::string::String> = ::std::sync::LazyLock::new(||
        { ::shelf::__private::patch_sql::<Product>(&["\"price_cents\"", "\"note\""]) });
        ::std::string::String::as_str(&SQL)
    }
    fn params(
        &self,
    ) -> ::std::vec::Vec<
        &(dyn ::shelf::__private::tokio_postgres::types::ToSql + ::core::marker::Sync),
    > {
        ::std::vec::Vec::from([
            ::shelf::__private::flag(&self.price_cents),
            ::shelf::__private::param(&self.price_cents),
            ::shelf::__private::flag(&self.note),
            ::shelf::__private::param(&self.note),
            ::shelf::__private::param(&self.version),
        ])
    }
}
const _: () = {
    if <Product as ::shelf::Table>::VERSION.is_none() {
        ::core::panic!(
            "ProductPatch: the model Product has no `version` column, so the patch's #[shelf(version)] field has nothing to be checked against"
        );
    }
};
