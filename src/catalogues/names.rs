//! The derived name index for linking on view (assisted sheet step 3a): one normalised name
//! per entry of the latest sealed version of each catalogue (`catalogue_entry_names`, migration
//! 033), compared by `campaigns::catalogue::entry_links`.
//!
//! Like the FTS name index it is derived and has no immutability triggers. The seal
//! transaction replaces a catalogue's rows ([`replace_rows`]); store open rebuilds them one
//! catalogue per transaction when the marker differs from this build's
//! [`text_fold::name_fold_version`] ([`ensure`]); `verify-catalogues` recomputes and compares
//! them ([`problems`]) and `rebuild-catalogue-index` rebuilds them ([`rebuild_in`]).
use super::publication::LATEST;
use crate::{AppError, text_fold};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use std::collections::BTreeSet;

/// The marker stored in the database, if any.
fn marker(db: &Connection) -> rusqlite::Result<Option<String>> {
    db.query_row(
        "SELECT version FROM catalogue_name_fold WHERE id=1",
        [],
        |row| row.get(0),
    )
    .optional()
}

fn set_marker(db: &Connection) -> rusqlite::Result<()> {
    db.execute(
        "INSERT INTO catalogue_name_fold(id,version) VALUES (1,?1) ON CONFLICT(id) DO UPDATE SET version=excluded.version",
        [text_fold::name_fold_version()],
    )?;
    Ok(())
}

/// The `(entry key, norm)` rows of one version, from its stored entry names.
fn norms(db: &Connection, version_id: i64) -> rusqlite::Result<Vec<(String, String)>> {
    let mut query = db.prepare_cached(
        "SELECT key,name FROM catalogue_entries WHERE version_id=?1 ORDER BY ordinal",
    )?;
    query
        .query_map([version_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .map(|row| row.map(|(key, name)| (key, text_fold::name_norm(&name))))
        .collect()
}

/// Replaces the rows of `catalogue` (surrogate id) with those of its newly sealed `version_id`:
/// the index holds the latest sealed version only.
pub(crate) fn replace_rows(
    db: &Connection,
    catalogue: i64,
    version_id: i64,
) -> rusqlite::Result<usize> {
    db.execute(
        "DELETE FROM catalogue_entry_names WHERE version_id IN (SELECT id FROM catalogue_versions WHERE catalogue=?1)",
        [catalogue],
    )?;
    let mut insert = db.prepare_cached(
        "INSERT INTO catalogue_entry_names(version_id,entry_key,norm) VALUES (?1,?2,?3)",
    )?;
    let rows = norms(db, version_id)?;
    for (key, norm) in &rows {
        insert.execute(params![version_id, key, norm])?;
    }
    Ok(rows.len())
}

fn latest(db: &Connection) -> rusqlite::Result<Vec<(i64, i64)>> {
    let mut query = db.prepare(&format!(
        "SELECT catalogue,version_id FROM ({LATEST}) ORDER BY catalogue"
    ))?;
    query
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect()
}

/// Store open: when the marker differs from this build's, rebuilds the rows one catalogue per
/// transaction (each within the publication lock budget), drops rows of versions that are not
/// the latest, and writes the marker after the last. A failure leaves the marker, so the next
/// open starts again.
///
/// # Errors
/// Storage failures.
pub(crate) fn ensure(db: &mut Connection) -> Result<(), AppError> {
    if marker(db)?.as_deref() == Some(text_fold::name_fold_version().as_str()) {
        return Ok(());
    }
    let catalogues: Vec<i64> = latest(db)?
        .into_iter()
        .map(|(catalogue, _)| catalogue)
        .collect();
    for catalogue in catalogues {
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        // The latest sealed version is read again under the write lock: a seal that landed
        // since the list was made is the one whose rows are kept.
        let version: Option<i64> = tx
            .query_row(
                &format!("SELECT version_id FROM ({LATEST}) WHERE catalogue=?1"),
                [catalogue],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(version_id) = version {
            replace_rows(&tx, catalogue, version_id)?;
        }
        tx.commit()?;
    }
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    tx.execute(
        &format!("DELETE FROM catalogue_entry_names WHERE version_id NOT IN (SELECT version_id FROM ({LATEST}))"),
        [],
    )?;
    set_marker(&tx)?;
    tx.commit()?;
    Ok(())
}

/// `rebuild-catalogue-index`: every row recomputed from the latest sealed projections inside
/// the caller's transaction. Returns the number of rows.
pub(crate) fn rebuild_in(db: &Connection) -> rusqlite::Result<usize> {
    db.execute("DELETE FROM catalogue_entry_names", [])?;
    let mut rows = 0;
    for (catalogue, version_id) in latest(db)? {
        rows += replace_rows(db, catalogue, version_id)?;
    }
    set_marker(db)?;
    Ok(rows)
}

/// `verify-catalogues`: the stored rows against a recomputation, and the marker. Codes only.
pub(crate) fn problems(db: &Connection) -> rusqlite::Result<Vec<&'static str>> {
    let mut problems = Vec::new();
    let mut expected = BTreeSet::new();
    for (_, version_id) in latest(db)? {
        for (key, norm) in norms(db, version_id)? {
            expected.insert((version_id, key, norm));
        }
    }
    let mut query = db.prepare("SELECT version_id,entry_key,norm FROM catalogue_entry_names")?;
    let mut actual = BTreeSet::new();
    let mut count = 0_usize;
    for row in query.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))? {
        actual.insert(row?);
        count += 1;
    }
    if actual != expected || count != expected.len() {
        problems.push("entry-names-mismatch");
    }
    if marker(db)?.as_deref() != Some(text_fold::name_fold_version().as_str()) {
        problems.push("entry-names-marker-mismatch");
    }
    Ok(problems)
}

/// The eligible entries (not retired, of `entry_type` when given) of `version_id` whose norm
/// is one of `norms`: `(norm, entry key)` pairs.
pub(crate) fn eligible(
    db: &Connection,
    version_id: i64,
    entry_type: Option<&str>,
    wanted: &[String],
) -> rusqlite::Result<Vec<(String, String)>> {
    let mut query = db.prepare_cached(
        "SELECT n.norm,n.entry_key FROM catalogue_entry_names n JOIN catalogue_entries e ON e.version_id=n.version_id AND e.key=n.entry_key WHERE n.version_id=?1 AND n.norm IN (SELECT value FROM json_each(?2)) AND e.retired=0 AND (?3 IS NULL OR e.type_id=?3) ORDER BY n.norm,n.entry_key",
    )?;
    let wanted = serde_json::to_string(wanted)
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
    query
        .query_map(params![version_id, wanted, entry_type], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?
        .collect()
}

/// The norms among `wanted` that any entry of `version_id` has, retired or of any type: what
/// tells "No current entry of this type has this name" from "Not in the catalogue" (step 4).
pub(crate) fn present(
    db: &Connection,
    version_id: i64,
    wanted: &[String],
) -> rusqlite::Result<BTreeSet<String>> {
    let mut query = db.prepare_cached(
        "SELECT DISTINCT norm FROM catalogue_entry_names WHERE version_id=?1 AND norm IN (SELECT value FROM json_each(?2))",
    )?;
    let wanted = serde_json::to_string(wanted)
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
    query
        .query_map(params![version_id, wanted], |row| row.get(0))?
        .collect()
}
