//! Merging selected units into one corrected owner `Text` unit (split/merge plan, slice 2).
use super::*;

/// Units 3 and 4 get boxes: 3 sits inside the union of units 1 and 2, 4 far from it.
const MERGE_UNITS: &str = "UPDATE source_units SET polygon='[[55,30],[58,30],[58,40],[55,40]]' WHERE id=3; UPDATE source_units SET polygon='[[150,150],[160,150],[160,160],[150,160]]' WHERE id=4; UPDATE source_units SET raw_text='First line' WHERE id=1; UPDATE source_units SET raw_text='Second line' WHERE id=2;";
const MERGES: &str = "/admin/materials/pages/1/merges";

async fn merge_fixture(extra: &'static str) -> (tempfile::TempDir, Store, Vec<RequestAuth>) {
    let leaked: &'static str = Box::leak(format!("{MERGE_UNITS}{extra}").into_boxed_str());
    two_page_table_fixture(leaked).await
}

fn merge_fields(
    revision: &str,
    units: &[&str],
    text: &str,
    rationale: &str,
) -> Vec<(String, String)> {
    let mut fields = vec![
        ("csrf".to_owned(), "a".repeat(64)),
        ("revision".to_owned(), revision.to_owned()),
    ];
    fields.extend(units.iter().map(|u| ("unit".to_owned(), (*u).to_owned())));
    fields.push(("text".to_owned(), text.to_owned()));
    fields.push(("rationale".to_owned(), rationale.to_owned()));
    fields
}

async fn merge_post(
    app: &axum::Router,
    auth: &RequestAuth,
    fields: Vec<(String, String)>,
) -> axum::response::Response {
    document_at(app, auth, MERGES, fields).await
}

fn merge_unit_text(db: &rusqlite::Connection, unit: i64) -> (String, String, i64, String) {
    db.query_row(
        "SELECT kind,state,revision,correction FROM source_units WHERE id=?1",
        [unit],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )
    .unwrap()
}

/// The stored result of merging units 1 and 2 into `unit`.
async fn assert_merge_rows(store: &Store, unit: i64) {
    store
        .run(move |db| {
            // Revision 1 -> corrected is revision 2; the text is the Owner's, line breaks as LF.
            assert_eq!(
                merge_unit_text(db, unit),
                ("Text".into(), "corrected".into(), 2, "Merged\ntext".into())
            );
            for id in [1, 2] {
                assert_eq!(merge_unit_text(db, id).1, "rejected");
            }
            assert_eq!(merge_unit_text(db, 3).1, "unreviewed");
            // The union of BOX1 and BOX2.
            let polygon: String =
                db.query_row("SELECT polygon FROM source_units WHERE id=?1", [unit], |r| {
                    r.get(0)
                })?;
            let points: [[f64; 2]; 4] = serde_json::from_str(&polygon).unwrap();
            assert_eq!(
                points,
                [[10.0, 10.0], [120.0, 10.0], [120.0, 100.0], [10.0, 100.0]]
            );
            // One operation, one rationale, the owner's account, an audit row per event.
            let rows: Vec<(i64, String, String, String, String)> = db
                .prepare("SELECT unit_id,state,actor,rationale,operation_id FROM source_review_events WHERE operation_id IS NOT NULL ORDER BY id")?
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?
                .collect::<Result<_, _>>()?;
            assert_eq!(rows.len(), 3);
            assert_eq!(rows[0].0, unit);
            assert_eq!(rows[0].1, "corrected");
            assert_eq!((rows[1].0, rows[1].1.as_str()), (1, "rejected"));
            assert_eq!((rows[2].0, rows[2].1.as_str()), (2, "rejected"));
            assert!(rows.iter().all(|r| r.2 == "account:1" && r.3 == "Joined across a break" && r.4 == rows[0].4));
            let audited: i64 = db.query_row(
                "SELECT count(*) FROM source_web_events w JOIN source_review_events e ON e.id=w.event_id WHERE e.operation_id=?1",
                [&rows[0].4],
                |r| r.get(0),
            )?;
            assert_eq!(audited, 3);
            let (anchor, part, key): (i64, i64, String) = db.query_row(
                "SELECT anchor_unit_id,part,order_key FROM source_owner_replacements WHERE unit_id=?1",
                [unit],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )?;
            assert_eq!((anchor, part), (1, 1));
            assert_eq!(key, "00000000000000000001.0001");
            Ok(())
        })
        .await
        .unwrap();
}

/// Reading order, the confirmation notice and the history line after a merge of units 1 and 2.
async fn assert_merge_screens(
    app: &axum::Router,
    auth: &[RequestAuth],
    store: &Store,
    unit: i64,
    location: &str,
) {
    // Reading order: unit 1, the merged unit at its anchor, then unit 2.
    for (id, cursor) in [(1, 0), (unit, 1), (2, 2)] {
        assert_eq!(
            store
                .material_unit_cursor(auth[0].clone(), 1, id)
                .await
                .unwrap(),
            cursor
        );
    }
    // The notice link is true only for a merge, and the history names the units.
    let page = body(router_get(app, &auth[0], location).await).await;
    assert!(page.contains(&format!("Merged into unit {unit}")), "{page}");
    assert!(
        page.contains(&format!("Units 1, 2 merged into {unit}")),
        "{page}"
    );
    let forged = body(router_get(app, &auth[0], "/admin/materials/pages/1?merge=3").await).await;
    assert!(!forged.contains("Merged into unit"));
    let table_link = body(
        router_get(
            app,
            &auth[0],
            &format!("/admin/materials/pages/1?table={unit}"),
        )
        .await,
    )
    .await;
    assert!(!table_link.contains("Created table unit"));
}

#[tokio::test]
async fn material_merge_saves_one_corrected_unit_and_rejects_the_originals() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) = merge_fixture("").await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    // The form is prefilled in reading order, from the units as ticked in any order.
    let form = router_get(
        &app,
        &auth[0],
        "/admin/materials/pages/1/merges/new?unit=2&unit=1",
    )
    .await;
    assert_eq!(form.status(), StatusCode::OK);
    let html = body(form).await;
    assert_eq!(
        field_value(&html, "textarea[name=text]"),
        "First line\nSecond line"
    );
    let hidden =
        scraper::Selector::parse("form[data-form=merge] input[type=hidden][name=unit]").unwrap();
    let units: Vec<_> = scraper::Html::parse_document(&html)
        .select(&hidden)
        .map(|e| e.value().attr("value").unwrap().to_owned())
        .collect();
    assert_eq!(units, ["1:1", "2:1"]);
    let before = counts(&store).await;
    let saved = merge_post(
        &app,
        &auth[0],
        merge_fields(
            "1",
            &["1:1", "2:1"],
            "Merged\r\ntext",
            "Joined across a break",
        ),
    )
    .await;
    assert_eq!(saved.status(), StatusCode::SEE_OTHER);
    let location = saved.headers()["location"].to_str().unwrap().to_owned();
    let unit: i64 = store
        .run(|db| {
            Ok(db.query_row(
                "SELECT unit_id FROM source_owner_replacements WHERE kind='merge'",
                [],
                |r| r.get(0),
            )?)
        })
        .await
        .unwrap();
    // The cursor is computed after the commit: the merged unit follows its anchor, unit 1.
    assert_eq!(
        location,
        format!("/admin/materials/pages/1?units=1&merge={unit}&unit={unit}#unit-{unit}")
    );
    let after = counts(&store).await;
    assert_eq!(after.0, before.0 + 1);
    assert_eq!(
        after.1,
        before.1 + 3,
        "one corrected and two rejected events"
    );
    assert_merge_rows(&store, unit).await;
    assert_merge_screens(&app, &auth, &store, unit, &location).await;
}

#[tokio::test]
async fn material_merge_refuses_stale_revisions_and_keeps_the_draft() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) = merge_fixture("").await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    let before = counts(&store).await;
    // Stale page revision: unit revisions are current, so only the page revision can refuse it.
    let stale_page = merge_post(
        &app,
        &auth[0],
        merge_fields("0", &["1:1", "2:1"], "My draft <text>", "My rationale"),
    )
    .await;
    assert_eq!(stale_page.status(), StatusCode::CONFLICT);
    let html = body(stale_page).await;
    assert_eq!(field_value(&html, "textarea[name=text]"), "My draft <text>");
    assert_eq!(
        field_value(&html, "textarea[name=rationale]"),
        "My rationale"
    );
    assert!(html.contains("merge-error"));
    assert_eq!(counts(&store).await, before);
    // Stale unit revision: the page revision is current.
    let stale_unit = merge_post(
        &app,
        &auth[0],
        merge_fields("1", &["1:1", "2:9"], "My draft", "r"),
    )
    .await;
    assert_eq!(stale_unit.status(), StatusCode::CONFLICT);
    assert_eq!(counts(&store).await, before);
    // A unit decided after the form was opened makes its revision stale too.
    store
        .material_review_unit(
            auth[0].clone(),
            auth[0].csrf_fixture(),
            1,
            2,
            crate::sources::UnitReview {
                revision: 1,
                state: "accepted".into(),
                correction: String::new(),
                actor: String::new(),
                rationale: String::new(),
            },
        )
        .await
        .unwrap();
    let after_review = counts(&store).await;
    let late = merge_post(&app, &auth[0], merge_fields("1", &["1:1", "2:1"], "t", "r")).await;
    assert_eq!(late.status(), StatusCode::CONFLICT);
    assert_eq!(counts(&store).await, after_review);
}

#[tokio::test]
async fn material_merge_refuses_rejected_foreign_and_wrongly_counted_units() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) = merge_fixture("").await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    store
        .material_review_unit(
            auth[0].clone(),
            auth[0].csrf_fixture(),
            1,
            2,
            crate::sources::UnitReview {
                revision: 1,
                state: "rejected".into(),
                correction: String::new(),
                actor: String::new(),
                rationale: "Noise".into(),
            },
        )
        .await
        .unwrap();
    let before = counts(&store).await;
    let revision = before.2.to_string();
    // A rejected unit whose revision is current: refused by state, not by revision.
    let rejected = merge_post(
        &app,
        &auth[0],
        merge_fields(&revision, &["1:1", "2:2"], "t", "r"),
    )
    .await;
    assert_eq!(rejected.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body(rejected).await.contains("already rejected"));
    let form = router_get(
        &app,
        &auth[0],
        "/admin/materials/pages/1/merges/new?unit=1&unit=2",
    )
    .await;
    assert_eq!(form.status(), StatusCode::UNPROCESSABLE_ENTITY);
    // A unit of another page of the same source.
    let foreign = merge_post(
        &app,
        &auth[0],
        merge_fields(&revision, &["1:1", "900:1"], "t", "r"),
    )
    .await;
    assert_eq!(foreign.status(), StatusCode::FORBIDDEN);
    let form = router_get(
        &app,
        &auth[0],
        "/admin/materials/pages/1/merges/new?unit=1&unit=900",
    )
    .await;
    assert_eq!(form.status(), StatusCode::FORBIDDEN);
    assert_eq!(counts(&store).await, before);
}

#[tokio::test]
async fn material_merge_refuses_wrong_counts_of_units() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) = merge_fixture("").await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    let before = counts(&store).await;
    let revision = before.2.to_string();
    // One unit, a repeated unit and 201 units.
    let one = merge_post(&app, &auth[0], merge_fields(&revision, &["1:1"], "t", "r")).await;
    assert_eq!(one.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let repeated = merge_post(
        &app,
        &auth[0],
        merge_fields(&revision, &["1:1", "1:1"], "t", "r"),
    )
    .await;
    assert_eq!(repeated.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let many: Vec<String> = (1..=201).map(|u| format!("{u}:1")).collect();
    let many_refs: Vec<&str> = many.iter().map(String::as_str).collect();
    let too_many = merge_post(
        &app,
        &auth[0],
        merge_fields(&revision, &many_refs, "t", "r"),
    )
    .await;
    assert_eq!(too_many.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body(too_many).await.contains("Choose 2-200"));
    for query in [
        "?unit=1".to_owned(),
        String::new(),
        "?unit=1&unit=1".into(),
        "?unit=1&unit=3&x=1".into(),
        "?unit=1&unit=x".into(),
    ] {
        let response = router_get(
            &app,
            &auth[0],
            &format!("/admin/materials/pages/1/merges/new{query}"),
        )
        .await;
        assert_eq!(
            response.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "{query}"
        );
    }
    let many_query: String = (1..=201)
        .map(|u| format!("unit={u}"))
        .collect::<Vec<_>>()
        .join("&");
    let response = router_get(
        &app,
        &auth[0],
        &format!("/admin/materials/pages/1/merges/new?{many_query}"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body(response).await.contains("Choose 2-200"));
    assert_eq!(counts(&store).await, before);
}

#[tokio::test]
async fn material_merge_bounds_the_text_and_the_rationale() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) = merge_fixture("").await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    let before = counts(&store).await;
    // 17,000 four-byte characters: under 20,000 characters, over 64 KiB.
    let heavy = "\u{1F600}".repeat(17_000);
    assert!(heavy.len() > 65_536 && heavy.chars().count() < 20_000);
    let cases = [
        (
            "x".repeat(20_001),
            "r".to_owned(),
            "at most 20,000 characters",
        ),
        (heavy, "r".to_owned(), "at most 64 KiB"),
        ("   ".to_owned(), "r".to_owned(), "nonempty correction text"),
        (
            "text".to_owned(),
            "r".repeat(5_001),
            "100/5,000 character limits",
        ),
        ("text\0".to_owned(), "r".to_owned(), "no NUL"),
    ];
    for (text, rationale, mention) in cases {
        let response = merge_post(
            &app,
            &auth[0],
            merge_fields("1", &["1:1", "2:1"], &text, &rationale),
        )
        .await;
        assert_eq!(
            response.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "{mention}"
        );
        let html = body(response).await;
        let message = error_text(&html);
        assert!(message.contains(mention), "{mention}: {message}");
        if !text.contains('\0') {
            assert_eq!(
                field_value(&html, "textarea[name=text]").trim_start_matches('\n'),
                text,
                "the draft is kept"
            );
        }
        assert_eq!(counts(&store).await, before);
    }
    // The bounds themselves are accepted: 20,000 characters.
    let exact = merge_post(
        &app,
        &auth[0],
        merge_fields("1", &["1:1", "2:1"], &"x".repeat(20_000), "r"),
    )
    .await;
    assert_eq!(exact.status(), StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn material_merge_refuses_a_missing_rationale_and_keeps_the_draft() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) = merge_fixture("").await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    let before = counts(&store).await;
    let response = merge_post(
        &app,
        &auth[0],
        merge_fields("1", &["1:1", "2:1"], "Kept text", "  "),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let html = body(response).await;
    let parsed = scraper::Html::parse_document(&html);
    assert!(
        parsed
            .select(&css("#merge-error[role=alert][data-save-error]"))
            .next()
            .is_some()
    );
    assert!(
        parsed
            .select(&css("textarea[name=rationale][aria-invalid=true]"))
            .next()
            .is_some()
    );
    assert_eq!(
        field_value(&html, "textarea[name=text]").trim_start_matches('\n'),
        "Kept text"
    );
    assert_eq!(counts(&store).await, before);
}

#[tokio::test]
async fn material_merge_prefill_over_64_kib_is_cut_with_a_note() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) = merge_fixture(
        "UPDATE source_units SET raw_text=substr(replace(hex(zeroblob(20000)),'0','é'),1,30000) WHERE id=1; UPDATE source_units SET raw_text=substr(replace(hex(zeroblob(20000)),'0','é'),1,30000) WHERE id=2;",
    )
    .await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    let html = body(
        router_get(
            &app,
            &auth[0],
            "/admin/materials/pages/1/merges/new?unit=1&unit=2",
        )
        .await,
    )
    .await;
    assert!(html.contains("id=\"merge-cut\""), "the cut is announced");
    let text = field_value(&html, "textarea[name=text]");
    let text = text.strip_prefix('\n').unwrap_or(&text);
    assert!(
        text.len() <= 65_536 && text.len() > 65_000,
        "{}",
        text.len()
    );
    // An uncut selection carries no note.
    let small = body(
        router_get(
            &app,
            &auth[0],
            "/admin/materials/pages/1/merges/new?unit=3&unit=4",
        )
        .await,
    )
    .await;
    assert!(!small.contains("id=\"merge-cut\""));
}

#[tokio::test]
async fn material_merge_warns_about_tables_empty_units_and_covered_units() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) =
        merge_fixture("UPDATE source_units SET raw_text='' WHERE id=4;").await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    // Unit 3 sits inside the area of units 1 and 2 but is not selected.
    let html = body(
        router_get(
            &app,
            &auth[0],
            "/admin/materials/pages/1/merges/new?unit=1&unit=2",
        )
        .await,
    )
    .await;
    let parsed = scraper::Html::parse_document(&html);
    let overlap: String = parsed
        .select(&css("#merge-overlap"))
        .next()
        .expect("overlap warning")
        .text()
        .collect();
    assert!(overlap.contains("unit 3"), "{overlap}");
    assert!(parsed.select(&css("#merge-warnings")).next().is_none());
    // Selecting it too clears the warning.
    let all = body(
        router_get(
            &app,
            &auth[0],
            "/admin/materials/pages/1/merges/new?unit=1&unit=2&unit=3",
        )
        .await,
    )
    .await;
    assert!(!all.contains("merge-overlap"));
    // A unit without text.
    let empty = body(
        router_get(
            &app,
            &auth[0],
            "/admin/materials/pages/1/merges/new?unit=3&unit=4",
        )
        .await,
    )
    .await;
    assert!(
        empty.contains("Unit 4 (Text) has no text to merge."),
        "{empty}"
    );
}

#[tokio::test]
async fn material_merge_warns_about_a_table_and_saves_owner_units() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) = merge_fixture("").await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    // A selection with a Table: the table is made from units 3 and 4 first.
    let (table, _) = store
        .material_create_table(
            auth[0].clone(),
            auth[0].csrf_fixture(),
            1,
            table_of(&[(3, 1), (4, 1)], grid(&[&["Name", "Damage"]])),
        )
        .await
        .unwrap();
    let with_table = body(
        router_get(
            &app,
            &auth[0],
            &format!("/admin/materials/pages/1/merges/new?unit=1&unit={table}"),
        )
        .await,
    )
    .await;
    let warnings: String = scraper::Html::parse_document(&with_table)
        .select(&css("#merge-warnings"))
        .next()
        .expect("table warning")
        .text()
        .collect();
    assert!(
        warnings.contains(&format!("Unit {table} is a table")),
        "{warnings}"
    );
    // Owner units can be merged (the warning does not block the save).
    let revision = counts(&store).await.2.to_string();
    let table_revision: i64 = store
        .run(move |db| {
            Ok(db.query_row(
                "SELECT revision FROM source_units WHERE id=?1",
                [table],
                |r| r.get(0),
            )?)
        })
        .await
        .unwrap();
    let saved = merge_post(
        &app,
        &auth[0],
        merge_fields(
            &revision,
            &["1:1", &format!("{table}:{table_revision}")],
            "Plain text",
            "r",
        ),
    )
    .await;
    assert_eq!(saved.status(), StatusCode::SEE_OTHER);
    store
        .run(move |db| {
            assert_eq!(merge_unit_text(db, table).1, "rejected");
            Ok(())
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn material_merge_checks_csrf_and_the_owner() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) = merge_fixture("").await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    let before = counts(&store).await;
    let mut wrong = merge_fields("1", &["1:1", "2:1"], "t", "r");
    wrong[0].1 = "wrong".into();
    assert_eq!(
        merge_post(&app, &auth[0], wrong).await.status(),
        StatusCode::FORBIDDEN
    );
    let mut missing = merge_fields("1", &["1:1", "2:1"], "t", "r");
    missing.remove(0);
    assert_eq!(
        merge_post(&app, &auth[0], missing).await.status(),
        StatusCode::FORBIDDEN
    );
    // A non-owner cannot open the form or save.
    assert_eq!(
        router_get(
            &app,
            &auth[1],
            "/admin/materials/pages/1/merges/new?unit=1&unit=2"
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        merge_post(&app, &auth[1], merge_fields("1", &["1:1", "2:1"], "t", "r"))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(counts(&store).await, before);
}

#[tokio::test]
async fn material_merge_rolls_back_everything_when_an_audit_row_fails() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) = merge_fixture("").await;
    let before = counts(&store).await;
    store
        .run(|db| {
            db.execute_batch("CREATE TRIGGER merge_audit_fault BEFORE INSERT ON source_web_events WHEN (SELECT count(*) FROM source_web_events)>=2 BEGIN SELECT RAISE(ABORT,'fixture fault'); END;")?;
            Ok(())
        })
        .await
        .unwrap();
    let merge = crate::sources::OwnerMerge {
        revision: 1,
        sources: vec![
            crate::sources::TableSource {
                unit: 1,
                revision: 1,
            },
            crate::sources::TableSource {
                unit: 2,
                revision: 1,
            },
        ],
        text: "merged".into(),
        actor: "forged".into(),
        rationale: "r".into(),
    };
    assert!(
        store
            .material_create_merge(auth[0].clone(), auth[0].csrf_fixture(), 1, merge)
            .await
            .is_err()
    );
    assert_eq!(counts(&store).await, before);
    let rows: i64 = store
        .run(|db| {
            Ok(
                db.query_row("SELECT count(*) FROM source_owner_replacements", [], |r| {
                    r.get(0)
                })?,
            )
        })
        .await
        .unwrap();
    assert_eq!(rows, 0);
}

#[tokio::test]
async fn material_merge_buttons_work_without_script() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) = merge_fixture("").await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    let html = body(router_get(&app, &auth[0], "/admin/materials/pages/1").await).await;
    let parsed = scraper::Html::parse_document(&html);
    let merge_buttons: Vec<_> = parsed
        .select(&css(
            "button[formaction='/admin/materials/pages/1/merges/new']",
        ))
        .collect();
    // One in the selection form, one in the bottom bar, one after the cards.
    assert_eq!(merge_buttons.len(), 3);
    let no_script: Vec<_> = merge_buttons
        .iter()
        .filter(|b| b.value().attr("data-no-script-table").is_some())
        .collect();
    assert_eq!(
        no_script.len(),
        2,
        "above and below the cards, hidden once script runs"
    );
    for button in &no_script {
        assert!(
            button
                .text()
                .collect::<String>()
                .contains("Merge selected units")
        );
    }
    let bar = parsed
        .select(&css(
            "form#table-select #table-bar button[type=submit][formaction]",
        ))
        .count();
    assert_eq!(bar, 1, "the bar's merge button");
    // Every merge button submits the shared GET form, whose action stays the table form.
    for button in &merge_buttons {
        let in_form = button.ancestors().any(|a| {
            a.value()
                .as_element()
                .is_some_and(|e| e.attr("id") == Some("table-select"))
        }) || button.value().attr("form") == Some("table-select");
        assert!(in_form);
    }
    assert_eq!(
        parsed
            .select(&css("form#table-select"))
            .next()
            .unwrap()
            .value()
            .attr("action"),
        Some("/admin/materials/pages/1/tables/new")
    );
    let label: String = parsed
        .select(&css("#unit-1 .select-unit label"))
        .next()
        .unwrap()
        .text()
        .collect();
    assert!(label.starts_with("Select"), "{label}");
}

#[tokio::test]
async fn material_merge_prefill_cut_adds_no_stray_separator() {
    let _lock = crate::auth::test_resource_lock().await;
    // Unit 1 fills exactly 64 KiB; units 2 and 3 follow.
    let (_dir, store, auth) = merge_fixture(
        "UPDATE source_units SET raw_text=replace(hex(zeroblob(32768)),'0','a') WHERE id=1;",
    )
    .await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    let html = body(
        router_get(
            &app,
            &auth[0],
            "/admin/materials/pages/1/merges/new?unit=1&unit=2&unit=3",
        )
        .await,
    )
    .await;
    assert!(html.contains("id=\"merge-cut\""), "the cut is announced");
    let text = field_value(&html, "textarea[name=text]");
    let text = text.strip_prefix('\n').unwrap_or(&text);
    assert_eq!(text.len(), 65_536, "no separator is pushed past the bound");
}

#[tokio::test]
async fn material_merge_prefill_notes_text_too_long_to_save() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) = merge_fixture(
        "UPDATE source_units SET raw_text=replace(hex(zeroblob(6000)),'0','abcd') WHERE id=1;",
    )
    .await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    // 24,000 characters: over the 20,000 the save accepts.
    let html = body(
        router_get(
            &app,
            &auth[0],
            "/admin/materials/pages/1/merges/new?unit=1&unit=2",
        )
        .await,
    )
    .await;
    let parsed = scraper::Html::parse_document(&html);
    let note: String = parsed
        .select(&css("#merge-too-long"))
        .next()
        .expect("the length notice")
        .text()
        .collect();
    assert!(
        note.contains("20,000") && note.contains("shorten"),
        "{note}"
    );
    assert!(!html.contains("write the rest yourself"));
    let small = body(
        router_get(
            &app,
            &auth[0],
            "/admin/materials/pages/1/merges/new?unit=3&unit=4",
        )
        .await,
    )
    .await;
    assert!(!small.contains("id=\"merge-too-long\""));
}

#[tokio::test]
async fn material_merge_refuses_text_whose_escaped_html_is_over_64_kib() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) = merge_fixture("").await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    let before = counts(&store).await;
    // 20,000 characters, 20,000 bytes; escaped to 100,000 bytes.
    let text = "&".repeat(20_000);
    let response = merge_post(
        &app,
        &auth[0],
        merge_fields("1", &["1:1", "2:1"], &text, "r"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let html = body(response).await;
    let message = error_text(&html);
    assert!(
        message.contains("too long") && message.contains("64 KiB"),
        "{message}"
    );
    assert_eq!(
        field_value(&html, "textarea[name=text]").trim_start_matches('\n'),
        text
    );
    assert_eq!(counts(&store).await, before);
}

#[tokio::test]
async fn material_forms_open_for_a_unit_corrected_with_20000_four_byte_characters() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) = merge_fixture("").await;
    // 20,000 emoji: 80,000 bytes, the largest correction the Owner can save.
    store
        .material_review_unit(
            auth[0].clone(),
            auth[0].csrf_fixture(),
            1,
            1,
            crate::sources::UnitReview {
                revision: 1,
                state: "corrected".into(),
                correction: "\u{1F600}".repeat(20_000),
                actor: String::new(),
                rationale: "Retyped".into(),
            },
        )
        .await
        .unwrap();
    let app = crate::auth::app(store.clone()).await.unwrap();
    for path in [
        "/admin/materials/pages/1/merges/new?unit=1&unit=2",
        "/admin/materials/pages/1/tables/new?unit=1&unit=2",
    ] {
        let response = router_get(&app, &auth[0], path).await;
        assert!(
            response.status() == StatusCode::OK
                || response.status() == StatusCode::UNPROCESSABLE_ENTITY,
            "{path}: {}",
            response.status()
        );
        let html = body(response).await;
        assert!(
            !html.contains("Internal"),
            "{path}: the form must not fail with a server error"
        );
    }
    let merge = router_get(
        &app,
        &auth[0],
        "/admin/materials/pages/1/merges/new?unit=1&unit=2",
    )
    .await;
    assert_eq!(merge.status(), StatusCode::OK);
}

#[tokio::test]
async fn material_table_refuses_rejected_units_on_the_form_and_on_save() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) = merge_fixture("").await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    store
        .material_review_unit(
            auth[0].clone(),
            auth[0].csrf_fixture(),
            1,
            2,
            crate::sources::UnitReview {
                revision: 1,
                state: "rejected".into(),
                correction: String::new(),
                actor: String::new(),
                rationale: "Noise".into(),
            },
        )
        .await
        .unwrap();
    let before = counts(&store).await;
    let revision = before.2.to_string();
    // Unit 2 is at revision 2 now: its current revision, refused by state.
    let mut fields = merge_fields(&revision, &["1:1", "2:2"], "", "r");
    fields.retain(|(k, _)| k != "text");
    fields.push(("cells".to_owned(), "| a | b |".to_owned()));
    let saved = document_at(&app, &auth[0], "/admin/materials/pages/1/tables", fields).await;
    assert_eq!(saved.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body(saved).await.contains("already rejected"));
    let form = router_get(
        &app,
        &auth[0],
        "/admin/materials/pages/1/tables/new?unit=1&unit=2",
    )
    .await;
    assert_eq!(form.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body(form).await.contains("already rejected"));
    assert_eq!(counts(&store).await, before);
}

/// The text inside `#merge-error`.
fn error_text(html: &str) -> String {
    scraper::Html::parse_document(html)
        .select(&css("#merge-error"))
        .next()
        .expect("the error panel")
        .text()
        .collect()
}
