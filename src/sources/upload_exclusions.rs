//! Deterministic repeated-text proposals. Text never enters the exclusion history.
use super::{
    AppError, Connection, OptionalExtension, OwnerWrite, RenderSettings, RequestAuth, Transaction,
    Upload, Uploads, hash, json, now, params, parse, remaining, worker,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub type BoxPoints = [f64; 4];
pub(crate) const COMPARISON_LIMIT: &str =
    "Repeated-text comparison limit reached; draw an exclusion by hand.";

#[cfg(test)]
thread_local! {
    pub(super) static PROPOSAL_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
}

/// Validated, selected, current staged page text in displayed space.
pub struct TextPage {
    pub page: u32,
    pub width: f64,
    pub height: f64,
    pub lines: Vec<worker::records::TextLine>,
}

/// Ephemeral Owner-only result; never persist or log this value.
pub struct Proposal {
    pub text: String,
    pub fingerprint: String,
    pub boxes: BTreeMap<u32, BoxPoints>,
    pub absent: BTreeMap<u32, BoxPoints>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Exclusion {
    pub pages: Vec<u32>,
    pub boxes: BTreeMap<u32, BoxPoints>,
    pub proposal_fingerprint: Option<String>,
    pub rationale: String,
}

pub(super) fn load(db: &Connection, id: &str) -> Result<(i64, Option<Exclusion>), AppError> {
    let row = db
        .query_row(
            "SELECT revision,pages,boxes,proposal_fingerprint,rationale,removed
         FROM source_upload_exclusions WHERE upload_id=?1 ORDER BY revision DESC LIMIT 1",
            [id],
            |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, bool>(5)?,
                ))
            },
        )
        .optional()?;
    match row {
        None => Ok((0, None)),
        Some((revision, pages, boxes, proposal_fingerprint, rationale, removed)) => Ok((
            revision,
            if removed {
                None
            } else {
                Some(Exclusion {
                    pages: parse(&pages)?,
                    boxes: parse(&boxes)?,
                    proposal_fingerprint,
                    rationale,
                })
            },
        )),
    }
}

impl Uploads {
    /// Owner-only proposal with admission rechecked before returning private text.
    /// # Errors
    /// Refuses stale revisions, incomplete/currentness failures or invalid staged text.
    pub(crate) fn propose_exclusions(
        &self,
        auth: &RequestAuth,
        id: &str,
        revision: i64,
    ) -> Result<Vec<Proposal>, AppError> {
        let _gate = self
            .0
            .gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let u = self.read_for_owner(auth, id)?;
        if u.revision != revision {
            return Err(AppError::Conflict);
        }
        let (proposals, _) = self.proposals(&u)?;
        self.0
            .store
            .upload_storage(|db| auth.recheck_owner(db).map(|_| ()))?;
        Ok(proposals)
    }

    fn proposals(&self, u: &Upload) -> Result<(Vec<Proposal>, [u8; 32]), AppError> {
        #[cfg(test)]
        PROPOSAL_HOOK.with(|hook| {
            if let Some(hook) = hook.borrow_mut().take() {
                hook();
            }
        });
        if matches!(u.job.state.as_str(), "queued" | "running")
            || u.selected_pages.is_empty()
            || !remaining(u).is_empty()
        {
            return Err(AppError::Conflict);
        }
        // Lazy random identity only: no source text or Owner action is persisted here.
        // Keeping it before computation also pins the identity for set_exclusion;
        // private results and exclusion writes still require the final Owner recheck.
        let key = self.0.store.upload_storage(|db| {
            let key: Option<Vec<u8>> = db.query_row(
                "SELECT fingerprint_key FROM source_uploads WHERE id=?1",
                [&u.id],
                |r| r.get(0),
            )?;
            if let Some(key) = key {
                return key.try_into().map_err(|_| AppError::Worker);
            }
            let mut key = [0; 32];
            getrandom::fill(&mut key).map_err(|_| AppError::Worker)?;
            db.execute(
                "UPDATE source_uploads SET fingerprint_key=?2 WHERE id=?1",
                params![u.id, key.as_slice()],
            )?;
            Ok(key)
        })?;
        let mut pages = Vec::new();
        for page in u
            .pages
            .iter()
            .filter(|p| u.selected_pages.contains(&p.pdf_page))
        {
            let bytes = self.0.files.read_text(&u.id, page.pdf_page)?;
            if bytes.len() as u64 != page.text_bytes || hash(&bytes) != page.text_sha256 {
                return Err(AppError::Invalid("Invalid staged text."));
            }
            // Parsing errors are deliberately mapped: serde diagnostics may contain text.
            let lines: Vec<worker::records::TextLine> = serde_json::from_slice(&bytes)
                .map_err(|_| AppError::Invalid("Invalid staged text."))?;
            let fact = u
                .inspection
                .as_ref()
                .and_then(|i| i.pages.iter().find(|p| p.n == page.pdf_page))
                .ok_or(AppError::Conflict)?;
            worker::records::validate_lines(&lines, fact)
                .map_err(|_| AppError::Invalid("Invalid staged text."))?;
            pages.push(TextPage {
                page: page.pdf_page,
                width: page.width_points,
                height: page.height_points,
                lines,
            });
        }
        Ok((propose_keyed(&pages, &key)?, key))
    }

    /// Append an Owner-confirmed exclusion in displayed points. Both revisions must match.
    /// `extend_absent` acknowledges the missing-text warning for proposal extension.
    /// # Errors
    /// Refuses stale admission/revisions, invalid geometry, rationale or fingerprint.
    pub fn set_exclusion(
        &self,
        owner: &OwnerWrite,
        id: &str,
        revision: i64,
        exclusion_revision: i64,
        exclusion: &Exclusion,
        extend_absent: bool,
    ) -> Result<(), AppError> {
        let _gate = self
            .0
            .gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let u = self.read_for_owner(&owner.auth, id)?;
        if u.revision != revision || u.exclusion_revision != exclusion_revision {
            return Err(AppError::Conflict);
        }
        validate(exclusion, &u)?;
        let verified_key = if let Some(fingerprint) = &exclusion.proposal_fingerprint {
            let (proposals, key) = self.proposals(&u)?;
            let matching: Vec<_> = proposals
                .iter()
                .filter(|p| &p.fingerprint == fingerprint)
                .collect();
            if matching.is_empty() {
                return Err(AppError::Invalid("Invalid proposal."));
            }
            if !extend_absent
                && !matching
                    .iter()
                    .any(|p| exclusion.pages.iter().all(|n| p.boxes.contains_key(n)))
            {
                return Err(AppError::Invalid("Confirm the absent-page warning first."));
            }
            Some(key)
        } else {
            None
        };
        self.write(owner, |tx| {
            // The upload gate pins the staged files and page facts while proposals
            // run outside the shared Store lock. Only scalar checks and the append
            // belong in this transaction, not another full staging snapshot/parse.
            let (current, excluded, key): (i64, i64, Option<Vec<u8>>) = tx.query_row(
                "SELECT revision,coalesce((SELECT max(revision) FROM source_upload_exclusions WHERE upload_id=?1),0),fingerprint_key FROM source_uploads WHERE id=?1",
                [id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            ).optional()?.ok_or(AppError::SourceNotFound)?;
            if current != revision || excluded != exclusion_revision || excluded == i64::MAX
                || verified_key.as_ref().is_some_and(|verified| key.as_deref() != Some(verified.as_slice())) {
                return Err(AppError::Conflict);
            }
            append(tx, owner, id, exclusion_revision + 1, exclusion, false)
        })
    }

    /// Append a removal event, preserving historical geometry and recording a new rationale.
    /// # Errors
    /// Refuses stale admission/revisions, absent exclusions or invalid rationale.
    pub fn remove_exclusion(
        &self,
        owner: &OwnerWrite,
        id: &str,
        revision: i64,
        exclusion_revision: i64,
        rationale: &str,
    ) -> Result<(), AppError> {
        let _gate = self
            .0
            .gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.write(owner, |tx| {
            let u = load_upload(tx, id, revision, exclusion_revision)?;
            let mut exclusion = u.exclusion.ok_or(AppError::Conflict)?;
            check_rationale(rationale)?;
            exclusion.rationale = rationale.into();
            append(tx, owner, id, exclusion_revision + 1, &exclusion, true)
        })
    }
}

fn load_upload(
    db: &Connection,
    id: &str,
    revision: i64,
    excluded: i64,
) -> Result<Upload, AppError> {
    let u = super::load(db, id)?;
    if u.revision != revision || u.exclusion_revision != excluded || excluded == i64::MAX {
        return Err(AppError::Conflict);
    }
    Ok(u)
}
fn check_rationale(rationale: &str) -> Result<(), AppError> {
    if rationale.trim().is_empty() || rationale.chars().count() > 5000 || rationale.contains('\0') {
        return Err(AppError::Invalid("Invalid exclusion rationale."));
    }
    Ok(())
}
pub(super) fn validate(e: &Exclusion, u: &Upload) -> Result<(), AppError> {
    check_rationale(&e.rationale)?;
    if e.pages.is_empty()
        || e.pages.len() > 64
        || e.pages.windows(2).any(|p| p[0] >= p[1])
        || e.pages.len() != e.boxes.len()
    {
        return Err(AppError::Invalid("Invalid exclusion pages."));
    }
    for n in &e.pages {
        let page = u
            .pages
            .iter()
            .find(|p| {
                p.pdf_page == *n
                    && u.selected_pages.contains(n)
                    && p.settings == RenderSettings::default()
                    && p.renderer_crate_version == worker::CRATE_VERSION
                    && p.mupdf_version == worker::MUPDF_VERSION
            })
            .ok_or(AppError::Invalid("Render selected exclusion pages first."))?;
        let b = e
            .boxes
            .get(n)
            .ok_or(AppError::Invalid("Missing exclusion box."))?;
        if !b.iter().all(|v| v.is_finite() && *v >= 0.)
            || b[0] >= b[2]
            || b[1] >= b[3]
            || b[2] > page.width_points
            || b[3] > page.height_points
        {
            return Err(AppError::Invalid("Invalid displayed exclusion box."));
        }
    }
    Ok(())
}
fn append(
    tx: &Transaction<'_>,
    owner: &OwnerWrite,
    id: &str,
    revision: i64,
    e: &Exclusion,
    removed: bool,
) -> Result<(), AppError> {
    let t = now()?;
    tx.execute(
        "INSERT INTO source_upload_exclusions
        (upload_id,revision,actor,actor_id,actor_epoch,session_digest,rationale,pages,boxes,
         proposal_fingerprint,removed,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
        params![
            id,
            revision,
            format!("account:{}", owner.auth.account_id()),
            owner.auth.account_id(),
            owner.auth.epoch(),
            owner.auth.digest(),
            e.rationale,
            json(&e.pages)?,
            json(&e.boxes)?,
            e.proposal_fingerprint,
            removed,
            t
        ],
    )?;
    tx.execute(
        "UPDATE source_uploads SET updated_at=?2 WHERE id=?1",
        params![id, t],
    )?;
    Ok(())
}

fn normalize(text: &str) -> String {
    let folded = crate::text_fold::full_fold(&crate::text_fold::collapse_whitespace(
        &crate::text_fold::nfkc(text),
    ));
    let mut result = String::new();
    let mut digits = false;
    for c in folded.chars() {
        let digit = unicode_general_category::get_general_category(c)
            == unicode_general_category::GeneralCategory::DecimalNumber;
        if digit {
            if !digits {
                result.push('#');
            }
        } else {
            result.push(c);
        }
        digits = digit;
    }
    result
}

fn fractions(b: BoxPoints, width: f64, height: f64) -> BoxPoints {
    [b[0] / width, b[1] / height, b[2] / width, b[3] / height]
}
fn padded(b: BoxPoints, width: f64, height: f64, pad: f64) -> BoxPoints {
    [
        (b[0] - pad).max(0.),
        (b[1] - pad).max(0.),
        (b[2] + pad).min(width),
        (b[3] + pad).min(height),
    ]
}
fn box_order(a: BoxPoints, b: BoxPoints) -> std::cmp::Ordering {
    [1, 0, 3, 2]
        .into_iter()
        .map(|i| a[i].total_cmp(&b[i]))
        .find(|order| !order.is_eq())
        .unwrap_or(std::cmp::Ordering::Equal)
}
fn iou(a: BoxPoints, b: BoxPoints) -> f64 {
    let intersection =
        (a[2].min(b[2]) - a[0].max(b[0])).max(0.) * (a[3].min(b[3]) - a[1].max(b[1])).max(0.);
    intersection / ((a[2] - a[0]) * (a[3] - a[1]) + (b[2] - b[0]) * (b[3] - b[1]) - intersection)
}

struct Group {
    text: String,
    length: usize,
    seed: BoxPoints,
    order: usize,
    boxes: BTreeMap<u32, BoxPoints>,
}
fn rank(a: &Group, b: &Group) -> std::cmp::Ordering {
    b.boxes
        .len()
        .cmp(&a.boxes.len())
        .then(b.length.cmp(&a.length))
        .then_with(|| box_order(a.seed, b.seed))
        .then(a.text.cmp(&b.text))
        .then(a.order.cmp(&b.order))
}
fn median(group: &Group, pages: &[&TextPage]) -> BoxPoints {
    let boxes: Vec<_> = pages
        .iter()
        .filter_map(|p| {
            group
                .boxes
                .get(&p.page)
                .map(|b| fractions(*b, p.width, p.height))
        })
        .collect();
    std::array::from_fn(|i| {
        let mut values: Vec<_> = boxes.iter().map(|b| b[i]).collect();
        values.sort_by(f64::total_cmp);
        let middle = values.len() / 2;
        if values.len().is_multiple_of(2) {
            values[middle - 1].midpoint(values[middle])
        } else {
            values[middle]
        }
    })
}

/// Pure, bounded proposal generation. An exhausted text is dropped in full;
/// other texts retain exact first-group semantics. Explain refusal if none survive.
fn propose_keyed(pages: &[TextPage], key: &[u8; 32]) -> Result<Vec<Proposal>, AppError> {
    let mut ordered: Vec<_> = pages.iter().collect();
    ordered.sort_by_key(|p| p.page);
    let mut groups: Vec<Group> = Vec::new();
    let mut by_text: BTreeMap<String, BTreeSet<usize>> = BTreeMap::new();
    let mut dropped = BTreeSet::new();
    let mut budgets = BTreeMap::new();
    let mut exhausted = false;
    let required = 3.max((pages.len() * 3).div_ceil(5));
    for (page_index, page) in ordered.iter().enumerate() {
        // A group is eligible once per page. Newly created and already matched
        // groups cannot be scanned again on this page. Indices retain seed order.
        // Copy only entries touched by this page, once each. Total copied indices
        // are bounded by the groups for this page's texts, not the document vocabulary.
        let mut available = BTreeMap::new();
        // Duplicate raw lines are common (and can be hostile). Normalize each
        // distinct value once per page, with memory bounded by that page's text.
        let mut normalized = BTreeMap::new();
        let mut lines: Vec<_> = page
            .lines
            .iter()
            .enumerate()
            .map(|(index, line)| {
                let text = normalized
                    .entry(line.text.as_str())
                    .or_insert_with(|| normalize(&line.text));
                (line.bounds, text.clone(), index)
            })
            .collect();
        lines.sort_by(|a, b| box_order(a.0, b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
        for (bounds, text, _) in lines {
            let length = text.chars().count();
            if !(4..=200).contains(&length) {
                continue;
            }
            if dropped.contains(&text) {
                continue;
            }
            let candidate = fractions(bounds, page.width, page.height);
            let candidates = available
                .entry(text.clone())
                .or_insert_with(|| by_text.get(&text).cloned().unwrap_or_default());
            let comparisons = budgets.entry(text.clone()).or_insert(2_000_000_usize);
            let mut matching = None;
            for &i in candidates.iter() {
                if *comparisons == 0 {
                    dropped.insert(text.clone());
                    exhausted = true;
                    break;
                }
                *comparisons -= 1;
                // Fraction conversion and area arithmetic can put an exact 0.8 edge
                // a few ulps below it. Tolerate only floating-point noise.
                let overlap = iou(groups[i].seed, candidate);
                if overlap >= 0.8 || (overlap - 0.8).abs() <= 4. * f64::EPSILON {
                    matching = Some(i);
                    break;
                }
            }
            if dropped.contains(&text) {
                by_text.remove(&text);
                available.remove(&text);
                continue;
            }
            if let Some(index) = matching {
                candidates.remove(&index);
                groups[index].boxes.insert(page.page, bounds);
            } else {
                by_text
                    .entry(text.clone())
                    .or_default()
                    .insert(groups.len());
                groups.push(Group {
                    text,
                    length,
                    seed: candidate,
                    order: groups.len(),
                    boxes: BTreeMap::from([(page.page, bounds)]),
                });
            }
        }
        let remaining = pages.len() - page_index - 1;
        if remaining < required {
            by_text.retain(|text, indices| {
                if indices
                    .iter()
                    .any(|&i| groups[i].boxes.len() + remaining >= required)
                {
                    true
                } else {
                    // Even the best group cannot reach the threshold, nor can a
                    // future seed. Stop tracking this text on subsequent pages.
                    dropped.insert(text.clone());
                    false
                }
            });
        }
    }
    groups.retain(|g| !dropped.contains(&g.text) && g.boxes.len() >= required);
    if groups.is_empty() && exhausted {
        return Err(AppError::Invalid(COMPARISON_LIMIT));
    }
    groups.sort_by(rank);
    Ok(groups
        .into_iter()
        .take(3)
        .map(|g| proposal(g, &ordered, key))
        .collect())
}

fn proposal(g: Group, ordered: &[&TextPage], key: &[u8; 32]) -> Proposal {
    let median = median(&g, ordered);
    Proposal {
        fingerprint: fingerprint(key, &g.text),
        text: g.text,
        boxes: ordered
            .iter()
            .filter_map(|p| {
                g.boxes
                    .get(&p.page)
                    .map(|b| (p.page, padded(*b, p.width, p.height, 2.)))
            })
            .collect(),
        absent: ordered
            .iter()
            .filter(|p| !g.boxes.contains_key(&p.page))
            .map(|p| {
                (
                    p.page,
                    padded(
                        [
                            median[0] * p.width,
                            median[1] * p.height,
                            median[2] * p.width,
                            median[3] * p.height,
                        ],
                        p.width,
                        p.height,
                        2.,
                    ),
                )
            })
            .collect(),
    }
}

fn fingerprint(key: &[u8; 32], text: &str) -> String {
    hex::encode(
        Sha256::new()
            .chain_update(key)
            .chain_update(text.as_bytes())
            .finalize(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn propose(pages: &[TextPage]) -> Vec<Proposal> {
        propose_keyed(pages, &[0; 32]).unwrap()
    }
    fn pages(count: u32, text: &str) -> Vec<TextPage> {
        (1..=count)
            .map(|page| TextPage {
                page,
                width: 100.,
                height: 200.,
                lines: vec![worker::records::TextLine {
                    text: text.into(),
                    bounds: [1., 2., 41., 12.],
                }],
            })
            .collect()
    }
    #[test]
    fn normalization_is_nfkc_full_casefold_whitespace_and_decimal_runs() {
        assert_eq!(
            normalize("  Ｓtraße\tΣς\nPAGE １２٣٤ ９ "),
            "strasse σσ page # #"
        );
        assert_eq!(normalize("ﬃ \u{212a} e\u{301} 12-34"), "ffi k é #-#");
    }
    #[test]
    fn proposal_padding_preserves_original_point_coordinates() {
        let mut input = pages(3, "test");
        for page in &mut input {
            page.width = 317.;
            page.lines[0].bounds = [3.1, 2., 41., 12.];
        }
        assert_eq!(propose(&input)[0].boxes[&1][0], 3.1 - 2.);
    }
    #[test]
    fn hostile_identical_lines_are_bounded() {
        let mut input = pages(64, "same buyer mark");
        for page in &mut input {
            page.lines = vec![
                worker::records::TextLine {
                    text: "same buyer mark".into(),
                    bounds: [1., 2., 41., 12.],
                };
                20_000
            ];
            worker::records::validate_lines(
                &page.lines,
                &worker::Page {
                    n: page.page,
                    width: page.width,
                    height: page.height,
                    rotate: 0,
                    user_unit: 1.,
                    transform: [1., 0., 0., -1., 0., page.height],
                    visible: [0., 0., page.width, page.height],
                    tagged: false,
                },
            )
            .unwrap();
        }
        // Run away from the test thread so the old quadratic algorithm fails within
        // a bounded interval rather than hanging the entire regression invocation.
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let result = propose(&input);
            tx.send(result.iter().map(|p| p.boxes.len()).collect::<Vec<_>>())
                .unwrap();
        });
        assert_eq!(
            rx.recv_timeout(std::time::Duration::from_secs(30)).unwrap(),
            [64; 3]
        );
    }
    #[test]
    fn colliding_numeric_text_does_not_hide_a_real_footer() {
        let mut input = pages(3, "real footer");
        let expected = propose(&input).remove(0);
        for page in &mut input {
            page.lines.extend((0..2001).map(|i| {
                let left = f64::from(i) / 100.;
                let top = f64::from(page.page) * 20.;
                worker::records::TextLine {
                    text: format!("{i} gp"),
                    bounds: [left, top, left + 0.005, top + 1.],
                }
            }));
        }
        let result = propose(&input);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].text, expected.text);
        assert_eq!(result[0].fingerprint, expected.fingerprint);
        assert_eq!(result[0].boxes, expected.boxes);
        assert_eq!(result[0].absent, expected.absent);
    }
    #[test]
    fn impossible_text_is_dropped_before_late_collisions() {
        let mut input = pages(5, "unused");
        for page in &mut input {
            page.lines = if page.page == 1 || page.page == 5 {
                (0..2001)
                    .map(|i| {
                        let left = f64::from(i) / 100.;
                        let top = f64::from(page.page) * 20.;
                        worker::records::TextLine {
                            text: format!("{i}.0%"),
                            bounds: [left, top, left + 0.005, top + 1.],
                        }
                    })
                    .collect()
            } else {
                Vec::new()
            };
        }
        // After page four, no existing or future group can reach three pages.
        // The final page must not consume the budget for an already impossible text.
        assert!(propose_keyed(&input, &[0; 32]).unwrap().is_empty());
    }
    #[test]
    fn hostile_nonmatching_geometry_refuses_without_partial_proposals() {
        let mut input = pages(3, "same text");
        for page in &mut input {
            page.lines = (0..2001)
                .map(|i| {
                    let left = f64::from(i) / 100.;
                    let top = f64::from(page.page) * 20.;
                    worker::records::TextLine {
                        text: "same text".into(),
                        bounds: [left, top, left + 0.005, top + 1.],
                    }
                })
                .collect();
        }
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            tx.send(matches!(
                propose_keyed(&input, &[0; 32]),
                Err(AppError::Invalid(COMPARISON_LIMIT))
            ))
            .unwrap();
        });
        assert!(rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap());
    }
    #[test]
    fn grouping_uses_seed_iou_not_transitive_overlap_and_one_line_per_page() {
        let seed = [0., 0., 10., 10.];
        assert_eq!(iou(seed, [0., 0., 8., 10.]), 0.8);
        assert!(iou(seed, [0., 0., 7.999, 10.]) < 0.8);
        assert!(iou(seed, [0., 0., 8.001, 10.]) > 0.8);
        let mut input = pages(3, "test");
        input[0].lines[0].bounds = seed;
        input[1].lines[0].bounds = [0., 0., 8., 10.];
        input[2].lines[0].bounds = [0., 0., 6.4, 10.];
        assert!(
            propose(&input).is_empty(),
            "third line overlaps second, not seed"
        );
        input[2].lines[0].bounds = seed;
        assert_eq!(propose(&input)[0].boxes.len(), 3);
        for page in &mut input {
            page.lines.push(worker::records::TextLine {
                text: "test".into(),
                bounds: seed,
            });
        }
        assert_eq!(propose(&input).len(), 2);
        assert!(propose(&input).iter().all(|g| g.boxes.len() == 3));
        input.reverse();
        assert_eq!(propose(&input)[0].boxes[&1], [0., 0., 12., 12.]);
    }
    #[test]
    fn ranking_has_a_total_order_on_every_key() {
        fn group() -> Group {
            Group {
                text: "same".into(),
                length: 4,
                seed: [0.1, 0.2, 0.3, 0.4],
                order: 10,
                boxes: BTreeMap::from([(1, [0.; 4]), (2, [0.; 4]), (3, [0.; 4])]),
            }
        }
        for key in 0..8 {
            let a = group();
            let mut b = group();
            match key {
                0 => {
                    b.boxes.remove(&3);
                }
                1 => b.length = 3,
                2 => b.seed[1] += 0.01,
                3 => b.seed[0] += 0.01,
                4 => b.seed[3] += 0.01,
                5 => b.seed[2] += 0.01,
                6 => b.text = "z...".into(),
                _ => b.order += 1,
            }
            assert!(rank(&a, &b).is_lt(), "key {key}");
            assert!(rank(&b, &a).is_gt(), "key {key}");
        }
        assert!(rank(&group(), &group()).is_eq());
        let mut input = pages(3, "aaaa");
        for page in &mut input {
            for (index, text) in ["bbbb", "cccc", "dddd"].iter().enumerate() {
                page.lines.push(worker::records::TextLine {
                    text: (*text).into(),
                    bounds: [
                        1.,
                        20. + f64::from(u32::try_from(index).unwrap()) * 20.,
                        41.,
                        30. + f64::from(u32::try_from(index).unwrap()) * 20.,
                    ],
                });
            }
        }
        let result = propose(&input);
        assert_eq!(
            result.iter().map(|p| p.text.as_str()).collect::<Vec<_>>(),
            ["aaaa", "bbbb", "cccc"]
        );
    }
    #[test]
    fn grouping_uses_page_fractions_and_medians_handle_even_and_different_page_sizes() {
        let mut input = pages(5, "test");
        input[1].width = 200.;
        input[1].height = 400.;
        input[1].lines[0].bounds = [2., 4., 82., 24.];
        input[2].lines[0].bounds = [2., 2., 42., 12.];
        input[3].lines[0].bounds = [2., 2., 42., 12.];
        input[4].lines.clear();
        let result = propose(&input);
        assert_eq!(result[0].boxes[&2], [0., 2., 84., 26.]);
        assert_eq!(result[0].absent[&5], [0., 0., 43.5, 14.]);
        let mut edge = pages(3, "test");
        for page in &mut edge {
            page.lines[0].bounds = [0., 199., 100., 200.];
        }
        assert_eq!(propose(&edge)[0].boxes[&1], [0., 197., 100., 200.]);
    }
    #[test]
    fn thresholds_clipping_absence_median_and_empty_layers() {
        assert!(propose(&pages(2, "test")).is_empty());
        assert!(propose(&pages(3, "abc")).is_empty());
        assert_eq!(propose(&pages(3, "abcd")).len(), 1);
        assert_eq!(propose(&pages(3, &"x".repeat(200))).len(), 1);
        assert!(propose(&pages(3, &"x".repeat(201))).is_empty());
        let mut input = pages(5, "test");
        input[3].lines.clear();
        input[4].lines.clear();
        let result = propose(&input);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].boxes.len(), 3);
        assert_eq!(result[0].boxes[&1], [0., 0., 43., 14.]);
        assert_eq!(result[0].absent[&4], [0., 0., 43., 14.]);
        input.push(TextPage {
            page: 6,
            width: 100.,
            height: 200.,
            lines: vec![],
        });
        assert!(propose(&input).is_empty());
        for page in &mut input {
            page.lines.clear();
        }
        assert!(propose(&input).is_empty());
    }
}
