//! Closed tool reports are untrusted advice, bound to the stored extraction.
use super::{SourceSuggestion, SuggestionTarget, SuggestionUnit};
use crate::{AppError, Store};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Report {
    schema_version: u32,
    state: String,
    source_sha256: String,
    #[serde(default, deserialize_with = "present")]
    bundle_sha256: Option<String>,
    #[serde(default, deserialize_with = "present")]
    artifact_sha256: Option<BTreeMap<String, String>>,
    #[serde(default, deserialize_with = "present")]
    tool: Option<String>,
    #[serde(default, deserialize_with = "present")]
    endpoint: Option<String>,
    #[serde(default, deserialize_with = "present")]
    requested_model: Option<String>,
    created_at: String,
    pages: Vec<ReportPage>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReportPage {
    pdf_page: i64,
    state: String,
    #[serde(default, deserialize_with = "present")]
    findings: Option<Findings>,
    #[serde(default, deserialize_with = "present")]
    error: Option<String>,
    #[serde(default, deserialize_with = "present")]
    render_sha256: Option<String>,
    #[serde(rename = "returned_model")]
    _returned_model: Option<String>,
    #[serde(rename = "usage")]
    _usage: Option<serde_json::Map<String, serde_json::Value>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Findings {
    #[serde(rename = "summary")]
    _summary: String,
    #[serde(rename = "needs_visual_review")]
    _needs_visual_review: bool,
    issues: Vec<Issue>,
}

// Value keeps required nullable fields required (Option alone also accepts missing fields).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Issue {
    block_id: serde_json::Value,
    #[serde(rename = "kind")]
    _kind: String,
    description: String,
    suggested_correction: serde_json::Value,
    #[serde(default, deserialize_with = "present")]
    action: Option<String>,
    #[serde(default, deserialize_with = "present")]
    merge_block_ids: Option<Vec<String>>,
    #[serde(default, deserialize_with = "present")]
    region: Option<[[f64; 2]; 4]>,
}

// Omission is supported for optional keys; an explicit null is not a typed value.
fn present<'de, D, T>(decoder: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(decoder).map(Some)
}

#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
pub struct SuggestionReportSummary {
    pub report_id: i64,
    pub counts: BTreeMap<String, usize>,
    pub failed_pages: usize,
    pub binding: String,
    pub repeat: bool,
}

fn invalid() -> AppError {
    AppError::Invalid("Invalid or inconsistent suggestion report.")
}
fn hash(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
fn valid_hash(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| matches!(b,b'0'..=b'9'|b'a'..=b'f'))
}
fn nullable_text(value: &serde_json::Value) -> Result<Option<String>, AppError> {
    match value {
        serde_json::Value::Null => Ok(None),
        serde_json::Value::String(text) => Ok(Some(text.clone())),
        _ => Err(invalid()),
    }
}

impl Report {
    fn binding(&self, bundle: &str, raw: &[u8]) -> Result<&'static str, AppError> {
        if self.bundle_sha256.as_deref() == Some(bundle) {
            Ok("bundle")
        } else if self.bundle_sha256.is_none()
            && self.schema_version == 1
            && self
                .artifact_sha256
                .as_ref()
                .and_then(|h| h.get("source.json"))
                == Some(&hash(raw))
        {
            Ok("extraction")
        } else {
            Err(AppError::Invalid(
                "Suggestion report binding is missing or does not match source.",
            ))
        }
    }

    fn origin(&self) -> Result<String, AppError> {
        let origin = match (
            self.state.as_str(),
            &self.tool,
            &self.requested_model,
            &self.endpoint,
        ) {
            ("tool_suggestions_not_verified", Some(tool), None, None)
                if !tool.trim().is_empty() =>
            {
                format!("tool:{tool}")
            }
            ("model_suggestions_not_verified", None, Some(model), Some(endpoint))
                if !model.trim().is_empty() && !endpoint.is_empty() =>
            {
                format!("reviewer:{model}")
            }
            _ => return Err(invalid()),
        };
        if !matches!(self.schema_version, 1 | 2)
            || origin.len() > 100
            || origin.chars().any(char::is_control)
            || self.created_at.is_empty()
            || self.created_at.len() > 64
            || self.created_at.chars().any(char::is_control)
            || !valid_hash(&self.source_sha256)
            || self.bundle_sha256.as_ref().is_some_and(|h| !valid_hash(h))
            || self.artifact_sha256.as_ref().is_some_and(|hashes| {
                hashes.iter().any(|(name, h)| {
                    name.is_empty() || name.contains(['/', '\\']) || !valid_hash(h)
                })
            })
        {
            return Err(invalid());
        }
        Ok(origin)
    }
}

// The raw tree was bounded and validated by source import. Keep its page ownership when
// resolving citations; a matching id elsewhere in the document is insufficient.
fn raw_ids(raw: &[u8]) -> Result<BTreeMap<i64, BTreeSet<String>>, AppError> {
    fn visit(node: &serde_json::Value, ids: &mut BTreeSet<String>) {
        if let Some(id) = node["id"].as_str() {
            ids.insert(id.into());
        }
        if let Some(children) = node["children"].as_array() {
            for child in children {
                visit(child, ids);
            }
        }
    }
    let document: serde_json::Value = serde_json::from_slice(raw)?;
    let mut pages = BTreeMap::new();
    for page in document["children"].as_array().ok_or_else(invalid)? {
        let index = page["id"]
            .as_str()
            .and_then(|id| id.strip_prefix("/page/"))
            .and_then(|id| id.split_once("/Page/"))
            .and_then(|(index, _)| index.parse::<i64>().ok())
            .ok_or_else(invalid)?;
        visit(page, pages.entry(index + 1).or_default());
    }
    Ok(pages)
}

fn unit(db: &Connection, page: i64, block: &str) -> Result<Option<SuggestionUnit>, AppError> {
    Ok(db
        .query_row(
            "SELECT id FROM source_units WHERE page_id=?1 AND block_id=?2 AND origin='extracted'",
            params![page, block],
            |r| {
                Ok(SuggestionUnit {
                    unit: r.get(0)?,
                    revision: 1,
                })
            },
        )
        .optional()?)
}

fn target(
    db: &Connection,
    page: i64,
    ids: &BTreeSet<String>,
    issue: &Issue,
    version: u32,
) -> Result<(String, SuggestionTarget, Option<String>), AppError> {
    let block = nullable_text(&issue.block_id)?;
    let text = nullable_text(&issue.suggested_correction)?;
    if block.as_ref().is_some_and(|id| !ids.contains(id)) {
        return Err(invalid());
    }
    let extracted = block
        .as_deref()
        .map(|id| unit(db, page, id))
        .transpose()?
        .flatten();
    let simple = extracted.map_or_else(
        || SuggestionTarget::Page {
            page,
            block_id: block.clone(),
        },
        |target| SuggestionTarget::Unit { target },
    );
    if version == 1 {
        if issue.action.is_some() || issue.merge_block_ids.is_some() || issue.region.is_some() {
            return Err(invalid());
        }
        let action = if matches!(simple, SuggestionTarget::Unit { .. })
            && text.as_ref().is_some_and(|text| !text.trim().is_empty())
        {
            "correction"
        } else {
            "note"
        };
        return Ok((action.into(), simple, text));
    }
    let action = issue.action.as_deref().ok_or_else(invalid)?;
    if (action != "merge" && issue.merge_block_ids.is_some())
        || (action != "missing_text" && issue.region.is_some())
    {
        return Err(invalid());
    }
    let target = match action {
        "correction" | "reject" | "note" => simple,
        "missing_text" if block.is_none() => SuggestionTarget::Region {
            page,
            polygon: issue.region.ok_or_else(invalid)?,
        },
        "merge" if block.is_none() => {
            let blocks = issue.merge_block_ids.as_ref().ok_or_else(invalid)?;
            if !(2..=1024).contains(&blocks.len()) {
                return Err(invalid());
            }
            let targets = blocks
                .iter()
                .map(|id| unit(db, page, id)?.ok_or_else(invalid))
                .collect::<Result<Vec<_>, _>>()?;
            // Nonzero bounding rectangle; polygons need not be individually convex in raw bundles.
            let mut xs = Vec::new();
            let mut ys = Vec::new();
            for target in &targets {
                let polygon: String = db.query_row(
                    "SELECT polygon FROM source_units WHERE id=?1",
                    [target.unit],
                    |r| r.get(0),
                )?;
                let points: [[f64; 2]; 4] = serde_json::from_str(&polygon)?;
                for [x, y] in points {
                    xs.push(x);
                    ys.push(y);
                }
            }
            if xs.iter().copied().reduce(f64::min) >= xs.iter().copied().reduce(f64::max)
                || ys.iter().copied().reduce(f64::min) >= ys.iter().copied().reduce(f64::max)
            {
                return Err(invalid());
            }
            SuggestionTarget::Units { targets }
        }
        _ => return Err(invalid()),
    };
    Ok((action.into(), target, text))
}

fn report_page<'a>(
    db: &Connection,
    source: i64,
    state: &str,
    page: &'a ReportPage,
    pages: &mut BTreeSet<i64>,
) -> Result<Option<(i64, &'a Findings)>, AppError> {
    if !pages.insert(page.pdf_page) {
        return Err(AppError::Invalid(
            "Suggestion report contains a duplicate page.",
        ));
    }
    let page_id = db
        .query_row(
            "SELECT id FROM source_pages WHERE source_id=?1 AND pdf_page=?2",
            params![source, page.pdf_page],
            |r| r.get(0),
        )
        .optional()?
        .ok_or(AppError::Invalid(
            "Suggestion report page is outside source.",
        ))?;
    if page.render_sha256.as_ref().is_some_and(|h| !valid_hash(h)) {
        return Err(invalid());
    }
    // Provider metadata is retained by the report file, never treated as authority.
    if page.state == "failed" {
        if page.error.is_none() || page.findings.is_some() {
            return Err(invalid());
        }
        return Ok(None);
    }
    if page.state != state || page.error.is_some() {
        return Err(invalid());
    }
    Ok(Some((page_id, page.findings.as_ref().ok_or_else(invalid)?)))
}

fn import(
    db: &mut Connection,
    source: i64,
    report: &Report,
    digest: &str,
) -> Result<SuggestionReportSummary, AppError> {
    let origin = report.origin()?;
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let (pdf,bundle,raw): (String,String,Option<Vec<u8>>)=tx.query_row("SELECT pdf_hash,bundle_hash,CASE WHEN length(raw_json)<=33554432 THEN raw_json END FROM sources WHERE id=?1",[source],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?.ok_or(AppError::SourceNotFound)?;
    let raw = raw.ok_or(AppError::Invalid(
        "Source extraction exceeds its byte limit.",
    ))?;
    if pdf != report.source_sha256 {
        return Err(AppError::Invalid(
            "Suggestion report PDF hash does not match source.",
        ));
    }
    let binding = report.binding(&bundle, &raw)?;
    let page_ids = raw_ids(&raw)?;
    let mut summary = SuggestionReportSummary {
        binding: binding.into(),
        ..Default::default()
    };
    for action in ["correction", "reject", "merge", "missing_text", "note"] {
        summary.counts.insert(action.into(), 0);
    }
    let existing: Option<i64> = tx
        .query_row(
            "SELECT id FROM source_suggestion_reports WHERE source_id=?1 AND report_sha256=?2",
            params![source, digest],
            |r| r.get(0),
        )
        .optional()?;
    let mut suggestions = Vec::new();
    let mut pages = BTreeSet::new();
    let mut issue_count = 0;
    for page in &report.pages {
        let Some((page_id, findings)) = report_page(&tx, source, &report.state, page, &mut pages)?
        else {
            summary.failed_pages += 1;
            continue;
        };
        issue_count += findings.issues.len();
        if issue_count > 5000 {
            return Err(AppError::Invalid("Suggestion report exceeds 5,000 issues."));
        }
        let ids = page_ids.get(&page.pdf_page).ok_or_else(invalid)?;
        let bounds = if findings
            .issues
            .iter()
            .any(|issue| issue.action.as_deref() == Some("missing_text"))
        {
            Some(super::documents::polygon_bounds(&tx, page_id)?)
        } else {
            None
        };
        for (index, issue) in findings.issues.iter().enumerate() {
            let issue_error = || AppError::SuggestionReportIssue {
                page: page.pdf_page,
                index: index + 1,
                reason: "invalid action, target, text or rationale",
            };
            let (kind, target, suggested_text) =
                target(&tx, page_id, ids, issue, report.schema_version).map_err(
                    |error| match error {
                        AppError::Invalid(_) => issue_error(),
                        other => other,
                    },
                )?;
            let suggestion = SourceSuggestion {
                source,
                bundle_hash: bundle.clone(),
                origin: origin.clone(),
                kind: kind.clone(),
                target,
                suggested_text,
                rationale: issue.description.clone(),
                report_id: None,
            };
            super::suggestions::validate_with_bounds(&tx, &suggestion, bounds).map_err(
                |error| match error {
                    AppError::Invalid(_) => issue_error(),
                    other => other,
                },
            )?;
            *summary.counts.get_mut(&kind).ok_or_else(issue_error)? += 1;
            suggestions.push(suggestion);
        }
    }
    if let Some(id) = existing {
        summary.report_id = id;
        summary.repeat = true;
        return Ok(summary);
    }
    tx.execute("INSERT INTO source_suggestion_reports(source_id,report_sha256,origin,schema_version,binding,created_at,correction,reject,merge,missing_text,note,failed_pages) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",params![source,digest,origin,report.schema_version,binding,report.created_at,summary.counts["correction"],summary.counts["reject"],summary.counts["merge"],summary.counts["missing_text"],summary.counts["note"],summary.failed_pages])?;
    summary.report_id = tx.last_insert_rowid();
    for mut suggestion in suggestions {
        suggestion.report_id = Some(summary.report_id);
        super::suggestions::insert_validated(&tx, &suggestion)?;
    }
    tx.commit()?;
    Ok(summary)
}

impl Store {
    /// Import a closed v1/v2 report as immutable, revision-1 advice in one transaction.
    ///
    /// # Errors
    /// Refuses nonregular/symlink/oversized files, invalid reports, bindings, targets or storage failures.
    pub async fn import_suggestion_report(
        &self,
        source: i64,
        path: &Path,
    ) -> Result<SuggestionReportSummary, AppError> {
        let path = path.to_owned();
        let (report, digest) = tokio::task::spawn_blocking(move || {
            let bytes = crate::local_file::read_path(&path, 4 * 1024 * 1024)?;
            let report: Report = serde_json::from_slice(&bytes).map_err(|error| {
                AppError::SuggestionReportParse {
                    line: error.line(),
                    column: error.column(),
                }
            })?;
            Ok::<_, AppError>((report, hash(&bytes)))
        })
        .await
        .map_err(|_| AppError::Worker)??;
        self.run(move |db| import(db, source, &report, &digest))
            .await
    }
}
