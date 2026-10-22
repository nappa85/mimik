use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use mimik::{CharacterInput, Store, app, mork_borg::EDITION};
use tower::ServiceExt;

fn identity() -> CharacterInput {
    CharacterInput {
        name: "Original".into(),
        system_id: "mork-borg".into(),
        edition: EDITION.into(),
        notes: "separate notes".into(),
        revision: 0,
    }
}

fn fields(revision: i64) -> String {
    format!(
        "revision={revision}&agility=0&presence=0&strength=0&toughness=0&hp=3&max_hp=5&worn_armor=0&protection=0&standard_items=0&silver=10&omens=1&equipment=gear"
    )
}

async fn save(router: axum::Router, id: i64, body: String, htmx: bool) -> (StatusCode, String) {
    let response = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/characters/{id}/mork-borg"))
                .header("host", "localhost:3000")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("hx-request", if htmx { "true" } else { "false" })
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    (
        status,
        String::from_utf8(
            to_bytes(response.into_body(), 200_000)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap(),
    )
}

#[tokio::test]
async fn malformed_numbers_name_and_miseries_preserve_every_draft_without_mutation() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("draft.sqlite3")).unwrap();
    let created = store.create(identity()).await.unwrap();
    let router = app(store.clone());
    for (body, expected) in [
        (
            fields(1).replace("hp=3", "hp=not-a-number")
                + "&name=Draft&description=long+description&powers=power+notes&campaign_miseries=",
            "value=\"not-a-number\"",
        ),
        (
            fields(1)
                + "&name=Draft&description=long+description&powers=power+notes&campaign_miseries=bad",
            "value=\"bad\"",
        ),
        (
            fields(1)
                + "&name=&description=long+description&powers=power+notes&campaign_miseries=0",
            "Character name",
        ),
    ] {
        for htmx in [false, true] {
            let (status, html) = save(router.clone(), created.id, body.clone(), htmx).await;
            assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
            assert!(html.contains(expected));
            assert!(html.contains("long description"));
            assert!(html.contains("power notes"));
            assert!(html.contains("name=\"revision\" value=\"1\""));
            assert_eq!(store.get(created.id).await.unwrap().input.name, "Original");
            assert_eq!(store.get(created.id).await.unwrap().input.revision, 1);
        }
    }
}

#[tokio::test]
async fn new_fields_name_and_unknown_miseries_persist_and_old_posts_do_not_erase_them() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("persist.sqlite3");
    let store = Store::open(&path).unwrap();
    let created = store.create(identity()).await.unwrap();
    let router = app(store.clone());
    let body = fields(1)
        + "&name=Named+sheet&description=description&class_notes=class&powers=powers&weapon_one=sword&weapon_two=bow&campaign_miseries=0";
    assert_eq!(
        save(router.clone(), created.id, body.clone(), true).await.0,
        StatusCode::OK
    );
    let (_, sheet, _) = store.mork_borg_sheet(created.id).await.unwrap();
    assert_eq!(sheet.campaign_miseries, Some(0));
    assert_eq!(sheet.description, "description");
    assert_eq!(
        store.get(created.id).await.unwrap().input.notes,
        "separate notes"
    );
    assert_eq!(
        save(router.clone(), created.id, fields(2), true).await.0,
        StatusCode::OK
    );
    let (character, sheet, _) = store.mork_borg_sheet(created.id).await.unwrap();
    assert_eq!(character.input.name, "Named sheet");
    assert_eq!(sheet.powers, "powers");
    assert_eq!(sheet.weapon_two, "bow");
    let (status, html) = save(router.clone(), created.id, body, true).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(html.contains("name=\"revision\" value=\"1\""));
    assert!(html.contains("Named sheet"));
    assert_eq!(store.get(created.id).await.unwrap().input.revision, 3);
    assert_eq!(
        save(
            router,
            created.id,
            fields(3) + "&campaign_miseries=&description=",
            true
        )
        .await
        .0,
        StatusCode::OK
    );
    drop(store);
    let reopened = Store::open(&path).unwrap();
    let (_, sheet, _) = reopened.mork_borg_sheet(created.id).await.unwrap();
    assert_eq!(sheet.campaign_miseries, None);
    assert_eq!(sheet.description, "");
    assert_eq!(sheet.class_notes, "class");
    assert_eq!(sheet.weapon_one, "sword");
}

#[tokio::test]
async fn actual_old_json_loads_with_defaults_and_round_trips_without_losing_stats() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("legacy.sqlite3");
    let store = Store::open(&path).unwrap();
    let created = store.create(identity()).await.unwrap();
    let (_, mut sheet, _) = store.mork_borg_sheet(created.id).await.unwrap();
    sheet.hp = 1;
    store.save_mork_borg_sheet(created.id, sheet).await.unwrap();
    drop(store);
    let db = rusqlite::Connection::open(&path).unwrap();
    let text: String = db
        .query_row("SELECT data FROM mork_borg_sheets", [], |row| row.get(0))
        .unwrap();
    let mut old: serde_json::Value = serde_json::from_str(&text).unwrap();
    for name in [
        "description",
        "class_notes",
        "powers",
        "weapon_one",
        "weapon_two",
        "campaign_miseries",
    ] {
        old.as_object_mut().unwrap().remove(name);
    }
    db.execute(
        "UPDATE mork_borg_sheets SET data=?1",
        [serde_json::to_string(&old).unwrap()],
    )
    .unwrap();
    drop(db);
    let store = Store::open(&path).unwrap();
    let (_, sheet, saved) = store.mork_borg_sheet(created.id).await.unwrap();
    assert!(saved);
    assert_eq!(sheet.description, "");
    assert_eq!(sheet.campaign_miseries, None);
    assert_eq!(sheet.hp, 1);
    store.save_mork_borg_sheet(created.id, sheet).await.unwrap();
}

#[tokio::test]
async fn transport_rejections_retarget_only_status_and_cannot_destroy_the_editor() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("transport.sqlite3")).unwrap();
    let created = store.create(identity()).await.unwrap();
    let router = app(store.clone());
    for extra in ["&unexpected_field=1", "&hp=4"] {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/characters/{}/mork-borg", created.id))
                    .header("host", "localhost:3000")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .header("hx-request", "true")
                    .body(Body::from(fields(1) + extra))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            response.headers().get("hx-retarget").unwrap(),
            "#mork-message"
        );
        assert_eq!(response.headers().get("hx-reswap").unwrap(), "innerHTML");
        assert_eq!(store.get(created.id).await.unwrap().input.revision, 1);
    }
}

#[tokio::test]
async fn origin_and_content_type_rejections_preserve_the_complete_saved_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("origin.sqlite3")).unwrap();
    let created = store.create(identity()).await.unwrap();
    let before = store.mork_borg_sheet(created.id).await.unwrap();
    let router = app(store.clone());
    for (header, value, content_type, status) in [
        (
            "origin",
            "https://example.com",
            "application/x-www-form-urlencoded",
            StatusCode::FORBIDDEN,
        ),
        (
            "sec-fetch-site",
            "cross-site",
            "application/x-www-form-urlencoded",
            StatusCode::FORBIDDEN,
        ),
        (
            "origin",
            "http://localhost:3000",
            "application/json",
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
        ),
    ] {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/characters/{}/mork-borg", created.id))
                    .header("host", "localhost:3000")
                    .header("hx-request", "true")
                    .header("content-type", content_type)
                    .header(header, value)
                    .body(Body::from(fields(1) + "&name=Rejected&weapon_one=Rejected"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), status);
        assert_eq!(
            response.headers().get("hx-retarget").unwrap(),
            "#mork-message"
        );
        assert_eq!(response.headers().get("hx-reswap").unwrap(), "innerHTML");
        let after = store.mork_borg_sheet(created.id).await.unwrap();
        assert_eq!(after.0.input.name, before.0.input.name);
        assert_eq!(after.0.input.notes, before.0.input.notes);
        assert_eq!(after.0.input.revision, before.0.input.revision);
        assert_eq!(
            serde_json::to_value(after.1).unwrap(),
            serde_json::to_value(&before.1).unwrap()
        );
        assert_eq!(after.2, before.2);
    }
}

#[tokio::test]
async fn ordinary_transport_rejections_do_not_promise_an_undelivered_draft() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("ordinary.sqlite3")).unwrap();
    let created = store.create(identity()).await.unwrap();
    let (_, mut sheet, _) = store.mork_borg_sheet(created.id).await.unwrap();
    sheet.weapon_one = "Saved weapon".into();
    store.save_mork_borg_sheet(created.id, sheet).await.unwrap();
    let before = store.mork_borg_sheet(created.id).await.unwrap();
    let router = app(store.clone());
    for htmx in [None, Some("false")] {
        for (header, value, content_type, extra, status) in [
            (
                "origin",
                "https://example.com",
                "application/x-www-form-urlencoded",
                "",
                StatusCode::FORBIDDEN,
            ),
            (
                "sec-fetch-site",
                "cross-site",
                "application/x-www-form-urlencoded",
                "",
                StatusCode::FORBIDDEN,
            ),
            (
                "origin",
                "http://localhost:3000",
                "application/json",
                "",
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
            ),
            (
                "origin",
                "http://localhost:3000",
                "application/x-www-form-urlencoded",
                "&unexpected=1",
                StatusCode::UNPROCESSABLE_ENTITY,
            ),
        ] {
            let mut request = Request::builder()
                .method("POST")
                .uri(format!("/characters/{}/mork-borg", created.id))
                .header("host", "localhost:3000")
                .header("content-type", content_type)
                .header(header, value);
            if let Some(value) = htmx {
                request = request.header("hx-request", value);
            }
            let response = router
                .clone()
                .oneshot(request.body(Body::from(fields(2) + extra)).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), status);
            assert_eq!(
                response.headers().get("hx-retarget").unwrap(),
                "#mork-message"
            );
            assert_eq!(response.headers().get("hx-reswap").unwrap(), "innerHTML");
            let text = String::from_utf8(
                to_bytes(response.into_body(), 200_000)
                    .await
                    .unwrap()
                    .to_vec(),
            )
            .unwrap();
            assert_ne!(text, "");
            assert!(!text.contains("displayed draft was kept"));
            let after = store.mork_borg_sheet(created.id).await.unwrap();
            assert_eq!(after.0.input.name, before.0.input.name);
            assert_eq!(after.0.input.notes, before.0.input.notes);
            assert_eq!(after.0.input.revision, before.0.input.revision);
            assert_eq!(
                serde_json::to_value(after.1).unwrap(),
                serde_json::to_value(&before.1).unwrap()
            );
            assert_eq!(after.2, before.2);
        }
    }
}
