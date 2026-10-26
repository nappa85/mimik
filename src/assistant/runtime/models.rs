//! The owner's model setting at run time (`docs/model-runtime.md`, "Owner model setting"): the model a
//! generation runs, the trial that measures a candidate, and what the page shows of both.
use super::Path;
use super::calibration::Calibration;
use crate::assistant::jobs::Budgets;
use std::time::Duration;

/// A model file of the models folder as the runtime starts it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Model {
    /// The owner's spec (`local:<name>.gguf`).
    pub spec: String,
    /// The file's name in the models folder.
    pub name: String,
    /// The SHA-256 the file must have at every load (64 lowercase hex digits).
    pub sha256: String,
    /// The server's `--alias` and the requests' model name.
    pub alias: String,
    pub max_tokens: u32,
    /// The file's size when it was hashed.
    pub size: Option<u64>,
}

/// What the launcher learns of a file by hashing it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ModelFile {
    pub sha256: String,
    pub size: u64,
}

/// Identifies a finished trial: its number, the generation it ran in and the candidate's hash.
/// "Switch" accepts only the latest finished trial's key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TrialKey {
    pub number: u64,
    pub generation: u64,
    pub sha256: String,
}

impl std::fmt::Display for TrialKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.number, self.generation, self.sha256)
    }
}

impl std::str::FromStr for TrialKey {
    type Err = ();

    fn from_str(text: &str) -> Result<Self, ()> {
        let mut parts = text.split('.');
        let (Some(number), Some(generation), Some(sha256), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(());
        };
        let number_ok = |part: &str| {
            !part.is_empty() && part.len() <= 18 && part.bytes().all(|b| b.is_ascii_digit())
        };
        if !number_ok(number)
            || !number_ok(generation)
            || !crate::assistant::model::valid_sha256(sha256)
        {
            return Err(());
        }
        Ok(Self {
            number: number.parse().map_err(|_| ())?,
            generation: generation.parse().map_err(|_| ())?,
            sha256: sha256.to_owned(),
        })
    }
}

/// How one synthetic phrase went.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PhraseResult {
    pub note: &'static str,
    /// The reply's generated tokens.
    pub predicted_n: f64,
    pub seconds: f64,
    /// Whether the expected value was among the options (information only, D-D).
    pub expected_offered: bool,
}

/// A passed trial: what it measured, ready to be switched to.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TrialReport {
    pub key: TrialKey,
    pub by: i64,
    /// The candidate with the `max_tokens` the trial derived.
    pub model: Model,
    pub path: Path,
    pub priming: Duration,
    pub phrases: Vec<PhraseResult>,
    /// The largest `predicted_n` of the phrases, which sets `max_tokens`.
    pub largest_predicted: u32,
    pub calibration: Calibration,
    pub budgets: Budgets,
    pub elapsed: Duration,
}

/// The latest trial, as the page shows it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum TrialState {
    Running {
        number: u64,
        spec: String,
        phase: &'static str,
    },
    Passed(Box<TrialReport>),
    Failed {
        number: u64,
        spec: String,
        why: String,
    },
    Cancelled {
        number: u64,
        spec: String,
        why: &'static str,
    },
}

impl TrialState {
    pub(crate) fn running(&self) -> bool {
        matches!(self, Self::Running { .. })
    }
}

/// The model setting as the page shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ModelStatus {
    pub active: Model,
    pub previous: Option<Model>,
    /// The active model comes from the owner's stored setting, not from the start flags.
    pub from_owner_setting: bool,
    /// The owner can try and switch models (not with the development endpoint).
    pub switchable: bool,
    /// The next trial's number.
    pub next_trial: u64,
    /// A model change (a Try, a Switch, a Switch back) is admitted and not yet carried out.
    pub pending: bool,
    /// What the last switch or switch back could not do.
    pub note: Option<String>,
}

/// The whole trial may take this long: priming, five phrases at their deadline and two
/// calibration samples at twice the cap.
pub(crate) fn trial_bound(path: Path) -> Duration {
    Duration::from_secs(match path {
        Path::Cpu => 30 * 60,
        Path::Gpu => 20 * 60,
    })
}

/// Each phrase has twice the path's run-deadline cap (180 s on the CPU, 120 s on the GPU).
pub(crate) fn phrase_bound(path: Path) -> Duration {
    Duration::from_secs(2 * path.run_cap())
}

/// The model the start flags describe, for a manager that has no stored setting.
impl Model {
    #[cfg(test)]
    pub(crate) fn test(name: &str, sha256: &str, max_tokens: u32) -> Self {
        Self {
            spec: format!("local:{name}"),
            name: name.to_owned(),
            sha256: sha256.to_owned(),
            alias: name.trim_end_matches(".gguf").to_ascii_lowercase(),
            max_tokens,
            size: Some(1 << 20),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trial_keys_round_trip_and_refuse_anything_else() {
        let key = TrialKey {
            number: 3,
            generation: 17,
            sha256: "a".repeat(64),
        };
        assert_eq!(key.to_string().parse(), Ok(key.clone()));
        for refused in [
            "",
            "3.17",
            "3.17.",
            &format!("3.17.{}.x", "a".repeat(64)),
            &format!("x.17.{}", "a".repeat(64)),
            &format!("3..{}", "a".repeat(64)),
            &format!("3.17.{}", "A".repeat(64)),
            &format!("3.17.{}", "a".repeat(63)),
            &format!("{}.17.{}", "9".repeat(19), "a".repeat(64)),
            &format!("-3.17.{}", "a".repeat(64)),
        ] {
            assert_eq!(refused.parse::<TrialKey>(), Err(()), "{refused:?}");
        }
    }

    #[test]
    fn bounds_follow_the_path() {
        assert_eq!(phrase_bound(Path::Cpu), Duration::from_mins(3));
        assert_eq!(phrase_bound(Path::Gpu), Duration::from_mins(2));
        assert_eq!(trial_bound(Path::Cpu), Duration::from_mins(30));
        assert_eq!(trial_bound(Path::Gpu), Duration::from_mins(20));
    }
}
