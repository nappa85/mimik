//! Slice 1.2 "Notes and scribbles" (assisted sheet plan §3a, §5 slice 1.2): change notes
//! written with the save, scribbles that keep a known value, no-op saves (D-U), history
//! with raw text (D-E), and the insert-only note table (D-T, migration 020).
use super::super::notes::{Note, NoteValue, Op};
use super::formula_tests::{Setup, body, setup, submitted};
use crate::AppError;
use axum::http::StatusCode;
use serde_json::{Value, json};

/// The demo definition (`assist-definition.json`), with a mapped source as publication needs.
pub(super) fn assist() -> Value {
    let mut definition = crate::definitions::source_fixture::assist_definition();
    definition["sources"] =
        json!([{"source_id": 1, "pdf_sha256": "0".repeat(64), "roles": ["sheet"]}]);
    definition["fields"][0]["mapping"] =
        json!({"state": "mapped", "regions": [{"source_id": 1, "page_id": 1, "unit_id": 1}]});
    definition
}

/// The 64-character enum option ID: its note value is the longest one admitted.
fn long_id() -> String {
    "a".repeat(64)
}

/// The assist definition plus an enum (`stance`, field 6) and a boolean (`inspired`, 7).
pub(super) fn choices() -> Value {
    let mut definition = assist();
    let unresolved = json!({"state": "unresolved", "reason": "Synthetic fixture"});
    let fields = definition["fields"].as_array_mut().unwrap();
    fields.push(
        json!({"id": "stance", "label": "Stance", "control": "enum", "nullable": false,
        "mapping": unresolved,
        "options": [{"id": "guard", "label": "Guard"}, {"id": long_id(), "label": "Long"}]}),
    );
    fields.push(
        json!({"id": "inspired", "label": "Inspired", "control": "boolean",
        "nullable": false, "mapping": unresolved}),
    );
    definition["groups"][0]["fields"]
        .as_array_mut()
        .unwrap()
        .extend([json!("stance"), json!("inspired")]);
    definition
}

pub(super) fn pair(key: &str, value: &str) -> (String, String) {
    (key.into(), value.into())
}

/// One `equipment` row (list field 4): host ID, name, quantity, weight, and the optional
/// `broken` (property 3) value and Set state.
#[derive(Clone, Copy)]
pub(super) struct Row<'a> {
    pub id: &'a str,
    pub name: &'a str,
    pub quantity: &'a str,
    pub broken: &'a str,
    pub set: &'a str,
}

pub(super) fn row<'a>(id: &'a str, quantity: &'a str) -> Row<'a> {
    Row {
        id,
        name: "Rope",
        quantity,
        broken: "",
        set: "absent",
    }
}

/// A full assisted-sheet form: HP 84, Max HP 84, Armour −1, Luck unknown, overridden (or
/// extended) by `changes`.
pub(super) fn form(
    revision: i64,
    changes: &[(&str, &str)],
    rows: &[Row<'_>],
) -> Vec<(String, String)> {
    let mut form = vec![
        pair("csrf", &"a".repeat(64)),
        pair("revision", &revision.to_string()),
        pair("name", "Assisted sheet"),
        pair("notes", ""),
        pair("value-hp", "84"),
        pair("value-max_hp", "84"),
        pair("value-armour", "-1"),
        pair("value-luck", ""),
        pair("null-luck", "unknown"),
    ];
    for (key, value) in changes {
        match form.iter_mut().find(|(k, _)| k == key) {
            Some(existing) => existing.1 = (*value).into(),
            None => form.push(pair(key, value)),
        }
    }
    for (r, row) in rows.iter().enumerate() {
        form.extend([
            pair(&format!("id-4-{r}"), row.id),
            pair(&format!("row-4-{r}-0"), row.name),
            pair(&format!("row-4-{r}-1"), row.quantity),
            pair(&format!("row-4-{r}-2"), "1"),
            pair(&format!("row-4-{r}-3"), row.broken),
            pair(&format!("set-4-{r}-3"), row.set),
        ]);
    }
    form
}

pub(super) fn create_path(setup: &Setup) -> String {
    format!(
        "/campaigns/{}/sheets/new?version={}",
        setup.campaign, setup.version
    )
}

/// Sheet 1 at revision 1 (one Rope row, quantity 1), created through the router.
pub(super) async fn created(setup: &Setup, extra: &[(&str, &str)]) {
    let create = form(0, extra, &[row("", "1")]);
    let (status, html) = setup.post(&create_path(setup), create).await;
    assert_eq!(status, StatusCode::SEE_OTHER, "create: {html}");
}

/// The saved revision and data of sheet `id`, read as the Master (account 2).
pub(super) async fn saved(setup: &Setup, id: i64) -> (i64, Value) {
    let (view, permit) = setup
        .store
        .private_read(
            setup.auth[1].clone(),
            id,
            None,
            crate::definitions::admit().unwrap(),
        )
        .await
        .unwrap();
    drop(permit);
    (view.snapshot.revision, view.snapshot.data)
}

pub(super) async fn history_len(setup: &Setup, id: i64) -> usize {
    setup
        .store
        .private_history(setup.auth[1].clone(), id, 0)
        .await
        .unwrap()
        .len()
}

pub(super) type NoteRow = (i64, String, String, String, Option<String>, String);

/// Every stored note of sheet `id`, as raw columns.
pub(super) async fn notes(setup: &Setup, id: i64) -> Vec<NoteRow> {
    setup
        .store
        .run(move |db| {
            let mut query = db.prepare("SELECT revision,address,op,raw,before,after FROM private_sheet_notes WHERE sheet_id=?1 ORDER BY revision,address")?;
            let rows = query
                .query_map([id], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
        .unwrap()
}

pub(super) async fn notes_at(setup: &Setup, id: i64, revision: i64) -> Vec<NoteRow> {
    notes(setup, id)
        .await
        .into_iter()
        .filter(|note| note.0 == revision)
        .collect()
}

pub(super) fn note(
    revision: i64,
    address: &str,
    op: &str,
    raw: &str,
    before: Option<&str>,
    after: &str,
) -> NoteRow {
    (
        revision,
        address.into(),
        op.into(),
        raw.into(),
        before.map(str::to_owned),
        after.into(),
    )
}

/// The inner HTML of the first element matching `selector`.
pub(super) fn inner(html: &str, selector: &str) -> Option<String> {
    let page = scraper::Html::parse_document(html);
    page.select(&scraper::Selector::parse(selector).unwrap())
        .next()
        .map(|element| element.inner_html())
}

/// The `value` of the control named `name`.
pub(super) fn value(html: &str, name: &str) -> Option<String> {
    let page = scraper::Html::parse_document(html);
    page.select(&scraper::Selector::parse(&format!("[name=\"{name}\"]")).unwrap())
        .next()
        .and_then(|element| element.value().attr("value").map(str::to_owned))
}

pub(super) async fn page(setup: &Setup) -> String {
    let (status, html) = setup.get("/private-sheets/1").await;
    assert_eq!(status, StatusCode::OK);
    html
}

#[tokio::test]
async fn sums_and_literals_are_noted_with_the_save_and_shown() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    created(&setup, &[]).await;
    // Create notes only interpreted numbers (sums); literals need no note.
    assert_eq!(notes(&setup, 1).await, vec![]);
    let edit = form(
        1,
        &[("value-hp", "84 -15"), ("value-armour", "-3")],
        &[row("r1-1", "2+1")],
    );
    assert_eq!(
        setup.post("/private-sheets/1", edit).await.0,
        StatusCode::SEE_OTHER
    );
    assert_eq!(saved(&setup, 1).await.1["hp"], json!(69));
    // Only the changed cells get notes; a sign-led input is applied literally and noted as
    // `sign_literal` (D-Q; slice 1.3 offers the "Did you mean" switch from it).
    assert_eq!(
        notes_at(&setup, 1, 2).await,
        vec![
            note(
                2,
                "cell:equipment/r1-1/quantity",
                "sum",
                "2+1",
                Some(r#"{"int":"1"}"#),
                r#"{"int":"3"}"#
            ),
            note(
                2,
                "field:armour",
                "sign_literal",
                "-3",
                Some(r#"{"int":"-1"}"#),
                r#"{"int":"-3"}"#
            ),
            note(
                2,
                "field:hp",
                "sum",
                "84 -15",
                Some(r#"{"int":"84"}"#),
                r#"{"int":"69"}"#
            ),
        ]
    );
    let html = page(&setup).await;
    assert_eq!(
        inner(&html, "#note-0").as_deref(),
        Some("<bdi>84 \u{2212} 15</bdi> = 69")
    );
    assert_eq!(
        inner(&html, "#note-4-0-1").as_deref(),
        Some("<bdi>2 + 1</bdi> = 3")
    );
    assert_eq!(value(&html, "value-hp").as_deref(), Some("69"));
    assert!(html.contains("aria-describedby=\"note-0\""), "{html}");
    // A literal shows nothing on the sheet; history lists it.
    assert_eq!(inner(&html, "#note-2"), None);
    assert_eq!(inner(&html, "#note-1"), None);
}

#[tokio::test]
async fn an_unreadable_entry_keeps_a_known_value_with_a_scribble() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    created(&setup, &[]).await;
    let (_, before) = saved(&setup, 1).await;
    let edit = form(1, &[("value-hp", "lots")], &[row("r1-1", "a few")]);
    assert_eq!(
        setup.post("/private-sheets/1", edit).await.0,
        StatusCode::SEE_OTHER
    );
    // A record-only revision: the data is unchanged, the scribbles are noted.
    assert_eq!(saved(&setup, 1).await, (2, before.clone()));
    assert_eq!(
        notes_at(&setup, 1, 2).await,
        vec![
            note(
                2,
                "cell:equipment/r1-1/quantity",
                "unresolved",
                "a few",
                Some(r#"{"int":"1"}"#),
                r#"{"int":"1"}"#
            ),
            note(
                2,
                "field:hp",
                "unresolved",
                "lots",
                Some(r#"{"int":"84"}"#),
                r#"{"int":"84"}"#
            ),
        ]
    );
    let html = page(&setup).await;
    assert_eq!(
        inner(&html, "#note-0").as_deref(),
        Some("kept 84 · ‘<bdi>lots</bdi>’ not understood")
    );
    assert_eq!(
        inner(&html, "#note-4-0-1").as_deref(),
        Some("kept 1 · ‘<bdi>a few</bdi>’ not understood")
    );
    // The cell shows what the player wrote, so re-posting the page changes nothing.
    assert_eq!(value(&html, "value-hp").as_deref(), Some("lots"));
    assert_eq!(value(&html, "row-4-0-1").as_deref(), Some("a few"));
    let (status, again) = setup.post("/private-sheets/1", submitted(&html)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(again.contains("No change"), "{again}");
    assert_eq!(saved(&setup, 1).await.0, 2);
    assert_eq!(notes(&setup, 1).await.len(), 2);
    // A different scribble is a new record-only revision; an out-of-range sum is one too.
    let edit = form(
        2,
        &[("value-hp", "99999999999999999999")],
        &[row("r1-1", "a few")],
    );
    assert_eq!(
        setup.post("/private-sheets/1", edit).await.0,
        StatusCode::SEE_OTHER
    );
    assert_eq!(saved(&setup, 1).await, (3, before));
    assert_eq!(
        notes_at(&setup, 1, 3).await,
        vec![note(
            3,
            "field:hp",
            "unresolved",
            "99999999999999999999",
            Some(r#"{"int":"84"}"#),
            r#"{"int":"84"}"#
        )]
    );
}

#[tokio::test]
async fn a_later_literal_replaces_a_scribble_as_the_current_note() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    created(&setup, &[]).await;
    let edit = form(1, &[("value-hp", "lots")], &[row("r1-1", "1")]);
    assert_eq!(
        setup.post("/private-sheets/1", edit).await.0,
        StatusCode::SEE_OTHER
    );
    let edit = form(2, &[("value-hp", "70")], &[row("r1-1", "1")]);
    assert_eq!(
        setup.post("/private-sheets/1", edit).await.0,
        StatusCode::SEE_OTHER
    );
    assert_eq!(saved(&setup, 1).await.1["hp"], json!(70));
    assert_eq!(
        notes_at(&setup, 1, 3).await,
        vec![note(
            3,
            "field:hp",
            "literal",
            "70",
            Some(r#"{"int":"84"}"#),
            r#"{"int":"70"}"#
        )]
    );
    let current = setup
        .store
        .run(|db| super::super::notes::current(db, 1, &["field:hp".into()]))
        .await
        .unwrap();
    assert_eq!(current["field:hp"].revision, 3);
    assert_eq!(current["field:hp"].note.op, Op::Literal);
    let html = page(&setup).await;
    assert_eq!(inner(&html, "#note-0"), None);
    assert_eq!(value(&html, "value-hp").as_deref(), Some("70"));
}

#[tokio::test]
async fn entering_the_kept_value_resolves_a_scribble() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    created(&setup, &[]).await;
    let (_, before) = saved(&setup, 1).await;
    let edit = form(1, &[("value-hp", "lots")], &[row("r1-1", "1")]);
    assert_eq!(
        setup.post("/private-sheets/1", edit).await.0,
        StatusCode::SEE_OTHER
    );
    // "84" equals the kept value: without the scribble this would be a no-op (D-U), but it
    // resolves the scribble, so a data-unchanged revision records a literal.
    let edit = form(2, &[("value-hp", "84")], &[row("r1-1", "1")]);
    assert_eq!(
        setup.post("/private-sheets/1", edit).await.0,
        StatusCode::SEE_OTHER
    );
    assert_eq!(saved(&setup, 1).await, (3, before.clone()));
    assert_eq!(
        notes_at(&setup, 1, 3).await,
        vec![note(
            3,
            "field:hp",
            "literal",
            "84",
            Some(r#"{"int":"84"}"#),
            r#"{"int":"84"}"#
        )]
    );
    let html = page(&setup).await;
    assert_eq!(inner(&html, "#note-0"), None);
    assert_eq!(value(&html, "value-hp").as_deref(), Some("84"));
    // Now entering 84 again is a no-op.
    let edit = form(3, &[("value-hp", "84")], &[row("r1-1", "1")]);
    let (status, html) = setup.post("/private-sheets/1", edit).await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("No change"), "{html}");
    assert_eq!(saved(&setup, 1).await, (3, before));
}

/// D-U with the per-control comparisons of plan §3a, and text edits that write no note.
#[tokio::test]
async fn a_save_that_changes_nothing_writes_nothing() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&choices()).await;
    let choice = [("value-stance", "guard"), ("value-inspired", "false")];
    created(&setup, &choice).await;
    let (_, before) = saved(&setup, 1).await;
    let notes_before = notes(&setup, 1).await;
    let no_change = |changes: Vec<(&'static str, &'static str)>, rows: Vec<Row<'static>>| {
        let mut all: Vec<(&str, &str)> = choice.to_vec();
        all.extend(changes);
        form(1, &all, &rows)
    };
    let rope = row("r1-1", "1");
    for (label, edit) in [
        ("identical", no_change(vec![], vec![rope])),
        (
            "zero padding 084",
            no_change(vec![("value-hp", "084")], vec![rope]),
        ),
        (
            "plus sign +84",
            no_change(vec![("value-hp", "+84")], vec![rope]),
        ),
        (
            "a sum to the same value",
            no_change(vec![("value-hp", "80+4")], vec![rope]),
        ),
        (
            "sign-led -1 over -1 (D-U wins)",
            no_change(vec![("value-armour", "-1")], vec![rope]),
        ),
        (
            "U+2212 minus",
            no_change(vec![("value-armour", "\u{2212}1")], vec![rope]),
        ),
        (
            "Name with spaces",
            no_change(vec![("name", "  Assisted sheet  ")], vec![rope]),
        ),
        (
            "enum option",
            no_change(vec![("value-stance", "guard")], vec![rope]),
        ),
        (
            "unknown ignores its text",
            no_change(vec![("value-luck", "5")], vec![rope]),
        ),
        (
            "Not set ignores its value",
            no_change(
                vec![],
                vec![Row {
                    broken: "true",
                    ..rope
                }],
            ),
        ),
        (
            "row quantity 01",
            no_change(vec![], vec![row("r1-1", "01")]),
        ),
    ] {
        let (status, html) = setup.post("/private-sheets/1", edit).await;
        assert_eq!(status, StatusCode::OK, "{label}");
        assert!(html.contains("No change"), "{label}: {html}");
        assert_eq!(saved(&setup, 1).await, (1, before.clone()), "{label}");
        assert_eq!(history_len(&setup, 1).await, 1, "{label}");
        assert_eq!(notes(&setup, 1).await, notes_before, "{label}");
    }
}

/// Text edits (Name, General notes, a row's text) write a revision but no note; present ↔
/// absent of an optional property is a change with typed values.
#[tokio::test]
async fn text_edits_write_no_note_and_presence_is_noted() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&choices()).await;
    let choice = [("value-stance", "guard"), ("value-inspired", "false")];
    created(&setup, &choice).await;
    let rope = row("r1-1", "1");
    let mut revision = 1;
    for changes in [
        vec![("name", "Renamed")],
        vec![("name", "Renamed"), ("notes", "General notes")],
    ] {
        let mut all: Vec<(&str, &str)> = choice.to_vec();
        all.extend(changes);
        let edit = form(revision, &all, &[rope]);
        assert_eq!(
            setup.post("/private-sheets/1", edit).await.0,
            StatusCode::SEE_OTHER
        );
        revision += 1;
        assert!(notes_at(&setup, 1, revision).await.is_empty());
    }
    let mut all: Vec<(&str, &str)> = choice.to_vec();
    all.extend([("name", "Renamed"), ("notes", "General notes")]);
    let edit = form(
        revision,
        &all,
        &[Row {
            name: "Old rope",
            ..rope
        }],
    );
    assert_eq!(
        setup.post("/private-sheets/1", edit).await.0,
        StatusCode::SEE_OTHER
    );
    revision += 1;
    assert_eq!(saved(&setup, 1).await.0, revision);
    assert!(notes_at(&setup, 1, revision).await.is_empty(), "item name");
    // Present ↔ absent of an optional property is a change with typed values.
    let present = Row {
        name: "Old rope",
        broken: "true",
        set: "present",
        ..rope
    };
    let edit = form(revision, &all, &[present]);
    assert_eq!(
        setup.post("/private-sheets/1", edit).await.0,
        StatusCode::SEE_OTHER
    );
    revision += 1;
    assert_eq!(
        notes_at(&setup, 1, revision).await,
        vec![note(
            revision,
            "cell:equipment/r1-1/broken",
            "literal",
            "true",
            Some(r#"{"absent":true}"#),
            r#"{"bool":true}"#
        )]
    );
    let absent = Row {
        name: "Old rope",
        broken: "true",
        set: "absent",
        ..rope
    };
    let edit = form(revision, &all, &[absent]);
    assert_eq!(
        setup.post("/private-sheets/1", edit).await.0,
        StatusCode::SEE_OTHER
    );
    revision += 1;
    assert_eq!(
        notes_at(&setup, 1, revision).await,
        vec![note(
            revision,
            "cell:equipment/r1-1/broken",
            "literal",
            "",
            Some(r#"{"bool":true}"#),
            r#"{"absent":true}"#
        )]
    );
}

/// Typed notes of enum, boolean and nullable cells read back exactly.
#[tokio::test]
async fn enum_boolean_and_unknown_notes_read_back_exactly() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&choices()).await;
    created(
        &setup,
        &[("value-stance", "guard"), ("value-inspired", "false")],
    )
    .await;
    let long = long_id();
    let edit = form(
        1,
        &[
            ("value-stance", &long),
            ("value-inspired", "true"),
            ("value-luck", "5"),
            ("null-luck", "known"),
        ],
        &[row("r1-1", "1")],
    );
    assert_eq!(
        setup.post("/private-sheets/1", edit).await.0,
        StatusCode::SEE_OTHER
    );
    let read = setup
        .store
        .run(|db| super::super::notes::of_revision(db, 1, 2))
        .await
        .unwrap();
    let typed = |address: &str, raw: &str, before: NoteValue, after: NoteValue| Note {
        address: address.into(),
        op: Op::Literal,
        raw: raw.into(),
        before: Some(before),
        after,
        target: None,
    };
    assert_eq!(
        read.into_iter()
            .map(|stored| (stored.revision, stored.note))
            .collect::<Vec<_>>(),
        vec![
            (
                2,
                typed(
                    "field:inspired",
                    "true",
                    NoteValue::Bool(false),
                    NoteValue::Bool(true)
                )
            ),
            (
                2,
                typed("field:luck", "5", NoteValue::Unknown, NoteValue::Int(5))
            ),
            (
                2,
                typed(
                    "field:stance",
                    &long,
                    NoteValue::Enum("guard".into()),
                    NoteValue::Enum(long.clone())
                )
            ),
        ]
    );
    let stored = notes_at(&setup, 1, 2).await;
    assert_eq!(stored[2].5, format!(r#"{{"enum":"{long}"}}"#));
    assert!(stored[2].5.len() <= 80);
    // Back to unknown: the before is the number, the after unknown (distinct from absent).
    let edit = form(
        2,
        &[
            ("value-stance", &long),
            ("value-inspired", "true"),
            ("value-luck", "5"),
        ],
        &[row("r1-1", "1")],
    );
    assert_eq!(
        setup.post("/private-sheets/1", edit).await.0,
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        notes_at(&setup, 1, 3).await,
        vec![note(
            3,
            "field:luck",
            "literal",
            "",
            Some(r#"{"int":"5"}"#),
            r#"{"unknown":true}"#
        )]
    );
}

/// A no-op save still rechecks rights inside a transaction: a caller who lost edit rights
/// gets 403, never "No change".
#[tokio::test]
async fn a_no_op_with_revoked_rights_is_403() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    let player = player_sheet(&setup).await;
    let unchanged = form(1, &[], &[row("r1-1", "1")]);
    let post = |form: Vec<(String, String)>| {
        super::router_request(&setup.app, &player, "/private-sheets/1", Some(form))
    };
    let response = post(unchanged.clone()).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(body(response).await.contains("No change"));
    let ran = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let seen = ran.clone();
    // The supervising Master is disabled after the page was loaded and checked.
    *super::super::sheets::BEFORE_UNCHANGED_RECHECK
        .lock()
        .unwrap() = Some(Box::new(move |db| {
        seen.store(true, std::sync::atomic::Ordering::SeqCst);
        db.execute(
            "UPDATE accounts SET epoch=2,state='disabled' WHERE id=2",
            [],
        )?;
        Ok(())
    }));
    let response = post(unchanged).await;
    assert!(
        ran.load(std::sync::atomic::Ordering::SeqCst),
        "the recheck ran"
    );
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(!body(response).await.contains("No change"));
    assert_eq!(history_len(&setup, 1).await, 1);
}

/// Slice-1.2 review row 2: a no-op still rechecks the submitting session inside the
/// transaction. The player's session ends after the page was loaded and checked: 403, never
/// "No change" (red when the session rechecks in `private_unchanged` are removed).
#[tokio::test]
async fn a_no_op_after_the_session_ended_is_403() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    let player = player_sheet(&setup).await;
    let unchanged = form(1, &[], &[row("r1-1", "1")]);
    let ran = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let seen = ran.clone();
    *super::super::sheets::BEFORE_UNCHANGED_RECHECK
        .lock()
        .unwrap() = Some(Box::new(move |db| {
        seen.store(true, std::sync::atomic::Ordering::SeqCst);
        let ended = db.execute(
            "UPDATE auth_sessions SET state='revoked',payload=NULL WHERE account_id=3",
            [],
        )?;
        if ended == 0 {
            return Err(AppError::Invalid("The player has no session to end."));
        }
        Ok(())
    }));
    let response =
        super::router_request(&setup.app, &player, "/private-sheets/1", Some(unchanged)).await;
    assert!(
        ran.load(std::sync::atomic::Ordering::SeqCst),
        "the recheck ran"
    );
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(!body(response).await.contains("No change"));
    assert_eq!(history_len(&setup, 1).await, 1);
}

/// Slice-1.2 review row 2: another save lands between the page check and the no-op's
/// recheck. The stale page gets 409 with its draft, never "No change" (red when the
/// revision check in `private_unchanged` is removed).
#[tokio::test]
async fn a_no_op_over_an_intervening_revision_is_409() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    created(&setup, &[]).await;
    let master = setup.auth[1].clone();
    let version = setup.version;
    *super::super::sheets::BEFORE_UNCHANGED_RECHECK
        .lock()
        .unwrap() = Some(Box::new(move |db| {
        let pin = crate::definitions::private_pin::validate(
            crate::definitions::private_pin::load(db, version)?,
        )?;
        let mut live = super::super::snapshots::load(db, 1)?;
        live.notes = "Saved in another tab".into();
        super::super::snapshots::write(db, &master, live, "edit", Some(&pin))?;
        Ok(())
    }));
    let unchanged = form(1, &[], &[row("r1-1", "1")]);
    let (status, html) = setup.post("/private-sheets/1", unchanged).await;
    assert!(
        super::super::sheets::BEFORE_UNCHANGED_RECHECK
            .lock()
            .unwrap()
            .is_none(),
        "the recheck ran"
    );
    assert_eq!(status, StatusCode::CONFLICT, "{html}");
    assert!(!html.contains("No change:"), "{html}");
    assert!(html.contains("No changes saved"), "{html}");
    assert_eq!(
        value(&html, "revision").as_deref(),
        Some("1"),
        "the draft is kept"
    );
    assert_eq!(history_len(&setup, 1).await, 1, "nothing written");
}

/// Account 3 joins the campaign as a player and creates sheet 1; returns its auth.
pub(super) async fn player_sheet(setup: &Setup) -> crate::auth::RequestAuth {
    setup
        .store
        .request_join(setup.auth[2].clone(), setup.campaign)
        .await
        .unwrap();
    setup
        .store
        .decide_join(setup.auth[1].clone(), setup.campaign, 3, 1, true)
        .await
        .unwrap();
    let player = setup.auth[2].clone();
    // A sum on create is noted (with no before value); literals are not.
    let response = super::router_request(
        &setup.app,
        &player,
        &create_path(setup),
        Some(form(0, &[("value-hp", "80+4")], &[row("", "1")])),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    player
}

/// D-E: the custodian and Masters see raw text and typed values in history.
#[tokio::test]
async fn masters_see_raw_text_in_history() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    let player = player_sheet(&setup).await;
    for (revision, hp) in [(1, "lots"), (2, "84 -15")] {
        let edit = form(revision, &[("value-hp", hp)], &[row("r1-1", "1")]);
        let response =
            super::router_request(&setup.app, &player, "/private-sheets/1", Some(edit)).await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
    }
    for reader in [&setup.auth[1], &player] {
        let response =
            super::router_request(&setup.app, reader, "/private-sheets/1/history/2", None).await;
        assert_eq!(response.status(), StatusCode::OK);
        let html = body(response).await;
        let notes = inner(&html, "#change-summary").unwrap();
        assert!(
            notes
                .contains("<code>field:hp</code> · unresolved · typed ‘<bdi>lots</bdi>’ · 84 → 84"),
            "{notes}"
        );
        let response =
            super::router_request(&setup.app, reader, "/private-sheets/1/history/3", None).await;
        let notes = inner(&body(response).await, "#change-summary").unwrap();
        assert!(
            notes.contains("<code>field:hp</code> · sum · typed ‘<bdi>84 -15</bdi>’ · 84 → 69"),
            "{notes}"
        );
        // Creation notes have no before value.
        let response =
            super::router_request(&setup.app, reader, "/private-sheets/1/history/1", None).await;
        let notes = inner(&body(response).await, "#change-summary").unwrap();
        assert!(
            notes.contains("<code>field:hp</code> · sum · typed ‘<bdi>80+4</bdi>’ · new → 84"),
            "{notes}"
        );
    }
    // A non-member reads nothing.
    let response = super::router_request(
        &setup.app,
        &setup.auth[3],
        "/private-sheets/1/history/2",
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn scribbles_render_escaped_inside_bdi() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    created(&setup, &[]).await;
    let scribble = "\u{5e9}\u{5dc}\u{5d5}\u{5e9} <b>x</b>";
    let edit = form(1, &[("value-hp", scribble)], &[row("r1-1", "1")]);
    assert_eq!(
        setup.post("/private-sheets/1", edit).await.0,
        StatusCode::SEE_OTHER
    );
    let (_, html) = setup.get("/private-sheets/1").await;
    assert_eq!(
        inner(&html, "#note-0").as_deref(),
        Some(
            "kept 84 · ‘<bdi>\u{5e9}\u{5dc}\u{5d5}\u{5e9} &lt;b&gt;x&lt;/b&gt;</bdi>’ not understood"
        )
    );
    let (_, history) = setup.get("/private-sheets/1/history/2").await;
    let history = inner(&history, "#change-summary").unwrap();
    assert!(
        history.contains("‘<bdi>\u{5e9}\u{5dc}\u{5d5}\u{5e9} &lt;b&gt;x&lt;/b&gt;</bdi>’"),
        "{history}"
    );
}

/// Control characters, over-long and blank input cannot be kept as a scribble; new rows and
/// create still refuse unreadable numbers until slice 1.4.
#[tokio::test]
async fn unkeepable_input_is_still_422() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    let create = form(0, &[("value-hp", "lots")], &[row("", "1")]);
    assert_eq!(
        setup.post(&create_path(&setup), create).await.0,
        StatusCode::UNPROCESSABLE_ENTITY,
        "create"
    );
    created(&setup, &[]).await;
    let (_, before) = saved(&setup, 1).await;
    let long = format!("1{}", " ".repeat(200));
    for (hp, rows) in [
        ("84\u{7}", vec![row("r1-1", "1")]),
        ("8\u{0}4", vec![row("r1-1", "1")]),
        (long.as_str(), vec![row("r1-1", "1")]),
        ("", vec![row("r1-1", "1")]),
        ("\t ", vec![row("r1-1", "1")]),
        ("84", vec![row("r1-1", "1"), row("", "lots")]),
    ] {
        let edit = form(1, &[("value-hp", hp)], &rows);
        let (status, _) = setup.post("/private-sheets/1", edit).await;
        assert!(
            matches!(
                status,
                StatusCode::UNPROCESSABLE_ENTITY | StatusCode::BAD_REQUEST
            ),
            "{hp:?}: {status}"
        );
        assert_eq!(saved(&setup, 1).await, (1, before.clone()), "{hp:?}");
        assert!(notes_at(&setup, 1, 2).await.is_empty(), "{hp:?}");
    }
}

#[tokio::test]
async fn freeze_writes_no_note_and_notes_stay_readable() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    let player = player_sheet(&setup).await;
    let edit = form(1, &[("value-hp", "84 -15")], &[row("r1-1", "1")]);
    let response =
        super::router_request(&setup.app, &player, "/private-sheets/1", Some(edit)).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let before = notes(&setup, 1).await;
    let campaign = setup.campaign;
    let member: i64 = setup
        .store
        .run(move |db| {
            Ok(db.query_row(
                "SELECT revision FROM campaign_members WHERE campaign_id=?1 AND account_id=3",
                [campaign],
                |row| row.get(0),
            )?)
        })
        .await
        .unwrap();
    setup
        .store
        .depart(setup.auth[1].clone(), setup.campaign, 3, member)
        .await
        .unwrap();
    assert_eq!(saved(&setup, 1).await.0, 3, "frozen at revision 3");
    assert_eq!(notes(&setup, 1).await, before, "freeze writes no note");
    let response = super::router_request(&setup.app, &player, "/private-sheets/1", None).await;
    assert_eq!(response.status(), StatusCode::OK);
    let html = body(response).await;
    assert_eq!(
        inner(&html, "#note-0").as_deref(),
        Some("<bdi>84 \u{2212} 15</bdi> = 69")
    );
    // The frozen revision is live but not writable: no note can name it.
    let refused = setup
        .store
        .run_campaign(setup.auth[1].clone(), |db| {
            db.execute("INSERT INTO private_sheet_notes(sheet_id,revision,address,op,raw,after) VALUES(1,3,'field:hp','literal','1','{\"int\":\"1\"}')", [])?;
            Ok(())
        })
        .await;
    assert!(
        format!("{refused:?}").contains("a note must name the live revision"),
        "{refused:?}"
    );
}

/// One integer field `a` and a list `rows` (field 1) of one integer property `n`.
fn many() -> Value {
    let unresolved = json!({"state": "unresolved", "reason": "Synthetic fixture"});
    json!({
        "contract_version": 1, "game_id": "synthetic-game", "edition_id": "first",
        "definition_id": "many-notes", "title": "Synthetic note limit",
        "sources": [{"source_id": 1, "pdf_sha256": "0".repeat(64), "roles": ["sheet"]}],
        "fields": [
            {"id": "a", "label": "A", "control": "integer", "nullable": false,
             "mapping": {"state": "mapped", "regions": [{"source_id": 1, "page_id": 1, "unit_id": 1}]}},
            {"id": "rows", "label": "Rows", "control": "list", "nullable": false,
             "mapping": unresolved,
             "items": {"properties": [{"id": "n", "label": "N", "control": "integer", "optional": false}]}},
        ],
        "groups": [{"id": "main", "label": "Main", "fields": ["a", "rows"], "column_hint": "main"}],
        "formula_profile": 1
    })
}

/// 256 notes in one revision are stored; 257 are a 422 size limit and nothing is saved.
#[tokio::test]
async fn more_than_256_notes_are_refused() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&many()).await;
    let create = |rows: usize| {
        let mut form = vec![
            pair("csrf", &"a".repeat(64)),
            pair("revision", "0"),
            pair("name", "Many"),
            pair("notes", ""),
            pair("value-a", "0+1"),
        ];
        // Sums: interpreted numbers are noted on create.
        for r in 0..rows {
            form.push(pair(&format!("id-1-{r}"), ""));
            form.push(pair(&format!("row-1-{r}-0"), "1+0"));
        }
        form
    };
    let (status, html) = setup.post(&create_path(&setup), create(256)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(html.contains("size limit"), "{html}");
    let sheets = setup
        .store
        .run(|db| {
            Ok(
                db.query_row("SELECT count(*) FROM private_sheets", [], |r| {
                    r.get::<_, i64>(0)
                })?,
            )
        })
        .await
        .unwrap();
    assert_eq!(sheets, 0, "nothing saved");
    assert_eq!(
        setup.post(&create_path(&setup), create(255)).await.0,
        StatusCode::SEE_OTHER
    );
    assert_eq!(notes_at(&setup, 1, 1).await.len(), 256);
}

/// The notes join the revision's transaction: a failure after `snapshots::write` rolls back
/// the whole save, and so does a note the table refuses.
#[tokio::test]
async fn a_failure_after_the_snapshot_write_rolls_back_everything() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    created(&setup, &[]).await;
    let (_, before) = saved(&setup, 1).await;
    let edit = || form(1, &[("value-hp", "84 -15")], &[row("r1-1", "1")]);
    let failures: Vec<super::super::notes::InsertHook> = vec![
        Box::new(|_| {
            Err(AppError::Invalid(
                "Synthetic failure after the snapshot write.",
            ))
        }),
        Box::new(|notes| {
            notes[0].address = "bogus".into();
            Ok(())
        }),
        Box::new(|notes| {
            let copy = notes[0].clone();
            notes.push(copy);
            Ok(())
        }),
    ];
    for (index, hook) in failures.into_iter().enumerate() {
        *super::super::notes::BEFORE_INSERT.lock().unwrap() = Some(hook);
        let (status, _) = setup.post("/private-sheets/1", edit()).await;
        assert!(
            super::super::notes::BEFORE_INSERT.lock().unwrap().is_none(),
            "hook {index} ran"
        );
        assert_ne!(status, StatusCode::SEE_OTHER, "hook {index}");
        assert_eq!(saved(&setup, 1).await, (1, before.clone()), "hook {index}");
        assert_eq!(history_len(&setup, 1).await, 1, "hook {index}");
        assert!(notes_at(&setup, 1, 2).await.is_empty(), "hook {index}");
    }
    assert_eq!(
        setup.post("/private-sheets/1", edit()).await.0,
        StatusCode::SEE_OTHER
    );
    assert_eq!(notes_at(&setup, 1, 2).await.len(), 1);
}

/// D-T: the note table is insert-only; inserts must name the live revision under the
/// revision actor's request auth.
#[tokio::test]
async fn notes_are_insert_only() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    created(&setup, &[]).await;
    let edit = form(1, &[("value-hp", "84 -15")], &[row("r1-1", "1")]);
    assert_eq!(
        setup.post("/private-sheets/1", edit).await.0,
        StatusCode::SEE_OTHER
    );
    let before = notes(&setup, 1).await;
    let insert = |revision: i64, address: &str| {
        format!(
            "INSERT INTO private_sheet_notes(sheet_id,revision,address,op,raw,after) VALUES(1,{revision},'{address}','literal','1','{{\"int\":\"1\"}}')"
        )
    };
    for (sql, refusal) in [
        (
            "UPDATE private_sheet_notes SET raw='70' WHERE revision=2".to_owned(),
            "notes are insert-only",
        ),
        (
            "UPDATE private_sheet_notes SET revision=3".to_owned(),
            "notes are insert-only",
        ),
        (
            "DELETE FROM private_sheet_notes WHERE revision=2".to_owned(),
            "notes are permanent",
        ),
        (
            "DELETE FROM private_sheet_notes".to_owned(),
            "notes are permanent",
        ),
        (
            insert(2, "field:hp").replacen("INSERT", "INSERT OR REPLACE", 1),
            "notes are insert-only",
        ),
        (
            insert(2, "field:hp").replacen("INSERT", "REPLACE", 1),
            "notes are insert-only",
        ),
        (
            format!(
                "{} ON CONFLICT DO UPDATE SET raw='x'",
                insert(2, "field:hp")
            ),
            "notes are insert-only",
        ),
        (
            format!("{} ON CONFLICT DO NOTHING", insert(2, "field:hp")),
            "notes are insert-only",
        ),
        // Revision 1 has no `field:luck` note (Luck was unknown at creation).
        (
            insert(1, "field:luck"),
            "a note must name the live revision",
        ),
        (
            insert(3, "field:luck"),
            "a note must name the live revision",
        ),
        (
            insert(2, "field:max_hp").replace("VALUES(1,", "VALUES(9,"),
            "a note must name the live revision",
        ),
    ] {
        for authenticated in [false, true] {
            let statement = sql.clone();
            let operation = move |db: &mut rusqlite::Connection| {
                db.execute_batch(&statement)?;
                Ok(())
            };
            let result = if authenticated {
                setup
                    .store
                    .run_campaign(setup.auth[1].clone(), operation)
                    .await
            } else {
                setup.store.run(operation).await
            };
            assert!(format!("{result:?}").contains(refusal), "{sql}: {result:?}");
        }
    }
    assert_eq!(notes(&setup, 1).await, before, "nothing changed");
}

/// Inserts need the revision actor's request auth; with it, known limitation L1 remains.
#[tokio::test]
async fn note_inserts_need_the_revision_actor() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    created(&setup, &[]).await;
    let edit = form(1, &[("value-hp", "84 -15")], &[row("r1-1", "1")]);
    assert_eq!(
        setup.post("/private-sheets/1", edit).await.0,
        StatusCode::SEE_OTHER
    );
    let before = notes(&setup, 1).await;
    let insert = |revision: i64, address: &str| {
        format!(
            "INSERT INTO private_sheet_notes(sheet_id,revision,address,op,raw,after) VALUES(1,{revision},'{address}','literal','1','{{\"int\":\"1\"}}')"
        )
    };
    // Without the revision actor's request auth no note is added.
    let unauthenticated = setup.store.run(move |db| {
        db.execute_batch(&insert(2, "field:max_hp"))?;
        Ok(())
    });
    let result = unauthenticated.await;
    assert!(
        format!("{result:?}").contains("note auth required"),
        "{result:?}"
    );
    let other = setup
        .store
        .run_campaign(setup.auth[2].clone(), move |db| {
            db.execute_batch(&insert(2, "field:max_hp"))?;
            Ok(())
        })
        .await;
    assert!(
        format!("{other:?}").contains("note auth required"),
        "{other:?}"
    );
    assert_eq!(notes(&setup, 1).await, before, "nothing changed");
    // Known limitation L1 (plan §6, D-T): with the actor's request auth a note can still be
    // added to the live revision. Sealing closes this before release.
    let added = setup
        .store
        .run_campaign(setup.auth[1].clone(), move |db| {
            db.execute_batch(&insert(2, "field:max_hp"))?;
            Ok(())
        })
        .await;
    assert!(added.is_ok(), "{added:?}");
}

/// Migration 020 on a populated schema-19 database: sheets and history are kept, the note
/// table starts empty with its guards, and the next save writes notes.
#[tokio::test]
async fn populated_schema19_upgrade_adds_the_note_table() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&assist()).await;
    created(&setup, &[]).await;
    let edit = form(1, &[("value-hp", "84 -15")], &[row("r1-1", "1")]);
    assert_eq!(
        setup.post("/private-sheets/1", edit).await.0,
        StatusCode::SEE_OTHER
    );
    let rows = |db: &rusqlite::Connection| -> Result<Vec<(i64, String, String)>, AppError> {
        let mut query = db.prepare(
            "SELECT revision,hash,snapshot FROM private_sheet_history ORDER BY sheet_id,revision",
        )?;
        let rows = query
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    };
    let history = setup
        .store
        .run(move |db| {
            let history = rows(db)?;
            // Rewind to a genuine schema-19 shape.
            db.execute_batch(crate::campaigns::DROP_NOTES_SCHEMA)?;
            db.execute_batch("PRAGMA user_version=19;")?;
            Ok(history)
        })
        .await
        .unwrap();
    assert_eq!(history.len(), 2);
    let reopened = crate::Store::open(&setup.dir.path().join("private.sqlite3")).unwrap();
    // The fixture sessions live on the fixture clock.
    reopened
        .run(|db| {
            crate::store::register_auth_clock(db, std::sync::Arc::new(|| Ok(100)))?;
            Ok(())
        })
        .await
        .unwrap();
    let (version, objects, upgraded, count) = reopened
        .run(move |db| {
            Ok((
                db.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))?,
                db.query_row("SELECT count(*) FROM sqlite_master WHERE tbl_name='private_sheet_notes' AND type IN ('trigger','index')", [], |r| r.get::<_, i64>(0))?,
                rows(db)?,
                db.query_row("SELECT count(*) FROM private_sheet_notes", [], |r| r.get::<_, i64>(0))?,
            ))
        })
        .await
        .unwrap();
    assert_eq!(version, crate::SCHEMA_VERSION);
    assert_eq!(version, 20);
    assert_eq!(upgraded, history, "history kept byte for byte");
    assert_eq!(count, 0, "existing revisions have no notes");
    // Three triggers, the current-note index and the primary key's autoindex.
    assert_eq!(objects, 5);
    let app = crate::auth::app(reopened.clone()).await.unwrap();
    let edit = form(2, &[("value-hp", "69 + 1")], &[row("r1-1", "1")]);
    let response =
        super::router_request(&app, &setup.auth[1], "/private-sheets/1", Some(edit)).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let written = reopened
        .run(|db| super::super::notes::of_revision(db, 1, 3))
        .await
        .unwrap();
    assert_eq!(written.len(), 1);
    assert_eq!(written[0].note.op, Op::Sum);
    assert_eq!(written[0].note.after, NoteValue::Int(70));
}
