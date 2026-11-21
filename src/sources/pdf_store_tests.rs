//! The PDF store's own guarantees: verified copies, no links, hash-checked opens, and the orphan
//! sweep with its lock and age rules.
use super::*;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};

fn sha(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn setup() -> (tempfile::TempDir, PdfStore) {
    let dir = tempfile::tempdir().unwrap();
    let store = PdfStore::in_directory(dir.path()).unwrap();
    (dir, store)
}

fn stored(dir: &tempfile::TempDir, hash: &str) -> std::path::PathBuf {
    dir.path().join(DIRECTORY).join(format!("{hash}{SUFFIX}"))
}

fn age(path: &Path, seconds: u64) {
    File::open(path)
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_secs(seconds))
        .unwrap();
}

#[test]
fn the_directory_is_private_and_files_are_read_only() {
    let (dir, store) = setup();
    let mode = std::fs::metadata(dir.path().join(DIRECTORY))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o700);
    let bytes = b"%PDF-1.4 one";
    let hash = sha(bytes);
    drop(store.put(&bytes[..], &hash).unwrap());
    let mode = std::fs::metadata(stored(&dir, &hash))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o400);
    let mut content = Vec::new();
    store
        .open(&hash)
        .unwrap()
        .read_to_end(&mut content)
        .unwrap();
    assert_eq!(content, bytes);
}

#[test]
fn a_directory_open_to_others_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join(DIRECTORY)).unwrap();
    std::fs::set_permissions(
        dir.path().join(DIRECTORY),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    assert!(PdfStore::in_directory(dir.path()).is_err());
}

#[test]
fn put_is_a_copy_never_a_link_and_is_idempotent() {
    let (dir, store) = setup();
    let staged = dir.path().join("document.pdf");
    let bytes = b"%PDF-1.4 staged";
    std::fs::write(&staged, bytes).unwrap();
    let hash = sha(bytes);
    for _ in 0..2 {
        drop(store.put(File::open(&staged).unwrap(), &hash).unwrap());
    }
    // The staged file keeps its single link, and the store file is another inode.
    assert_eq!(std::fs::metadata(&staged).unwrap().nlink(), 1);
    let copy = std::fs::metadata(stored(&dir, &hash)).unwrap();
    assert_ne!(copy.ino(), std::fs::metadata(&staged).unwrap().ino());
    assert_eq!(copy.nlink(), 1);
    let entries = std::fs::read_dir(dir.path().join(DIRECTORY))
        .unwrap()
        .count();
    assert_eq!(entries, 1, "no temporary file is left");
}

#[test]
fn bytes_that_miss_their_hash_are_refused_and_leave_nothing() {
    let (dir, store) = setup();
    let hash = sha(b"expected");
    assert!(matches!(
        store.put(&b"other bytes"[..], &hash),
        Err(AppError::Invalid(_))
    ));
    assert!(matches!(
        store.put(&b"x"[..], "../../etc/passwd"),
        Err(AppError::Invalid(_))
    ));
    assert_eq!(
        std::fs::read_dir(dir.path().join(DIRECTORY))
            .unwrap()
            .count(),
        0
    );
}

#[test]
fn a_tampered_or_missing_file_is_pdf_corrupt() {
    let (dir, store) = setup();
    let bytes = b"%PDF-1.4 original";
    let hash = sha(bytes);
    assert!(matches!(store.open(&hash), Err(AppError::PdfCorrupt)));
    drop(store.put(&bytes[..], &hash).unwrap());
    let path = stored(&dir, &hash);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    std::fs::write(&path, b"%PDF-1.4 altered").unwrap();
    assert!(matches!(store.open(&hash), Err(AppError::PdfCorrupt)));
    // A new put repairs it.
    drop(store.put(&bytes[..], &hash).unwrap());
    assert!(store.open(&hash).is_ok());
    assert_eq!(
        AppError::PdfCorrupt.to_string(),
        "The stored PDF of this source is missing or damaged; import the source again."
    );
}

#[test]
fn a_matching_store_id_sweeps_and_a_foreign_one_never_deletes() {
    let (dir, store) = setup();
    store.bind_identity(&"a".repeat(32)).unwrap();
    assert_eq!(
        std::fs::read(dir.path().join(DIRECTORY).join(MARKER)).unwrap(),
        "a".repeat(32).as_bytes()
    );
    let bytes = b"orphan";
    let hash = sha(bytes);
    drop(store.put(&bytes[..], &hash).unwrap());
    age(&stored(&dir, &hash), 2 * 3600);
    assert!(!store.shared().unwrap());
    assert_eq!(store.sweep(|| Ok(HashSet::new())).unwrap(), Some(1));
    // Another database opens the same folder: it sees a different id and marks the folder.
    drop(store.put(&bytes[..], &hash).unwrap());
    age(&stored(&dir, &hash), 2 * 3600);
    let other = PdfStore::in_directory(dir.path()).unwrap();
    other.bind_identity(&"b".repeat(32)).unwrap();
    assert!(other.shared().unwrap());
    for side in [&other, &store] {
        assert_eq!(side.sweep(|| Ok(HashSet::new())).unwrap(), None);
        assert!(stored(&dir, &hash).exists());
    }
    // Reads and writes keep working.
    assert!(store.open(&hash).is_ok());
    drop(other.put(&b"more"[..], &sha(b"more")).unwrap());
}

#[test]
fn a_foreign_marker_alone_stops_the_sweep() {
    let (dir, store) = setup();
    std::fs::write(dir.path().join(DIRECTORY).join(MARKER), "c".repeat(32)).unwrap();
    store.bind_identity(&"d".repeat(32)).unwrap();
    let hash = sha(b"x");
    drop(store.put(&b"x"[..], &hash).unwrap());
    age(&stored(&dir, &hash), 2 * 3600);
    assert_eq!(store.sweep(|| Ok(HashSet::new())).unwrap(), None);
    assert!(stored(&dir, &hash).exists());
}

#[test]
fn the_idempotent_put_path_still_succeeds_on_an_existing_file() {
    let (_dir, store) = setup();
    let hash = sha(b"again");
    drop(store.put(&b"again"[..], &hash).unwrap());
    drop(store.put(&b"again"[..], &hash).unwrap());
    assert!(store.open(&hash).is_ok());
}

#[test]
fn a_link_in_the_store_is_refused_and_never_followed() {
    let (dir, store) = setup();
    let bytes = b"%PDF-1.4 outside";
    let hash = sha(bytes);
    let outside = dir.path().join("outside.pdf");
    std::fs::write(&outside, bytes).unwrap();
    symlink(&outside, stored(&dir, &hash)).unwrap();
    assert!(matches!(store.open(&hash), Err(AppError::PdfCorrupt)));
    assert!(!store.present(&hash));
    // Not even the sweep or a put writes through it.
    drop(store.put(&bytes[..], &hash).unwrap());
    assert_eq!(std::fs::read(&outside).unwrap(), bytes);
    assert!(
        !std::fs::symlink_metadata(stored(&dir, &hash))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(store.open(&hash).is_ok());
}

#[test]
fn a_directory_in_place_of_a_file_is_pdf_corrupt() {
    let (dir, store) = setup();
    let hash = sha(b"x");
    std::fs::create_dir(stored(&dir, &hash)).unwrap();
    assert!(matches!(store.open(&hash), Err(AppError::PdfCorrupt)));
}

#[test]
fn the_sweep_removes_old_orphans_and_keeps_young_and_referenced_files() {
    let (dir, store) = setup();
    let mut hashes = Vec::new();
    for name in ["old orphan", "young orphan", "old referenced"] {
        let hash = sha(name.as_bytes());
        drop(store.put(name.as_bytes(), &hash).unwrap());
        hashes.push(hash);
    }
    age(&stored(&dir, &hashes[0]), 2 * 3600);
    age(&stored(&dir, &hashes[2]), 2 * 3600);
    let temporary = dir.path().join(DIRECTORY).join(".tmp-0123456789abcdef");
    std::fs::write(&temporary, b"partial").unwrap();
    age(&temporary, 2 * 3600);
    let young_temporary = dir.path().join(DIRECTORY).join(".tmp-fedcba9876543210");
    std::fs::write(&young_temporary, b"partial").unwrap();
    let unrelated = dir.path().join(DIRECTORY).join("notes.txt");
    std::fs::write(&unrelated, b"kept").unwrap();
    age(&unrelated, 2 * 3600);
    let keep: HashSet<String> = [hashes[2].clone()].into();
    assert_eq!(store.sweep(|| Ok(keep)).unwrap(), Some(2));
    assert!(!stored(&dir, &hashes[0]).exists());
    assert!(stored(&dir, &hashes[1]).exists(), "younger than one hour");
    assert!(stored(&dir, &hashes[2]).exists(), "a row names it");
    assert!(!temporary.exists());
    assert!(young_temporary.exists());
    assert!(unrelated.exists(), "foreign names are left alone");
}

#[test]
fn an_import_holding_the_shared_lock_keeps_its_file_from_the_sweep() {
    let (dir, store) = setup();
    let bytes = b"%PDF-1.4 racing";
    let hash = sha(bytes);
    // The writer is between `put` and its database commit; its file is old enough to sweep.
    let pending = store.put(&bytes[..], &hash).unwrap();
    age(&stored(&dir, &hash), 2 * 3600);
    let mut called = false;
    assert_eq!(
        store
            .sweep(|| {
                called = true;
                Ok(HashSet::new())
            })
            .unwrap(),
        None,
        "a busy store skips the run"
    );
    assert!(
        !called,
        "the rows are not even read while a writer holds the lock"
    );
    assert!(stored(&dir, &hash).exists());
    // A writer in another thread (its own open file description) blocks the sweep the same way.
    drop(pending);
    let held = std::thread::scope(|scope| {
        let (tx, rx) = std::sync::mpsc::channel();
        let (release, wait) = std::sync::mpsc::channel::<()>();
        let (shared, name) = (&store, &hash);
        let writer = scope.spawn(move || {
            let pending = shared.put(&bytes[..], name).unwrap();
            tx.send(()).unwrap();
            wait.recv().unwrap();
            drop(pending);
        });
        rx.recv().unwrap();
        let skipped = store.sweep(|| Ok(HashSet::new())).unwrap();
        release.send(()).unwrap();
        writer.join().unwrap();
        skipped
    });
    assert_eq!(held, None);
    // Once the row would have committed (or never did), the old orphan goes.
    assert_eq!(store.sweep(|| Ok(HashSet::new())).unwrap(), Some(1));
    assert!(!stored(&dir, &hash).exists());
}
