//! Priming and calibration (assisted sheet plan §1.4a, "Priming and calibration"): after
//! `/health`, priming sends the fixed prefix with a minimal data envelope and `max_tokens` 1,
//! which fills the prompt cache (priming is the readiness gate: no job is admitted before
//! the prefix is cached); then two calibration samples, fixed synthetic jobs
//! shipped with Mimik (no sheet or player text), each with a dynamic part of exactly the
//! path's byte cap built from filler shaped like real context items, 64 tokens
//! ([`budgets::SAMPLE_TOKENS`]), `ignore_eos` and no schema, so each generates exactly 64
//! tokens (the first comes from the prompt pass, which makes the generation speed about 1.6 %
//! optimistic). The reply length the owner allows enters the estimate through that speed. The
//! two fillers differ from their first byte, so neither reuses the other's cache. Neither
//! counts toward the breaker.
use super::Path;
use super::budgets::{self, Derived, Measured, NoBudgets, SAMPLE_TIMEOUT_S, SAMPLE_TOKENS, Sample};
use super::server::{Server, Timings};
use crate::assistant::contract::{Envelope, EnvelopeTarget, TARGET_ALIAS, envelope};
use crate::assistant::interpreter::Failure;
use serde_json::json;

/// IDs, labels and (unsent) values of the shipped demo and 2b synthetic sheets
/// (`tools/assist-eval`), the source of the filler's token density.
const FIELDS: &[(&str, &str, i64)] = &[
    ("hp", "Hit points", 20),
    ("max_hp", "Maximum hit points", 24),
    ("strength", "Strength", 3),
    ("agility", "Agility", 1),
    ("presence", "Presence", -1),
    ("toughness", "Toughness", 2),
    ("omens", "Omens", 2),
    ("silver", "Silver", 120),
];

/// The minimal data envelope priming sends.
pub(crate) fn priming_envelope() -> String {
    envelope(&Envelope {
        note: "took 1 damage".into(),
        target: EnvelopeTarget {
            alias: TARGET_ALIAS,
            id: "hp".into(),
            label: None,
            value: Some(1),
        },
        context: Vec::new(),
    })
}

fn item(n: usize, variant: usize, label_suffix: &str) -> String {
    // A context item as a request carries it (ASSIST-56): ID, label and type, no value.
    let (id, label, _) = FIELDS[(n + variant * 3) % FIELDS.len()];
    json!({
        "id": format!("{id}_{}", n / FIELDS.len()),
        "label": format!("{label}{label_suffix}"),
        "type": "integer",
    })
    .to_string()
}

/// A calibration filler of exactly `cap` bytes (ASCII): context items as requests carry
/// them, variant 0 opening with `[`, variant 1 with `{`, the last item's label padded to the
/// exact length.
pub(crate) fn filler(cap: usize, variant: usize) -> String {
    let (open, close) = if variant == 0 { ("[", "]") } else { ("", "") };
    let text = |items: &[String]| format!("{open}{}{close}", items.join(","));
    let mut items: Vec<String> = Vec::new();
    // Whole items while one more and room for the last surely fit (an item is under 128
    // bytes), so the padding stays under one item.
    loop {
        let mut more = items.clone();
        more.push(item(items.len(), variant, ""));
        if text(&more).len() + 128 > cap {
            break;
        }
        items = more;
    }
    // The last item, its label padded to the exact length.
    let mut last = items.clone();
    last.push(item(items.len(), variant, ""));
    let pad = cap.saturating_sub(text(&last).len());
    let suffix: String = " notes".chars().cycle().take(pad).collect();
    items.push(item(items.len(), variant, &suffix));
    text(&items)
}

/// What priming measured: the prefix's prompt tokens and, for a reminder's cost model, the
/// tokens it evaluated and the time that took (`prompt_ms` is NaN when the server gave none).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Primed {
    /// Prompt tokens on an empty cache: the fixed prefix with the template and the minimal
    /// envelope (evaluated plus cached).
    pub prefix_tokens: f64,
    /// Tokens evaluated by the priming request.
    pub prompt_n: f64,
    pub prompt_ms: f64,
}

impl Primed {
    /// Tests: a priming of `tokens` evaluated at `ms_per_token`.
    #[cfg(test)]
    pub(crate) fn of(tokens: f64, ms_per_token: f64) -> Self {
        Self {
            prefix_tokens: tokens,
            prompt_n: tokens,
            prompt_ms: tokens * ms_per_token,
        }
    }
}

/// What priming and calibration measured, shown on the assistant page.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Calibration {
    /// Priming's prompt tokens on an empty cache: the fixed prefix (with the template and the
    /// minimal envelope).
    pub prefix_tokens: f64,
    /// Priming's own measurement (kept for the reminder budgets, plan 3c-1).
    pub primed: Primed,
    pub samples: [Sample; 2],
    /// Bytes per token of each filler (the density that sets N(B)).
    pub density: [f64; 2],
    pub derived: Result<Derived, NoBudgets>,
}

/// Why priming or a sample did not complete.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Interrupted {
    Priming(Failure),
    Sample(Failure),
    Tokenize(Failure),
    /// A sample ran past [`SAMPLE_TIMEOUT_S`]: R would exceed the ceiling at every bound, so
    /// it is "too slow", not a failure.
    SampleTimedOut {
        limit_s: u64,
    },
}

impl Interrupted {
    /// The failed exchange as the page names it (a status or "no well-formed answer").
    pub(crate) fn describe(self) -> String {
        let (what, failure) = match self {
            Self::Priming(failure) => ("the priming request", failure),
            Self::Sample(failure) => ("a calibration sample", failure),
            Self::Tokenize(failure) => ("/tokenize", failure),
            Self::SampleTimedOut { limit_s } => {
                return format!("a calibration sample took more than {limit_s} s");
            }
        };
        match failure {
            Failure::Status(status) => format!("{what} answered HTTP {status}"),
            Failure::Transport => format!("{what} got no well-formed answer"),
        }
    }
}

/// Primes the cache: the readiness gate. Returns what priming measured.
///
/// # Errors
///
/// The exchange's failure; a priming answer without its prompt tokens is a transport
/// failure.
pub(crate) async fn prime(server: &dyn Server) -> Result<Primed, Interrupted> {
    let timings = server
        .plain(&priming_envelope(), 1, false)
        .await
        .map_err(Interrupted::Priming)?;
    let evaluated = timings
        .prompt_n
        .ok_or(Interrupted::Priming(Failure::Transport))?;
    Ok(Primed {
        prefix_tokens: evaluated + timings.cache_n.unwrap_or(0.0),
        prompt_n: evaluated,
        prompt_ms: timings.prompt_ms.unwrap_or(f64::NAN),
    })
}

fn sample(timings: Timings, wall_ms: f64, filler_tokens: usize) -> Sample {
    let missing = f64::NAN;
    #[expect(
        clippy::cast_precision_loss,
        reason = "a token count of at most a 4 KiB filler"
    )]
    let filler_tokens = filler_tokens as f64;
    Sample {
        wall_ms,
        prompt_n: timings.prompt_n.unwrap_or(missing),
        prompt_ms: timings.prompt_ms.unwrap_or(missing),
        cached_n: timings.cache_n.unwrap_or(missing),
        predicted_n: timings.predicted_n.unwrap_or(missing),
        predicted_ms: timings.predicted_ms.unwrap_or(missing),
        filler_tokens,
    }
}

/// The two samples and the budgets they derive for replies of up to `max_tokens` (after
/// [`prime`]). `progress` is told which sample (1 or 2) begins.
///
/// # Errors
///
/// A failed exchange; invalid timings are not an error here but `derived`'s
/// [`NoBudgets::Invalid`].
pub(crate) async fn calibrate(
    server: &dyn Server,
    path: Path,
    max_tokens: u32,
    primed: Primed,
    progress: &(dyn Fn(u8) + Sync),
) -> Result<Calibration, Interrupted> {
    let cap = path.cap();
    let mut samples = Vec::with_capacity(2);
    let mut density = [0.0; 2];
    for (variant, density) in density.iter_mut().enumerate() {
        progress(u8::try_from(variant + 1).unwrap_or(u8::MAX));
        let text = filler(cap, variant);
        let tokens = server.tokens(&text).await.map_err(Interrupted::Tokenize)?;
        let started = tokio::time::Instant::now();
        let timings = tokio::time::timeout(
            std::time::Duration::from_secs(SAMPLE_TIMEOUT_S),
            server.plain(&text, SAMPLE_TOKENS, true),
        )
        .await
        .map_err(|_| Interrupted::SampleTimedOut {
            limit_s: SAMPLE_TIMEOUT_S,
        })?
        .map_err(Interrupted::Sample)?;
        let wall_ms = started.elapsed().as_secs_f64() * 1000.0;
        samples.push(sample(timings, wall_ms, tokens));
        #[expect(
            clippy::cast_precision_loss,
            reason = "byte and token counts of at most a 4 KiB filler"
        )]
        let bytes_per_token = cap as f64 / tokens.max(1) as f64;
        *density = bytes_per_token;
    }
    let prefix_tokens = primed.prefix_tokens;
    let samples = [samples[0], samples[1]];
    let derived = budgets::derive(&Measured {
        path,
        max_tokens,
        prefix_tokens,
        samples,
    });
    Ok(Calibration {
        prefix_tokens,
        primed,
        samples,
        density,
        derived,
    })
}

#[cfg(test)]
#[path = "calibration_tests.rs"]
mod tests;
