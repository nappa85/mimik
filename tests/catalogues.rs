//! Catalogue drafts (plan `docs/catalogue-plan.md` §3 step 1): source consistency, revisions,
//! storage constraints, the schema-19 upgrade and CLI redaction. All fixtures are synthetic.
use mimik::{Store, catalogues::CatalogueError, registry::RegisterRuleset, sources::UnitReview};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    process::{Command, Output},
};

#[path = "support/source_fixture.rs"]
mod source_fixture;

static TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Distinctive synthetic strings placed in names, keys, property ids/values, labels, the
/// title and owner text; plus the fixture's unit text. None may appear in any output.
const SENTINELS: &[&str] = &[
    "zqxname",
    "zqxkey",
    "zqxowner",
    "zqxprop",
    "zqxvalue",
    "zqxlabel",
    "zqxtitle",
    "zqxmember",
    "Strength capacity",
];

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    database: PathBuf,
    store: Store,
    source: i64,
    pdf_hash: String,
    page: i64,
    unit: i64,
}

async fn fixture() -> Fixture {
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
    fn catalogue(&self) -> Value {
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

    async fn save(&self, value: &Value, revision: i64) -> Result<i64, CatalogueError> {
        self.store
            .save_catalogue_draft(serde_json::to_vec(value).unwrap(), revision)
            .await
            .map(|draft| draft.revision)
    }

    fn write(&self, name: &str, value: &[u8]) -> PathBuf {
        let path = self.root.join(name);
        std::fs::write(&path, value).unwrap();
        path
    }

    fn cli(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_mimik"))
            .arg("--database")
            .arg(&self.database)
            .args(args)
            .output()
            .unwrap()
    }

    fn import(&self, file: &Path, revision: i64) -> Output {
        self.cli(&[
            "import-catalogue-draft",
            "--file",
            file.to_str().unwrap(),
            "--expected-revision",
            &revision.to_string(),
        ])
    }

    fn drafts(&self) -> i64 {
        rusqlite::Connection::open(&self.database)
            .unwrap()
            .query_row("SELECT count(*) FROM catalogue_drafts", [], |row| {
                row.get(0)
            })
            .unwrap()
    }
}

fn assert_redacted(output: &Output) {
    for stream in [&output.stdout, &output.stderr] {
        let text = String::from_utf8_lossy(stream);
        for sentinel in SENTINELS {
            assert!(!text.contains(sentinel), "output leaked {sentinel}: {text}");
        }
    }
}

fn code(error: &CatalogueError) -> &str {
    match error {
        CatalogueError::Invalid(report) => &report.diagnostics[0].code,
        other => panic!("unexpected error {other:?}"),
    }
}

#[tokio::test]
async fn cli_import_and_show_print_metadata_only() {
    let _guard = TEST_LOCK.lock().await;
    let fixture = fixture().await;
    let file = fixture.write(
        "spells.json",
        &serde_json::to_vec(&fixture.catalogue()).unwrap(),
    );
    let output = fixture.import(&file, 0);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_redacted(&output);
    let printed: Value = serde_json::from_slice(&output.stdout).unwrap();
    let keys: Vec<&str> = printed
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        [
            "catalogue_id",
            "content_hash",
            "counts",
            "edition_id",
            "game_id",
            "id",
            "revision",
            "status"
        ]
    );
    assert_eq!(printed["revision"], 1);
    assert_eq!(printed["status"], "draft");
    assert_eq!(
        printed["counts"],
        json!({"sources": 1, "entry_types": 1, "entries": 2, "retired": 1, "owner_written": 1,
               "unit_references": 1, "distinct_units": 1})
    );
    let id = printed["id"].as_i64().unwrap().to_string();
    let shown = fixture.cli(&["show-catalogue-draft", "--id", &id]);
    assert!(shown.status.success());
    assert_redacted(&shown);
    assert_eq!(
        serde_json::from_slice::<Value>(&shown.stdout).unwrap(),
        printed
    );
    let updated = fixture.import(&file, 1);
    assert!(updated.status.success());
    assert_redacted(&updated);
    assert_eq!(
        serde_json::from_slice::<Value>(&updated.stdout).unwrap()["revision"],
        2
    );
}

#[tokio::test]
async fn cli_failure_paths_are_redacted_and_save_nothing() {
    let _guard = TEST_LOCK.lock().await;
    let fixture = fixture().await;
    let base = fixture.catalogue();
    let mut cases: Vec<(&str, Vec<u8>)> = [
        ("unknown-member", "/entries/0/zqxmember", json!("zqxvalue")),
        ("unknown-root-member", "/zqxmember", json!("zqxname")),
        ("undeclared-type", "/entries/0/type", json!("zqxkey")),
        (
            "undeclared-property",
            "/entries/0/properties/zqxmember",
            json!("zqxvalue"),
        ),
        ("property-type", "/entries/0/properties/zqxprop", json!(3)),
        (
            "property-array",
            "/entries/0/properties/zqxprop",
            json!(["zqxvalue"]),
        ),
        ("duplicate-key", "/entries/1/key", json!("zqxkey-flare")),
        ("bad-key", "/entries/1/key", json!("Zqxkey")),
        ("long-name", "/entries/0/name", json!("zqxname".repeat(20))),
        (
            "long-owner-text",
            "/entries/1/description/owner_text",
            json!("zqxowner".repeat(2_600)),
        ),
        (
            "both-descriptions",
            "/entries/0/description/owner_text",
            json!("zqxowner"),
        ),
        (
            "wrong-page",
            "/entries/0/description/units/0/page_id",
            json!(999),
        ),
        (
            "wrong-unit",
            "/entries/0/description/units/0/unit_id",
            json!(999),
        ),
        ("wrong-hash", "/sources/0/pdf_sha256", json!("0".repeat(64))),
        ("unregistered-pair", "/edition_id", json!("zqxkey")),
    ]
    .into_iter()
    .map(|(name, pointer, member)| {
        let mut value = base.clone();
        let (parent, key) = pointer.rsplit_once('/').unwrap();
        value
            .pointer_mut(parent)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert(key.into(), member);
        (name, serde_json::to_vec(&value).unwrap())
    })
    .collect();
    let compact = serde_json::to_string(&base).unwrap();
    cases.push((
        "duplicate-member",
        compact
            .replacen(
                r#""name":"Name zqxname""#,
                r#""name":"Name zqxname","name":"zqxname""#,
                1,
            )
            .into_bytes(),
    ));
    let mut oversized = compact.clone().into_bytes();
    oversized.resize(mimik::catalogues::MAX_BYTES + 1, b' ');
    cases.push(("oversized", oversized));
    for (name, bytes) in cases {
        let file = fixture.write(&format!("{name}.json"), &bytes);
        let output = fixture.import(&file, 0);
        assert!(!output.status.success(), "{name} accepted");
        assert!(!output.stderr.is_empty(), "{name} gave no diagnostic");
        assert_redacted(&output);
    }
    let file = fixture.write("valid.json", compact.as_bytes());
    let link = fixture.root.join("link.json");
    std::os::unix::fs::symlink(&file, &link).unwrap();
    let output = fixture.import(&link, 0);
    assert!(!output.status.success());
    assert_redacted(&output);
    assert_eq!(fixture.drafts(), 0);
}

#[tokio::test]
async fn cli_conflict_unknown_and_corrupt_draft_paths_are_redacted() {
    let _guard = TEST_LOCK.lock().await;
    let fixture = fixture().await;
    let file = fixture.write(
        "valid.json",
        &serde_json::to_vec(&fixture.catalogue()).unwrap(),
    );
    assert!(fixture.import(&file, 0).status.success());
    for output in [
        fixture.import(&file, 0),
        fixture.import(&file, 7),
        fixture.import(&file, -1),
        fixture.cli(&["show-catalogue-draft", "--id", "999"]),
        fixture.cli(&["show-catalogue-draft", "--id", "0"]),
    ] {
        assert!(!output.status.success());
        assert_redacted(&output);
    }
    let db = rusqlite::Connection::open(&fixture.database).unwrap();
    db.execute(
        "UPDATE catalogue_drafts SET content_hash=?1",
        ["0".repeat(64)],
    )
    .unwrap();
    let output = fixture.cli(&["show-catalogue-draft", "--id", "1"]);
    assert!(!output.status.success());
    assert_redacted(&output);
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("stored-draft-identity-or-hash-mismatch")
    );
}

#[tokio::test]
async fn units_must_be_on_their_page_in_a_listed_finalized_source_of_the_pair() {
    let _guard = TEST_LOCK.lock().await;
    let fixture = fixture().await;
    let base = fixture.catalogue();
    for (member, value, expected) in [
        ("page_id", 999, "unit-page-source-mismatch"),
        ("unit_id", 999, "unit-page-source-mismatch"),
        ("source_id", 999, "unit-source-not-listed"),
    ] {
        let mut value_object = base.clone();
        value_object["entries"][0]["description"]["units"][0][member] = value.into();
        let error = fixture.save(&value_object, 0).await.unwrap_err();
        assert_eq!(code(&error), expected, "{member}");
    }
    // A listed source id that does not exist at all fails the source check before any
    // unit check (cross-wired real sources are covered by `cross_wired_real_sources_*`).
    let mut other = base.clone();
    other["sources"]
        .as_array_mut()
        .unwrap()
        .push(json!({"source_id": 999, "pdf_sha256": fixture.pdf_hash}));
    other["entries"][0]["description"]["units"][0]["source_id"] = 999.into();
    let error = fixture.save(&other, 0).await.unwrap_err();
    assert_eq!(code(&error), "source-ruleset-or-pdf-hash-mismatch");
    let mut hash = base.clone();
    hash["sources"][0]["pdf_sha256"] = "0".repeat(64).into();
    assert_eq!(
        code(&fixture.save(&hash, 0).await.unwrap_err()),
        "source-ruleset-or-pdf-hash-mismatch"
    );
    // The exact pair: another registered edition of the same game does not own the source.
    fixture
        .store
        .register_ruleset(RegisterRuleset {
            game_id: "synthetic-game".into(),
            game_label: "Synthetic game".into(),
            edition_id: "second".into(),
            edition_label: "Second edition".into(),
        })
        .await
        .unwrap();
    let mut pair = base.clone();
    pair["edition_id"] = "second".into();
    assert_eq!(
        code(&fixture.save(&pair, 0).await.unwrap_err()),
        "source-ruleset-or-pdf-hash-mismatch"
    );
    for (game, edition) in [("synthetic-game", "unregistered"), ("mork-borg", "it-2025")] {
        let mut pair = base.clone();
        pair["game_id"] = game.into();
        pair["edition_id"] = edition.into();
        assert_eq!(
            code(&fixture.save(&pair, 0).await.unwrap_err()),
            "registered-nonnative-ruleset-required"
        );
    }
    assert_eq!(fixture.drafts(), 0);
    // A rejected unit stays allowed in a draft; publication refuses it later.
    fixture
        .store
        .review_source_unit(
            fixture.page,
            fixture.unit,
            UnitReview {
                revision: 1,
                state: "rejected".into(),
                correction: String::new(),
                actor: "fixture-reviewer".into(),
                rationale: "Synthetic rejection".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(fixture.save(&base, 0).await.unwrap(), 1);
}

#[tokio::test]
async fn sources_must_be_finalized() {
    let _guard = TEST_LOCK.lock().await;
    let fixture = fixture().await;
    let db = rusqlite::Connection::open(&fixture.database).unwrap();
    let pdf_hash = "a".repeat(64);
    let prepared = json!({"contract_version": 1, "extraction_sha256": "0".repeat(64)}).to_string();
    db.execute(
        "INSERT INTO sources(id,bundle_hash,source_id,title,system_id,edition,language,pdf_hash,pdf,raw_json,prepared_json,total_pages) VALUES(50,?1,'unfinalized','Unfinalized','synthetic-game','First','en',?2,x'',x'',?3,1)",
        rusqlite::params!["b".repeat(64), pdf_hash, prepared.as_bytes()],
    )
    .unwrap();
    db.execute(
        "INSERT INTO source_rulesets VALUES(50,'synthetic-game','first')",
        [],
    )
    .unwrap();
    db.execute(
        "INSERT INTO source_pages(id,source_id,pdf_page,png) VALUES(50,50,1,x'')",
        [],
    )
    .unwrap();
    db.execute("INSERT INTO source_units(id,page_id,block_id,kind,polygon,raw_html,raw_text) VALUES(50,50,'block','text','[[0,0],[1,0],[1,1],[0,1]]','','')", [])
        .unwrap();
    let mut value = fixture.catalogue();
    value["sources"] = json!([{"source_id": 50, "pdf_sha256": pdf_hash}]);
    value["entries"][0]["description"]["units"] =
        json!([{"source_id": 50, "page_id": 50, "unit_id": 50}]);
    let error = fixture.save(&value, 0).await.unwrap_err();
    assert_eq!(code(&error), "source-not-finalized");
    let file = fixture.write("unfinalized.json", &serde_json::to_vec(&value).unwrap());
    let output = fixture.import(&file, 0);
    assert!(!output.status.success());
    assert_redacted(&output);
    assert_eq!(fixture.drafts(), 0);
}

#[tokio::test]
async fn revisions_are_checked_and_reads_reverify_content() {
    let _guard = TEST_LOCK.lock().await;
    let fixture = fixture().await;
    let value = fixture.catalogue();
    assert!(matches!(
        fixture.save(&value, 1).await,
        Err(CatalogueError::Conflict)
    ));
    assert_eq!(fixture.save(&value, 0).await.unwrap(), 1);
    assert!(matches!(
        fixture.save(&value, 0).await,
        Err(CatalogueError::Conflict)
    ));
    assert!(fixture.save(&value, -1).await.is_err());
    assert_eq!(fixture.save(&value, 1).await.unwrap(), 2);
    assert!(matches!(
        fixture.save(&value, 1).await,
        Err(CatalogueError::Conflict)
    ));
    let shown = fixture.store.catalogue_draft(1).await.unwrap();
    assert_eq!(shown.revision, 2);
    assert!(matches!(
        fixture.store.catalogue_draft(2).await,
        Err(CatalogueError::NotFound)
    ));
    assert!(fixture.store.catalogue_draft(0).await.is_err());
}

#[tokio::test]
async fn storage_rejects_malformed_rows_and_identity_rebinding() {
    let _guard = TEST_LOCK.lock().await;
    let fixture = fixture().await;
    assert_eq!(fixture.save(&fixture.catalogue(), 0).await.unwrap(), 1);
    let db = rusqlite::Connection::open(&fixture.database).unwrap();
    db.execute_batch("PRAGMA foreign_keys=ON").unwrap();
    let oversized = format!(
        "UPDATE catalogue_drafts SET content=json_set(content,'$.title','{}')",
        "x".repeat(4 * 1024 * 1024)
    );
    for sql in [
        "UPDATE catalogue_drafts SET revision=0",
        "UPDATE catalogue_drafts SET revision=1.5",
        "UPDATE catalogue_drafts SET id=0",
        "UPDATE catalogue_drafts SET catalogue_id='other'",
        "UPDATE catalogue_drafts SET content='[]'",
        "UPDATE catalogue_drafts SET content='{}'",
        "UPDATE catalogue_drafts SET content=json_set(content,'$.catalogue_id','other')",
        "UPDATE catalogue_drafts SET content=json_set(content,'$.contract_version',2)",
        "UPDATE catalogue_drafts SET content_hash='bad'",
        "UPDATE catalogue_drafts SET content_hash=upper(content_hash)",
        "UPDATE catalogue_drafts SET content_hash=content_hash||char(0)",
        oversized.as_str(),
        "INSERT INTO catalogue_drafts(game_id,edition_id,catalogue_id,revision,content,content_hash) SELECT game_id,'unregistered',catalogue_id,1,json_set(content,'$.edition_id','unregistered'),content_hash FROM catalogue_drafts",
    ] {
        assert!(
            db.execute(sql, []).is_err(),
            "accepted invalid draft row: {}",
            &sql[..sql.len().min(120)]
        );
    }
    // Same-identity rebinding through two coordinated changes is the trigger's job.
    let error = db
        .execute(
            "UPDATE catalogue_drafts SET catalogue_id='other',content=json_set(content,'$.catalogue_id','other')",
            [],
        )
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("Catalogue draft identity is immutable")
    );
    assert_eq!(fixture.store.catalogue_draft(1).await.unwrap().revision, 1);
    // A content change that keeps the stored hash is detected on read.
    db.execute(
        "UPDATE catalogue_drafts SET content=json_set(content,'$.title','Changed')",
        [],
    )
    .unwrap();
    assert_eq!(
        code(&fixture.store.catalogue_draft(1).await.unwrap_err()),
        "stored-draft-identity-or-hash-mismatch"
    );
}

#[tokio::test]
async fn cross_process_create_has_one_winner() {
    let _guard = TEST_LOCK.lock().await;
    let fixture = fixture().await;
    let file = fixture.write(
        "race.json",
        &serde_json::to_vec(&fixture.catalogue()).unwrap(),
    );
    let threads: Vec<_> = (0..2)
        .map(|_| {
            let database = fixture.database.clone();
            let file = file.clone();
            std::thread::spawn(move || {
                Command::new(env!("CARGO_BIN_EXE_mimik"))
                    .arg("--database")
                    .arg(database)
                    .args(["import-catalogue-draft", "--file"])
                    .arg(file)
                    .args(["--expected-revision", "0"])
                    .output()
                    .unwrap()
            })
        })
        .collect();
    let outputs: Vec<Output> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    assert_eq!(outputs.iter().filter(|o| o.status.success()).count(), 1);
    for output in &outputs {
        assert_redacted(output);
        if !output.status.success() {
            // The loser is a revision conflict, never a storage (lock) failure.
            assert!(
                String::from_utf8_lossy(&output.stderr).contains("Conflict"),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
    assert_eq!(fixture.drafts(), 1);
}

#[tokio::test]
async fn populated_schema18_database_upgrades_to_19_atomically() {
    let _guard = TEST_LOCK.lock().await;
    let fixture = fixture().await;
    let definition: Value =
        serde_json::from_str(include_str!("../schemas/definition-example.json")).unwrap();
    fixture
        .store
        .save_definition_draft(serde_json::to_vec(&definition).unwrap(), 0)
        .await
        .unwrap();
    let database = &fixture.database;
    let counts = |db: &rusqlite::Connection| -> (i64, i64, i64, i64) {
        db.query_row(
            "SELECT (SELECT count(*) FROM sources),(SELECT count(*) FROM source_units),(SELECT count(*) FROM definition_drafts),(SELECT count(*) FROM registry_editions)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap()
    };
    let db = rusqlite::Connection::open(database).unwrap();
    let before = counts(&db);
    assert_eq!(before.0, 1);
    assert_eq!(before.2, 1);
    // Rewind to a genuine schema-18 shape.
    db.execute_batch("DROP TABLE catalogue_drafts; PRAGMA user_version=18;")
        .unwrap();
    // A conflicting object makes the upgrade fail as a whole, leaving schema 18.
    db.execute_batch("CREATE TABLE catalogue_drafts(zqxmember TEXT);")
        .unwrap();
    drop(db);
    assert!(Store::open(database).is_err());
    let db = rusqlite::Connection::open(database).unwrap();
    assert_eq!(
        db.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        18
    );
    db.execute_batch("DROP TABLE catalogue_drafts;").unwrap();
    drop(db);
    let reopened = Store::open(database).unwrap();
    let db = rusqlite::Connection::open(database).unwrap();
    assert_eq!(
        db.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        19
    );
    assert_eq!(mimik::SCHEMA_VERSION, 19);
    assert_eq!(counts(&db), before);
    let saved = reopened
        .save_catalogue_draft(serde_json::to_vec(&fixture.catalogue()).unwrap(), 0)
        .await
        .unwrap();
    assert_eq!(saved.revision, 1);
    assert_eq!(
        db.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap(),
        0
    );
}

#[tokio::test]
async fn cli_schema_paths_never_echo_author_property_keys() {
    let _guard = TEST_LOCK.lock().await;
    let fixture = fixture().await;
    for key in ["314159265358979", "name", "properties"] {
        let mut value = fixture.catalogue();
        value["entries"][0]["properties"]
            .as_object_mut()
            .unwrap()
            .insert(key.into(), Value::Null);
        let file = fixture.write("numeric.json", &serde_json::to_vec(&value).unwrap());
        let output = fixture.import(&file, 0);
        assert!(!output.status.success());
        assert_redacted(&output);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!stderr.contains(&format!("properties/{key}")), "{stderr}");
        assert!(stderr.contains("/entries/0/properties/*"), "{stderr}");
    }
}

/// A second real, finalized source of the same pair (same synthetic PDF, its own bundle).
async fn second_source(fixture: &Fixture) -> (i64, i64, i64) {
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

#[tokio::test]
async fn cross_wired_real_sources_are_rejected() {
    let _guard = TEST_LOCK.lock().await;
    let fixture = fixture().await;
    let (source_b, page_b, unit_b) = second_source(&fixture).await;
    let mut base = fixture.catalogue();
    base["sources"]
        .as_array_mut()
        .unwrap()
        .push(json!({"source_id": source_b, "pdf_sha256": fixture.pdf_hash}));
    // Both sources and every id exist; only the relationships are wrong.
    let (a, pa, ua) = (fixture.source, fixture.page, fixture.unit);
    for (label, source, page, unit) in [
        ("unit and page of A cited with source B", source_b, pa, ua),
        (
            "unit of A cited with page and source B",
            source_b,
            page_b,
            ua,
        ),
        ("unit of A cited with page of B and source A", a, page_b, ua),
        ("unit of B cited with page and source A", a, pa, unit_b),
    ] {
        let mut value = base.clone();
        value["entries"][0]["description"]["units"] =
            json!([{"source_id": source, "page_id": page, "unit_id": unit}]);
        let error = fixture.save(&value, 0).await.unwrap_err();
        assert_eq!(code(&error), "unit-page-source-mismatch", "{label}");
    }
    assert_eq!(fixture.drafts(), 0);
    // Correct wiring of both sources is accepted.
    base["entries"][0]["description"]["units"] = json!([
        {"source_id": a, "page_id": pa, "unit_id": ua},
        {"source_id": source_b, "page_id": page_b, "unit_id": unit_b}
    ]);
    assert_eq!(fixture.save(&base, 0).await.unwrap(), 1);
}

#[tokio::test]
async fn identical_content_with_the_current_revision_still_advances_it() {
    let _guard = TEST_LOCK.lock().await;
    let fixture = fixture().await;
    let bytes = serde_json::to_vec(&fixture.catalogue()).unwrap();
    let first = fixture
        .store
        .save_catalogue_draft(bytes.clone(), 0)
        .await
        .unwrap();
    let second = fixture.store.save_catalogue_draft(bytes, 1).await.unwrap();
    assert_eq!((first.revision, second.revision), (1, 2));
    assert_eq!(first.content_hash, second.content_hash);
    assert_eq!(first.id, second.id);
}

#[tokio::test]
async fn a_nonnative_pair_with_a_built_in_adapter_is_refused() {
    let _guard = TEST_LOCK.lock().await;
    let fixture = fixture().await;
    // The only adapter pair ships registered as native and registry editions are immutable,
    // so no product path builds this state. This test database drops the immutability
    // trigger and flips the flag, to reach the adapter predicate on its own.
    let db = rusqlite::Connection::open(&fixture.database).unwrap();
    db.execute_batch("DROP TRIGGER registry_editions_immutable_update")
        .unwrap();
    let changed = db
        .execute(
            "UPDATE registry_editions SET native=0 WHERE game_id='mork-borg' AND edition_id='it-2025'",
            [],
        )
        .unwrap();
    assert_eq!(changed, 1);
    let mut value = fixture.catalogue();
    value["game_id"] = "mork-borg".into();
    value["edition_id"] = "it-2025".into();
    value["sources"] = json!([]);
    value["entries"][0]["description"] = json!({"owner_text": "Synthetic."});
    assert_eq!(
        code(&fixture.save(&value, 0).await.unwrap_err()),
        "registered-nonnative-ruleset-required"
    );
}
