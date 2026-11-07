// `flatten` reads a nested FromRow from the same row; `skip` reads nothing.
#[derive(FromRow)]
struct Line {
    id: i64,
    #[shelf(flatten)]
    price: Price,
    #[shelf(skip)]
    selected: bool,
}
