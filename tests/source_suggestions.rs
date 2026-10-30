//! Synthetic reports only: import never approves evidence.
use mimik::{
    AppError, Store,
    sources::{
        OwnerUnit, SourceSuggestion, SuggestionApplication, SuggestionTarget, SuggestionUnit,
        UnitReview,
    },
};
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use std::{fs, path::PathBuf};

#[path = "support/source_fixture.rs"]
pub mod fixture;
#[path = "support/schema_fixture.rs"]
mod schema_fixture;

struct Context {
    dir: tempfile::TempDir,
    store: Store,
    db: Connection,
    report: PathBuf,
    bundle: PathBuf,
    pdf: PathBuf,
}

async fn context() -> Context {
    context_with_second_page(false).await
}

async fn context_with_second_page(second_page: bool) -> Context {
    context_with_polygon(second_page, None).await
}

async fn context_with_polygon(second_page: bool, polygon: Option<Value>) -> Context {
    let dir = tempfile::tempdir().unwrap();
    let (bundle, pdf) = fixture::fixture(dir.path()).unwrap();
    // Add a container and another extracted unit, retaining the shared fixture's validated shapes.
    let raw_path = bundle.join("source.json");
    let mut raw: Value = serde_json::from_slice(&fs::read(&raw_path).unwrap()).unwrap();
    let mut first = raw["children"][0]["children"][0].clone();
    if let Some(polygon) = &polygon {
        first["polygon"] = polygon.clone();
    }
    let mut second = first.clone();
    second["id"] = json!("/page/0/Text/2");
    raw["children"][0]["children"] = json!([{"id":"/page/0/Section/1","block_type":"Section","html":"","polygon":[],"children":[first,second]}]);
    let bytes = serde_json::to_vec(&raw).unwrap();
    fs::write(raw_path, &bytes).unwrap();
    let manifest = bundle.join("bundle.json");
    let mut prepared: Value = serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();
    prepared["extraction_sha256"] = json!(fixture::hash(&bytes));
    if let Some(polygon) = polygon {
        prepared["pages"][0]["units"][0]["polygon"] = polygon;
    }
    let mut second = prepared["pages"][0]["units"][0].clone();
    second["id"] = json!("/page/0/Text/2");
    prepared["pages"][0]["units"]
        .as_array_mut()
        .unwrap()
        .push(second);
    if second_page {
        let mut document = lopdf::Document::load(&pdf).unwrap();
        let first = document.get_pages()[&1];
        let page = document.get_dictionary(first).unwrap().clone();
        let parent = page.get(b"Parent").unwrap().as_reference().unwrap();
        let second = document.add_object(page);
        let pages = document.get_dictionary_mut(parent).unwrap();
        pages
            .get_mut(b"Kids")
            .unwrap()
            .as_array_mut()
            .unwrap()
            .push(second.into());
        pages.set("Count", 2);
        document.save(&pdf).unwrap();
        prepared["source_sha256"] = json!(fixture::hash(&fs::read(&pdf).unwrap()));
        let raw_page: Value = serde_json::from_str(
            &raw["children"][0]
                .to_string()
                .replace("/page/0/", "/page/1/"),
        )
        .unwrap();
        raw["children"].as_array_mut().unwrap().push(raw_page);
        let bytes = serde_json::to_vec(&raw).unwrap();
        fs::write(bundle.join("source.json"), &bytes).unwrap();
        prepared["extraction_sha256"] = json!(fixture::hash(&bytes));
        let mut page: Value = serde_json::from_str(
            &prepared["pages"][0]
                .to_string()
                .replace("/page/0/", "/page/1/"),
        )
        .unwrap();
        page["pdf_page"] = json!(2);
        prepared["pages"].as_array_mut().unwrap().push(page);
        fs::copy(bundle.join("page-1.png"), bundle.join("page-2.png")).unwrap();
    }
    fs::write(manifest, serde_json::to_vec(&prepared).unwrap()).unwrap();
    let path = dir.path().join("reports.sqlite3");
    let store = Store::open(&path).unwrap();
    store.import_source(&bundle, &pdf).await.unwrap();
    let report = dir.path().join("report.json");
    Context {
        dir,
        store,
        db: Connection::open(path).unwrap(),
        report,
        bundle,
        pdf,
    }
}

fn issue(block: Value, text: Value) -> Value {
    let mut issue = json!({"kind":"merge","description":"Check original image"});
    issue["block_id"] = block;
    issue["suggested_correction"] = text;
    issue
}
fn report(c: &Context, version: u32, issues: Vec<Value>) -> Value {
    let mut report = json!({"schema_version":version,"state":"tool_suggestions_not_verified","tool":"fixture","source_sha256":fixture::hash(&fs::read(&c.pdf).unwrap()),"bundle_sha256":fixture::hash(&fs::read(c.bundle.join("bundle.json")).unwrap()),"created_at":"2026-10-30T12:00:00Z","pages":[{"pdf_page":1,"state":"tool_suggestions_not_verified","findings":{"summary":"Fixture","needs_visual_review":true}}]});
    report["pages"][0]["findings"]["issues"] = Value::Array(issues);
    report
}
async fn import(
    c: &Context,
    value: &Value,
) -> Result<mimik::sources::SuggestionReportSummary, AppError> {
    fs::write(&c.report, serde_json::to_vec(value).unwrap()).unwrap();
    c.store.import_suggestion_report(1, &c.report).await
}
fn count(db: &Connection, table: &str) -> i64 {
    db.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}
fn advice(c: &Context, kind: &str, target: SuggestionTarget) -> SourceSuggestion {
    SourceSuggestion {
        source: 1,
        bundle_hash: c
            .db
            .query_row("SELECT bundle_hash FROM sources", [], |r| r.get(0))
            .unwrap(),
        origin: "tool:fixture".into(),
        kind: kind.into(),
        target,
        suggested_text: if kind == "reject" {
            None
        } else {
            Some("Advice".into())
        },
        rationale: "Check image".into(),
        report_id: None,
    }
}
fn unit_target() -> SuggestionTarget {
    SuggestionTarget::Unit {
        target: SuggestionUnit {
            unit: 1,
            revision: 1,
        },
    }
}
fn application(suggestion: i64, event: i64) -> SuggestionApplication {
    SuggestionApplication {
        suggestion_id: suggestion,
        event_id: event,
        operation_id: None,
        owner_unit_id: None,
        edited: false,
    }
}
async fn correction(c: &Context) -> i64 {
    c.store
        .review_source_unit(
            1,
            1,
            UnitReview {
                revision: 1,
                state: "corrected".into(),
                correction: "Advice".into(),
                actor: "owner".into(),
                rationale: "Read image".into(),
            },
        )
        .await
        .unwrap();
    c.db.query_row("SELECT max(id) FROM source_review_events", [], |r| r.get(0))
        .unwrap()
}

#[tokio::test]
async fn v1_shape_mapping_both_bindings_repeat_and_revision_one() {
    let c = context().await;
    correction(&c).await;
    let mut correction_issue = issue(json!("/page/0/Text/1"), json!("Advice"));
    correction_issue["kind"] = json!("reject");
    let mut reject_note = issue(json!("/page/0/Text/1"), Value::Null);
    reject_note["kind"] = json!("reject");
    let value = report(
        &c,
        1,
        vec![
            correction_issue,
            issue(json!("/page/0/Text/1"), Value::Null),
            issue(json!("/page/0/Section/1"), json!("container advice")),
            issue(json!("/page/0/Page/0"), Value::Null),
            issue(Value::Null, json!("uncited text")),
            reject_note,
        ],
    );
    let result = import(&c, &value).await.unwrap();
    assert_eq!((result.counts["correction"], result.counts["note"]), (1, 5));
    assert_eq!(result.binding, "bundle");
    let targets: Vec<Value> =
        c.db.prepare("SELECT targets FROM source_suggestions ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .map(|r| serde_json::from_str(&r.unwrap()).unwrap())
            .collect();
    assert_eq!(targets[0]["target"]["revision"], 1);
    assert_eq!(targets[1]["type"], "unit");
    assert_eq!(
        targets[2],
        json!({"type":"page","page":1,"block_id":"/page/0/Section/1"})
    );
    assert_eq!(targets[4], json!({"type":"page","page":1}));
    assert!(import(&c, &value).await.unwrap().repeat);
    assert_eq!(count(&c.db, "source_suggestions"), 6);
    let mut extraction = value;
    extraction.as_object_mut().unwrap().remove("bundle_sha256");
    extraction["artifact_sha256"] =
        json!({"source.json":fixture::hash(&fs::read(c.bundle.join("source.json")).unwrap())});
    assert_eq!(import(&c, &extraction).await.unwrap().binding, "extraction");
    extraction["schema_version"] = json!(2);
    assert!(import(&c, &extraction).await.is_err());
}

#[tokio::test]
async fn v2_all_actions_are_advice_and_reports_are_immutable() {
    let c = context().await;
    let mut issues = Vec::new();
    for (action, block, text) in [
        ("correction", json!("/page/0/Text/1"), json!("Advice")),
        ("reject", json!("/page/0/Text/2"), Value::Null),
        ("note", Value::Null, Value::Null),
        ("merge", Value::Null, json!("joined")),
        ("missing_text", Value::Null, json!("missing")),
    ] {
        let mut i = issue(block, text);
        i["action"] = json!(action);
        if action == "merge" {
            i["merge_block_ids"] = json!(["/page/0/Text/1", "/page/0/Text/2"]);
        }
        if action == "missing_text" {
            i["region"] = json!([[1, 1], [20, 1], [20, 20], [1, 20]]);
        }
        issues.push(i);
    }
    let summary = import(&c, &report(&c, 2, issues)).await.unwrap();
    assert!(summary.counts.values().all(|n| *n == 1));
    assert_eq!(count(&c.db, "source_review_events"), 0);
    assert!(
        c.store
            .source_page(1)
            .await
            .unwrap()
            .units
            .iter()
            .all(|u| u.revision == 1)
    );
    for table in ["source_suggestions", "source_suggestion_reports"] {
        assert!(c.db.execute(&format!("DELETE FROM {table}"), []).is_err());
        assert!(
            c.db.execute(&format!("UPDATE {table} SET origin=origin"), [])
                .is_err()
        );
        assert!(
            c.db.execute(
                &format!("INSERT OR REPLACE INTO {table} SELECT * FROM {table}"),
                []
            )
            .is_err()
        );
    }
}

#[tokio::test]
async fn report_refuses_bad_bindings_pages_citations_rationales_and_closed_fields_atomically() {
    let c = context().await;
    let good = report(&c, 1, vec![issue(json!("/page/0/Text/1"), json!("Advice"))]);
    let mut bads = Vec::new();
    for field in ["source_sha256", "bundle_sha256"] {
        let mut bad = good.clone();
        bad[field] = json!("0".repeat(64));
        bads.push(bad);
    }
    let mut bad = good.clone();
    bad["pages"][0]["pdf_page"] = json!(99);
    bads.push(bad);
    let mut bad = good.clone();
    bad["pages"][0]["findings"]["issues"][0]["block_id"] = json!("/page/0/Text/99");
    bads.push(bad);
    for text in ["", "   "] {
        let mut bad = good.clone();
        bad["pages"][0]["findings"]["issues"][0]["description"] = json!(text);
        bads.push(bad);
    }
    for path in ["top", "page", "findings", "issue"] {
        let mut bad = good.clone();
        let object = match path {
            "top" => &mut bad,
            "page" => &mut bad["pages"][0],
            "findings" => &mut bad["pages"][0]["findings"],
            _ => &mut bad["pages"][0]["findings"]["issues"][0],
        };
        object["extra"] = json!(true);
        bads.push(bad);
    }
    for bad in bads {
        assert!(import(&c, &bad).await.is_err(), "{bad}");
        assert_eq!(count(&c.db, "source_suggestion_reports"), 0);
        assert_eq!(count(&c.db, "source_suggestions"), 0);
    }
    let mut bad = good.clone();
    bad["pages"][0]["findings"]["issues"][0]["description"] = json!("");
    assert!(matches!(
        import(&c, &bad).await,
        Err(AppError::SuggestionReportIssue {
            page: 1,
            index: 0,
            ..
        })
    ));
    // A distinct reading of the same PDF also requires its own bundle binding.
    let path = c.bundle.join("bundle.json");
    let mut manifest: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    manifest["title"] = json!("other source");
    fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let second = c.store.import_source(&c.bundle, &c.pdf).await.unwrap();
    fs::write(&c.report, serde_json::to_vec(&good).unwrap()).unwrap();
    assert!(
        c.store
            .import_suggestion_report(second, &c.report)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn failed_pages_preserve_metadata_without_suggestions() {
    let c = context().await;
    let mut value = report(&c, 1, vec![]);
    value["state"] = json!("model_suggestions_not_verified");
    value.as_object_mut().unwrap().remove("tool");
    value["endpoint"] = json!("https://example.invalid/v1");
    value["requested_model"] = json!("synthetic-model");
    value["pages"] = json!([{"pdf_page":1,"state":"failed","error":"provider failed","render_sha256":"a".repeat(64),"returned_model":null,"usage":{"output_tokens":1}}]);
    let summary = import(&c, &value).await.unwrap();
    assert_eq!(summary.failed_pages, 1);
    assert_eq!(count(&c.db, "source_suggestions"), 0);
    assert_eq!(
        c.db.query_row("SELECT origin FROM source_suggestion_reports", [], |r| {
            r.get::<_, String>(0)
        })
        .unwrap(),
        "reviewer:synthetic-model"
    );
}

#[tokio::test]
async fn optional_fields_cannot_smuggle_null_or_version_two_fields_into_v1() {
    let c = context().await;
    let base = report(&c, 1, vec![issue(Value::Null, Value::Null)]);
    for key in ["action", "region", "merge_block_ids"] {
        let mut bad = base.clone();
        bad["pages"][0]["findings"]["issues"][0][key] = Value::Null;
        assert!(
            import(&c, &bad).await.is_err(),
            "v1 {key}: null must be refused"
        );
    }
    for key in ["block_id", "suggested_correction"] {
        let mut bad = base.clone();
        bad["pages"][0]["findings"]["issues"][0]
            .as_object_mut()
            .unwrap()
            .remove(key);
        assert!(import(&c, &bad).await.is_err(), "missing {key}");
    }
    let mut bad = base.clone();
    bad["schema_version"] = json!(2);
    bad["pages"][0]["findings"]["issues"][0]["action"] = json!("note");
    bad["pages"][0]["findings"]["issues"][0]["region"] = Value::Null;
    assert!(import(&c, &bad).await.is_err());
    for key in ["artifact_sha256", "tool", "endpoint", "requested_model"] {
        let mut bad = base.clone();
        bad[key] = Value::Null;
        assert!(import(&c, &bad).await.is_err(), "{key}: null");
    }
}

#[tokio::test]
async fn v2_shape_geometry_and_merge_bounds_fail_closed() {
    let c = context().await;
    let base = issue(json!("/page/0/Text/1"), json!("Advice"));
    let mut bads = Vec::new();
    for action in ["bogus", "reject", "merge", "missing_text"] {
        let mut i = base.clone();
        i["action"] = json!(action);
        bads.push(i);
    }
    for polygon in [
        json!([[0, 0], [1, 1], [1, 0], [0, 1]]),
        json!([[0, 0], [300, 0], [300, 20], [0, 20]]),
        json!([[0, 0], [0, 0], [0, 0], [0, 0]]),
        Value::Null,
    ] {
        let mut i = issue(Value::Null, json!("Missing"));
        i["action"] = json!("missing_text");
        i["region"] = polygon;
        bads.push(i);
    }
    for blocks in [
        json!(["/page/0/Text/1"]),
        json!(["/page/0/Text/1", "/page/0/Text/1"]),
        json!(["/page/0/Text/1", "/page/0/Section/1"]),
        json!(vec!["/page/0/Text/1"; 1025]),
    ] {
        let mut i = issue(Value::Null, json!("Merge"));
        i["action"] = json!("merge");
        i["merge_block_ids"] = blocks;
        bads.push(i);
    }
    let mut i = base.clone();
    i["action"] = json!("note");
    i["region"] = json!([[0, 0], [1, 0], [1, 1], [0, 1]]);
    bads.push(i);
    for i in bads {
        assert!(
            import(&c, &report(&c, 2, vec![i.clone()])).await.is_err(),
            "{i}"
        );
    }
    assert_eq!(count(&c.db, "source_suggestions"), 0);
}

#[tokio::test]
async fn limits_regular_files_and_five_thousand_issue_transaction() {
    let c = context().await;
    let i = issue(Value::Null, Value::Null);
    assert!(
        import(&c, &report(&c, 1, vec![i.clone(); 5001]))
            .await
            .is_err()
    );
    fs::write(&c.report, vec![b' '; 4 * 1024 * 1024 + 1]).unwrap();
    assert!(
        c.store
            .import_suggestion_report(1, &c.report)
            .await
            .is_err()
    );
    fs::remove_file(&c.report).unwrap();
    std::os::unix::fs::symlink(c.bundle.join("bundle.json"), &c.report).unwrap();
    assert!(
        c.store
            .import_suggestion_report(1, &c.report)
            .await
            .is_err()
    );
    fs::remove_file(&c.report).unwrap();
    rustix::fs::mknodat(
        rustix::fs::CWD,
        &c.report,
        rustix::fs::FileType::Fifo,
        rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
        0,
    )
    .unwrap();
    assert!(
        c.store
            .import_suggestion_report(1, &c.report)
            .await
            .is_err()
    );
    fs::remove_file(&c.report).unwrap();
    let start = std::time::Instant::now();
    let summary = import(&c, &report(&c, 1, vec![i; 5000])).await.unwrap();
    eprintln!(
        "synthetic 5,000-issue import duration: {:?}",
        start.elapsed()
    );
    assert_eq!(summary.counts["note"], 5000);
    assert_eq!(count(&c.db, "source_suggestions"), 5000);
}

#[tokio::test]
async fn migration_24_keeps_ids_dismissals_and_rewind_restores_original_guards() {
    let c = context().await;
    schema_fixture::rewind_review(&c.db);
    c.db.execute("INSERT INTO source_suggestions(id,source_id,bundle_hash,origin,kind,targets,suggested_text,rationale) SELECT 42,1,bundle_hash,'tool','correction',?1,'Advice','why' FROM sources WHERE id=1",[r#"{"type":"unit","target":{"unit":1,"revision":1}}"#]).unwrap();
    c.db.execute("INSERT INTO source_suggestions(id,source_id,bundle_hash,origin,kind,targets,rationale) SELECT 7,1,bundle_hash,'tool','reject',?1,'Other advice' FROM sources WHERE id=1",[r#"{"type":"unit","target":{"unit":2,"revision":1}}"#]).unwrap();
    c.db.execute("UPDATE source_suggestions SET dismissed_by='legacy',dismissed_at='2026-10-30 12:00:00' WHERE id=42",[]).unwrap();
    drop(c.store);
    let _migrated = Store::open(&c.dir.path().join("reports.sqlite3")).unwrap();
    assert_eq!(count(&c.db, "source_suggestions"), 2);
    let open: (i64, Option<i64>, String, String) =
        c.db.query_row(
            "SELECT id,report_id,kind,rationale FROM source_suggestions WHERE id=7",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    assert_eq!(open, (7, None, "reject".into(), "Other advice".into()));
    let row: (i64,Option<i64>,String,String)=c.db.query_row("SELECT s.id,s.report_id,d.actor,d.created_at FROM source_suggestions s JOIN source_suggestion_dismissals d ON d.suggestion_id=s.id",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).unwrap();
    assert_eq!(
        row,
        (42, None, "legacy".into(), "2026-10-30 12:00:00".into())
    );
    schema_fixture::rewind_review(&c.db);
    assert_eq!(
        c.db.query_row(
            "SELECT dismissed_by FROM source_suggestions WHERE id=42",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "legacy"
    );
    assert!(
        c.db.execute("UPDATE source_suggestions SET dismissed_by='again'", [])
            .is_err()
    );
}

#[tokio::test]
async fn sql_notes_reports_and_null_envelopes() {
    let c = context().await;
    let insert = |target: &str| {
        c.db.execute("INSERT INTO source_suggestions(source_id,bundle_hash,origin,kind,targets,rationale) SELECT 1,bundle_hash,'tool','note',?1,'why' FROM sources WHERE id=1",[target])
    };
    for target in [
        r#"{"type":"page","page":1}"#,
        r#"{"type":"page","page":1,"block_id":"raw"}"#,
        r#"{"type":"unit","target":{"unit":1,"revision":1}}"#,
    ] {
        insert(target).unwrap();
    }
    for target in [
        r#"{"type":"page"}"#,
        r#"{"type":"page","page":null}"#,
        r#"{"type":"page","page":2}"#,
        r#"{"type":"page","page":1,"block_id":null}"#,
        r#"{"type":"page","page":1,"block_id":42}"#,
        r#"{"type":"unit","target":{"unit":1,"revision":null}}"#,
    ] {
        assert!(insert(target).is_err(), "{target}");
    }
    for block in ["x".repeat(513), "x\0y".into()] {
        assert!(insert(&json!({"type":"page","page":1,"block_id":block}).to_string()).is_err());
    }
    let value = report(&c, 1, vec![]);
    import(&c, &value).await.unwrap();
    assert!(c.db.execute("INSERT INTO source_suggestions(source_id,bundle_hash,origin,kind,targets,rationale,report_id) SELECT 1,bundle_hash,'tool','note','{\"type\":\"page\",\"page\":1}','why',999 FROM sources WHERE id=1",[]).is_err());
    // Report id from a distinct source, with foreign keys off too.
    c.db.execute_batch("PRAGMA foreign_keys=OFF").unwrap();
    c.db.execute("INSERT INTO sources(bundle_hash,source_id,title,system_id,edition,language,pdf_hash,pdf,raw_json,prepared_json) VALUES('second','s','t','mork-borg','e','en','other',x'00',x'7b7d',x'7b7d')",[]).unwrap();
    c.db.execute("INSERT INTO source_suggestion_reports(source_id,report_sha256,origin,schema_version,binding,created_at,correction,reject,merge,missing_text,note,failed_pages) VALUES(2,?1,'tool',1,'bundle','now',0,0,0,0,0,0)",["a".repeat(64)]).unwrap();
    assert!(c.db.execute("INSERT INTO source_suggestions(source_id,bundle_hash,origin,kind,targets,rationale,report_id) SELECT 1,bundle_hash,'tool','note','{\"type\":\"page\",\"page\":1}','why',2 FROM sources WHERE id=1",[]).is_err());
}

#[tokio::test]
async fn application_and_dismissal_exclude_each_other_and_do_not_change_evidence() {
    let c = context().await;
    let first = c
        .store
        .insert_source_suggestion(advice(&c, "correction", unit_target()))
        .await
        .unwrap();
    let second = c
        .store
        .insert_source_suggestion(advice(&c, "correction", unit_target()))
        .await
        .unwrap();
    c.store
        .dismiss_source_suggestion(second, "owner".into(), Some("Not this reading".into()))
        .await
        .unwrap();
    let event = correction(&c).await;
    assert!(matches!(
        c.store
            .record_source_suggestion_application(application(second, event))
            .await,
        Err(AppError::Conflict)
    ));
    assert!(c.db.execute("INSERT INTO source_suggestion_applications(suggestion_id,event_id,edited) VALUES(?1,?2,0)",params![second,event]).is_err());
    c.store
        .record_source_suggestion_application(application(first, event))
        .await
        .unwrap();
    assert!(matches!(
        c.store
            .dismiss_source_suggestion(first, "owner".into(), None)
            .await,
        Err(AppError::Conflict)
    ));
    assert!(
        c.db.execute(
            "INSERT INTO source_suggestion_dismissals(suggestion_id,actor) VALUES(?1,'owner')",
            [first]
        )
        .is_err()
    );
    assert!(matches!(
        c.store
            .dismiss_source_suggestion(second, "owner".into(), None)
            .await,
        Err(AppError::Conflict)
    ));
    assert_eq!(count(&c.db, "source_review_events"), 1);
    assert_eq!(c.store.source_page(1).await.unwrap().units[0].revision, 2);
    for table in [
        "source_suggestion_applications",
        "source_suggestion_dismissals",
    ] {
        assert!(c.db.execute(&format!("DELETE FROM {table}"), []).is_err());
        let column = if table.ends_with("applications") {
            "edited"
        } else {
            "actor"
        };
        assert!(
            c.db.execute(&format!("UPDATE {table} SET {column}={column}"), [])
                .is_err()
        );
        assert!(
            c.db.execute(
                &format!("INSERT OR REPLACE INTO {table} SELECT * FROM {table}"),
                []
            )
            .is_err()
        );
    }
}

#[tokio::test]
async fn application_requires_latest_matching_event_and_kind() {
    let c = context().await;
    let correction_id = c
        .store
        .insert_source_suggestion(advice(&c, "correction", unit_target()))
        .await
        .unwrap();
    let reject = c
        .store
        .insert_source_suggestion(advice(&c, "reject", unit_target()))
        .await
        .unwrap();
    let note = c
        .store
        .insert_source_suggestion(advice(&c, "note", unit_target()))
        .await
        .unwrap();
    let event = correction(&c).await;
    for id in [reject, note] {
        assert!(
            c.store
                .record_source_suggestion_application(application(id, event))
                .await
                .is_err()
        );
    }
    let mut wrong = application(correction_id, event);
    wrong.owner_unit_id = Some(2);
    assert!(
        c.store
            .record_source_suggestion_application(wrong)
            .await
            .is_err()
    );
    c.store
        .review_source_unit(
            1,
            2,
            UnitReview {
                revision: 1,
                state: "rejected".into(),
                correction: String::new(),
                actor: "owner".into(),
                rationale: "Other unit".into(),
            },
        )
        .await
        .unwrap();
    assert!(
        c.store
            .record_source_suggestion_application(application(correction_id, event))
            .await
            .is_err()
    );
    assert!(
        c.store
            .record_source_suggestion_application(application(correction_id, event + 1))
            .await
            .is_err()
    );
    assert_eq!(count(&c.db, "source_suggestion_applications"), 0);
}

#[tokio::test]
async fn missing_text_application_needs_new_owner_correction_on_its_page() {
    let c = context().await;
    let polygon = [[1.0, 1.0], [20.0, 1.0], [20.0, 20.0], [1.0, 20.0]];
    let id = c
        .store
        .insert_source_suggestion(advice(
            &c,
            "missing_text",
            SuggestionTarget::Region { page: 1, polygon },
        ))
        .await
        .unwrap();
    let event = c
        .store
        .add_owner_unit(
            1,
            OwnerUnit {
                revision: 1,
                polygon,
                text: "Advice".into(),
                actor: "owner".into(),
                rationale: "Read original".into(),
            },
        )
        .await
        .unwrap();
    assert!(
        c.store
            .record_source_suggestion_application(application(id, event))
            .await
            .is_err()
    );
    let mut app = application(id, event);
    app.owner_unit_id = Some(3);
    c.store
        .record_source_suggestion_application(app)
        .await
        .unwrap();
}

#[tokio::test]
async fn apply_racing_dismiss_leaves_exactly_one_state() {
    let c = context().await;
    let id = c
        .store
        .insert_source_suggestion(advice(&c, "correction", unit_target()))
        .await
        .unwrap();
    let event = correction(&c).await;
    let competitor = Store::open(&c.dir.path().join("reports.sqlite3")).unwrap();
    let (a, b) = tokio::join!(
        c.store
            .record_source_suggestion_application(application(id, event)),
        competitor.dismiss_source_suggestion(id, "owner".into(), None)
    );
    assert_ne!(a.is_ok(), b.is_ok());
    assert_eq!(
        count(&c.db, "source_suggestion_applications")
            + count(&c.db, "source_suggestion_dismissals"),
        1
    );
}

#[tokio::test]
async fn merge_application_requires_batch_rejections_and_its_new_owner_unit() {
    use mimik::sources::UnitDecision;
    let c = context().await;
    let targets = vec![
        SuggestionUnit {
            unit: 1,
            revision: 1,
        },
        SuggestionUnit {
            unit: 2,
            revision: 1,
        },
    ];
    let id = c
        .store
        .insert_source_suggestion(advice(&c, "merge", SuggestionTarget::Units { targets }))
        .await
        .unwrap();
    c.db.execute("INSERT INTO source_units(page_id,block_id,kind,polygon,raw_html,raw_text,origin) VALUES(1,'owner:1','Text','[[1,1],[100,1],[100,40],[1,40]]','<p>Advice</p>','Advice','owner')",[]).unwrap();
    let decisions = (1..=3)
        .rev()
        .map(|unit| UnitDecision {
            page: 1,
            unit,
            review: UnitReview {
                revision: 1,
                state: if unit == 3 { "corrected" } else { "rejected" }.into(),
                correction: if unit == 3 { "Advice" } else { "" }.into(),
                actor: "owner".into(),
                rationale: "Merge reading".into(),
            },
        })
        .collect();
    let events = c
        .store
        .review_units_atomic(decisions, "Merge reading".into(), |_, _| Ok(()))
        .await
        .unwrap();
    let event = *events.last().unwrap();
    let operation: String =
        c.db.query_row(
            "SELECT operation_id FROM source_review_events WHERE id=?1",
            [event],
            |r| r.get(0),
        )
        .unwrap();
    for (op, owner) in [
        (None, Some(3)),
        (Some("wrong".into()), Some(3)),
        (Some(operation.clone()), Some(1)),
        (Some(operation.clone()), None),
    ] {
        let app = SuggestionApplication {
            operation_id: op,
            owner_unit_id: owner,
            ..application(id, event)
        };
        assert!(
            c.store
                .record_source_suggestion_application(app)
                .await
                .is_err()
        );
    }
    let app = SuggestionApplication {
        operation_id: Some(operation),
        owner_unit_id: Some(3),
        edited: true,
        ..application(id, event)
    };
    c.store
        .record_source_suggestion_application(app)
        .await
        .unwrap();
    assert_eq!(count(&c.db, "source_suggestion_applications"), 1);
}

#[tokio::test]
async fn dismissal_sidecar_binds_authenticated_actor_and_latest_dismissal_and_is_immutable() {
    let c = context().await;
    let a = c
        .store
        .insert_source_suggestion(advice(&c, "note", unit_target()))
        .await
        .unwrap();
    let b = c
        .store
        .insert_source_suggestion(advice(&c, "note", unit_target()))
        .await
        .unwrap();
    c.store
        .dismiss_source_suggestion(a, "account:1".into(), None)
        .await
        .unwrap();
    c.store
        .dismiss_source_suggestion(b, "account:1".into(), None)
        .await
        .unwrap();
    // Test the sidecar's boundary with a synthetic authenticated-request view. The real
    // request view and session rechecks have their own authentication tests.
    c.db.execute_batch("PRAGMA foreign_keys=OFF; DROP VIEW account_web_request; CREATE VIEW account_web_request AS SELECT 1 actor_id,2 actor_epoch,printf('%064d',0) session_digest;").unwrap();
    let insert = |event, actor, epoch| {
        c.db.execute(
            "INSERT INTO source_suggestion_web_events VALUES(?1,?2,?3,?4)",
            params![event, actor, epoch, "0".repeat(64)],
        )
    };
    assert!(insert(1, 1, 2).is_err());
    assert!(insert(2, 2, 2).is_err());
    assert!(insert(2, 1, 1).is_err());
    insert(2, 1, 2).unwrap();
    for sql in [
        "UPDATE source_suggestion_web_events SET actor_id=actor_id",
        "DELETE FROM source_suggestion_web_events",
        "INSERT OR REPLACE INTO source_suggestion_web_events SELECT * FROM source_suggestion_web_events",
    ] {
        assert!(c.db.execute(sql, []).is_err(), "{sql}");
    }
}

#[tokio::test]
async fn merge_application_cannot_create_its_owner_unit_on_another_page() {
    use mimik::sources::UnitDecision;
    let c = context_with_second_page(true).await;
    let targets = (1..=2)
        .map(|unit| SuggestionUnit { unit, revision: 1 })
        .collect();
    let id = c
        .store
        .insert_source_suggestion(advice(&c, "merge", SuggestionTarget::Units { targets }))
        .await
        .unwrap();
    c.db.execute("INSERT INTO source_units(page_id,block_id,kind,polygon,raw_html,raw_text,origin) VALUES(2,'owner:1','Text','[[1,1],[100,1],[100,40],[1,40]]','<p>Advice</p>','Advice','owner')", []).unwrap();
    let decisions = [1, 2, 5]
        .into_iter()
        .map(|unit| UnitDecision {
            page: if unit == 5 { 2 } else { 1 },
            unit,
            review: UnitReview {
                revision: 1,
                state: if unit == 5 { "corrected" } else { "rejected" }.into(),
                correction: if unit == 5 { "Advice" } else { "" }.into(),
                actor: "owner".into(),
                rationale: "Read image".into(),
            },
        })
        .collect();
    let events = c
        .store
        .review_units_atomic(decisions, "Read image".into(), |_, _| Ok(()))
        .await
        .unwrap();
    let event = *events.last().unwrap();
    let operation: String =
        c.db.query_row(
            "SELECT operation_id FROM source_review_events WHERE id=?1",
            [event],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        c.db.execute(
            "INSERT INTO source_suggestion_applications VALUES(?1,?2,?3,5,0)",
            params![id, event, operation]
        )
        .is_err()
    );
    assert!(
        c.store
            .record_source_suggestion_application(SuggestionApplication {
                operation_id: Some(operation),
                owner_unit_id: Some(5),
                ..application(id, event)
            })
            .await
            .is_err()
    );
}

#[tokio::test]
async fn cli_import_prints_summary_and_repeat_without_owner_setup() {
    let c = context().await;
    fs::write(
        &c.report,
        serde_json::to_vec(&report(&c, 1, vec![issue(Value::Null, Value::Null)])).unwrap(),
    )
    .unwrap();
    for repeat in [false, true] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_mimik"))
            .arg("--database")
            .arg(c.dir.path().join("reports.sqlite3"))
            .args(["import-suggestions", "--source", "1", "--report"])
            .arg(&c.report)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let summary: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(summary["repeat"], repeat);
        assert_eq!(summary["counts"]["note"], 1);
        assert_eq!(summary["binding"], "bundle");
        assert_eq!(summary["failed_pages"], 0);
    }
}

#[tokio::test]
async fn report_merge_refuses_zero_area_and_cross_page_targets() {
    for c in [
        context_with_polygon(false, Some(json!([[10, 10], [10, 10], [10, 10], [10, 10]]))).await,
        context_with_second_page(true).await,
    ] {
        let mut i = issue(Value::Null, json!("Merged"));
        i["action"] = json!("merge");
        i["merge_block_ids"] = if count(&c.db, "source_pages") == 2 {
            json!(["/page/0/Text/1", "/page/1/Text/1"])
        } else {
            json!(["/page/0/Text/1", "/page/0/Text/2"])
        };
        assert!(import(&c, &report(&c, 2, vec![i])).await.is_err());
        assert_eq!(count(&c.db, "source_suggestion_reports"), 0);
    }
}

#[tokio::test]
async fn report_origin_and_dismissal_bounds_and_citation_page_are_enforced() {
    let c = context_with_second_page(true).await;
    let base = report(&c, 1, vec![issue(Value::Null, Value::Null)]);
    for tool in [String::new(), "x".repeat(96), "é".repeat(48), "x\0y".into()] {
        let mut value = base.clone();
        value["tool"] = json!(tool);
        assert!(import(&c, &value).await.is_err());
    }
    let mut wrong_page = base.clone();
    wrong_page["pages"][0]["findings"]["issues"][0]["block_id"] = json!("/page/1/Text/1");
    assert!(matches!(
        import(&c, &wrong_page).await,
        Err(AppError::SuggestionReportIssue {
            page: 1,
            index: 0,
            ..
        })
    ));
    let summary = import(&c, &base).await.unwrap();
    assert_eq!(summary.counts["note"], 1);
    let id: i64 =
        c.db.query_row("SELECT max(id) FROM source_suggestions", [], |r| r.get(0))
            .unwrap();
    for reason in ["x".repeat(5001), "x\0y".into()] {
        assert!(
            c.store
                .dismiss_source_suggestion(id, "owner".into(), Some(reason.clone()))
                .await
                .is_err()
        );
        assert!(c.db.execute("INSERT INTO source_suggestion_dismissals(suggestion_id,actor,reason) VALUES(?1,'owner',?2)", params![id,reason]).is_err());
    }
    c.store
        .dismiss_source_suggestion(id, "owner".into(), Some("é".repeat(5000)))
        .await
        .unwrap();
    assert_eq!(count(&c.db, "source_review_events"), 0);
    assert!(
        c.store
            .source_page(1)
            .await
            .unwrap()
            .units
            .iter()
            .all(|unit| unit.revision == 1)
    );
}

#[tokio::test]
async fn dismissal_preserves_complete_page_history_and_search() {
    use mimik::sources::{PageReview, UnitDecision};
    let c = context().await;
    let decisions = (1..=2)
        .map(|unit| UnitDecision {
            page: 1,
            unit,
            review: UnitReview {
                revision: 1,
                state: "accepted".into(),
                correction: String::new(),
                actor: "owner".into(),
                rationale: "Checked image".into(),
            },
        })
        .collect();
    c.store
        .review_units_atomic(decisions, "Checked image".into(), |_, _| Ok(()))
        .await
        .unwrap();
    c.store
        .complete_source_page(
            1,
            PageReview {
                revision: c.store.source_page(1).await.unwrap().revision,
                printed_label: "1".into(),
                actor: "owner".into(),
                rationale: "Checked omissions".into(),
            },
        )
        .await
        .unwrap();
    let id = c
        .store
        .insert_source_suggestion(advice(&c, "note", unit_target()))
        .await
        .unwrap();
    let snapshot = || {
        [
            "SELECT json_array(id,state,correction,revision) FROM source_units ORDER BY id",
            "SELECT json_array(id,complete,printed_label,revision) FROM source_pages ORDER BY id",
            "SELECT json_array(id,unit_id,state,correction,previous_revision) FROM source_review_events ORDER BY id",
            "SELECT json_array(unit_id,normaliser,search_text) FROM source_search_text ORDER BY unit_id",
            "SELECT json_array(rowid,body) FROM source_search_fts ORDER BY rowid",
        ].into_iter().flat_map(|sql| c.db.prepare(sql).unwrap().query_map([],|r|r.get::<_,String>(0)).unwrap().collect::<Result<Vec<_>,_>>().unwrap()).collect::<Vec<_>>()
    };
    let before = snapshot();
    c.store
        .dismiss_source_suggestion(id, "owner".into(), None)
        .await
        .unwrap();
    assert_eq!(snapshot(), before);
    assert!(c.store.source_page(1).await.unwrap().complete);
    assert_eq!(
        c.store
            .search_evidence(
                "mork-borg".into(),
                "fixture-edition".into(),
                "Strength".into()
            )
            .await
            .unwrap()
            .matches
            .len(),
        2
    );
}
