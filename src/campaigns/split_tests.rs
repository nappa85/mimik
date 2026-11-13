//! Splitting one unit into several corrected owner `Text` units (split/merge plan, slice 3).
use super::*;

/// Unit 1 (box x 10-50, y 10-100) holds three lines; the page is 200 x 300 points.
const SPLIT_UNITS: &str = "UPDATE source_units SET raw_text='First\nSecond\nThird' WHERE id=1; UPDATE source_units SET kind='Table' WHERE id=4;";

async fn split_fixture(extra: &'static str) -> (tempfile::TempDir, Store, Vec<RequestAuth>) {
    let leaked: &'static str = Box::leak(format!("{SPLIT_UNITS}{extra}").into_boxed_str());
    two_page_table_fixture(leaked).await
}

/// The page's words: one per line of unit 1, matching its text.
async fn add_words(store: &Store, words: &'static [(&'static str, [f32; 4])]) {
    let words: Vec<crate::sources::page_words::PageWord> = words
        .iter()
        .map(|(text, bounds)| crate::sources::page_words::PageWord {
            text: (*text).to_owned(),
            bounds: *bounds,
        })
        .collect();
    let bytes = crate::sources::page_words::encode(&words, 200.0, 300.0).unwrap();
    store
        .run(move |db| {
            db.execute(
                "INSERT INTO source_page_words(page_id,words) VALUES(1,?1)",
                [bytes],
            )?;
            Ok(())
        })
        .await
        .unwrap();
}

const WORDS: &[(&str, [f32; 4])] = &[
    ("First", [12.0, 12.0, 40.0, 22.0]),
    ("Second", [12.0, 40.0, 45.0, 50.0]),
    ("Third", [12.0, 70.0, 40.0, 80.0]),
];
const THREE: &str = "First\n---\nSecond\n---\nThird";

fn split_path(unit: i64) -> String {
    format!("/admin/materials/pages/1/units/{unit}/split")
}

/// The form's fields: revisions, text, rationale and, per part, the four box values.
fn split_form_fields(
    revision: &str,
    unit_revision: &str,
    text: &str,
    rationale: &str,
    boxes: &[[&str; 4]],
) -> Vec<(String, String)> {
    let mut fields = vec![
        ("csrf".to_owned(), "a".repeat(64)),
        ("revision".to_owned(), revision.to_owned()),
        ("unit_revision".to_owned(), unit_revision.to_owned()),
        ("text".to_owned(), text.to_owned()),
        ("rationale".to_owned(), rationale.to_owned()),
    ];
    for (part, values) in boxes.iter().enumerate() {
        for (corner, value) in values.iter().enumerate() {
            fields.push((format!("box_{}_{corner}", part + 1), (*value).to_owned()));
        }
    }
    fields
}

async fn split_post(
    app: &axum::Router,
    auth: &RequestAuth,
    unit: i64,
    fields: Vec<(String, String)>,
) -> axum::response::Response {
    document_at(app, auth, &split_path(unit), fields).await
}

async fn areas_post(
    app: &axum::Router,
    auth: &RequestAuth,
    unit: i64,
    fields: Vec<(String, String)>,
) -> axum::response::Response {
    document_at(app, auth, &format!("{}/areas", split_path(unit)), fields).await
}

/// The text inside `#split-error`.
fn split_error(html: &str) -> String {
    scraper::Html::parse_document(html)
        .select(&css("#split-error"))
        .next()
        .expect("the error panel")
        .text()
        .collect()
}

/// `(name, value)` of every box field of a form, in document order.
fn box_values(html: &str) -> Vec<(String, String)> {
    scraper::Html::parse_document(html)
        .select(&css("input[name^=box_]"))
        .map(|e| {
            (
                e.value().attr("name").unwrap().to_owned(),
                e.value().attr("value").unwrap().to_owned(),
            )
        })
        .collect()
}

/// The ids of the owner units of the split, in part order.
async fn split_units(store: &Store) -> Vec<i64> {
    store
        .run(|db| {
            Ok(db
                .prepare("SELECT unit_id FROM source_owner_replacements WHERE kind='split' ORDER BY operation_id,part")?
                .query_map([], |r| r.get(0))?
                .collect::<Result<_, _>>()?)
        })
        .await
        .unwrap()
}

type Row = (String, String, i64, String, String);

fn unit_row(db: &rusqlite::Connection, unit: i64) -> Row {
    db.query_row(
        "SELECT kind,state,revision,correction,polygon FROM source_units WHERE id=?1",
        [unit],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
    )
    .unwrap()
}

/// The stored result of splitting unit 1 into `units` (the boxes as `expected` says).
async fn assert_split_rows(store: &Store, units: &[i64], expected: [[&'static str; 4]; 3]) {
    let ids = units.to_vec();
    store
        .run(move |db| {
            for (i, (id, text)) in ids.iter().zip(["First", "Second", "Third"]).enumerate() {
                let (kind, state, revision, correction, polygon) = unit_row(db, *id);
                assert_eq!((kind.as_str(), state.as_str(), revision), ("Text", "corrected", 2));
                assert_eq!(correction, text);
                let points: [[f64; 2]; 4] = serde_json::from_str(&polygon).unwrap();
                let b = expected[i].map(|v| v.parse::<f64>().unwrap());
                assert_eq!(points, [[b[0], b[1]], [b[2], b[1]], [b[2], b[3]], [b[0], b[3]]]);
            }
            assert_eq!(unit_row(db, 1).1, "rejected");
            // One operation, one rationale, the Owner's account, an audit row per event.
            let rows: Vec<(i64, String, String, String, i64, String)> = db
                .prepare("SELECT unit_id,state,actor,rationale,previous_revision,operation_id FROM source_review_events WHERE operation_id IS NOT NULL ORDER BY id")?
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))?
                .collect::<Result<_, _>>()?;
            assert_eq!(rows.len(), 4);
            for (row, id) in rows.iter().zip(&ids) {
                assert_eq!((row.0, row.1.as_str(), row.4), (*id, "corrected", 1));
            }
            assert_eq!((rows[3].0, rows[3].1.as_str()), (1, "rejected"));
            assert!(rows.iter().all(|r| r.2 == "account:1" && r.3 == "Checked the page" && r.5 == rows[0].5));
            let audited: i64 = db.query_row(
                "SELECT count(*) FROM source_web_events w JOIN source_review_events e ON e.id=w.event_id WHERE e.operation_id=?1",
                [&rows[0].5],
                |r| r.get(0),
            )?;
            assert_eq!(audited, 4);
            let replaced: Vec<(i64, i64, String)> = db
                .prepare("SELECT anchor_unit_id,part,order_key FROM source_owner_replacements WHERE kind='split' ORDER BY part")?
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect::<Result<_, _>>()?;
            assert_eq!(
                replaced,
                [
                    (1, 1, "00000000000000000001.0001".to_owned()),
                    (1, 2, "00000000000000000001.0002".to_owned()),
                    (1, 3, "00000000000000000001.0003".to_owned()),
                ]
            );
            Ok(())
        })
        .await
        .unwrap();
}

/// Reading order, the notice and the history line after splitting unit 1 into `units`.
async fn assert_split_screens(
    app: &axum::Router,
    auth: &[RequestAuth],
    store: &Store,
    units: &[i64],
    location: &str,
) {
    // Reading order: the split unit, its parts right after it, then unit 2.
    for (cursor, id) in [(0, 1), (1, units[0]), (2, units[1]), (3, units[2]), (4, 2)] {
        assert_eq!(
            store
                .material_unit_cursor(auth[0].clone(), 1, id)
                .await
                .unwrap(),
            cursor
        );
    }
    // The notice link is true only for a split; the history names the parts once.
    let page = body(router_get(app, &auth[0], location).await).await;
    assert!(
        page.contains(&format!("Split: unit {} is the first", units[0])),
        "{page}"
    );
    let line = format!(
        "Unit 1 split into {}, {}, {}.",
        units[0], units[1], units[2]
    );
    assert_eq!(page.matches(&line).count(), 1, "{page}");
    let forged = body(router_get(app, &auth[0], "/admin/materials/pages/1?split=3").await).await;
    assert!(!forged.contains("Split: unit"));
    let as_merge = body(
        router_get(
            app,
            &auth[0],
            &format!("/admin/materials/pages/1?merge={}", units[0]),
        )
        .await,
    )
    .await;
    assert!(!as_merge.contains("Merged into unit"));
    // The rejected unit has no "Split…" link any more; its parts do.
    let parsed = scraper::Html::parse_document(&page);
    assert!(
        parsed
            .select(&css("a[href='/admin/materials/pages/1/units/1/split']"))
            .next()
            .is_none()
    );
    assert!(
        parsed
            .select(&css(&format!(
                "a[href='/admin/materials/pages/1/units/{}/split']",
                units[0]
            )))
            .next()
            .is_some()
    );
}

#[tokio::test]
async fn material_split_previews_the_areas_then_saves_corrected_units() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) = split_fixture("").await;
    add_words(&store, WORDS).await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    let form = router_get(&app, &auth[0], &split_path(1)).await;
    assert_eq!(form.status(), StatusCode::OK);
    let html = body(form).await;
    assert_eq!(
        field_value(&html, "textarea[name=text]").trim_start_matches('\n'),
        "First\nSecond\nThird"
    );
    assert!(box_values(&html).is_empty(), "no areas before the preview");
    let before = counts(&store).await;
    // The preview writes nothing and carries the text and revisions through.
    let shown = areas_post(
        &app,
        &auth[0],
        1,
        split_form_fields("1", "1", THREE, "Checked the page", &[]),
    )
    .await;
    assert_eq!(shown.status(), StatusCode::OK);
    let shown = body(shown).await;
    assert_eq!(counts(&store).await, before);
    let parsed = scraper::Html::parse_document(&shown);
    let note: String = parsed
        .select(&css("#split-areas-note"))
        .next()
        .unwrap()
        .text()
        .collect();
    assert!(note.contains("match this text exactly"), "{note}");
    assert_eq!(
        parsed.select(&css("fieldset[data-region-editor]")).count(),
        3
    );
    let boxes = box_values(&shown);
    let expected = [
        ["12.00", "12.00", "40.00", "22.00"],
        ["12.00", "40.00", "45.00", "50.00"],
        ["12.00", "70.00", "40.00", "80.00"],
    ];
    let flat: Vec<&str> = expected.iter().flatten().copied().collect();
    assert_eq!(boxes.iter().map(|b| b.1.as_str()).collect::<Vec<_>>(), flat);
    for name in ["revision", "unit_revision", "csrf"] {
        assert!(
            parsed
                .select(&css(&format!(
                    "form[data-form=split] input[type=hidden][name={name}]"
                )))
                .next()
                .is_some()
        );
    }
    // Save with the shown boxes.
    let saved = split_post(
        &app,
        &auth[0],
        1,
        split_form_fields("1", "1", THREE, "Checked the page", &expected),
    )
    .await;
    assert_eq!(saved.status(), StatusCode::SEE_OTHER);
    let location = saved.headers()["location"].to_str().unwrap().to_owned();
    let units = split_units(&store).await;
    assert_eq!(units.len(), 3);
    assert_eq!(
        location,
        format!(
            "/admin/materials/pages/1?units=1&split={0}#unit-{0}",
            units[0]
        )
    );
    let after = counts(&store).await;
    assert_eq!(after.0, before.0 + 3);
    assert_eq!(
        after.1,
        before.1 + 4,
        "three corrected events and one rejected"
    );
    assert_split_rows(&store, &units, expected).await;
    assert_split_screens(&app, &auth, &store, &units, &location).await;
}

#[tokio::test]
async fn material_split_without_a_preview_computes_the_areas_and_falls_back_to_bands() {
    let _lock = crate::auth::test_resource_lock().await;
    // With words: the same boxes the preview shows.
    let (_dir, store, auth) = split_fixture("").await;
    add_words(&store, WORDS).await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    let saved = split_post(
        &app,
        &auth[0],
        1,
        split_form_fields("1", "1", THREE, "r", &[]),
    )
    .await;
    assert_eq!(saved.status(), StatusCode::SEE_OTHER);
    let units = split_units(&store).await;
    let polygons: Vec<[[f64; 2]; 2]> = store
        .run(move |db| {
            Ok(units
                .iter()
                .map(|u| {
                    let p: [[f64; 2]; 4] = serde_json::from_str(&unit_row(db, *u).4).unwrap();
                    [p[0], p[2]]
                })
                .collect())
        })
        .await
        .unwrap();
    assert_eq!(
        polygons,
        [
            [[12.0, 12.0], [40.0, 22.0]],
            [[12.0, 40.0], [45.0, 50.0]],
            [[12.0, 70.0], [40.0, 80.0]]
        ]
    );
    // Without words: bands cut from the unit's box (x 10-50, y 10-100), and the form says so.
    let (_dir2, store2, auth2) = split_fixture("").await;
    let app2 = crate::auth::app(store2.clone()).await.unwrap();
    let shown = body(
        areas_post(
            &app2,
            &auth2[0],
            1,
            split_form_fields("1", "1", THREE, "r", &[]),
        )
        .await,
    )
    .await;
    let note: String = scraper::Html::parse_document(&shown)
        .select(&css("#split-areas-note"))
        .next()
        .unwrap()
        .text()
        .collect();
    assert!(note.contains("approximate"), "{note}");
    let saved = split_post(
        &app2,
        &auth2[0],
        1,
        split_form_fields("1", "1", THREE, "r", &[]),
    )
    .await;
    assert_eq!(saved.status(), StatusCode::SEE_OTHER);
    let units = split_units(&store2).await;
    let bands: Vec<[f64; 4]> = store2
        .run(move |db| {
            Ok(units
                .iter()
                .map(|u| {
                    let p: [[f64; 2]; 4] = serde_json::from_str(&unit_row(db, *u).4).unwrap();
                    [p[0][0], p[0][1], p[2][0], p[2][1]]
                })
                .collect())
        })
        .await
        .unwrap();
    assert_eq!(
        bands,
        [
            [10.0, 10.0, 50.0, 40.0],
            [10.0, 40.0, 50.0, 70.0],
            [10.0, 70.0, 50.0, 100.0]
        ]
    );
}

#[tokio::test]
async fn material_split_refuses_bad_boxes_and_keeps_them() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) = split_fixture("").await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    let before = counts(&store).await;
    // A box outside the page, an empty box and a box that is not a number.
    for (boxes, mention) in [
        (
            [["0", "0", "250", "50"], ["5", "60", "40", "90"]],
            "inside the page",
        ),
        (
            [["10", "10", "10", "40"], ["5", "60", "40", "90"]],
            "not empty",
        ),
        (
            [["10", "10", "40", "40"], ["5", "60", "x", "90"]],
            "not a number",
        ),
        (
            [["-1", "10", "40", "40"], ["5", "60", "40", "90"]],
            "inside the page",
        ),
    ] {
        let response = split_post(
            &app,
            &auth[0],
            1,
            split_form_fields("1", "1", "One\n---\nTwo", "My rationale", &boxes),
        )
        .await;
        assert_eq!(
            response.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "{boxes:?}"
        );
        let html = body(response).await;
        let message = split_error(&html);
        assert!(message.contains(mention), "{mention}: {message}");
        // The draft, the revisions and the submitted boxes are kept.
        assert_eq!(
            field_value(&html, "textarea[name=text]").trim_start_matches('\n'),
            "One\n---\nTwo"
        );
        assert_eq!(
            field_value(&html, "textarea[name=rationale]").trim_start_matches('\n'),
            "My rationale"
        );
        let kept: Vec<String> = box_values(&html).into_iter().map(|b| b.1).collect();
        assert_eq!(
            kept,
            boxes
                .iter()
                .flatten()
                .map(|v| (*v).to_owned())
                .collect::<Vec<_>>()
        );
        assert_eq!(counts(&store).await, before);
    }
    // Boxes that do not match the parts (two boxes for three parts, and none for a part).
    let mismatch = split_post(
        &app,
        &auth[0],
        1,
        split_form_fields(
            "1",
            "1",
            THREE,
            "r",
            &[["1", "1", "9", "9"], ["1", "1", "9", "9"]],
        ),
    )
    .await;
    assert_eq!(mismatch.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(split_error(&body(mismatch).await).contains("do not match"));
    assert_eq!(counts(&store).await, before);
}

#[tokio::test]
async fn material_split_saves_edited_boxes_as_submitted() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) = split_fixture("").await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    // Edited boxes, one reaching outside the unit but inside the page, are saved as submitted.
    let saved = split_post(
        &app,
        &auth[0],
        1,
        split_form_fields(
            "1",
            "1",
            "One\n---\nTwo",
            "r",
            &[["5", "6", "45.5", "50"], ["0", "60", "200", "300"]],
        ),
    )
    .await;
    assert_eq!(saved.status(), StatusCode::SEE_OTHER);
    let units = split_units(&store).await;
    let polygons: Vec<[[f64; 2]; 4]> = store
        .run(move |db| {
            Ok(units
                .iter()
                .map(|u| serde_json::from_str(&unit_row(db, *u).4).unwrap())
                .collect())
        })
        .await
        .unwrap();
    assert_eq!(
        polygons[0],
        [[5.0, 6.0], [45.5, 6.0], [45.5, 50.0], [5.0, 50.0]]
    );
    assert_eq!(
        polygons[1],
        [[0.0, 60.0], [200.0, 60.0], [200.0, 300.0], [0.0, 300.0]]
    );
}

#[tokio::test]
async fn material_split_splits_a_corrected_unit_and_an_owner_unit() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) = split_fixture("").await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    store
        .material_review_unit(
            auth[0].clone(),
            auth[0].csrf_fixture(),
            1,
            1,
            crate::sources::UnitReview {
                revision: 1,
                state: "corrected".into(),
                correction: "Fixed one\nFixed two".into(),
                actor: String::new(),
                rationale: "Typo".into(),
            },
        )
        .await
        .unwrap();
    // The form holds the correction, not the extraction.
    let html = body(router_get(&app, &auth[0], &split_path(1)).await).await;
    assert_eq!(
        field_value(&html, "textarea[name=text]").trim_start_matches('\n'),
        "Fixed one\nFixed two"
    );
    let page_revision = counts(&store).await.2.to_string();
    let saved = split_post(
        &app,
        &auth[0],
        1,
        split_form_fields(
            &page_revision,
            "2",
            "Fixed one\n---\nFixed two",
            "Split a corrected unit",
            &[],
        ),
    )
    .await;
    assert_eq!(saved.status(), StatusCode::SEE_OTHER);
    let parts = split_units(&store).await;
    assert_eq!(parts.len(), 2);
    // Splitting a part (an owner unit) again: its parts follow it in reading order.
    let revision = counts(&store).await.2.to_string();
    let again = split_post(
        &app,
        &auth[0],
        parts[0],
        split_form_fields(&revision, "2", "Fixed\n---\none", "Again", &[]),
    )
    .await;
    assert_eq!(again.status(), StatusCode::SEE_OTHER);
    let anchor = parts[0];
    let owner_parts: Vec<(i64, String)> = store
        .run(move |db| {
            Ok(db
                .prepare("SELECT unit_id,order_key FROM source_owner_replacements WHERE kind='split' AND anchor_unit_id=?1 ORDER BY part")?
                .query_map([anchor], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<Result<_, _>>()?)
        })
        .await
        .unwrap();
    assert_eq!(owner_parts.len(), 2);
    assert!(
        owner_parts[0].1.starts_with("00000000000000000001.0001."),
        "{owner_parts:?}"
    );
    let first = store
        .material_unit_cursor(auth[0].clone(), 1, parts[0])
        .await
        .unwrap();
    let next = store
        .material_unit_cursor(auth[0].clone(), 1, owner_parts[0].0)
        .await
        .unwrap();
    assert_eq!(next, first + 1);
}

#[tokio::test]
async fn material_split_warns_when_the_original_has_a_marker_line() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) =
        split_fixture("UPDATE source_units SET raw_text='before\n  ---  \nafter' WHERE id=2;")
            .await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    let warned = body(router_get(&app, &auth[0], &split_path(2)).await).await;
    let panel: String = scraper::Html::parse_document(&warned)
        .select(&css("#split-has-marker"))
        .next()
        .expect("the warning")
        .text()
        .collect();
    assert!(panel.contains("split there"), "{panel}");
    let plain = body(router_get(&app, &auth[0], &split_path(1)).await).await;
    assert!(!plain.contains("split-has-marker"));
}

#[tokio::test]
async fn material_split_reads_crlf_markers_and_refuses_wrong_part_counts() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) = split_fixture("").await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    let before = counts(&store).await;
    let cases: Vec<(String, &str)> = vec![
        ("A\n---\n\n---\nC".into(), "Part 2 is empty."),
        ("---\nB".into(), "Part 1 is empty."),
        ("A\n---".into(), "Part 2 is empty."),
        ("A\n---\n   \n---\nC\n---\nD".into(), "Part 2 is empty."),
        ("Only one part".into(), "2-20 parts"),
        ("A\n--\nB".into(), "2-20 parts"),
        (vec!["x"; 21].join("\n---\n"), "2-20 parts"),
    ];
    for (text, mention) in &cases {
        for areas in [false, true] {
            let fields = split_form_fields("1", "1", text, "r", &[]);
            let response = if areas {
                areas_post(&app, &auth[0], 1, fields).await
            } else {
                split_post(&app, &auth[0], 1, fields).await
            };
            assert_eq!(
                response.status(),
                StatusCode::UNPROCESSABLE_ENTITY,
                "{text:?}"
            );
            let html = body(response).await;
            let message = split_error(&html);
            assert!(message.contains(mention), "{text:?}: {message}");
            assert_eq!(
                field_value(&html, "textarea[name=text]").trim_start_matches('\n'),
                *text
            );
        }
    }
    assert_eq!(counts(&store).await, before);
    // CRLF line breaks and padded markers: the parts are the same as with LF.
    let crlf = split_post(
        &app,
        &auth[0],
        1,
        split_form_fields("1", "1", "One\r\nline\r\n  ---\t\r\nTwo\r\n", "r", &[]),
    )
    .await;
    assert_eq!(crlf.status(), StatusCode::SEE_OTHER);
    let units = split_units(&store).await;
    let texts: Vec<String> = store
        .run(move |db| Ok(units.iter().map(|u| unit_row(db, *u).3).collect()))
        .await
        .unwrap();
    assert_eq!(texts, ["One\nline", "Two"]);
}

#[tokio::test]
async fn material_split_takes_twenty_parts_and_bounds_each_part_and_the_whole() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) = split_fixture("").await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    let before = counts(&store).await;
    // 11,000 quotes: 11,000 characters, 66,000 bytes once escaped.
    let heavy = format!("{}\n---\nb", "\"".repeat(11_000));
    let whole = format!("{}\n---\n{}", "x".repeat(40_000), "y".repeat(40_000));
    for (text, mention) in [
        (
            format!("{}\n---\nb", "x".repeat(20_001)),
            "at most 20,000 characters",
        ),
        (heavy, "at most 64 KiB, and"),
        (whole, "64 KiB in all"),
    ] {
        let response = split_post(
            &app,
            &auth[0],
            1,
            split_form_fields("1", "1", &text, "r", &[]),
        )
        .await;
        assert_eq!(
            response.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "{mention}"
        );
        let message = split_error(&body(response).await);
        assert!(message.contains(mention), "{mention}: {message}");
    }
    assert_eq!(counts(&store).await, before);
    // A part of exactly 20,000 characters and 20 parts are taken.
    let twenty = (1..=20)
        .map(|n| {
            if n == 1 {
                "x".repeat(20_000)
            } else {
                format!("part {n}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n---\n");
    let saved = split_post(
        &app,
        &auth[0],
        1,
        split_form_fields("1", "1", &twenty, "r", &[]),
    )
    .await;
    assert_eq!(saved.status(), StatusCode::SEE_OTHER);
    let units = split_units(&store).await;
    assert_eq!(units.len(), 20);
    // Parts keep their order in reading order, right after the split unit.
    for (i, unit) in units.iter().enumerate() {
        assert_eq!(
            store
                .material_unit_cursor(auth[0].clone(), 1, *unit)
                .await
                .unwrap(),
            i64::try_from(i).unwrap() + 1
        );
    }
}

#[tokio::test]
async fn material_split_refuses_a_missing_rationale_and_keeps_the_draft() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) = split_fixture("").await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    let before = counts(&store).await;
    let response = split_post(
        &app,
        &auth[0],
        1,
        split_form_fields(
            "1",
            "1",
            "One\n---\nTwo",
            "  ",
            &[["1", "1", "9", "9"], ["1", "1", "9", "9"]],
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let html = body(response).await;
    let parsed = scraper::Html::parse_document(&html);
    assert!(
        parsed
            .select(&css("#split-error[role=alert][data-save-error]"))
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
        "One\n---\nTwo"
    );
    assert_eq!(box_values(&html).len(), 8);
    assert_eq!(counts(&store).await, before);
}

#[tokio::test]
async fn material_split_refuses_stale_revisions_after_a_preview() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) = split_fixture("").await;
    add_words(&store, WORDS).await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    let shown = body(
        areas_post(
            &app,
            &auth[0],
            1,
            split_form_fields("1", "1", THREE, "r", &[]),
        )
        .await,
    )
    .await;
    let boxes: Vec<(String, String)> = box_values(&shown);
    let mut fields = split_form_fields("1", "1", THREE, "My rationale", &[]);
    fields.extend(boxes);
    let before = counts(&store).await;
    // The page revision moved on (another unit decided), unit 1's revision did not.
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
    assert_eq!(after_review.2, before.2 + 1);
    let stale_page = split_post(&app, &auth[0], 1, fields.clone()).await;
    assert_eq!(stale_page.status(), StatusCode::CONFLICT);
    let html = body(stale_page).await;
    assert_ne!(split_error(&html), "");
    assert_eq!(
        field_value(&html, "textarea[name=text]").trim_start_matches('\n'),
        THREE
    );
    assert_eq!(
        field_value(&html, "textarea[name=rationale]").trim_start_matches('\n'),
        "My rationale"
    );
    assert_eq!(box_values(&html).len(), 12, "the shown boxes are kept");
    assert_eq!(counts(&store).await, after_review);
    // The unit itself decided after the preview: its revision is stale though the page one is current.
    let page_now = after_review.2.to_string();
    store
        .material_review_unit(
            auth[0].clone(),
            auth[0].csrf_fixture(),
            1,
            1,
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
    let after_unit = counts(&store).await;
    let page_now_2 = after_unit.2.to_string();
    let stale_unit = split_post(
        &app,
        &auth[0],
        1,
        split_form_fields(&page_now_2, "1", THREE, "r", &[]),
    )
    .await;
    assert_eq!(stale_unit.status(), StatusCode::CONFLICT);
    assert_eq!(counts(&store).await, after_unit);
    // A stale unit revision with the previous page revision is refused too.
    let both = split_post(
        &app,
        &auth[0],
        1,
        split_form_fields(&page_now, "1", THREE, "r", &[]),
    )
    .await;
    assert_eq!(both.status(), StatusCode::CONFLICT);
    // The current revisions save (the accepted unit may be split).
    let current = split_post(
        &app,
        &auth[0],
        1,
        split_form_fields(&page_now_2, "2", THREE, "r", &[]),
    )
    .await;
    assert_eq!(current.status(), StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn material_split_refuses_a_rejected_unit() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) = split_fixture("").await;
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
    // A rejected unit, with its current revision: refused by state; no link on its card.
    let rejected = split_post(
        &app,
        &auth[0],
        2,
        split_form_fields(&revision, "2", THREE, "r", &[]),
    )
    .await;
    assert_eq!(rejected.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(split_error(&body(rejected).await).contains("already rejected"));
    let form = router_get(&app, &auth[0], &split_path(2)).await;
    assert_eq!(form.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body(form).await.contains("already rejected"));
    let areas = areas_post(
        &app,
        &auth[0],
        2,
        split_form_fields(&revision, "2", THREE, "r", &[]),
    )
    .await;
    assert_eq!(areas.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let page = body(router_get(&app, &auth[0], "/admin/materials/pages/1").await).await;
    assert!(
        scraper::Html::parse_document(&page)
            .select(&css("a[href='/admin/materials/pages/1/units/2/split']"))
            .next()
            .is_none(),
        "a rejected unit has no Split link"
    );
    assert_eq!(counts(&store).await, before);
}

#[tokio::test]
async fn material_split_refuses_non_text_and_foreign_units() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) = split_fixture("").await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    let before = counts(&store).await;
    let revision = before.2.to_string();
    // A table (unit 4): no link, refused on the form, the preview and the save.
    let page = body(router_get(&app, &auth[0], "/admin/materials/pages/1").await).await;
    let parsed = scraper::Html::parse_document(&page);
    assert!(
        parsed
            .select(&css("a[href='/admin/materials/pages/1/units/1/split']"))
            .next()
            .is_some()
    );
    assert!(
        parsed
            .select(&css("a[href='/admin/materials/pages/1/units/4/split']"))
            .next()
            .is_none(),
        "a table has no Split link"
    );
    let table = split_post(
        &app,
        &auth[0],
        4,
        split_form_fields(&revision, "1", THREE, "r", &[]),
    )
    .await;
    assert_eq!(table.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(split_error(&body(table).await).contains("Only text units"));
    assert_eq!(
        router_get(&app, &auth[0], &split_path(4)).await.status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        areas_post(
            &app,
            &auth[0],
            4,
            split_form_fields(&revision, "1", THREE, "r", &[])
        )
        .await
        .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    // A unit of another page of the same source.
    assert_eq!(
        router_get(&app, &auth[0], &split_path(900)).await.status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        split_post(
            &app,
            &auth[0],
            900,
            split_form_fields(&revision, "1", THREE, "r", &[])
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        areas_post(
            &app,
            &auth[0],
            900,
            split_form_fields(&revision, "1", THREE, "r", &[])
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(counts(&store).await, before);
}

#[tokio::test]
async fn material_split_checks_csrf_and_the_owner() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) = split_fixture("").await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    let before = counts(&store).await;
    let (app_ref, auth_ref) = (&app, &auth);
    for areas in [false, true] {
        let post = |fields| async move {
            if areas {
                areas_post(app_ref, &auth_ref[0], 1, fields).await
            } else {
                split_post(app_ref, &auth_ref[0], 1, fields).await
            }
        };
        let mut wrong = split_form_fields("1", "1", THREE, "r", &[]);
        wrong[0].1 = "wrong".into();
        assert_eq!(post(wrong).await.status(), StatusCode::FORBIDDEN);
        let mut missing = split_form_fields("1", "1", THREE, "r", &[]);
        missing.remove(0);
        assert_eq!(post(missing).await.status(), StatusCode::FORBIDDEN);
    }
    // A non-owner can neither open the form, preview nor save.
    assert_eq!(
        router_get(&app, &auth[1], &split_path(1)).await.status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        areas_post(
            &app,
            &auth[1],
            1,
            split_form_fields("1", "1", THREE, "r", &[])
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        split_post(
            &app,
            &auth[1],
            1,
            split_form_fields("1", "1", THREE, "r", &[])
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(counts(&store).await, before);
}

#[tokio::test]
async fn material_split_rolls_back_everything_when_an_audit_row_fails() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) = split_fixture("").await;
    let before = counts(&store).await;
    store
        .run(|db| {
            db.execute_batch("CREATE TRIGGER split_audit_fault BEFORE INSERT ON source_web_events WHEN (SELECT count(*) FROM source_web_events)>=2 BEGIN SELECT RAISE(ABORT,'fixture fault'); END;")?;
            Ok(())
        })
        .await
        .unwrap();
    let split = crate::sources::OwnerSplit {
        revision: 1,
        unit: crate::sources::TableSource {
            unit: 1,
            revision: 1,
        },
        text: THREE.into(),
        boxes: None,
        actor: "forged".into(),
        rationale: "r".into(),
    };
    assert!(
        store
            .material_create_split(auth[0].clone(), auth[0].csrf_fixture(), 1, split)
            .await
            .is_err()
    );
    assert_eq!(counts(&store).await, before);
    assert_eq!(split_units(&store).await.len(), 0);
}

#[tokio::test]
async fn material_split_form_and_preview_work_without_script() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) = split_fixture("").await;
    add_words(&store, WORDS).await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    let page = body(router_get(&app, &auth[0], "/admin/materials/pages/1").await).await;
    let link: String = scraper::Html::parse_document(&page)
        .select(&css(
            "#unit-1 .unit-actions a[href='/admin/materials/pages/1/units/1/split']",
        ))
        .next()
        .expect("the Split link in the card actions")
        .text()
        .collect();
    assert!(link.starts_with("Split"), "{link}");
    let form = body(router_get(&app, &auth[0], &split_path(1)).await).await;
    let parsed = scraper::Html::parse_document(&form);
    let form_element = parsed.select(&css("form[data-form=split]")).next().unwrap();
    assert_eq!(form_element.value().attr("method"), Some("post"));
    assert_eq!(
        form_element.value().attr("action"),
        Some("/admin/materials/pages/1/units/1/split")
    );
    let preview_button = parsed
        .select(&css(
            "button[formaction='/admin/materials/pages/1/units/1/split/areas']",
        ))
        .next()
        .unwrap();
    assert!(preview_button.value().attr("hidden").is_none());
    assert!(preview_button.value().attr("formnovalidate").is_some());
    // The Split button comes first, so Enter in a field saves rather than recomputes.
    let first_submit = parsed
        .select(&css("form[data-form=split] button[type=submit]"))
        .next()
        .unwrap();
    assert!(first_submit.value().attr("formaction").is_none());
    // The preview has four plain number fields per part, in page points.
    let shown = body(
        areas_post(
            &app,
            &auth[0],
            1,
            split_form_fields("1", "1", THREE, "r", &[]),
        )
        .await,
    )
    .await;
    let parsed = scraper::Html::parse_document(&shown);
    for part in 1..=3 {
        for corner in 0..4 {
            let field = parsed
                .select(&css(&format!(
                    "input[type=number][name=box_{part}_{corner}][data-coordinate='{corner}']"
                )))
                .next()
                .unwrap_or_else(|| panic!("box {part} {corner}"));
            assert!(field.value().attr("hidden").is_none());
        }
    }
    assert_eq!(
        parsed
            .select(&css("fieldset[data-split-part] [data-region-draw][hidden]"))
            .count(),
        3,
        "script-only buttons are hidden until the script runs"
    );
}

#[tokio::test]
async fn material_split_text_never_goes_in_the_url() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) = split_fixture("").await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    // The preview is a POST: the same path as a GET is only the form, and a text query is ignored.
    let html = body(
        router_get(
            &app,
            &auth[0],
            &format!("{}?text=Injected%0A---%0Atext", split_path(1)),
        )
        .await,
    )
    .await;
    assert!(!html.contains("Injected"));
    assert_eq!(box_values(&html).len(), 0);
}

/// A fixture whose page is A4 (595.2756 x 841.8898, not integral) and whose unit 1 spans it.
async fn a4_fixture() -> (tempfile::TempDir, Store, Vec<RequestAuth>) {
    sized_two_page_fixture(
        SPLIT_UNITS,
        "[[10,10],[595.2756,10],[595.2756,841.8898],[10,841.8898]]",
        (serde_json::json!(595.2756), serde_json::json!(841.8898)),
    )
    .await
}

#[tokio::test]
async fn material_split_saves_an_unedited_preview_on_a_page_of_non_integral_size() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) = a4_fixture().await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    let shown = body(
        areas_post(
            &app,
            &auth[0],
            1,
            split_form_fields("1", "1", THREE, "r", &[]),
        )
        .await,
    )
    .await;
    let shown_boxes = box_values(&shown);
    assert_eq!(shown_boxes.len(), 12);
    // The unedited values go back as they are shown.
    let values: Vec<String> = shown_boxes.iter().map(|b| b.1.clone()).collect();
    let rows: Vec<[&str; 4]> = values
        .chunks(4)
        .map(|c| [c[0].as_str(), c[1].as_str(), c[2].as_str(), c[3].as_str()])
        .collect();
    let saved = split_post(
        &app,
        &auth[0],
        1,
        split_form_fields("1", "1", THREE, "r", &rows),
    )
    .await;
    assert_eq!(
        saved.status(),
        StatusCode::SEE_OTHER,
        "shown boxes {values:?}"
    );
    assert_eq!(split_units(&store).await.len(), 3);
}

#[tokio::test]
async fn material_split_crops_load_through_the_region_retry_and_only_the_first_editor_opens() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) = split_fixture("").await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    let shown = body(
        areas_post(
            &app,
            &auth[0],
            1,
            split_form_fields("1", "1", THREE, "r", &[]),
        )
        .await,
    )
    .await;
    let parsed = scraper::Html::parse_document(&shown);
    let images: Vec<_> = parsed.select(&css("form[data-form=split] img")).collect();
    assert_eq!(images.len(), 4, "the overview and one crop per part");
    // Each crop sits in a `.source-region` figure with a caption, which the retry script needs.
    assert_eq!(
        parsed
            .select(&css(
                "form[data-form=split] figure.source-region:has(img):has(figcaption)"
            ))
            .count(),
        4
    );
    for image in &images {
        assert_eq!(image.value().attr("loading"), Some("lazy"));
    }
    assert!(shown.contains("/assets/source-regions.js"));
    // Each part's editor is a details; only the first is open, the others load when opened.
    let editors: Vec<_> = parsed
        .select(&css("fieldset[data-split-part] details"))
        .collect();
    assert_eq!(editors.len(), 3);
    assert_eq!(
        editors
            .iter()
            .map(|d| d.value().attr("open").is_some())
            .collect::<Vec<_>>(),
        [true, false, false]
    );
    // The fields stay outside the closed details, so they submit without script.
    assert_eq!(
        parsed
            .select(&css("fieldset[data-split-part] details input"))
            .count(),
        0
    );
}

#[tokio::test]
async fn material_split_refuses_text_cut_for_the_form_even_after_it_was_shortened() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) = split_fixture("").await;
    // A correction of 17,000 four-byte characters: 68,000 bytes, within 20,000 characters.
    store
        .material_review_unit(
            auth[0].clone(),
            auth[0].csrf_fixture(),
            1,
            1,
            crate::sources::UnitReview {
                revision: 1,
                state: "corrected".into(),
                correction: "\u{1F600}".repeat(17_000),
                actor: String::new(),
                rationale: "Long".into(),
            },
        )
        .await
        .unwrap();
    let app = crate::auth::app(store.clone()).await.unwrap();
    let before = counts(&store).await;
    let form = body(router_get(&app, &auth[0], &split_path(1)).await).await;
    let text = field_value(&form, "textarea[name=text]");
    let text = text.trim_start_matches('\n');
    assert_eq!(text.len(), 65_536, "the form shows the cut text");
    // Replace the last bytes by a marker: the text now fits, and the lost tail is not noticed.
    let edited = format!("{}\n---\nb", &text[..text.len() - 20]);
    let response = split_post(
        &app,
        &auth[0],
        1,
        split_form_fields(&before.2.to_string(), "2", &edited, "r", &[]),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let message = split_error(&body(response).await);
    assert!(message.contains("more than 64 KiB"), "{message}");
    assert_eq!(counts(&store).await, before);
}

#[tokio::test]
async fn material_split_names_the_part_count_before_the_boxes() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) = split_fixture("").await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    let text = (1..=21)
        .map(|n| format!("part {n}"))
        .collect::<Vec<_>>()
        .join("\n---\n");
    // Boxes for 21 parts cannot be posted for 20; the part count is what is refused first.
    let boxes: Vec<[&str; 4]> = (0..21).map(|_| ["10", "10", "40", "20"]).collect();
    let response = split_post(
        &app,
        &auth[0],
        1,
        split_form_fields("1", "1", &text, "r", &boxes),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let message = split_error(&body(response).await);
    assert!(message.contains("2-20 parts"), "{message}");
}

#[tokio::test]
async fn material_split_areas_keeps_edited_boxes_while_the_part_count_holds() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) = split_fixture("").await;
    add_words(&store, WORDS).await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    let edited = [
        ["11", "11", "41", "23"],
        ["13", "41", "46", "51"],
        ["14", "71", "41", "81"],
    ];
    let shown = body(
        areas_post(
            &app,
            &auth[0],
            1,
            split_form_fields("1", "1", THREE, "r", &edited),
        )
        .await,
    )
    .await;
    let values: Vec<String> = box_values(&shown).into_iter().map(|b| b.1).collect();
    let expected: Vec<&str> = edited.iter().flatten().copied().collect();
    assert_eq!(values, expected);
    let note = field_value(&shown, "#split-areas-note");
    assert!(note.contains("kept"), "{note}");
    // A fourth part changes the count: the areas are computed again for the new text.
    let more = format!("{THREE}\n---\nFourth");
    let shown = body(
        areas_post(
            &app,
            &auth[0],
            1,
            split_form_fields("1", "1", &more, "r", &edited),
        )
        .await,
    )
    .await;
    let values: Vec<String> = box_values(&shown).into_iter().map(|b| b.1).collect();
    assert_eq!(values.len(), 16);
    assert_ne!(values[..12], expected);
    let note = field_value(&shown, "#split-areas-note");
    assert!(note.contains("replaced"), "{note}");
    // A box that is not a number set is recomputed too.
    let mut broken = edited;
    broken[1][2] = "";
    let shown = body(
        areas_post(
            &app,
            &auth[0],
            1,
            split_form_fields("1", "1", THREE, "r", &broken),
        )
        .await,
    )
    .await;
    let values: Vec<String> = box_values(&shown).into_iter().map(|b| b.1).collect();
    assert_eq!(values[4], "12.00");
    assert!(field_value(&shown, "#split-areas-note").contains("replaced"));
}
