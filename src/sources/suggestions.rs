//! Suggestions are immutable, bundle-bound advice, separate from review decisions.
use crate::{AppError, Store};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SuggestionUnit {
    pub unit: i64,
    pub revision: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum SuggestionTarget {
    Unit { target: SuggestionUnit },
    Units { targets: Vec<SuggestionUnit> },
    Region { page: i64, polygon: [[f64; 2]; 4] },
}

#[derive(Clone, Debug)]
pub struct SourceSuggestion {
    pub source: i64,
    pub bundle_hash: String,
    pub origin: String,
    pub kind: String,
    pub target: SuggestionTarget,
    pub suggested_text: Option<String>,
    pub rationale: String,
}

fn validate(db: &Connection, suggestion: &SourceSuggestion) -> Result<(), AppError> {
    super::review_identity(&suggestion.origin, &suggestion.rationale)?;
    let text_present = suggestion
        .suggested_text
        .as_ref()
        .is_some_and(|text| !text.trim().is_empty());
    let shape_ok = match (suggestion.kind.as_str(), &suggestion.target) {
        ("correction", SuggestionTarget::Unit { .. })
        | ("missing_text", SuggestionTarget::Region { .. })
        | ("merge", SuggestionTarget::Units { .. }) => text_present,
        ("reject", SuggestionTarget::Unit { .. }) => suggestion.suggested_text.is_none(),
        _ => false,
    };
    if !shape_ok {
        return Err(AppError::Invalid(
            "A suggestion is a correction (one unit, text), merge (2+ units on one page, text), missing_text (region, text) or reject (one unit, no text).",
        ));
    }
    if suggestion.bundle_hash.len() != 64
        || !suggestion
            .bundle_hash
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        || suggestion
            .suggested_text
            .as_ref()
            .is_some_and(|text| text.chars().count() > 20000 || text.contains('\0'))
    {
        return Err(AppError::Invalid(
            "Invalid suggestion hash or text (20,000 characters, no NUL).",
        ));
    }
    let matches: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sources WHERE id=?1 AND bundle_hash=?2)",
        params![suggestion.source, suggestion.bundle_hash],
        |r| r.get(0),
    )?;
    if !matches {
        return Err(AppError::Invalid(
            "Suggestion bundle does not match its source.",
        ));
    }
    match &suggestion.target {
        SuggestionTarget::Unit { target } => {
            validate_units(db, suggestion.source, std::slice::from_ref(target))?;
        }
        SuggestionTarget::Units { targets } => {
            if targets.len() < 2 {
                return Err(AppError::Invalid(
                    "A multi-unit suggestion needs at least two units.",
                ));
            }
            validate_units(db, suggestion.source, targets)?;
            let pages: i64 = db.query_row(
                "SELECT count(DISTINCT page_id) FROM source_units WHERE id IN (SELECT value FROM json_each(?1))",
                [serde_json::to_string(&targets.iter().map(|t| t.unit).collect::<Vec<_>>())?],
                |r| r.get(0),
            )?;
            if pages != 1 {
                return Err(AppError::Invalid("A merge suggestion stays on one page."));
            }
        }
        SuggestionTarget::Region { page, polygon } => {
            let belongs: bool = db.query_row(
                "SELECT EXISTS(SELECT 1 FROM source_pages WHERE id=?1 AND source_id=?2)",
                params![page, suggestion.source],
                |r| r.get(0),
            )?;
            if !belongs {
                return Err(AppError::Invalid(
                    "Suggestion region belongs to another source.",
                ));
            }
            super::documents::validate_polygon(db, *page, polygon)?;
        }
    }
    Ok(())
}

fn validate_units(
    db: &Connection,
    source: i64,
    targets: &[SuggestionUnit],
) -> Result<(), AppError> {
    if targets.is_empty() || targets.len() > 1024 {
        return Err(AppError::Invalid("Choose 1–1,024 suggestion units."));
    }
    let mut seen = std::collections::BTreeSet::new();
    for target in targets {
        if target.revision <= 0 || !seen.insert(target.unit) {
            return Err(AppError::Invalid(
                "Suggestion targets need distinct units and positive revisions.",
            ));
        }
        let revision: Option<i64>=db.query_row("SELECT u.revision FROM source_units u JOIN source_pages p ON p.id=u.page_id WHERE u.id=?1 AND p.source_id=?2",params![target.unit,source],|r|r.get(0)).optional()?;
        if revision.is_none_or(|revision| target.revision > revision) {
            return Err(AppError::Invalid(
                "Suggestion unit or revision does not belong to this source.",
            ));
        }
    }
    Ok(())
}

impl Store {
    /// Store advice for an exact bundle, without changing evidence or completeness.
    /// Older target revisions are retained so a consumer can show the advice as stale.
    ///
    /// # Errors
    /// Invalid bounds, hash, target identities/geometry/revisions or database failures.
    pub async fn insert_source_suggestion(
        &self,
        suggestion: SourceSuggestion,
    ) -> Result<i64, AppError> {
        self.run(move |db| {
            let tx=db.transaction()?;
            validate(&tx,&suggestion)?;
            let targets=serde_json::to_string(&suggestion.target)?;
            if targets.len()>65536 {return Err(AppError::Invalid("Suggestion targets exceed 64 KiB."));}
            tx.execute("INSERT INTO source_suggestions(source_id,bundle_hash,origin,kind,targets,suggested_text,rationale) VALUES(?1,?2,?3,?4,?5,?6,?7)",params![suggestion.source,suggestion.bundle_hash,suggestion.origin,suggestion.kind,targets,suggestion.suggested_text,suggestion.rationale])?;
            let id=tx.last_insert_rowid(); tx.commit()?; Ok(id)
        }).await
    }

    /// Record the first dismissal independently of unit review state.
    ///
    /// # Errors
    /// Invalid actor, unknown/already dismissed suggestion or database failures.
    pub async fn dismiss_source_suggestion(
        &self,
        suggestion: i64,
        actor: String,
    ) -> Result<(), AppError> {
        super::review_identity(&actor, "Dismiss suggestion")?;
        self.run(move |db| {
            let tx=db.transaction()?;
            let dismissed: Option<String>=tx.query_row("SELECT dismissed_by FROM source_suggestions WHERE id=?1",[suggestion],|r|r.get(0)).optional()?.ok_or(AppError::NotFound)?;
            if dismissed.is_some() {return Err(AppError::Conflict);}
            tx.execute("UPDATE source_suggestions SET dismissed_by=?1,dismissed_at=CURRENT_TIMESTAMP WHERE id=?2",params![actor,suggestion])?;
            tx.commit()?;Ok(())
        }).await
    }
}
