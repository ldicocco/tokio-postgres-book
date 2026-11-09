// `Patch` lists its columns and its parameters; its statement is built once, at first use,
// from the model's constants. The `const` checks the version field against the model.
#[derive(Patch)]
#[shelf(model = Product)]
struct ProductPatch {
    price_cents: Option<i64>,
    note: Option<Option<String>>,
    #[shelf(version)]
    version: i32,
}
