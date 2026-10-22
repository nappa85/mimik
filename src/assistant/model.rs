//! The model spec of 2c (assisted sheet plan §1.4b, "Before 2d"): `--assistant-model`
//! (`local:<name>.gguf`, default the Granite 4.2 3B file with its SHA-256 compiled in),
//! `--assistant-model-sha256` (required for any other file) and `--assistant-model-alias`.
/// The default model spec (§1.4b) and the SHA-256 compiled in for it: the Granite 4.2 3B
/// `Q4_K_M` file, equal to the Hub's LFS SHA-256 at revision `c40945d…`.
pub const DEFAULT_MODEL: &str = "local:granite-4.2-3b-Q4_K_M.gguf";
pub const DEFAULT_MODEL_SHA256: &str =
    "e0406663965846ae22a403456eb826ccce5f450840491f71952f18a7cb78e7d5";
/// `max_tokens` of the default file under the adopted prefix, by the 2b rule (round up to a
/// multiple of 32 of 1.25 × the largest `predicted_n` of a valid, untruncated reply, at most
/// 384) over the 2c-1 screen of that prefix (v2-short): largest 263, 1.25 × 263 = 328.75, so
/// 352.
pub(crate) const DEFAULT_MAX_TOKENS: u32 = 352;
/// Any other file until the 2d trial measures it (never below 256, §1.4b).
pub(crate) const OTHER_MAX_TOKENS: u32 = 256;

/// The model's file name from a `local:` spec (the 2d-1 grammar's `local:` form).
pub(crate) fn local_name(spec: &str) -> Result<&str, String> {
    let name = spec
        .strip_prefix("local:")
        .ok_or("the model must be a local:<name>.gguf spec in this build")?;
    let valid = (1..=128).contains(&name.len())
        && name
            .rsplit_once('.')
            .is_some_and(|(_, extension)| extension == "gguf")
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
    if valid {
        Ok(name)
    } else {
        Err("the model spec is not local:<name>.gguf with [A-Za-z0-9._-] characters".into())
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

/// The model's `max_tokens`: 352 for the default file by its SHA-256, 256 otherwise.
pub(crate) fn max_tokens(spec: &str, sha256: Option<&str>) -> Result<u32, String> {
    local_name(spec)?;
    let sha256 = match sha256 {
        Some(sha)
            if sha.len() == 64 && sha.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) =>
        {
            sha
        }
        Some(_) => return Err("--assistant-model-sha256 must be 64 lowercase hex digits".into()),
        None if spec == DEFAULT_MODEL => DEFAULT_MODEL_SHA256,
        None => return Err("--assistant-model-sha256 is required for this model".into()),
    };
    Ok(if sha256 == DEFAULT_MODEL_SHA256 {
        DEFAULT_MAX_TOKENS
    } else {
        OTHER_MAX_TOKENS
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_specs_aliases_and_max_tokens() {
        assert_eq!(
            default_alias(DEFAULT_MODEL).unwrap(),
            "granite-4.2-3b-q4_k_m"
        );
        assert_eq!(max_tokens(DEFAULT_MODEL, None), Ok(352));
        assert_eq!(
            max_tokens(DEFAULT_MODEL, Some(DEFAULT_MODEL_SHA256)),
            Ok(352)
        );
        // Whatever its name, another file is 256.
        assert_eq!(max_tokens(DEFAULT_MODEL, Some(&"a".repeat(64))), Ok(256));
        assert_eq!(
            max_tokens("local:other.gguf", Some(DEFAULT_MODEL_SHA256)),
            Ok(352)
        );
        assert!(max_tokens("local:other.gguf", None).is_err());
        assert!(max_tokens("local:other.gguf", Some("ABC")).is_err());
        for refused in [
            "hf:x/y/z.gguf@0",
            "local:.hidden.gguf",
            "local:a/b.gguf",
            "local:x.bin",
            "local:",
        ] {
            assert!(default_alias(refused).is_err(), "{refused}");
        }
    }
}
