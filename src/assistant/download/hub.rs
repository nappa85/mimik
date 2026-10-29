//! Strict admission of the Hugging Face API's answer for one file at one commit
//! (`docs/model-downloads.md`). Pure: bytes in, typed facts out; nothing here touches the
//! network. Anything missing or malformed refuses the review.
use crate::assistant::model::{HfSpec, MAX_MODEL_BYTES};
use serde_json::Value;

/// The largest metadata answer read (1 MiB).
pub(crate) const MAX_METADATA_BYTES: usize = 1 << 20;
/// The largest licence file read (1 MiB).
pub(crate) const MAX_LICENCE_BYTES: usize = 1 << 20;

/// A refused review: the reason, in words for the owner. Never holds a URL.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub(crate) struct Refusal(pub String);

fn refuse<T>(why: &str) -> Result<T, Refusal> {
    Err(Refusal(why.to_owned()))
}

/// What the Hub says of one file at one commit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Facts {
    pub commit: String,
    pub sha256: String,
    pub size: u64,
    /// The card's `license` (1 to 64 of `[A-Za-z0-9._+-]`), when it states one.
    pub licence_id: Option<String>,
    /// The licence file to fetch, if any.
    pub licence_file: Option<String>,
    /// An external link the card names, shown as text only.
    pub licence_link: Option<String>,
    /// The size the listing gives the licence file, when it does.
    pub licence_size: Option<u64>,
}

/// Root file names tried when the card names no repository file.
const ROOT_LICENCES: [&str; 3] = ["LICENSE", "LICENSE.md", "LICENSE.txt"];

/// The grammar of a repository-relative licence path: segments of 1 to 128 `[A-Za-z0-9._-]`,
/// none `.` or `..`, joined by `/`, and no `..` anywhere (the stored row's table check is the
/// same rule).
pub(crate) fn valid_repo_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 512
        && path.split('/').all(|segment| {
            (1..=128).contains(&segment.len())
                && segment
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
                && segment != "."
                && !segment.contains("..")
        })
}

fn valid_sha256(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// A licence ID: 1 to 64 of `[A-Za-z0-9._+-]`.
pub(crate) fn valid_licence_id(text: &str) -> bool {
    (1..=64).contains(&text.len())
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'+' | b'-'))
}

/// A whole number from a JSON value that is an unsigned integer (not a float or a string).
fn integer(value: &Value) -> Option<u64> {
    value.as_u64()
}

type Object = serde_json::Map<String, Value>;

/// The repository must be neither private nor gated (those are downloaded by hand).
fn open_access(root: &Object) -> Result<(), Refusal> {
    match root.get("private") {
        Some(Value::Bool(false)) => {}
        Some(Value::Bool(true)) => {
            return refuse(
                "the repository is private: download the file by hand and use local:<name>.gguf",
            );
        }
        _ => return refuse("the repository's metadata does not say whether it is private"),
    }
    match root.get("gated") {
        Some(Value::Bool(false)) => Ok(()),
        Some(_) => {
            refuse("the repository is gated: download the file by hand and use local:<name>.gguf")
        }
        None => refuse("the repository's metadata does not say whether it is gated"),
    }
}

/// The SHA-256 and size of the listed file, from its LFS data.
fn lfs_of(file: &Value) -> Result<(String, u64), Refusal> {
    let Some(lfs) = file.get("lfs").and_then(Value::as_object) else {
        return refuse("the file has no LFS data, so its SHA-256 is not known");
    };
    let sha256 = lfs
        .get("sha256")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !valid_sha256(sha256) {
        return refuse("the file's SHA-256 is missing or malformed");
    }
    let Some(size) = lfs.get("size").and_then(integer) else {
        return refuse("the file's size is missing or malformed");
    };
    if file
        .get("size")
        .is_some_and(|listed| integer(listed) != Some(size))
    {
        return refuse("the file's two sizes differ");
    }
    if size == 0 || size > MAX_MODEL_BYTES {
        return refuse("the file is empty or larger than 8 GiB");
    }
    Ok((sha256.to_owned(), size))
}

/// What the card and the listing say of the licence: its ID, the file to fetch, the external
/// link to show, and the listed file's size.
struct LicenceFacts {
    id: Option<String>,
    file: Option<String>,
    link: Option<String>,
    size: Option<u64>,
}

fn licence_of<'a>(
    card: Option<&Object>,
    listed: &dyn Fn(&str) -> Option<&'a Value>,
) -> Result<LicenceFacts, Refusal> {
    let text = |key: &str| card.and_then(|card| card.get(key));
    let id = match text("license") {
        None | Some(Value::Null) => None,
        Some(Value::String(id)) if valid_licence_id(id) => Some(id.clone()),
        Some(_) => return refuse("the card's licence is not a plain identifier"),
    };
    let mut file = None;
    let mut link = None;
    match text("license_link") {
        None | Some(Value::Null) => {}
        Some(Value::String(named)) => {
            if named.contains("://") || named.starts_with("//") || !valid_repo_path(named) {
                // External, or not a path we could fetch: shown as text, never fetched.
                link = Some(named.chars().take(200).collect());
            } else if listed(named).is_some() {
                file = Some(named.clone());
            } else {
                return refuse("the card names a licence file the repository does not list");
            }
        }
        Some(_) => return refuse("the card's licence link is not a string"),
    }
    if file.is_none() {
        file = ROOT_LICENCES
            .iter()
            .find(|name| listed(name).is_some())
            .map(|name| (*name).to_owned());
    }
    let size = file
        .as_deref()
        .and_then(listed)
        .and_then(|entry| entry.get("size"))
        .and_then(integer);
    if size.is_some_and(|size| size > MAX_LICENCE_BYTES as u64) {
        return refuse("the licence file is larger than 1 MiB");
    }
    Ok(LicenceFacts {
        id,
        file,
        link,
        size,
    })
}

/// Admits the answer of `GET /api/models/<org>/<repo>/revision/<revision>?blobs=true`.
///
/// # Errors
///
/// The first thing missing or malformed, by message.
pub(crate) fn admit(body: &[u8], spec: &HfSpec) -> Result<Facts, Refusal> {
    if body.len() > MAX_METADATA_BYTES {
        return refuse("the repository's metadata is larger than 1 MiB");
    }
    let Ok(json) = serde_json::from_slice::<Value>(body) else {
        return refuse("the repository's metadata is not JSON");
    };
    let Some(root) = json.as_object() else {
        return refuse("the repository's metadata is not an object");
    };
    let commit = root.get("sha").and_then(Value::as_str).unwrap_or_default();
    if commit != spec.revision {
        return refuse("the repository's commit is not the revision named");
    }
    open_access(root)?;
    let Some(siblings) = root.get("siblings").and_then(Value::as_array) else {
        return refuse("the repository lists no files");
    };
    let listed = |name: &str| {
        siblings
            .iter()
            .find(|entry| entry.get("rfilename").and_then(Value::as_str) == Some(name))
    };
    let Some(file) = listed(&spec.path) else {
        return refuse("the file is not in the repository at that revision");
    };
    let (sha256, size) = lfs_of(file)?;
    let licence = licence_of(root.get("cardData").and_then(Value::as_object), &listed)?;
    Ok(Facts {
        commit: commit.to_owned(),
        sha256,
        size,
        licence_id: licence.id,
        licence_file: licence.file,
        licence_link: licence.link,
        licence_size: licence.size,
    })
}

#[cfg(test)]
#[path = "hub_tests.rs"]
mod tests;
