// Type parameters get the bounds their fields need, and only those.
#[derive(FromRow)]
struct Page<T, M> {
    total: i64,
    item: T,
    #[shelf(flatten)]
    meta: M,
    #[shelf(json)]
    extra: Vec<T>,
}

// Nothing to read: the parameters are named so as not to warn.
#[derive(FromRow)]
struct Nothing {
    #[shelf(skip)]
    cache: Vec<u8>,
}
