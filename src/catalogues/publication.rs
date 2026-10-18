//! Catalogue publication (plan §3 step 2, D6, R1/R5/R17–R20): the redacted comparison and its
//! ticket, publication of an immutable sealed version with its projections and name index,
//! and the Owner integrity check and index repair.
//!
//! Every output is metadata and counts; errors are static codes. The seal trigger
//! (`migrations/019_catalogues.sql`) independently binds the projections to the hashed content.
use super::{
    Catalogue, CatalogueCounts, CatalogueError,
    admission::{self, Admitted},
    invalid,
    store::{Draft, admit, load_draft, references},
};
use crate::Store;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

/// The name index, verbatim as in migration 019 (a test compares it with `sqlite_master`).
pub(super) const NAME_INDEX: &str = "CREATE VIRTUAL TABLE catalogue_names_fts USING fts5(name, catalogue_row_id UNINDEXED, key UNINDEXED, tokenize = 'unicode61 remove_diacritics 2', prefix = '3')";
/// Index rowid = catalogue surrogate id << 20 | entry ordinal (entries < 2^20).
const ROWID_SHIFT: u32 = 20;
const _: () = assert!(super::decode::MAX_ENTRIES < 1 << ROWID_SHIFT);
const TICKET_PREFIX: &[u8] = b"mimik-catalogue-comparison-v1\0";
/// Latest sealed version of every catalogue (used by the index check and rebuild).
const LATEST: &str = "SELECT v.catalogue AS catalogue, v.id AS version_id FROM catalogue_versions v JOIN catalogue_seals s ON s.version_id=v.id WHERE v.version=(SELECT max(v2.version) FROM catalogue_versions v2 JOIN catalogue_seals s2 ON s2.version_id=v2.id WHERE v2.catalogue=v.catalogue)";

fn publication(code: &str) -> CatalogueError {
    invalid("/publication", code)
}

fn count(value: usize) -> Result<i64, CatalogueError> {
    i64::try_from(value).map_err(|_| CatalogueError::Configuration)
}

/// One affected row, as every publication insert must report (R19).
fn one(changed: usize) -> Result<(), CatalogueError> {
    if changed == 1 {
        Ok(())
    } else {
        Err(publication("unexpected-rows-affected"))
    }
}

#[derive(Debug, PartialEq, Eq)]
struct TypeRow {
    type_id: String,
    ordinal: i64,
    label: String,
    properties: String,
}

#[derive(Debug, PartialEq, Eq)]
struct EntryRow {
    key: String,
    ordinal: i64,
    type_id: String,
    name: String,
    retired: bool,
    desc_kind: String,
    owner_text: Option<String>,
    properties: String,
}

#[derive(Debug, PartialEq, Eq)]
struct UnitRow {
    key: String,
    ordinal: i64,
    unit_id: i64,
    page_id: i64,
    source_id: i64,
}

/// The relational projections of one version, in content order.
#[derive(Debug, Default, PartialEq, Eq)]
struct Projections {
    types: Vec<TypeRow>,
    entries: Vec<EntryRow>,
    units: Vec<UnitRow>,
}

fn encode<T: Serialize>(value: &T) -> Result<String, CatalogueError> {
    serde_json::to_string(value).map_err(|_| CatalogueError::Configuration)
}

/// The projections an admitted catalogue must have (the seal checks the same in SQL).
fn projections(catalogue: &Catalogue) -> Result<Projections, CatalogueError> {
    let mut result = Projections::default();
    for (ordinal, entry_type) in catalogue.entry_types.iter().enumerate() {
        result.types.push(TypeRow {
            type_id: entry_type.id.clone(),
            ordinal: count(ordinal)?,
            label: entry_type.label.clone(),
            properties: encode(&entry_type.properties)?,
        });
    }
    for (ordinal, entry) in catalogue.entries.iter().enumerate() {
        let owner_text = entry.description.owner_text.clone();
        result.entries.push(EntryRow {
            key: entry.key.clone(),
            ordinal: count(ordinal)?,
            type_id: entry.entry_type.clone(),
            name: entry.name.clone(),
            retired: entry.retired,
            desc_kind: if owner_text.is_some() {
                "owner_text"
            } else {
                "units"
            }
            .into(),
            owner_text,
            properties: encode(&entry.properties)?,
        });
        for (number, unit) in entry.description.units.iter().flatten().enumerate() {
            result.units.push(UnitRow {
                key: entry.key.clone(),
                ordinal: count(number)?,
                unit_id: unit.unit_id,
                page_id: unit.page_id,
                source_id: unit.source_id,
            });
        }
    }
    Ok(result)
}

fn read_projections(db: &Connection, version_id: i64) -> Result<Projections, CatalogueError> {
    let mut result = Projections::default();
    let mut types = db.prepare_cached("SELECT type_id,ordinal,label,properties FROM catalogue_entry_types WHERE version_id=?1 ORDER BY ordinal")?;
    result.types = types
        .query_map([version_id], |row| {
            Ok(TypeRow {
                type_id: row.get(0)?,
                ordinal: row.get(1)?,
                label: row.get(2)?,
                properties: row.get(3)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    let mut entries = db.prepare_cached("SELECT key,ordinal,type_id,name,retired,desc_kind,owner_text,properties FROM catalogue_entries WHERE version_id=?1 ORDER BY ordinal")?;
    result.entries = entries
        .query_map([version_id], |row| {
            Ok(EntryRow {
                key: row.get(0)?,
                ordinal: row.get(1)?,
                type_id: row.get(2)?,
                name: row.get(3)?,
                retired: row.get(4)?,
                desc_kind: row.get(5)?,
                owner_text: row.get(6)?,
                properties: row.get(7)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    let mut units = db.prepare_cached("SELECT u.key,u.ordinal,u.unit_id,u.page_id,u.source_id FROM catalogue_entry_units u JOIN catalogue_entries e ON e.version_id=u.version_id AND e.key=u.key WHERE u.version_id=?1 ORDER BY e.ordinal,u.ordinal")?;
    result.units = units
        .query_map([version_id], |row| {
            Ok(UnitRow {
                key: row.get(0)?,
                ordinal: row.get(1)?,
                unit_id: row.get(2)?,
                page_id: row.get(3)?,
                source_id: row.get(4)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    Ok(result)
}

/// The latest sealed version of one catalogue.
struct Latest {
    catalogue: i64,
    version_id: i64,
    version: i64,
    content_hash: String,
}

fn latest(
    db: &Connection,
    game: &str,
    edition: &str,
    catalogue: &str,
) -> Result<Option<Latest>, CatalogueError> {
    Ok(db
        .query_row(
            "SELECT c.id,v.id,v.version,v.content_hash FROM catalogues c JOIN catalogue_versions v ON v.catalogue=c.id JOIN catalogue_seals s ON s.version_id=v.id WHERE c.game_id=?1 AND c.edition_id=?2 AND c.catalogue_id=?3 ORDER BY v.version DESC LIMIT 1",
            params![game, edition, catalogue],
            |row| {
                Ok(Latest {
                    catalogue: row.get(0)?,
                    version_id: row.get(1)?,
                    version: row.get(2)?,
                    content_hash: row.get(3)?,
                })
            },
        )
        .optional()?)
}

/// Review state of a referenced unit and its page at comparison time; the ticket binds it.
#[derive(Debug, Serialize)]
struct UnitTicket {
    unit_id: i64,
    unit_revision: i64,
    state: String,
    page_id: i64,
    page_revision: i64,
    #[serde(skip)]
    page_complete: bool,
}

#[derive(Serialize)]
struct Ticket<'a> {
    encoding: u32,
    draft_id: i64,
    draft_revision: i64,
    content_hash: &'a str,
    /// The latest sealed version (number, content hash), or `null`.
    latest: Option<(i64, &'a str)>,
    units: &'a [UnitTicket],
}

/// Distinct referenced units by review state.
#[derive(Clone, Copy, Debug, Default, Serialize, PartialEq, Eq)]
pub struct UnitStates {
    pub unreviewed: usize,
    pub accepted: usize,
    pub corrected: usize,
    pub rejected: usize,
}

/// Entries by the badge they would show now (plan D4).
#[derive(Clone, Copy, Debug, Default, Serialize, PartialEq, Eq)]
pub struct BadgeCounts {
    pub owner_written: usize,
    pub reviewed: usize,
    pub unchecked: usize,
    pub unavailable: usize,
}

/// The difference from the latest sealed version, as counts only.
#[derive(Clone, Copy, Debug, Default, Serialize, PartialEq, Eq)]
pub struct ChangeCounts {
    pub identical: bool,
    pub added_entries: usize,
    pub added_entry_types: usize,
    pub newly_retired: usize,
    pub unretired: usize,
    pub description_references_changed: usize,
    pub owner_text_changed: usize,
}

/// What publication would refuse (D3, D6).
#[derive(Clone, Copy, Debug, Default, Serialize, PartialEq, Eq)]
pub struct RefusalCounts {
    pub dropped_entries: usize,
    pub retyped_entries: usize,
    pub dropped_entry_types: usize,
    pub rejected_units: usize,
}

impl RefusalCounts {
    fn codes(self) -> Vec<&'static str> {
        [
            (self.dropped_entries, "dropped-entry-keys"),
            (self.retyped_entries, "retyped-entry-keys"),
            (self.dropped_entry_types, "dropped-entry-types"),
            (self.rejected_units, "rejected-units"),
        ]
        .into_iter()
        .filter(|(count, _)| *count > 0)
        .map(|(_, code)| code)
        .collect()
    }
}

/// `compare-catalogue` output: identity, counts and the ticket; never names, keys or text.
#[derive(Debug, Serialize)]
pub struct CatalogueComparison {
    pub draft_id: i64,
    pub draft_revision: i64,
    pub game_id: String,
    pub edition_id: String,
    pub catalogue_id: String,
    pub content_hash: String,
    pub latest_version: Option<i64>,
    pub counts: CatalogueCounts,
    pub units: UnitStates,
    pub badges: BadgeCounts,
    pub changes: ChangeCounts,
    pub refusals: RefusalCounts,
    pub publishable: bool,
    pub comparison_hash: String,
}

/// What the latest version held for one key.
struct Previous {
    type_id: String,
    retired: bool,
    owner_text: Option<String>,
    units: Vec<(i64, i64, i64)>,
}

fn previous(
    db: &Connection,
    version_id: i64,
) -> Result<(BTreeMap<String, Previous>, BTreeSet<String>), CatalogueError> {
    let rows = read_projections(db, version_id)?;
    let mut entries: BTreeMap<String, Previous> = rows
        .entries
        .into_iter()
        .map(|entry| {
            (
                entry.key,
                Previous {
                    type_id: entry.type_id,
                    retired: entry.retired,
                    owner_text: entry.owner_text,
                    units: Vec::new(),
                },
            )
        })
        .collect();
    for unit in rows.units {
        if let Some(entry) = entries.get_mut(&unit.key) {
            entry
                .units
                .push((unit.source_id, unit.page_id, unit.unit_id));
        }
    }
    Ok((
        entries,
        rows.types.into_iter().map(|row| row.type_id).collect(),
    ))
}

fn unit_tickets(db: &Connection, catalogue: &Catalogue) -> Result<Vec<UnitTicket>, CatalogueError> {
    let ids: BTreeSet<i64> = catalogue
        .entries
        .iter()
        .flat_map(|entry| entry.description.units.iter().flatten())
        .map(|unit| unit.unit_id)
        .collect();
    let mut query = db.prepare_cached("SELECT u.revision,u.state,u.page_id,p.revision,p.complete FROM source_units u JOIN source_pages p ON p.id=u.page_id WHERE u.id=?1 AND typeof(u.revision)='integer' AND typeof(p.revision)='integer' AND u.state IN ('unreviewed','accepted','corrected','rejected') AND p.complete IN (0,1)")?;
    ids.into_iter()
        .map(|unit_id| {
            query
                .query_row([unit_id], |row| {
                    Ok(UnitTicket {
                        unit_id,
                        unit_revision: row.get(0)?,
                        state: row.get(1)?,
                        page_id: row.get(2)?,
                        page_revision: row.get(3)?,
                        page_complete: row.get::<_, i64>(4)? == 1,
                    })
                })
                .optional()?
                .ok_or_else(|| publication("unit-missing-or-invalid-metadata"))
        })
        .collect()
}

fn unit_states(tickets: &[UnitTicket]) -> UnitStates {
    let mut units = UnitStates::default();
    for ticket in tickets {
        match ticket.state.as_str() {
            "accepted" => units.accepted += 1,
            "corrected" => units.corrected += 1,
            "rejected" => units.rejected += 1,
            _ => units.unreviewed += 1,
        }
    }
    units
}

/// Entries by the badge they would show now (plan D4), from the ticketed unit states.
fn badges(catalogue: &Catalogue, tickets: &[UnitTicket]) -> BadgeCounts {
    let by_id: BTreeMap<i64, &UnitTicket> = tickets.iter().map(|t| (t.unit_id, t)).collect();
    let mut badges = BadgeCounts::default();
    for entry in &catalogue.entries {
        let Some(cited) = &entry.description.units else {
            badges.owner_written += 1;
            continue;
        };
        let states: Vec<&UnitTicket> = cited
            .iter()
            .filter_map(|unit| by_id.get(&unit.unit_id).copied())
            .collect();
        if states.iter().any(|t| t.state == "rejected") {
            badges.unavailable += 1;
        } else if states
            .iter()
            .all(|t| matches!(t.state.as_str(), "accepted" | "corrected") && t.page_complete)
        {
            badges.reviewed += 1;
        } else {
            badges.unchecked += 1;
        }
    }
    badges
}

/// Counts of changes from the latest sealed version, and the append-only refusals (D3).
fn diff(
    db: &Connection,
    latest: Option<&Latest>,
    draft: &Draft,
    refusals: &mut RefusalCounts,
) -> Result<ChangeCounts, CatalogueError> {
    let catalogue = &draft.data.catalogue;
    let mut changes = ChangeCounts::default();
    let Some(latest) = latest else {
        changes.added_entries = catalogue.entries.len();
        changes.added_entry_types = catalogue.entry_types.len();
        return Ok(changes);
    };
    changes.identical = latest.content_hash == draft.data.hash;
    let (old, old_types) = previous(db, latest.version_id)?;
    let new_types: BTreeSet<&str> = catalogue
        .entry_types
        .iter()
        .map(|t| t.id.as_str())
        .collect();
    changes.added_entry_types = new_types
        .iter()
        .filter(|id| !old_types.contains(**id))
        .count();
    refusals.dropped_entry_types = old_types
        .iter()
        .filter(|id| !new_types.contains(id.as_str()))
        .count();
    let mut seen = 0;
    for entry in &catalogue.entries {
        let Some(before) = old.get(&entry.key) else {
            changes.added_entries += 1;
            continue;
        };
        seen += 1;
        if before.type_id != entry.entry_type {
            refusals.retyped_entries += 1;
        }
        match (before.retired, entry.retired) {
            (false, true) => changes.newly_retired += 1,
            (true, false) => changes.unretired += 1,
            _ => {}
        }
        let now: Vec<(i64, i64, i64)> = entry
            .description
            .units
            .iter()
            .flatten()
            .map(|unit| (unit.source_id, unit.page_id, unit.unit_id))
            .collect();
        if now != before.units {
            changes.description_references_changed += 1;
        } else if before.owner_text != entry.description.owner_text {
            changes.owner_text_changed += 1;
        }
    }
    refusals.dropped_entries = old.len() - seen;
    Ok(changes)
}

/// The comparison of a draft with the latest sealed version, from one database snapshot.
fn compare_in(db: &Connection, draft: &Draft) -> Result<CatalogueComparison, CatalogueError> {
    references(db, &draft.data)?;
    let catalogue = &draft.data.catalogue;
    let latest = latest(
        db,
        &catalogue.game_id,
        &catalogue.edition_id,
        &catalogue.catalogue_id,
    )?;
    let tickets = unit_tickets(db, catalogue)?;
    let units = unit_states(&tickets);
    let mut refusals = RefusalCounts {
        rejected_units: units.rejected,
        ..RefusalCounts::default()
    };
    let changes = diff(db, latest.as_ref(), draft, &mut refusals)?;
    let ticket = Ticket {
        encoding: 1,
        draft_id: draft.id,
        draft_revision: draft.revision,
        content_hash: &draft.data.hash,
        latest: latest
            .as_ref()
            .map(|latest| (latest.version, latest.content_hash.as_str())),
        units: &tickets,
    };
    let comparison_hash = hash(TICKET_PREFIX, encode(&ticket)?.as_bytes());
    Ok(CatalogueComparison {
        draft_id: draft.id,
        draft_revision: draft.revision,
        game_id: catalogue.game_id.clone(),
        edition_id: catalogue.edition_id.clone(),
        catalogue_id: catalogue.catalogue_id.clone(),
        content_hash: draft.data.hash.clone(),
        latest_version: latest.map(|latest| latest.version),
        counts: draft.data.counts,
        units,
        badges: badges(catalogue, &tickets),
        changes,
        publishable: refusals.codes().is_empty(),
        refusals,
        comparison_hash,
    })
}

fn hash(prefix: &[u8], bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    hash.update(prefix);
    hash.update(bytes);
    hex::encode(hash.finalize())
}

/// The draft row still has the revision and content that were admitted.
fn draft_unchanged(db: &Connection, draft: &Draft) -> Result<(), CatalogueError> {
    let current: (i64, String) = db
        .query_row(
            "SELECT revision,content_hash FROM catalogue_drafts WHERE id=?1",
            [draft.id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?
        .ok_or(CatalogueError::NotFound)?;
    if current != (draft.revision, draft.data.hash.clone()) {
        return Err(CatalogueError::Conflict);
    }
    Ok(())
}

/// The operator's assertion for one publication.
pub struct PublishRequest {
    pub comparison_hash: String,
    pub actor: String,
    pub rationale: String,
}

/// Metadata of one sealed version; never names, keys or text.
#[derive(Debug, Serialize)]
pub struct CatalogueVersionSummary {
    pub version_id: i64,
    pub game_id: String,
    pub edition_id: String,
    pub catalogue_id: String,
    pub version: i64,
    pub latest_version: i64,
    pub content_hash: String,
    pub draft_id: i64,
    pub draft_revision: i64,
    pub created_at: String,
    pub status: &'static str,
    pub counts: CatalogueCounts,
}

/// `publish-catalogue` output. `published` is false when the draft equals the latest version.
#[derive(Debug, Serialize)]
pub struct PublishedCatalogue {
    pub published: bool,
    #[serde(flatten)]
    pub version: CatalogueVersionSummary,
}

fn summary_in(
    db: &Connection,
    version_id: i64,
    counts: CatalogueCounts,
) -> Result<CatalogueVersionSummary, CatalogueError> {
    Ok(db.query_row(
        "SELECT v.game_id,v.edition_id,v.catalogue_id,v.version,(SELECT max(v2.version) FROM catalogue_versions v2 JOIN catalogue_seals s2 ON s2.version_id=v2.id WHERE v2.catalogue=v.catalogue),v.content_hash,v.draft_id,v.draft_revision,v.created_at FROM catalogue_versions v JOIN catalogue_seals s ON s.version_id=v.id WHERE v.id=?1",
        [version_id],
        |row| {
            Ok(CatalogueVersionSummary {
                version_id,
                game_id: row.get(0)?,
                edition_id: row.get(1)?,
                catalogue_id: row.get(2)?,
                version: row.get(3)?,
                latest_version: row.get(4)?,
                content_hash: row.get(5)?,
                draft_id: row.get(6)?,
                draft_revision: row.get(7)?,
                created_at: row.get(8)?,
                status: "sealed",
                counts,
            })
        },
    )?)
}

/// A publication stage, for fault-injection tests of the single transaction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Stage {
    Projections,
    NameIndex,
    Seal,
}

/// Recompute the comparison and publish inside the caller's `IMMEDIATE` transaction.
/// `checkpoint` runs before each stage (tests inject failures there).
pub(super) fn publish_in(
    db: &Connection,
    draft: &Draft,
    request: &PublishRequest,
    checkpoint: &dyn Fn(Stage) -> Result<(), CatalogueError>,
) -> Result<PublishedCatalogue, CatalogueError> {
    draft_unchanged(db, draft)?;
    let check = compare_in(db, draft)?;
    if check.comparison_hash != request.comparison_hash {
        return Err(CatalogueError::StaleComparison);
    }
    let codes = check.refusals.codes();
    if !codes.is_empty() {
        return Err(CatalogueError::Invalid(super::report(
            codes
                .into_iter()
                .map(|code| super::diagnostic("/publication", code))
                .collect(),
            false,
        )));
    }
    let catalogue = &draft.data.catalogue;
    let current = latest(
        db,
        &catalogue.game_id,
        &catalogue.edition_id,
        &catalogue.catalogue_id,
    )?;
    if let Some(current) = current.as_ref().filter(|_| check.changes.identical) {
        return Ok(PublishedCatalogue {
            published: false,
            version: summary_in(db, current.version_id, draft.data.counts)?,
        });
    }
    // R1: the canonical content re-hashes to the version hash before anything is inserted.
    if admission::hash(draft.data.content.as_bytes()) != draft.data.hash {
        return Err(publication("content-hash-mismatch"));
    }
    let rows = projections(catalogue)?;
    let surrogate = if let Some(current) = &current {
        current.catalogue
    } else {
        one(db.execute(
            "INSERT INTO catalogues(game_id,edition_id,catalogue_id) VALUES (?1,?2,?3)",
            params![
                catalogue.game_id,
                catalogue.edition_id,
                catalogue.catalogue_id
            ],
        )?)?;
        db.last_insert_rowid()
    };
    let version = current
        .as_ref()
        .map_or(Some(1), |current| current.version.checked_add(1))
        .ok_or_else(|| publication("version-overflow"))?;
    one(db.execute(
        "INSERT INTO catalogue_versions(catalogue,game_id,edition_id,catalogue_id,version,title,content,content_hash,draft_id,draft_revision,comparison_hash,actor,rationale) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
        params![surrogate, catalogue.game_id, catalogue.edition_id, catalogue.catalogue_id, version, catalogue.title, draft.data.content, draft.data.hash, draft.id, draft.revision, check.comparison_hash, request.actor, request.rationale],
    )?)?;
    let version_id = db.last_insert_rowid();
    checkpoint(Stage::Projections)?;
    insert_projections(db, version_id, &rows)?;
    checkpoint(Stage::NameIndex)?;
    let base = surrogate
        .checked_mul(1 << ROWID_SHIFT)
        .ok_or_else(|| publication("index-rowid-overflow"))?;
    db.execute(
        "DELETE FROM catalogue_names_fts WHERE rowid BETWEEN ?1 AND ?2",
        params![base, base + (1 << ROWID_SHIFT) - 1],
    )?;
    let indexed = db.execute(
        "INSERT INTO catalogue_names_fts(rowid,name,catalogue_row_id,key) SELECT ?1+ordinal,name,?2,key FROM catalogue_entries WHERE version_id=?3",
        params![base, surrogate, version_id],
    )?;
    if indexed != rows.entries.len() {
        return Err(publication("unexpected-rows-affected"));
    }
    checkpoint(Stage::Seal)?;
    one(db.execute(
        "INSERT INTO catalogue_seals(version_id) VALUES (?1)",
        [version_id],
    )?)?;
    Ok(PublishedCatalogue {
        published: true,
        version: summary_in(db, version_id, draft.data.counts)?,
    })
}

fn insert_projections(
    db: &Connection,
    version_id: i64,
    rows: &Projections,
) -> Result<(), CatalogueError> {
    let mut types = db.prepare_cached("INSERT INTO catalogue_entry_types(version_id,type_id,ordinal,label,properties) VALUES (?1,?2,?3,?4,?5)")?;
    for row in &rows.types {
        one(types.execute(params![
            version_id,
            row.type_id,
            row.ordinal,
            row.label,
            row.properties
        ])?)?;
    }
    let mut entries = db.prepare_cached("INSERT INTO catalogue_entries(version_id,key,ordinal,type_id,name,retired,desc_kind,owner_text,properties) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)")?;
    for row in &rows.entries {
        one(entries.execute(params![
            version_id,
            row.key,
            row.ordinal,
            row.type_id,
            row.name,
            row.retired,
            row.desc_kind,
            row.owner_text,
            row.properties
        ])?)?;
    }
    let mut units = db.prepare_cached("INSERT INTO catalogue_entry_units(version_id,key,ordinal,unit_id,page_id,source_id) VALUES (?1,?2,?3,?4,?5,?6)")?;
    for row in &rows.units {
        one(units.execute(params![
            version_id,
            row.key,
            row.ordinal,
            row.unit_id,
            row.page_id,
            row.source_id
        ])?)?;
    }
    Ok(())
}

/// Re-verify one stored version (R5): sealed, bounded content that re-admits to the same
/// canonical bytes and hash, identity columns, and projections equal to the content.
fn verify_version(
    db: &Connection,
    version_id: i64,
) -> Result<Result<Admitted, &'static str>, CatalogueError> {
    struct Stored {
        content: Option<String>,
        hash: String,
        identity: [String; 4],
        sealed: bool,
    }
    let row = db
        .query_row(
            "SELECT CASE WHEN length(CAST(v.content AS BLOB))<=4194304 THEN v.content END,v.content_hash,v.game_id,v.edition_id,v.catalogue_id,v.title,EXISTS(SELECT 1 FROM catalogue_seals s WHERE s.version_id=v.id) FROM catalogue_versions v JOIN catalogues c ON c.id=v.catalogue AND c.game_id=v.game_id AND c.edition_id=v.edition_id AND c.catalogue_id=v.catalogue_id WHERE v.id=?1",
            [version_id],
            |row| {
                Ok(Stored {
                    content: row.get(0)?,
                    hash: row.get(1)?,
                    identity: [row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?],
                    sealed: row.get(6)?,
                })
            },
        )
        .optional()?;
    let Some(Stored {
        content,
        hash: stored_hash,
        identity: [game, edition, catalogue_id, title],
        sealed,
    }) = row
    else {
        return Ok(Err("version-identity-mismatch"));
    };
    if !sealed {
        return Ok(Err("version-not-sealed"));
    }
    let Some(content) = content else {
        return Ok(Err("content-byte-limit"));
    };
    let Ok(data) = admission::validate(content.as_bytes()) else {
        return Ok(Err("content-not-admissible"));
    };
    if data.content != content || data.hash != stored_hash {
        return Ok(Err("content-hash-mismatch"));
    }
    let catalogue = &data.catalogue;
    if catalogue.game_id != game
        || catalogue.edition_id != edition
        || catalogue.catalogue_id != catalogue_id
        || catalogue.title != title
    {
        return Ok(Err("version-identity-mismatch"));
    }
    if projections(catalogue)? != read_projections(db, version_id)? {
        return Ok(Err("projection-mismatch"));
    }
    Ok(Ok(data))
}

/// One integrity problem: surrogate catalogue and version ids with a static code.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct IntegrityProblem {
    pub catalogue: i64,
    pub version: Option<i64>,
    pub code: &'static str,
}

/// `verify-catalogues` output.
#[derive(Debug, Serialize)]
pub struct CatalogueVerification {
    pub catalogues: usize,
    pub versions: usize,
    pub index_rows: i64,
    pub problems: Vec<IntegrityProblem>,
    pub status: &'static str,
}

/// `rebuild-catalogue-index` output.
#[derive(Debug, Serialize)]
pub struct CatalogueIndexRebuild {
    pub catalogues: i64,
    pub index_rows: usize,
}

/// FTS identity, name and multiplicity against the latest sealed projections (R20).
fn index_matches(db: &Connection) -> Result<(bool, i64), CatalogueError> {
    let sql = format!(
        "WITH latest AS ({LATEST}), expected AS (SELECT (l.catalogue<<{ROWID_SHIFT})+e.ordinal AS rid,l.catalogue AS catalogue,e.key AS key,e.name AS name FROM latest l JOIN catalogue_entries e ON e.version_id=l.version_id) \
         SELECT (SELECT count(*) FROM (SELECT rowid,catalogue_row_id,key,name FROM catalogue_names_fts EXCEPT SELECT rid,catalogue,key,name FROM expected)), \
         (SELECT count(*) FROM (SELECT rid,catalogue,key,name FROM expected EXCEPT SELECT rowid,catalogue_row_id,key,name FROM catalogue_names_fts)), \
         (SELECT count(*) FROM catalogue_names_fts),(SELECT count(*) FROM expected)"
    );
    let (extra, missing, actual, expected): (i64, i64, i64, i64) =
        db.query_row(&sql, [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?;
    Ok((extra == 0 && missing == 0 && actual == expected, actual))
}

fn index_definition_matches(db: &Connection) -> Result<bool, CatalogueError> {
    let sql: Option<String> = db
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type='table' AND name='catalogue_names_fts'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    Ok(sql.as_deref() == Some(NAME_INDEX))
}

fn verify_all(db: &mut Connection) -> Result<CatalogueVerification, CatalogueError> {
    let versions: Vec<(i64, i64, i64)> = {
        let mut query = db.prepare(
            "SELECT id,catalogue,version FROM catalogue_versions ORDER BY catalogue,version",
        )?;
        query
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
            .collect::<Result<_, _>>()?
    };
    let catalogues: i64 = db.query_row("SELECT count(*) FROM catalogues", [], |row| row.get(0))?;
    let mut problems = Vec::new();
    // One short read transaction per immutable version, so a long check never holds a
    // shared lock that would make a concurrent writer's commit time out.
    for &(version_id, catalogue, version) in &versions {
        let tx = db.transaction()?;
        if let Err(code) = verify_version(&tx, version_id)? {
            problems.push(IntegrityProblem {
                catalogue,
                version: Some(version),
                code,
            });
        }
        tx.commit()?;
    }
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let (matches, index_rows) = index_matches(&tx)?;
    if !index_definition_matches(&tx)? {
        problems.push(IntegrityProblem {
            catalogue: 0,
            version: None,
            code: "name-index-definition-mismatch",
        });
    }
    if !matches {
        problems.push(IntegrityProblem {
            catalogue: 0,
            version: None,
            code: "name-index-mismatch",
        });
    }
    if tx
        .execute(
            "INSERT INTO catalogue_names_fts(catalogue_names_fts) VALUES ('integrity-check')",
            [],
        )
        .is_err()
    {
        problems.push(IntegrityProblem {
            catalogue: 0,
            version: None,
            code: "name-index-integrity-check-failed",
        });
    }
    tx.rollback()?;
    Ok(CatalogueVerification {
        catalogues: usize::try_from(catalogues).map_err(|_| CatalogueError::Configuration)?,
        versions: versions.len(),
        index_rows,
        status: if problems.is_empty() { "ok" } else { "failed" },
        problems,
    })
}

fn rebuild_in(db: &Connection) -> Result<CatalogueIndexRebuild, CatalogueError> {
    db.execute_batch("DROP TABLE IF EXISTS catalogue_names_fts")?;
    db.execute_batch(NAME_INDEX)?;
    let index_rows = db.execute(
        &format!("INSERT INTO catalogue_names_fts(rowid,name,catalogue_row_id,key) SELECT (l.catalogue<<{ROWID_SHIFT})+e.ordinal,e.name,l.catalogue,e.key FROM ({LATEST}) l JOIN catalogue_entries e ON e.version_id=l.version_id"),
        [],
    )?;
    let catalogues: i64 = db.query_row(&format!("SELECT count(*) FROM ({LATEST})"), [], |row| {
        row.get(0)
    })?;
    Ok(CatalogueIndexRebuild {
        catalogues,
        index_rows,
    })
}

fn valid_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl Store {
    /// Compare a stored draft with the latest sealed version; counts and the ticket only.
    ///
    /// # Errors
    /// Rejects saturation, unknown or corrupt drafts, bad source references or storage failures.
    pub async fn compare_catalogue(&self, id: i64) -> Result<CatalogueComparison, CatalogueError> {
        let (draft, permit) = load_draft(self, id).await?;
        self.run(move |db| {
            let _permit = permit;
            Ok((|| {
                let tx = db.transaction()?;
                draft_unchanged(&tx, &draft)?;
                let result = compare_in(&tx, &draft)?;
                tx.commit()?;
                Ok(result)
            })())
        })
        .await?
    }

    /// Publish a draft as the next sealed version, with the exact current ticket.
    ///
    /// # Errors
    /// Rejects a stale ticket, refused changes (dropped or retyped keys or types, rejected
    /// units), invalid actor or rationale, saturation or storage failures. Nothing is written
    /// on any error.
    pub async fn publish_catalogue(
        &self,
        id: i64,
        request: PublishRequest,
    ) -> Result<PublishedCatalogue, CatalogueError> {
        crate::sources::review_identity(&request.actor, &request.rationale)?;
        if !valid_hash(&request.comparison_hash) {
            return Err(invalid("/comparison_hash", "comparison-hash-format"));
        }
        let (draft, permit) = load_draft(self, id).await?;
        self.run(move |db| {
            let _permit = permit;
            Ok((|| {
                let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
                let result = publish_in(&tx, &draft, &request, &|_| Ok(()))?;
                tx.commit()?;
                Ok(result)
            })())
        })
        .await?
    }

    /// Show a sealed version's metadata (latest by default) after re-verifying its content
    /// hash, identity and projections (R5).
    ///
    /// # Errors
    /// Rejects malformed ids, unknown versions, integrity failures or storage failures.
    pub async fn catalogue_version(
        &self,
        game_id: String,
        edition_id: String,
        catalogue_id: String,
        version: Option<i64>,
    ) -> Result<CatalogueVersionSummary, CatalogueError> {
        for (path, id) in [
            ("/game_id", &game_id),
            ("/edition_id", &edition_id),
            ("/catalogue_id", &catalogue_id),
        ] {
            if !crate::registry::valid_id(id) {
                return Err(invalid(path, "invalid-stable-id"));
            }
        }
        if version.is_some_and(|version| version <= 0) {
            return Err(invalid("/version", "positive-version-required"));
        }
        let permit = admit()?;
        self.run(move |db| {
            let _permit = permit;
            Ok((|| {
                let tx = db.transaction()?;
                let version_id: i64 = tx
                    .query_row(
                        "SELECT v.id FROM catalogues c JOIN catalogue_versions v ON v.catalogue=c.id JOIN catalogue_seals s ON s.version_id=v.id WHERE c.game_id=?1 AND c.edition_id=?2 AND c.catalogue_id=?3 AND (?4 IS NULL OR v.version=?4) ORDER BY v.version DESC LIMIT 1",
                        params![game_id, edition_id, catalogue_id, version],
                        |row| row.get(0),
                    )
                    .optional()?
                    .ok_or(CatalogueError::NotFound)?;
                let data = verify_version(&tx, version_id)?
                    .map_err(|code| invalid("/version", code))?;
                let result = summary_in(&tx, version_id, data.counts)?;
                tx.commit()?;
                Ok(result)
            })())
        })
        .await?
    }

    /// Owner integrity check of every catalogue version and the name index (R5, R20).
    ///
    /// # Errors
    /// Rejects saturation or storage failures; integrity problems are reported, not errors.
    pub async fn verify_catalogues(&self) -> Result<CatalogueVerification, CatalogueError> {
        let permit = admit()?;
        self.run(move |db| {
            let _permit = permit;
            Ok(verify_all(db))
        })
        .await?
    }

    /// Rebuild the derived name index from the latest sealed projections in one transaction.
    ///
    /// # Errors
    /// Rejects saturation or storage failures; nothing changes on error.
    pub async fn rebuild_catalogue_index(&self) -> Result<CatalogueIndexRebuild, CatalogueError> {
        let permit = admit()?;
        self.run(move |db| {
            let _permit = permit;
            Ok((|| {
                let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
                let result = rebuild_in(&tx)?;
                tx.commit()?;
                Ok(result)
            })())
        })
        .await?
    }
}

#[cfg(test)]
#[path = "publication_tests.rs"]
mod tests;
