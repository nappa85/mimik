//! Shared synthetic catalogue fixture for the catalogue integration suites (reference
//! `docs/catalogues.md`): a registered nonnative pair, a finalized synthetic source, CLI
//! helpers and the redaction sentinels.
#![allow(
    dead_code,
    reason = "Shared by the draft and publication suites; each uses a subset"
)]
use mimik::{Store, catalogues::CatalogueError, registry::RegisterRuleset};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    process::{Command, Output},
};

#[path = "source_fixture.rs"]
pub mod source_fixture;

pub static TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Distinctive synthetic strings placed in names, keys, property ids/values, labels, the
/// title, owner text, the publication actor and rationale; plus the fixture's unit text.
/// None may appear in any output.
pub const SENTINELS: &[&str] = &[
    "zqxname",
    "zqxkey",
    "zqxowner",
    "zqxprop",
    "zqxvalue",
    "zqxlabel",
    "zqxtitle",
    "zqxmember",
    "zqxactor",
    "zqxrationale",
    "Strength capacity",
];

/// Every catalogue object, children first, for rewinding a fixture to an older schema.
pub const DROP_CATALOGUE_SCHEMA: &str = "DROP TABLE catalogue_name_fold; DROP TABLE catalogue_entry_names; DROP TABLE catalogue_names_fts; DROP TABLE catalogue_seals; DROP TABLE catalogue_entry_units; DROP TABLE catalogue_entries; DROP TABLE catalogue_entry_types; DROP TABLE catalogue_versions; DROP TABLE catalogues; DROP TABLE catalogue_drafts;";

pub struct Fixture {
    pub _dir: tempfile::TempDir,
    pub root: PathBuf,
    pub database: PathBuf,
    pub store: Store,
    pub source: i64,
    pub pdf_hash: String,
    pub page: i64,
    pub unit: i64,
}

pub async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_owned();
    source_fixture::fixture_pair(&root).unwrap();
    let database = root.join("catalogue.sqlite3");
    let store = Store::open(&database).unwrap();
    store
        .register_ruleset(RegisterRuleset {
            game_id: "synthetic-game".into(),
            game_label: "Synthetic game".into(),
            edition_id: "first".into(),
            edition_label: "First edition".into(),
        })
        .await
        .unwrap();
    store
        .import_source(&root.join("unknown-bundle"), &root.join("original.pdf"))
        .await
        .unwrap();
    let source = store.sources().await.unwrap().remove(0);
    let page = store.source_page(source.pages[0].id).await.unwrap();
    Fixture {
        _dir: dir,
        root,
        database,
        store,
        source: source.id,
        pdf_hash: source.pdf_hash,
        page: page.id,
        unit: page.units[0].id,
    }
}

impl Fixture {
    /// A catalogue citing the fixture unit, with a sentinel in every author-chosen string.
    pub fn catalogue(&self) -> Value {
        json!({
            "contract_version": 1, "game_id": "synthetic-game", "edition_id": "first",
            "catalogue_id": "spells", "title": "Title zqxtitle",
            "sources": [{"source_id": self.source, "pdf_sha256": self.pdf_hash}],
            "entry_types": [{"id": "spell", "label": "zqxlabel", "properties": [
                {"id": "zqxprop", "label": "Prop zqxlabel", "control": "text"},
                {"id": "level", "label": "Level", "control": "integer"}]}],
            "entries": [
                {"key": "zqxkey-flare", "type": "spell", "name": "Name zqxname",
                 "properties": {"zqxprop": "zqxvalue", "level": 1},
                 "description": {"units": [{"source_id": self.source, "page_id": self.page, "unit_id": self.unit}]}},
                {"key": "zqxkey-charm", "type": "spell", "name": "Charm zqxname", "retired": true,
                 "properties": {"zqxprop": "", "level": 0},
                 "description": {"owner_text": "zqxowner synthetic house text"}}]
        })
    }

    pub async fn save(&self, value: &Value, revision: i64) -> Result<i64, CatalogueError> {
        self.store
            .save_catalogue_draft(serde_json::to_vec(value).unwrap(), revision)
            .await
            .map(|draft| draft.revision)
    }

    pub fn write(&self, name: &str, value: &[u8]) -> PathBuf {
        let path = self.root.join(name);
        std::fs::write(&path, value).unwrap();
        path
    }

    pub fn cli(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_mimik"))
            .arg("--database")
            .arg(&self.database)
            .args(args)
            .output()
            .unwrap()
    }

    pub fn import(&self, file: &Path, revision: i64) -> Output {
        self.cli(&[
            "import-catalogue-draft",
            "--file",
            file.to_str().unwrap(),
            "--expected-revision",
            &revision.to_string(),
        ])
    }

    pub fn drafts(&self) -> i64 {
        self.count("catalogue_drafts")
    }

    /// Rows of `table` (a fixed identifier from the test, never input).
    pub fn count(&self, table: &str) -> i64 {
        self.db()
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap()
    }

    /// A separate connection, configured like the application's.
    pub fn db(&self) -> rusqlite::Connection {
        let db = rusqlite::Connection::open(&self.database).unwrap();
        db.busy_timeout(std::time::Duration::from_secs(5)).unwrap();
        db.execute_batch("PRAGMA foreign_keys=ON").unwrap();
        db
    }
}

pub fn assert_redacted(output: &Output) {
    for stream in [&output.stdout, &output.stderr] {
        let text = String::from_utf8_lossy(stream);
        for sentinel in SENTINELS {
            assert!(!text.contains(sentinel), "output leaked {sentinel}: {text}");
        }
    }
}

pub fn code(error: &CatalogueError) -> &str {
    match error {
        CatalogueError::Invalid(report) => &report.diagnostics[0].code,
        other => panic!("unexpected error {other:?}"),
    }
}

/// A second real, finalized source of the same pair (same synthetic PDF, its own bundle).
pub async fn second_source(fixture: &Fixture) -> (i64, i64, i64) {
    let second = fixture.root.join("second-bundle");
    std::fs::create_dir(&second).unwrap();
    let first = fixture.root.join("unknown-bundle");
    for name in ["source.json", "page-1.png"] {
        std::fs::copy(first.join(name), second.join(name)).unwrap();
    }
    let mut manifest: Value =
        serde_json::from_slice(&std::fs::read(first.join("bundle.json")).unwrap()).unwrap();
    manifest["source_id"] = json!("synthetic-second-core");
    manifest["title"] = json!("Second synthetic source");
    std::fs::write(
        second.join("bundle.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    fixture
        .store
        .import_source(&second, &fixture.root.join("original.pdf"))
        .await
        .unwrap();
    let source = fixture
        .store
        .sources()
        .await
        .unwrap()
        .into_iter()
        .find(|source| source.id != fixture.source)
        .unwrap();
    let page = fixture.store.source_page(source.pages[0].id).await.unwrap();
    assert_ne!(page.id, fixture.page);
    (source.id, page.id, page.units[0].id)
}
