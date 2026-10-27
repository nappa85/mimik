//! Rendering a request within its bounds (assisted sheet plan §1.4, "Request builder"):
//! the dynamic part holds at most B bytes after escaping and at most N(B) tokens as the
//! server counts them; whole context items are dropped from the end until both hold, then
//! the target's label; the phrase, the target's ID and its value are never dropped, and a
//! request that still does not fit is "too long for the assistant on this machine".
use super::contract::{Envelope, EnvelopeItem, EnvelopeTarget, envelope};
use super::interpreter::{Failure, Interpreter};
use super::output::{Checks, Supplied};
use std::collections::BTreeMap;

/// Everything a request could carry, in the order it is sent: the phrase, the target, then
/// the context items. Item IDs are unique and differ from the target's name and ID (the
/// builder, `campaigns::interpret::draft`, leaves colliding items out).
#[derive(Clone, Debug)]
pub(crate) struct Draft {
    pub phrase: String,
    pub target: EnvelopeTarget,
    /// The target's typed address, bound as `t`.
    pub target_address: String,
    pub items: Vec<(EnvelopeItem, Supplied)>,
}

/// The bounds of the dynamic part.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Bounds {
    /// B: bytes after escaping.
    pub bytes: usize,
    /// N(B): tokens as the server's `/tokenize` counts them.
    pub tokens: usize,
}

/// A rendered request: the dynamic part, the IDs of its context items (the reply schema's
/// `ref` enum, [`super::contract::schema_for`]) and what the reply is checked against.
#[derive(Clone, Debug)]
pub(crate) struct Rendered {
    pub dynamic: String,
    pub ref_ids: Vec<String>,
    pub checks: Checks,
}

/// Why nothing was rendered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NotRendered {
    /// The phrase and the target alone exceed a bound: no completion request is sent and
    /// the breaker is not touched.
    TooLong,
    /// `/tokenize` failed.
    Tokenizer(Failure),
}

impl Draft {
    fn content(&self, kept: usize, label: bool) -> Envelope {
        let mut target = self.target.clone();
        if !label {
            target.label = None;
        }
        Envelope {
            note: self.phrase.clone(),
            target,
            context: self.items[..kept]
                .iter()
                .map(|(item, _)| item.clone())
                .collect(),
        }
    }

    fn checks(&self, kept: usize) -> Checks {
        let mut supplied = BTreeMap::from([(
            super::contract::TARGET_ALIAS.to_owned(),
            Supplied {
                address: self.target_address.clone(),
                integer: true,
                value: self.target.value,
            },
        )]);
        for (item, bound) in &self.items[..kept] {
            supplied.insert(item.id.clone(), bound.clone());
        }
        Checks {
            phrase: self.phrase.clone(),
            target_value: self.target.value,
            supplied,
        }
    }

    /// The candidate shapes in the order they are tried: every item and the label, then
    /// one item fewer each time, then no label.
    fn shapes(&self) -> impl Iterator<Item = (usize, bool)> {
        let label = self.target.label.is_some();
        (0..=self.items.len())
            .rev()
            .map(move |kept| (kept, label))
            .chain(label.then_some((0, false)))
    }
}

/// Renders `draft` within `bounds`, counting tokens with `interpreter` only for shapes that
/// already fit the byte bound.
pub(crate) async fn render(
    draft: &Draft,
    bounds: Bounds,
    interpreter: &dyn Interpreter,
) -> Result<Rendered, NotRendered> {
    for (kept, label) in draft.shapes() {
        let dynamic = envelope(&draft.content(kept, label));
        if dynamic.len() > bounds.bytes {
            continue;
        }
        let tokens = interpreter
            .tokens(&dynamic)
            .await
            .map_err(NotRendered::Tokenizer)?;
        if tokens <= bounds.tokens {
            return Ok(Rendered {
                dynamic,
                // Enum items are context for reading; the schema lets a reply name integers.
                ref_ids: draft.items[..kept]
                    .iter()
                    .filter(|(_, bound)| bound.integer)
                    .map(|(item, _)| item.id.clone())
                    .collect(),
                checks: draft.checks(kept),
            });
        }
    }
    Err(NotRendered::TooLong)
}

#[cfg(test)]
#[path = "request_tests.rs"]
mod tests;
