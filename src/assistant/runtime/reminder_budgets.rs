//! Reminder budgets (assisted sheet plan 3c-1, "Budgets"; 3c-1c): a reminder never shares
//! [`Budgets`](crate::assistant::jobs::Budgets) with an interpretation. It has its own run
//! deadline (its prompt at the measured prompt rate, its reply bound at the per-token speed
//! and the path's slowdown) and two busy terms, the extra wait a scribble reading gets while a
//! reminder runs: the whole reminder prompt until a reminder has shown its system message
//! cached (unverified), and the variable part plus restoring the reminder's cache entry once
//! one has (verified). Both include restoring the interpretation prefix, which every reminder
//! that started leaves behind. Reminders are unavailable for a generation whose numbers do not
//! fit, with the reason for the page.
use super::Path;
use super::budgets::{Derived, Measured, RUN_CEILING_S, TEMPLATE_MARGIN_TOKENS, run_deadline};
use super::launch::CTX_SIZE;
use std::time::Duration;

/// The cache entry load time per MiB of entry on the CPU path, in seconds, as measured on the
/// development host (3c-0, `docs/results.md`: about 20-55 ms for a 150 MiB entry; the figure
/// scales with the entry's size and the host's memory bandwidth, and a slower host is slower).
pub(crate) const RESTORE_S_PER_MIB: f64 = 0.055 / 150.0;
/// The same on the GPU path (3c-0: save plus load of a 263 MiB entry took up to 276 ms, about
/// 0.53 ms per MiB at worst), and the fixed cost of restoring the checkpoint (about 10 ms).
pub(crate) const GPU_RESTORE_S_PER_MIB: f64 = 0.00053;
pub(crate) const GPU_RESTORE_FIXED_S: f64 = 0.010;

/// The restore cost of the interpretation prefix after a reminder.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Restore {
    pub seconds: f64,
    /// The cost is a constant measured on the development host, not on this host.
    pub dev_host_constant: bool,
}

/// The restore after a reminder: loading the interpretation's cache entry (`entry_mib`: its
/// size, from the prefix's tokens), on both paths, from the constants measured on the
/// development host.
pub(crate) fn restore(path: Path, entry_mib: f64) -> Restore {
    Restore {
        seconds: match path {
            Path::Cpu => entry_mib * RESTORE_S_PER_MIB,
            Path::Gpu => entry_mib * GPU_RESTORE_S_PER_MIB + GPU_RESTORE_FIXED_S,
        },
        dev_host_constant: true,
    }
}

/// What a reminder needs, from the generation's measurements.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Inputs {
    pub measured: Measured,
    pub derived: Derived,
    /// The reminder system message's tokens (`/tokenize`; the chat template's are not counted).
    pub system_tokens: f64,
    /// The reminder reply bound in tokens.
    pub reply_tokens: u32,
    /// The size of the interpretation's cache entry, MiB (prefix, N(B) and `max_tokens`).
    pub entry_mib: f64,
    /// The size of the reminder's cache entry, MiB (system message, N(B) and reply bound).
    pub reminder_entry_mib: f64,
}

/// A reminder's budgets for a generation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ReminderBudgets {
    /// The reminder's run deadline.
    pub run: Duration,
    /// The estimated time of a reminder (before the 1.25 margin).
    pub estimate_s: f64,
    /// The busy term (the extra wait X) once a reminder of the generation has shown its system
    /// message cached: restoring the reminder's entry, the variable part of the prompt and the
    /// template's tail, and restoring the interpretation's entry afterwards.
    pub busy_verified: Duration,
    /// The busy term (Y) until then, and again after a cancelled reminder: the whole reminder
    /// prompt (a cancel can come before the first token) and the interpretation's restore.
    pub busy_unverified: Duration,
    /// The restore of the interpretation's entry after a reminder.
    pub restore: Restore,
    pub reply_tokens: u32,
    /// The interpretation prefix's tokens (priming's): the next interpretation after a reminder
    /// must read at least 75 % of them from the cache (checked at run time).
    pub prefix_tokens: u64,
    /// The reminder system message's tokens: a completed reminder must read at least 75 % of
    /// them from the cache (checked at run time).
    pub system_tokens: u64,
}

/// Why reminders are unavailable for a generation.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Unavailable {
    /// A measurement is missing or not finite.
    Unmeasured,
    /// The run deadline would pass the 600 s ceiling.
    OverCeiling { needed_s: u64 },
    /// The restore does not fit the margin of an interpretation's run deadline.
    BeyondMargin,
    /// The system message, N(B), the reply bound and the template's margin exceed the
    /// context.
    BeyondContext { needed: u64, context: u32 },
    /// The unverified busy term is more than a running interpretation makes a scribble reading
    /// wait (R plus its busy term).
    WorseThanInterpretation {
        needed_s: u64,
        interpretation_s: u64,
    },
}

impl Unavailable {
    /// The reason as the page states it.
    pub(crate) fn describe(&self, path: Path) -> String {
        let help = match path {
            Path::Cpu => "; more threads or the GPU path could make them available",
            Path::Gpu => "",
        };
        format!("{}{help}", self.reason())
    }

    fn reason(&self) -> String {
        match self {
            Self::Unmeasured => "the generation's measurements are incomplete".to_owned(),
            Self::OverCeiling { needed_s } => format!(
                "a reminder would need a run deadline of {needed_s} s, over the {RUN_CEILING_S} s ceiling"
            ),
            Self::BeyondMargin => {
                "restoring the interpretation prefix would exceed the run deadline's margin"
                    .to_owned()
            }
            Self::BeyondContext { needed, context } => format!(
                "a reminder's prompt and reply would need {needed} tokens, over the {context}-token context"
            ),
            Self::WorseThanInterpretation {
                needed_s,
                interpretation_s,
            } => format!(
                "a reminder would keep the model busy {needed_s} s, more than the {interpretation_s} s a running interpretation does"
            ),
        }
    }
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a positive token count of at most a few thousand"
)]
fn prefix_tokens(tokens: f64) -> u64 {
    tokens.max(0.0).round() as u64
}

fn rate(sample: &super::budgets::Sample) -> (f64, f64, f64) {
    (
        sample.prompt_ms / sample.prompt_n / 1000.0,
        sample.predicted_ms / sample.predicted_n / 1000.0,
        (sample.wall_ms - sample.prompt_ms - sample.predicted_ms).max(0.0) / 1000.0,
    )
}

/// The reminder budgets for a generation, or why they are unavailable.
///
/// # Errors
///
/// [`Unavailable`].
pub(crate) fn derive(inputs: &Inputs) -> Result<ReminderBudgets, Unavailable> {
    let path = inputs.measured.path;
    let rates = inputs.measured.samples.iter().map(rate);
    let (mut p, mut g, mut o) = (0.0_f64, 0.0_f64, 0.0_f64);
    for (rp, rg, ro) in rates {
        p = p.max(rp);
        g = g.max(rg);
        o = o.max(ro);
    }
    #[expect(
        clippy::cast_precision_loss,
        reason = "a token count of at most a few thousand"
    )]
    let bound_tokens = inputs.derived.budgets.bounds.tokens as f64;
    if ![p, g, o, inputs.system_tokens, bound_tokens]
        .into_iter()
        .all(f64::is_finite)
        || p <= 0.0
        || g <= 0.0
        || inputs.system_tokens <= 0.0
    {
        return Err(Unavailable::Unmeasured);
    }
    let reply = f64::from(inputs.reply_tokens);
    // The prompt (system message and N(B)), the reply bound and the template's margin must fit
    // the context, as an interpretation's do.
    let needed_tokens = inputs.system_tokens + bound_tokens + reply + TEMPLATE_MARGIN_TOKENS;
    if needed_tokens > f64::from(CTX_SIZE) {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a positive token count of at most a few thousand"
        )]
        return Err(Unavailable::BeyondContext {
            needed: needed_tokens.ceil() as u64,
            context: CTX_SIZE,
        });
    }
    let reply_s = g * reply * (1.0 + path.slowdown() * reply / 2.0);
    let prompt_s = p * (inputs.system_tokens + bound_tokens);
    let estimate_s = o + prompt_s + reply_s;
    let run = run_deadline(estimate_s);
    if run > RUN_CEILING_S {
        return Err(Unavailable::OverCeiling { needed_s: run });
    }
    let restore = restore(path, inputs.entry_mib);
    let reminder_restore = self::restore(path, inputs.reminder_entry_mib);
    if !restore.seconds.is_finite() || !reminder_restore.seconds.is_finite() {
        return Err(Unavailable::Unmeasured);
    }
    // Rounded up to whole seconds, as Q is. Unverified (Y): the whole prompt, then the
    // interpretation's restore. Verified (X): the reminder's own restore, the variable part and
    // the user turn's template tail, then the interpretation's restore.
    let unverified_s = prompt_s + restore.seconds;
    let verified_s =
        reminder_restore.seconds + p * (bound_tokens + TEMPLATE_MARGIN_TOKENS) + restore.seconds;
    let seconds = |s: f64| Duration::from_secs_f64(s.ceil());
    let (busy_unverified, busy_verified) = (seconds(unverified_s), seconds(verified_s));
    // The restore from the host-RAM cache happens inside the next interpretation's request, so
    // it must fit the margin R leaves over the estimate, on both paths.
    #[expect(
        clippy::cast_precision_loss,
        reason = "a run deadline of at most 600 s"
    )]
    let margin = inputs.derived.budgets.run.as_secs() as f64 - inputs.derived.estimate_s;
    if restore.seconds > margin {
        return Err(Unavailable::BeyondMargin);
    }
    // Never worse for a scribble reading than a running interpretation: R plus its busy term.
    let interpretation_s =
        (inputs.derived.budgets.run + inputs.derived.budgets.busy_after_expiry).as_secs();
    if busy_unverified.as_secs() > interpretation_s {
        return Err(Unavailable::WorseThanInterpretation {
            needed_s: busy_unverified.as_secs(),
            interpretation_s,
        });
    }
    Ok(ReminderBudgets {
        run: Duration::from_secs(run),
        estimate_s,
        busy_verified,
        busy_unverified,
        restore,
        reply_tokens: inputs.reply_tokens,
        prefix_tokens: prefix_tokens(inputs.measured.prefix_tokens),
        system_tokens: prefix_tokens(inputs.system_tokens),
    })
}

#[cfg(test)]
#[path = "reminder_budgets_tests.rs"]
pub(crate) mod tests;
