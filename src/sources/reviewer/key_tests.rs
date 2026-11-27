//! The key file (docs/api-reviewer-plan.md step 3): the `secrets/` folder's and the file's
//! modes, the atomic write, replace and delete, the bounded read and the in-memory identity.
//! The key is a dummy; no environment key is read.
use super::key::{self, DIRECTORY, KEY_FILE, KEY_MAX_BYTES};
use crate::Store;
use sha2::{Digest, Sha256};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::Duration;

const KEY: &str = "dummy-key-not-a-secret";
const OTHER: &str = "another-dummy-key-2";

fn store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("private.sqlite3")).unwrap();
    (dir, store)
}

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

fn key_path(dir: &tempfile::TempDir) -> std::path::PathBuf {
    dir.path().join(DIRECTORY).join(KEY_FILE)
}

/// Every 8-character substring of `key` (the secrecy tests' search set).
fn substrings(key: &str) -> Vec<&str> {
    let bytes = key.as_bytes();
    (0..=bytes.len().saturating_sub(8))
        .map(|start| &key[start..start + 8])
        .collect()
}

#[test]
fn the_secrets_folder_is_0700_and_the_key_file_0600_with_no_temp_left() {
    let (dir, store) = store();
    let secrets_dir = dir.path().join(DIRECTORY);
    assert_eq!(mode(&secrets_dir), 0o700, "the folder Mimik made is 0700");
    assert_eq!(store.secrets().status().unwrap(), None);
    let status = store.secrets().write(KEY.as_bytes()).unwrap();
    assert!(status.changed_at > 0);
    assert_eq!(mode(&key_path(&dir)), 0o600, "the key file is 0600");
    assert_eq!(store.secrets().status().unwrap(), Some(status));
    // The atomic write leaves no temporary file behind.
    let left: Vec<String> = std::fs::read_dir(&secrets_dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(left, vec![KEY_FILE.to_owned()]);
    // The file holds exactly the key.
    assert_eq!(std::fs::read(key_path(&dir)).unwrap(), KEY.as_bytes());
}

#[test]
fn replace_changes_the_file_and_delete_removes_it() {
    let (dir, store) = store();
    let first = store.secrets().write(KEY.as_bytes()).unwrap();
    let first_identity = key::key_identity(&store).unwrap().expect("the key is set");
    assert_eq!(
        first_identity.digest,
        *Sha256::digest(KEY.as_bytes()).as_slice()
    );
    // Consistency only: the identity's change time equals the status's, both describing the
    // same file state. This assertion does not test the same-descriptor binding.
    assert_eq!(
        first_identity.changed_at,
        store.secrets().status().unwrap().unwrap().changed_at
    );
    // ctime has sub-second resolution; make the change unambiguous anyway.
    std::thread::sleep(Duration::from_millis(20));
    let second = store.secrets().write(OTHER.as_bytes()).unwrap();
    assert_eq!(std::fs::read(key_path(&dir)).unwrap(), OTHER.as_bytes());
    assert_eq!(store.secrets().status().unwrap(), Some(second));
    let second_identity = key::key_identity(&store).unwrap().unwrap();
    assert_eq!(
        second_identity.digest,
        *Sha256::digest(OTHER.as_bytes()).as_slice()
    );
    assert_ne!(
        first_identity, second_identity,
        "the identity follows the file"
    );
    assert!(second.changed_at >= first.changed_at);
    assert!(store.secrets().delete().unwrap());
    assert!(!key_path(&dir).exists());
    assert_eq!(store.secrets().status().unwrap(), None);
    assert_eq!(store.secrets().read().unwrap(), None);
    assert_eq!(key::key_identity(&store).unwrap(), None);
    assert!(
        !store.secrets().delete().unwrap(),
        "deleting again reports not set"
    );
}

#[test]
fn the_write_refuses_empty_overlong_and_nul_keys_and_leaves_no_file() {
    let (dir, store) = store();
    for bad in ["", &"k".repeat(KEY_MAX_BYTES + 1), "with\0nul"] {
        assert!(store.secrets().write(bad.as_bytes()).is_err(), "{bad:?}");
    }
    assert!(!key_path(&dir).exists());
    assert_eq!(store.secrets().status().unwrap(), None);
    // Exactly at the bound is fine.
    let longest = "k".repeat(KEY_MAX_BYTES);
    assert!(store.secrets().write(longest.as_bytes()).is_ok());
    assert_eq!(
        store
            .secrets()
            .read()
            .unwrap()
            .as_deref()
            .map(Vec::as_slice),
        Some(longest.as_bytes())
    );
}

/// The save rule: after trimming the ends, only visible ASCII (0x21 to 0x7E) — a line break, a
/// space, a DEL or a non-ASCII character in the middle is refused, and the trimmed key is what is
/// stored.
#[test]
fn the_write_accepts_only_visible_ascii_after_trimming_the_ends() {
    let (dir, store) = store();
    for bad in [
        "ab\ncd",
        "ab\rcd",
        "ab\tcd",
        "ab cd",
        "ab\x7Fcd",
        "ab\u{e9}cd",
        "ab\u{2603}cd",
        &"k".repeat(KEY_MAX_BYTES + 1),
    ] {
        let error = store.secrets().write(bad.as_bytes()).unwrap_err();
        assert!(
            error.to_string().contains("visible ASCII"),
            "{bad:?}: {error}"
        );
        assert!(!key_path(&dir).exists(), "{bad:?}");
    }
    // The ends are trimmed (whitespace is not part of the key) and the trimmed key is stored.
    store.secrets().write(b"  padded-key-1 \n").unwrap();
    assert_eq!(std::fs::read(key_path(&dir)).unwrap(), b"padded-key-1");
    assert_eq!(mode(&key_path(&dir)), 0o600);
}

/// The child of the umask test: writes under umask 0o277 and checks the file is still 0600 (the
/// temporary file is `fchmod`ed, so the mode does not depend on the umask). Runs only in a child
/// process, where changing the process-wide umask is safe.
#[test]
#[ignore = "run in a child process by the parent test"]
fn child_write_under_umask_0277() {
    if std::env::var_os("MIMIK_KEY_UMASK_CHILD").is_none() {
        return;
    }
    let (dir, store) = store();
    // SAFETY: the child process is single-threaded (`--test-threads=1`) and short-lived.
    unsafe { libc::umask(0o277) };
    store.secrets().write(KEY.as_bytes()).unwrap();
    assert_eq!(
        mode(&key_path(&dir)),
        0o600,
        "the key file is 0600 under umask 0o277"
    );
    assert_eq!(mode(&dir.path().join(DIRECTORY)), 0o700);
}

/// The key file is 0600 whatever the umask: the test re-runs itself in a child process with
/// umask 0o277 (the umask is process-wide, so it is not changed in the test process itself).
#[test]
fn the_key_file_is_0600_under_umask_0277() {
    let name = "sources::reviewer::key_tests::child_write_under_umask_0277";
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            name,
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("MIMIK_KEY_UMASK_CHILD", "1")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}");
    assert!(
        stdout.contains("1 passed"),
        "the child test ran and passed: {stdout}"
    );
}

#[test]
fn a_key_file_that_is_not_private_is_refused() {
    let (dir, store) = store();
    store.secrets().write(KEY.as_bytes()).unwrap();
    // Group-readable: the status, read and identity all refuse it.
    std::fs::set_permissions(key_path(&dir), std::fs::Permissions::from_mode(0o640)).unwrap();
    assert!(store.secrets().status().is_err());
    assert!(store.secrets().read().is_err());
    assert!(key::key_identity(&store).is_err());
    // A link in place of the file is refused too.
    std::fs::set_permissions(key_path(&dir), std::fs::Permissions::from_mode(0o600)).unwrap();
    let link = dir.path().join(DIRECTORY).join("link");
    std::os::unix::fs::symlink(key_path(&dir), &link).unwrap();
    std::fs::remove_file(key_path(&dir)).unwrap();
    std::os::unix::fs::symlink(&link, key_path(&dir)).unwrap();
    assert!(store.secrets().status().is_err(), "a link is refused");
    assert!(store.secrets().read().is_err(), "a link is refused");
    // A too-long file (hand-placed, 0600) is refused by the read bound.
    std::fs::remove_file(key_path(&dir)).unwrap();
    std::fs::write(key_path(&dir), "k".repeat(KEY_MAX_BYTES + 1)).unwrap();
    std::fs::set_permissions(key_path(&dir), std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(store.secrets().read().is_err());
    assert!(key::key_identity(&store).is_err());
}

/// The read rule has no trimming at read time: the stored bytes themselves must satisfy the
/// save rule, so a 0600 file holding a key with a leading or trailing line break (or space) is
/// read as stored but is not a ready key.
#[test]
fn a_stored_key_with_leading_or_trailing_whitespace_is_not_ready() {
    let (dir, store) = store();
    for stored in ["key\n", "\nkey", "key ", " key", "key\r\n"] {
        std::fs::write(key_path(&dir), stored.as_bytes()).unwrap();
        std::fs::set_permissions(key_path(&dir), std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(
            store
                .secrets()
                .read()
                .unwrap()
                .as_deref()
                .map(Vec::as_slice),
            Some(stored.as_bytes()),
            "{stored:?}: the bytes are read as stored, without trimming"
        );
        assert!(!store.secrets().ready().unwrap(), "{stored:?}: not ready");
    }
}

#[tokio::test]
async fn the_key_never_reaches_the_database_file() {
    let (dir, store) = store();
    store.secrets().write(KEY.as_bytes()).unwrap();
    let valid = super::settings::validate("mistral", "https://api.mistral.ai", "m").unwrap();
    store
        .run(move |db| super::settings::Settings::save(db, 1, &valid))
        .await
        .unwrap();
    let database = std::fs::read(dir.path().join("private.sqlite3")).unwrap();
    for piece in substrings(KEY) {
        assert!(
            !contains(&database, piece.as_bytes()),
            "the database file holds {piece:?}"
        );
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}
