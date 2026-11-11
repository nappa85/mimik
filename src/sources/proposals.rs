//! Table proposals on the review page: runs of unreviewed extracted units that read as a table.
//!
//! A proposal is a link to the create-table form with those units and the shape preselected. It
//! is computed from stored data only (the units' text and the page's stored words; no worker job
//! is ever started), stores nothing, and is never a decision: the Owner still reads the draft,
//! edits it and saves it with a rationale. Pure functions plus a small memo cache.
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use super::materials::overlapping_parsed;
use super::page_words::PageWord;
use super::shapes::{self, Shape, ShapeInput};
use super::table_words;
use super::tables::{MAX_HTML_BYTES, MAX_TABLE_SOURCES};

/// A non-plain shape must score at least this much to be proposed.
const MIN_SCORE: u32 = 80;
/// ... and read at least this many rows.
const MIN_ROWS: usize = 3;
/// Proposals shown on a page.
pub(crate) const MAX_PROPOSALS: usize = 8;
/// Pages with more units than this get no proposals (the same bound as the overlap check).
pub(super) const MAX_UNITS: usize = 2000;
/// Page entries kept in the memo cache.
const CACHE_ENTRIES: usize = 32;
/// Id ranges named in a label before "...".
const LABEL_RANGES: usize = 6;

/// One unit of the page in reading order, as the proposals see it.
pub(super) struct Candidate {
    pub id: i64,
    pub polygon: String,
    pub rejected: bool,
    /// The extraction text when the unit is an unreviewed extracted unit with usable text.
    pub text: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Proposal {
    pub units: Vec<i64>,
    pub shape: Shape,
}

impl Proposal {
    /// "Likely roll table: units 12-17".
    pub(crate) fn label(&self) -> String {
        let mut ranges: Vec<(i64, i64)> = Vec::new();
        for &id in &self.units {
            match ranges.last_mut() {
                Some((_, end)) if id == *end + 1 => *end = id,
                _ => ranges.push((id, id)),
            }
        }
        let mut named: Vec<String> = ranges
            .iter()
            .take(LABEL_RANGES)
            .map(|&(a, b)| {
                if a == b {
                    a.to_string()
                } else {
                    format!("{a}-{b}")
                }
            })
            .collect();
        if ranges.len() > LABEL_RANGES {
            named.push("\u{2026}".into());
        }
        let noun = if self.units.len() == 1 {
            "unit"
        } else {
            "units"
        };
        format!(
            "Likely {}: {noun} {}",
            self.shape.label().to_lowercase(),
            named.join(", ")
        )
    }

    /// The create-table form with these units and this shape selected.
    pub(crate) fn link(&self, page: i64) -> String {
        let units: Vec<String> = self.units.iter().map(|u| format!("unit={u}")).collect();
        format!(
            "/admin/materials/pages/{page}/tables/new?{}&shape={}",
            units.join("&"),
            self.shape.key()
        )
    }
}

/// The shape a run reads as, when it is worth proposing.
fn read(texts: &[&str], polygons: &[&str], words: Option<&[PageWord]>) -> Option<Shape> {
    let inside = words
        .map(|all| table_words::words_among(all, polygons))
        .filter(|inside| !inside.is_empty());
    let input = ShapeInput::new(texts).with_words(inside, false);
    let draft = shapes::best(&input)
        .into_iter()
        .find(|draft| !draft.rows.is_empty())?;
    (draft.shape != Shape::Plain && draft.score >= MIN_SCORE && draft.rows.len() >= MIN_ROWS)
        .then_some(draft.shape)
}

/// Proposals over the page's units in reading order: each maximal run of consecutive unreviewed
/// extracted units (split at 200 units), and, when a run as a whole is no table, each of its
/// units alone (a merged paragraph). A run whose union box covers an unselected unit is refused,
/// so a run never crosses page columns. At most 64 KiB of text is read per page, and at most
/// [`MAX_PROPOSALS`] are returned.
pub(super) fn propose(units: &[Candidate], words: Option<&[PageWord]>) -> Vec<Proposal> {
    if units.len() > MAX_UNITS {
        return Vec::new();
    }
    let boxes: Vec<(i64, Option<[f64; 4]>, bool)> = units
        .iter()
        .map(|u| {
            (
                u.id,
                super::materials::polygon_bounds(&u.polygon),
                u.rejected,
            )
        })
        .collect();
    let mut out = Vec::new();
    let mut budget = MAX_HTML_BYTES;
    let mut start = 0;
    while start < units.len() && out.len() < MAX_PROPOSALS {
        if units[start].text.is_none() {
            start += 1;
            continue;
        }
        let mut end = start;
        while end < units.len() && end - start < MAX_TABLE_SOURCES && units[end].text.is_some() {
            end += 1;
        }
        let run = &units[start..end];
        let mut try_run = |group: &[Candidate]| -> Option<Proposal> {
            let texts: Vec<&str> = group.iter().filter_map(|u| u.text.as_deref()).collect();
            let bytes: usize = texts.iter().map(|t| t.len()).sum();
            if bytes > budget {
                return None;
            }
            budget -= bytes;
            let polygons: Vec<&str> = group.iter().map(|u| u.polygon.as_str()).collect();
            let shape = read(&texts, &polygons, words)?;
            let ids: Vec<i64> = group.iter().map(|u| u.id).collect();
            let selected: BTreeSet<i64> = ids.iter().copied().collect();
            overlapping_parsed(&boxes, &selected)
                .is_empty()
                .then_some(Proposal { units: ids, shape })
        };
        if let Some(proposal) = try_run(run) {
            out.push(proposal);
        } else if run.len() > 1 {
            for unit in run {
                if out.len() == MAX_PROPOSALS {
                    break;
                }
                out.extend(try_run(std::slice::from_ref(unit)));
            }
        }
        start = end;
    }
    out
}

/// Page id, page revision and stored-words size.
pub(super) type CacheKey = (i64, i64, i64);

/// Proposals memoised per page, page revision and stored-words size: a decision, a table or an
/// edit advances the revision, and re-importing words changes the size. Small and bounded.
#[derive(Default)]
pub(crate) struct ProposalCache {
    entries: Mutex<Vec<(CacheKey, Arc<Vec<Proposal>>)>>,
    #[cfg(test)]
    computed: std::sync::atomic::AtomicUsize,
}

impl ProposalCache {
    pub(super) fn get(&self, key: CacheKey) -> Option<Arc<Vec<Proposal>>> {
        let entries = self.entries.lock().ok()?;
        entries
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| Arc::clone(v))
    }

    pub(super) fn put(&self, key: CacheKey, value: Vec<Proposal>) -> Arc<Vec<Proposal>> {
        let value = Arc::new(value);
        #[cfg(test)]
        self.computed
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if let Ok(mut entries) = self.entries.lock() {
            // A new revision of a page replaces the old one.
            entries.retain(|(k, _)| k.0 != key.0);
            if entries.len() >= CACHE_ENTRIES {
                entries.remove(0);
            }
            entries.push((key, Arc::clone(&value)));
        }
        value
    }

    #[cfg(test)]
    pub(crate) fn computed(&self) -> usize {
        self.computed.load(std::sync::atomic::Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(id: i64, polygon: &str, text: &str) -> Candidate {
        Candidate {
            id,
            polygon: polygon.into(),
            rejected: false,
            text: Some(text.into()),
        }
    }

    const ROLL: &str = "1 Rat\n2 Crow\n3 Wolf\n4 Bear\n5 Cat\n6 Dog";

    #[test]
    fn a_merged_paragraph_is_a_run_of_its_own() {
        let units = [unit(1, "[]", "Intro text"), unit(2, "[]", ROLL)];
        // Whether the intro joins the run as a caption or the unit is read alone, unit 2's
        // roll table is found.
        let found = propose(&units, None);
        assert!(
            found
                .iter()
                .any(|p| p.units.contains(&2) && p.shape == Shape::Roll)
        );
    }

    #[test]
    fn plain_text_and_short_runs_are_not_proposed() {
        let units = [
            unit(1, "[]", "Some prose."),
            unit(2, "[]", "More prose."),
            unit(3, "[]", "1 Rat\n2 Crow"),
        ];
        assert_eq!(propose(&units, None), Vec::new());
    }

    #[test]
    fn a_unit_without_text_breaks_the_run() {
        let mut units: Vec<Candidate> = (1..=6)
            .map(|i| unit(i, "[]", &format!("{i} Row {i}")))
            .collect();
        units[2].text = None;
        // Units 1-2 and 4-6 are separate runs; 4-6 has three rows.
        let found = propose(&units, None);
        assert!(found.iter().all(|p| !p.units.contains(&3)));
        assert!(
            found
                .iter()
                .all(|p| p.units.windows(2).all(|w| w[1] == w[0] + 1))
        );
    }

    #[test]
    fn labels_name_ranges_and_links_preselect_the_shape() {
        let proposal = Proposal {
            units: vec![12, 13, 14, 15, 16, 17, 20],
            shape: Shape::Roll,
        };
        assert_eq!(proposal.label(), "Likely roll table: units 12-17, 20");
        assert_eq!(
            Proposal {
                units: vec![5],
                shape: Shape::Stat
            }
            .label(),
            "Likely stat line: unit 5"
        );
        assert!(
            proposal
                .link(9)
                .starts_with("/admin/materials/pages/9/tables/new?unit=12&unit=13")
        );
        assert!(proposal.link(9).ends_with("&shape=roll"));
    }
}
