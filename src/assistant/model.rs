//! The model spec (`docs/model-runtime.md`, "Owner model setting"): the grammar of `--assistant-model` and of the
//! owner's "Model" field (`local:<name>.gguf`, default the Granite 4.2 3B file with its
//! SHA-256 compiled in), `--assistant-model-sha256` (required for any other file) and
//! `--assistant-model-alias`, and the `max_tokens` rules.
/// The default model spec and the SHA-256 compiled in for it: the Granite 4.2 3B
/// `Q4_K_M` file, equal to the Hub's LFS SHA-256 at revision `c40945d…`.
pub const DEFAULT_MODEL: &str = "local:granite-4.2-3b-Q4_K_M.gguf";
pub const DEFAULT_MODEL_SHA256: &str =
    "e0406663965846ae22a403456eb826ccce5f450840491f71952f18a7cb78e7d5";
/// `max_tokens` of the default file, by the 2b rule (round up to a multiple of 32 of 1.25 ×
/// the largest `predicted_n` of a valid, untruncated reply, at most 384) over the 2c-1
/// screen of the v2-short prefix: largest 263, 1.25 × 263 = 328.75, so 352. The shipped
/// v2-compact prefix (ASSIST-52) asks for shorter replies, so 352 still holds; re-deriving it
/// from a screen of the shipped prefix is owed (`TODO.md`).
pub(crate) const DEFAULT_MAX_TOKENS: u32 = 352;
/// The default model on the GPU path (ASSIST-55): the Qwen3.5-9B `Q4_K_M` file
/// (unsloth/Qwen3.5-9B-GGUF at revision `3885219b`, Apache-2.0), 5 680 522 464 bytes, with
/// its SHA-256 compiled in. The CPU path keeps [`DEFAULT_MODEL`].
pub const GPU_DEFAULT_MODEL: &str = "local:Qwen3.5-9B-Q4_K_M.gguf";
pub const GPU_DEFAULT_MODEL_SHA256: &str =
    "03b74727a860a56338e042c4420bb3f04b2fec5734175f4cb9fa853daf52b7e8";
/// `max_tokens` of the 9B file: 352, from the 352-token field-reference screens, where no
/// reply was truncated. A derivation by the 2b rule from a screen of the shipped
/// v2-compact prefix is owed (ASSIST-L47), as for Granite.
pub(crate) const GPU_DEFAULT_MAX_TOKENS: u32 = 352;
/// Any other file until the 2d trial measures it (never below 256).
pub(crate) const OTHER_MAX_TOKENS: u32 = 256;

/// The Hugging Face spec and size of the Granite default file (ASSIST-55): the downloads fetch
/// exactly this file, checked against [`DEFAULT_MODEL_SHA256`].
pub(crate) const GRANITE_HF: &str = "hf:ibm-granite/granite-4.2-3b-GGUF/granite-4.2-3b-Q4_K_M.gguf@c40945d71cd90f249a56985e8155551a9188dc30";
pub(crate) const GRANITE_SIZE: u64 = 2_244_011_552;
/// The same for the Qwen3.5-9B default file, checked against [`GPU_DEFAULT_MODEL_SHA256`].
pub(crate) const QWEN_HF: &str =
    "hf:unsloth/Qwen3.5-9B-GGUF/Qwen3.5-9B-Q4_K_M.gguf@3885219b6810b007914f3a7950a8d1b469d598a5";
pub(crate) const QWEN_SIZE: u64 = 5_680_522_464;

/// A default file the downloads can fetch: its `hf:` spec, the `local:` name it is stored
/// under, and the SHA-256 and size compiled into Mimik. Built from the existing constants.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Pinned {
    pub hf: &'static str,
    pub local: &'static str,
    pub sha256: &'static str,
    pub size: u64,
}

/// The two pinned default files, the CPU path's first.
pub(crate) fn pinned() -> [Pinned; 2] {
    [
        Pinned {
            hf: GRANITE_HF,
            local: DEFAULT_MODEL,
            sha256: DEFAULT_MODEL_SHA256,
            size: GRANITE_SIZE,
        },
        Pinned {
            hf: QWEN_HF,
            local: GPU_DEFAULT_MODEL,
            sha256: GPU_DEFAULT_MODEL_SHA256,
            size: QWEN_SIZE,
        },
    ]
}

impl Pinned {
    /// The file name inside the models folder (the `local:` spec without its prefix).
    pub(crate) fn file_name(&self) -> &'static str {
        self.local.strip_prefix("local:").unwrap_or(self.local)
    }
}

/// A validated `hf:<org>/<repo>/<path>@<revision>` spec: a file at an exact commit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HfSpec {
    pub org: String,
    pub repo: String,
    /// One or more `/`-joined segments, ending `.gguf`.
    pub path: String,
    /// Exactly 40 lowercase hex digits.
    pub revision: String,
}

impl HfSpec {
    /// The spec as the owner writes it.
    #[must_use]
    pub fn canonical(&self) -> String {
        format!(
            "hf:{}/{}/{}@{}",
            self.org, self.repo, self.path, self.revision
        )
    }
}

/// A validated model spec.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Spec {
    /// `local:<name>.gguf`: a file already in the models folder.
    Local(String),
    /// `hf:<org>/<repo>/<path>@<revision>`: a file to download (step 2d-2). The model table
    /// never holds it: a downloaded file is `local:hf-<sha256>.gguf`.
    Hf(HfSpec),
}

/// The grammar rule a refused spec broke; `Display` names it for the
/// owner, who keeps the input to correct it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rule {
    TooLong,
    NotAscii,
    Whitespace,
    /// An `hf:` spec that is not `hf:<org>/<repo>/<path>@<revision>`.
    HfForm,
    HfOwner,
    HfRepo,
    HfPath,
    HfExtension,
    HfSplit,
    HfRevision,
    Form,
    NameLength,
    PathSeparator,
    Charset,
    LeadingDot,
    Extension,
    DeviceName,
}

/// A refused model spec.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpecError {
    pub rule: Rule,
}

impl std::fmt::Display for SpecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self.rule {
            Rule::TooLong => "the model spec is longer than 512 bytes",
            Rule::NotAscii => "the model spec must be ASCII",
            Rule::Whitespace => "the model spec must not contain white space",
            Rule::HfForm => "an hf: spec must be hf:<org>/<repo>/<path>.gguf@<40-digit revision>",
            Rule::HfOwner => "the owner must be 1 to 96 of letters, digits, '.', '_' and '-', starting and ending with a letter or digit, without '--' or '..'",
            Rule::HfRepo => "the repository must be 1 to 96 of letters, digits, '.', '_' and '-', starting and ending with a letter or digit, without '--' or '..'",
            Rule::HfPath => "the file path must be segments of 1 to 128 of letters, digits, '.', '_' and '-', joined by '/', none '.' or '..'",
            Rule::HfExtension => "the file must end with .gguf",
            Rule::HfSplit => "a split model file (-00001-of-00002.gguf) is not supported",
            Rule::HfRevision => "the revision must be exactly 40 lowercase hex digits (a commit, not a branch or tag)",
            Rule::Form => "the model spec must be local:<name>.gguf",
            Rule::NameLength => "the model file name must be 1 to 128 characters",
            Rule::PathSeparator => "the model file name must be one name, without / or \\",
            Rule::Charset => "the model file name may use only letters, digits, '.', '_' and '-'",
            Rule::LeadingDot => "the model file name must not start with '.'",
            Rule::Extension => "the model file name must end with .gguf",
            Rule::DeviceName => "the model file name must not be a Windows device name (CON, NUL, COM1 and so on)",
        })
    }
}

impl std::error::Error for SpecError {}

/// Windows device names, refused in any case with or without an extension.
fn device_name(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name).to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || stem
            .strip_prefix("COM")
            .or_else(|| stem.strip_prefix("LPT"))
            .is_some_and(|n| matches!(n, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9"))
}

/// Parses the owner's model spec by the grammar: at most 512 bytes, ASCII, no white
/// space, and `local:<name>.gguf` with one name of 1 to 128 characters from `[A-Za-z0-9._-]`,
/// not starting with `.`, not a Windows device name.
///
/// # Errors
///
/// The first rule the spec breaks.
pub fn parse_spec(text: &str) -> Result<Spec, SpecError> {
    let refuse = |rule| Err(SpecError { rule });
    if text.len() > 512 {
        return refuse(Rule::TooLong);
    }
    if !text.is_ascii() {
        return refuse(Rule::NotAscii);
    }
    if text
        .bytes()
        .any(|b| b.is_ascii_whitespace() || b.is_ascii_control())
    {
        return refuse(Rule::Whitespace);
    }
    if let Some(rest) = text.strip_prefix("hf:") {
        return parse_hf(rest).map(Spec::Hf);
    }
    let Some(name) = text.strip_prefix("local:") else {
        return refuse(Rule::Form);
    };
    if !(1..=128).contains(&name.len()) {
        return refuse(Rule::NameLength);
    }
    if name.contains(['/', '\\']) {
        return refuse(Rule::PathSeparator);
    }
    if !name
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    {
        return refuse(Rule::Charset);
    }
    if name.starts_with('.') {
        return refuse(Rule::LeadingDot);
    }
    if std::path::Path::new(name)
        .extension()
        .is_none_or(|extension| extension != "gguf")
    {
        return refuse(Rule::Extension);
    }
    if device_name(name) {
        return refuse(Rule::DeviceName);
    }
    Ok(Spec::Local(name.to_owned()))
}

fn name_part(text: &str) -> bool {
    let bytes = text.as_bytes();
    (1..=96).contains(&bytes.len())
        && bytes
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        && bytes.first().is_some_and(u8::is_ascii_alphanumeric)
        && bytes.last().is_some_and(u8::is_ascii_alphanumeric)
        && !text.contains("--")
        && !text.contains("..")
}

fn path_segment(text: &str) -> bool {
    (1..=128).contains(&text.len())
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        && text != "."
        && text != ".."
}

/// `-NNNNN-of-NNNNN.gguf` at the end of a file name.
fn split_file(name: &str) -> bool {
    let Some(stem) = name.strip_suffix(".gguf") else {
        return false;
    };
    let bytes = stem.as_bytes();
    let Some(tail) = bytes.len().checked_sub(15).map(|start| &bytes[start..]) else {
        return false;
    };
    tail[0] == b'-'
        && tail[1..6].iter().all(u8::is_ascii_digit)
        && &tail[6..10] == b"-of-"
        && tail[10..].iter().all(u8::is_ascii_digit)
}

fn parse_hf(rest: &str) -> Result<HfSpec, SpecError> {
    let refuse = |rule| Err(SpecError { rule });
    let Some((location, revision)) = rest.rsplit_once('@') else {
        return refuse(Rule::HfForm);
    };
    let mut parts = location.splitn(3, '/');
    let (Some(org), Some(repo), Some(path)) = (parts.next(), parts.next(), parts.next()) else {
        return refuse(Rule::HfForm);
    };
    if revision.len() != 40
        || !revision
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return refuse(Rule::HfRevision);
    }
    if !name_part(org) {
        return refuse(Rule::HfOwner);
    }
    if !name_part(repo) {
        return refuse(Rule::HfRepo);
    }
    if !path.split('/').all(path_segment) {
        return refuse(Rule::HfPath);
    }
    let file = path.rsplit('/').next().unwrap_or(path);
    if file.strip_suffix(".gguf").is_none_or(str::is_empty) {
        return refuse(Rule::HfExtension);
    }
    if split_file(file) {
        return refuse(Rule::HfSplit);
    }
    Ok(HfSpec {
        org: org.to_owned(),
        repo: repo.to_owned(),
        path: path.to_owned(),
        revision: revision.to_owned(),
    })
}

/// The model's file name from a `local:` spec (the CLI flag and the owner's field share the
/// grammar).
pub(crate) fn local_name(spec: &str) -> Result<String, String> {
    match parse_spec(spec).map_err(|error| error.to_string())? {
        Spec::Local(name) => Ok(name),
        Spec::Hf(_) => Err(
            "an hf: spec names a download, not a file in the models folder; use local:<name>.gguf here"
                .to_owned(),
        ),
    }
}

/// The model alias derived from a spec: its file name without `.gguf`, lower case
/// (`granite-4.2-3b-q4_k_m` for the default).
///
/// # Errors
///
/// A spec that is not `local:<name>.gguf`, by message.
pub fn default_alias(spec: &str) -> Result<String, String> {
    let name = local_name(spec)?;
    Ok(name.trim_end_matches(".gguf").to_ascii_lowercase())
}

/// A SHA-256 as the specs and records write it: 64 lowercase hex digits.
pub(crate) fn valid_sha256(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// The SHA-256 and `max_tokens` a spec resolves to, before any trial: the hash given with
/// `--assistant-model-sha256`, else the compiled-in hash of the spec's own default file (the
/// Granite or the Qwen3.5-9B file); `max_tokens` 352 for either default file by its SHA-256,
/// 256 otherwise. Every place that turns a spec into a model (the flags, the development
/// endpoint, the probe) uses this.
pub(crate) fn resolve(spec: &str, sha256: Option<&str>) -> Result<(String, u32), String> {
    local_name(spec)?;
    let sha256 = match sha256 {
        Some(sha) if valid_sha256(sha) => sha,
        Some(_) => return Err("--assistant-model-sha256 must be 64 lowercase hex digits".into()),
        None if spec == DEFAULT_MODEL => DEFAULT_MODEL_SHA256,
        None if spec == GPU_DEFAULT_MODEL => GPU_DEFAULT_MODEL_SHA256,
        None => return Err("--assistant-model-sha256 is required for this model".into()),
    };
    let tokens = compiled_in_max_tokens(sha256).unwrap_or(OTHER_MAX_TOKENS);
    Ok((sha256.to_owned(), tokens))
}

/// The `max_tokens` of a file whose hash is compiled in (the two defaults), by its SHA-256.
fn compiled_in_max_tokens(sha256: &str) -> Option<u32> {
    match sha256 {
        DEFAULT_MODEL_SHA256 => Some(DEFAULT_MAX_TOKENS),
        GPU_DEFAULT_MODEL_SHA256 => Some(GPU_DEFAULT_MAX_TOKENS),
        _ => None,
    }
}

/// The largest model file a trial accepts (the stored size's limit, ASSIST-35).
pub(crate) const MAX_MODEL_BYTES: u64 = 8 << 30;

/// An alias the server and the stored setting accept: 1 to 128 characters from
/// `[A-Za-z0-9._-]`.
pub(crate) fn valid_alias(alias: &str) -> bool {
    (1..=128).contains(&alias.len())
        && alias
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// The `max_tokens` a trial's phrases are sent with (the measurement cap) and the most any
/// model may have.
pub(crate) const TRIAL_MEASURE_TOKENS: u32 = 384;
/// The least a trial sets (five phrases cannot measure a tail).
pub(crate) const MIN_TRIAL_TOKENS: u32 = 256;

/// The trial's rule: `min(384, max(256, round up to 32 of 1.25 × the largest
/// predicted_n))`, over the five phrases' replies. The default Granite file keeps
/// [`DEFAULT_MAX_TOKENS`], identified by its SHA-256, never by name
/// ([`trial_max_tokens`]).
pub(crate) fn measured_max_tokens(largest_predicted_n: u32) -> u32 {
    // 1.25 × n rounded up to a whole token, then to a multiple of 32.
    let scaled = u64::from(largest_predicted_n) * 5;
    let tokens = scaled.div_ceil(4);
    let rounded = tokens.div_ceil(32) * 32;
    let capped = rounded.clamp(u64::from(MIN_TRIAL_TOKENS), u64::from(TRIAL_MEASURE_TOKENS));
    u32::try_from(capped).unwrap_or(TRIAL_MEASURE_TOKENS)
}

/// The `max_tokens` a passed trial of the file with `sha256` records.
pub(crate) fn trial_max_tokens(sha256: &str, largest_predicted_n: u32) -> u32 {
    compiled_in_max_tokens(sha256).unwrap_or_else(|| measured_max_tokens(largest_predicted_n))
}

#[cfg(test)]
#[path = "model_tests.rs"]
mod tests;
