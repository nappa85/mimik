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

/// The page's note when the GPU path is chosen but the Qwen3.5-9B file is missing.
pub(crate) fn gpu_default_missing(folder: &str) -> String {
    format!("Qwen3.5-9B not found or not readable in {folder}; using Granite 4.2 3B")
}
/// The page's note when the 9B file is there but does not have its pinned hash.
pub(crate) fn gpu_default_mismatch(folder: &str) -> String {
    format!("Qwen3.5-9B in {folder} does not match its pinned hash; using Granite 4.2 3B")
}
/// The page's note when the GPU is wanted but the CPU path runs.
pub(crate) const GPU_DEFAULT_ON_CPU: &str = "Qwen3.5-9B is the default on the GPU only; \
     the GPU is not in use, so Granite 4.2 3B runs on the CPU";

/// The GPU default's file in the models folder, as far as the manager could tell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NineB {
    Present,
    /// Not there, or not openable as a model file.
    Missing,
    /// There, but without its pinned SHA-256.
    Mismatch,
}

impl Model {
    /// A compiled-in default (its spec resolves without a flag).
    pub(crate) fn compiled(spec: &str) -> Self {
        use crate::assistant::model::{default_alias, local_name, resolve};
        let (sha256, max_tokens) = resolve(spec, None).unwrap_or_default();
        Self {
            spec: spec.to_owned(),
            name: local_name(spec).unwrap_or_default(),
            sha256,
            alias: default_alias(spec).unwrap_or_default(),
            max_tokens,
            size: None,
        }
    }
}

/// The file name of the GPU default (the 9B) in the models folder.
pub(crate) fn gpu_default_name() -> String {
    crate::assistant::model::local_name(crate::assistant::model::GPU_DEFAULT_MODEL)
        .unwrap_or_default()
}

/// The default model by device (ASSIST-55; used only while neither a start flag nor the
/// owner's stored setting names a model): the Qwen3.5-9B on the GPU path when its file is in
/// the models folder with its pinned hash, Granite 4.2 3B otherwise, with the note the page
/// shows when the 9B is not what runs although the GPU is wanted. `gpu`: the path being
/// started is the GPU's; `gpu_wanted`: the GPU path is the first choice (not
/// `--assistant-device cpu`); `file`: the 9B file's state (read only when `gpu`); `folder`:
/// the models folder as the notes name it (its absolute path).
pub(crate) fn default_model(
    gpu: bool,
    gpu_wanted: bool,
    file: NineB,
    folder: &str,
) -> (Model, Option<String>) {
    use crate::assistant::model::{DEFAULT_MODEL, GPU_DEFAULT_MODEL};
    match (gpu, gpu_wanted, file) {
        (true, _, NineB::Present) => (Model::compiled(GPU_DEFAULT_MODEL), None),
        (true, _, NineB::Missing) => (
            Model::compiled(DEFAULT_MODEL),
            Some(gpu_default_missing(folder)),
        ),
        (true, _, NineB::Mismatch) => (
            Model::compiled(DEFAULT_MODEL),
            Some(gpu_default_mismatch(folder)),
        ),
        (false, true, _) => (
            Model::compiled(DEFAULT_MODEL),
            Some(GPU_DEFAULT_ON_CPU.to_owned()),
        ),
        (false, false, _) => (Model::compiled(DEFAULT_MODEL), None),
    }
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

/// Which prompt a model's stored measurement (its `max_tokens` and speeds) was made with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Measurement {
    /// Under the prompt in force, or never measured (a model the start flags named).
    Current,
    /// Under an older prompt (ASSIST-56).
    OlderPrompt,
}

impl Measurement {
    pub(crate) fn of(older: bool) -> Self {
        if older {
            Self::OlderPrompt
        } else {
            Self::Current
        }
    }

    pub(crate) fn is_older(self) -> bool {
        self == Self::OlderPrompt
    }
}

/// Whether the stored trials of the active and of the previous model were made with an older
/// prompt than the current one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct OlderPrompt {
    pub active: bool,
    pub previous: bool,
}

impl OlderPrompt {
    /// The pair after "Switch back": the two models change places.
    pub(crate) fn swapped(self) -> Self {
        Self {
            active: self.previous,
            previous: self.active,
        }
    }
}

/// The model setting as the page shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ModelStatus {
    pub active: Model,
    pub previous: Option<Model>,
    /// The active model's stored trial measured it with an older prompt than the current one
    /// (ASSIST-56): the page says so and the owner may try it again.
    pub older_prompt: Measurement,
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
    /// Whether the active model is the device default (ASSIST-55).
    pub device_default: DeviceDefault,
}

/// The device default (ASSIST-55) in the page's model status.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DeviceDefault {
    /// The active model is a flag's or the owner's.
    Off,
    /// The manager picks the model at each start; `note` says why it is not the 9B although
    /// the GPU is wanted.
    On { note: Option<String> },
}

impl DeviceDefault {
    pub(crate) fn is_on(&self) -> bool {
        matches!(self, Self::On { .. })
    }

    pub(crate) fn note(&self) -> Option<&str> {
        match self {
            Self::On { note } => note.as_deref(),
            Self::Off => None,
        }
    }
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
    fn the_default_model_follows_the_device() {
        use crate::assistant::model::{
            DEFAULT_MODEL_SHA256, GPU_DEFAULT_MODEL, GPU_DEFAULT_MODEL_SHA256,
        };
        // GPU path, file there: the 9B with its compiled-in hash and 352 tokens.
        let (model, note) = default_model(true, true, NineB::Present, "/m");
        assert_eq!(model.spec, GPU_DEFAULT_MODEL);
        assert_eq!(model.name, "Qwen3.5-9B-Q4_K_M.gguf");
        assert_eq!(model.sha256, GPU_DEFAULT_MODEL_SHA256);
        assert_eq!(
            (model.alias.as_str(), model.max_tokens),
            ("qwen3.5-9b-q4_k_m", 352)
        );
        assert_eq!(note, None);
        // GPU path, file with another hash: Granite, and why.
        let (model, note) = default_model(true, true, NineB::Mismatch, "/m");
        assert_eq!(model.sha256, DEFAULT_MODEL_SHA256);
        assert_eq!(
            note.as_deref(),
            Some("Qwen3.5-9B in /m does not match its pinned hash; using Granite 4.2 3B")
        );
        // GPU path, file missing: Granite, and why.
        let (model, note) = default_model(true, true, NineB::Missing, "/m");
        assert_eq!(model.sha256, DEFAULT_MODEL_SHA256);
        assert_eq!(
            note.as_deref(),
            Some("Qwen3.5-9B not found or not readable in /m; using Granite 4.2 3B")
        );
        // CPU path after a GPU failure: Granite, and why.
        let (model, note) = default_model(false, true, NineB::Present, "/m");
        assert_eq!(model.sha256, DEFAULT_MODEL_SHA256);
        assert_eq!(note, Some(GPU_DEFAULT_ON_CPU.to_owned()));
        // The CPU chosen: Granite, nothing to explain.
        let (model, note) = default_model(false, false, NineB::Missing, "/m");
        assert_eq!((model.sha256.as_str(), note), (DEFAULT_MODEL_SHA256, None));
    }

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
