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

#[tokio::test]
async fn browser_review_fixture_imports_all_actions_and_repeats_without_changes() {
    let dir = tempfile::tempdir().unwrap();
    fixture::fixture_pair(dir.path()).unwrap();
    let store = Store::open(&dir.path().join("browser.sqlite3")).unwrap();
    store
        .import_source(
            &dir.path().join("review-bundle"),
            &dir.path().join("original.pdf"),
        )
        .await
        .unwrap();
    let source = store.sources().await.unwrap()[0].id;
    let report = dir.path().join("review-report.v2.json");
    let summary = store
        .import_suggestion_report(source, &report)
        .await
        .unwrap();
    assert_eq!(summary.binding, "bundle");
    assert!(!summary.repeat);
    let db = Connection::open(dir.path().join("browser.sqlite3")).unwrap();
    for (action, count) in [
        ("correction", 4),
        ("reject", 1),
        ("merge", 1),
        ("missing_text", 1),
        ("note", 2),
    ] {
        assert_eq!(summary.counts[action], count, "{action}");
        assert_eq!(
            db.query_row(
                "SELECT count(*) FROM source_suggestions WHERE kind=?1",
                [action],
                |row| row.get::<_, usize>(0),
            )
            .unwrap(),
            count,
            "stored {action}"
        );
    }
    assert_eq!(summary.failed_pages, 0);
    assert_eq!(
        db.query_row("SELECT count(*) FROM source_units", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        60
    );
    let repeated = store
        .import_suggestion_report(source, &report)
        .await
        .unwrap();
    assert!(repeated.repeat);
    assert_eq!(repeated.report_id, summary.report_id);
    assert_eq!(
        db.query_row("SELECT count(*) FROM source_suggestions", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        9
    );
}

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
    context_with_units(second_page, polygon, 2).await
}

async fn context_with_units(second_page: bool, polygon: Option<Value>, units: usize) -> Context {
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
    for index in 3..=units {
        let id = format!("/page/0/Text/{index}");
        let html = format!(
            "<p>{}</p>",
            "Synthetic extraction text with values +8 and d6. ".repeat(20)
        );
        let mut raw_unit = raw["children"][0]["children"][0]["children"][0].clone();
        raw_unit["id"] = json!(id);
        raw_unit["html"] = json!(html);
        raw["children"][0]["children"][0]["children"]
            .as_array_mut()
            .unwrap()
            .push(raw_unit);
        let mut unit = prepared["pages"][0]["units"][0].clone();
        unit["id"] = json!(id);
        unit["html"] = json!(html);
        prepared["pages"][0]["units"]
            .as_array_mut()
            .unwrap()
            .push(unit);
    }
    let bytes = serde_json::to_vec(&raw).unwrap();
    fs::write(bundle.join("source.json"), &bytes).unwrap();
    prepared["extraction_sha256"] = json!(fixture::hash(&bytes));
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
            index: 1,
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
    let mut oversized = report(&c, 1, vec![]);
    oversized["pages"][0]["findings"]["summary"] = json!("x".repeat(4 * 1024 * 1024));
    assert!(matches!(
        import(&c, &oversized).await,
        Err(AppError::Invalid("Local input exceeds its byte limit."))
    ));
    fs::remove_file(&c.report).unwrap();
    let valid_path = c.dir.path().join("valid-report.json");
    fs::write(
        &valid_path,
        serde_json::to_vec(&report(&c, 1, vec![])).unwrap(),
    )
    .unwrap();
    c.store
        .import_suggestion_report(1, &valid_path)
        .await
        .unwrap();
    std::os::unix::fs::symlink(&valid_path, &c.report).unwrap();
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
    let c = context_with_second_page(true).await;
    schema_fixture::rewind_review(&c.db);
    // This source and its pages genuinely originate under v24.
    insert_unfinalized_source(
        &c.db,
        2,
        &json!({"pages":[{"pdf_page":7,"width_points":123,"height_points":234}]}),
    );
    insert_unfinalized_page(&c.db, 30, 2, 7);
    c.db.execute(
        "UPDATE sources SET raw_json=?1 WHERE id=2",
        [
            serde_json::to_vec(&json!({"children":[{"id":"/page/6/Page/0","children":[]}]}))
                .unwrap(),
        ],
    )
    .unwrap();
    c.db.execute("INSERT INTO source_suggestions(id,source_id,bundle_hash,origin,kind,targets,suggested_text,rationale) SELECT 42,1,bundle_hash,'tool','correction',?1,'Advice','why' FROM sources WHERE id=1",[r#"{"type":"unit","target":{"unit":1,"revision":1}}"#]).unwrap();
    c.db.execute("INSERT INTO source_suggestions(id,source_id,bundle_hash,origin,kind,targets,rationale) SELECT 7,1,bundle_hash,'tool','reject',?1,'Other advice' FROM sources WHERE id=1",[r#"{"type":"unit","target":{"unit":2,"revision":1}}"#]).unwrap();
    c.db.execute("UPDATE source_suggestions SET dismissed_by='legacy',dismissed_at='2026-10-30 12:00:00' WHERE id=42",[]).unwrap();
    let mut value = report(
        &c,
        2,
        vec![
            json!({"action":"missing_text","block_id":null,"kind":"omission","description":"Read image","suggested_correction":"Advice","region":[[1,1],[20,1],[20,20],[1,20]]}),
        ],
    );
    drop(c.store);
    let migrated = Store::open(&c.dir.path().join("reports.sqlite3")).unwrap();
    assert_eq!(count(&c.db, "source_suggestion_page_bounds"), 3);
    let mismatches: i64 = c.db.query_row(
        "SELECT count(*) FROM source_pages p JOIN sources s ON s.id=p.source_id, json_each(s.prepared_json,'$.pages') g LEFT JOIN source_suggestion_page_bounds b ON b.source_id=p.source_id AND b.pdf_page=p.pdf_page WHERE json_extract(g.value,'$.pdf_page')=p.pdf_page AND (b.width IS NOT json_extract(g.value,'$.width_points') OR b.height IS NOT json_extract(g.value,'$.height_points'))",
        [], |r| r.get(0)).unwrap();
    assert_eq!(mismatches, 0);
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
    drop(migrated);
    let migrated = Store::open(&c.dir.path().join("reports.sqlite3")).unwrap();
    value["pages"][0]["pdf_page"] = json!(7);
    value["bundle_sha256"] = json!("b".repeat(64));
    value["source_sha256"] = json!("a".repeat(64));
    fs::write(&c.report, serde_json::to_vec(&value).unwrap()).unwrap();
    let summary = migrated
        .import_suggestion_report(2, &c.report)
        .await
        .unwrap();
    assert_eq!(summary.counts["missing_text"], 1);
}

fn insert_unfinalized_source(db: &Connection, id: i64, prepared: &Value) {
    db.execute("INSERT INTO sources(id,bundle_hash,source_id,title,system_id,edition,language,pdf_hash,raw_json,prepared_json) VALUES(?1,?2,'synthetic','Synthetic','mork-borg','fixture','en',?3,x'7b7d',?4)",params![id, "b".repeat(64), "a".repeat(64), serde_json::to_vec(prepared).unwrap()]).unwrap();
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM source_finalizations WHERE source_id=?1",
            [id],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
}

fn insert_unfinalized_page(db: &Connection, id: i64, source: i64, page: i64) {
    db.execute(
        "INSERT INTO source_pages(id,source_id,pdf_page,png) VALUES(?1,?2,?3,x'00')",
        params![id, source, page],
    )
    .unwrap();
}

fn insert_region(db: &Connection, source: i64, page: i64, extent: i64) -> rusqlite::Result<usize> {
    let target = json!({"type":"region","page":page,"polygon":[[0,0],[extent,0],[extent,extent],[0,extent]]});
    db.execute("INSERT INTO source_suggestions(source_id,bundle_hash,origin,kind,targets,suggested_text,rationale) SELECT id,bundle_hash,'tool:fixture','missing_text',?2,'Advice','Read image' FROM sources WHERE id=?1", params![source,target.to_string()])
}

#[test]
fn unfinalized_bounds_survive_update_or_replace_page_id_collision() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("source.sqlite3");
    let _store = Store::open(&path).unwrap();
    let db = Connection::open(path).unwrap();
    insert_unfinalized_source(
        &db,
        1,
        &json!({"pages":[{"pdf_page":1,"width_points":200,"height_points":300}]}),
    );
    insert_unfinalized_page(&db, 10, 1, 1);
    insert_unfinalized_page(&db, 20, 1, 2); // No prepared dimensions for B.
    db.execute("UPDATE OR REPLACE source_pages SET id=10 WHERE id=20", [])
        .unwrap();
    assert!(
        insert_region(&db, 1, 10, 20).is_err(),
        "A's bounds must not describe B"
    );
}

#[test]
fn unfinalized_bounds_survive_insert_or_replace_page() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("source.sqlite3");
    let _store = Store::open(&path).unwrap();
    let db = Connection::open(path).unwrap();
    insert_unfinalized_source(
        &db,
        1,
        &json!({"pages":[{"pdf_page":1,"width_points":200,"height_points":300},{"pdf_page":2,"width_points":40,"height_points":50}]}),
    );
    insert_unfinalized_page(&db, 10, 1, 1);
    db.execute(
        "INSERT OR REPLACE INTO source_pages(id,source_id,pdf_page,png) VALUES(10,1,2,x'00')",
        [],
    )
    .unwrap();
    insert_region(&db, 1, 10, 30).unwrap();
    assert!(insert_region(&db, 1, 10, 60).is_err());
}

#[test]
fn unfinalized_bounds_survive_source_replacement() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("source.sqlite3");
    let _store = Store::open(&path).unwrap();
    let db = Connection::open(path).unwrap();
    let prepared = json!({"pages":[{"pdf_page":1,"width_points":200,"height_points":300}]});
    insert_unfinalized_source(&db, 1, &prepared);
    insert_unfinalized_page(&db, 10, 1, 1);
    // Replacing the source by id or by bundle hash, with metadata that has no pages, must not
    // leave the old bounds describing the replacement.
    for (id, bundle) in [
        (1, "b".repeat(64)),
        (2, "b".repeat(64)),
        (1, "c".repeat(64)),
    ] {
        assert!(
            db.execute("INSERT OR REPLACE INTO sources(id,bundle_hash,source_id,title,system_id,edition,language,pdf_hash,raw_json,prepared_json) VALUES(?1,?2,'synthetic','Synthetic','mork-borg','fixture','en',?3,x'7b7d',x'7b7d')", params![id, bundle, "a".repeat(64)]).is_err(),
            "replacement {id} {bundle}"
        );
    }
    db.execute("INSERT INTO sources(id,bundle_hash,source_id,title,system_id,edition,language,pdf_hash,raw_json,prepared_json) VALUES(3,?1,'synthetic','Synthetic','mork-borg','fixture','en',?2,x'7b7d',x'7b7d')", params!["d".repeat(64), "a".repeat(64)]).unwrap();
    assert!(
        db.execute("UPDATE OR REPLACE sources SET id=1 WHERE id=3", [])
            .is_err()
    );
    assert!(
        db.execute("UPDATE sources SET id=4 WHERE id=1", [])
            .is_err()
    );
    insert_region(&db, 1, 10, 150).unwrap();
}

#[test]
fn unfinalized_bounds_populate_when_prepared_metadata_becomes_valid() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("source.sqlite3");
    let _store = Store::open(&path).unwrap();
    let db = Connection::open(path).unwrap();
    insert_unfinalized_source(&db, 1, &json!({}));
    insert_unfinalized_page(&db, 10, 1, 1);
    assert!(insert_region(&db, 1, 10, 20).is_err());
    let prepared =
        serde_json::to_vec(&json!({"pages":[{"pdf_page":1,"width_points":40,"height_points":50}]}))
            .unwrap();
    db.execute("UPDATE sources SET prepared_json=?1 WHERE id=1", [prepared])
        .unwrap();
    insert_region(&db, 1, 10, 30).unwrap();
    assert!(insert_region(&db, 1, 10, 60).is_err());
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
    c.db.execute("INSERT INTO sources(bundle_hash,source_id,title,system_id,edition,language,pdf_hash,raw_json,prepared_json) VALUES('second','s','t','mork-borg','e','en','other',x'7b7d',x'7b7d')",[]).unwrap();
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
    c.db.execute("INSERT INTO source_units(page_id,block_id,kind,polygon,raw_html,raw_text,origin) SELECT 1,'owner:1','Text',polygon,'<p>Advice</p>','Advice','owner' FROM source_units WHERE id=1",[]).unwrap();
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
        edited: false,
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
            index: 1,
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

#[tokio::test]
async fn competing_open_corrections_cannot_share_a_decision() {
    let c = context().await;
    let a = c
        .store
        .insert_source_suggestion(advice(&c, "correction", unit_target()))
        .await
        .unwrap();
    let b = c
        .store
        .insert_source_suggestion(advice(&c, "correction", unit_target()))
        .await
        .unwrap();
    let event = correction(&c).await;
    c.store
        .record_source_suggestion_application(application(a, event))
        .await
        .unwrap();
    assert!(
        c.store
            .record_source_suggestion_application(application(b, event))
            .await
            .is_err()
    );
    assert_eq!(count(&c.db, "source_suggestion_applications"), 1);
    assert_eq!(count(&c.db, "source_suggestion_dismissals"), 0);
}

#[tokio::test]
async fn correction_application_requires_truthful_edited_flag() {
    for (text, edited) in [("Advice", true), ("Different", false)] {
        let c = context().await;
        let mut suggestion = advice(&c, "correction", unit_target());
        suggestion.suggested_text = Some(text.into());
        let id = c.store.insert_source_suggestion(suggestion).await.unwrap();
        let event = correction(&c).await;
        let mut app = application(id, event);
        app.edited = edited;
        assert!(
            c.store
                .record_source_suggestion_application(app)
                .await
                .is_err(),
            "text={text}, edited={edited}"
        );
    }
}

#[tokio::test]
async fn v1_empty_unit_text_is_a_note() {
    let c = context().await;
    for text in ["", " \t\n", "\u{2003}"] {
        let summary = import(
            &c,
            &report(&c, 1, vec![issue(json!("/page/0/Text/1"), json!(text))]),
        )
        .await
        .unwrap();
        assert_eq!(summary.counts["note"], 1);
        assert_eq!(summary.counts["correction"], 0);
    }
}

#[tokio::test]
async fn contradictory_v1_bundle_binding_cannot_fall_back_to_extraction() {
    let c = context().await;
    let mut value = report(&c, 1, vec![]);
    value["bundle_sha256"] = json!("0".repeat(64));
    value["artifact_sha256"] =
        json!({"source.json":fixture::hash(&fs::read(c.bundle.join("source.json")).unwrap())});
    assert!(import(&c, &value).await.is_err());
}

#[tokio::test]
async fn report_timestamp_is_bounded_and_control_free() {
    let c = context().await;
    for timestamp in [
        "x".repeat(65),
        "é".repeat(33),
        "now\n".into(),
        "now\0".into(),
    ] {
        let mut value = report(&c, 1, vec![]);
        value["created_at"] = json!(timestamp);
        assert!(
            import(&c, &value).await.is_err(),
            "timestamp must be refused"
        );
    }
}

#[tokio::test]
async fn sql_report_timestamp_is_bounded_and_nul_free() {
    let c = context().await;
    for timestamp in ["x".repeat(65), "é".repeat(33), "now\0".into()] {
        assert!(c.db.execute("INSERT INTO source_suggestion_reports(source_id,report_sha256,origin,schema_version,binding,created_at,correction,reject,merge,missing_text,note,failed_pages) VALUES(1,?1,'tool',1,'bundle',?2,0,0,0,0,0,0)", params!["a".repeat(64), timestamp]).is_err());
    }
}

#[tokio::test]
async fn report_tool_name_is_control_free() {
    let c = context().await;
    for tool in ["fixture\n", "fixture\t", "fixture\u{7f}"] {
        let mut value = report(&c, 1, vec![]);
        value["tool"] = json!(tool);
        assert!(import(&c, &value).await.is_err());
    }
}

#[tokio::test]
async fn blank_dismissal_reason_is_null() {
    let c = context().await;
    for reason in ["", " \t\n", "\u{2003}"] {
        let id = c
            .store
            .insert_source_suggestion(advice(&c, "note", unit_target()))
            .await
            .unwrap();
        c.store
            .dismiss_source_suggestion(id, "owner".into(), Some(reason.into()))
            .await
            .unwrap();
        let reason: Option<String> =
            c.db.query_row(
                "SELECT reason FROM source_suggestion_dismissals WHERE suggestion_id=?1",
                [id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(reason, None);
    }
}

#[tokio::test]
async fn cli_unknown_source_names_source_and_prints_no_summary() {
    let c = context().await;
    fs::write(
        &c.report,
        serde_json::to_vec(&report(&c, 1, vec![])).unwrap(),
    )
    .unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_mimik"))
        .arg("--database")
        .arg(c.dir.path().join("reports.sqlite3"))
        .args(["import-suggestions", "--source", "999", "--report"])
        .arg(&c.report)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(output.stdout, [] as [u8; 0]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Source") || stderr.contains("source"),
        "{stderr}"
    );
    assert!(!stderr.contains("Character"));
}

#[tokio::test]
async fn report_refusals_are_specific_and_issue_numbers_are_one_based() {
    let c = context().await;
    let base = report(&c, 1, vec![]);
    for (field, expected) in [
        (
            "source_sha256",
            "Suggestion report PDF hash does not match source.",
        ),
        (
            "bundle_sha256",
            "Suggestion report binding is missing or does not match source.",
        ),
    ] {
        let mut bad = base.clone();
        bad[field] = json!("0".repeat(64));
        assert_eq!(import(&c, &bad).await.unwrap_err().to_string(), expected);
    }
    let mut bad = base.clone();
    bad["pages"][0]["pdf_page"] = json!(99);
    assert_eq!(
        import(&c, &bad).await.unwrap_err().to_string(),
        "Suggestion report page is outside source."
    );
    let mut bad = base.clone();
    bad["pages"]
        .as_array_mut()
        .unwrap()
        .push(base["pages"][0].clone());
    assert_eq!(
        import(&c, &bad).await.unwrap_err().to_string(),
        "Suggestion report contains a duplicate page."
    );
    let bad = report(&c, 1, vec![issue(Value::Null, json!("x".repeat(20001)))]);
    assert!(
        import(&c, &bad)
            .await
            .unwrap_err()
            .to_string()
            .contains("issue 1:")
    );
}

#[tokio::test]
async fn report_parse_error_names_location_without_echoing_content() {
    let c = context().await;
    fs::write(&c.report, b"{\n\"private-content\": true}").unwrap();
    let error = c
        .store
        .import_suggestion_report(1, &c.report)
        .await
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("schema/parse error")
            && error.contains("line 2")
            && error.contains("column"),
        "{error}"
    );
    assert!(!error.contains("private-content"));
}

async fn merge_case(
    polygon: &str,
    raw_text: &str,
    suggested: &str,
) -> (Context, i64, SuggestionApplication) {
    use mimik::sources::UnitDecision;
    let c = context().await;
    let targets = (1..=2)
        .map(|unit| SuggestionUnit { unit, revision: 1 })
        .collect();
    let mut suggestion = advice(&c, "merge", SuggestionTarget::Units { targets });
    suggestion.suggested_text = Some(suggested.into());
    // Both alternatives exist while all target units are still at revision 1.
    let competitor = c
        .store
        .insert_source_suggestion(suggestion.clone())
        .await
        .unwrap();
    let id = c.store.insert_source_suggestion(suggestion).await.unwrap();
    c.db.execute("INSERT INTO source_units(page_id,block_id,kind,polygon,raw_html,raw_text,origin) VALUES(1,'owner:1','Text',?1,'<p>Advice</p>',?2,'owner')", params![polygon, raw_text]).unwrap();
    let decisions = (1..=3)
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
    let operation =
        c.db.query_row(
            "SELECT operation_id FROM source_review_events WHERE id=?1",
            [event],
            |r| r.get(0),
        )
        .unwrap();
    let app = SuggestionApplication {
        operation_id: Some(operation),
        owner_unit_id: Some(3),
        ..application(id, event)
    };
    (c, competitor, app)
}

#[tokio::test]
async fn merge_owner_must_match_target_bounding_rectangle() {
    let (c, _, app) = merge_case("[[1,1],[100,1],[100,40],[1,40]]", "Advice", "Advice").await;
    assert!(
        c.store
            .record_source_suggestion_application(app)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn merge_owner_text_must_equal_its_correction() {
    let c = context().await;
    // Use the exact target bounds so only the text check can reject this application.
    let polygon: String =
        c.db.query_row("SELECT polygon FROM source_units WHERE id=1", [], |r| {
            r.get(0)
        })
        .unwrap();
    let (c, _, app) = merge_case(&polygon, "Unrelated", "Advice").await;
    assert!(
        c.store
            .record_source_suggestion_application(app)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn merge_application_requires_truthful_edited_flag() {
    let c = context().await;
    let polygon: String =
        c.db.query_row("SELECT polygon FROM source_units WHERE id=1", [], |r| {
            r.get(0)
        })
        .unwrap();
    for suggested in ["Advice", "Different"] {
        let (c, _, mut app) = merge_case(&polygon, "Advice", suggested).await;
        app.edited = suggested == "Advice";
        assert!(
            c.store
                .record_source_suggestion_application(app)
                .await
                .is_err()
        );
    }
}

#[tokio::test]
async fn competing_open_merges_cannot_share_event_or_owner() {
    let c = context().await;
    let polygon: String =
        c.db.query_row("SELECT polygon FROM source_units WHERE id=1", [], |r| {
            r.get(0)
        })
        .unwrap();
    let (c, b, app) = merge_case(&polygon, "Advice", "Advice").await;
    c.store
        .record_source_suggestion_application(app.clone())
        .await
        .unwrap();
    assert!(
        c.store
            .record_source_suggestion_application(SuggestionApplication {
                suggestion_id: b,
                ..app.clone()
            })
            .await
            .is_err()
    );
    // A distinct latest event isolates owner uniqueness from event uniqueness.
    let event = duplicate_latest_event(&c);
    assert!(
        c.store
            .record_source_suggestion_application(SuggestionApplication {
                suggestion_id: b,
                event_id: event,
                ..app
            })
            .await
            .is_err()
    );
    assert_eq!(count(&c.db, "source_suggestion_applications"), 1);
}

fn duplicate_latest_event(c: &Context) -> i64 {
    c.db.execute("INSERT INTO source_review_events(page_id,unit_id,previous_revision,state,correction,actor,rationale,operation_id) SELECT page_id,unit_id,previous_revision,state,correction,actor,rationale,operation_id FROM source_review_events ORDER BY id DESC LIMIT 1", []).unwrap();
    c.db.last_insert_rowid()
}

#[tokio::test]
async fn missing_text_requires_truthful_edited_flag() {
    for suggested in ["Advice", "Different"] {
        let c = context().await;
        let polygon = [[1.0, 1.0], [20.0, 1.0], [20.0, 20.0], [1.0, 20.0]];
        let mut suggestion = advice(
            &c,
            "missing_text",
            SuggestionTarget::Region { page: 1, polygon },
        );
        suggestion.suggested_text = Some(suggested.into());
        let id = c.store.insert_source_suggestion(suggestion).await.unwrap();
        let event = c
            .store
            .add_owner_unit(
                1,
                OwnerUnit {
                    revision: 1,
                    polygon,
                    text: "Advice".into(),
                    actor: "owner".into(),
                    rationale: "Image".into(),
                },
            )
            .await
            .unwrap();
        let app = SuggestionApplication {
            owner_unit_id: Some(3),
            edited: suggested == "Advice",
            ..application(id, event)
        };
        assert!(
            c.store
                .record_source_suggestion_application(app)
                .await
                .is_err()
        );
    }
}

#[tokio::test]
async fn competing_open_missing_text_cannot_share_owner_or_event() {
    let c = context().await;
    let polygon = [[1.0, 1.0], [20.0, 1.0], [20.0, 20.0], [1.0, 20.0]];
    let a = c
        .store
        .insert_source_suggestion(advice(
            &c,
            "missing_text",
            SuggestionTarget::Region { page: 1, polygon },
        ))
        .await
        .unwrap();
    let b = c
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
                rationale: "Image".into(),
            },
        )
        .await
        .unwrap();
    let app = SuggestionApplication {
        owner_unit_id: Some(3),
        ..application(a, event)
    };
    c.store
        .record_source_suggestion_application(app.clone())
        .await
        .unwrap();
    assert!(
        c.store
            .record_source_suggestion_application(SuggestionApplication {
                suggestion_id: b,
                ..app.clone()
            })
            .await
            .is_err()
    );
    let event = duplicate_latest_event(&c);
    assert!(
        c.store
            .record_source_suggestion_application(SuggestionApplication {
                suggestion_id: b,
                event_id: event,
                ..app
            })
            .await
            .is_err()
    );
    assert_eq!(count(&c.db, "source_suggestion_applications"), 1);
}

#[tokio::test]
async fn rejection_application_cannot_be_edited() {
    let c = context().await;
    let id = c
        .store
        .insert_source_suggestion(advice(&c, "reject", unit_target()))
        .await
        .unwrap();
    c.store
        .review_source_unit(
            1,
            1,
            UnitReview {
                revision: 1,
                state: "rejected".into(),
                correction: String::new(),
                actor: "owner".into(),
                rationale: "Image".into(),
            },
        )
        .await
        .unwrap();
    let event =
        c.db.query_row("SELECT max(id) FROM source_review_events", [], |r| r.get(0))
            .unwrap();
    let app = SuggestionApplication {
        edited: true,
        ..application(id, event)
    };
    assert!(
        c.store
            .record_source_suggestion_application(app)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn worst_case_five_thousand_regions_and_merges_on_large_prepared_page() {
    for action in ["missing_text", "merge"] {
        let c = context_with_units(false, None, 300).await;
        let prepared_bytes: i64 =
            c.db.query_row("SELECT length(prepared_json) FROM sources", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert!(prepared_bytes > 300_000);
        let mut i = issue(Value::Null, json!("Advice"));
        i["action"] = json!(action);
        if action == "merge" {
            i["merge_block_ids"] = json!(["/page/0/Text/1", "/page/0/Text/2"]);
        } else {
            i["region"] = json!([[1, 1], [20, 1], [20, 20], [1, 20]]);
        }
        let value = report(&c, 2, vec![i; 5000]);
        let start = std::time::Instant::now();
        let summary = import(&c, &value).await.unwrap();
        eprintln!(
            "5,000 {action} issues; 300 units; prepared_json {prepared_bytes} bytes: {:?}",
            start.elapsed()
        );
        assert_eq!(summary.counts[action], 5000);
        assert_eq!(count(&c.db, "source_suggestions"), 5000);
    }
}

#[tokio::test]
async fn import_four_hundred_pages_near_prepared_json_cap() {
    let dir = tempfile::tempdir().unwrap();
    // The public importer caps bundles at 64 pages. Measure the database write path
    // in isolation for this requested 400-page stress case, with every guard active.
    let prepared = json!({"pages":(1..=400).map(|page| json!({"pdf_page":page,"width_points":200,"height_points":300,"units":[{"html":"x".repeat(20_000)}]})).collect::<Vec<_>>()});
    let bytes = serde_json::to_vec(&prepared).unwrap();
    assert!((8_000_000..8_388_608).contains(&bytes.len()));
    let path = dir.path().join("source.sqlite3");
    let _store = Store::open(&path).unwrap();
    let mut db = Connection::open(path).unwrap();
    let start = std::time::Instant::now();
    let tx = db.transaction().unwrap();
    insert_unfinalized_source(&tx, 1, &prepared);
    for page in 1..=400 {
        insert_unfinalized_page(&tx, page, 1, page);
    }
    tx.commit().unwrap();
    eprintln!(
        "400-page source database import; prepared_json {} bytes: {:?}",
        bytes.len(),
        start.elapsed()
    );
    assert_eq!(count(&db, "source_suggestion_page_bounds"), 400);
}

#[tokio::test]
async fn cached_region_bounds_cannot_diverge_from_unfinalized_source_or_page() {
    let c = context().await;
    c.db.execute_batch("DROP TRIGGER source_finalize_delete; DELETE FROM source_finalizations;")
        .unwrap();
    assert!(
        c.db.execute("UPDATE sources SET prepared_json=x'7b7d' WHERE id=1", [])
            .is_err()
    );
    c.db.execute("UPDATE source_pages SET pdf_page=99 WHERE id=1", [])
        .unwrap();
    assert!(insert_region(&c.db, 1, 1, 20).is_err());
    c.db.execute("UPDATE source_pages SET pdf_page=1 WHERE id=1", [])
        .unwrap();
    insert_region(&c.db, 1, 1, 20).unwrap();
    assert!(
        c.db.execute(
            "UPDATE source_suggestion_page_bounds SET width=4000 WHERE source_id=1 AND pdf_page=1",
            []
        )
        .is_err()
    );
    assert!(
        c.db.execute(
            "INSERT OR REPLACE INTO source_suggestion_page_bounds VALUES(1,1,4000,4000)",
            []
        )
        .is_err()
    );
    assert!(
        c.db.execute(
            "DELETE FROM source_suggestion_page_bounds WHERE source_id=1 AND pdf_page=1",
            []
        )
        .is_err()
    );
}

#[tokio::test]
async fn oversized_source_extraction_is_not_reported_as_missing_source() {
    let c = context().await;
    c.db.execute_batch("DROP TRIGGER sources_finalized_update; UPDATE sources SET raw_json=zeroblob(33554433) WHERE id=1;").unwrap();
    assert!(matches!(
        import(&c, &report(&c, 1, vec![])).await,
        Err(AppError::Invalid(
            "Source extraction exceeds its byte limit."
        ))
    ));
}

#[tokio::test]
async fn report_target_database_errors_are_not_invalid_issues() {
    let c = context().await;
    c.db.execute_batch("PRAGMA foreign_keys=OFF; DROP TABLE source_units;")
        .unwrap();
    let error = import(
        &c,
        &report(&c, 1, vec![issue(json!("/page/0/Text/1"), json!("Advice"))]),
    )
    .await
    .unwrap_err();
    assert!(matches!(error, AppError::Database(_)), "{error:?}");
}
