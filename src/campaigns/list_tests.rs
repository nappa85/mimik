//! M4: list fields on private runtime sheets (writer IDs, list save/render, draft ops,
//! history and its change summary, large forms).
use super::formula_tests::{Setup, setup};
use super::snapshots::{self, PrivateSnapshot};
use crate::{AppError, auth::RequestAuth};
use rusqlite::params;
use serde_json::{Value, json};

fn mapped() -> Value {
    json!({"state": "mapped", "regions": [{"source_id": 1, "page_id": 1, "unit_id": 1}]})
}

fn property(id: &str, label: &str, control: &str, optional: bool) -> Value {
    json!({"id": id, "label": label, "control": control, "optional": optional})
}

fn list(id: &str, label: &str, properties: Vec<Value>) -> Value {
    json!({"id": id, "label": label, "control": "list", "nullable": false, "mapping": mapped(),
           "items": {"properties": Value::Array(properties)}})
}

/// A formula-free definition with a scalar and two lists: `equipment` (text, integer,
/// optional integer, optional enum, boolean) and `tags` (one text property).
pub(super) fn list_definition() -> Value {
    let mut kind = property("kind", "Kind", "enum", true);
    kind["options"] = json!([{"id": "light", "label": "Light"}, {"id": "heavy", "label": "Heavy"}]);
    let fields = vec![
        json!({"id": "label", "label": "Label", "control": "text", "nullable": false,
               "mapping": mapped()}),
        list(
            "equipment",
            "Equipment",
            vec![
                property("name", "Name", "text", false),
                property("quantity", "Quantity", "integer", false),
                property("armor", "Armor", "integer", true),
                kind,
                property("equipped", "Equipped", "boolean", false),
            ],
        ),
        list("tags", "Tags", vec![property("tag", "Tag", "text", false)]),
    ];
    json!({
        "contract_version": 1, "game_id": "synthetic-game", "edition_id": "first",
        "definition_id": "list-sheet", "title": "List sheet",
        "sources": [{"source_id": 1, "pdf_sha256": "0".repeat(64), "roles": ["sheet"]}],
        "fields": fields, "formula_profile": 1,
        "groups": [{"id": "main", "label": "Main", "fields": ["label", "equipment", "tags"],
                    "column_hint": "main"}],
    })
}

/// `lists` lists of 32 required integer properties each.
fn integer_lists_definition(lists: usize) -> Value {
    let fields: Vec<Value> = (0..lists)
        .map(|l| {
            list(
                &format!("l{l}"),
                &format!("List {l}"),
                (0..32)
                    .map(|p| property(&format!("p{p}"), &format!("P{p}"), "integer", false))
                    .collect(),
            )
        })
        .collect();
    let ids: Vec<Value> = fields.iter().map(|field| field["id"].clone()).collect();
    json!({
        "contract_version": 1, "game_id": "synthetic-game", "edition_id": "first",
        "definition_id": "integer-lists", "title": "Integer lists",
        "sources": [{"source_id": 1, "pdf_sha256": "0".repeat(64), "roles": ["sheet"]}],
        "fields": fields, "formula_profile": 1,
        "groups": [{"id": "main", "label": "Main", "fields": ids, "column_hint": "main"}],
    })
}

/// Lists are unpublishable until the M4 gate lift; until then these tests publish on their
/// own test thread through the test-only path.
async fn list_setup(definition: &Value) -> Setup {
    crate::definitions::publish_unshipped_for_tests();
    setup(definition).await
}

fn item(id: &str, name: &str) -> Value {
    json!({"id": id, "name": name, "quantity": 1, "equipped": false})
}

fn tag(id: &str) -> Value {
    json!({"id": id, "tag": "t"})
}

fn data(equipment: Vec<Value>, tags: Vec<Value>) -> Value {
    json!({"label": "x", "equipment": Value::Array(equipment), "tags": Value::Array(tags)})
}

/// An unsaved runtime snapshot of the published version for `auth`'s campaign.
async fn blank(setup: &Setup, auth: usize) -> PrivateSnapshot {
    let (view, permit) = setup
        .store
        .private_create_context(
            setup.auth[auth].clone(),
            setup.campaign,
            setup.version,
            crate::definitions::admit().unwrap(),
        )
        .await
        .unwrap();
    drop(permit);
    let mut snapshot = view.snapshot;
    snapshot.name = "List sheet".into();
    snapshot
}

/// Runs the sole writer directly with the admitted pin, committing only on success.
async fn write(
    setup: &Setup,
    auth: usize,
    snapshot: PrivateSnapshot,
    operation: &'static str,
) -> Result<PrivateSnapshot, AppError> {
    let auth: RequestAuth = setup.auth[auth].clone();
    let version = setup.version;
    setup
        .store
        .run_campaign(auth.clone(), move |db| {
            let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let pin = crate::definitions::private_pin::validate(
                crate::definitions::private_pin::load(&tx, version)?,
            )?;
            let result = snapshots::write(&tx, &auth, snapshot, operation, Some(&pin));
            if result.is_ok() {
                tx.commit()?;
            }
            Ok(result)
        })
        .await
        .unwrap()
}

async fn create(setup: &Setup, values: Value) -> Result<PrivateSnapshot, AppError> {
    let mut snapshot = blank(setup, 1).await;
    snapshot.sheet_id = 1;
    snapshot.data = values;
    write(setup, 1, snapshot, "create").await
}

async fn edit(
    setup: &Setup,
    previous: &PrivateSnapshot,
    values: Value,
) -> Result<PrivateSnapshot, AppError> {
    let mut snapshot = previous.clone();
    snapshot.data = values;
    write(setup, 1, snapshot, "edit").await
}

async fn sheet_count(setup: &Setup) -> i64 {
    setup
        .store
        .run(|db| Ok(db.query_row("SELECT count(*) FROM private_sheets", [], |r| r.get(0))?))
        .await
        .unwrap()
}

fn invalid(result: Result<PrivateSnapshot, AppError>, context: &str) {
    assert!(
        matches!(result, Err(AppError::Invalid(_))),
        "{context}: want Invalid, got {:?}",
        result.map(|s| s.revision)
    );
}

#[tokio::test]
async fn writer_refuses_client_chosen_new_ids() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = list_setup(&list_definition()).await;
    // Paired positive baseline on the same fixture: host IDs are accepted.
    let created = create(
        &setup,
        data(vec![item("r1-1", "Rope"), item("r1-2", "Torch")], vec![]),
    )
    .await
    .unwrap();
    assert_eq!(created.revision, 1);
    // A second sheet cannot choose its own IDs on create.
    let mut snapshot = blank(&setup, 1).await;
    snapshot.sheet_id = 2;
    snapshot.data = data(vec![item("sword", "Sword")], vec![]);
    invalid(write(&setup, 1, snapshot, "create").await, "create");
    // Nor can an edit add a row with a client-chosen ID.
    invalid(
        edit(
            &setup,
            &created,
            data(
                vec![
                    item("r1-1", "Rope"),
                    item("r1-2", "Torch"),
                    item("dagger", "D"),
                ],
                vec![],
            ),
        )
        .await,
        "edit",
    );
    assert_eq!(sheet_count(&setup).await, 1);
}

#[tokio::test]
async fn writer_refuses_removed_id_reintroduced_and_wrong_or_noncontiguous_k() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = list_setup(&list_definition()).await;
    let first = create(
        &setup,
        data(vec![item("r1-1", "Rope"), item("r1-2", "Torch")], vec![]),
    )
    .await
    .unwrap();
    let second = edit(&setup, &first, data(vec![item("r1-1", "Rope")], vec![]))
        .await
        .unwrap();
    assert_eq!(second.revision, 2);
    invalid(
        edit(
            &setup,
            &second,
            data(vec![item("r1-1", "Rope"), item("r1-2", "Torch")], vec![]),
        )
        .await,
        "removed ID reintroduced",
    );
    for (ids, context) in [
        (vec!["r1-1", "r3-2"], "k starts at 2"),
        (vec!["r1-1", "r3-2", "r3-1"], "k out of row order"),
        (vec!["r1-1", "r2-1"], "pre-increment revision"),
        (vec!["r1-1", "r3-1", "r3-3"], "k gap"),
    ] {
        let equipment = ids.iter().map(|id| item(id, "N")).collect();
        invalid(
            edit(&setup, &second, data(equipment, vec![])).await,
            context,
        );
    }
    // Paired positive: the right k at the post-increment revision is accepted.
    let third = edit(
        &setup,
        &second,
        data(
            vec![item("r1-1", "Rope"), item("r3-1", "N"), item("r3-2", "N")],
            vec![],
        ),
    )
    .await
    .unwrap();
    assert_eq!(third.revision, 3);
}

#[tokio::test]
async fn writer_accepts_mixed_retained_new_rows_per_list_reuse_and_swaps() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = list_setup(&list_definition()).await;
    let first = create(
        &setup,
        data(
            vec![item("r1-1", "A"), item("r1-2", "B"), item("r1-3", "C")],
            vec![tag("r1-1")],
        ),
    )
    .await
    .unwrap();
    // Retained rows reordered, new rows between them, k per list, a reused slot (every
    // property of r1-1 overwritten) and swapped hidden IDs between rows are all accepted.
    let mut reused = item("r1-1", "Completely different");
    reused["quantity"] = 9.into();
    reused["armor"] = 3.into();
    let second = edit(
        &setup,
        &first,
        data(
            vec![
                item("r1-3", "A"),
                item("r2-1", "New"),
                reused,
                item("r2-2", "Newer"),
                item("r1-2", "C"),
            ],
            vec![tag("r2-1"), tag("r1-1")],
        ),
    )
    .await
    .unwrap();
    assert_eq!(second.revision, 2);
    let ids: Vec<&str> = second.data["equipment"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["r1-3", "r2-1", "r1-1", "r2-2", "r1-2"]);
}

/// Inserts a create row with arbitrary (pre-M4, non-host) item IDs, as the writer's own SQL
/// does, bypassing only the host-ID invariant.
async fn legacy_create(setup: &Setup, auth: usize, values: Value) {
    let mut snapshot = blank(setup, auth).await;
    snapshot.sheet_id = 1;
    snapshot.revision = 1;
    snapshot.data = values;
    let auth = setup.auth[auth].clone();
    setup
        .store
        .run_campaign(auth.clone(), move |db| {
            let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let content = serde_json::to_string(&snapshot)?;
            let hash = snapshots::hash(content.as_bytes());
            let pin = serde_json::to_string(&snapshot.pin)?;
            let now = auth.recheck(&tx)?;
            tx.execute("INSERT INTO private_sheets(id,campaign_id,custodian_id,game_id,edition_id,kind,pin,revision,state,snapshot,hash) VALUES(1,?1,?2,?3,?4,'runtime',?5,1,'active',?6,?7)",params![snapshot.campaign_id,snapshot.custodian_id,snapshot.game_id,snapshot.edition_id,pin,content,hash])?;
            tx.execute("INSERT INTO private_sheet_history(sheet_id,revision,hash,snapshot,operation,previous_revision,actor_id,actor_epoch,session_digest,occurred_at) VALUES(1,1,?1,?2,'create',0,?3,?4,?5,?6)",params![hash,content,auth.account_id(),auth.epoch(),auth.digest(),now])?;
            tx.commit()?;
            Ok(())
        })
        .await
        .unwrap();
}

/// Account 3 (`auth[2]`) joins the campaign as a player.
async fn join_player(setup: &Setup) {
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
}

/// The Master removes account 3, which freezes its sheets.
async fn remove_player(setup: &Setup) {
    let campaign = setup.campaign;
    let revision: i64 = setup
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
        .depart(setup.auth[1].clone(), setup.campaign, 3, revision)
        .await
        .unwrap();
}

async fn reopen(setup: &Setup, auth: usize) -> PrivateSnapshot {
    let (view, permit) = setup
        .store
        .private_read(
            setup.auth[auth].clone(),
            1,
            None,
            crate::definitions::admit().unwrap(),
        )
        .await
        .unwrap();
    drop(permit);
    view.snapshot
}

#[tokio::test]
async fn departure_and_removal_freeze_keep_arbitrary_valid_ids() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = list_setup(&list_definition()).await;
    join_player(&setup).await;
    let legacy = data(vec![item("i-0", "Old"), item("i-1", "Older")], vec![]);
    legacy_create(&setup, 2, legacy.clone()).await;
    // Retained arbitrary IDs stay editable: only absent IDs must be host IDs.
    let kept = reopen(&setup, 2).await;
    let mut snapshot = kept.clone();
    snapshot.data = data(vec![item("i-1", "Older"), item("i-0", "Old")], vec![]);
    let edited = write(&setup, 2, snapshot, "edit").await.unwrap();
    assert_eq!(edited.revision, 2);
    remove_player(&setup).await;
    let frozen = reopen(&setup, 2).await;
    assert_eq!(frozen.state, "frozen");
    assert_eq!(frozen.revision, 3);
    assert_eq!(frozen.data, edited.data, "freeze keeps the arbitrary IDs");
}

#[tokio::test]
async fn stale_write_with_bad_ids_is_409_not_422() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = list_setup(&list_definition()).await;
    let first = create(&setup, data(vec![item("r1-1", "A")], vec![]))
        .await
        .unwrap();
    edit(&setup, &first, data(vec![], vec![])).await.unwrap();
    // `first` is now stale (revision 1 against current 2) and carries a client-chosen ID.
    let result = edit(&setup, &first, data(vec![item("forged", "F")], vec![])).await;
    assert!(
        matches!(result, Err(AppError::Conflict)),
        "want Conflict, got {:?}",
        result.map(|s| s.revision)
    );
}

fn integer_items(lists: usize) -> Value {
    let mut values = serde_json::Map::new();
    for l in 0..lists {
        let items: Vec<Value> = (1..=256)
            .map(|k| {
                let mut item = json!({"id": format!("r1-{k}")});
                for p in 0..32 {
                    item[format!("p{p}")] = 0.into();
                }
                item
            })
            .collect();
        values.insert(format!("l{l}"), items.into());
    }
    values.into()
}

#[tokio::test]
async fn writer_refuses_undecodable_snapshot_atomically() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = list_setup(&integer_lists_definition(4)).await;
    let values = integer_items(4);
    // Small enough for the 1 MiB byte cap, too many nodes for the sheet decoder.
    assert!(serde_json::to_vec(&values).unwrap().len() < 300 * 1024);
    let result = create(&setup, values).await;
    match result {
        Err(AppError::Invalid(reason)) => {
            assert_eq!(reason, "Private snapshot exceeds the sheet decoder limits.");
        }
        other => panic!("want Invalid, got {:?}", other.map(|s| s.revision)),
    }
    assert_eq!(sheet_count(&setup).await, 0, "nothing written");
}

#[tokio::test]
async fn decoder_bound_snapshot_writes_reopens_and_freezes() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = list_setup(&integer_lists_definition(3)).await;
    join_player(&setup).await;
    let mut snapshot = blank(&setup, 2).await;
    snapshot.sheet_id = 1;
    snapshot.data = integer_items(3);
    let created = write(&setup, 2, snapshot, "create").await.unwrap();
    assert_eq!(reopen(&setup, 2).await.data, created.data);
    remove_player(&setup).await;
    let frozen = reopen(&setup, 2).await;
    assert_eq!((frozen.state.as_str(), frozen.revision), ("frozen", 2));
    assert_eq!(frozen.data, created.data);
}

fn pair(key: impl Into<String>, value: impl Into<String>) -> (String, String) {
    (key.into(), value.into())
}

/// One `equipment` row (field 1): name, quantity, armor (optional), kind (optional),
/// equipped.
fn equipment_row(
    r: usize,
    id: &str,
    name: &str,
    quantity: &str,
    armor: Option<&str>,
    kind: Option<&str>,
    equipped: &str,
) -> Vec<(String, String)> {
    let state = |value: Option<&str>| if value.is_some() { "present" } else { "absent" };
    vec![
        pair(format!("id-1-{r}"), id),
        pair(format!("row-1-{r}-0"), name),
        pair(format!("row-1-{r}-1"), quantity),
        pair(format!("row-1-{r}-2"), armor.unwrap_or_default()),
        pair(format!("set-1-{r}-2"), state(armor)),
        pair(format!("row-1-{r}-3"), kind.unwrap_or_default()),
        pair(format!("set-1-{r}-3"), state(kind)),
        pair(format!("row-1-{r}-4"), equipped),
    ]
}

/// One `tags` row (field 2).
fn tag_row(r: usize, id: &str, text: &str) -> Vec<(String, String)> {
    vec![
        pair(format!("id-2-{r}"), id),
        pair(format!("row-2-{r}-0"), text),
    ]
}

/// A full list-sheet form: the scalars plus the given rows.
fn list_form(revision: i64, rows: Vec<Vec<(String, String)>>) -> Vec<(String, String)> {
    let mut form = vec![
        pair("csrf", "a".repeat(64)),
        pair("revision", revision.to_string()),
        pair("name", "List sheet"),
        pair("notes", ""),
        pair("value-label", "x"),
    ];
    form.extend(rows.into_iter().flatten());
    form
}

fn create_path(setup: &Setup) -> String {
    format!(
        "/campaigns/{}/sheets/new?version={}",
        setup.campaign, setup.version
    )
}

/// A two-row equipment list and one tag, as a create form.
fn rows_form() -> Vec<(String, String)> {
    list_form(
        0,
        vec![
            equipment_row(0, "", "Rope", "1", None, None, "false"),
            equipment_row(1, "", "Mail", "1", Some("2"), Some("heavy"), "true"),
            tag_row(0, "", "brave"),
        ],
    )
}

#[tokio::test]
async fn list_row_keys_admit_and_bad_key_sets_are_400() {
    use axum::http::StatusCode;
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = list_setup(&list_definition()).await;
    let path = create_path(&setup);
    // Positive: the D1 key set is admitted (whatever the save then decides).
    let (status, _) = setup.post(&path, rows_form()).await;
    assert_ne!(status, StatusCode::BAD_REQUEST, "list row keys admitted");
    let replace = |from: &str, to: &str| {
        let mut form = rows_form();
        for (key, _) in &mut form {
            if key == from {
                *key = to.into();
            }
        }
        form
    };
    let mut cases = vec![
        ("non-canonical f", replace("id-1-0", "id-01-0")),
        ("signed f", replace("id-1-0", "id-+1-0")),
        ("non-list f", replace("id-2-0", "id-0-0")),
        ("same-count substitution", replace("row-1-1-0", "row-1-1-9")),
        ("unknown 70-byte key", replace("notes", &"n".repeat(70))),
        ("71-byte key", replace("notes", &"n".repeat(71))),
    ];
    let mut gap = rows_form();
    for (key, _) in &mut gap {
        if let Some(rest) = key.strip_prefix("row-1-1-").map(String::from) {
            *key = format!("row-1-2-{rest}");
        } else if let Some(rest) = key.strip_prefix("set-1-1-").map(String::from) {
            *key = format!("set-1-2-{rest}");
        } else if key == "id-1-1" {
            *key = "id-1-2".into();
        }
    }
    cases.push(("row gap", gap));
    let mut overflow = list_form(
        0,
        (0..=256)
            .map(|r| equipment_row(r, "", "N", "1", None, None, "false"))
            .collect(),
    );
    cases.push(("r = 256", overflow.clone()));
    overflow.truncate(5);
    overflow.extend(equipment_row(256, "", "N", "1", None, None, "false"));
    cases.push(("lone r = 256", overflow));
    for (context, form) in cases {
        assert_eq!(
            setup.post(&path, form).await.0,
            StatusCode::BAD_REQUEST,
            "{context}"
        );
    }
    // A `%2D` alias decodes to a duplicate key: refused by the parser.
    let mut encoded = serde_urlencoded::to_string(rows_form()).unwrap();
    encoded.push_str("&id%2D1-0=");
    let response = raw_post(&setup, &path, encoded).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST, "%2D duplicate");
    // At most the positive case was saved; no refused form wrote anything.
    let saved = i64::from(status == StatusCode::SEE_OTHER);
    assert_eq!(sheet_count(&setup).await, saved);
}

async fn raw_post(setup: &Setup, path: &str, body: String) -> axum::response::Response {
    use tower::ServiceExt;
    let request = axum::http::Request::builder()
        .uri(path)
        .method("POST")
        .header("host", "127.0.0.1:3000")
        .header("cookie", setup.auth[1].fixture_cookie())
        .header("origin", "http://127.0.0.1:3000")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(axum::body::Body::from(body))
        .unwrap();
    setup.app.clone().oneshot(request).await.unwrap()
}

/// A stale draft answers 409 even when its values are invalid (plan D4): the revision check
/// now precedes value validation on scalar runtime and private native sheets.
#[tokio::test]
async fn stale_invalid_draft_conflicts() {
    use axum::http::StatusCode;
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&super::formula_tests::formula_definition()).await;
    setup.create().await;
    let current = super::formula_tests::form(1, "4", Some("2"));
    assert_eq!(
        setup.post("/private-sheets/1", current).await.0,
        StatusCode::SEE_OTHER
    );
    // Paired baseline: an invalid draft at the current revision is 422.
    let invalid_current = super::formula_tests::form(2, "x", Some("2"));
    assert_eq!(
        setup.post("/private-sheets/1", invalid_current).await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let stale = super::formula_tests::form(1, "x", Some("2"));
    let (status, html) = setup.post("/private-sheets/1", stale).await;
    assert_eq!(status, StatusCode::CONFLICT, "scalar runtime");
    assert!(
        html.contains("name=\"revision\" value=\"1\""),
        "submitted revision kept"
    );
    // Private native sheet.
    let store = &setup.store;
    let campaign = store
        .create_campaign(
            setup.auth[1].clone(),
            "Native".into(),
            "mork-borg".into(),
            "it-2025".into(),
        )
        .await
        .unwrap();
    let mut native = super::native(campaign, 2);
    native.sheet_id = 2;
    super::insert_native(store, setup.auth[1].clone(), native).await;
    let mut form = super::native_form();
    for (key, value) in &mut form {
        if key == "revision" {
            *value = "1".into();
        }
    }
    assert_eq!(
        setup.post("/private-sheets/2", form.clone()).await.0,
        StatusCode::SEE_OTHER
    );
    for (key, value) in &mut form {
        if key == "agility" {
            *value = "x".into();
        }
    }
    assert_eq!(
        setup.post("/private-sheets/2", form).await.0,
        StatusCode::CONFLICT,
        "private native"
    );
}

#[tokio::test]
async fn bad_keys_after_csrf_and_authorization() {
    use axum::http::StatusCode;
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = setup(&super::formula_tests::formula_definition()).await;
    setup.create().await;
    let mut bad = super::formula_tests::form(1, "4", Some("2"));
    bad.push(pair("id-0-0", ""));
    let mut bad_csrf = bad.clone();
    bad_csrf[0].1 = "b".repeat(64);
    assert_eq!(
        setup.post("/private-sheets/1", bad_csrf).await.0,
        StatusCode::FORBIDDEN,
        "bad CSRF with bad keys"
    );
    let outsider = super::router_request(
        &setup.app,
        &setup.auth[3],
        "/private-sheets/1",
        Some(bad.clone()),
    )
    .await;
    assert_eq!(
        outsider.status(),
        StatusCode::FORBIDDEN,
        "no edit rights with bad keys"
    );
    assert_eq!(
        setup.post("/private-sheets/1", bad).await.0,
        StatusCode::BAD_REQUEST,
        "authorized with bad keys"
    );
}

fn selector(css: &str) -> scraper::Selector {
    scraper::Selector::parse(css).unwrap()
}

/// The sheet created by `create` with `rows` equipment rows `r1-1…` and one tag.
async fn create_rows(setup: &Setup, rows: usize) -> PrivateSnapshot {
    let equipment = (1..=rows)
        .map(|k| {
            let mut item = item(&format!("r1-{k}"), &format!("Item {k}"));
            if k % 2 == 0 {
                item["armor"] = 1.into();
                item["kind"] = "heavy".into();
            }
            item
        })
        .collect();
    create(setup, data(equipment, vec![tag("r1-1")]))
        .await
        .unwrap()
}

#[tokio::test]
async fn list_sheet_rendered_form_round_trips() {
    use axum::http::StatusCode;
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = list_setup(&list_definition()).await;
    let created = create_rows(&setup, 70).await;
    let (status, html) = setup.get("/private-sheets/1").await;
    assert_eq!(status, StatusCode::OK);
    let form = super::formula_tests::submitted(&html);
    assert!(form.len() > 516, "{} pairs", form.len());
    let (status, _) = setup.post("/private-sheets/1", form).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let saved = reopen(&setup, 1).await;
    assert_eq!(saved.revision, 2);
    assert_eq!(saved.data, created.data, "unchanged round trip");
}

#[tokio::test]
async fn list_rows_render_as_labelled_fieldsets_unique_ids() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = list_setup(&list_definition()).await;
    create_rows(&setup, 3).await;
    let (_, html) = setup.get("/private-sheets/1").await;
    let page = scraper::Html::parse_document(&html);
    let lists: Vec<_> = page.select(&selector("fieldset.list-field")).collect();
    assert_eq!(lists.len(), 2, "equipment and tags");
    assert_eq!(lists[0].value().attr("id"), Some("list-1"));
    let legend: String = lists[0]
        .select(&selector("legend"))
        .next()
        .unwrap()
        .text()
        .collect();
    assert_eq!(legend, "Equipment");
    let rows: Vec<_> = lists[0].select(&selector("fieldset.list-row")).collect();
    assert_eq!(rows.len(), 3);
    let row_legend: String = rows[1]
        .select(&selector("legend"))
        .next()
        .unwrap()
        .text()
        .collect();
    assert_eq!(row_legend, "Row 2 of 3 · ID r1-2");
    assert_eq!(rows[1].value().attr("id"), Some("row-1-1"));
    let mut ids = std::collections::BTreeSet::new();
    for element in page.select(&selector("[id]")) {
        let id = element.value().attr("id").unwrap();
        assert!(ids.insert(id.to_owned()), "duplicate id {id}");
    }
    for control in lists[0].select(&selector("input:not([type=hidden]), select, textarea")) {
        let id = control.value().attr("id").expect("control id");
        assert_eq!(
            page.select(&selector(&format!("label[for=\"{id}\"]")))
                .count(),
            1,
            "label for {id}"
        );
    }
    // Optional properties carry an explicit Set/Not set state; required ones do not.
    assert!(html.contains("name=\"set-1-1-2\""));
    assert!(!html.contains("name=\"set-1-1-0\""));
    let empty = list_setup(&list_definition()).await;
    let (_, create_page) = empty
        .get(&format!(
            "/campaigns/{}/sheets/new?version={}",
            empty.campaign, empty.version
        ))
        .await;
    assert!(create_page.contains("No rows yet."));
}

#[tokio::test]
async fn failed_save_rerender_keeps_draft_and_submitted_revision() {
    use axum::http::StatusCode;
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = list_setup(&list_definition()).await;
    create_rows(&setup, 1).await;
    let form = list_form(
        1,
        vec![
            equipment_row(0, "r1-1", "Kept <draft>", "x", None, None, "true"),
            equipment_row(
                1,
                "",
                "New draft row",
                "2",
                Some("4"),
                Some("light"),
                "false",
            ),
        ],
    );
    let (status, html) = setup.post("/private-sheets/1", form).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let page = scraper::Html::parse_document(&html);
    let value = |id: &str| {
        page.select(&selector(&format!("[name=\"{id}\"]")))
            .next()
            .unwrap_or_else(|| panic!("{id} missing"))
            .value()
            .attr("value")
            .map(str::to_owned)
    };
    assert_eq!(value("row-1-0-0").as_deref(), Some("Kept <draft>"));
    assert_eq!(value("row-1-0-1").as_deref(), Some("x"));
    assert_eq!(value("id-1-1").as_deref(), Some(""));
    assert_eq!(value("row-1-1-2").as_deref(), Some("4"));
    assert_eq!(value("revision").as_deref(), Some("1"));
    assert_eq!(reopen(&setup, 1).await.revision, 1, "nothing saved");
}

#[test]
fn submitted_helper_sends_only_the_clicked_submitter() {
    use super::formula_tests::{default_submitter, submitted_by};
    let page = r#"<form><input type="hidden" name="csrf" value="t"><button name="op" value="save">Save</button>
<input name="a" value="1"><input name="off" value="x" disabled>
<fieldset disabled><legend><input name="legend" value="kept"></legend><input name="inner" value="y"></fieldset>
<select name="s"><option value="u">U</option><option value="v" selected>V</option></select>
<textarea name="t">&lt;text&gt;</textarea><button name="op" value="add-1">Add</button><button name="op" value="up-1-1" disabled>Up</button></form>"#;
    let base = vec![
        ("csrf".to_owned(), "t".to_owned()),
        ("a".into(), "1".into()),
        ("legend".into(), "kept".into()),
        ("s".into(), "v".into()),
        ("t".into(), "<text>".into()),
    ];
    assert_eq!(submitted_by(page, None), base);
    let mut add = base.clone();
    add.push(("op".into(), "add-1".into()));
    assert_eq!(submitted_by(page, Some(("op", "add-1"))), add);
    let mut save = base;
    save.insert(1, ("op".into(), "save".into()));
    assert_eq!(submitted_by(page, Some(("op", "save"))), save);
    assert_eq!(
        default_submitter(page),
        Some(("op".to_owned(), "save".to_owned()))
    );
    let disabled = std::panic::catch_unwind(|| submitted_by(page, Some(("op", "up-1-1"))));
    assert!(disabled.is_err(), "a disabled button cannot be clicked");
}

#[tokio::test]
async fn first_submitter_in_tree_order_is_op_save_before_groups() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = list_setup(&list_definition()).await;
    create_rows(&setup, 2).await;
    let (_, html) = setup.get("/private-sheets/1").await;
    let page = scraper::Html::parse_document(&html);
    let order: Vec<String> = page
        .select(&selector(
            "form button, form input[type=submit], form section.panel, form fieldset.list-field",
        ))
        .map(|element| {
            let value = element.value();
            format!(
                "{}:{}={}",
                value.name(),
                value.attr("name").unwrap_or_default(),
                value.attr("value").unwrap_or_default()
            )
        })
        .collect();
    assert_eq!(
        order.first().map(String::as_str),
        Some("button:op=save"),
        "{order:?}"
    );
    assert_eq!(
        super::formula_tests::default_submitter(&html),
        Some(("op".into(), "save".into()))
    );
    let saves = order
        .iter()
        .filter(|item| *item == "button:op=save")
        .count();
    assert_eq!(saves, 2, "top and bottom Save: {order:?}");
    assert_eq!(order.last().map(String::as_str), Some("button:op=save"));
    // Scalar sheets keep their single (bottom) Save, now named op=save.
    let scalar = setup_scalar().await;
    let (_, html) = scalar.get("/private-sheets/1").await;
    let page = scraper::Html::parse_document(&html);
    let buttons: Vec<_> = page.select(&selector("form button")).collect();
    assert_eq!(buttons.len(), 1);
    assert_eq!(buttons[0].value().attr("value"), Some("save"));
}

async fn setup_scalar() -> Setup {
    let setup = setup(&super::formula_tests::formula_definition()).await;
    setup.create().await;
    setup
}

#[tokio::test]
async fn stale_invalid_list_draft_conflicts() {
    use axum::http::StatusCode;
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = list_setup(&list_definition()).await;
    let created = create_rows(&setup, 1).await;
    edit(&setup, &created, created.data.clone()).await.unwrap();
    let stale = list_form(
        1,
        vec![
            equipment_row(0, "r1-1", "A", "not a number", None, None, "true"),
            equipment_row(1, "forged", "B", "1", None, None, "maybe"),
        ],
    );
    assert_eq!(
        setup.post("/private-sheets/1", stale).await.0,
        StatusCode::CONFLICT
    );
}
