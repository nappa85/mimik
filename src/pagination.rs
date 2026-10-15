//! Exclusive50-row keyset page with one authorized lookahead row.
pub(crate) struct Listing<T> {
    pub rows: Vec<T>,
    pub next: Option<i64>,
}

impl<T> Listing<T> {
    pub(crate) fn new(mut rows: Vec<T>, id: impl Fn(&T) -> i64) -> Self {
        let more = rows.len() > 50;
        rows.truncate(50);
        let next = if more { rows.last().map(id) } else { None };
        Self { rows, next }
    }
}
