//! Implemented adapters are distinct from planned families and imported books.
use crate::{AppError, Character, CharacterInput, Store, mork_borg::EDITION};
use rusqlite::params;
use serde::Deserialize;

pub struct Adapter {
    pub id: &'static str,
    pub system: &'static str,
    pub edition: &'static str,
    pub edition_id: &'static str,
    pub label: &'static str,
}

pub const ADAPTERS: &[Adapter] = &[Adapter {
    id: "mork-borg-it-2025",
    system: "mork-borg",
    edition: EDITION,
    edition_id: "it-2025",
    label: "MÖRK BORG — Italian 2025 core",
}];

#[must_use]
pub fn adapter(id: &str) -> Option<&'static Adapter> {
    ADAPTERS.iter().find(|adapter| adapter.id == id)
}

#[must_use]
pub fn adapter_for_ruleset(key: &crate::registry::RulesetKey) -> Option<&'static Adapter> {
    ADAPTERS
        .iter()
        .find(|adapter| adapter.system == key.game_id && adapter.edition_id == key.edition_id)
}

pub struct CreationCatalog {
    pub options: Vec<&'static Adapter>,
    pub source_count: i64,
}

pub(crate) fn native_available(
    db: &rusqlite::Connection,
    key: &crate::registry::RulesetKey,
) -> Result<bool, AppError> {
    if adapter_for_ruleset(key).is_none() {
        return Ok(false);
    }
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM source_rulesets WHERE game_id=?1 AND edition_id=?2)",
        params![key.game_id, key.edition_id],
        |row| row.get(0),
    )?)
}

#[derive(Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateRequest {
    #[serde(default)]
    pub adapter_id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default, deserialize_with = "crate::form_text::lf")]
    pub notes: String,
}

impl Store {
    /// Offer only implemented adapters with an exactly matching registered source.
    ///
    /// # Errors
    /// Returns storage/worker errors.
    pub async fn creation_catalog(&self) -> Result<CreationCatalog, AppError> {
        self.run(|db| {
            let source_count =
                db.query_row("SELECT count(*) FROM sources", [], |row| row.get(0))?;
            let mut options = Vec::new();
            for adapter in ADAPTERS {
                if native_available(
                    db,
                    &crate::registry::RulesetKey {
                        game_id: adapter.system.into(),
                        edition_id: adapter.edition_id.into(),
                    },
                )? {
                    options.push(adapter);
                }
            }
            Ok(CreationCatalog {
                options,
                source_count,
            })
        })
        .await
    }

    /// Create manual sheet entry for a loaded, implemented adapter atomically.
    ///
    /// # Errors
    /// Rejects unknown/unloaded adapters, invalid identity or storage failures.
    pub async fn create_available_character(
        &self,
        request: CreateRequest,
    ) -> Result<Character, AppError> {
        let selected = adapter(&request.adapter_id).ok_or(AppError::Invalid(
            "Choose a loaded, implemented game from the creation list.",
        ))?;
        let input = CharacterInput {
            name: request.name,
            notes: request.notes,
            system_id: selected.system.into(),
            edition: selected.edition.into(),
            revision: 0,
        }
        .normalize();
        input.validate()?;
        self.run(move |db| {
            let tx = db.transaction()?;
            if !native_available(
                &tx,
                &crate::registry::RulesetKey {
                    game_id: selected.system.into(),
                    edition_id: selected.edition_id.into(),
                },
            )? {
                return Err(AppError::Invalid(
                    "Register this game's exact source edition before creating a character.",
                ));
            }
            let character = crate::store::insert_character(&tx, &input)?;
            tx.commit()?;
            Ok(character)
        })
        .await
    }
}
