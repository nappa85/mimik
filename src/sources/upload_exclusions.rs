//! Deterministic repeated-text proposals. Text never enters the exclusion history.
use super::{
    AppError, Connection, OptionalExtension, OwnerWrite, RenderSettings, RequestAuth, Transaction,
    Upload, Uploads, hash, json, now, params, parse, remaining, worker,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

pub type BoxPoints = [f64; 4];
pub(crate) const COMPARISON_LIMIT: &str =
    "Repeated-text comparison limit reached; draw an exclusion by hand.";

/// Boxes on one page: the Owner's header, footer, page number and side tabs. The migration view
/// bounds one page at 384 bytes, which eight boxes of at most 33 bytes each stay far below.
pub const MAX_BOXES_PER_PAGE: usize = 8;
/// Confirmed proposals in one exclusion; the migration checks the same count.
pub const MAX_FINGERPRINTS: usize = 8;

/// Bounds of one proposal run (decision SRC-15): the work and the memory of a run that holds the
/// upload gate. Exhaustion of any of them refuses with [`COMPARISON_LIMIT`].
#[derive(Clone, Copy, Debug)]
struct Limits {
    /// Distinct comparisons of one text (whole text dropped when exhausted).
    per_text: usize,
    /// Comparisons of the whole run (bounds the time the upload gate is held).
    comparisons: u64,
    /// Live groups, tombstones included.
    groups: usize,
    /// Live accounting of group, text and box entries, in bytes.
    memory: usize,
    /// Deterministic work units of the whole run (`WORK_BUDGET`).
    work: u64,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            per_text: 2_000_000,
            comparisons: 50_000_000,
            groups: 50_000,
            memory: 64 * 1024 * 1024,
            work: WORK_BUDGET,
        }
    }
}
/// Work units of one proposal run (comparisons, index steps of scans and prunes, lines, and
/// parsed text bytes). Unmeasured: slice 6 sets it from timings on large inputs.
const WORK_BUDGET: u64 = 500_000_000;
/// Work units charged for each text line of a page (normalising, digesting, sorting).
const LINE_WORK: u64 = 8;
/// Parsed text bytes that cost one work unit (a page's JSON is read, hashed and parsed once).
const BYTES_PER_WORK: u64 = 8;
/// Accounted bytes of one group (the structure and its seed) besides its text.
const GROUP_COST: usize = 160;
/// Accounted bytes of one stored box of a group (a `BTreeMap` entry).
const BOX_COST: usize = 64;
/// Accounted bytes of a dropped text's digest in the skip set.
const DROPPED_COST: usize = 48;
/// A tracked text is held as the grouping key and as the budget key.
const fn text_cost(length: usize) -> usize {
    2 * length + 200
}

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

/// The stored exclusion: 1-8 distinct boxes on each listed page, in displayed points rounded
/// outward to 0.01 pt (see `round_boxes`), and the fingerprints of the confirmed proposals.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Exclusion {
    pub pages: Vec<u32>,
    pub boxes: BTreeMap<u32, Vec<BoxPoints>>,
    pub proposal_fingerprints: Vec<String>,
    pub rationale: String,
}

/// One confirmed proposal: its fingerprint (never an index), the pages of its found boxes the
/// Owner leaves out, and the pages where it is extended to its offered median box.
#[derive(Clone, Debug, Default)]
pub struct Confirmation {
    pub fingerprint: String,
    pub skip: Vec<u32>,
    pub absent: Vec<u32>,
}

/// What the Owner submits. The boxes of confirmed proposals are rebuilt on the server from a
/// fresh proposal run; only the per-page manual boxes travel in the request.
#[derive(Clone, Debug, Default)]
pub struct ExclusionRequest {
    pub confirmed: Vec<Confirmation>,
    pub manual: BTreeMap<u32, Vec<BoxPoints>>,
    /// Further boxes posted for pages whose region was not ticked: ignored (the page is dropped
    /// as before) only while they equal the page's saved further boxes, which the form prefills.
    pub unticked_extra: BTreeMap<u32, Vec<BoxPoints>>,
    /// The first and last page of the screen that was edited: the current boxes of the pages
    /// outside it are kept. `None` replaces the whole exclusion.
    pub window: Option<(u32, u32)>,
    pub rationale: String,
    /// The missing-text warning of proposal extension was acknowledged.
    pub extend_absent: bool,
}

pub(super) fn load(db: &Connection, id: &str) -> Result<(i64, Option<Exclusion>), AppError> {
    let row = db
        .query_row(
            "SELECT revision,pages,boxes,proposal_fingerprints,rationale,removed
         FROM source_upload_exclusions WHERE upload_id=?1 ORDER BY revision DESC LIMIT 1",
            [id],
            |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, bool>(5)?,
                ))
            },
        )
        .optional()?;
    match row {
        None => Ok((0, None)),
        Some((revision, pages, boxes, fingerprints, rationale, removed)) => Ok((
            revision,
            if removed {
                None
            } else {
                Some(Exclusion {
                    pages: parse(&pages)?,
                    boxes: parse(&boxes)?,
                    proposal_fingerprints: parse(&fingerprints)?,
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
        // One page at a time: read, hash-verify, parse, validate, feed, drop. Only the candidate
        // groups of the proposer outlive a page (decision 8).
        let facts: HashMap<u32, &worker::Page> = u
            .inspection
            .as_ref()
            .ok_or(AppError::Conflict)?
            .pages
            .iter()
            .map(|p| (p.n, p))
            .collect();
        let selected: HashSet<u32> = u.selected_pages.iter().copied().collect();
        let chosen: Vec<_> = u
            .pages
            .iter()
            .filter(|p| selected.contains(&p.pdf_page))
            .collect();
        let mut proposer = Proposer::new(chosen.len(), Limits::default());
        for page in chosen {
            let bytes = self.0.files.read_text(&u.id, page.pdf_page)?;
            // Read, hash and parse are work too: charged before the page is parsed.
            proposer.spend(bytes.len() as u64 / BYTES_PER_WORK)?;
            if bytes.len() as u64 != page.text_bytes || hash(&bytes) != page.text_sha256 {
                return Err(AppError::Invalid("Invalid staged text."));
            }
            // Parsing errors are deliberately mapped: serde diagnostics may contain text.
            let lines: Vec<worker::records::TextLine> = serde_json::from_slice(&bytes)
                .map_err(|_| AppError::Invalid("Invalid staged text."))?;
            drop(bytes);
            let fact = facts.get(&page.pdf_page).ok_or(AppError::Conflict)?;
            worker::records::validate_lines(&lines, fact)
                .map_err(|_| AppError::Invalid("Invalid staged text."))?;
            proposer.feed(&TextPage {
                page: page.pdf_page,
                width: page.width_points,
                height: page.height_points,
                lines,
            })?;
        }
        Ok((proposer.finish(&key)?, key))
    }

    /// Append an Owner-confirmed exclusion in displayed points. Both revisions must match. The
    /// boxes of confirmed proposals are rebuilt from a fresh proposal run; the request carries
    /// their fingerprints and the per-page manual boxes only. Boxes are rounded outward to
    /// 0.01 pt and de-duplicated before they are stored.
    /// # Errors
    /// Refuses stale admission/revisions, invalid geometry, rationale or fingerprints, more
    /// than eight boxes on a page, and proposal extension without its acknowledged warning.
    pub fn set_exclusion(
        &self,
        owner: &OwnerWrite,
        id: &str,
        revision: i64,
        exclusion_revision: i64,
        request: &ExclusionRequest,
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
        check_rationale(&request.rationale)?;
        let fingerprints: BTreeSet<&str> = request
            .confirmed
            .iter()
            .map(|c| c.fingerprint.as_str())
            .collect();
        if request.confirmed.len() > MAX_FINGERPRINTS
            || fingerprints.len() != request.confirmed.len()
        {
            return Err(AppError::Invalid("Invalid proposal."));
        }
        // Unticking a page drops it; the further boxes the form prefilled for it go with it. Any
        // other further boxes were typed for a page that is no longer ticked: refused.
        for (n, extra) in &request.unticked_extra {
            let saved = u
                .exclusion
                .as_ref()
                .and_then(|e| e.boxes.get(n))
                .and_then(|b| b.get(1..))
                .unwrap_or(&[]);
            if extra.as_slice() != saved {
                return Err(AppError::Invalid(
                    "Further boxes need their page's region ticked.",
                ));
            }
        }
        let mut raw: BTreeMap<u32, Vec<BoxPoints>> = BTreeMap::new();
        if let (Some((from, to)), Some(current)) = (request.window, &u.exclusion) {
            for (n, boxes) in &current.boxes {
                if !(from..=to).contains(n) {
                    raw.entry(*n).or_default().extend(boxes);
                }
            }
        }
        let verified_key = if request.confirmed.is_empty() {
            None
        } else {
            let (proposals, key) = self.proposals(&u)?;
            for confirmation in &request.confirmed {
                confirm(&proposals, confirmation, request.extend_absent, &mut raw)?;
            }
            Some(key)
        };
        for (n, boxes) in &request.manual {
            raw.entry(*n).or_default().extend(boxes);
        }
        let boxes = round_boxes(raw, &usable_pages(&u))?;
        let confirmed: Vec<String> = request
            .confirmed
            .iter()
            .map(|c| c.fingerprint.clone())
            .collect();
        // A windowed edit keeps the boxes of the other screens, so it keeps the provenance of
        // those boxes too: the stored fingerprints join the newly confirmed ones.
        let proposal_fingerprints = match (&u.exclusion, request.window) {
            (Some(current), Some(_)) => {
                merge_fingerprints(&current.proposal_fingerprints, &confirmed)?
            }
            _ => confirmed,
        };
        let exclusion = Exclusion {
            pages: boxes.keys().copied().collect(),
            boxes,
            proposal_fingerprints,
            rationale: request.rationale.clone(),
        };
        let exclusion = &exclusion;
        validate(exclusion, &u)?;
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
/// The selected pages with a current rendering, by number: one pass over the rows.
fn usable_pages(u: &Upload) -> HashMap<u32, &super::PageModel> {
    let selected: HashSet<u32> = u.selected_pages.iter().copied().collect();
    u.pages
        .iter()
        .filter(|p| {
            selected.contains(&p.pdf_page)
                && p.settings == RenderSettings::default()
                && p.renderer_crate_version == worker::CRATE_VERSION
                && p.mupdf_version == worker::MUPDF_VERSION
        })
        .map(|p| (p.pdf_page, p))
        .collect()
}

/// `v` rounded up to 0.01 pt (the shortest decimal text of the result has at most two decimals).
fn grid_up(v: f64) -> f64 {
    let mut k = (v * 100.).ceil();
    if (k - 1.) / 100. >= v {
        k -= 1.;
    }
    // `k / 100` is the nearest double to the decimal and may sit one ulp below `v`.
    if k / 100. < v {
        k += 1.;
    }
    unsigned(k / 100.)
}
/// `v` rounded down to 0.01 pt.
fn grid_down(v: f64) -> f64 {
    let mut k = (v * 100.).floor();
    if (k + 1.) / 100. <= v {
        k += 1.;
    }
    if k / 100. > v {
        k -= 1.;
    }
    unsigned(k / 100.)
}
/// Negative zero would store as `-0.0`, a different text from `0.0`.
fn unsigned(v: f64) -> f64 {
    if v == 0. { 0. } else { v }
}
/// A box's room on its page: the page size rounded up to 0.01 pt, so a box that touches the page
/// edge of a page such as 595.2756 pt wide, rounded outward, stays valid and stores at most seven
/// characters per coordinate (the migration's 384 bytes per page rest on that).
fn extent(page: &super::PageModel) -> (f64, f64) {
    box_extent(page.width_points, page.height_points)
}
/// The largest right and bottom coordinate a stored box may have on a page of this size
/// (points): the size rounded up to 0.01 pt. The form's inputs use the same figures.
#[must_use]
pub fn box_extent(width_points: f64, height_points: f64) -> (f64, f64) {
    (grid_up(width_points), grid_up(height_points))
}
fn check_box(b: &BoxPoints, (width, height): (f64, f64)) -> Result<(), AppError> {
    if !b.iter().all(|v| v.is_finite() && *v >= 0.)
        || b[0] >= b[2]
        || b[1] >= b[3]
        || b[2] > width
        || b[3] > height
    {
        return Err(AppError::Invalid("Invalid displayed exclusion box."));
    }
    Ok(())
}

/// Round every box outward to 0.01 pt (minimum floored, maximum ceiled, so coverage is never
/// lost), then make equal boxes bit-identical and drop the repeats: the migration compares boxes
/// as text (`[1,1,2,2]` and `[1.0,1,2,2]` differ). Refuses a box outside its page, a page that
/// is not a selected rendered page and more than [`MAX_BOXES_PER_PAGE`] boxes on a page.
fn round_boxes(
    raw: BTreeMap<u32, Vec<BoxPoints>>,
    usable: &HashMap<u32, &super::PageModel>,
) -> Result<BTreeMap<u32, Vec<BoxPoints>>, AppError> {
    let mut result = BTreeMap::new();
    for (n, boxes) in raw {
        let page = usable
            .get(&n)
            .ok_or(AppError::Invalid("Render selected exclusion pages first."))?;
        let room = extent(page);
        let mut rounded = Vec::with_capacity(boxes.len());
        for b in boxes {
            check_box(&b, room)?;
            rounded.push([
                grid_down(b[0]),
                grid_down(b[1]),
                grid_up(b[2]).min(room.0),
                grid_up(b[3]).min(room.1),
            ]);
        }
        rounded.sort_by(|a, b| box_order(*a, *b));
        rounded.dedup();
        if rounded.len() > MAX_BOXES_PER_PAGE {
            return Err(AppError::Invalid("At most eight exclusion boxes per page."));
        }
        result.insert(n, rounded);
    }
    Ok(result)
}

/// The boxes one confirmed proposal adds: its found boxes (minus the skipped pages) and, once
/// the warning is acknowledged, its median box on the pages the Owner extends it to. Every
/// proposal with this fingerprint counts (one text can recur at two positions).
fn confirm(
    proposals: &[Proposal],
    confirmation: &Confirmation,
    extend_absent: bool,
    raw: &mut BTreeMap<u32, Vec<BoxPoints>>,
) -> Result<(), AppError> {
    let matching: Vec<_> = proposals
        .iter()
        .filter(|p| p.fingerprint == confirmation.fingerprint)
        .collect();
    if matching.is_empty() {
        return Err(AppError::Invalid("Invalid proposal."));
    }
    let skip: HashSet<u32> = confirmation.skip.iter().copied().collect();
    if confirmation
        .skip
        .iter()
        .any(|n| !matching.iter().any(|p| p.boxes.contains_key(n)))
    {
        return Err(AppError::Invalid("Invalid proposal pages."));
    }
    for proposal in &matching {
        for (n, b) in &proposal.boxes {
            if !skip.contains(n) {
                raw.entry(*n).or_default().push(*b);
            }
        }
    }
    if !confirmation.absent.is_empty() && !extend_absent {
        return Err(AppError::Invalid("Confirm the absent-page warning first."));
    }
    for n in &confirmation.absent {
        let offered: Vec<BoxPoints> = matching
            .iter()
            .filter_map(|p| p.absent.get(n).copied())
            .collect();
        if offered.is_empty() {
            return Err(AppError::Invalid("Invalid proposal pages."));
        }
        raw.entry(*n).or_default().extend(offered);
    }
    Ok(())
}

pub(super) fn validate(e: &Exclusion, u: &Upload) -> Result<(), AppError> {
    check_rationale(&e.rationale)?;
    if e.pages.is_empty()
        || e.pages.windows(2).any(|p| p[0] >= p[1])
        || e.pages.len() != e.boxes.len()
        || e.proposal_fingerprints.len() > MAX_FINGERPRINTS
        || e.proposal_fingerprints
            .iter()
            .collect::<BTreeSet<_>>()
            .len()
            != e.proposal_fingerprints.len()
        || e.proposal_fingerprints.iter().any(|f| {
            f.len() != 64
                || !f
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
    {
        return Err(AppError::Invalid("Invalid exclusion pages."));
    }
    let usable = usable_pages(u);
    for n in &e.pages {
        let page = usable
            .get(n)
            .ok_or(AppError::Invalid("Render selected exclusion pages first."))?;
        let boxes = e
            .boxes
            .get(n)
            .ok_or(AppError::Invalid("Missing exclusion box."))?;
        if boxes.is_empty() || boxes.len() > MAX_BOXES_PER_PAGE {
            return Err(AppError::Invalid("At most eight exclusion boxes per page."));
        }
        for (i, b) in boxes.iter().enumerate() {
            check_box(b, extent(page))?;
            if boxes[..i].contains(b) {
                return Err(AppError::Invalid("Duplicate exclusion box."));
            }
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
         proposal_fingerprints,removed,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
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
            json(&e.proposal_fingerprints)?,
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
    /// The group alone can no longer reach the threshold: its boxes are freed, its seed and
    /// index stay so that later scans and budgets behave as if it were whole (N1).
    tombstone: bool,
}
impl Group {
    fn can_reach(&self, remaining: usize, required: usize) -> bool {
        !self.tombstone && self.boxes.len() + remaining >= required
    }
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
/// The size of a fed page, in page order: all a finished proposal needs of the other pages.
struct PageSize {
    page: u32,
    width: f64,
    height: f64,
}
fn median(group: &Group, pages: &[PageSize]) -> BoxPoints {
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

/// The fingerprints of an exclusion written from a windowed edit: those already stored (their
/// boxes on other screens are kept) and the newly confirmed ones, at most [`MAX_FINGERPRINTS`].
fn merge_fingerprints(current: &[String], confirmed: &[String]) -> Result<Vec<String>, AppError> {
    let mut all: Vec<String> = Vec::new();
    for fingerprint in current.iter().chain(confirmed) {
        if !all.contains(fingerprint) {
            all.push(fingerprint.clone());
        }
    }
    if all.len() > MAX_FINGERPRINTS {
        return Err(AppError::Invalid("At most eight confirmed proposals."));
    }
    Ok(all)
}

fn limit() -> AppError {
    AppError::Invalid(COMPARISON_LIMIT)
}
fn text_digest(text: &str) -> [u8; 16] {
    let mut digest = [0; 16];
    digest.copy_from_slice(&Sha256::digest(text.as_bytes())[..16]);
    digest
}

/// Streaming, bounded proposal generation (decision 8): pages are fed one at a time in page
/// order and only candidate groups are kept. An exhausted text is dropped in full; other texts
/// keep the exact first-group semantics of [`propose_oracle`] (N1), which the tests compare.
/// Refuses with [`COMPARISON_LIMIT`] when a [`Limits`] bound is reached.
struct Proposer {
    limits: Limits,
    total: usize,
    required: usize,
    fed: usize,
    sizes: Vec<PageSize>,
    /// Live groups by creation order (the order of the rank's last key).
    groups: BTreeMap<usize, Group>,
    next_order: usize,
    by_text: BTreeMap<String, BTreeSet<usize>>,
    /// Digests of the texts no group of which can reach the threshold: skipped from then on.
    dropped: HashSet<[u8; 16]>,
    budgets: BTreeMap<String, usize>,
    exhausted: bool,
    comparisons: u64,
    memory: usize,
    work: u64,
}
impl Proposer {
    /// `total` is the number of pages that will be fed.
    fn new(total: usize, limits: Limits) -> Self {
        Self {
            limits,
            total,
            required: 3.max((total * 3).div_ceil(5)),
            fed: 0,
            sizes: Vec::new(),
            groups: BTreeMap::new(),
            next_order: 0,
            by_text: BTreeMap::new(),
            dropped: HashSet::new(),
            budgets: BTreeMap::new(),
            exhausted: false,
            comparisons: 0,
            memory: 0,
            work: 0,
        }
    }
    fn charge(&mut self, bytes: usize) -> Result<(), AppError> {
        self.memory += bytes;
        if self.memory > self.limits.memory {
            return Err(limit());
        }
        Ok(())
    }
    /// Live entries: groups (tombstones included) and their stored boxes.
    #[cfg(test)]
    fn entries(&self) -> usize {
        self.groups.len() + self.groups.values().map(|g| g.boxes.len()).sum::<usize>()
    }
    /// Stop tracking `text`: free its groups and budget, remember its digest.
    fn drop_text(&mut self, text: &str) {
        let mut freed = 0;
        for index in self.by_text.remove(text).unwrap_or_default() {
            if let Some(group) = self.groups.remove(&index) {
                freed += GROUP_COST + group.text.len() + BOX_COST * group.boxes.len();
            }
        }
        if self.budgets.remove(text).is_some() {
            freed += text_cost(text.len());
        }
        self.memory = self.memory.saturating_sub(freed);
        if self.dropped.insert(text_digest(text)) {
            self.memory += DROPPED_COST;
        }
    }
    fn feed(&mut self, page: &TextPage) -> Result<(), AppError> {
        let index = self.fed;
        self.fed += 1;
        self.sizes.push(PageSize {
            page: page.page,
            width: page.width,
            height: page.height,
        });
        self.spend_lines(page)?;
        // A group is eligible once per page: the ones matched on this page, and the ones created
        // on it (orders from `first_new`), are skipped. Nothing is copied from `by_text`.
        let first_new = self.next_order;
        let mut consumed: HashSet<usize> = HashSet::new();
        for (bounds, text, _) in self.eligible_lines(page)? {
            let length = text.chars().count();
            if !(4..=200).contains(&length) || self.dropped.contains(&text_digest(&text)) {
                continue;
            }
            let candidate = fractions(bounds, page.width, page.height);
            if !self.budgets.contains_key(&text) {
                self.charge(text_cost(text.len()))?;
            }
            let budget = self
                .budgets
                .entry(text.clone())
                .or_insert(self.limits.per_text);
            let mut matching = None;
            let mut exhausted_text = false;
            for &i in self.by_text.get(&text).into_iter().flatten() {
                if i >= first_new {
                    break;
                }
                // Every index step is work, matched, consumed or compared.
                self.work += 1;
                if self.work > self.limits.work {
                    return Err(limit());
                }
                if consumed.contains(&i) {
                    continue;
                }
                if *budget == 0 {
                    exhausted_text = true;
                    break;
                }
                if self.comparisons >= self.limits.comparisons {
                    return Err(limit());
                }
                *budget -= 1;
                self.comparisons += 1;
                // Fraction conversion and area arithmetic can put an exact 0.8 edge a few ulps
                // below it. Tolerate only floating-point noise.
                let overlap = iou(self.groups[&i].seed, candidate);
                if overlap >= 0.8 || (overlap - 0.8).abs() <= 4. * f64::EPSILON {
                    matching = Some(i);
                    break;
                }
            }
            if exhausted_text {
                self.exhausted = true;
                self.drop_text(&text);
                continue;
            }
            if let Some(i) = matching {
                consumed.insert(i);
                // A match on a tombstone is consumed but stores no box.
                let stored = self
                    .groups
                    .get_mut(&i)
                    .is_some_and(|g| !g.tombstone && g.boxes.insert(page.page, bounds).is_none());
                if stored {
                    self.charge(BOX_COST)?;
                }
            } else {
                self.seed(text, length, candidate, page.page, bounds)?;
            }
        }
        let remaining = self.total.saturating_sub(index + 1);
        if remaining < self.required {
            self.prune(remaining)?;
        }
        Ok(())
    }
    /// A new group seeded by a line no existing group of its text took.
    fn seed(
        &mut self,
        text: String,
        length: usize,
        seed: BoxPoints,
        page: u32,
        bounds: BoxPoints,
    ) -> Result<(), AppError> {
        if self.groups.len() >= self.limits.groups {
            return Err(limit());
        }
        self.charge(GROUP_COST + text.len() + BOX_COST)?;
        let order = self.next_order;
        self.next_order += 1;
        self.by_text.entry(text.clone()).or_default().insert(order);
        self.groups.insert(
            order,
            Group {
                text,
                length,
                seed,
                order,
                boxes: BTreeMap::from([(page, bounds)]),
                tombstone: false,
            },
        );
        Ok(())
    }
    /// The lines of a page whose normalized text is 4 to 200 characters, in visiting order (the
    /// others are never used, so they are not kept or sorted). Duplicate raw lines are common and
    /// can be hostile: each distinct raw text is normalized once, and NFKC can expand text about
    /// elevenfold, so every normalized text is charged by its byte length.
    fn eligible_lines(
        &mut self,
        page: &TextPage,
    ) -> Result<Vec<(BoxPoints, String, usize)>, AppError> {
        let mut normalized: BTreeMap<&str, Option<String>> = BTreeMap::new();
        let mut lines = Vec::new();
        for (index, line) in page.lines.iter().enumerate() {
            if !normalized.contains_key(line.text.as_str()) {
                let text = normalize(&line.text);
                self.spend((text.len() as u64).div_ceil(BYTES_PER_WORK))?;
                let kept = (4..=200).contains(&text.chars().count());
                normalized.insert(line.text.as_str(), kept.then_some(text));
            }
            if let Some(Some(text)) = normalized.get(line.text.as_str()) {
                lines.push((line.bounds, text.clone(), index));
            }
        }
        lines.sort_by(|a, b| box_order(a.0, b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
        Ok(lines)
    }
    /// The work of a page's lines: each line, and its text bytes.
    fn spend_lines(&mut self, page: &TextPage) -> Result<(), AppError> {
        let bytes: u64 = page.lines.iter().map(|l| l.text.len() as u64).sum();
        self.spend(page.lines.len() as u64 * LINE_WORK + bytes / BYTES_PER_WORK)
    }
    /// Count `units` of work; the run is refused once the budget is spent.
    fn spend(&mut self, units: u64) -> Result<(), AppError> {
        self.work = self.work.saturating_add(units);
        if self.work > self.limits.work {
            return Err(limit());
        }
        Ok(())
    }
    /// After a page of the tail: a text none of whose groups can reach the threshold is dropped
    /// (nor can a future seed); a group that alone cannot becomes a tombstone.
    fn prune(&mut self, remaining: usize) -> Result<(), AppError> {
        let required = self.required;
        let groups = &mut self.groups;
        let mut freed = 0;
        let mut steps = 0_u64;
        let mut gone = Vec::new();
        self.by_text.retain(|text, indices| {
            // Each index of each tracked text is visited once: that is the work of the prune.
            steps += 1 + indices.len() as u64;
            if indices
                .iter()
                .any(|i| groups[i].can_reach(remaining, required))
            {
                for i in indices.iter() {
                    if let Some(group) = groups.get_mut(i)
                        && !group.tombstone
                        && group.boxes.len() + remaining < required
                    {
                        freed += BOX_COST * group.boxes.len();
                        group.boxes = BTreeMap::new();
                        group.tombstone = true;
                    }
                }
                true
            } else {
                for i in indices.iter() {
                    if let Some(group) = groups.remove(i) {
                        freed += GROUP_COST + group.text.len() + BOX_COST * group.boxes.len();
                    }
                }
                gone.push(text.clone());
                false
            }
        });
        self.memory = self.memory.saturating_sub(freed);
        for text in gone {
            if self.budgets.remove(&text).is_some() {
                self.memory = self.memory.saturating_sub(text_cost(text.len()));
            }
            if self.dropped.insert(text_digest(&text)) {
                self.memory += DROPPED_COST;
            }
        }
        self.spend(steps)
    }
    /// The three best proposals (an error if every candidate was lost to a budget).
    fn finish(self, key: &[u8; 32]) -> Result<Vec<Proposal>, AppError> {
        let required = self.required;
        let mut groups: Vec<Group> = self
            .groups
            .into_values()
            .filter(|g| !g.tombstone && g.boxes.len() >= required)
            .collect();
        if groups.is_empty() && self.exhausted {
            return Err(limit());
        }
        groups.sort_by(rank);
        Ok(groups
            .into_iter()
            .take(3)
            .map(|g| proposal(g, &self.sizes, key))
            .collect())
    }
}

/// The previous whole-corpus function, kept as the oracle for [`Proposer`] (N1). Pure, bounded
/// by the per-text budget only. An exhausted text is dropped in full; other texts retain exact
/// first-group semantics.
#[cfg(test)]
#[allow(clippy::too_many_lines)] // the previous function, unchanged on purpose
fn propose_oracle(pages: &[TextPage], key: &[u8; 32]) -> Result<Vec<Proposal>, AppError> {
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
                    tombstone: false,
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
    let sizes: Vec<_> = ordered
        .iter()
        .map(|p| PageSize {
            page: p.page,
            width: p.width,
            height: p.height,
        })
        .collect();
    Ok(groups
        .into_iter()
        .take(3)
        .map(|g| proposal(g, &sizes, key))
        .collect())
}

/// The streaming proposer over a whole corpus, as the tests feed it.
#[cfg(test)]
fn propose_keyed(pages: &[TextPage], key: &[u8; 32]) -> Result<Vec<Proposal>, AppError> {
    let mut ordered: Vec<_> = pages.iter().collect();
    ordered.sort_by_key(|p| p.page);
    let mut proposer = Proposer::new(pages.len(), Limits::default());
    for page in ordered {
        proposer.feed(page)?;
    }
    proposer.finish(key)
}

fn proposal(g: Group, ordered: &[PageSize], key: &[u8; 32]) -> Proposal {
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

/// How a unit's rectangle stands to the exclusion boxes of its page (route A's rule, which
/// derivation applies after staging: staging only validates and carries the boxes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Masking {
    /// Wholly contained, edges inclusive, in the union of the boxes: rejected. Adjacent boxes
    /// that together cover the unit count.
    Contained,
    /// Positive-area overlap without containment: stays unreviewed and flagged.
    Crossing,
    /// No positive-area overlap: edge contact alone is not crossing.
    Clear,
}

/// Classify `unit` (left, top, right, bottom) against `boxes`.
#[must_use]
pub fn masking(unit: BoxPoints, boxes: &[BoxPoints]) -> Masking {
    let overlap = |b: &BoxPoints| {
        unit[2].min(b[2]) - unit[0].max(b[0]) > 0. && unit[3].min(b[3]) - unit[1].max(b[1]) > 0.
    };
    if unit[0] >= unit[2] || unit[1] >= unit[3] || !boxes.iter().any(overlap) {
        return Masking::Clear;
    }
    // The union covers the rectangle iff it covers every cell of the grid that the box edges
    // cut it into; a cell lies inside or outside each closed box, so its centre decides.
    let cuts = |lo: f64, hi: f64, edges: Vec<f64>| {
        let mut cuts = vec![lo, hi];
        cuts.extend(edges.into_iter().filter(|e| *e > lo && *e < hi));
        cuts.sort_by(f64::total_cmp);
        cuts.dedup();
        cuts
    };
    let xs = cuts(
        unit[0],
        unit[2],
        boxes.iter().flat_map(|b| [b[0], b[2]]).collect(),
    );
    let ys = cuts(
        unit[1],
        unit[3],
        boxes.iter().flat_map(|b| [b[1], b[3]]).collect(),
    );
    let covered = xs.windows(2).all(|x| {
        ys.windows(2).all(|y| {
            let (cx, cy) = (x[0].midpoint(x[1]), y[0].midpoint(y[1]));
            boxes
                .iter()
                .any(|b| b[0] <= cx && cx <= b[2] && b[1] <= cy && cy <= b[3])
        })
    });
    if covered {
        Masking::Contained
    } else {
        Masking::Crossing
    }
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
        // 20,000 identical lines a page make 20,000 groups that each gain a box on every later
        // page (a line matches the next unconsumed group, after stepping over the consumed
        // ones): that is quadratic work a page, and 1.28 million box entries over 64 pages (the
        // 64 MiB box budget). Either budget refuses; a small work budget makes it quick here.
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut hostile = Proposer::new(
                input.len(),
                Limits {
                    work: 20_000_000,
                    ..Limits::default()
                },
            );
            let refused = input
                .iter()
                .any(|page| matches!(hostile.feed(page), Err(AppError::Invalid(COMPARISON_LIMIT))));
            // A smaller hostile page stays within the caps and groups in bounded time.
            for page in &mut input {
                page.lines.truncate(500);
            }
            let result = propose(&input);
            tx.send((
                refused,
                result.iter().map(|p| p.boxes.len()).collect::<Vec<_>>(),
            ))
            .unwrap();
        });
        assert_eq!(
            rx.recv_timeout(std::time::Duration::from_secs(60)).unwrap(),
            (true, vec![64; 3])
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
                tombstone: false,
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

    type View = Result<Vec<(String, String, Vec<(u32, BoxPoints)>, Vec<(u32, BoxPoints)>)>, String>;
    fn view(result: Result<Vec<Proposal>, AppError>) -> View {
        result
            .map(|list| {
                list.into_iter()
                    .map(|p| {
                        (
                            p.text,
                            p.fingerprint,
                            p.boxes.into_iter().collect(),
                            p.absent.into_iter().collect(),
                        )
                    })
                    .collect()
            })
            .map_err(|e| e.to_string())
    }
    /// The streaming proposer and the previous whole-corpus function give the same result.
    fn assert_identical(pages: &[TextPage]) -> View {
        let new = view(propose_keyed(pages, &[7; 32]));
        let old = view(propose_oracle(pages, &[7; 32]));
        assert_eq!(new, old);
        new
    }
    fn line(text: &str, bounds: BoxPoints) -> worker::records::TextLine {
        worker::records::TextLine {
            text: text.into(),
            bounds,
        }
    }
    /// Base-26 letters: distinct texts that the digit folding of `normalize` keeps distinct.
    fn letters(mut n: usize) -> String {
        let mut text = String::new();
        loop {
            text.push(char::from(b'a' + u8::try_from(n % 26).unwrap()));
            n /= 26;
            if n == 0 {
                return text;
            }
        }
    }

    #[test]
    fn the_streaming_proposer_matches_the_oracle_on_ordinary_corpora() {
        // Header and footer with drifting positions, a page number, body lines and absences.
        let mut input = pages(12, "running header");
        for page in &mut input {
            let drift = f64::from(page.page % 3) * 0.2;
            page.lines[0].bounds = [1. + drift, 2., 41. + drift, 12.];
            page.lines.push(line("page footer", [30., 180., 70., 190.]));
            page.lines
                .push(line(&format!("page {}", page.page), [48., 190., 52., 198.]));
            for i in 0..6 {
                page.lines.push(line(
                    &format!("body {} {}", letters(i), letters(page.page as usize)),
                    [
                        5.,
                        30. + f64::from(u32::try_from(i).unwrap()) * 12.,
                        95.,
                        40. + f64::from(u32::try_from(i).unwrap()) * 12.,
                    ],
                ));
            }
        }
        input[4].lines.clear();
        input[9].lines.truncate(1);
        let result = assert_identical(&input).unwrap();
        assert!(result.len() >= 2);
    }
    #[test]
    fn the_streaming_proposer_matches_the_oracle_on_duplicate_lines() {
        let mut input = pages(6, "same buyer mark");
        for page in &mut input {
            page.lines = vec![line("same buyer mark", [1., 2., 41., 12.]); 25];
            page.lines
                .push(line("same buyer mark", [50., 60., 90., 70.]));
            page.lines.push(line("same buyer mark", [1., 2., 41., 12.]));
        }
        let result = assert_identical(&input).unwrap();
        assert_eq!(result.len(), 3);
    }
    #[test]
    fn the_streaming_proposer_matches_the_oracle_when_a_text_budget_is_exhausted() {
        // One text exhausts its 2,000,000 comparisons and is dropped; the footer survives.
        let mut input = pages(3, "real footer");
        for page in &mut input {
            page.lines.extend((0..2001).map(|i| {
                let left = f64::from(i) / 100.;
                let top = f64::from(page.page) * 20.;
                line(&format!("{i} gp"), [left, top, left + 0.005, top + 1.])
            }));
        }
        let result = assert_identical(&input).unwrap();
        assert_eq!(result.len(), 1);
        // Nothing survives: the old and the new function both refuse.
        for page in &mut input {
            page.lines.remove(0);
        }
        assert_eq!(assert_identical(&input), Err(COMPARISON_LIMIT.to_owned()),);
    }
    #[test]
    fn a_group_that_cannot_reach_the_threshold_stays_as_a_tombstone_and_still_consumes_matches() {
        // Ten pages need six. The header recurs at A on pages 1-6; a second seed at B on pages
        // 2-3 can no longer reach six after page 6 and is freed; B-lines on pages 8-9 are
        // consumed by it, so no new group is seeded from them.
        let mut input = pages(10, "x");
        for page in &mut input {
            page.lines.clear();
            if page.page <= 6 {
                page.lines.push(line("running header", [1., 2., 41., 12.]));
            }
            if (2..=3).contains(&page.page) || (8..=9).contains(&page.page) {
                page.lines
                    .push(line("running header", [1., 100., 41., 110.]));
            }
        }
        let new = view(propose_keyed(&input, &[1; 32]));
        assert_identical(&input).unwrap();
        assert_eq!(new.unwrap()[0].2.len(), 6);
        let mut proposer = Proposer::new(10, Limits::default());
        for page in &input {
            proposer.feed(page).unwrap();
        }
        let tombs: Vec<_> = proposer.groups.values().filter(|g| g.tombstone).collect();
        assert_eq!(tombs.len(), 1);
        assert!(tombs[0].boxes.is_empty(), "boxes are freed");
        assert!(tombs[0].seed[1] > 0.4, "seed is kept");
    }
    #[test]
    fn hostile_many_distinct_lines_refuse_at_the_group_cap() {
        let mut proposer = Proposer::new(5, Limits::default());
        let mut refused = None;
        for page in 1..=5 {
            let lines = (0..20_000)
                .map(|i| {
                    let text = format!("line {} {}", letters(i), letters(page as usize));
                    line(&text, [1., 2., 41., 12.])
                })
                .collect();
            let result = proposer.feed(&TextPage {
                page,
                width: 100.,
                height: 200.,
                lines,
            });
            if let Err(error) = result {
                refused = Some((page, error.to_string()));
                break;
            }
        }
        assert_eq!(refused, Some((3, COMPARISON_LIMIT.to_owned())));
        assert!(proposer.groups.len() <= Limits::default().groups);
    }
    #[test]
    fn hostile_recurring_lines_refuse_at_the_memory_budget_with_flat_memory() {
        let limits = Limits {
            memory: 2 * 1024 * 1024,
            ..Limits::default()
        };
        // 400 lines that recur on every page: a group per line, one box entry per page.
        let mut proposer = Proposer::new(3000, limits);
        let mut refused_at = None;
        for page in 1..=3000 {
            let lines = (0..400_u32)
                .map(|i| {
                    let top = f64::from(i);
                    line(
                        &format!("line {}", letters(i as usize)),
                        [1., top, 41., top + 0.5],
                    )
                })
                .collect();
            if proposer
                .feed(&TextPage {
                    page,
                    width: 100.,
                    height: 500.,
                    lines,
                })
                .is_err()
            {
                refused_at = Some(page);
                break;
            }
            assert!(proposer.memory <= limits.memory);
        }
        let page = refused_at.expect("the budget refuses before the last page");
        assert!(page < 200, "refused at page {page}");
        // The live entries are what the budget allows, not what the pages would have added.
        assert!(proposer.entries() <= limits.memory / BOX_COST);
    }
    #[test]
    fn the_global_comparison_budget_refuses_and_a_small_corpus_stays_under_it() {
        let limits = Limits {
            comparisons: 1000,
            ..Limits::default()
        };
        let mut proposer = Proposer::new(3, limits);
        let hostile = |page| TextPage {
            page,
            width: 100.,
            height: 200.,
            lines: (0..200)
                .map(|i| {
                    line(
                        "same text",
                        // Each page's lines sit between the previous page's: none matches.
                        {
                            let left = f64::from(i) / 10. + f64::from(page) * 0.03;
                            [left, 0., left + 0.005, 1.]
                        },
                    )
                })
                .collect(),
        };
        let mut result = Ok(());
        for page in 1..=3 {
            result = result.and_then(|()| proposer.feed(&hostile(page)));
        }
        assert!(matches!(result, Err(AppError::Invalid(COMPARISON_LIMIT))));
        assert_eq!(Limits::default().comparisons, 50_000_000);
        assert_eq!(Limits::default().groups, 50_000);
        assert_eq!(Limits::default().memory, 64 * 1024 * 1024);
        assert_eq!(Limits::default().per_text, 2_000_000);
        let mut easy = Proposer::new(3, limits);
        for page in pages(3, "test") {
            easy.feed(&page).unwrap();
        }
        assert_eq!(easy.finish(&[0; 32]).unwrap().len(), 1);
    }
    #[test]
    fn live_entries_stay_flat_across_five_hundred_text_pages() {
        let mut proposer = Proposer::new(500, Limits::default());
        let mut peak = 0;
        for page in 1..=500_u32 {
            let mut lines = vec![line("page footer", [30., 180., 70., 190.])];
            lines.extend((0..30).map(|i| {
                line(
                    &format!("body {} {}", letters(i), letters(page as usize)),
                    [
                        5.,
                        f64::from(u32::try_from(i).unwrap()) * 5.,
                        95.,
                        f64::from(u32::try_from(i).unwrap()) * 5. + 4.,
                    ],
                )
            }));
            proposer
                .feed(&TextPage {
                    page,
                    width: 100.,
                    height: 200.,
                    lines,
                })
                .unwrap();
            peak = peak.max(proposer.entries());
        }
        // The 15,000 distinct body lines are freed in the tail; holding every page would keep
        // at least 30,000 entries.
        assert!(peak < 15_000, "peak {peak}");
        assert!(proposer.entries() <= 1 + 500, "{}", proposer.entries());
        assert!(proposer.memory < 1024 * 1024, "{}", proposer.memory);
        let result = proposer.finish(&[0; 32]).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].boxes.len(), 500);
    }

    #[test]
    fn masking_covers_the_union_and_treats_edge_contact_as_clear() {
        let unit = [10., 10., 50., 20.];
        // One box, exactly the unit: contained (edges inclusive).
        assert_eq!(masking(unit, &[unit]), Masking::Contained);
        assert_eq!(masking(unit, &[[0., 0., 100., 100.]]), Masking::Contained);
        // A header drawn as adjacent left and right boxes covers the unit together.
        assert_eq!(
            masking(unit, &[[0., 0., 30., 30.], [30., 0., 60., 30.]]),
            Masking::Contained
        );
        // A gap between the boxes leaves part of the unit: crossing, not contained.
        assert_eq!(
            masking(unit, &[[0., 0., 29., 30.], [31., 0., 60., 30.]]),
            Masking::Crossing
        );
        // A box over half of the unit crosses.
        assert_eq!(masking(unit, &[[0., 0., 30., 30.]]), Masking::Crossing);
        // Edge contact (a shared edge, a shared corner) is not crossing.
        assert_eq!(masking(unit, &[[50., 10., 60., 20.]]), Masking::Clear);
        assert_eq!(masking(unit, &[[0., 20., 60., 30.]]), Masking::Clear);
        assert_eq!(masking(unit, &[[50., 20., 60., 30.]]), Masking::Clear);
        assert_eq!(masking(unit, &[]), Masking::Clear);
        // Edge contact beside a real overlap does not make a partial cover whole.
        assert_eq!(
            masking(unit, &[[0., 0., 30., 30.], [50., 10., 60., 20.]]),
            Masking::Crossing
        );
        // An L-shaped union that misses a corner cell.
        assert_eq!(
            masking(unit, &[[0., 0., 60., 15.], [0., 0., 30., 30.]]),
            Masking::Crossing
        );
    }
    #[test]
    fn rounding_is_outward_idempotent_short_and_never_negative_zero() {
        assert_eq!(grid_down(0.299), 0.29);
        assert_eq!(grid_up(0.291), 0.3);
        assert_eq!(grid_up(595.2756), 595.28);
        assert_eq!(grid_down(595.2756), 595.27);
        for v in [
            0., 0.01, 0.07, 0.29, 1.1, 1.15, 12.34, 595.28, 3999.99, 4000.,
        ] {
            assert_eq!(grid_up(v), v, "{v}");
            assert_eq!(grid_down(v), v, "{v}");
        }
        assert!(grid_down(-0.).is_sign_positive());
        for v in [0.001, 3999.991, 1234.5678, 595.2756, 0.999, 12.0001] {
            for r in [grid_up(v), grid_down(v)] {
                assert!(serde_json::to_string(&r).unwrap().len() <= 7, "{v} -> {r}");
            }
            assert!(grid_up(v) >= v && grid_down(v) <= v);
        }
    }
    #[test]
    fn rounding_is_strictly_outward_one_ulp_either_side_of_every_grid_point() {
        // The step `k / 100` can land one ulp on the wrong side of `v`: 3745.7599999999998 is
        // just below the grid point 3745.76 and must round up to it, never down.
        let just_below = 3_745.759_999_999_999_8_f64;
        assert!(grid_up(just_below) >= just_below);
        let mut checked = 0_u32;
        for k in 0..=400_000_u32 {
            let grid = f64::from(k) / 100.;
            let mut below = grid;
            let mut above = grid;
            for _ in 0..3 {
                below = below.next_down();
                above = above.next_up();
                for v in [below, above] {
                    if (0. ..=4000.).contains(&v) {
                        assert!(grid_up(v) >= v, "up {v:?} -> {:?}", grid_up(v));
                        assert!(grid_down(v) <= v, "down {v:?} -> {:?}", grid_down(v));
                        checked += 1;
                    }
                }
            }
        }
        assert!(checked > 2_000_000);
    }
    #[test]
    fn a_windowed_edit_keeps_the_stored_fingerprints_up_to_eight() {
        let f = |n: u32| format!("{n:064x}");
        let current: Vec<String> = (1..=3).map(f).collect();
        let merged = merge_fingerprints(&current, &[f(3), f(9)]).unwrap();
        assert_eq!(merged, [f(1), f(2), f(3), f(9)]);
        let eight: Vec<String> = (1..=8).map(f).collect();
        assert_eq!(merge_fingerprints(&eight, &[f(8)]).unwrap(), eight);
        assert!(merge_fingerprints(&eight, &[f(9)]).is_err());
        assert_eq!(merge_fingerprints(&[], &[]).unwrap(), Vec::<String>::new());
    }
    #[test]
    fn expanded_normalized_text_is_charged_and_over_long_lines_are_dropped_before_sorting() {
        // U+FDFA is one scalar of three bytes that NFKC expands to 18 letters (33 bytes): the
        // raw page costs little work, the normalized text a good deal more.
        let fdfa = "\u{FDFA}";
        let page = |lines| TextPage {
            page: 1,
            width: 100.,
            height: 200.,
            lines,
        };
        let many = |n| -> Vec<_> {
            (0..n)
                .map(|i| {
                    line(
                        &format!("{}{}", fdfa.repeat(3), letters(i)),
                        [1., 2., 41., 12.],
                    )
                })
                .collect()
        };
        let limits = Limits {
            work: 12_000,
            ..Limits::default()
        };
        // The raw charge of 1,000 such lines is under the budget on its own.
        let mut raw_only = Proposer::new(3, limits);
        raw_only.spend_lines(&page(many(1000))).unwrap();
        assert!(raw_only.work < limits.work);
        let mut proposer = Proposer::new(3, limits);
        assert!(matches!(
            proposer.feed(&page(many(1000))),
            Err(AppError::Invalid(COMPARISON_LIMIT))
        ));
        // Lines that normalize past 200 characters never reach the sort or a group.
        let mut long = Proposer::new(3, Limits::default());
        long.feed(&page(vec![line(&fdfa.repeat(12), [1., 2., 41., 12.]); 50]))
            .unwrap();
        assert!(long.groups.is_empty());
    }
    #[test]
    fn tombstones_and_recurring_text_cannot_run_past_the_work_budget() {
        // Text X recurs at one place on every page (one comparison a page) while 3,000 other
        // seeds of X on page 1 turn into tombstones that every later prune and scan walks:
        // work the comparison budget never sees.
        let limits = Limits {
            work: 60_000,
            ..Limits::default()
        };
        let total = 40_u32;
        let mut proposer = Proposer::new(total as usize, limits);
        let mut refused_at = None;
        for page in 1..=total {
            let mut lines = vec![line("recurring header", [0., 0., 40., 10.])];
            if page == 1 {
                lines.extend((0..3000_u32).map(|i| {
                    let top = 20. + f64::from(i) * 0.05;
                    line("recurring header", [60., top, 61., top + 0.01])
                }));
            }
            let fed = proposer.feed(&TextPage {
                page,
                width: 100.,
                height: 200.,
                lines,
            });
            if let Err(error) = fed {
                assert_eq!(error.to_string(), COMPARISON_LIMIT);
                refused_at = Some(page);
                break;
            }
        }
        assert!(refused_at.is_some(), "the run was never refused");
        // The refusal comes at once, not after the work was done: bounded by the budget plus
        // the step that crossed it.
        assert!(proposer.work <= limits.work + 5_000, "{}", proposer.work);
        // The same input under the default budget is fine (a small corpus, N1 unchanged).
        let mut easy = Proposer::new(total as usize, Limits::default());
        for page in 1..=total {
            easy.feed(&TextPage {
                page,
                width: 100.,
                height: 200.,
                lines: vec![line("recurring header", [0., 0., 40., 10.])],
            })
            .unwrap();
        }
        assert_eq!(easy.finish(&[0; 32]).unwrap().len(), 1);
    }
}
