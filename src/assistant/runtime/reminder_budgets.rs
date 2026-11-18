//! Reminder budgets (assisted sheet plan 3c-1, "Budgets"): a reminder never shares
//! [`Budgets`](crate::assistant::jobs::Budgets) with an interpretation. It has its own run
//! deadline (its prompt at the measured prompt rate, its reply bound at the per-token speed
//! and the path's slowdown), its own busy term (the whole reminder prompt, as a cancel can
//! come before its first token) and a restore cost for the interpretation prefix that every
//! reminder that started leaves behind. Reminders are unavailable for a generation whose
//! numbers do not fit, with the reason for the page.
use super::Path;
use super::budgets::{Derived, Measured, RUN_CEILING_S, TEMPLATE_MARGIN_TOKENS, run_deadline};
use super::calibration::Primed;
use super::launch::CTX_SIZE;
use std::time::Duration;

/// The largest the cache entry load time per MiB of entry, in seconds, as measured on the
/// development host (3c-0, `docs/results.md`: about 20-55 ms for a 150 MiB entry, so 0.37
/// ms/MiB; the figure scales with the entry's size and the host's memory bandwidth, and a
/// slower host is slower). Used for the CPU path's host-RAM prompt cache (choice (c)).
pub(crate) const RESTORE_S_PER_MIB: f64 = 0.055 / 150.0;

/// The restore cost of the interpretation prefix after a reminder.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Restore {
    pub seconds: f64,
    /// The cost is the dev host's constant [`RESTORE_S_PER_MIB`] (CPU, choice (c)), not
    /// measured on this host; false for the GPU path's re-priming, which priming measured.
    pub dev_host_constant: bool,
}

/// The restore after a reminder: on the CPU path loading the interpretation's cache entry
/// (`entry_mib`: its size, from the prefix's tokens), on the GPU path re-priming the prefix
/// (the time priming measured; without that time, the measured prompt rate times the prefix).
pub(crate) fn restore(path: Path, primed: &Primed, entry_mib: f64, prompt_rate_s: f64) -> Restore {
    match path {
        Path::Cpu => Restore {
            seconds: entry_mib * RESTORE_S_PER_MIB,
            dev_host_constant: true,
        },
        Path::Gpu => {
            let measured = primed.prompt_ms / 1000.0;
            Restore {
                seconds: if measured.is_finite() && measured > 0.0 {
                    measured
                } else {
                    prompt_rate_s * primed.prefix_tokens
                },
                dev_host_constant: false,
            }
        }
    }
}

/// What a reminder needs, from the generation's measurements.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Inputs {
    pub measured: Measured,
    pub derived: Derived,
    pub primed: Primed,
    /// The reminder system message's tokens (`/tokenize`; the chat template's are not counted).
    pub system_tokens: f64,
    /// The reminder reply bound in tokens.
    pub reply_tokens: u32,
    /// The size of the interpretation prefix's cache entry, MiB (CPU path).
    pub entry_mib: f64,
}

/// A reminder's budgets for a generation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ReminderBudgets {
    /// The reminder's run deadline.
    pub run: Duration,
    /// The estimated time of a reminder (before the 1.25 margin).
    pub estimate_s: f64,
    /// The busy term: `p x (reminder prompt + N(B))`, the whole prompt (a cancel can come
    /// before the first token), counted by `presume_busy` and by admission while one runs.
    pub busy: Duration,
    pub restore: Restore,
    pub reply_tokens: u32,
    /// The interpretation prefix's tokens (priming's): the next interpretation after a reminder
    /// must read at least 75 % of them from the cache (CPU path, checked at run time).
    pub prefix_tokens: u64,
    /// The worst extra wait of a scribble reading right after a cancelled reminder: its whole
    /// prompt plus the restore.
    pub extra_wait: Duration,
}

/// Why reminders are unavailable for a generation.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Unavailable {
    /// A measurement is missing or not finite.
    Unmeasured,
    /// The run deadline would pass the 600 s ceiling.
    OverCeiling { needed_s: u64 },
    /// The busy term, the restore and the readings ahead (R each) are more than Q: a reminder
    /// could make a scribble reading refused.
    BeyondQueue { needed_s: u64, queue_s: u64 },
    /// The CPU restore does not fit the margin of an interpretation's run deadline.
    BeyondMargin,
    /// The system message, N(B), the reply bound and the template's margin exceed the
    /// context.
    BeyondContext { needed: u64, context: u32 },
    /// The busy term plus the restore is more than a running interpretation makes a scribble
    /// reading wait (R plus its busy term).
    WorseThanInterpretation {
        needed_s: u64,
        interpretation_s: u64,
    },
}

impl Unavailable {
    /// The reason as the page states it.
    pub(crate) fn describe(&self) -> String {
        match self {
            Self::Unmeasured => "the generation's measurements are incomplete".to_owned(),
            Self::OverCeiling { needed_s } => format!(
                "a reminder would need a run deadline of {needed_s} s, over the {RUN_CEILING_S} s ceiling"
            ),
            Self::BeyondQueue { needed_s, queue_s } => format!(
                "a scribble reading waiting behind a reminder and the readings ahead of it could wait {needed_s} s, more than the {queue_s} s it may"
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
    let restore = restore(path, &inputs.primed, inputs.entry_mib, p);
    if !restore.seconds.is_finite() {
        return Err(Unavailable::Unmeasured);
    }
    let busy = Duration::from_secs_f64(prompt_s.ceil());
    // Rounded up to whole seconds, as Q is.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a non-negative number of seconds, far below u64"
    )]
    let needed_s = (prompt_s + restore.seconds).ceil() as u64;
    // A reading may wait behind the reminder's busy time and the restore, and behind the readings
    // admitted ahead of it: one on the CPU path (D is 1 and the first reading does not count
    // toward it), D on the GPU path; each takes R. All within Q.
    let queue_s = inputs.derived.budgets.queue_wait.as_secs();
    let ahead = match path {
        Path::Cpu => 1,
        Path::Gpu => u64::try_from(inputs.derived.budgets.depth).unwrap_or(u64::MAX),
    };
    let ahead_s = inputs.derived.budgets.run.as_secs().saturating_mul(ahead);
    if needed_s.saturating_add(ahead_s) > queue_s {
        return Err(Unavailable::BeyondQueue {
            needed_s: needed_s.saturating_add(ahead_s),
            queue_s,
        });
    }
    // With the host-RAM cache the restore happens inside the next interpretation's request,
    // so it must fit the margin R leaves over the estimate.
    #[expect(
        clippy::cast_precision_loss,
        reason = "a run deadline of at most 600 s"
    )]
    let margin = inputs.derived.budgets.run.as_secs() as f64 - inputs.derived.estimate_s;
    if path == Path::Cpu && restore.seconds > margin {
        return Err(Unavailable::BeyondMargin);
    }
    // Never worse for a scribble reading than a running interpretation: R plus its busy term.
    let interpretation_s =
        (inputs.derived.budgets.run + inputs.derived.budgets.busy_after_expiry).as_secs();
    if needed_s > interpretation_s {
        return Err(Unavailable::WorseThanInterpretation {
            needed_s,
            interpretation_s,
        });
    }
    Ok(ReminderBudgets {
        run: Duration::from_secs(run),
        estimate_s,
        busy,
        restore,
        reply_tokens: inputs.reply_tokens,
        prefix_tokens: prefix_tokens(inputs.primed.prefix_tokens),
        extra_wait: Duration::from_secs(needed_s),
    })
}

#[cfg(test)]
#[path = "reminder_budgets_tests.rs"]
pub(crate) mod tests;
