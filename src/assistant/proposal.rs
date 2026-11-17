//! A validated option with its bound read/write set (assisted sheet plan §1.4, "Proposal
//! freshness", and §1.5, "Bundles", step 3b): one to four changes to distinct integer, enum
//! or boolean targets, with a content hash over every change and every bound value.
use super::jobs::{JobId, Origin};
use super::output::Candidate;
use crate::campaigns::NoteValue;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

/// The most changes of one proposal (and so the most notes of one bundle).
pub(crate) const MAX_CHANGES: usize = 4;

/// What a job is for: the origin check of its apply depends on it (plan §1.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    /// A scribble's reading: bound to the target's current `unresolved` note.
    Interpretation,
    /// A hint after a saved decrease (step 3c): bound to the decrease note, which must still
    /// be current on its field, the field still holding its `after`. Its producer lands in 3c.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "built by step 3c's reminder producer")
    )]
    Reminder,
}

impl Kind {
    fn tag(self) -> &'static [u8] {
        match self {
            Self::Interpretation => b"interpretation",
            Self::Reminder => b"reminder",
        }
    }
}

/// How one change computes its value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Edit {
    /// An integer: an expression checked as in step 2c (provenance, checked evaluation).
    Expr { candidate: Candidate, shown: String },
    /// An enum option ID the pin declares (checked by the apply).
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "built by step 3c's reminder producer")
    )]
    Enum(String),
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "built by step 3c's reminder producer")
    )]
    Bool(bool),
}

/// One change of a proposal: the typed address, the value it had when the job was started
/// (`Unknown` for an unknown number, `Absent` for an optional cell not set) and how the new
/// value is made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Change {
    pub address: String,
    pub before: NoteValue,
    pub edit: Edit,
}

impl Change {
    /// The value the change writes.
    pub(crate) fn after(&self) -> NoteValue {
        match &self.edit {
            Edit::Expr { candidate, .. } => NoteValue::Int(candidate.value),
            Edit::Enum(id) => NoteValue::Enum(id.clone()),
            Edit::Bool(value) => NoteValue::Bool(*value),
        }
    }

    /// What the page shows as the change: the expression as shown (`−`, `×`, `÷`), an enum
    /// option ID or the boolean.
    pub(crate) fn shown(&self) -> String {
        match &self.edit {
            Edit::Expr { shown, .. } => shown.clone(),
            Edit::Enum(id) => id.clone(),
            Edit::Bool(value) => value.to_string(),
        }
    }

    /// Whether the edit's kind fits the value before (an unknown or unset one fits any).
    fn fits(&self) -> bool {
        matches!(
            (&self.edit, &self.before),
            (_, NoteValue::Unknown | NoteValue::Absent)
                | (Edit::Expr { .. }, NoteValue::Int(_))
                | (Edit::Enum(_), NoteValue::Enum(_))
                | (Edit::Bool(_), NoteValue::Bool(_))
        )
    }
}

/// A validated option with its bound read/write set.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Proposal {
    pub changes: Vec<Change>,
    /// The model's explanation, shown in full next to the changes.
    pub explanation: String,
    /// SHA-256 of the bound content.
    pub hash: [u8; 32],
}

/// Binds option `n` of job `id` to `origin`: `None` drops the option, when it has no change
/// or more than four, two changes with one target, a target that is not a field or a saved
/// row cell (no row adds or removes), a change that does not fit the value before, or a
/// change equal to the value before (plan §1.5).
pub(crate) fn bind(
    id: JobId,
    n: usize,
    origin: &Origin,
    changes: Vec<Change>,
    explanation: String,
) -> Option<Proposal> {
    let targets: BTreeSet<&str> = changes
        .iter()
        .map(|change| change.address.as_str())
        .collect();
    let typed = |address: &str| address.starts_with("field:") || address.starts_with("cell:");
    if changes.is_empty()
        || changes.len() > MAX_CHANGES
        || targets.len() != changes.len()
        || !changes.iter().all(|change| {
            typed(&change.address)
                && change.fits()
                && change.after() != change.before
                && change.before.encode().is_ok()
                && change.after().encode().is_ok()
        })
    {
        return None;
    }
    let hash = content_hash(id, n, origin, &changes, &explanation)?;
    Some(Proposal {
        changes,
        explanation,
        hash,
    })
}

/// Whether `proposal`, option `n` of job `id` bound to `origin`, still has the content
/// hash it was stored with (the apply's check).
///
/// It guards the apply against a stored proposal that no longer matches what was shown:
/// another index, job, kind or origin, a changed target, value before, value after,
/// expression, explanation or reference of any change. It does not guard against changes on
/// the sheet (the apply's live rechecks of the origin note, every target and every
/// reference do), against a client (the apply route takes only `csrf`, never proposal
/// content), or against the model's judgement of relevance.
pub(crate) fn verify(id: JobId, n: usize, origin: &Origin, proposal: &Proposal) -> bool {
    content_hash(id, n, origin, &proposal.changes, &proposal.explanation) == Some(proposal.hash)
}

fn content_hash(
    id: JobId,
    n: usize,
    origin: &Origin,
    changes: &[Change],
    explanation: &str,
) -> Option<[u8; 32]> {
    let mut hash = Sha256::new();
    hash.update(b"mimik-proposal-v2\0");
    let mut field = |bytes: &[u8]| {
        hash.update(u64::try_from(bytes.len()).unwrap_or(u64::MAX).to_be_bytes());
        hash.update(bytes);
    };
    field(&id.bytes());
    field(&u64::try_from(n).unwrap_or(u64::MAX).to_be_bytes());
    field(origin.kind.tag());
    field(&origin.sheet.to_be_bytes());
    field(origin.target.as_bytes());
    field(&origin.revision.to_be_bytes());
    field(
        &u64::try_from(changes.len())
            .unwrap_or(u64::MAX)
            .to_be_bytes(),
    );
    for change in changes {
        field(change.address.as_bytes());
        // A value that cannot be encoded has no hash: no collision between such values.
        field(change.before.encode().ok()?.as_bytes());
        field(change.after().encode().ok()?.as_bytes());
        if let Edit::Expr { candidate, .. } = &change.edit {
            field(b"expr");
            field(
                &u64::try_from(candidate.refs.len())
                    .unwrap_or(u64::MAX)
                    .to_be_bytes(),
            );
            for (alias, supplied) in &candidate.refs {
                field(alias.as_bytes());
                field(supplied.address.as_bytes());
                field(
                    supplied
                        .value
                        .map_or_else(|| "unknown".to_owned(), |v| v.to_string())
                        .as_bytes(),
                );
            }
            field(
                serde_json::to_string(&candidate.expr)
                    .unwrap_or_default()
                    .as_bytes(),
            );
        }
    }
    field(explanation.as_bytes());
    Some(hash.finalize().into())
}
