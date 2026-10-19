//! Read-only lookups behind the sheet's catalogue routes (plan §3 step 3: D4, D8, D11, R5,
//! R7, R12, R13, R20, R21, R27).
//!
//! Readers use the latest **sealed** version, chosen through `catalogue_seals`, and only its
//! projections, never `content` (R17). Requests check that the version is sealed and that its
//! identity columns match; the content hash and projection equality are checked at publish,
//! by `show-catalogue-version` and by `verify-catalogues` (R5). Descriptions and badges are
//! computed live from the cited units and pages (D4); nothing is materialised.
use super::{PropertyControl, PropertyDecl, publication::ROWID_SHIFT};
use crate::{
    AppError,
    sources::search::{Budget, QUERY_BUDGET, fts_query, guarded, interrupted, read_guarded},
};
use rusqlite::{Connection, OptionalExtension, params};
use std::time::Duration;

/// Results per browse page (D5, D11).
pub(crate) const BROWSE_PAGE: usize = 50;
/// Search query length in Unicode scalars (D5); `fts_query` bounds the pieces to 16.
pub(crate) const MAX_QUERY_SCALARS: usize = 100;
/// Displayed description characters per entry (R7: replaces "64 KiB"; at most 80 KB of UTF-8).
pub(crate) const DISPLAY_CHARACTERS: usize = 20_000;
/// Largest effective unit text accepted for display: `raw_text` is at most 256 KiB at import
/// and a correction at most 80,000 bytes (`publication.rs` of the definitions); a larger value
/// is malformed.
const MAX_UNIT_BYTES: i64 = 256 * 1024;
/// Byte guard of names, labels and titles (100 scalars of at most 4 bytes).
const LABEL_BYTES: usize = 400;
/// The effective text of a unit (R21): the correction when the state is `corrected`.
const EFFECTIVE: &str = "CASE WHEN u.state='corrected' THEN u.correction ELSE u.raw_text END";

/// The latest sealed version of a linked catalogue.
pub(crate) struct Linked {
    pub version_id: i64,
    surrogate: i64,
    pub title: String,
}

/// The latest sealed version of `(game, edition, catalogue)`, or `None` when the catalogue has
/// no sealed version. The version's identity columns must equal the catalogue's.
///
/// # Errors
/// Storage failures, or a title outside its byte bound.
pub(crate) fn latest_sealed(
    db: &Connection,
    game: &str,
    edition: &str,
    catalogue: &str,
) -> Result<Option<Linked>, AppError> {
    let sql = format!(
        "SELECT v.id,c.id,{} FROM catalogues c JOIN catalogue_versions v ON v.catalogue=c.id AND v.game_id=c.game_id AND v.edition_id=c.edition_id AND v.catalogue_id=c.catalogue_id JOIN catalogue_seals s ON s.version_id=v.id WHERE c.game_id=?1 AND c.edition_id=?2 AND c.catalogue_id=?3 ORDER BY v.version DESC LIMIT 1",
        guarded("v.title", LABEL_BYTES)
    );
    Ok(db
        .query_row(&sql, params![game, edition, catalogue], |row| {
            Ok(Linked {
                version_id: row.get(0)?,
                surrogate: row.get(1)?,
                title: read_guarded(row, 2)?,
            })
        })
        .optional()?)
}

/// The badge an entry shows now (D4, R12).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Badge {
    OwnerWritten,
    Reviewed,
    Unchecked,
    Unavailable,
}

impl Badge {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::OwnerWritten => "Owner-written",
            Self::Reviewed => "Reviewed extraction",
            Self::Unchecked => "Unchecked source text",
            Self::Unavailable => "Description unavailable",
        }
    }

    pub(crate) fn sentence(self) -> &'static str {
        match self {
            Self::OwnerWritten => {
                "Written by the catalogue owner (a house rule or homebrew entry)."
            }
            Self::Reviewed => {
                "Source text reviewed. Which passages form this entry was chosen by the catalogue author."
            }
            Self::Unchecked => "Extracted from your manual; not yet reviewed.",
            Self::Unavailable => {
                "A cited passage was rejected in source review, so the text is not shown. The citation stays."
            }
        }
    }

    /// Unit-sourced badge from each cited unit's state and its page's completeness. No cited
    /// unit at all (only damage can cause it) is never "reviewed".
    fn of_units<'a>(units: impl IntoIterator<Item = (&'a str, bool)>) -> Self {
        let mut reviewed = true;
        let mut any = false;
        for (state, complete) in units {
            any = true;
            if state == "rejected" {
                return Self::Unavailable;
            }
            reviewed &= matches!(state, "accepted" | "corrected") && complete;
        }
        if reviewed && any {
            Self::Reviewed
        } else {
            Self::Unchecked
        }
    }
}

/// Browse filters, already syntax-checked by the route (`valid_id`, 100 scalars).
#[derive(Default)]
pub(crate) struct BrowseQuery {
    pub entry_type: Option<String>,
    pub q: Option<String>,
    pub after: Option<String>,
}

pub(crate) struct BrowseRow {
    pub key: String,
    pub name: String,
    pub type_label: String,
    pub retired: bool,
    pub badge: Badge,
}

pub(crate) struct BrowsePage {
    /// Entry types in declared order: (id, label).
    pub types: Vec<(String, String)>,
    pub rows: Vec<BrowseRow>,
    /// Another page follows the last row.
    pub more: bool,
}

/// One browse page: at most 50 entries ordered by `(name, key)`, after the cursor key's
/// `(name, key)` (R13). With `q`, the name index is a filter only, never a rank. Every
/// statement runs under the search time budget.
///
/// # Errors
/// `NotFound` for an entry type the version does not declare; `Invalid` for a query without
/// searchable words, with control characters or more than 16 pieces, or one that exceeds the
/// time budget; storage failures.
pub(crate) fn browse(
    db: &Connection,
    linked: &Linked,
    query: &BrowseQuery,
) -> Result<BrowsePage, AppError> {
    browse_within(db, linked, query, QUERY_BUDGET)
}

fn browse_within(
    db: &Connection,
    linked: &Linked,
    query: &BrowseQuery,
    budget: Duration,
) -> Result<BrowsePage, AppError> {
    let expression = query.q.as_deref().map(fts_query).transpose()?;
    let _budget = Budget::start(db, budget);
    let types = types(db, linked.version_id)?;
    if let Some(entry_type) = &query.entry_type
        && !types.iter().any(|(id, _)| id == entry_type)
    {
        return Err(AppError::NotFound);
    }
    // An unknown cursor key restarts from the beginning (R13).
    let cursor: Option<(String, String)> = match &query.after {
        Some(key) => db
            .query_row(
                &format!(
                    "SELECT {},key FROM catalogue_entries WHERE version_id=?1 AND key=?2",
                    guarded("name", LABEL_BYTES)
                ),
                params![linked.version_id, key],
                |row| Ok((read_guarded(row, 0)?, row.get(1)?)),
            )
            .optional()
            .map_err(interrupted)?,
        None => None,
    };
    let (cursor_name, cursor_key) = cursor.unzip();
    let mut rows = entry_rows(
        db,
        linked,
        query.entry_type.as_deref(),
        expression.as_deref(),
        cursor_name.as_deref().zip(cursor_key.as_deref()),
    )?;
    let more = rows.len() > BROWSE_PAGE;
    rows.truncate(BROWSE_PAGE);
    let keys: Vec<&str> = rows
        .iter()
        .filter(|row| row.4 == "units")
        .map(|row| row.0.as_str())
        .collect();
    let states = unit_states(db, linked.version_id, &keys)?;
    let rows = rows
        .iter()
        .map(|(key, name, type_id, retired, kind)| BrowseRow {
            key: key.clone(),
            name: name.clone(),
            type_label: types
                .iter()
                .find(|(id, _)| id == type_id)
                .map_or_else(|| type_id.clone(), |(_, label)| label.clone()),
            retired: *retired,
            badge: if kind == "owner_text" {
                Badge::OwnerWritten
            } else {
                Badge::of_units(
                    states
                        .iter()
                        .filter(|(unit_key, _, _)| unit_key == key)
                        .map(|(_, state, complete)| (state.as_str(), *complete)),
                )
            },
        })
        .collect();
    Ok(BrowsePage { types, rows, more })
}

/// (key, name, type, retired, description kind) of at most 51 entries after the cursor.
type EntryRow = (String, String, String, bool, String);

fn entry_rows(
    db: &Connection,
    linked: &Linked,
    entry_type: Option<&str>,
    expression: Option<&str>,
    cursor: Option<(&str, &str)>,
) -> Result<Vec<EntryRow>, AppError> {
    let (cursor_name, cursor_key) = cursor.unzip();
    let name = guarded("e.name", LABEL_BYTES);
    let limit = BROWSE_PAGE + 1;
    let filters = "(?2 IS NULL OR e.type_id=?2) AND (?3 IS NULL OR (e.name,e.key)>(?3,?4))";
    let map = |row: &rusqlite::Row<'_>| -> rusqlite::Result<EntryRow> {
        Ok((
            row.get(0)?,
            read_guarded(row, 1)?,
            row.get(2)?,
            row.get(3)?,
            row.get(4)?,
        ))
    };
    let mut rows = Vec::new();
    if let Some(expression) = expression {
        // The name index drives the join (CROSS JOIN fixes the order), restricted to this
        // catalogue's rowid range and joined back on (version, key) of its latest sealed
        // version, so keys that collide across catalogues cannot leak (R20).
        let base = linked
            .surrogate
            .checked_mul(1 << ROWID_SHIFT)
            .ok_or(AppError::Invalid("Catalogue index identity out of range."))?;
        let sql = format!(
            "SELECT e.key,{name},e.type_id,e.retired,e.desc_kind FROM catalogue_names_fts CROSS JOIN catalogue_entries e ON e.version_id=?1 AND e.key=catalogue_names_fts.key WHERE catalogue_names_fts MATCH ?5 AND catalogue_names_fts.rowid BETWEEN ?6 AND ?7 AND catalogue_names_fts.catalogue_row_id=?8 AND {filters} ORDER BY e.name,e.key LIMIT {limit}"
        );
        let mut statement = db.prepare(&sql).map_err(interrupted)?;
        let found = statement
            .query_map(
                params![
                    linked.version_id,
                    entry_type,
                    cursor_name,
                    cursor_key,
                    expression,
                    base,
                    base + (1 << ROWID_SHIFT) - 1,
                    linked.surrogate
                ],
                map,
            )
            .map_err(interrupted)?;
        for row in found {
            rows.push(row.map_err(interrupted)?);
        }
    } else {
        let sql = format!(
            "SELECT e.key,{name},e.type_id,e.retired,e.desc_kind FROM catalogue_entries e WHERE e.version_id=?1 AND {filters} ORDER BY e.name,e.key LIMIT {limit}"
        );
        let mut statement = db.prepare(&sql).map_err(interrupted)?;
        let found = statement
            .query_map(
                params![linked.version_id, entry_type, cursor_name, cursor_key],
                map,
            )
            .map_err(interrupted)?;
        for row in found {
            rows.push(row.map_err(interrupted)?);
        }
    }
    Ok(rows)
}

fn types(db: &Connection, version_id: i64) -> Result<Vec<(String, String)>, AppError> {
    let mut statement = db
        .prepare(&format!(
            "SELECT type_id,{} FROM catalogue_entry_types WHERE version_id=?1 ORDER BY ordinal",
            guarded("label", LABEL_BYTES)
        ))
        .map_err(interrupted)?;
    let rows = statement
        .query_map([version_id], |row| Ok((row.get(0)?, read_guarded(row, 1)?)))
        .map_err(interrupted)?;
    rows.collect::<Result<_, _>>().map_err(interrupted)
}

/// Live (key, unit state, page complete) rows of the given unit-sourced entries.
fn unit_states(
    db: &Connection,
    version_id: i64,
    keys: &[&str],
) -> Result<Vec<(String, String, bool)>, AppError> {
    if keys.is_empty() {
        return Ok(Vec::new());
    }
    let mut statement = db
        .prepare("SELECT eu.key,u.state,p.complete FROM catalogue_entry_units eu JOIN source_units u ON u.id=eu.unit_id JOIN source_pages p ON p.id=u.page_id WHERE eu.version_id=?1 AND eu.key IN (SELECT value FROM json_each(?2))")
        .map_err(interrupted)?;
    let rows = statement
        .query_map(params![version_id, serde_json::to_string(keys)?], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .map_err(interrupted)?;
    rows.collect::<Result<_, _>>().map_err(interrupted)
}

/// Where a cited unit is in the manual, read live (D4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Citation {
    pub source: String,
    pub printed_label: String,
    pub pdf_page: i64,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Description {
    OwnerText(String),
    /// Unit texts in author order; `cut` names the page of the first unit not fully shown.
    Units {
        parts: Vec<String>,
        cut: Option<Citation>,
    },
    /// A cited unit is rejected now: no text at all (D4).
    Unavailable,
    /// A cited unit's text is not text, contains NUL or exceeds its bound (R21).
    Malformed,
}

pub(crate) struct EntryPage {
    pub name: String,
    pub type_label: String,
    pub retired: bool,
    /// (property label, display value) in declared order.
    pub properties: Vec<(String, String)>,
    pub badge: Badge,
    pub description: Description,
    /// Distinct cited pages in unit order.
    pub citations: Vec<Citation>,
}

struct UnitMeta {
    id: i64,
    state: String,
    complete: bool,
    citation: Citation,
    text: bool,
    bytes: i64,
    characters: i64,
    nul: bool,
}

/// The entry page of `key` in the latest sealed version, or `None` for an unknown key. The
/// metadata of **every** cited unit is read first (R7), so a rejected unit after the cut-off
/// still makes the description unavailable; then texts are read in author order with
/// character-safe `substr` prefixes up to 20,000 characters in all.
///
/// # Errors
/// Storage failures, or stored projections outside their bounds.
pub(crate) fn entry(
    db: &Connection,
    linked: &Linked,
    key: &str,
) -> Result<Option<EntryPage>, AppError> {
    let sql = format!(
        "SELECT {},e.type_id,{},{},e.retired,e.desc_kind,{},{} FROM catalogue_entries e JOIN catalogue_entry_types t ON t.version_id=e.version_id AND t.type_id=e.type_id WHERE e.version_id=?1 AND e.key=?2",
        guarded("e.name", LABEL_BYTES),
        guarded("t.label", LABEL_BYTES),
        guarded("t.properties", 1024 * 1024),
        guarded("e.owner_text", 80_000),
        guarded("e.properties", 262_144),
    );
    let row = db
        .query_row(&sql, params![linked.version_id, key], |row| {
            Ok((
                read_guarded(row, 0)?,
                row.get::<_, String>(1)?,
                read_guarded(row, 2)?,
                read_guarded(row, 3)?,
                row.get::<_, bool>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, Option<String>>(6)?,
                read_guarded(row, 7)?,
            ))
        })
        .optional()?;
    let Some((name, _type_id, type_label, declared, retired, kind, owner_text, values)) = row
    else {
        return Ok(None);
    };
    let properties = properties(&declared, &values)?;
    if kind == "owner_text" {
        let text = owner_text.ok_or(AppError::Invalid("Owner text missing or oversized."))?;
        return Ok(Some(EntryPage {
            name,
            type_label,
            retired,
            properties,
            badge: Badge::OwnerWritten,
            description: Description::OwnerText(text),
            citations: Vec::new(),
        }));
    }
    let units = unit_metadata(db, linked.version_id, key)?;
    if units.is_empty() {
        return Err(AppError::Invalid("Catalogue entry cites no units."));
    }
    let badge = Badge::of_units(units.iter().map(|u| (u.state.as_str(), u.complete)));
    let mut citations: Vec<Citation> = Vec::new();
    for unit in &units {
        if !citations.contains(&unit.citation) {
            citations.push(unit.citation.clone());
        }
    }
    let description = if badge == Badge::Unavailable {
        Description::Unavailable
    } else if units
        .iter()
        .any(|u| !u.text || u.nul || !(0..=MAX_UNIT_BYTES).contains(&u.bytes))
    {
        Description::Malformed
    } else {
        unit_texts(db, &units)?
    };
    Ok(Some(EntryPage {
        name,
        type_label,
        retired,
        properties,
        badge,
        description,
        citations,
    }))
}

fn unit_metadata(db: &Connection, version_id: i64, key: &str) -> Result<Vec<UnitMeta>, AppError> {
    let sql = format!(
        "SELECT u.id,u.state,p.complete,{},{},p.pdf_page,typeof({EFFECTIVE})='text',length(CAST({EFFECTIVE} AS BLOB)),length({EFFECTIVE}),instr(CAST({EFFECTIVE} AS BLOB),x'00')>0 FROM catalogue_entry_units eu JOIN source_units u ON u.id=eu.unit_id JOIN source_pages p ON p.id=u.page_id JOIN sources s ON s.id=p.source_id WHERE eu.version_id=?1 AND eu.key=?2 ORDER BY eu.ordinal",
        guarded("s.title", 200),
        guarded("p.printed_label", 200),
    );
    let mut statement = db.prepare(&sql)?;
    let rows = statement.query_map(params![version_id, key], |row| {
        Ok(UnitMeta {
            id: row.get(0)?,
            state: row.get(1)?,
            complete: row.get(2)?,
            citation: Citation {
                source: read_guarded(row, 3)?,
                printed_label: read_guarded(row, 4)?,
                pdf_page: row.get(5)?,
            },
            text: row.get(6)?,
            bytes: row.get::<_, Option<i64>>(7)?.unwrap_or(-1),
            characters: row.get::<_, Option<i64>>(8)?.unwrap_or(-1),
            nul: row.get::<_, Option<bool>>(9)?.unwrap_or(true),
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Unit texts in author order, cut at 20,000 characters in all (R7).
fn unit_texts(db: &Connection, units: &[UnitMeta]) -> Result<Description, AppError> {
    let mut statement = db.prepare(&format!(
        "SELECT CASE WHEN typeof({EFFECTIVE})='text' THEN substr({EFFECTIVE},1,?2) END FROM source_units u WHERE u.id=?1"
    ))?;
    let mut remaining = DISPLAY_CHARACTERS;
    let mut parts = Vec::new();
    for unit in units {
        if remaining == 0 {
            return Ok(Description::Units {
                parts,
                cut: Some(unit.citation.clone()),
            });
        }
        let take = i64::try_from(remaining).map_err(|_| AppError::Invalid("Display bound."))?;
        let text: Option<String> = statement.query_row(params![unit.id, take], |row| row.get(0))?;
        let text = text.ok_or(AppError::Invalid("Unit text changed type during the read."))?;
        let shown = text.chars().count();
        remaining = remaining.saturating_sub(shown);
        parts.push(text);
        if unit.characters > take {
            return Ok(Description::Units {
                parts,
                cut: Some(unit.citation.clone()),
            });
        }
    }
    Ok(Description::Units { parts, cut: None })
}

/// Display values of an entry's properties in declared order. Booleans read Yes/No; enum
/// values show their option label; an empty text value is the explicit "no value".
fn properties(declared: &str, values: &str) -> Result<Vec<(String, String)>, AppError> {
    let declared: Vec<PropertyDecl> = serde_json::from_str(declared)?;
    let values: serde_json::Map<String, serde_json::Value> = serde_json::from_str(values)?;
    Ok(declared
        .iter()
        .map(|decl| {
            let value = values.get(&decl.id);
            let shown = match (decl.control, value) {
                (PropertyControl::Boolean, Some(serde_json::Value::Bool(flag))) => {
                    if *flag { "Yes" } else { "No" }.to_owned()
                }
                (PropertyControl::Integer, Some(serde_json::Value::Number(number))) => {
                    number.to_string()
                }
                (PropertyControl::Enumeration, Some(serde_json::Value::String(id))) => decl
                    .options
                    .iter()
                    .flatten()
                    .find(|option| &option.id == id)
                    .map_or_else(|| id.clone(), |option| option.label.clone()),
                (PropertyControl::Text, Some(serde_json::Value::String(text)))
                    if !text.is_empty() =>
                {
                    text.clone()
                }
                _ => "No value".to_owned(),
            };
            (decl.label.clone(), shown)
        })
        .collect())
}

#[cfg(test)]
#[path = "read_tests.rs"]
mod tests;

#[cfg(test)]
mod badge_tests {
    use super::Badge;

    #[test]
    fn unit_badges_follow_states_and_page_completion() {
        let of = |units: &[(&str, bool)]| Badge::of_units(units.iter().copied());
        assert_eq!(
            of(&[("accepted", true), ("corrected", true)]),
            Badge::Reviewed
        );
        assert_eq!(
            of(&[("accepted", true), ("corrected", false)]),
            Badge::Unchecked
        );
        assert_eq!(
            of(&[("accepted", true), ("unreviewed", true)]),
            Badge::Unchecked
        );
        assert_eq!(
            of(&[("unreviewed", false), ("rejected", true)]),
            Badge::Unavailable
        );
        assert_eq!(of(&[]), Badge::Unchecked, "no cited unit is never reviewed");
    }
}
