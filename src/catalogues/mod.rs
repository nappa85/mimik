//! Read-only game catalogues (plan `docs/catalogue-plan.md`). Step 1: the strict envelope,
//! its canonical identity and revisioned drafts. Step 2: comparison, publication of immutable
//! sealed versions with projections and a name index, integrity check and index repair.
//!
//! Redaction (plan D6): no error, diagnostic or summary carries entry names, keys, property
//! ids or values, owner text or unit text. Diagnostics are static codes with index-only paths.
mod admission;
pub(crate) mod decode;
mod publication;
mod store;

pub use decode::MAX_BYTES;

/// Every catalogue object, children first, for test fixtures that rewind to older schemas.
#[cfg(test)]
pub(crate) const DROP_SCHEMA: &str = "DROP TABLE catalogue_names_fts; DROP TABLE catalogue_seals; DROP TABLE catalogue_entry_units; DROP TABLE catalogue_entries; DROP TABLE catalogue_entry_types; DROP TABLE catalogue_versions; DROP TABLE catalogues; DROP TABLE catalogue_drafts;";
pub use publication::{
    BadgeCounts, CatalogueComparison, CatalogueIndexRebuild, CatalogueVerification,
    CatalogueVersionSummary, ChangeCounts, IntegrityProblem, PublishRequest, PublishedCatalogue,
    RefusalCounts, UnitStates,
};

use crate::definitions::{Diagnostic, ValidationReport};
use serde::{Deserialize, Serialize, Serializer, ser::SerializeMap};
use std::collections::BTreeMap;

#[derive(Debug, thiserror::Error)]
pub enum CatalogueError {
    #[error("Catalogue admission busy; retry after the current job completes.")]
    Busy,
    #[error("Catalogue rejected: {0}")]
    Invalid(ValidationReport),
    #[error("Catalogue draft changed, or expected revision does not match. No changes saved.")]
    Conflict,
    #[error("Catalogue comparison ticket is stale; compare again. Nothing published.")]
    StaleComparison,
    #[error("Catalogue draft or version not found.")]
    NotFound,
    #[error("Catalogue worker failed.")]
    Worker,
    #[error("Embedded catalogue validation configuration is invalid.")]
    Configuration,
    #[error("Catalogue storage failed.")]
    Storage(#[source] crate::AppError),
}

impl From<crate::AppError> for CatalogueError {
    fn from(error: crate::AppError) -> Self {
        match error {
            crate::AppError::NotFound => Self::NotFound,
            // Local-file failures carry static messages only.
            crate::AppError::Invalid(message) => {
                Self::Invalid(report(vec![diagnostic("/", message)], false))
            }
            other => Self::Storage(other),
        }
    }
}

impl From<rusqlite::Error> for CatalogueError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Storage(error.into())
    }
}

fn diagnostic(path: &str, code: &str) -> Diagnostic {
    Diagnostic {
        path: path.into(),
        code: code.into(),
    }
}

fn report(diagnostics: Vec<Diagnostic>, truncated: bool) -> ValidationReport {
    ValidationReport {
        diagnostics,
        truncated,
    }
}

/// A one-diagnostic rejection. Callers pass static codes and index-only paths.
pub(crate) fn invalid(path: &str, code: &str) -> CatalogueError {
    CatalogueError::Invalid(report(vec![diagnostic(path, code)], false))
}

/// The closed catalogue envelope (`schemas/catalogue-v1.json`). Member order is the canonical
/// property order.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Catalogue {
    pub contract_version: u32,
    pub game_id: String,
    pub edition_id: String,
    pub catalogue_id: String,
    pub title: String,
    pub sources: Vec<CatalogueSource>,
    pub entry_types: Vec<EntryType>,
    pub entries: Vec<Entry>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogueSource {
    pub source_id: i64,
    pub pdf_sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EntryType {
    pub id: String,
    pub label: String,
    pub properties: Vec<PropertyDecl>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PropertyControl {
    Text,
    Integer,
    Boolean,
    #[serde(rename = "enum")]
    Enumeration,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PropertyDecl {
    pub id: String,
    pub label: String,
    pub control: PropertyControl,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<crate::definitions::Choice>>,
}

/// A scalar entry property value; the declared control decides which variant is legal.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum PropertyValue {
    Boolean(bool),
    Integer(i64),
    Text(String),
}

/// Entry properties, serialized in the entry type's declared order once admitted.
#[derive(Clone, Debug, Default)]
pub struct EntryProperties(pub Vec<(String, PropertyValue)>);

impl Serialize for EntryProperties {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (key, value) in &self.0 {
            map.serialize_entry(key, value)?;
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for EntryProperties {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // Duplicates were already refused by the decode profile.
        Ok(Self(
            BTreeMap::<String, PropertyValue>::deserialize(deserializer)?
                .into_iter()
                .collect(),
        ))
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub key: String,
    #[serde(rename = "type")]
    pub entry_type: String,
    pub name: String,
    pub properties: EntryProperties,
    /// Absent and `false` are the same state; the canonical form omits `false`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub retired: bool,
    pub description: Description,
}

/// Exactly one member is present (schema `oneOf`, then the domain check).
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Description {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub units: Option<Vec<crate::definitions::Region>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_text: Option<String>,
}

/// Metadata-only counts of an admitted catalogue; never names, keys or text.
#[derive(Clone, Copy, Debug, Default, Serialize, PartialEq, Eq)]
pub struct CatalogueCounts {
    pub sources: usize,
    pub entry_types: usize,
    pub entries: usize,
    pub retired: usize,
    pub owner_written: usize,
    pub unit_references: usize,
    pub distinct_units: usize,
}

/// What every catalogue command prints (plan D6): identity, revision, hash and counts.
#[derive(Debug, Serialize)]
pub struct CatalogueDraftSummary {
    pub id: i64,
    pub game_id: String,
    pub edition_id: String,
    pub catalogue_id: String,
    pub revision: i64,
    pub content_hash: String,
    pub status: &'static str,
    pub counts: CatalogueCounts,
}

#[cfg(test)]
mod admission_tests;
