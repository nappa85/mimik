//! Strict declarative proposals. Draft storage never approves evidence or enables a sheet.
mod admission;
pub(crate) mod decode;
mod formula_admission;
// Public so `examples/formula_eval.rs` (development CLI) links and the library-only
// `formula::preview` is reachable without a caller (decided at M3, see the parent plan §16).
pub mod formula;
#[cfg(test)]
mod formula_admission_tests;
#[cfg(test)]
pub(crate) mod formula_pack_tests;
pub(crate) mod private_pin;
mod publication;
#[cfg(test)]
pub(crate) use publication::tests::fixture as source_fixture;
mod store;
pub use publication::{ApprovalRequest, Comparison, PublishedDefinition, WiringStatus};

use serde::{Deserialize, Serialize};
use std::{
    fmt,
    sync::{Arc, LazyLock},
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

pub const MAX_BYTES: usize = 256 * 1024;
static JOBS: LazyLock<Arc<Semaphore>> = LazyLock::new(|| Arc::new(Semaphore::new(1)));
#[cfg(test)]
pub(super) static TEST_RESOURCE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub(crate) fn admit() -> Result<OwnedSemaphorePermit, DefinitionError> {
    JOBS.clone()
        .try_acquire_owned()
        .map_err(|_| DefinitionError::Busy)
}

#[derive(Debug, thiserror::Error)]
pub enum DefinitionError {
    #[error("Definition admission busy; retry after the current job completes.")]
    Busy,
    #[error("Definition rejected: {0}")]
    Invalid(ValidationReport),
    #[error("Definition draft changed, or expected revision does not match. No changes saved.")]
    Conflict,
    #[error("Definition draft not found.")]
    NotFound,
    #[error("Definition worker failed.")]
    Worker,
    #[error("Embedded definition validation configuration is invalid.")]
    Configuration,
    #[error(transparent)]
    Storage(#[from] crate::AppError),
}

#[derive(Debug, Serialize)]
pub struct Diagnostic {
    pub path: String,
    pub code: String,
}

#[derive(Debug, Serialize)]
pub struct ValidationReport {
    pub diagnostics: Vec<Diagnostic>,
    pub truncated: bool,
}

fn safe_text(value: &str) -> String {
    value
        .chars()
        .flat_map(char::escape_default)
        .take(256)
        .collect()
}

impl ValidationReport {
    pub(crate) fn one(path: &str, code: &str) -> DefinitionError {
        DefinitionError::Invalid(Self {
            diagnostics: vec![Diagnostic {
                path: safe_text(path),
                code: safe_text(code),
            }],
            truncated: false,
        })
    }
}

impl fmt::Display for ValidationReport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for diagnostic in &self.diagnostics {
            write!(formatter, "{}: {}; ", diagnostic.path, diagnostic.code)?;
        }
        if self.truncated {
            formatter.write_str("additional diagnostics omitted")?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Definition {
    pub contract_version: u32,
    pub game_id: String,
    pub edition_id: String,
    pub definition_id: String,
    pub title: String,
    pub sources: Vec<SourceRef>,
    pub fields: Vec<Field>,
    pub groups: Vec<Group>,
    /// CEL profile marker (`1` = "cel-mimik-1"); present exactly when formulas, lists or
    /// constraints are. New members follow the existing ones and are omitted when absent, so
    /// formula-free canonical bytes and hashes are unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub formula_profile: Option<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub constraints: Vec<Constraint>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SourceRef {
    pub source_id: i64,
    pub pdf_sha256: String,
    pub roles: Vec<SourceRole>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum SourceRole {
    Core,
    Sheet,
    Supplement,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Control {
    Text,
    Multiline,
    Integer,
    Boolean,
    #[serde(rename = "enum")]
    Enumeration,
    /// Records with declared properties and a host-managed `id` (formula profile only).
    List,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Field {
    pub id: String,
    pub label: String,
    pub control: Control,
    pub nullable: bool,
    #[serde(default)]
    pub help: Option<String>,
    #[serde(default)]
    pub options: Option<Vec<Choice>>,
    pub mapping: Mapping,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub items: Option<Items>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub formula: Option<Formula>,
}

/// Declared properties of every record in a list field.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Items {
    pub properties: Vec<ItemProperty>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ItemProperty {
    pub id: String,
    pub label: String,
    pub control: Control,
    pub optional: bool,
    #[serde(default)]
    pub options: Option<Vec<Choice>>,
}

/// A derived value: CEL source plus rule citations and source evidence.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Formula {
    pub expr: String,
    pub rule_ids: Vec<String>,
    pub evidence: Mapping,
}

/// An advisory constraint; violations are reported, never enforced.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Constraint {
    pub id: String,
    pub expr: String,
    pub rule_ids: Vec<String>,
    pub message: String,
    pub evidence: Mapping,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Choice {
    pub id: String,
    pub label: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum Mapping {
    Mapped { regions: Vec<Region> },
    Unresolved { reason: String },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
pub struct Region {
    pub source_id: i64,
    pub page_id: i64,
    pub unit_id: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Group {
    pub id: String,
    pub label: String,
    pub fields: Vec<String>,
    pub column_hint: ColumnHint,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ColumnHint {
    Auto,
    Main,
    Sidebar,
}

#[derive(Debug, Serialize)]
pub struct DefinitionDraft {
    pub id: i64,
    pub revision: i64,
    pub content_hash: String,
    pub status: &'static str,
    pub definition: Definition,
}

#[derive(Debug, Serialize)]
pub struct DraftSummary {
    pub id: i64,
    pub definition_id: String,
    pub revision: i64,
    pub content_hash: String,
    pub status: &'static str,
}
