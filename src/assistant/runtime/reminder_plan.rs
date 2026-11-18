//! What a generation's measurements allow for reminders (plan 3c-1): the reminder system
//! message's tokens and the reply bound from the server's `/tokenize` (outside the breaker:
//! nothing here is an interpretation exchange), then [`reminder_budgets::derive`]; and, before
//! Ready, the prime that evaluates the reminder system message once (3c-1c).
use super::Path;
use super::budgets::Measured;
use super::calibration::Calibration;
use super::launch::entry_mib;
use super::reminder_budgets::{self, Inputs};
use super::server::Server;
use crate::assistant::contract::{
    REMINDER_PREFIX, ReminderDecrease, ReminderEnvelope, largest_reminder_reply, reminder_envelope,
};
use crate::assistant::interpreter::{Completion, Interpreter, RequestKind};
use crate::assistant::jobs::Reminders;
use std::time::Duration;

/// How long the two `/tokenize` calls may take.
pub(crate) const TOKENIZE_WITHIN: Duration = Duration::from_secs(30);

/// The reminders of a generation measured as `calibration` says on `path`. `model_sha256` and
/// `model_size` (0 when unknown) are the model file's, which size the cache entry.
pub(crate) async fn plan(
    server: &dyn Server,
    calibration: &Calibration,
    max_tokens: u32,
    path: Path,
    (model_sha256, model_size): (&str, u64),
) -> Reminders {
    let Ok(derived) = &calibration.derived else {
        return Reminders::Unavailable("the generation has no budgets".to_owned());
    };
    let tokens = tokio::time::timeout(TOKENIZE_WITHIN, async {
        let system = server.tokens(REMINDER_PREFIX.as_str()).await?;
        let reply = server.tokens(&largest_reminder_reply()).await?;
        Ok::<_, crate::assistant::interpreter::Failure>((system, reply))
    })
    .await;
    let Ok(Ok((system, reply))) = tokens else {
        return Reminders::Unavailable("the reminder prompt could not be measured".to_owned());
    };
    #[expect(
        clippy::cast_precision_loss,
        reason = "token counts of a few thousand, exact in f64"
    )]
    let (system_tokens, reply_tokens, bound_tokens, max_reply) = (
        system as f64,
        reply as f64,
        derived.budgets.bounds.tokens as f64,
        f64::from(max_tokens),
    );
    // The whole saved entries: the interpretation's from its prefix, N(B) and `max_tokens`, the
    // reminder's from its system message, N(B) and reply bound.
    let entry_mib_of = |tokens: f64| entry_mib(model_sha256, model_size, tokens);
    let inputs = Inputs {
        measured: Measured {
            path,
            max_tokens,
            prefix_tokens: calibration.prefix_tokens,
            samples: calibration.samples,
        },
        derived: *derived,
        system_tokens,
        reply_tokens: u32::try_from(reply).unwrap_or(u32::MAX),
        entry_mib: entry_mib_of(calibration.prefix_tokens + bound_tokens + max_reply),
        reminder_entry_mib: entry_mib_of(system_tokens + bound_tokens + reply_tokens),
    };
    match reminder_budgets::derive(&inputs) {
        Ok(budgets) => Reminders::Available(budgets),
        Err(why) => Reminders::Unavailable(why.describe(path)),
    }
}

/// A fixed synthetic reminder envelope with no sheet data: one decrease and nothing else.
fn prime_envelope() -> String {
    reminder_envelope(&ReminderEnvelope {
        decrease: ReminderDecrease {
            id: "hp".to_owned(),
            label: "Hit points".to_owned(),
            before: 2,
            after: 1,
            phrase: "took 1 damage".to_owned(),
        },
        fields: Vec::new(),
        rows: Vec::new(),
    })
}

/// Why the reminder prime did not complete.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PrimeFailure {
    /// The request failed: the server is not presumed busy.
    Failed,
    /// It ran past its bound: the server is presumed busy for that bound (`--parallel 1`
    /// keeps evaluating).
    TimedOut,
}

/// Sends one reminder request through the interpreter, as real reminders do (the same system
/// message, template and schema kind), with a fixed synthetic envelope and `max_tokens` 1, so
/// the reminder prefix is in the cache before the first reminder. `within` is the reminder's
/// run deadline.
pub(crate) async fn prime(
    interpreter: &dyn Interpreter,
    within: Duration,
) -> Result<(), PrimeFailure> {
    let completion = Completion {
        dynamic: prime_envelope(),
        ref_ids: vec!["hp".to_owned()],
        max_tokens: 1,
        kind: RequestKind::Reminder {
            rows: Vec::new(),
            targets: vec!["hp".to_owned()],
        },
    };
    match tokio::time::timeout(within, interpreter.complete(&completion)).await {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(_)) => Err(PrimeFailure::Failed),
        Err(_) => Err(PrimeFailure::TimedOut),
    }
}

#[cfg(test)]
#[path = "reminder_plan_tests.rs"]
mod tests;
