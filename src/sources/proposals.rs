//! Table proposals on the review page: runs of unreviewed extracted units that read as a table.
//!
//! A proposal is a link to the create-table form with those units and the shape preselected. It
//! is computed from stored data only (the units' text and the page's stored words; no worker job
//! is ever started), stores nothing, and is never a decision: the Owner still reads the draft,
//! edits it and saves it with a rationale. Pure functions plus a small memo cache.
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex, PoisonError};

use super::materials::overlapping_parsed;
use super::page_words::PageWord;
use super::shapes::{self, Shape, ShapeInput};
use super::table_words;
use super::tables::MAX_TABLE_SOURCES;

/// A non-plain shape must score at least this much to be proposed.
const MIN_SCORE: u32 = 80;
/// ... and read at least this many rows.
const MIN_ROWS: usize = 3;
/// Proposals listed on a page.
pub(crate) const MAX_PROPOSALS: usize = 8;
/// Pages with more units than this get no proposals (the same bound as the overlap check).
pub(super) const MAX_UNITS: usize = 2000;
/// Page entries kept in the memo cache.
const CACHE_ENTRIES: usize = 32;
/// At most this many units that start no row may lie between two units that do, in one table.
const GAP: usize = 2;
/// A caption before a table is a single short line without sentence punctuation.
const CAPTION_CHARS: usize = 60;
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

/// The trimmed nonempty lines of a unit's text.
fn lines(text: &str) -> impl Iterator<Item = &str> {
    text.lines().map(str::trim).filter(|line| !line.is_empty())
}

/// A draft worth proposing: a non-plain shape with a high score and enough body rows (not
/// counting a header or lead lines).
fn worth(draft: &shapes::Draft) -> bool {
    draft.shape != Shape::Plain
        && draft.score >= MIN_SCORE
        && draft.body_rows() >= MIN_ROWS
        && !draft.rows.is_empty()
}

/// A candidate proposal: the units `[start, end)` of the page and the shape that reads them.
struct Found {
    start: usize,
    end: usize,
    shape: Shape,
    score: u32,
}

fn texts_of(group: &[Candidate]) -> Vec<&str> {
    group.iter().filter_map(|u| u.text.as_deref()).collect()
}

/// Whether unit `i` starts below-and-beside or above the previous one: a layout break (another
/// page column, a box that does not line up). Units without a usable box never break.
fn layout_break(bounds: &[Option<[f64; 4]>], i: usize) -> bool {
    match (bounds[i - 1], bounds[i]) {
        (Some(previous), Some(this)) => {
            this[0] >= previous[2] || this[2] <= previous[0] || this[1] < previous[1]
        }
        _ => false,
    }
}

fn caption(line: &str) -> bool {
    line.chars().count() <= CAPTION_CHARS && !line.ends_with(['.', '!', '?', ';', ','])
}

/// Text-shape candidates over one layout run `[from, to)`: for each shape, the clusters of
/// units that start rows of that shape (at most [`GAP`] other units between two starters), each
/// trimmed from the first starting unit (with a short single-line caption before it) to the
/// last one, so prose before or after a table is left out.
fn clusters(units: &[Candidate], from: usize, to: usize, out: &mut Vec<Found>) {
    for shape in [Shape::Roll, Shape::Progression, Shape::Stat, Shape::Label] {
        let starts: Vec<bool> = units[from..to]
            .iter()
            .map(|u| {
                u.text
                    .as_deref()
                    .is_some_and(|t| lines(t).any(|line| shapes::starts_row(shape, line)))
            })
            .collect();
        let mut i = 0;
        while i < starts.len() {
            if !starts[i] {
                i += 1;
                continue;
            }
            let first = i;
            let mut last = i;
            let mut j = i + 1;
            while j < starts.len() && j - last <= GAP + 1 {
                if starts[j] {
                    last = j;
                }
                j += 1;
            }
            i = last + 1;
            let mut start = first;
            if start > 0
                && !starts[start - 1]
                && units[from + start - 1].text.as_deref().is_some_and(|t| {
                    let mut it = lines(t);
                    it.next().is_some_and(caption) && it.next().is_none()
                })
            {
                start -= 1;
            }
            let group = &units[from + start..=from + last];
            let input = ShapeInput::new(&texts_of(group));
            let draft = shapes::read(&input, shape);
            if worth(&draft) {
                out.push(Found {
                    start: from + start,
                    end: from + last + 1,
                    shape,
                    score: draft.score,
                });
            }
        }
    }
}

/// Proposals over the page's units in reading order. Per maximal run of consecutive unreviewed
/// extracted units with text:
/// - the text shapes read clusters of units that start rows, within runs split at layout
///   breaks (another page column, a box that starts above the previous one), trimmed to the
///   recognised rows;
/// - Column grid reads the whole run (split at 200 units) from the stored words, since a
///   table's columns are units side by side;
/// - each unit alone is read by every shape (a table merged into one paragraph).
///
/// Candidates are ranked by score and kept when they do not overlap a better one and their
/// union box covers no unselected unit (a run never swallows another column's units). The text
/// read is bounded by the caller at 64 KiB per page, so the work is linear in it. The result
/// is in reading order and uncapped; the caller takes what it shows.
pub(super) fn propose(units: &[Candidate], words: Option<&[PageWord]>) -> Vec<Proposal> {
    if units.len() > MAX_UNITS {
        return Vec::new();
    }
    let bounds: Vec<Option<[f64; 4]>> = units
        .iter()
        .map(|u| super::materials::polygon_bounds(&u.polygon))
        .collect();
    let boxes: Vec<(i64, Option<[f64; 4]>, bool)> = units
        .iter()
        .zip(&bounds)
        .map(|(u, b)| (u.id, *b, u.rejected))
        .collect();
    let mut found: Vec<Found> = Vec::new();
    let mut start = 0;
    while start < units.len() {
        if units[start].text.is_none() {
            start += 1;
            continue;
        }
        let mut end = start;
        while end < units.len() && end - start < MAX_TABLE_SOURCES && units[end].text.is_some() {
            end += 1;
        }
        // Text shapes, per layout run.
        let mut from = start;
        for i in start + 1..=end {
            if i == end || layout_break(&bounds, i) {
                clusters(units, from, i, &mut found);
                from = i;
            }
        }
        // Column grid over the whole run, from stored words.
        if let Some(all) = words {
            let group = &units[start..end];
            let polygons: Vec<&str> = group.iter().map(|u| u.polygon.as_str()).collect();
            let inside = table_words::words_among(all, &polygons);
            if !inside.is_empty() {
                let input = ShapeInput::new(&texts_of(group)).with_words(Some(inside), false);
                let draft = shapes::read(&input, Shape::Column);
                if worth(&draft) {
                    found.push(Found {
                        start,
                        end,
                        shape: Shape::Column,
                        score: draft.score,
                    });
                }
            }
        }
        // Each unit alone (a table merged into one paragraph).
        for (offset, unit) in units[start..end].iter().enumerate() {
            let input = ShapeInput::new(&texts_of(std::slice::from_ref(unit)));
            let drafts = shapes::best(&input);
            if let Some(draft) = drafts.iter().find(|d| !d.rows.is_empty())
                && worth(draft)
            {
                found.push(Found {
                    start: start + offset,
                    end: start + offset + 1,
                    shape: draft.shape,
                    score: draft.score,
                });
            }
        }
        start = end;
    }
    // Best first (ties: longer, then earlier, then the fixed shape order), keeping those that
    // do not overlap a kept one and cover no unselected unit.
    found.sort_by_key(|c| {
        (
            std::cmp::Reverse(c.score),
            std::cmp::Reverse(c.end - c.start),
            c.start,
            Shape::ALL.iter().position(|s| *s == c.shape),
        )
    });
    let mut kept: Vec<Found> = Vec::new();
    for candidate in found {
        if kept
            .iter()
            .any(|k| candidate.start < k.end && k.start < candidate.end)
        {
            continue;
        }
        let ids: BTreeSet<i64> = units[candidate.start..candidate.end]
            .iter()
            .map(|u| u.id)
            .collect();
        if overlapping_parsed(&boxes, &ids).is_empty() {
            kept.push(candidate);
        }
    }
    kept.sort_by_key(|c| c.start);
    kept.into_iter()
        .map(|c| Proposal {
            units: units[c.start..c.end].iter().map(|u| u.id).collect(),
            shape: c.shape,
        })
        .collect()
}

/// Page id, page revision and stored-words size.
pub(super) type CacheKey = (i64, i64, i64);

/// Proposals memoised per page, page revision and stored-words size: a decision, a table or an
/// edit advances the revision, and re-importing words changes the size. Small and bounded.
#[derive(Default)]
pub(crate) struct ProposalCache {
    entries: Mutex<Vec<(CacheKey, Arc<Vec<Proposal>>)>>,
    /// The agreement pass without the caller's exclusions, beside the proposals.
    passes: Mutex<
        Vec<(
            super::agreement_view::PassKey,
            Arc<super::agreement_view::Pass>,
        )>,
    >,
    #[cfg(test)]
    computed: std::sync::atomic::AtomicUsize,
    #[cfg(test)]
    passes_computed: std::sync::atomic::AtomicUsize,
}

impl ProposalCache {
    pub(super) fn get(&self, key: CacheKey) -> Option<Arc<Vec<Proposal>>> {
        let entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
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
        // A poisoned lock only means another request panicked; the entries stay usable.
        let mut entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        // A new revision of a page replaces the old one.
        entries.retain(|(k, _)| k.0 != key.0);
        if entries.len() >= CACHE_ENTRIES {
            entries.remove(0);
        }
        entries.push((key, Arc::clone(&value)));
        value
    }

    pub(super) fn pass_get(
        &self,
        key: &super::agreement_view::PassKey,
    ) -> Option<Arc<super::agreement_view::Pass>> {
        let passes = self.passes.lock().unwrap_or_else(PoisonError::into_inner);
        passes
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| Arc::clone(v))
    }

    pub(super) fn pass_put(
        &self,
        key: super::agreement_view::PassKey,
        value: super::agreement_view::Pass,
    ) -> Arc<super::agreement_view::Pass> {
        let value = Arc::new(value);
        #[cfg(test)]
        self.passes_computed
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut passes = self.passes.lock().unwrap_or_else(PoisonError::into_inner);
        passes.retain(|(k, _)| k.0 != key.0);
        if passes.len() >= CACHE_ENTRIES {
            passes.remove(0);
        }
        passes.push((key, Arc::clone(&value)));
        value
    }

    #[cfg(test)]
    pub(crate) fn passes_computed(&self) -> usize {
        self.passes_computed
            .load(std::sync::atomic::Ordering::Relaxed)
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
    fn the_minimum_counts_body_rows_not_a_caption() {
        let units = [
            unit(1, "[]", "Bestiary"),
            unit(2, "[]", "Hit points: sixteen"),
            unit(3, "[]", "Armor: light leather"),
        ];
        // Three rows with the caption, two body rows: not enough.
        assert_eq!(propose(&units, None), Vec::new());
        let more = [
            unit(1, "[]", "Bestiary"),
            unit(2, "[]", "Hit points: sixteen"),
            unit(3, "[]", "Armor: light leather"),
            unit(4, "[]", "Weapon: rusty axe"),
        ];
        let found = propose(&more, None);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].units, [1, 2, 3, 4]);
    }

    #[test]
    fn a_table_is_trimmed_from_the_prose_around_it() {
        let mut units = vec![
            unit(1, "[]", "Some prose here."),
            unit(2, "[]", "More prose."),
        ];
        for (i, row) in ["1 Rat", "2 Crow", "3 Wolf", "4 Bear"]
            .into_iter()
            .enumerate()
        {
            units.push(unit(3 + i64::try_from(i).unwrap(), "[]", row));
        }
        units.push(unit(7, "[]", "Trailing prose."));
        units.push(unit(8, "[]", "More trailing prose."));
        units.push(unit(9, "[]", "And more."));
        units.push(unit(10, "[]", "Still more."));
        let found = propose(&units, None);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].units, [3, 4, 5, 6]);
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
