//! The per-page findings value and the schema the request asks for it in. The schema is the
//! findings of `schemas/suggestion-report.v2.json` as `tools/review_extraction.py` sends them
//! (no `action`: the importer chooses it, never the model), restated in strict-compatible form:
//! every property required, nullable types spelled as type arrays, and no keywords the strict
//! modes do not support. Length and count bounds are not sent; they stay in `validate_page`
//! (docs/api-reviewer-plan.md step 6).
use super::ReviewError;
use rig_core::schemars::Schema;
use serde::Deserialize;
use serde_json::json;

/// The findings of one reviewed page.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "returned by the send path of docs/api-reviewer-plan.md step 4"
    )
)]
pub struct Findings {
    pub summary: String,
    pub needs_visual_review: bool,
    pub issues: Vec<Issue>,
}

/// One finding: an existing block id or null for a missing region.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "a field of the send path of docs/api-reviewer-plan.md step 4's Findings"
    )
)]
pub struct Issue {
    pub block_id: Option<String>,
    pub kind: String,
    pub description: String,
    pub suggested_correction: Option<String>,
}

/// The schema every provider's request carries as its structured output. rig sanitizes it for
/// strict mode on the wire (all properties required, `additionalProperties: false` injected,
/// `$ref` siblings stripped), so it is written the way the sanitizer leaves it.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "called by the send path of docs/api-reviewer-plan.md step 4"
    )
)]
pub(crate) fn schema() -> Schema {
    Schema::try_from(schema_value()).expect("the findings schema is a valid JSON schema")
}

/// The schema as it is declared, before rig's sanitizer touches it.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "called by the send path of docs/api-reviewer-plan.md step 4 (through `schema`)"
    )
)]
pub(crate) fn schema_value() -> serde_json::Value {
    json!({
        "title": "Findings",
        "type": "object",
        "additionalProperties": false,
        "required": ["summary", "issues", "needs_visual_review"],
        "properties": {
            "summary": { "type": "string" },
            "needs_visual_review": { "type": "boolean" },
            "issues": {
                "type": "array",
                "items": { "$ref": "#/$defs/issue" }
            }
        },
        "$defs": {
            "issue": {
                "type": "object",
                "additionalProperties": false,
                "required": ["block_id", "kind", "description", "suggested_correction"],
                "properties": {
                    "block_id": { "type": ["string", "null"] },
                    "kind": { "type": "string" },
                    "description": { "type": "string" },
                    "suggested_correction": { "type": ["string", "null"] }
                }
            }
        }
    })
}

/// Parses the reply text as findings. A refusal or a non-JSON reply fails the page the same
/// fixed way: rig folds a refusal into the answer text, so it cannot be told apart here and is
/// never shown.
///
/// # Errors
///
/// The reply is not the findings JSON.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "called by the send path of docs/api-reviewer-plan.md step 4"
    )
)]
pub(crate) fn parse(text: &str) -> Result<Findings, ReviewError> {
    serde_json::from_str(text).map_err(|_| ReviewError::InvalidReply)
}
