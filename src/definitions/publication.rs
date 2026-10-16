//! Immutable wiring publications; source review, game legality and identity authentication are separate.
use super::{Definition, DefinitionError, Mapping, ValidationReport, admission, admit};
use crate::{AppError, Store, registry::RulesetKey};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

const SNAPSHOT_LIMIT: usize = 1024 * 1024;
#[cfg(test)]
thread_local! { static SNAPSHOTS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
#[cfg(test)]
thread_local! {
    /// Route tests of unshipped sheet capabilities publish on their own test thread only.
    static UNSHIPPED_FOR_TESTS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Lets the current test thread publish definitions the publication gate still refuses.
#[cfg(test)]
pub(crate) fn publish_unshipped_for_tests() {
    UNSHIPPED_FOR_TESTS.with(|allowed| allowed.set(true));
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceSnapshot {
    id: i64,
    source_id: String,
    title: String,
    edition: String,
    language: String,
    pdf_hash: String,
    prepared_hash: String,
    extraction_hash: String,
    total_pages: i64,
    roles: Vec<super::SourceRole>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PageSnapshot {
    id: i64,
    source_id: i64,
    pdf_page: i64,
    printed_label: String,
    revision: i64,
    complete: bool,
    image_hash: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct UnitSnapshot {
    id: i64,
    page_id: i64,
    block_id: String,
    kind: String,
    polygon: String,
    revision: i64,
    state: String,
    raw_html_hash: String,
    raw_text_hash: String,
    effective_hash: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Snapshot {
    encoding: u32,
    ruleset: RulesetKey,
    definition_id: String,
    content_hash: String,
    sources: Vec<SourceSnapshot>,
    pages: Vec<PageSnapshot>,
    units: Vec<UnitSnapshot>,
    unresolved: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Comparison {
    pub target: &'static str,
    pub target_id: i64,
    pub draft_revision: i64,
    pub latest_version_id: i64,
    pub current_approval_id: i64,
    pub withdrawn: bool,
    pub snapshot_hash: String,
    pub comparison_hash: String,
    snapshot: Snapshot,
    pub original_page_urls: Vec<String>,
    pub original_region_urls: Vec<String>,
}

pub struct ApprovalRequest {
    pub comparison_hash: String,
    pub actor: String,
    pub rationale: String,
    pub originals_compared: bool,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WiringStatus {
    Current,
    Stale,
    Withdrawn,
    NeverApproved,
}

#[derive(Debug, Serialize)]
pub struct PublishedDefinition {
    pub id: i64,
    pub version: i64,
    pub content_hash: String,
    pub approval_id: i64,
    pub wiring_status: WiringStatus,
    pub unresolved_fields: usize,
    /// Derived fields / constraints whose rule evidence is unresolved; omitted when zero so
    /// formula-free output is unchanged.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unresolved_formulas: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unresolved_constraints: Vec<String>,
    pub definition: Definition,
    pub advisory_tasks: &'static str,
    pub deterministic_rules: &'static str,
}

/// Deterministic-rule capability: formulas are advisory results, never enforced writes.
fn deterministic_rules(definition: &Definition) -> &'static str {
    if definition.formula_profile.is_some() {
        "formulas-advisory"
    } else {
        "unsupported"
    }
}

/// M2 admits formula and list definitions as drafts only. Publishing waits for evaluation on
/// view/save (M3) and list editing (M4), so no sheet can pin a definition it cannot show.
fn publishable(definition: &Definition) -> Result<(), DefinitionError> {
    #[cfg(test)]
    if UNSHIPPED_FOR_TESTS.with(std::cell::Cell::get) {
        return Ok(());
    }
    if definition
        .fields
        .iter()
        .any(|field| field.control == super::Control::List)
    {
        return Err(invalid("list-editing-unsupported"));
    }
    if definition.formula_profile.is_some() {
        return Err(invalid("formula-evaluation-unsupported"));
    }
    Ok(())
}

fn hash(prefix: &[u8], bytes: &[u8]) -> String {
    let mut hash = Sha256::new();
    hash.update(prefix);
    hash.update(bytes);
    hex::encode(hash.finalize())
}
fn invalid(code: &str) -> DefinitionError {
    ValidationReport::one("/publication", code)
}
fn encode<T: Serialize>(data: &T) -> Result<String, DefinitionError> {
    serde_json::to_string(data).map_err(|_| DefinitionError::Configuration)
}
fn budget<T: Serialize>(data: &T, used: &mut usize) -> Result<(), DefinitionError> {
    *used = used
        .checked_add(encode(data)?.len() + 1)
        .ok_or_else(|| invalid("snapshot-byte-limit"))?;
    if *used > SNAPSHOT_LIMIT {
        return Err(invalid("snapshot-byte-limit"));
    }
    Ok(())
}

fn snapshot(db: &Connection, data: &admission::Admitted) -> Result<Snapshot, DefinitionError> {
    #[cfg(test)]
    SNAPSHOTS.with(|count| count.set(count.get() + 1));
    super::store::references(db, data)?;
    let definition = &data.definition;
    let mut snapshot = Snapshot {
        encoding: 1,
        ruleset: RulesetKey {
            game_id: definition.game_id.clone(),
            edition_id: definition.edition_id.clone(),
        },
        definition_id: definition.definition_id.clone(),
        content_hash: data.hash.clone(),
        sources: Vec::new(),
        pages: Vec::new(),
        units: Vec::new(),
        unresolved: Vec::new(),
    };
    let mut pages = BTreeSet::new();
    let mut units = BTreeSet::new();
    for field in &definition.fields {
        match &field.mapping {
            Mapping::Mapped { regions } => {
                for region in regions {
                    pages.insert(region.page_id);
                    units.insert(region.unit_id);
                }
            }
            Mapping::Unresolved { .. } => snapshot.unresolved.push(field.id.clone()),
        }
    }
    if units.is_empty() {
        return Err(invalid("at-least-one-mapped-field-required"));
    }
    // Formula/constraint evidence is captured with the wiring (page-scoped invalidation); its
    // unresolved IDs are derived from the immutable content, so the encoding-1 envelope and its
    // field-only `unresolved` list are unchanged.
    for region in super::formula_admission::evidence_regions(definition) {
        pages.insert(region.page_id);
        units.insert(region.unit_id);
    }
    snapshot.unresolved.sort();
    let mut used = encode(&snapshot)?.len();
    for source in &definition.sources {
        let row = db.query_row("SELECT s.source_id,s.title,s.edition,s.language,s.pdf_hash,f.prepared_hash,f.extraction_hash,s.total_pages FROM sources s JOIN source_finalizations f ON f.source_id=s.id WHERE s.id=?1 AND length(CAST(s.source_id AS BLOB))<=100 AND length(CAST(s.title AS BLOB))<=200 AND length(CAST(s.edition AS BLOB))<=400 AND length(CAST(s.language AS BLOB))<=30", [source.source_id], |row| Ok(SourceSnapshot { id: source.source_id, source_id: row.get(0)?, title: row.get(1)?, edition: row.get(2)?, language: row.get(3)?, pdf_hash: row.get(4)?, prepared_hash: row.get(5)?, extraction_hash: row.get(6)?, total_pages: row.get(7)?, roles: source.roles.clone() })).optional().map_err(AppError::from)?.ok_or_else(|| invalid("source-unfinalized-missing-or-oversized"))?;
        budget(&row, &mut used)?;
        snapshot.sources.push(row);
    }
    for id in pages {
        let row = db.query_row("SELECT p.source_id,p.pdf_page,p.printed_label,p.revision,p.complete,h.image_hash FROM source_pages p JOIN source_page_hashes h ON h.page_id=p.id AND h.source_id=p.source_id WHERE p.id=?1 AND length(CAST(p.printed_label AS BLOB))<=200 AND typeof(p.revision)='integer' AND p.revision>0", [id], |row| Ok(PageSnapshot { id, source_id: row.get(0)?, pdf_page: row.get(1)?, printed_label: row.get(2)?, revision: row.get(3)?, complete: row.get(4)?, image_hash: row.get(5)? })).optional().map_err(AppError::from)?.ok_or_else(|| invalid("page-missing-or-invalid-metadata"))?;
        budget(&row, &mut used)?;
        snapshot.pages.push(row);
    }
    for id in units {
        let row = unit_snapshot(db, id)?;
        budget(&row, &mut used)?;
        snapshot.units.push(row);
    }
    if encode(&snapshot)?.len() > SNAPSHOT_LIMIT {
        return Err(invalid("snapshot-byte-limit"));
    }
    Ok(snapshot)
}

fn unit_snapshot(db: &Connection, id: i64) -> Result<UnitSnapshot, DefinitionError> {
    struct Raw {
        page: i64,
        block: String,
        kind: String,
        polygon: String,
        revision: i64,
        state: String,
        html: String,
        text: String,
        correction: String,
    }
    let raw = db.query_row("SELECT page_id,block_id,kind,polygon,revision,state,raw_html,raw_text,correction FROM source_units WHERE id=?1 AND typeof(revision)='integer' AND revision>0 AND length(CAST(block_id AS BLOB))<=512 AND length(CAST(kind AS BLOB))<=100 AND length(CAST(polygon AS BLOB))<=1024 AND length(CAST(raw_html AS BLOB))<=65536 AND length(CAST(raw_text AS BLOB))<=262144 AND length(CAST(correction AS BLOB))<=80000 AND state IN ('unreviewed','accepted','corrected','rejected')", [id], |row| Ok(Raw { page: row.get(0)?, block: row.get(1)?, kind: row.get(2)?, polygon: row.get(3)?, revision: row.get(4)?, state: row.get(5)?, html: row.get(6)?, text: row.get(7)?, correction: row.get(8)? })).optional().map_err(AppError::from)?.ok_or_else(|| invalid("unit-missing-or-invalid-metadata"))?;
    let effective = if raw.state == "corrected" {
        &raw.correction
    } else {
        &raw.text
    };
    Ok(UnitSnapshot {
        id,
        page_id: raw.page,
        block_id: raw.block,
        kind: raw.kind,
        polygon: raw.polygon,
        revision: raw.revision,
        state: raw.state,
        raw_html_hash: hash(b"", raw.html.as_bytes()),
        raw_text_hash: hash(b"", raw.text.as_bytes()),
        effective_hash: hash(b"", effective.as_bytes()),
    })
}

fn latest_version(db: &Connection, data: &admission::Admitted) -> Result<i64, DefinitionError> {
    Ok(db.query_row("SELECT id FROM definition_versions WHERE game_id=?1 AND edition_id=?2 AND definition_id=?3 ORDER BY version DESC LIMIT 1", params![data.definition.game_id,data.definition.edition_id,data.definition.definition_id], |row| row.get(0)).optional().map_err(AppError::from)?.unwrap_or(0))
}
fn approval_state(db: &Connection, version: i64) -> Result<(i64, bool), DefinitionError> {
    Ok(db.query_row("SELECT a.id,EXISTS(SELECT 1 FROM definition_withdrawals w WHERE w.approval_id=a.id) FROM definition_approvals a WHERE version_id=?1 ORDER BY a.id DESC LIMIT 1", [version], |row| Ok((row.get(0)?,row.get(1)?))).optional().map_err(AppError::from)?.unwrap_or((0,false)))
}

fn comparison(
    db: &Connection,
    target: &'static str,
    id: i64,
    revision: i64,
    data: &admission::Admitted,
) -> Result<Comparison, DefinitionError> {
    let snapshot = snapshot(db, data)?;
    let latest_version_id = latest_version(db, data)?;
    let (current_approval_id, withdrawn) = approval_state(
        db,
        if target == "version" {
            id
        } else {
            latest_version_id
        },
    )?;
    let snapshot_hash = hash(b"mimik-wiring-snapshot-v1\0", encode(&snapshot)?.as_bytes());
    let original_page_urls = snapshot
        .pages
        .iter()
        .map(|page| format!("/sources/pages/{}/image", page.id))
        .collect();
    let original_region_urls = snapshot
        .units
        .iter()
        .map(|unit| format!("/sources/pages/{}/units/{}/image", unit.page_id, unit.id))
        .collect();
    let mut result = Comparison {
        target,
        target_id: id,
        draft_revision: revision,
        latest_version_id,
        current_approval_id,
        withdrawn,
        snapshot_hash,
        comparison_hash: String::new(),
        snapshot,
        original_page_urls,
        original_region_urls,
    };
    result.comparison_hash = hash(b"mimik-wiring-comparison-v1\0", encode(&result)?.as_bytes());
    Ok(result)
}

struct Candidate {
    id: i64,
    revision: i64,
    data: admission::Admitted,
}

async fn candidate(
    store: &Store,
    target: &'static str,
    id: i64,
) -> Result<(Candidate, tokio::sync::OwnedSemaphorePermit), DefinitionError> {
    if id <= 0 {
        return Err(invalid("positive-target-id-required"));
    }
    let permit = admit()?;
    let (content, expected_hash, revision, permit) = store
        .run(move |db| {
            let sql = if target == "draft" {
                "SELECT CASE WHEN length(CAST(content AS BLOB))<=262144 THEN content END,content_hash,revision FROM definition_drafts WHERE id=?1"
            } else {
                "SELECT CASE WHEN length(CAST(content AS BLOB))<=262144 THEN content END,content_hash,version FROM definition_versions WHERE id=?1"
            };
            let row: (Option<String>, String, i64) = db
                .query_row(sql, [id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                .optional()?
                .ok_or(AppError::NotFound)?;
            Ok((row.0, row.1, row.2, permit))
        })
        .await
        .map_err(|error| {
            if matches!(error, AppError::NotFound) {
                DefinitionError::NotFound
            } else {
                error.into()
            }
        })?;
    tokio::task::spawn_blocking(move || {
        let content = content.ok_or_else(|| invalid("stored-publication-content-byte-limit"))?;
        let data = admission::validate(content.as_bytes())?;
        if data.content != content || data.hash != expected_hash {
            return Err(invalid("stored-content-or-hash-invalid"));
        }
        Ok((Candidate { id, revision, data }, permit))
    })
    .await
    .map_err(|_| DefinitionError::Worker)?
}

fn target_unchanged(
    db: &Connection,
    target: &str,
    candidate: &Candidate,
) -> Result<(), DefinitionError> {
    let sql = if target == "draft" {
        "SELECT revision,content_hash FROM definition_drafts WHERE id=?1"
    } else {
        "SELECT version,content_hash FROM definition_versions WHERE id=?1"
    };
    let current: (i64, String) = db
        .query_row(sql, [candidate.id], |row| Ok((row.get(0)?, row.get(1)?)))
        .optional()
        .map_err(AppError::from)?
        .ok_or(DefinitionError::NotFound)?;
    if current != (candidate.revision, candidate.data.hash.clone()) {
        return Err(DefinitionError::Conflict);
    }
    Ok(())
}

fn verify_approval(
    db: &Connection,
    id: i64,
    data: &admission::Admitted,
) -> Result<String, DefinitionError> {
    // Drafts may advance after approval. Retain their identity and a monotonic
    // revision bound; only the still-current approved revision can be compared
    // byte-for-byte. Version targets have immutable numbers and exact identity.
    // Any future draft deletion feature must preserve referenced targets; direct
    // SQL deletion deliberately makes their retained approval an integrity error.
    let provenance: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM definition_approvals a JOIN definition_versions v ON v.id=a.version_id WHERE a.id=?1 AND v.game_id=?2 AND v.edition_id=?3 AND v.definition_id=?4 AND v.content_hash=?5 AND v.content=?6 AND ((a.target_kind='version' AND a.target_id=v.id AND a.target_revision=v.version) OR (a.target_kind='draft' AND EXISTS(SELECT 1 FROM definition_drafts d WHERE d.id=a.target_id AND d.game_id=v.game_id AND d.edition_id=v.edition_id AND d.definition_id=v.definition_id AND d.revision>=a.target_revision AND a.target_revision>0 AND (d.revision>a.target_revision OR (d.content_hash=v.content_hash AND d.content=v.content))))))",
        params![id, data.definition.game_id, data.definition.edition_id, data.definition.definition_id, data.hash, data.content],
        |row| row.get(0),
    ).map_err(AppError::from)?;
    if !provenance {
        return Err(invalid("stored-approval-target-integrity"));
    }
    let (content,stored_hash,encoding):(Option<String>,String,i64)=db.query_row("SELECT CASE WHEN length(CAST(snapshot AS BLOB))<=1048576 THEN snapshot END,snapshot_hash,encoding FROM definition_approvals WHERE id=?1",[id],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).map_err(AppError::from)?;
    let content = content.ok_or_else(|| invalid("stored-approval-snapshot-byte-limit"))?;
    let snapshot: Snapshot = serde_json::from_str(&content)
        .map_err(|_| invalid("stored-approval-snapshot-integrity"))?;
    if encoding != 1
        || snapshot.encoding != 1
        || snapshot.sources.len() > 32
        || snapshot.sources.is_empty()
        || snapshot.pages.len() > 1024
        || snapshot.pages.is_empty()
        || snapshot.units.len() > 1024
        || snapshot.units.is_empty()
        || snapshot.unresolved.len() > 256
        || snapshot.ruleset.game_id != data.definition.game_id
        || snapshot.ruleset.edition_id != data.definition.edition_id
        || snapshot.definition_id != data.definition.definition_id
        || snapshot.content_hash != data.hash
        || encode(&snapshot)? != content
        || hash(b"mimik-wiring-snapshot-v1\0", content.as_bytes()) != stored_hash
    {
        return Err(invalid("stored-approval-snapshot-integrity"));
    }
    Ok(stored_hash)
}

fn published(
    db: &Connection,
    id: i64,
    data: admission::Admitted,
) -> Result<PublishedDefinition, DefinitionError> {
    let version: i64 = db
        .query_row(
            "SELECT version FROM definition_versions WHERE id=?1",
            [id],
            |row| row.get(0),
        )
        .map_err(AppError::from)?;
    let (approval_id, withdrawn) = approval_state(db, id)?;
    let stored_hash = if approval_id > 0 {
        Some(verify_approval(db, approval_id, &data)?)
    } else {
        None
    };
    let (unresolved_formulas, unresolved_constraints) =
        super::formula_admission::unresolved_evidence(&data.definition);
    let wiring_status = if approval_id == 0 {
        WiringStatus::NeverApproved
    } else if withdrawn {
        WiringStatus::Withdrawn
    } else {
        let current = hash(
            b"mimik-wiring-snapshot-v1\0",
            encode(&snapshot(db, &data)?)?.as_bytes(),
        );
        if stored_hash.as_ref() == Some(&current) {
            WiringStatus::Current
        } else {
            WiringStatus::Stale
        }
    };
    Ok(PublishedDefinition {
        id,
        version,
        content_hash: data.hash,
        approval_id,
        wiring_status,
        unresolved_fields: data
            .definition
            .fields
            .iter()
            .filter(|field| matches!(field.mapping, Mapping::Unresolved { .. }))
            .count(),
        unresolved_formulas,
        unresolved_constraints,
        deterministic_rules: deterministic_rules(&data.definition),
        definition: data.definition,
        advisory_tasks: "unsupported",
    })
}

pub(super) fn private_readiness(
    db: &Connection,
    id: i64,
    data: &admission::Admitted,
) -> Result<WiringStatus, DefinitionError> {
    let (approval, withdrawn) = approval_state(db, id)?;
    if approval == 0 {
        return Ok(WiringStatus::NeverApproved);
    }
    let stored = verify_approval(db, approval, data)?;
    let available = private_evidence_available(db, data)?;
    if withdrawn {
        return Ok(WiringStatus::Withdrawn);
    }
    if !available {
        return Ok(WiringStatus::Stale);
    }
    let current = hash(
        b"mimik-wiring-snapshot-v1\0",
        encode(&snapshot(db, data)?)?.as_bytes(),
    );
    Ok(if current == stored {
        WiringStatus::Current
    } else {
        WiringStatus::Stale
    })
}

fn private_evidence_available(
    db: &Connection,
    data: &admission::Admitted,
) -> Result<bool, DefinitionError> {
    let mut available = true;
    // Validate every present dependency, including when another dependency is absent.
    for source in &data.definition.sources {
        let present: bool = db
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sources WHERE id=?1)",
                [source.source_id],
                |r| r.get(0),
            )
            .map_err(AppError::from)?;
        if present {
            let valid:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM sources s WHERE s.id=?1 AND length(CAST(s.source_id AS BLOB))<=100 AND length(CAST(s.title AS BLOB))<=200 AND length(CAST(s.edition AS BLOB))<=400 AND length(CAST(s.language AS BLOB))<=30 AND s.pdf_hash=?2 AND typeof(s.total_pages)='integer' AND s.total_pages>0)",params![source.source_id,source.pdf_sha256],|r|r.get(0)).map_err(AppError::from)?;
            if !valid {
                return Err(invalid("present-source-metadata-corrupt"));
            }
        }
        let corrupt:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM source_finalizations f WHERE f.source_id=?1 AND (length(CAST(f.prepared_hash AS BLOB))!=64 OR f.prepared_hash GLOB '*[^0-9a-f]*' OR length(CAST(f.extraction_hash AS BLOB))!=64 OR f.extraction_hash GLOB '*[^0-9a-f]*' OR (EXISTS(SELECT 1 FROM sources WHERE id=f.source_id) AND f.prepared_hash IS NOT (SELECT bundle_hash FROM sources WHERE id=f.source_id))))",[source.source_id],|r|r.get(0)).map_err(AppError::from)?;
        if corrupt {
            return Err(invalid("present-finalization-metadata-corrupt"));
        }
        let exists: bool = db
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sources s JOIN source_finalizations f ON f.source_id=s.id JOIN source_rulesets r ON r.source_id=s.id WHERE s.id=?1 AND r.game_id=?2 AND r.edition_id=?3)",
                params![source.source_id,data.definition.game_id,data.definition.edition_id],
                |r| r.get(0),
            )
            .map_err(AppError::from)?;
        if !exists {
            available = false;
        }
    }
    let mut pages = BTreeSet::new();
    let mut units = BTreeSet::new();
    // Field wiring and formula/constraint evidence are all dependencies.
    for region in super::formula_admission::regions(&data.definition) {
        if pages.insert((region.page_id, region.source_id)) {
            let present: bool = db
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM source_pages WHERE id=?1)",
                    [region.page_id],
                    |r| r.get(0),
                )
                .map_err(AppError::from)?;
            if present {
                let valid:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM source_pages WHERE id=?1 AND source_id=?2 AND typeof(revision)='integer' AND revision>0 AND length(CAST(printed_label AS BLOB))<=200 AND complete IN (0,1) AND pdf_page>0)",params![region.page_id,region.source_id],|r|r.get(0)).map_err(AppError::from)?;
                if !valid {
                    return Err(invalid("present-page-metadata-corrupt"));
                }
            }
            let corrupt:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM source_page_hashes WHERE page_id=?1 AND (source_id!=?2 OR length(CAST(image_hash AS BLOB))!=64 OR image_hash GLOB '*[^0-9a-f]*'))",params![region.page_id,region.source_id],|r|r.get(0)).map_err(AppError::from)?;
            if corrupt {
                return Err(invalid("present-page-hash-metadata-corrupt"));
            }
        }
        if units.insert((region.unit_id, region.page_id)) {
            let present: bool = db
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM source_units WHERE id=?1)",
                    [region.unit_id],
                    |r| r.get(0),
                )
                .map_err(AppError::from)?;
            if present && unit_snapshot(db, region.unit_id)?.page_id != region.page_id {
                return Err(invalid("present-unit-parent-corrupt"));
            }
        }
        let exists:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM source_pages p JOIN source_units u ON u.page_id=p.id JOIN source_page_hashes h ON h.page_id=p.id AND h.source_id=p.source_id WHERE p.id=?1 AND u.id=?2)",params![region.page_id,region.unit_id],|r|r.get(0)).map_err(AppError::from)?;
        if !exists {
            available = false;
        }
    }
    Ok(available)
}

impl Store {
    /// Capture a coherent original-region comparison without approving anything.
    /// # Errors
    /// Rejects bad/unmapped targets, unfinalized evidence, saturation or storage failures.
    pub async fn compare_definition(
        &self,
        id: i64,
        published_version: bool,
    ) -> Result<Comparison, DefinitionError> {
        let target = if published_version {
            "version"
        } else {
            "draft"
        };
        let (candidate, permit) = candidate(self, target, id).await?;
        self.run(move |db| {
            let _permit = permit;
            Ok((|| {
                let tx = db.transaction().map_err(AppError::from)?;
                target_unchanged(&tx, target, &candidate)?;
                let result = comparison(
                    &tx,
                    target,
                    id,
                    if published_version {
                        0
                    } else {
                        candidate.revision
                    },
                    &candidate.data,
                )?;
                tx.commit().map_err(AppError::from)?;
                Ok(result)
            })())
        })
        .await?
    }

    /// Publish or explicitly reapprove using the exact comparison ticket and human assertion.
    /// # Errors
    /// Rejects stale comparisons, missing human assertions, counters or storage failures.
    pub async fn approve_definition(
        &self,
        id: i64,
        published_version: bool,
        request: ApprovalRequest,
    ) -> Result<PublishedDefinition, DefinitionError> {
        crate::sources::review_identity(&request.actor, &request.rationale)?;
        if !request.originals_compared {
            return Err(invalid("explicit-original-comparison-required"));
        }
        let target = if published_version {
            "version"
        } else {
            "draft"
        };
        let (candidate, permit) = candidate(self, target, id).await?;
        publishable(&candidate.data.definition)?;
        self.run(move|db| { let _permit=permit; Ok((|| {
            let tx=db.transaction_with_behavior(TransactionBehavior::Immediate).map_err(AppError::from)?;
            target_unchanged(&tx,target,&candidate)?;
            #[cfg(test)] let before=SNAPSHOTS.with(std::cell::Cell::get);
            let check=comparison(&tx,target,id,if published_version {0}else{candidate.revision},&candidate.data)?;
            if check.comparison_hash!=request.comparison_hash { return Err(DefinitionError::Conflict); }
            let version_id=if published_version {id} else { publish_version(&tx,&candidate.data,check.latest_version_id)? };
            tx.execute("INSERT INTO definition_approvals(version_id,encoding,snapshot,snapshot_hash,actor,rationale,originals_compared,target_kind,target_id,target_revision,comparison_encoding,comparison_hash) VALUES (?1,1,?2,?3,?4,?5,1,?6,?7,?8,1,?9)",params![version_id,encode(&check.snapshot)?,check.snapshot_hash,request.actor,request.rationale,target,id,candidate.revision,check.comparison_hash]).map_err(AppError::from)?;
            let approval_id=tx.last_insert_rowid();
            let version:i64=tx.query_row("SELECT version FROM definition_versions WHERE id=?1",[version_id],|row|row.get(0)).map_err(AppError::from)?;
            let (unresolved_formulas,unresolved_constraints)=super::formula_admission::unresolved_evidence(&candidate.data.definition);
            let result=PublishedDefinition{id:version_id,version,content_hash:candidate.data.hash,approval_id,wiring_status:WiringStatus::Current,unresolved_fields:check.snapshot.unresolved.len(),unresolved_formulas,unresolved_constraints,deterministic_rules:deterministic_rules(&candidate.data.definition),definition:candidate.data.definition,advisory_tasks:"unsupported"};
            #[cfg(test)] tests::assert_one(before);
            tx.commit().map_err(AppError::from)?; Ok(result)
        })()) }).await?
    }

    /// Show immutable version data with current evidence status from one read snapshot.
    /// # Errors
    /// Propagates saturation, invalid content/evidence and storage failures.
    pub async fn definition_version(
        &self,
        id: i64,
    ) -> Result<PublishedDefinition, DefinitionError> {
        let (candidate, permit) = candidate(self, "version", id).await?;
        self.run(move |db| {
            let _permit = permit;
            Ok((|| {
                let tx = db.transaction().map_err(AppError::from)?;
                target_unchanged(&tx, "version", &candidate)?;
                let result = published(&tx, id, candidate.data)?;
                tx.commit().map_err(AppError::from)?;
                Ok(result)
            })())
        })
        .await?
    }

    /// Withdraw exactly the current approval; no fallback to an older approval.
    /// # Errors
    /// Rejects stale/duplicate withdrawal, invalid labels or storage failures.
    pub async fn withdraw_definition(
        &self,
        id: i64,
        expected_approval: i64,
        actor: String,
        rationale: String,
    ) -> Result<(), DefinitionError> {
        crate::sources::review_identity(&actor, &rationale)?;
        let permit = admit()?;
        self.run(move|db|{let _permit=permit;Ok((||{let tx=db.transaction_with_behavior(TransactionBehavior::Immediate).map_err(AppError::from)?;let(current,withdrawn)=approval_state(&tx,id)?;if current==0 || current!=expected_approval || withdrawn {return Err(DefinitionError::Conflict);}tx.execute("INSERT INTO definition_withdrawals(approval_id,version_id,actor,rationale) VALUES (?1,?2,?3,?4)",params![current,id,actor,rationale]).map_err(AppError::from)?;tx.commit().map_err(AppError::from)?;Ok(())})())}).await?
    }
}

fn publish_version(
    db: &Connection,
    data: &admission::Admitted,
    latest: i64,
) -> Result<i64, DefinitionError> {
    let previous: Option<(i64, String, String)> = db
        .query_row(
            "SELECT version,content_hash,content FROM definition_versions WHERE id=?1",
            [latest],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(AppError::from)?;
    if previous
        .as_ref()
        .is_some_and(|(_, hash, content)| hash == &data.hash && content == &data.content)
    {
        return Ok(latest);
    }
    let version = previous
        .map_or(Some(1), |(version, _, _)| version.checked_add(1))
        .ok_or_else(|| invalid("published-version-overflow"))?;
    let definition = &data.definition;
    db.execute("INSERT INTO definition_versions(game_id,edition_id,definition_id,version,content,content_hash) VALUES (?1,?2,?3,?4,?5,?6)",params![definition.game_id,definition.edition_id,definition.definition_id,version,data.content,data.hash]).map_err(AppError::from)?;
    Ok(db.last_insert_rowid())
}

#[cfg(test)]
#[path = "publication_tests.rs"]
mod tests;
