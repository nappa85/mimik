//! What a generation's measurements allow for reminders (plan 3c-1): the reminder system
//! message's tokens and the reply bound from the server's `/tokenize` (outside the breaker:
//! nothing here is an interpretation exchange), then [`reminder_budgets::derive`].
use super::Path;
use super::budgets::Measured;
use super::calibration::Calibration;
use super::launch::entry_kib_per_token;
use super::reminder_budgets::{self, Inputs};
use super::server::Server;
use crate::assistant::contract::{REMINDER_PREFIX, largest_reminder_reply};
use crate::assistant::jobs::Reminders;
use std::time::Duration;

/// How long the two `/tokenize` calls may take.
pub(crate) const TOKENIZE_WITHIN: Duration = Duration::from_secs(30);

/// The reminders of a generation measured as `calibration` says on `path`. `model_size` is the
/// model file's size (0 when unknown), which bounds the cache entry's size.
pub(crate) async fn plan(
    server: &dyn Server,
    calibration: &Calibration,
    max_tokens: u32,
    path: Path,
    model_size: u64,
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
    let (system_tokens, entry_mib) = (
        system as f64,
        calibration.prefix_tokens * entry_kib_per_token(model_size) as f64 / 1024.0,
    );
    let inputs = Inputs {
        measured: Measured {
            path,
            max_tokens,
            prefix_tokens: calibration.prefix_tokens,
            samples: calibration.samples,
        },
        derived: *derived,
        primed: calibration.primed,
        system_tokens,
        reply_tokens: u32::try_from(reply).unwrap_or(u32::MAX),
        entry_mib,
    };
    match reminder_budgets::derive(&inputs) {
        Ok(budgets) => Reminders::Available(budgets),
        Err(why) => Reminders::Unavailable(why.describe()),
    }
}

#[cfg(test)]
#[path = "reminder_plan_tests.rs"]
mod tests;
