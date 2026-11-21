//! The content-addressed PDF store through the CLI import path (SRC-15 slice 7).
use mimik::{
    AppError, Store,
    sources::{Fetched, WantedPage, WordSource},
};
use std::{
    collections::BTreeSet,
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt, symlink},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
    time::{Duration, SystemTime},
};

/// A word source that stops inside `fetch` (after the import stored its PDF, before the row
/// commits) until released, and may then panic like a crash.
struct Blocking {
    entered: Mutex<Option<mpsc::Sender<()>>>,
    release: Mutex<mpsc::Receiver<()>>,
    crash: bool,
}

impl WordSource for Blocking {
    fn fetch(&self, _: &[u8], _: &str, _: &[WantedPage]) -> Fetched {
        if let Some(entered) = self.entered.lock().unwrap().take() {
            entered.send(()).unwrap();
        }
        self.release.lock().unwrap().recv().unwrap();
        assert!(!self.crash, "simulated crash before the commit");
        Fetched::default()
    }
}

fn age(path: &Path) {
    fs::File::open(path)
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_secs(7200))
        .unwrap();
}

#[allow(
    dead_code,
    reason = "The shared fixture serves many suites; this one uses a subset"
)]
#[path = "support/source_fixture.rs"]
mod fixture;

struct Setup {
    dir: tempfile::TempDir,
    store: Store,
    bundle: PathBuf,
    pdf: PathBuf,
    hash: String,
}

impl Setup {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let (bundle, pdf) = fixture::fixture(dir.path()).unwrap();
        let store = Store::open(&dir.path().join("mimik.sqlite3")).unwrap();
        let hash = fixture::hash(&fs::read(&pdf).unwrap());
        Self {
            dir,
            store,
            bundle,
            pdf,
            hash,
        }
    }

    fn stored(&self) -> PathBuf {
        self.dir
            .path()
            .join("sources")
            .join(format!("{}.pdf", self.hash))
    }

    fn files(&self) -> BTreeSet<String> {
        fs::read_dir(self.dir.path().join("sources"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .filter(|name| !name.starts_with('.'))
            .collect()
    }
}

/// A second bundle of the same PDF under another source identity.
fn second_bundle(root: &Path, first: &Path) -> PathBuf {
    let second = root.join("second-bundle");
    fs::create_dir(&second).unwrap();
    for name in ["source.json", "page-1.png"] {
        fs::copy(first.join(name), second.join(name)).unwrap();
    }
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(first.join("bundle.json")).unwrap()).unwrap();
    manifest["source_id"] = serde_json::json!("synthetic-second-core");
    manifest["title"] = serde_json::json!("Second synthetic source");
    fs::write(
        second.join("bundle.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    second
}

#[tokio::test]
async fn two_sources_with_one_pdf_share_one_read_only_file() {
    let s = Setup::new();
    let first = s.store.import_source(&s.bundle, &s.pdf).await.unwrap();
    let second_dir = second_bundle(s.dir.path(), &s.bundle);
    let second = s.store.import_source(&second_dir, &s.pdf).await.unwrap();
    assert_ne!(first, second);
    assert_eq!(s.files(), BTreeSet::from([format!("{}.pdf", s.hash)]));
    assert_eq!(fs::metadata(s.stored()).unwrap().mode() & 0o777, 0o400);
    let bytes = fs::read(&s.pdf).unwrap();
    for id in [first, second] {
        let mut file = s.store.open_source_pdf(id).await.unwrap();
        let mut content = Vec::new();
        std::io::Read::read_to_end(&mut file, &mut content).unwrap();
        assert_eq!(content, bytes);
    }
    // The source PDF the importer read keeps its single link: the store holds a copy.
    assert_eq!(fs::metadata(&s.pdf).unwrap().nlink(), 1);
    // Importing the same bundle again changes nothing.
    s.store.import_source(&s.bundle, &s.pdf).await.unwrap();
    assert_eq!(s.files().len(), 1);
    assert_eq!(s.store.missing_pdf_count().await.unwrap(), 0);
}

#[tokio::test]
async fn a_missing_file_is_pdf_corrupt_is_counted_and_a_reimport_restores_it() {
    let s = Setup::new();
    let id = s.store.import_source(&s.bundle, &s.pdf).await.unwrap();
    fs::remove_file(s.stored()).unwrap();
    assert_eq!(s.store.missing_pdf_count().await.unwrap(), 1);
    assert!(matches!(
        s.store.open_source_pdf(id).await,
        Err(AppError::PdfCorrupt)
    ));
    assert!(matches!(
        s.store.open_source_pdf(id + 100).await,
        Err(AppError::SourceNotFound)
    ));
    // Everything that never reads the PDF keeps working.
    assert_eq!(s.store.sources().await.unwrap().len(), 1);
    s.store.source_page(1).await.unwrap();
    // Importing the same source again stores the file; the row is not duplicated.
    assert_eq!(s.store.import_source(&s.bundle, &s.pdf).await.unwrap(), id);
    assert_eq!(s.store.missing_pdf_count().await.unwrap(), 0);
    assert!(s.store.open_source_pdf(id).await.is_ok());
}

#[tokio::test]
async fn a_tampered_file_or_a_link_in_its_place_is_pdf_corrupt() {
    let s = Setup::new();
    let id = s.store.import_source(&s.bundle, &s.pdf).await.unwrap();
    let path = s.stored();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let mut bytes = fs::read(&path).unwrap();
    bytes.push(b'\n');
    fs::write(&path, &bytes).unwrap();
    assert!(matches!(
        s.store.open_source_pdf(id).await,
        Err(AppError::PdfCorrupt)
    ));
    // A link to a file with the right bytes is refused too, and never followed.
    fs::remove_file(&path).unwrap();
    symlink(&s.pdf, &path).unwrap();
    assert!(matches!(
        s.store.open_source_pdf(id).await,
        Err(AppError::PdfCorrupt)
    ));
}

#[tokio::test]
async fn the_sweep_removes_old_orphans_only() {
    let s = Setup::new();
    let id = s.store.import_source(&s.bundle, &s.pdf).await.unwrap();
    let orphan = "e".repeat(64);
    let young = "f".repeat(64);
    let sources = s.dir.path().join("sources");
    for name in [&orphan, &young] {
        fs::write(sources.join(format!("{name}.pdf")), b"orphan").unwrap();
    }
    let two_hours = SystemTime::now() - Duration::from_secs(7200);
    for path in [s.stored(), sources.join(format!("{orphan}.pdf"))] {
        fs::File::open(path)
            .unwrap()
            .set_modified(two_hours)
            .unwrap();
    }
    assert_eq!(s.store.sweep_pdf_store().await.unwrap(), Some(1));
    assert_eq!(
        s.files(),
        BTreeSet::from([format!("{}.pdf", s.hash), format!("{young}.pdf")])
    );
    assert!(s.store.open_source_pdf(id).await.is_ok());
}

fn blocking(crash: bool) -> (Arc<Blocking>, mpsc::Receiver<()>, mpsc::Sender<()>) {
    let (entered, wait_entered) = mpsc::channel();
    let (release, wait_release) = mpsc::channel();
    let source = Arc::new(Blocking {
        entered: Mutex::new(Some(entered)),
        release: Mutex::new(wait_release),
        crash,
    });
    (source, wait_entered, release)
}

#[tokio::test]
async fn an_import_between_its_put_and_its_commit_keeps_its_file_from_the_sweep() {
    let s = Setup::new();
    let (source, entered, release) = blocking(false);
    let store = s.store.clone();
    let (bundle, pdf) = (s.bundle.clone(), s.pdf.clone());
    let import =
        tokio::spawn(async move { store.import_source_from(&bundle, &pdf, Some(source)).await });
    tokio::task::spawn_blocking(move || entered.recv().unwrap())
        .await
        .unwrap();
    // The file is stored and old enough to sweep, but the writer holds the shared lock.
    assert!(s.stored().exists());
    age(&s.stored());
    assert_eq!(s.store.sweep_pdf_store().await.unwrap(), None);
    assert!(s.stored().exists());
    release.send(()).unwrap();
    let (id, _) = import.await.unwrap().unwrap();
    assert!(s.store.open_source_pdf(id).await.is_ok());
    // Committed and referenced: an aged file survives the next sweep too.
    assert_eq!(s.store.sweep_pdf_store().await.unwrap(), Some(0));
    assert!(s.stored().exists());
}

#[tokio::test]
async fn a_crash_before_the_commit_leaves_an_orphan_that_only_ages_out() {
    let s = Setup::new();
    let (source, entered, release) = blocking(true);
    let store = s.store.clone();
    let (bundle, pdf) = (s.bundle.clone(), s.pdf.clone());
    let import =
        tokio::spawn(async move { store.import_source_from(&bundle, &pdf, Some(source)).await });
    tokio::task::spawn_blocking(move || entered.recv().unwrap())
        .await
        .unwrap();
    release.send(()).unwrap();
    assert!(import.await.unwrap().is_err());
    assert_eq!(s.store.sources().await.unwrap().len(), 0);
    // The writer is gone, so the lock is free; the young orphan stays, the aged one goes.
    assert_eq!(s.store.sweep_pdf_store().await.unwrap(), Some(0));
    assert!(s.stored().exists());
    age(&s.stored());
    assert_eq!(s.store.sweep_pdf_store().await.unwrap(), Some(1));
    assert!(!s.stored().exists());
}

#[tokio::test]
async fn two_databases_in_one_folder_never_sweep_each_others_pdfs() {
    let s = Setup::new();
    s.store.import_source(&s.bundle, &s.pdf).await.unwrap();
    let other = Store::open(&s.dir.path().join("other.sqlite3")).unwrap();
    assert!(other.pdf_store_shared().unwrap());
    assert!(s.store.pdf_store_shared().unwrap());
    age(&s.stored());
    // The other database names no rows: a sweep that ran would delete the first one's PDF.
    assert_eq!(other.sweep_pdf_store().await.unwrap(), None);
    assert_eq!(s.store.sweep_pdf_store().await.unwrap(), None);
    assert!(s.stored().exists());
    // Reads and writes still work through either database.
    assert!(s.store.open_source_pdf(1).await.is_ok());
    let second = second_bundle(s.dir.path(), &s.bundle);
    other.import_source(&second, &s.pdf).await.unwrap();
    assert_eq!(s.files().len(), 1);
}

#[tokio::test]
async fn a_database_alone_in_its_folder_sweeps_normally() {
    let s = Setup::new();
    assert!(!s.store.pdf_store_shared().unwrap());
    let marker = fs::read_to_string(s.dir.path().join("sources/.store-id")).unwrap();
    assert_eq!(marker.len(), 32);
    // Reopening the same database keeps the id and the folder unshared.
    let again = Store::open(&s.dir.path().join("mimik.sqlite3")).unwrap();
    assert!(!again.pdf_store_shared().unwrap());
    assert_eq!(again.sweep_pdf_store().await.unwrap(), Some(0));
}
