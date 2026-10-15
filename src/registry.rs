//! Registered identities are not approved sheet definitions or rule coverage.
use crate::{AppError, Store};
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
    pub native: bool,
    pub native_adapter: Option<&'static str>,
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
    if legacy.is_some() || crate::SYSTEMS.iter().any(|known| known.id == system) {
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

impl Store {
    /// Register immutable game/edition identities; exact repeated registration is idempotent.
    ///
    /// # Errors
    /// Rejects invalid IDs/labels, identity/alias collisions or storage failures.
    pub async fn register_ruleset(&self, input: RegisterRuleset) -> Result<(), AppError> {
        if !valid_id(&input.game_id)
            || !valid_id(&input.edition_id)
            || !valid_label(&input.game_label)
            || !valid_label(&input.edition_label)
        {
            return Err(invalid());
        }
        self.run(move |db| {
            let tx = db.transaction()?;
            if alias(&tx, &input.game_id, &input.edition_label)?.is_some_and(|target| target.edition_id != input.edition_id) { return Err(invalid()); }
            let game: Option<String> = tx.query_row("SELECT label FROM registry_games WHERE game_id=?1", [&input.game_id], |row| row.get(0)).optional()?;
            match game {
                Some(label) if label != input.game_label => return Err(invalid()),
                Some(_) => {},
                None => { tx.execute("INSERT INTO registry_games VALUES (?1,?2)", params![input.game_id,input.game_label])?; }
            }
            let edition: Option<(String,String)> = tx.query_row("SELECT edition_id,label FROM registry_editions WHERE game_id=?1 AND (edition_id=?2 OR label=?3)", params![input.game_id,input.edition_id,input.edition_label], |row| Ok((row.get(0)?,row.get(1)?))).optional()?;
            match edition {
                Some((id,label)) if id != input.edition_id || label != input.edition_label => return Err(invalid()),
                Some(_) => {},
                None => { tx.execute("INSERT INTO registry_editions (game_id,edition_id,label) VALUES (?1,?2,?3)", params![input.game_id,input.edition_id,input.edition_label])?; }
            }
            tx.commit()?;
            Ok(())
        }).await
    }

    /// Add an exact same-game legacy alias and attach matching unresolved records atomically.
    ///
    /// # Errors
    /// Rejects native shadowing, collisions, unknown targets or invalid labels/IDs.
    pub async fn register_ruleset_alias(&self, input: RegisterAlias) -> Result<(), AppError> {
        if !valid_id(&input.game_id)
            || !valid_id(&input.edition_id)
            || !valid_label(&input.legacy_edition)
        {
            return Err(invalid());
        }
        self.run(move |db| {
            let tx = db.transaction()?;
            let native: bool = tx
                .query_row(
                    "SELECT native FROM registry_editions WHERE game_id=?1 AND edition_id=?2",
                    params![input.game_id, input.edition_id],
                    |row| row.get(0),
                )
                .optional()?
                .ok_or_else(invalid)?;
            let label_target: Option<String> = tx
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
            match alias(&tx, &input.game_id, &input.legacy_edition)? {
                Some(existing) if existing != desired => return Err(invalid()),
                Some(_) => {}
                None if native => {
                    return Err(AppError::Invalid(
                        "The native ruleset accepts only its seeded exact legacy alias.",
                    ));
                }
                None => {
                    tx.execute(
                        "INSERT INTO legacy_ruleset_aliases VALUES (?1,?2,?1,?3)",
                        params![input.game_id, input.legacy_edition, input.edition_id],
                    )?;
                }
            }
            backfill(
                &tx,
                &input.game_id,
                &input.legacy_edition,
                &input.edition_id,
            )?;
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
            let mut query = db.prepare("SELECT e.game_id,g.label,e.edition_id,e.label,e.native FROM registry_editions e JOIN registry_games g ON g.game_id=e.game_id ORDER BY e.game_id,e.edition_id")?;
            let entries = query.query_map([], |row| {
                let key = RulesetKey { game_id: row.get(0)?, edition_id: row.get(2)? };
                let adapter = crate::games::adapter_for_ruleset(&key);
                Ok(RulesetEntry { game_id: key.game_id, game_label: row.get(1)?, edition_id: key.edition_id, edition_label: row.get(3)?, native: row.get(4)?, native_adapter: adapter.map(|adapter| adapter.id), generic_sheet_available: false })
            })?;
            Ok(entries.collect::<Result<Vec<_>, _>>()?)
        }).await
    }
}
