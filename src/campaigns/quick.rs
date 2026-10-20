//! The quick-add line of a list (assisted sheet slice 1.6, D-C, D-S, ASSIST-3;
//! `docs/assisted-entry.md` "Quick add"): "3 torches" or "healing potion x2" read by a fixed
//! rule into a name and a count. No model reads it; the name is kept as typed.
use regex::Regex;
use std::sync::LazyLock;

/// The most characters a quick-add line takes (as a number entry, `entry::MAX_CHARS`).
pub(crate) const MAX_CHARS: usize = 200;

/// A leading count: 1–6 ASCII digits, white space, then the name.
static LEADING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^([0-9]{1,6})\s+(.+)$").expect("static quick-add leading-count pattern")
});
/// A trailing count: the name, `x` or `×`, then 1–6 ASCII digits.
static TRAILING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(.+?)\s*[x×]\s*([0-9]{1,6})$").expect("static quick-add trailing-count pattern")
});

/// What a quick-add line adds.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Item<'a> {
    pub name: &'a str,
    /// The count, when the list maps one; 1 when none was typed.
    pub quantity: Option<i64>,
}

/// Reads `text` (the line as posted): over [`MAX_CHARS`] characters or with a control
/// character other than tab it is refused with the reason; a blank line is `None` ("Nothing
/// to add"). With `counted` (the list maps a quantity) a leading count and a space, or a
/// trailing `x`/`×` and count, is the count and the rest the name; otherwise, and always
/// without `counted`, the whole trimmed line is the name, so a typed count is never lost.
pub(crate) fn read(text: &str, counted: bool) -> Result<Option<Item<'_>>, &'static str> {
    if text.chars().nth(MAX_CHARS).is_some() {
        return Err("A quick-add line takes at most 200 characters.");
    }
    if text.chars().any(|c| c.is_control() && c != '\t') {
        return Err("The quick-add line contains a control character.");
    }
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    if !counted {
        return Ok(Some(Item {
            name: text,
            quantity: None,
        }));
    }
    let split = LEADING
        .captures(text)
        .map(|found| (found.get(2), found.get(1)))
        .or_else(|| {
            TRAILING
                .captures(text)
                .map(|found| (found.get(1), found.get(2)))
        });
    let item = match split {
        Some((Some(name), Some(count))) => Item {
            name: name.as_str(),
            // At most six ASCII digits: always an i64.
            quantity: count.as_str().parse().ok(),
        },
        _ => Item {
            name: text,
            quantity: Some(1),
        },
    };
    Ok(Some(item))
}
