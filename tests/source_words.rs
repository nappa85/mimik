//! Word boxes stored at import: the source of the words is a fake, so no worker is needed.
//! Synthetic evidence only. The confined worker itself has live tests in `src/sources/worker`.
use mimik::{
    Store,
    sources::{Fetched, WantedPage, WordSource},
};
use rusqlite::Connection;
use std::{
    collections::BTreeMap,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

#[path = "support/source_fixture.rs"]
pub mod fixture;

/// A version 1 row with one word at the given box.
fn row(text: &str, bounds: [f32; 4]) -> Vec<u8> {
    let mut out = vec![1];
    for v in bounds {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&u16::try_from(text.len()).unwrap().to_le_bytes());
    out.extend_from_slice(text.as_bytes());
    out
}

#[derive(Default)]
struct Fake {
    calls: AtomicUsize,
    pages: std::sync::Mutex<Vec<u32>>,
    rows: BTreeMap<u32, Vec<u8>>,
    unavailable: Option<String>,
    /// Open a second connection and take the write lock while the "worker" runs.
    database: Option<std::path::PathBuf>,
}
impl WordSource for Fake {
    fn fetch(&self, pdf: &[u8], sha256: &str, pages: &[WantedPage]) -> Fetched {
        self.calls.fetch_add(1, Ordering::SeqCst);
        assert_ne!(pdf.len(), 0);
        assert_eq!(sha256.len(), 64);
        self.pages
            .lock()
            .unwrap()
            .extend(pages.iter().map(|p| p.pdf_page));
        if let Some(path) = &self.database {
            // The worker never runs inside the write transaction: this takes the lock at once.
            let db = Connection::open(path).unwrap();
            db.execute_batch("PRAGMA busy_timeout=0; BEGIN IMMEDIATE; ROLLBACK;")
                .unwrap();
        }
        Fetched {
            rows: self.rows.clone(),
            size_mismatch: Vec::new(),
            unavailable: self.unavailable.clone(),
        }
    }
}

fn words_rows(path: &Path) -> Vec<(i64, Vec<u8>)> {
    let db = Connection::open(path).unwrap();
    let mut q = db
        .prepare("SELECT page_id,words FROM source_page_words ORDER BY page_id")
        .unwrap();
    q.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

#[tokio::test]
async fn import_stores_validated_rows_outside_the_write_transaction() {
    let dir = tempfile::tempdir().unwrap();
    let (bundle, pdf) = fixture::fixture(dir.path()).unwrap();
    let path = dir.path().join("words.sqlite3");
    let store = Store::open(&path).unwrap();
    let stored = row("Sword", [10.0, 10.0, 40.0, 20.0]);
    let fake = Arc::new(Fake {
        rows: BTreeMap::from([(1, stored.clone()), (7, vec![1])]),
        database: Some(path.clone()),
        ..Fake::default()
    });
    let (id, report) = store
        .import_source_from(&bundle, &pdf, Some(fake.clone()))
        .await
        .unwrap();
    assert_eq!(id, 1);
    assert_eq!(fake.calls.load(Ordering::SeqCst), 1);
    assert_eq!(*fake.pages.lock().unwrap(), [1]);
    // The row for a page the bundle does not have (7) is ignored.
    assert_eq!((report.stored, report.missing), (1, 0));
    assert_eq!(report.unavailable, None);
    assert_eq!(words_rows(&path), [(1, stored)]);
}

#[tokio::test]
async fn re_import_fills_only_pages_without_words_and_leaves_review_alone() {
    let dir = tempfile::tempdir().unwrap();
    let (bundle, pdf) = fixture::fixture(dir.path()).unwrap();
    let path = dir.path().join("fill.sqlite3");
    let store = Store::open(&path).unwrap();
    // Imported earlier without words (the plain import reads none).
    store.import_source(&bundle, &pdf).await.unwrap();
    assert_eq!(words_rows(&path).len(), 0);
    let events = || {
        Connection::open(&path)
            .unwrap()
            .query_row("SELECT count(*) FROM source_review_events", [], |r| {
                r.get::<_, i64>(0)
            })
            .unwrap()
    };
    let before = events();
    let first = row("Sword", [10.0, 10.0, 40.0, 20.0]);
    let fill = Arc::new(Fake {
        rows: BTreeMap::from([(1, first.clone())]),
        ..Fake::default()
    });
    let (id, report) = store
        .import_source_from(&bundle, &pdf, Some(fill.clone()))
        .await
        .unwrap();
    assert_eq!(id, 1);
    assert_eq!(*fill.pages.lock().unwrap(), [1]);
    assert_eq!((report.stored, report.missing), (1, 0));
    assert_eq!(words_rows(&path), [(1, first.clone())]);
    assert_eq!(events(), before);
    // Idempotent: nothing is missing, so the source is not even asked.
    let other = Arc::new(Fake {
        rows: BTreeMap::from([(1, row("Other", [1.0, 1.0, 5.0, 5.0]))]),
        ..Fake::default()
    });
    let (_, report) = store
        .import_source_from(&bundle, &pdf, Some(other.clone()))
        .await
        .unwrap();
    assert_eq!(other.calls.load(Ordering::SeqCst), 0);
    assert_eq!((report.stored, report.missing), (0, 0));
    assert_eq!(words_rows(&path), [(1, first)]);
    assert_eq!(
        store.import_source(&bundle, &pdf).await.unwrap(),
        1,
        "the plain import of the same source is unchanged"
    );
}

#[tokio::test]
async fn an_unavailable_worker_still_imports_and_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let (bundle, pdf) = fixture::fixture(dir.path()).unwrap();
    let path = dir.path().join("unavailable.sqlite3");
    let store = Store::open(&path).unwrap();
    let fake = Arc::new(Fake {
        unavailable: Some("no sandbox".into()),
        ..Fake::default()
    });
    let (id, report) = store
        .import_source_from(&bundle, &pdf, Some(fake))
        .await
        .unwrap();
    assert_eq!(id, 1);
    assert_eq!(report.unavailable.as_deref(), Some("no sandbox"));
    assert_eq!((report.stored, report.missing), (0, 1));
    assert_eq!(words_rows(&path).len(), 0);
    // Running it again where the worker works fills the page.
    let fake = Arc::new(Fake {
        rows: BTreeMap::from([(1, vec![1])]),
        ..Fake::default()
    });
    let (_, report) = store
        .import_source_from(&bundle, &pdf, Some(fake))
        .await
        .unwrap();
    assert_eq!((report.stored, report.missing), (1, 0));
    assert_eq!(words_rows(&path), [(1, vec![1])]);
}

#[tokio::test]
async fn a_refused_import_stores_no_words() {
    let dir = tempfile::tempdir().unwrap();
    let (bundle, pdf) = fixture::fixture(dir.path()).unwrap();
    let path = dir.path().join("refused.sqlite3");
    let store = Store::open(&path).unwrap();
    let mut raw = std::fs::read(dir.path().join("original.pdf")).unwrap();
    raw.extend_from_slice(b"\n% changed");
    std::fs::write(&pdf, raw).unwrap();
    let fake = Arc::new(Fake {
        rows: BTreeMap::from([(1, vec![1])]),
        ..Fake::default()
    });
    assert!(
        store
            .import_source_from(&bundle, &pdf, Some(fake.clone()))
            .await
            .is_err()
    );
    // The PDF did not match its hash: refused before any worker was asked.
    assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
    assert_eq!(words_rows(&path).len(), 0);
}

#[tokio::test]
async fn page_words_are_append_only_and_bounded() {
    let dir = tempfile::tempdir().unwrap();
    let (bundle, pdf) = fixture::fixture(dir.path()).unwrap();
    let path = dir.path().join("append.sqlite3");
    let store = Store::open(&path).unwrap();
    let fake = Arc::new(Fake {
        rows: BTreeMap::from([(1, vec![1])]),
        ..Fake::default()
    });
    store
        .import_source_from(&bundle, &pdf, Some(fake))
        .await
        .unwrap();
    let db = Connection::open(&path).unwrap();
    assert!(
        db.execute("UPDATE source_page_words SET words=x'0100'", [])
            .is_err()
    );
    assert!(db.execute("DELETE FROM source_page_words", []).is_err());
    assert!(
        db.execute(
            "INSERT OR REPLACE INTO source_page_words VALUES(1,x'01')",
            []
        )
        .is_err(),
        "REPLACE deletes the old row and is refused"
    );
    assert_eq!(words_rows(&path), [(1, vec![1])]);
    // A second row for a page, a wrong version byte, an empty or an oversized row.
    assert!(
        db.execute("INSERT INTO source_page_words VALUES(1,x'01')", [])
            .is_err()
    );
    // With foreign keys off only the CHECK can refuse these rows (page 99 does not exist).
    db.execute_batch("PRAGMA foreign_keys=OFF").unwrap();
    for bad in [
        "x'00'",
        "x''",
        "x'02'",
        "CAST(x'01'||zeroblob(5480001) AS BLOB)",
    ] {
        assert!(
            db.execute(
                &format!("INSERT INTO source_page_words VALUES(99,{bad})"),
                []
            )
            .is_err(),
            "{bad}"
        );
    }
    // The bound itself is accepted: 20,000 words x (256 + 18) bytes + the version byte.
    db.execute(
        "INSERT INTO source_page_words VALUES(99,CAST(x'01'||zeroblob(5480000) AS BLOB))",
        [],
    )
    .unwrap();
}

#[tokio::test]
async fn two_imports_of_the_same_source_at_once_leave_one_consistent_source() {
    let dir = tempfile::tempdir().unwrap();
    let (bundle, pdf) = fixture::fixture(dir.path()).unwrap();
    let path = dir.path().join("race.sqlite3");
    let store = Store::open(&path).unwrap();
    let fake = || {
        Arc::new(Fake {
            rows: BTreeMap::from([(1, row("Sword", [10.0, 10.0, 40.0, 20.0]))]),
            ..Fake::default()
        })
    };
    let (a, b) = tokio::join!(
        store.import_source_from(&bundle, &pdf, Some(fake())),
        store.import_source_from(&bundle, &pdf, Some(fake())),
    );
    // Either both succeed on the same source (the second a no-op) or one is refused as busy;
    // never two sources, two rows or a corrupt row.
    assert!(a.is_ok() || b.is_ok());
    for result in [&a, &b].into_iter().flatten() {
        assert_eq!(result.0, 1);
    }
    let db = Connection::open(&path).unwrap();
    let count = |sql: &str| db.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap();
    assert_eq!(count("SELECT count(*) FROM sources"), 1);
    assert_eq!(count("SELECT count(*) FROM source_page_words"), 1);
    let integrity: String = db
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .unwrap();
    assert_eq!(integrity, "ok");
}

#[tokio::test]
async fn invalid_or_foreign_rows_are_not_stored_and_count_as_unread() {
    let dir = tempfile::tempdir().unwrap();
    let (bundle, pdf) = fixture::fixture(dir.path()).unwrap();
    let path = dir.path().join("invalid.sqlite3");
    let store = Store::open(&path).unwrap();
    // Version 1 but truncated: the CHECK would take it, the reader never would.
    let fake = Arc::new(Fake {
        rows: BTreeMap::from([(1, vec![1, 0, 0])]),
        ..Fake::default()
    });
    let (_, report) = store
        .import_source_from(&bundle, &pdf, Some(fake))
        .await
        .unwrap();
    assert_eq!(
        (report.stored, report.missing, report.size_mismatch),
        (0, 1, 0)
    );
    assert_eq!(words_rows(&path).len(), 0);
}

struct Sized;
impl WordSource for Sized {
    fn fetch(&self, _: &[u8], _: &str, _: &[WantedPage]) -> Fetched {
        Fetched {
            size_mismatch: vec![1],
            ..Fetched::default()
        }
    }
}

#[tokio::test]
async fn size_mismatched_pages_are_counted_apart_from_unread_ones() {
    let dir = tempfile::tempdir().unwrap();
    let (bundle, pdf) = fixture::fixture(dir.path()).unwrap();
    let path = dir.path().join("size.sqlite3");
    let store = Store::open(&path).unwrap();
    let (_, report) = store
        .import_source_from(&bundle, &pdf, Some(Arc::new(Sized)))
        .await
        .unwrap();
    assert_eq!(
        (report.stored, report.missing, report.size_mismatch),
        (0, 0, 1)
    );
}
