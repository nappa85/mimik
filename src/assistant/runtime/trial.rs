//! The five synthetic phrases of a model trial (`docs/model-runtime.md`, "Owner model
//! setting"): shipped with Mimik, no sheet or player text. Each is sent as a job would be
//! (the fixed prefix, the schema of its context IDs, a data envelope) at the default reply
//! length, and its reply must be schema-valid and untruncated; the largest `predicted_n` is
//! recorded as information (the reply length is the owner's setting).
use super::models::PhraseResult;
use super::server::Server;
use crate::assistant::contract::{
    Envelope, EnvelopeItem, EnvelopeTarget, ItemKind, TARGET_ALIAS, envelope,
};
use crate::assistant::model::reply_length::DEFAULT as TRIAL_TOKENS;
use crate::assistant::output::{self, Checks, Reading, Supplied};
use std::collections::BTreeMap;
use std::time::Duration;

/// One synthetic phrase: a note typed into a target, a little context, and the values that
/// would be right (information only: D-D says model quality is not a gate).
pub(crate) struct Phrase {
    pub note: &'static str,
    /// The target's field ID, label and value.
    pub target: (&'static str, &'static str, i64),
    /// Context fields: ID, label, value.
    pub context: &'static [(&'static str, &'static str, i64)],
    /// The values an option should reach.
    pub expected: &'static [i64],
}

/// The phrases: damage, healing, a half with an odd value (both roundings), spending, and
/// an Italian note.
pub(crate) const PHRASES: [Phrase; 5] = [
    Phrase {
        note: "took 6 damage",
        target: ("hp", "Hit points", 20),
        context: &[
            ("max_hp", "Maximum hit points", 24),
            ("silver", "Silver", 120),
        ],
        expected: &[14],
    },
    Phrase {
        note: "healed 4",
        target: ("hp", "Hit points", 11),
        context: &[("max_hp", "Maximum hit points", 24)],
        expected: &[15],
    },
    Phrase {
        note: "lost half",
        target: ("hp", "Hit points", 9),
        context: &[("max_hp", "Maximum hit points", 24), ("omens", "Omens", 2)],
        expected: &[5, 4],
    },
    Phrase {
        note: "spent 15 silver",
        target: ("silver", "Silver", 120),
        context: &[("hp", "Hit points", 20)],
        expected: &[105],
    },
    Phrase {
        note: "ho perso 3 punti",
        target: ("hp", "Hit points", 17),
        context: &[("max_hp", "Maximum hit points", 24)],
        expected: &[14],
    },
];

impl Phrase {
    /// The dynamic part, the IDs of its context items (the reply schema's `ref` enum) and
    /// what its reply is checked against.
    pub(crate) fn request(&self) -> (String, Vec<String>, Checks) {
        let (id, label, value) = self.target;
        let mut supplied = BTreeMap::from([(
            TARGET_ALIAS.to_owned(),
            Supplied {
                address: format!("field:{id}"),
                integer: true,
                value: Some(value),
            },
        )]);
        let mut context = Vec::new();
        for &(id, label, value) in self.context {
            supplied.insert(
                id.to_owned(),
                Supplied {
                    address: format!("field:{id}"),
                    integer: true,
                    value: Some(value),
                },
            );
            context.push(EnvelopeItem {
                id: id.to_owned(),
                label: label.to_owned(),
                kind: ItemKind::Integer,
            });
        }
        let ref_ids = context.iter().map(|item| item.id.clone()).collect();
        let dynamic = envelope(&Envelope {
            note: self.note.to_owned(),
            target: EnvelopeTarget {
                alias: TARGET_ALIAS,
                id: id.to_owned(),
                label: Some(label.to_owned()),
                value: Some(value),
            },
            context,
        });
        let checks = Checks {
            phrase: self.note.to_owned(),
            target_value: Some(value),
            supplied,
        };
        (dynamic, ref_ids, checks)
    }
}

/// Why the phrases did not all pass.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PhraseFailure {
    /// Past its deadline.
    Deadline { note: &'static str, seconds: u64 },
    /// The exchange failed.
    Exchange { note: &'static str, what: String },
    /// The reply stopped at the default reply length.
    Truncated { note: &'static str },
    /// The reply is not valid against the schema.
    Invalid { note: &'static str },
}

impl PhraseFailure {
    pub(crate) fn describe(&self) -> String {
        match self {
            Self::Deadline { note, seconds } => {
                format!("the phrase \"{note}\" took more than {seconds} s")
            }
            Self::Exchange { note, what } => format!("the phrase \"{note}\": {what}"),
            Self::Truncated { note } => {
                format!("the reply to \"{note}\" was cut off at {TRIAL_TOKENS} tokens")
            }
            Self::Invalid { note } => {
                format!("the reply to \"{note}\" does not follow the reply schema")
            }
        }
    }
}

/// Sends the phrases in order, each within `bound`, and returns how each went. `progress` is
/// told which phrase is next (1-based).
///
/// # Errors
///
/// The first phrase that fails: past its deadline, a failed exchange, a reply cut off at the
/// default reply length or one the schema refuses.
pub(crate) async fn run(
    server: &dyn Server,
    bound: Duration,
    progress: &(dyn Fn(usize) + Sync),
) -> Result<Vec<PhraseResult>, PhraseFailure> {
    let mut results = Vec::with_capacity(PHRASES.len());
    for (n, phrase) in PHRASES.iter().enumerate() {
        progress(n + 1);
        let (dynamic, ref_ids, checks) = phrase.request();
        let began = tokio::time::Instant::now();
        let reply =
            tokio::time::timeout(bound, server.structured(&dynamic, &ref_ids, TRIAL_TOKENS))
                .await
                .map_err(|_| PhraseFailure::Deadline {
                    note: phrase.note,
                    seconds: bound.as_secs(),
                })?
                .map_err(|failure| PhraseFailure::Exchange {
                    note: phrase.note,
                    what: describe_failure(failure),
                })?;
        let seconds = began.elapsed().as_secs_f64();
        if reply.truncated {
            return Err(PhraseFailure::Truncated { note: phrase.note });
        }
        let reading = output::read(&reply.text, &checks)
            .map_err(|_| PhraseFailure::Invalid { note: phrase.note })?;
        let expected_offered = match reading {
            Reading::Options { shown, .. } => shown
                .iter()
                .any(|candidate| phrase.expected.contains(&candidate.value)),
            Reading::Ambiguous(_) => false,
        };
        // A reply without its generated-token count is not a measurement.
        let predicted_n = reply
            .predicted_n
            .filter(|n| n.is_finite() && *n >= 1.0)
            .ok_or_else(|| PhraseFailure::Exchange {
                note: phrase.note,
                what: "the server did not report the generated tokens".into(),
            })?;
        results.push(PhraseResult {
            note: phrase.note,
            predicted_n,
            seconds,
            expected_offered,
        });
    }
    Ok(results)
}

fn describe_failure(failure: crate::assistant::interpreter::Failure) -> String {
    use crate::assistant::interpreter::Failure;
    match failure {
        Failure::Status(status) => format!("the server answered HTTP {status}"),
        Failure::Transport => "no well-formed answer".into(),
    }
}

/// The largest `predicted_n` of the phrases, as whole tokens.
pub(crate) fn largest_predicted(results: &[PhraseResult]) -> u32 {
    let largest = results
        .iter()
        .map(|r| r.predicted_n)
        .fold(0.0_f64, f64::max);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a finite token count of at most the default reply length"
    )]
    let tokens = largest.ceil().min(f64::from(TRIAL_TOKENS)) as u32;
    tokens
}

#[cfg(test)]
#[path = "trial_tests.rs"]
mod tests;
