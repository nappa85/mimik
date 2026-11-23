//! Registered identities are not approved sheet definitions or rule coverage.
use crate::{
    AppError, Store,
    pagination::Listing,
    sources::materials::{read_text, text},
};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RulesetKey {
    pub game_id: String,
    pub edition_id: String,
}

#[derive(Debug, Serialize)]
pub struct RulesetEntry {
    pub game_id: String,
    pub game_label: String,
    pub edition_id: String,
    pub edition_label: String,
    pub generic_sheet_available: bool,
}

pub struct RegisterRuleset {
    pub game_id: String,
    pub game_label: String,
    pub edition_id: String,
    pub edition_label: String,
}

pub struct RegisterAlias {
    pub game_id: String,
    pub edition_id: String,
    pub legacy_edition: String,
}

pub(crate) fn valid_id(value: &str) -> bool {
    (1..=64).contains(&value.len())
        && value.as_bytes()[0].is_ascii_lowercase()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn valid_label(value: &str) -> bool {
    !value.is_empty() && value == value.trim() && value.chars().count() <= 100
}

fn invalid() -> AppError {
    AppError::Invalid(
        "Invalid or conflicting ruleset identity/label. IDs are lowercase slugs; labels contain 1–100 characters without surrounding whitespace.",
    )
}

pub(crate) fn alias(
    db: &Connection,
    system: &str,
    edition: &str,
) -> Result<Option<RulesetKey>, AppError> {
    Ok(db.query_row("SELECT game_id,edition_id FROM legacy_ruleset_aliases WHERE system_id=?1 AND edition=?2", params![system,edition], |row| Ok(RulesetKey { game_id: row.get(0)?, edition_id: row.get(1)? })).optional()?)
}

pub(crate) fn source_identity(
    db: &Connection,
    system: &str,
    edition: &str,
    stable_edition: Option<&str>,
) -> Result<Option<RulesetKey>, AppError> {
    let legacy = alias(db, system, edition)?;
    if let Some(edition_id) = stable_edition {
        let label: String = db
            .query_row(
                "SELECT label FROM registry_editions WHERE game_id=?1 AND edition_id=?2",
                params![system, edition_id],
                |row| row.get(0),
            )
            .optional()?
            .ok_or(AppError::Invalid(
                "Register this exact game/edition before importing a version-2 source.",
            ))?;
        let key = RulesetKey {
            game_id: system.into(),
            edition_id: edition_id.into(),
        };
        if legacy.as_ref().is_some_and(|target| target != &key)
            || (label != edition && legacy.as_ref() != Some(&key))
        {
            return Err(AppError::Invalid(
                "Source edition label/alias disagrees with the supplied stable game/edition identity.",
            ));
        }
        return Ok(Some(key));
    }
    // A version-1 bundle names its game only by a slug; it stays unresolved until an exact
    // alias attaches it. No game list lives in code.
    if legacy.is_some() || valid_id(system) {
        return Ok(legacy);
    }
    Err(AppError::Invalid(
        "Unknown legacy source identity: register an exact alias or use a version-2 bundle with registered IDs.",
    ))
}

fn backfill(
    db: &Connection,
    game: &str,
    old_edition: &str,
    edition_id: &str,
) -> Result<(), AppError> {
    db.execute("INSERT INTO character_rulesets SELECT c.id,?1,?3 FROM characters c WHERE c.system_id=?1 AND c.edition=?2 AND NOT EXISTS (SELECT 1 FROM character_rulesets r WHERE r.character_id=c.id)", params![game,old_edition,edition_id])?;
    db.execute("INSERT INTO source_rulesets SELECT s.id,?1,?3 FROM sources s WHERE s.system_id=?1 AND s.edition=?2 AND NOT EXISTS (SELECT 1 FROM source_rulesets r WHERE r.source_id=s.id)", params![game,old_edition,edition_id])?;
    Ok(())
}

fn validate_ruleset(input: &RegisterRuleset) -> Result<(), AppError> {
    if valid_id(&input.game_id)
        && valid_id(&input.edition_id)
        && valid_label(&input.game_label)
        && valid_label(&input.edition_label)
    {
        Ok(())
    } else {
        Err(invalid())
    }
}

fn validate_alias(input: &RegisterAlias) -> Result<(), AppError> {
    if valid_id(&input.game_id) && valid_id(&input.edition_id) && valid_label(&input.legacy_edition)
    {
        Ok(())
    } else {
        Err(invalid())
    }
}

/// Shared registration transition for the CLI and Owner browser, inside the caller's
/// transaction. Returns the appended history event, or `None` for an exact repeat.
pub(crate) fn register_in(
    db: &Connection,
    input: &RegisterRuleset,
) -> Result<Option<i64>, AppError> {
    validate_ruleset(input)?;
    if alias(db, &input.game_id, &input.edition_label)?
        .is_some_and(|target| target.edition_id != input.edition_id)
    {
        return Err(invalid());
    }
    let game: Option<String> = db
        .query_row(
            "SELECT label FROM registry_games WHERE game_id=?1",
            [&input.game_id],
            |row| row.get(0),
        )
        .optional()?;
    match game {
        Some(label) if label != input.game_label => return Err(invalid()),
        Some(_) => {}
        None => {
            db.execute(
                "INSERT INTO registry_games VALUES (?1,?2)",
                params![input.game_id, input.game_label],
            )?;
        }
    }
    let edition: Option<(String, String)> = db
        .query_row(
            "SELECT edition_id,label FROM registry_editions WHERE game_id=?1 AND (edition_id=?2 OR label=?3)",
            params![input.game_id, input.edition_id, input.edition_label],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    match edition {
        Some((id, label)) if id != input.edition_id || label != input.edition_label => {
            Err(invalid())
        }
        Some(_) => Ok(None),
        None => {
            db.execute(
                "INSERT INTO registry_editions (game_id,edition_id,label) VALUES (?1,?2,?3)",
                params![input.game_id, input.edition_id, input.edition_label],
            )?;
            db.execute(
                "INSERT INTO registry_events(kind,game_id,edition_id,game_label,label) VALUES('ruleset',?1,?2,?3,?4)",
                params![
                    input.game_id,
                    input.edition_id,
                    input.game_label,
                    input.edition_label
                ],
            )?;
            Ok(Some(db.last_insert_rowid()))
        }
    }
}

/// Shared exact-alias transition inside the caller's transaction. Returns the appended
/// history event, or `None` when the identical alias already existed.
pub(crate) fn alias_in(db: &Connection, input: &RegisterAlias) -> Result<Option<i64>, AppError> {
    validate_alias(input)?;
    let known: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM registry_editions WHERE game_id=?1 AND edition_id=?2)",
        params![input.game_id, input.edition_id],
        |row| row.get(0),
    )?;
    if !known {
        return Err(invalid());
    }
    let label_target: Option<String> = db
        .query_row(
            "SELECT edition_id FROM registry_editions WHERE game_id=?1 AND label=?2",
            params![input.game_id, input.legacy_edition],
            |row| row.get(0),
        )
        .optional()?;
    if label_target.is_some_and(|edition| edition != input.edition_id) {
        return Err(invalid());
    }
    let desired = RulesetKey {
        game_id: input.game_id.clone(),
        edition_id: input.edition_id.clone(),
    };
    let event = match alias(db, &input.game_id, &input.legacy_edition)? {
        Some(existing) if existing != desired => return Err(invalid()),
        Some(_) => None,
        None => {
            db.execute(
                "INSERT INTO legacy_ruleset_aliases VALUES (?1,?2,?1,?3)",
                params![input.game_id, input.legacy_edition, input.edition_id],
            )?;
            db.execute(
                "INSERT INTO registry_events(kind,game_id,edition_id,label) VALUES('alias',?1,?2,?3)",
                params![input.game_id, input.edition_id, input.legacy_edition],
            )?;
            Some(db.last_insert_rowid())
        }
    };
    backfill(db, &input.game_id, &input.legacy_edition, &input.edition_id)?;
    Ok(event)
}

fn read_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<RulesetEntry> {
    let key = RulesetKey {
        game_id: read_text(row, 0)?,
        edition_id: read_text(row, 2)?,
    };
    Ok(RulesetEntry {
        game_id: key.game_id,
        game_label: read_text(row, 1)?,
        edition_id: key.edition_id,
        edition_label: read_text(row, 3)?,
        generic_sheet_available: false,
    })
}

/// One bounded page ordered by the composite primary key, after an exclusive cursor.
pub(crate) fn rulesets_page(
    db: &Connection,
    after: Option<&RulesetKey>,
) -> Result<Listing<RulesetEntry, RulesetKey>, AppError> {
    if after.is_some_and(|key| !valid_id(&key.game_id) || !valid_id(&key.edition_id)) {
        return Err(AppError::Invalid(
            "Ruleset cursors need both parts as registered lowercase IDs.",
        ));
    }
    let sql = format!(
        "SELECT {},{},{},{} FROM registry_editions e JOIN registry_games g ON g.game_id=e.game_id WHERE ?1 IS NULL OR (e.game_id,e.edition_id)>(?1,?2) ORDER BY e.game_id,e.edition_id LIMIT 51",
        text("e.game_id", 64),
        text("g.label", 400),
        text("e.edition_id", 64),
        text("e.label", 400)
    );
    let mut query = db.prepare(&sql)?;
    let rows = query
        .query_map(
            params![
                after.map(|key| key.game_id.as_str()),
                after.map(|key| key.edition_id.as_str())
            ],
            read_entry,
        )?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Listing::new(rows, |row| RulesetKey {
        game_id: row.game_id.clone(),
        edition_id: row.edition_id.clone(),
    }))
}

impl Store {
    /// Register immutable game/edition identities; exact repeated registration is idempotent.
    ///
    /// # Errors
    /// Rejects invalid IDs/labels, identity/alias collisions or storage failures.
    pub async fn register_ruleset(&self, input: RegisterRuleset) -> Result<(), AppError> {
        validate_ruleset(&input)?;
        self.run(move |db| {
            let tx = db.transaction()?;
            register_in(&tx, &input)?;
            tx.commit()?;
            Ok(())
        })
        .await
    }

    /// Add an exact same-game legacy alias and attach matching unresolved records atomically.
    ///
    /// # Errors
    /// Rejects collisions, unknown targets or invalid labels/IDs.
    pub async fn register_ruleset_alias(&self, input: RegisterAlias) -> Result<(), AppError> {
        validate_alias(&input)?;
        self.run(move |db| {
            let tx = db.transaction()?;
            alias_in(&tx, &input)?;
            tx.commit()?;
            Ok(())
        })
        .await
    }

    /// List registered rulesets deterministically, without claiming generic sheet support.
    ///
    /// # Errors
    /// Returns storage/worker failures.
    pub async fn rulesets(&self) -> Result<Vec<RulesetEntry>, AppError> {
        self.run(|db| {
            let mut query = db.prepare("SELECT e.game_id,g.label,e.edition_id,e.label FROM registry_editions e JOIN registry_games g ON g.game_id=e.game_id ORDER BY e.game_id,e.edition_id")?;
            let entries = query.query_map([], read_entry)?;
            Ok(entries.collect::<Result<Vec<_>, _>>()?)
        }).await
    }
}
