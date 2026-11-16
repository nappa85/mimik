//! Job budgets from the calibration samples (assisted sheet plan §1.4a, "Budgets"; ASSIST-31,
//! ASSIST-32): a latency estimate within one safety ceiling, recomputed at every start, not
//! a worst-case bound.
//!
//! Each sample generates [`SAMPLE_TOKENS`] tokens; the reply the owner allows (`max_tokens`,
//! `model::reply_length`) enters the estimate through the measured per-token time.
//! Per sample i: per-token prompt time pᵢ = `prompt_ms` / `prompt_n`, per-token generation
//! time gᵢ = `predicted_ms` / `predicted_n`, overhead oᵢ = max(0, W − `prompt_ms` −
//! `predicted_ms`) and the uncached tail tᵢ = `prompt_n` − Nᵢ. For a candidate bound B,
//! N(B) = floor(`N_cal` × B / cap) with `N_cal` = min(N₁, N₂), Tᵢ(B) = oᵢ + pᵢ × (tᵢ + N(B)) +
//! gᵢ × `max_tokens`, T(B) = max(T₁(B), T₂(B)) and R(B) = max(15, 1.25 × T(B) rounded up to
//! 5 s). B is the largest candidate whose R fits [`RUN_CEILING_S`] and whose request, with
//! the reply and a template margin, fits the context.
//! The queue wait Q is the total wait's rest plus the calibrated prompt term, the time the
//! server stays busy after a request cancelled at R (CPU: total − R + busy; GPU: D × (R +
//! busy)): a job queued behind ones that expire still starts within Q.
use super::Path;
use super::launch::CTX_SIZE;
use crate::assistant::jobs::Budgets;
use crate::assistant::request::Bounds;
use std::time::Duration;

/// The margin over the slower sample (ASSIST-32).
const MARGIN: f64 = 1.25;
/// The run-deadline floor, in seconds (ASSIST-32).
const FLOOR_S: u64 = 15;
/// R is rounded up to a multiple of this many seconds.
const STEP_S: u64 = 5;
/// Queue wait plus run deadline, at least this long (ASSIST-32), in seconds: the total wait is
/// the larger of this and 2 × R.
const TOTAL_S: u64 = 120;
/// The most R may be, in seconds, on both paths (ASSIST-31, ASSIST-32): a limit on stuck
/// requests, not a wait.
pub(crate) const RUN_CEILING_S: u64 = 600;
/// The tokens each calibration sample generates (`ignore_eos`).
pub(crate) const SAMPLE_TOKENS: u32 = 64;
/// One calibration sample is abandoned after this long: a wait above it already means R is
/// over [`RUN_CEILING_S`].
pub(crate) const SAMPLE_TIMEOUT_S: u64 = 480;
/// Tokens kept free in the context for the chat template, on top of the request and the reply.
const TEMPLATE_MARGIN_TOKENS: f64 = 64.0;
/// The cached share of the fixed prefix a sample must show (a timing criterion).
const MIN_CACHED: f64 = 0.75;

impl Path {
    /// The byte cap of the dynamic part, which the calibration fillers fill exactly.
    pub(crate) fn cap(self) -> usize {
        match self {
            Self::Cpu => 1024,
            Self::Gpu => crate::assistant::contract::MAX_DYNAMIC_BYTES,
        }
    }

    /// The candidate bounds, largest first.
    fn candidates(self) -> &'static [usize] {
        match self {
            Self::Cpu => &[1024, 768, 512],
            Self::Gpu => &[4096, 2048, 1024],
        }
    }
}

/// One calibration sample as the server reported it, plus its filler's token count.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Sample {
    /// Wall time of the exchange, milliseconds.
    pub wall_ms: f64,
    /// Prompt tokens evaluated (not read from the cache).
    pub prompt_n: f64,
    pub prompt_ms: f64,
    /// Prompt tokens read from the cache.
    pub cached_n: f64,
    pub predicted_n: f64,
    pub predicted_ms: f64,
    /// The filler's token count from `/tokenize` (Nᵢ).
    pub filler_tokens: f64,
}

/// What the measurement must derive from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Measured {
    pub path: Path,
    pub max_tokens: u32,
    /// The fixed prefix's tokens as priming measured them (its `prompt_n` on an empty cache).
    pub prefix_tokens: f64,
    pub samples: [Sample; 2],
}

/// Why no budgets derive.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum NoBudgets {
    /// A sample's timing is missing, zero, not finite, `predicted_n` differs from
    /// [`SAMPLE_TOKENS`], its tail is negative or too little of the prefix was cached: the attempt
    /// fails (a timing criterion, not a content one).
    Invalid(&'static str),
    /// Even the smallest candidate does not fit: about `needed_s` would be needed, the cap
    /// is `cap_s`.
    TooSlow { needed_s: u64, cap_s: u64 },
    /// No candidate bound leaves room in the context for the fixed prefix, the request, the
    /// reply the owner allows and the template margin (never cut silently).
    ReplyTooLarge,
}

/// The derived budgets with what they were derived from (shown on the assistant page).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Derived {
    pub budgets: Budgets,
    /// T(B) in seconds for the chosen B.
    pub estimate_s: f64,
    /// The prompt term of the slower sample at the chosen B, in seconds: how long a request
    /// cancelled during its prompt evaluation may keep the server busy.
    pub prompt_s: f64,
}

struct Rates {
    p: f64,
    g: f64,
    o: f64,
    t: f64,
}

fn positive(value: f64) -> bool {
    value.is_finite() && value > 0.0
}

fn rates(sample: &Sample, prefix_tokens: f64) -> Result<Rates, NoBudgets> {
    let fields = [
        sample.wall_ms,
        sample.prompt_n,
        sample.prompt_ms,
        sample.cached_n,
        sample.predicted_n,
        sample.predicted_ms,
        sample.filler_tokens,
    ];
    if !fields.into_iter().all(positive) {
        return Err(NoBudgets::Invalid(
            "a calibration timing is missing, zero or not finite",
        ));
    }
    if (sample.predicted_n - f64::from(SAMPLE_TOKENS)).abs() > f64::EPSILON {
        return Err(NoBudgets::Invalid(
            "a calibration sample did not generate exactly 64 tokens",
        ));
    }
    let t = sample.prompt_n - sample.filler_tokens;
    if t < 0.0 {
        return Err(NoBudgets::Invalid(
            "a calibration sample's prompt tail is negative",
        ));
    }
    if !positive(prefix_tokens) || sample.cached_n < MIN_CACHED * prefix_tokens {
        return Err(NoBudgets::Invalid(
            "a calibration sample read too little of the fixed prefix from the cache",
        ));
    }
    Ok(Rates {
        p: sample.prompt_ms / sample.prompt_n / 1000.0,
        g: sample.predicted_ms / sample.predicted_n / 1000.0,
        o: (sample.wall_ms - sample.prompt_ms - sample.predicted_ms).max(0.0) / 1000.0,
        t,
    })
}

/// R for an estimate: 1.25 × T rounded up to 5 s, at least 15 s.
fn run_deadline(estimate_s: f64) -> u64 {
    let scaled = (MARGIN * estimate_s).ceil();
    // A finite, positive estimate below 2^53 s converts exactly; anything else is over every
    // cap anyway.
    let whole = if scaled.is_finite() && scaled < 9.0e15 {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "finite, non-negative and below 2^53, checked above"
        )]
        let whole = scaled.max(0.0) as u64;
        whole
    } else {
        u64::MAX - STEP_S
    };
    whole.div_ceil(STEP_S).saturating_mul(STEP_S).max(FLOOR_S)
}

/// Derives B, N(B), R, D and Q from two samples (plan §1.4a, "Budgets").
///
/// # Errors
///
/// [`NoBudgets::Invalid`] for a sample that fails the timing checks, [`NoBudgets::TooSlow`]
/// when no candidate fits.
pub(crate) fn derive(measured: &Measured) -> Result<Derived, NoBudgets> {
    let path = measured.path;
    let rates = [
        rates(&measured.samples[0], measured.prefix_tokens)?,
        rates(&measured.samples[1], measured.prefix_tokens)?,
    ];
    let n_cal = measured.samples[0]
        .filler_tokens
        .min(measured.samples[1].filler_tokens);
    let cap = path.cap();
    let max_tokens = f64::from(measured.max_tokens);
    let mut slowest_needed = 0;
    let fits_context = |tokens: f64| {
        measured.prefix_tokens + tokens + max_tokens + TEMPLATE_MARGIN_TOKENS <= f64::from(CTX_SIZE)
    };
    let smallest = path.candidates().last().copied().unwrap_or(0);
    #[expect(
        clippy::cast_precision_loss,
        reason = "byte bounds are at most 4,096, exact in f64"
    )]
    let smallest_tokens = (n_cal * (smallest as f64 / cap as f64)).floor();
    if !fits_context(smallest_tokens) {
        return Err(NoBudgets::ReplyTooLarge);
    }
    for &bytes in path.candidates() {
        #[expect(
            clippy::cast_precision_loss,
            reason = "byte bounds are at most 4,096, exact in f64"
        )]
        let share = bytes as f64 / cap as f64;
        let tokens = (n_cal * share).floor();
        if !fits_context(tokens) {
            continue;
        }
        let (estimate_s, prompt_s) = rates.iter().fold((0.0_f64, 0.0_f64), |(t, p), r| {
            let prompt = r.p * (r.t + tokens);
            (t.max(r.o + prompt + r.g * max_tokens), p.max(prompt))
        });
        let run = run_deadline(estimate_s);
        slowest_needed = run;
        if run > RUN_CEILING_S {
            continue;
        }
        let total = TOTAL_S.max(2 * run);
        // The server stays busy for `busy_after_expiry` after a request cancelled at R, and a
        // job queued behind it waits that long too: Q carries that slack, so such a job still
        // starts within Q (CPU: total - R + busy; GPU: each of D jobs may follow one cancelled
        // at R, D x (R + busy)).
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a non-negative whole number of seconds, far below u64"
        )]
        let busy = prompt_s.ceil() as u64;
        let (depth, queue) = match path {
            Path::Cpu => (1, total - run + busy),
            Path::Gpu => {
                let depth = ((total - run) / run).clamp(1, 4);
                (depth, depth * (run + busy))
            }
        };
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a non-negative token count of at most a 4 KiB filler"
        )]
        let tokens = tokens as usize;
        return Ok(Derived {
            budgets: Budgets {
                bounds: Bounds { bytes, tokens },
                run: Duration::from_secs(run),
                depth: usize::try_from(depth).unwrap_or(1),
                queue_wait: Duration::from_secs(queue),
                max_tokens: measured.max_tokens,
                busy_after_expiry: Duration::from_secs(busy),
            },
            estimate_s,
            prompt_s,
        });
    }
    Err(NoBudgets::TooSlow {
        needed_s: slowest_needed,
        cap_s: RUN_CEILING_S,
    })
}

#[cfg(test)]
#[path = "budgets_tests.rs"]
mod tests;
