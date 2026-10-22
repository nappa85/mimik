pub mod accounts;
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "step 2c-1 in progress: wired to the routes later in this branch"
    )
)]
mod assistant;
pub mod auth;
pub mod campaigns;
pub mod catalogues;
pub mod definitions;
mod form_text;
pub mod games;
mod local_file;
pub mod mork_advice;
pub mod mork_borg;
mod mork_form;
pub mod network;
mod presentation;
pub mod registry;
mod registry_admin;
pub mod sources;
mod store;
mod web;

pub use store::{SCHEMA_VERSION, Store};
pub use web::app;

use serde::Deserialize;

/// Family identifiers only: these do not claim implemented edition rules.
pub struct System {
    pub id: &'static str,
    pub name: &'static str,
}

pub const SYSTEMS: &[System] = &[
    System {
        id: "dnd",
        name: "D&D",
    },
    System {
        id: "pathfinder",
        name: "Pathfinder",
    },
    System {
        id: "vampire",
        name: "Vampire",
    },
    System {
        id: "mork-borg",
        name: "MÖRK BORG",
    },
];

#[derive(Clone, Default, Deserialize)]
pub struct CharacterInput {
    pub name: String,
    pub system_id: String,
    pub edition: String,
    #[serde(deserialize_with = "crate::form_text::lf")]
    pub notes: String,
    #[serde(default)]
    pub revision: i64,
}

impl CharacterInput {
    fn normalize(mut self) -> Self {
        self.name = self.name.trim().into();
        self.edition = self.edition.trim().into();
        self
    }

    fn validate(&self) -> Result<(), AppError> {
        if self.name.is_empty() || self.name.chars().count() > 100 {
            return Err(AppError::Invalid("Name must contain 1–100 characters."));
        }
        if !SYSTEMS.iter().any(|s| s.id == self.system_id) {
            return Err(AppError::Invalid("Select a known game family."));
        }
        if self.edition.is_empty() || self.edition.chars().count() > 100 {
            return Err(AppError::Invalid(
                "Specify the exact game title/edition (1–100 characters).",
            ));
        }
        if self.notes.chars().count() > 20_000 {
            return Err(AppError::Invalid(
                "Notes must not exceed 20,000 characters.",
            ));
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct Character {
    pub id: i64,
    pub input: CharacterInput,
    pub ruleset: Option<registry::RulesetKey>,
}

impl Character {
    #[must_use]
    pub fn has_mork_borg_adapter(&self) -> bool {
        self.ruleset
            .as_ref()
            .and_then(games::adapter_for_ruleset)
            .is_some_and(|adapter| adapter.id == "mork-borg-it-2025")
    }

    #[must_use]
    pub fn system_name(&self) -> &str {
        SYSTEMS
            .iter()
            .find(|s| s.id == self.input.system_id)
            .map_or("Unknown system", |s| s.name)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("Auth storage worker is busy.")]
    AuthStorageBusy,
    #[error("Account password worker is busy.")]
    PasswordBusy,
    #[error("{0}")]
    Invalid(&'static str),
    #[error("Character not found.")]
    NotFound,
    #[error("This character was changed in another tab. Copy your edits, reload, and try again.")]
    Conflict,
    #[error("{0}")]
    Database(#[from] rusqlite::Error),
    #[error("Storage worker failed.")]
    Worker,
    #[error("Template rendering failed.")]
    Template,
    #[error("Sheet data could not be read or encoded.")]
    SheetData(#[from] serde_json::Error),
    #[error("Local source file could not be read.")]
    SourceIo(#[from] std::io::Error),
}
mod pagination;
