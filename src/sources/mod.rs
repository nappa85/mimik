//! Local, source-bound review evidence. Models cannot approve these records.
mod import;
pub(crate) mod materials;
pub(crate) mod region;
pub(crate) mod search;
pub use search::SEARCH_SCHEMA;
mod store;
pub(crate) mod web;

use serde::Deserialize;

pub(crate) fn review_kind(kind: &str) -> Result<bool, crate::AppError> {
    static KINDS: std::sync::LazyLock<Result<Vec<String>, serde_json::Error>> =
        std::sync::LazyLock::new(|| {
            serde_json::from_str(include_str!("../../schemas/source-kinds.json"))
        });
    let kinds = KINDS
        .as_ref()
        .map_err(|_| crate::AppError::Invalid("Embedded source-kind schema is invalid."))?;
    Ok(kinds.iter().any(|candidate| candidate == kind))
}

pub struct SourceSummary {
    pub id: i64,
    pub title: String,
    pub system_id: String,
    pub edition: String,
    pub pdf_hash: String,
    pub total_pages: i64,
    pub pages: Vec<PageSummary>,
    pub ruleset: Option<crate::registry::RulesetKey>,
    pub finalized: bool,
}

pub struct PageSummary {
    pub id: i64,
    pub pdf_page: i64,
    pub complete: bool,
    pub printed_label: String,
}

pub struct PageView {
    pub id: i64,
    pub source_id: i64,
    pub title: String,
    pub edition: String,
    pub pdf_page: i64,
    pub printed_label: String,
    pub complete: bool,
    pub revision: i64,
    pub units: Vec<UnitView>,
    pub events: Vec<ReviewEvent>,
}

pub struct ReviewEvent {
    pub unit_id: Option<i64>,
    pub actor: String,
    pub rationale: String,
    pub state: String,
    pub correction: String,
    pub previous_revision: i64,
    pub created_at: String,
}

pub struct UnitView {
    pub id: i64,
    pub block_id: String,
    pub kind: String,
    pub polygon: String,
    pub sanitized_html: String,
    pub state: String,
    pub correction: String,
    pub revision: i64,
}

#[derive(Clone, Deserialize)]
pub struct UnitReview {
    pub revision: i64,
    pub state: String,
    #[serde(deserialize_with = "crate::form_text::lf")]
    pub correction: String,
    pub actor: String,
    #[serde(deserialize_with = "crate::form_text::lf")]
    pub rationale: String,
}

#[derive(Deserialize)]
pub struct PageReview {
    pub revision: i64,
    pub printed_label: String,
    pub actor: String,
    #[serde(deserialize_with = "crate::form_text::lf")]
    pub rationale: String,
}

/// One reviewed unit hit. Cite by `unit_id` + `unit_revision`; rank order is not stable
/// across imports because bm25 statistics are corpus-wide.
pub struct Evidence {
    pub unit_id: i64,
    pub unit_revision: i64,
    pub block_id: String,
    pub kind: String,
    /// Stored polygon JSON (PDF points), as in [`UnitView`].
    pub polygon: String,
    /// `accepted` or `corrected`.
    pub state: String,
    pub page_id: i64,
    pub page_revision: i64,
    pub source_id: i64,
    pub title: String,
    pub pdf_page: i64,
    pub printed_label: String,
    /// Effective indexed text: the correction, or the unwrapped normalised extraction.
    pub text: String,
}

pub struct EvidenceSearch {
    pub source_count: i64,
    pub incomplete_pages: i64,
    pub partial_runs: i64,
    /// Accepted units on complete pages excluded because their text could not be
    /// normalised for search (record a correction or rejection to resolve).
    pub unindexed_units: i64,
    /// More than the returned 50 matches exist.
    pub truncated: bool,
    pub matches: Vec<Evidence>,
}

pub(crate) fn sanitized(html: &str) -> Result<String, crate::AppError> {
    let bad_span = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let invalid_span = bad_span.clone();
    let tags = [
        "p",
        "br",
        "b",
        "strong",
        "i",
        "em",
        "sub",
        "sup",
        "table",
        "thead",
        "tbody",
        "tfoot",
        "tr",
        "td",
        "th",
        "ul",
        "ol",
        "li",
        "h1",
        "h2",
        "h3",
        "h4",
        "blockquote",
        "code",
        "pre",
    ]
    .into_iter()
    .collect();
    let clean = ammonia::Builder::new()
        .tags(tags)
        .generic_attributes(std::collections::HashSet::new())
        .tag_attributes(std::collections::HashMap::new())
        .add_tag_attributes("td", &["colspan", "rowspan"])
        .add_tag_attributes("th", &["colspan", "rowspan"])
        .attribute_filter(move |_, name, value| {
            if matches!(name, "colspan" | "rowspan") {
                if !value
                    .parse::<u16>()
                    .is_ok_and(|span| (1..=100).contains(&span))
                {
                    invalid_span.store(true, std::sync::atomic::Ordering::Relaxed);
                    return None;
                }
                return Some(std::borrow::Cow::Owned(value.to_owned()));
            }
            None
        })
        .url_schemes(std::collections::HashSet::new())
        .link_rel(None)
        .clean(html)
        .to_string();
    if bad_span.load(std::sync::atomic::Ordering::Relaxed) {
        return Err(crate::AppError::Invalid(
            "Unsupported table span; correct its source structure before import.",
        ));
    }
    Ok(clean)
}

pub(crate) fn review_identity(actor: &str, rationale: &str) -> Result<(), crate::AppError> {
    if actor.trim().is_empty()
        || actor.contains('\0')
        || rationale.contains('\0')
        || actor.chars().count() > 100
        || rationale.trim().is_empty()
        || rationale.chars().count() > 5_000
    {
        return Err(crate::AppError::Invalid(
            "Reviewer and rationale are required (100/5,000 character limits); remove any NUL characters.",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod sanitizer_tests {
    /// RUSTSEC-2026-0373 (ammonia < 4.2.4 matched the CSS `url(` function case-sensitively
    /// when filtering style properties). The source sanitizer allows no attributes except
    /// table spans, so `style` never survives whatever its case; this pins that.
    #[test]
    fn style_attributes_never_survive_in_any_case() {
        for style in [
            "background:url(javascript:alert(1))",
            "background:URL(javascript:alert(1))",
            "background:uRl(https://outside.example/x)",
            "background-image:Url( 'https://outside.example/x' )",
        ] {
            let html = format!("<p style=\"{style}\">text</p><td style=\"{style}\">c</td>");
            let clean = super::sanitized(&html).unwrap();
            assert!(!clean.to_ascii_lowercase().contains("url"), "{clean}");
            assert!(!clean.contains("style"), "{clean}");
            assert!(clean.contains("<p>text</p>"), "{clean}");
        }
    }
}
