//! Exclusive50-row keyset page with one authorized lookahead row.
pub(crate) struct Listing<T, C = i64> {
    pub rows: Vec<T>,
    pub next: Option<C>,
}

impl<T, C> Listing<T, C> {
    pub(crate) fn new(mut rows: Vec<T>, id: impl Fn(&T) -> C) -> Self {
        let more = rows.len() > 50;
        rows.truncate(50);
        let next = if more { rows.last().map(id) } else { None };
        Self { rows, next }
    }
}
