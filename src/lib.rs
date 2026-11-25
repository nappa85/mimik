pub mod accounts;
pub mod assistant;
pub mod auth;
pub mod campaigns;
pub mod catalogues;
pub mod definitions;
mod form_text;
mod hub;
mod local_file;
pub mod network;
mod presentation;
pub mod registry;
mod registry_admin;
pub(crate) mod sandbox;
pub mod sources;
mod store;
mod streams;
mod text_fold;
mod web;

pub use store::{SCHEMA_VERSION, Store};
pub use web::app;

use serde::Deserialize;

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
        if !registry::valid_id(&self.system_id) {
            return Err(AppError::Invalid(
                "The game identifier must be a lowercase slug of at most 64 characters.",
            ));
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

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("Auth storage worker is busy.")]
    AuthStorageBusy,
    #[error("Account password worker is busy.")]
    PasswordBusy,
    #[error("{0}")]
    Invalid(&'static str),
    #[error("Suggestion report page {page}, issue {index}: {reason}.")]
    SuggestionReportIssue {
        page: i64,
        index: usize,
        reason: &'static str,
    },
    #[error("Suggestion report schema/parse error at line {line}, column {column}.")]
    SuggestionReportParse { line: usize, column: usize },
    #[error("Source not found.")]
    SourceNotFound,
    /// The stored PDF of a source is missing, replaced or does not match its hash.
    #[error("The stored PDF of this source is missing or damaged; import the source again.")]
    PdfCorrupt,
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
