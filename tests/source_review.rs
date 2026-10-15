use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use mimik::{
    AppError, Store, app,
    sources::{PageReview, UnitReview},
};
use serde_json::json;
use std::{
    fs,
    path::{Path, PathBuf},
};
use tower::ServiceExt;

#[path = "support/source_fixture.rs"]
pub mod support;
use support::hash;

fn fixture(root: &Path) -> (PathBuf, PathBuf) {
    support::fixture(root).unwrap()
}

fn review(revision: i64, state: &str, correction: &str) -> UnitReview {
    UnitReview {
        revision,
        state: state.into(),
        correction: correction.into(),
        actor: "fixture-reviewer".into(),
        rationale: "Compared synthetic source".into(),
    }
}

fn completion(revision: i64) -> PageReview {
    PageReview {
        revision,
        printed_label: "1".into(),
        actor: "fixture-reviewer".into(),
        rationale: "Checked completeness".into(),
    }
}

fn replace_html(bundle: &Path, html: &str, kind: &str) {
    let raw_path = bundle.join("source.json");
    let mut raw: serde_json::Value = serde_json::from_slice(&fs::read(&raw_path).unwrap()).unwrap();
    raw["children"][0]["children"][0]["html"] = json!(html);
    raw["children"][0]["children"][0]["block_type"] = json!(kind);
    let bytes = serde_json::to_vec(&raw).unwrap();
    fs::write(raw_path, &bytes).unwrap();
    let path = bundle.join("bundle.json");
    let mut prepared: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    prepared["extraction_sha256"] = json!(hash(&bytes));
    prepared["pages"][0]["units"][0]["html"] = json!(html);
    prepared["pages"][0]["units"][0]["kind"] = json!(kind);
    fs::write(path, serde_json::to_vec(&prepared).unwrap()).unwrap();
}

#[tokio::test]
async fn table_spans_survive_sanitizing_but_active_attributes_do_not() {
    let dir = tempfile::tempdir().unwrap();
    let (bundle, pdf) = fixture(dir.path());
    replace_html(
        &bundle,
        "<table><tr><th colspan='2' onclick='evil()'>Heading</th></tr><tr><td rowspan='2'>Group</td><td>Value</td></tr><tr><td>Other</td></tr></table><img src='https://outside.example'>",
        "Table",
    );
    let store = Store::open(&dir.path().join("test.sqlite3")).unwrap();
    store.import_source(&bundle, &pdf).await.unwrap();
    let page_id = store.sources().await.unwrap()[0].pages[0].id;
    let page = store.source_page(page_id).await.unwrap();
    assert!(page.units[0].sanitized_html.contains("colspan=\"2\""));
    assert!(page.units[0].sanitized_html.contains("rowspan=\"2\""));
    assert!(!page.units[0].sanitized_html.contains("onclick"));
    assert!(!page.units[0].sanitized_html.contains("https://"));
}

#[tokio::test]
async fn stale_completeness_draft_keeps_its_token_label_and_rationale() {
    let dir = tempfile::tempdir().unwrap();
    let (bundle, pdf) = fixture(dir.path());
    let store = Store::open(&dir.path().join("test.sqlite3")).unwrap();
    store.import_source(&bundle, &pdf).await.unwrap();
    let page_id = store.sources().await.unwrap()[0].pages[0].id;
    let page = store.source_page(page_id).await.unwrap();
    store
        .review_source_unit(page_id, page.units[0].id, review(1, "accepted", ""))
        .await
        .unwrap();
    let stale_revision = store.source_page(page_id).await.unwrap().revision;
    store
        .review_source_unit(
            page_id,
            page.units[0].id,
            review(2, "corrected", "new correction"),
        )
        .await
        .unwrap();
    let router = app(store.clone());
    let body = format!(
        "revision={stale_revision}&printed_label=draft-label&actor=draft-reviewer&rationale=draft-rationale"
    );
    let response = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/sources/pages/{page_id}"))
                .header("host", "localhost:3000")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let html = String::from_utf8(
        to_bytes(response.into_body(), 200_000)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(html.contains(&format!("name=\"revision\" value=\"{stale_revision}\"")));
    assert!(html.contains("value=\"draft-label\""));
    assert!(html.contains("draft-rationale"));
    assert!(!store.source_page(page_id).await.unwrap().complete);
}

#[tokio::test]
async fn provenance_and_true_geometry_or_png_failures_are_rejected() {
    for mutation in ["metadata", "geometry", "png", "span"] {
        let dir = tempfile::tempdir().unwrap();
        let (bundle, pdf) = fixture(dir.path());
        let prepared_path = bundle.join("bundle.json");
        let raw_path = bundle.join("source.json");
        if mutation == "span" {
            replace_html(
                &bundle,
                "<table><tr><td colspan='999999'>Unsupported</td></tr></table>",
                "Table",
            );
        }
        let mut prepared: serde_json::Value =
            serde_json::from_slice(&fs::read(&prepared_path).unwrap()).unwrap();
        match mutation {
            "metadata" => prepared["extractor"] = json!({}),
            "geometry" => {
                let mut raw: serde_json::Value =
                    serde_json::from_slice(&fs::read(&raw_path).unwrap()).unwrap();
                raw["children"][0]["children"][0]["polygon"][0][0] = json!(-5);
                let bytes = serde_json::to_vec(&raw).unwrap();
                fs::write(&raw_path, &bytes).unwrap();
                prepared["extraction_sha256"] = json!(hash(&bytes));
                prepared["pages"][0]["units"][0]["polygon"][0][0] = json!(-5);
            }
            "png" => {
                fs::write(bundle.join("page-1.png"), b"invalid PNG").unwrap();
                prepared["pages"][0]["image_sha256"] = json!(hash(b"invalid PNG"));
            }
            "span" => {}
            _ => unreachable!(),
        }
        fs::write(prepared_path, serde_json::to_vec(&prepared).unwrap()).unwrap();
        let store = Store::open(&dir.path().join("test.sqlite3")).unwrap();
        assert!(
            store.import_source(&bundle, &pdf).await.is_err(),
            "{mutation}"
        );
    }
}

#[tokio::test]
async fn zero_sized_original_pdf_cannot_hide_inside_rounding_tolerance() {
    let dir = tempfile::tempdir().unwrap();
    let (bundle, pdf) = fixture(dir.path());
    let mut document = lopdf::Document::load(&pdf).unwrap();
    let id = *document.get_pages().values().next().unwrap();
    document
        .get_dictionary_mut(id)
        .unwrap()
        .set("MediaBox", vec![0.into(), 0.into(), 0.into(), 0.into()]);
    document.save(&pdf).unwrap();
    let small = json!([[0, 0], [1, 0], [1, 1], [0, 1]]);
    let raw_path = bundle.join("source.json");
    let mut raw: serde_json::Value = serde_json::from_slice(&fs::read(&raw_path).unwrap()).unwrap();
    raw["children"][0]["polygon"] = small.clone();
    raw["children"][0]["children"][0]["polygon"] = small.clone();
    let bytes = serde_json::to_vec(&raw).unwrap();
    fs::write(&raw_path, &bytes).unwrap();
    let mut encoder = png::Encoder::new(fs::File::create(bundle.join("page-1.png")).unwrap(), 1, 1);
    encoder.set_color(png::ColorType::Rgb);
    encoder
        .write_header()
        .unwrap()
        .write_image_data(&[255, 255, 255])
        .unwrap();
    let prepared_path = bundle.join("bundle.json");
    let mut prepared: serde_json::Value =
        serde_json::from_slice(&fs::read(&prepared_path).unwrap()).unwrap();
    prepared["source_sha256"] = json!(hash(&fs::read(&pdf).unwrap()));
    prepared["extraction_sha256"] = json!(hash(&bytes));
    prepared["pages"][0]["width_points"] = json!(1);
    prepared["pages"][0]["height_points"] = json!(1);
    prepared["pages"][0]["image_sha256"] =
        json!(hash(&fs::read(bundle.join("page-1.png")).unwrap()));
    prepared["pages"][0]["units"][0]["polygon"] = small;
    fs::write(prepared_path, serde_json::to_vec(&prepared).unwrap()).unwrap();
    let store = Store::open(&dir.path().join("test.sqlite3")).unwrap();
    assert!(store.import_source(&bundle, &pdf).await.is_err());
}

#[tokio::test]
async fn partial_original_book_coverage_is_visible_after_selected_page_is_complete() {
    let dir = tempfile::tempdir().unwrap();
    let (bundle, pdf) = fixture(dir.path());
    let mut document = lopdf::Document::load(&pdf).unwrap();
    let first = *document.get_pages().values().next().unwrap();
    let parent = document
        .get_dictionary(first)
        .unwrap()
        .get(b"Parent")
        .unwrap()
        .as_reference()
        .unwrap();
    let second = document.add_object(lopdf::dictionary! {"Type"=>"Page","Parent"=>parent,"MediaBox"=>vec![0.into(),0.into(),200.into(),300.into()]});
    let pages = document.get_dictionary_mut(parent).unwrap();
    pages.set(
        "Kids",
        vec![
            lopdf::Object::Reference(first),
            lopdf::Object::Reference(second),
        ],
    );
    pages.set("Count", 2);
    document.save(&pdf).unwrap();
    let path = bundle.join("bundle.json");
    let mut prepared: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    prepared["source_sha256"] = json!(hash(&fs::read(&pdf).unwrap()));
    fs::write(path, serde_json::to_vec(&prepared).unwrap()).unwrap();
    let store = Store::open(&dir.path().join("test.sqlite3")).unwrap();
    store.import_source(&bundle, &pdf).await.unwrap();
    let page_id = store.sources().await.unwrap()[0].pages[0].id;
    let page = store.source_page(page_id).await.unwrap();
    store
        .review_source_unit(page_id, page.units[0].id, review(1, "accepted", ""))
        .await
        .unwrap();
    let page = store.source_page(page_id).await.unwrap();
    store
        .complete_source_page(page_id, completion(page.revision))
        .await
        .unwrap();
    let response = app(store)
        .oneshot(
            Request::builder()
                .uri("/sources/search?system=mork-borg&edition=fixture-edition&query=Strength")
                .header("host", "localhost:3000")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let html = String::from_utf8(
        to_bytes(response.into_body(), 100_000)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(html.contains("Partial or unknown original-PDF coverage"));
}

#[tokio::test]
async fn legacy_unknown_page_totals_backfill_without_changing_review_history() {
    let dir = tempfile::tempdir().unwrap();
    let (bundle, pdf) = fixture(dir.path());
    let database = dir.path().join("legacy.sqlite3");
    let db = rusqlite::Connection::open(&database).unwrap();
    db.execute_batch(include_str!("../migrations/001_characters.sql"))
        .unwrap();
    db.execute_batch(include_str!("../migrations/002_mork_borg.sql"))
        .unwrap();
    db.execute_batch(include_str!("../migrations/003_sources.sql"))
        .unwrap();
    let prepared = fs::read(bundle.join("bundle.json")).unwrap();
    let pdf_bytes = fs::read(&pdf).unwrap();
    db.execute("INSERT INTO sources (id,bundle_hash,source_id,title,system_id,edition,language,pdf_hash,pdf,raw_json,prepared_json) VALUES (1,?1,'synthetic-core','Synthetic source','mork-borg','fixture-edition','en',?2,?3,?4,?5)", rusqlite::params![hash(&prepared),hash(&pdf_bytes),pdf_bytes,fs::read(bundle.join("source.json")).unwrap(),prepared]).unwrap();
    db.execute("INSERT INTO source_pages (id,source_id,pdf_page,png,complete,revision,printed_label) VALUES (1,1,1,?1,1,5,'1')", [fs::read(bundle.join("page-1.png")).unwrap()]).unwrap();
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(bundle.join("bundle.json")).unwrap()).unwrap();
    let unit = &manifest["pages"][0]["units"][0];
    let polygon: Vec<[f64; 2]> = serde_json::from_value(unit["polygon"].clone()).unwrap();
    let text = html2text::from_read("<p>Strength capacity +8</p>".as_bytes(), 120).unwrap();
    db.execute("INSERT INTO source_units (id,page_id,block_id,kind,polygon,raw_html,raw_text,state,revision) VALUES (1,1,'/page/0/Text/1','Text',?1,?2,?3,'accepted',7)",rusqlite::params![serde_json::to_string(&polygon).unwrap(),unit["html"].as_str().unwrap(),text]).unwrap();
    db.execute_batch("INSERT INTO source_review_events (page_id,unit_id,actor,rationale,state,correction,previous_revision) VALUES (1,1,'legacy reviewer','legacy rationale','accepted','',6);").unwrap();
    drop(db);
    let store = Store::open(&database).unwrap();
    assert_eq!(store.sources().await.unwrap()[0].total_pages, 0);
    assert_eq!(
        store
            .search_evidence(
                "mork-borg".into(),
                "fixture-edition".into(),
                "Legacy".into()
            )
            .await
            .unwrap()
            .partial_runs,
        1
    );
    assert_eq!(store.import_source(&bundle, &pdf).await.unwrap(), 1);
    let page = store.source_page(1).await.unwrap();
    assert_eq!(page.revision, 5);
    assert_eq!(page.units[0].revision, 7);
    assert_eq!(page.units[0].state, "accepted");
    assert_eq!(page.events.len(), 1);
    assert!(page.complete);
    drop(store);
    let store = Store::open(&database).unwrap();
    assert_eq!(store.sources().await.unwrap()[0].total_pages, 1);
    assert_eq!(
        store
            .search_evidence(
                "mork-borg".into(),
                "fixture-edition".into(),
                "Legacy".into()
            )
            .await
            .unwrap()
            .partial_runs,
        0
    );
}

#[tokio::test]
async fn imports_remain_unreviewed_and_search_requires_complete_exact_edition() {
    let dir = tempfile::tempdir().unwrap();
    let (bundle, pdf) = fixture(dir.path());
    let database = dir.path().join("test.sqlite3");
    let store = Store::open(&database).unwrap();
    let id = store.import_source(&bundle, &pdf).await.unwrap();
    assert_eq!(store.import_source(&bundle, &pdf).await.unwrap(), id);
    let sources = store.sources().await.unwrap();
    let page_id = sources[0].pages[0].id;
    let page = store.source_page(page_id).await.unwrap();
    assert!(!page.complete);
    assert_eq!(page.units[0].state, "unreviewed");
    assert!(!page.units[0].sanitized_html.contains("<script"));
    assert!(!page.units[0].sanitized_html.contains("https://"));
    assert!(
        store
            .search_evidence(
                "mork-borg".into(),
                "fixture-edition".into(),
                "Strength".into()
            )
            .await
            .unwrap()
            .matches
            .is_empty()
    );
    assert!(
        store
            .complete_source_page(page_id, completion(page.revision))
            .await
            .is_err()
    );
    store
        .review_source_unit(page_id, page.units[0].id, review(1, "accepted", ""))
        .await
        .unwrap();
    assert!(matches!(
        store
            .review_source_unit(page_id, page.units[0].id, review(1, "rejected", ""))
            .await,
        Err(AppError::Conflict)
    ));
    let current = store.source_page(page_id).await.unwrap();
    store
        .complete_source_page(page_id, completion(current.revision))
        .await
        .unwrap();
    let result = store
        .search_evidence(
            "mork-borg".into(),
            "fixture-edition".into(),
            "Strength".into(),
        )
        .await
        .unwrap();
    assert_eq!(result.matches.len(), 1);
    assert_eq!(result.incomplete_pages, 0);
    assert_eq!(
        store
            .search_evidence(
                "mork-borg".into(),
                "wrong-edition".into(),
                "Strength".into()
            )
            .await
            .unwrap()
            .source_count,
        0
    );
}

#[tokio::test]
async fn corrections_invalidate_completeness_and_persist_with_history() {
    let dir = tempfile::tempdir().unwrap();
    let (bundle, pdf) = fixture(dir.path());
    let database = dir.path().join("test.sqlite3");
    let store = Store::open(&database).unwrap();
    store.import_source(&bundle, &pdf).await.unwrap();
    let page_id = store.sources().await.unwrap()[0].pages[0].id;
    let page = store.source_page(page_id).await.unwrap();
    store
        .review_source_unit(page_id, page.units[0].id, review(1, "accepted", ""))
        .await
        .unwrap();
    let current = store.source_page(page_id).await.unwrap();
    store
        .complete_source_page(page_id, completion(current.revision))
        .await
        .unwrap();
    store
        .review_source_unit(
            page_id,
            page.units[0].id,
            review(2, "corrected", "Corrected capacity reference"),
        )
        .await
        .unwrap();
    assert!(
        store
            .search_evidence(
                "mork-borg".into(),
                "fixture-edition".into(),
                "Corrected".into()
            )
            .await
            .unwrap()
            .matches
            .is_empty()
    );
    let current = store.source_page(page_id).await.unwrap();
    assert_eq!(current.events.len(), 3);
    store
        .complete_source_page(page_id, completion(current.revision))
        .await
        .unwrap();
    drop(store);
    let store = Store::open(&database).unwrap();
    assert_eq!(
        store
            .search_evidence(
                "mork-borg".into(),
                "fixture-edition".into(),
                "Corrected".into()
            )
            .await
            .unwrap()
            .matches
            .len(),
        1
    );
    // A former LIKE wildcard has no searchable word: rejected, never match-all.
    assert!(matches!(
        store
            .search_evidence("mork-borg".into(), "fixture-edition".into(), "%".into())
            .await,
        Err(AppError::Invalid(_))
    ));
}

#[tokio::test]
async fn mutated_geometry_content_images_and_source_are_rejected_without_registration() {
    for mutation in ["html", "polygon", "page", "image", "pdf"] {
        let dir = tempfile::tempdir().unwrap();
        let (bundle, pdf) = fixture(dir.path());
        let mut data: serde_json::Value =
            serde_json::from_slice(&fs::read(bundle.join("bundle.json")).unwrap()).unwrap();
        match mutation {
            "html" => data["pages"][0]["units"][0]["html"] = json!("forged"),
            "polygon" => data["pages"][0]["units"][0]["polygon"][0][0] = json!(-1),
            "page" => data["pages"][0]["pdf_page"] = json!(2),
            "image" => fs::write(bundle.join("page-1.png"), b"wrong").unwrap(),
            "pdf" => fs::write(&pdf, b"different source").unwrap(),
            _ => unreachable!(),
        }
        fs::write(
            bundle.join("bundle.json"),
            serde_json::to_vec(&data).unwrap(),
        )
        .unwrap();
        let store = Store::open(&dir.path().join("test.sqlite3")).unwrap();
        assert!(
            store.import_source(&bundle, &pdf).await.is_err(),
            "{mutation}"
        );
        assert!(store.sources().await.unwrap().is_empty());
    }
}

#[cfg(unix)]
#[tokio::test]
async fn symlink_artifacts_are_not_followed() {
    let dir = tempfile::tempdir().unwrap();
    let (bundle, pdf) = fixture(dir.path());
    fs::rename(
        bundle.join("source.json"),
        dir.path().join("elsewhere.json"),
    )
    .unwrap();
    std::os::unix::fs::symlink(
        dir.path().join("elsewhere.json"),
        bundle.join("source.json"),
    )
    .unwrap();
    let store = Store::open(&dir.path().join("test.sqlite3")).unwrap();
    assert!(store.import_source(&bundle, &pdf).await.is_err());
}

#[tokio::test]
async fn source_pages_and_images_are_local_and_html_is_safe() {
    let dir = tempfile::tempdir().unwrap();
    let (bundle, pdf) = fixture(dir.path());
    let store = Store::open(&dir.path().join("test.sqlite3")).unwrap();
    store.import_source(&bundle, &pdf).await.unwrap();
    let page_id = store.sources().await.unwrap()[0].pages[0].id;
    let router = app(store);
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/sources/pages/{page_id}"))
                .header("host", "localhost:3000")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let html = String::from_utf8(
        to_bytes(response.into_body(), 200_000)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(!html.contains("<script>alert"));
    assert!(!html.contains("https://outside.example"));
    assert!(html.contains("excluded from lookup"));
    let response = router
        .oneshot(
            Request::builder()
                .uri(format!("/sources/pages/{page_id}/image"))
                .header("host", "localhost:3000")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.headers()["content-type"], "image/png");
    assert_eq!(response.headers()["cache-control"], "no-store");
}
