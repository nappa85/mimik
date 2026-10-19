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
pub(crate) fn list_definition() -> Value {
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

/// Publishes a list definition through the real publication path (the M4 gate lift).
async fn list_setup(definition: &Value) -> Setup {
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
    write_pinned(setup, auth, snapshot, operation, setup.version).await
}

/// `write` with the immutable pin of definition version `version`.
async fn write_pinned(
    setup: &Setup,
    auth: usize,
    snapshot: PrivateSnapshot,
    operation: &'static str,
    version: i64,
) -> Result<PrivateSnapshot, AppError> {
    let auth: RequestAuth = setup.auth[auth].clone();
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
    let setup = list_setup(&list_definition()).await;
    // Four lists of 256 × 32 integers are refused at publication by the render and node
    // budgets (step 8), so the writer is reached directly: the admitted definition is stored
    // as a version row without being published.
    let definition = integer_lists_definition(4);
    let pin = crate::definitions::private_pin::for_tests(&definition);
    assert!(
        !super::values::within(
            super::values::form_budget(pin.definition()),
            super::values::FORM_LIMITS
        ),
        "unpublishable"
    );
    let (content, hash) = (pin.raw.content.clone(), pin.raw.hash.clone());
    let version: i64 = setup
        .store
        .run(move |db| {
            db.execute(
                "INSERT INTO definition_versions(game_id,edition_id,definition_id,version,content,content_hash) VALUES('synthetic-game','first','integer-lists',1,?1,?2)",
                params![content, hash],
            )?;
            Ok(db.last_insert_rowid())
        })
        .await
        .unwrap();
    let mut snapshot = blank(&setup, 1).await;
    snapshot.sheet_id = 1;
    snapshot.pin = snapshots::Pin::Runtime {
        version_id: version,
        version: 1,
        content_hash: pin.raw.hash.clone(),
        schema: 1,
    };
    let values = integer_items(4);
    // Small enough for the 1 MiB byte cap, too many nodes for the sheet decoder.
    assert!(serde_json::to_vec(&values).unwrap().len() < 300 * 1024);
    snapshot.data = values;
    let result = write_pinned(&setup, 1, snapshot, "create", version).await;
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
    // A real change (an unchanged save writes nothing, D-U).
    let current = super::formula_tests::form(1, "5", Some("2"));
    assert_eq!(
        setup.post("/private-sheets/1", current).await.0,
        StatusCode::SEE_OTHER
    );
    // Paired baseline: an invalid draft at the current revision is 422 (a control character
    // is still refused; blank and unreadable numbers are unknown or scribbles since 1.4).
    let invalid_current = super::formula_tests::form(2, "\u{7}", Some("2"));
    assert_eq!(
        setup.post("/private-sheets/1", invalid_current).await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let stale = super::formula_tests::form(1, "\u{7}", Some("2"));
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
        match key.as_str() {
            "revision" => *value = "1".into(),
            "notes" => *value = "Edited".into(),
            _ => {}
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
    // Admitted unchanged, so nothing is written (D-U).
    let (status, page) = setup.post("/private-sheets/1", form).await;
    assert_eq!(status, StatusCode::OK);
    assert!(page.contains("No change"));
    let saved = reopen(&setup, 1).await;
    assert_eq!(saved.revision, 1);
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
            // A control character is still refused (blank is unknown since slice 1.4).
            equipment_row(0, "r1-1", "Kept <draft>", "1\u{7}", None, None, "true"),
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
    assert_eq!(value("row-1-0-1").as_deref(), Some("1\u{7}"));
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

// ---- Step 4: draft operations (plan D5) ----

use super::formula_tests::{submitted, submitted_by};
use axum::http::StatusCode;

/// The sheet page form with `op` clicked.
fn clicked(html: &str, op: &str) -> Vec<(String, String)> {
    submitted_by(html, Some(("op", op)))
}

/// The page form with `op` added even when its button is disabled or absent (a forged op).
fn forged(html: &str, op: &str) -> Vec<(String, String)> {
    let mut form = submitted_by(html, None);
    form.push(pair("op", op));
    form
}

fn set(form: &mut [(String, String)], key: &str, value: &str) {
    let pair = form
        .iter_mut()
        .find(|(k, _)| k == key)
        .unwrap_or_else(|| panic!("{key} missing"));
    pair.1 = value.into();
}

fn get(form: &[(String, String)], key: &str) -> String {
    form.iter()
        .find(|(k, _)| k == key)
        .map_or_else(|| panic!("{key} missing"), |(_, v)| v.clone())
}

/// The hidden IDs and names of the equipment rows of a page, in row order.
fn rows_of(html: &str) -> Vec<(String, String)> {
    let form = submitted_by(html, None);
    (0..)
        .map_while(|r| {
            let id = form.iter().find(|(k, _)| *k == format!("id-1-{r}"))?;
            let name = form.iter().find(|(k, _)| *k == format!("row-1-{r}-0"))?;
            Some((id.1.clone(), name.1.clone()))
        })
        .collect()
}

fn autofocus(html: &str) -> Vec<String> {
    scraper::Html::parse_document(html)
        .select(&selector("[autofocus]"))
        .map(|element| element.value().attr("id").unwrap_or_default().to_owned())
        .collect()
}

fn status_text(html: &str) -> String {
    scraper::Html::parse_document(html)
        .select(&selector("p[role=status]"))
        .next()
        .map(|element| element.text().collect())
        .unwrap_or_default()
}

async fn history_len(setup: &Setup) -> usize {
    setup
        .store
        .private_history(setup.auth[1].clone(), 1, 0)
        .await
        .unwrap()
        .len()
}

async fn sheet_page(setup: &Setup) -> String {
    let (status, html) = setup.get("/private-sheets/1").await;
    assert_eq!(status, StatusCode::OK);
    html
}

/// Three rows A, B, C (`r1-1…r1-3`); B has its optional properties set.
async fn abc(setup: &Setup) -> PrivateSnapshot {
    let mut b = item("r1-2", "B");
    b["armor"] = 2.into();
    b["kind"] = "heavy".into();
    create(
        setup,
        data(vec![item("r1-1", "A"), b, item("r1-3", "C")], vec![]),
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn competing_writer_loser_unchanged() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = list_setup(&list_definition()).await;
    abc(&setup).await;
    let html = sheet_page(&setup).await;
    let mut first = submitted(&html);
    set(&mut first, "row-1-0-0", "First writer");
    let mut second = submitted(&html);
    set(&mut second, "row-1-0-0", "Second writer");
    assert_eq!(
        setup.post("/private-sheets/1", first).await.0,
        StatusCode::SEE_OTHER
    );
    let (status, page) = setup.post("/private-sheets/1", second).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(page.contains("Second writer"), "the loser's draft is kept");
    let saved = reopen(&setup, 1).await;
    assert_eq!(saved.revision, 2);
    assert_eq!(saved.data["equipment"][0]["name"], "First writer");
    assert_eq!(history_len(&setup).await, 2);
}

#[tokio::test]
async fn op_409_op_save_never_rebases() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = list_setup(&list_definition()).await;
    let created = abc(&setup).await;
    let html = sheet_page(&setup).await;
    edit(&setup, &created, created.data.clone()).await.unwrap();
    let (status, op_page) = setup
        .post("/private-sheets/1", clicked(&html, "add-1"))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(get(&submitted_by(&op_page, None), "revision"), "1");
    let (status, conflict) = setup.post("/private-sheets/1", submitted(&op_page)).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(get(&submitted_by(&conflict, None), "revision"), "1");
    assert_eq!(rows_of(&conflict).len(), 4, "the added draft row is kept");
    let (status, again) = setup
        .post("/private-sheets/1", clicked(&conflict, "remove-1-3"))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(get(&submitted_by(&again, None), "revision"), "1");
    assert_eq!(reopen(&setup, 1).await.revision, 2);
}

#[tokio::test]
async fn add_rerenders_200_no_store_without_writing() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = list_setup(&list_definition()).await;
    abc(&setup).await;
    let html = sheet_page(&setup).await;
    let response = super::router_request(
        &setup.app,
        &setup.auth[1],
        "/private-sheets/1",
        Some(clicked(&html, "add-1")),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["cache-control"], "no-store");
    let page = super::formula_tests::body(response).await;
    let rows = rows_of(&page);
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[3], (String::new(), String::new()), "an empty new row");
    assert!(page.contains("Row 4 of 4 · New row · ID assigned on save"));
    let form = submitted_by(&page, None);
    assert_eq!(
        get(&form, "set-1-3-2"),
        "absent",
        "new optional properties start Not set"
    );
    assert_eq!(reopen(&setup, 1).await.revision, 1, "nothing written");
    assert_eq!(history_len(&setup).await, 1);
}

#[tokio::test]
async fn remove_rerenders_without_writing() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = list_setup(&list_definition()).await;
    abc(&setup).await;
    let html = sheet_page(&setup).await;
    let (status, page) = setup
        .post("/private-sheets/1", clicked(&html, "remove-1-1"))
        .await;
    assert_eq!(status, StatusCode::OK);
    let rows: Vec<String> = rows_of(&page).into_iter().map(|row| row.0).collect();
    assert_eq!(rows, ["r1-1", "r1-3"]);
    assert_eq!(reopen(&setup, 1).await.revision, 1, "nothing written");
    assert_eq!(history_len(&setup).await, 1);
    // Saving the draft persists the removal.
    assert_eq!(
        setup.post("/private-sheets/1", submitted(&page)).await.0,
        StatusCode::SEE_OTHER
    );
    let saved = reopen(&setup, 1).await;
    assert_eq!(saved.data["equipment"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn move_up_down_rerender_new_order_without_writing() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = list_setup(&list_definition()).await;
    abc(&setup).await;
    let html = sheet_page(&setup).await;
    let (status, up) = setup
        .post("/private-sheets/1", clicked(&html, "up-1-2"))
        .await;
    assert_eq!(status, StatusCode::OK);
    let names: Vec<String> = rows_of(&up).into_iter().map(|row| row.1).collect();
    assert_eq!(names, ["A", "C", "B"]);
    let (status, down) = setup
        .post("/private-sheets/1", clicked(&up, "down-1-0"))
        .await;
    assert_eq!(status, StatusCode::OK);
    let names: Vec<String> = rows_of(&down).into_iter().map(|row| row.1).collect();
    assert_eq!(names, ["C", "A", "B"]);
    assert_eq!(reopen(&setup, 1).await.revision, 1, "nothing written");
    assert_eq!(history_len(&setup).await, 1);
}

#[tokio::test]
async fn move_carries_id_values_and_set_state() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = list_setup(&list_definition()).await;
    abc(&setup).await;
    let html = sheet_page(&setup).await;
    let mut form = clicked(&html, "up-1-1");
    set(&mut form, "row-1-1-1", "not a number");
    let (status, page) = setup.post("/private-sheets/1", form).await;
    assert_eq!(status, StatusCode::OK);
    let moved = submitted_by(&page, None);
    for (key, want) in [
        ("id-1-0", "r1-2"),
        ("row-1-0-0", "B"),
        ("row-1-0-1", "not a number"),
        ("row-1-0-2", "2"),
        ("set-1-0-2", "present"),
        ("row-1-0-3", "heavy"),
        ("set-1-0-3", "present"),
        ("id-1-1", "r1-1"),
        ("row-1-1-0", "A"),
        ("set-1-1-2", "absent"),
        ("set-1-1-3", "absent"),
    ] {
        assert_eq!(get(&moved, key), want, "{key}");
    }
}

#[tokio::test]
async fn save_after_moves_persists_order() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = list_setup(&list_definition()).await;
    create(
        &setup,
        data(vec![item("r1-1", "A"), item("r1-2", "B")], vec![]),
    )
    .await
    .unwrap();
    let html = sheet_page(&setup).await;
    // Add N1 and N2 at the end, then move N2 to the top and N1 above A:
    // [A, B, N1, N2] → [N2, A, B, N1] → [N2, A, N1, B] → [N2, N1, A, B].
    let (_, page) = setup
        .post("/private-sheets/1", clicked(&html, "add-1"))
        .await;
    let (_, page) = setup
        .post("/private-sheets/1", clicked(&page, "add-1"))
        .await;
    let mut form = clicked(&page, "up-1-3");
    for (key, value) in [
        ("row-1-2-0", "N1"),
        ("row-1-2-1", "1"),
        ("row-1-2-4", "false"),
        ("row-1-3-0", "N2"),
        ("row-1-3-1", "2"),
        ("row-1-3-4", "true"),
    ] {
        set(&mut form, key, value);
    }
    let (_, page) = setup.post("/private-sheets/1", form).await;
    let (_, page) = setup
        .post("/private-sheets/1", clicked(&page, "up-1-2"))
        .await;
    let (_, page) = setup
        .post("/private-sheets/1", clicked(&page, "up-1-1"))
        .await;
    let (_, page) = setup
        .post("/private-sheets/1", clicked(&page, "up-1-3"))
        .await;
    let (_, page) = setup
        .post("/private-sheets/1", clicked(&page, "up-1-2"))
        .await;
    let names: Vec<String> = rows_of(&page).into_iter().map(|row| row.1).collect();
    assert_eq!(names, ["N2", "N1", "A", "B"]);
    assert_eq!(reopen(&setup, 1).await.revision, 1);
    assert_eq!(
        setup.post("/private-sheets/1", submitted(&page)).await.0,
        StatusCode::SEE_OTHER
    );
    let saved = reopen(&setup, 1).await;
    let stored: Vec<(&str, &str)> = saved.data["equipment"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| (item["id"].as_str().unwrap(), item["name"].as_str().unwrap()))
        .collect();
    // New IDs follow the final row order (plan N-D).
    assert_eq!(
        stored,
        [("r2-1", "N2"), ("r2-2", "N1"), ("r1-1", "A"), ("r1-2", "B")]
    );
}

#[tokio::test]
async fn edge_move_buttons_disabled() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = list_setup(&list_definition()).await;
    abc(&setup).await;
    let html = sheet_page(&setup).await;
    let page = scraper::Html::parse_document(&html);
    let disabled = |id: &str| {
        page.select(&selector(&format!("button#{id}")))
            .next()
            .unwrap_or_else(|| panic!("{id} missing"))
            .value()
            .attr("disabled")
            .is_some()
    };
    assert!(disabled("up-1-0") && disabled("down-1-2"));
    for id in [
        "down-1-0",
        "up-1-1",
        "down-1-1",
        "up-1-2",
        "remove-1-0",
        "add-1",
    ] {
        assert!(!disabled(id), "{id}");
    }
}

#[tokio::test]
async fn edge_or_out_of_range_move_is_400() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = list_setup(&list_definition()).await;
    abc(&setup).await;
    let html = sheet_page(&setup).await;
    // Paired positive on the same page.
    assert_eq!(
        setup
            .post("/private-sheets/1", clicked(&html, "up-1-1"))
            .await
            .0,
        StatusCode::OK
    );
    for op in ["up-1-0", "down-1-2", "up-1-3", "down-1-3", "remove-1-3"] {
        assert_eq!(
            setup.post("/private-sheets/1", forged(&html, op)).await.0,
            StatusCode::BAD_REQUEST,
            "{op}"
        );
    }
}

#[tokio::test]
async fn add_at_256_is_422_with_message() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = list_setup(&list_definition()).await;
    create_rows(&setup, 256).await;
    let html = sheet_page(&setup).await;
    let page = scraper::Html::parse_document(&html);
    let add = page.select(&selector("button#add-1")).next().unwrap();
    assert!(add.value().attr("disabled").is_some());
    assert_eq!(
        add.text().collect::<String>(),
        "Add Equipment row (limit reached: 256 rows)"
    );
    let response = super::router_request(
        &setup.app,
        &setup.auth[1],
        "/private-sheets/1",
        Some(forged(&html, "add-1")),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let page = super::formula_tests::body(response).await;
    assert!(
        status_text(&page)
            .contains("This list already has 256 rows, the most a list can hold. Not saved yet."),
        "{}",
        status_text(&page)
    );
    assert_eq!(rows_of(&page).len(), 256, "the draft is re-rendered");
}

#[tokio::test]
async fn bad_op_or_index_is_400_and_two_ops_is_400() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = list_setup(&list_definition()).await;
    abc(&setup).await;
    let html = sheet_page(&setup).await;
    for op in [
        "add-0",
        "add-3",
        "add-01",
        "add-+1",
        "add-1-0",
        "remove-1",
        "up-1",
        "down-1-01",
        "frobnicate",
        "save-1",
        "",
        "Add-1",
    ] {
        assert_eq!(
            setup.post("/private-sheets/1", forged(&html, op)).await.0,
            StatusCode::BAD_REQUEST,
            "{op:?}"
        );
    }
    let mut two = clicked(&html, "add-1");
    two.push(pair("op", "save"));
    assert_eq!(
        setup.post("/private-sheets/1", two).await.0,
        StatusCode::BAD_REQUEST,
        "two ops"
    );
    // A non-integer revision makes any op forged.
    let mut form = clicked(&html, "add-1");
    set(&mut form, "revision", "one");
    assert_eq!(
        setup.post("/private-sheets/1", form).await.0,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn ops_need_csrf_and_edit_rights() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = list_setup(&list_definition()).await;
    abc(&setup).await;
    let html = sheet_page(&setup).await;
    let mut bad = clicked(&html, "add-1");
    set(&mut bad, "csrf", &"b".repeat(64));
    assert_eq!(
        setup.post("/private-sheets/1", bad).await.0,
        StatusCode::FORBIDDEN
    );
    let outsider = super::router_request(
        &setup.app,
        &setup.auth[3],
        "/private-sheets/1",
        Some(clicked(&html, "add-1")),
    )
    .await;
    assert_eq!(outsider.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn stale_op_warns() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = list_setup(&list_definition()).await;
    let created = abc(&setup).await;
    let html = sheet_page(&setup).await;
    let warning = "The saved sheet changed (revision 1 → 2); saving this draft will be refused \
                   with a conflict (409). Copy your changes, reload and re-apply them.";
    let (_, current) = setup
        .post("/private-sheets/1", clicked(&html, "add-1"))
        .await;
    assert!(
        !current.contains("The saved sheet changed"),
        "no warning while current"
    );
    edit(&setup, &created, created.data.clone()).await.unwrap();
    let (status, stale) = setup
        .post("/private-sheets/1", clicked(&html, "add-1"))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(stale.contains(warning));
}

/// The list fixture plus a derived `load` over the equipment quantities.
fn formula_list_definition() -> Value {
    let mut definition = list_definition();
    definition["fields"].as_array_mut().unwrap().push(json!({
        "id": "load", "label": "Load", "control": "integer", "nullable": false,
        "mapping": mapped(),
        "formula": {"expr": "sum(equipment.map(item, item.quantity))", "rule_ids": ["r-1"],
                    "evidence": {"state": "unresolved", "reason": "Synthetic"}}}));
    definition["groups"][0]["fields"]
        .as_array_mut()
        .unwrap()
        .push("load".into());
    definition["constraints"] = json!([{"id": "light-load", "expr": "load <= 10",
        "rule_ids": [], "message": "Load at most 10.",
        "evidence": {"state": "unresolved", "reason": "Synthetic"}}]);
    definition
}

#[tokio::test]
async fn ops_never_evaluate_draft() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = list_setup(&formula_list_definition()).await;
    abc(&setup).await;
    let html = sheet_page(&setup).await;
    assert!(
        html.contains("Advisory checks: 0 violated"),
        "saved load 3 passes"
    );
    let mut form = clicked(&html, "add-1");
    set(&mut form, "row-1-0-1", "100");
    let (status, page) = setup.post("/private-sheets/1", form).await;
    assert_eq!(status, StatusCode::OK);
    assert!(page.contains("Computed from saved revision 1 — not from your unsaved changes"));
    assert!(page.contains("Advisory checks of saved revision 1: 0 violated"));
    assert!(!page.contains(">100<"), "the draft's load was not computed");
    assert!(!page.contains("1 violated"));
}

#[tokio::test]
async fn op_never_reveals_saved_ids() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = list_setup(&list_definition()).await;
    abc(&setup).await;
    let html = sheet_page(&setup).await;
    let (_, removed) = setup
        .post("/private-sheets/1", clicked(&html, "remove-1-1"))
        .await;
    let (status, page) = setup
        .post("/private-sheets/1", clicked(&removed, "add-1"))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        !page.contains("r1-2"),
        "a saved ID the draft lacked is not revealed"
    );
}

#[tokio::test]
async fn unknown_id_draft_op_rerenders_then_save_422_or_409() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = list_setup(&list_definition()).await;
    let created = abc(&setup).await;
    let html = sheet_page(&setup).await;
    let mut form = clicked(&html, "add-1");
    set(&mut form, "id-1-0", "r9-9");
    let (status, page) = setup.post("/private-sheets/1", form.clone()).await;
    assert_eq!(status, StatusCode::OK, "ops never validate IDs");
    let (status, _) = setup.post("/private-sheets/1", submitted(&page)).await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "current: tampering"
    );
    edit(&setup, &created, created.data.clone()).await.unwrap();
    let (status, stale) = setup.post("/private-sheets/1", form).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = setup.post("/private-sheets/1", submitted(&stale)).await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "stale: the revision check comes first"
    );
}

#[tokio::test]
/// Op buttons skip browser validation and post to the form's own action, with no URL
/// fragment: a fragment that names an element blocks `autofocus` in browsers, and the
/// focused control already scrolls into view.
async fn op_buttons_have_formnovalidate_and_no_fragment() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = list_setup(&list_definition()).await;
    let (_, create_page) = setup.get(&create_path(&setup)).await;
    abc(&setup).await;
    let html = sheet_page(&setup).await;
    for (page, action) in [
        (&html, "/private-sheets/1".to_owned()),
        (&create_page, create_path(&setup)),
    ] {
        let parsed = scraper::Html::parse_document(page);
        let ops: Vec<_> = parsed
            .select(&selector("button[name=op]:not([value=save])"))
            .collect();
        assert!(!ops.is_empty());
        for button in ops {
            let value = button.value();
            let op = value.attr("value").unwrap();
            assert!(value.attr("formnovalidate").is_some(), "{op}");
            assert_eq!(value.attr("formaction"), None, "{op}");
        }
        let form = parsed.select(&selector("form")).next().unwrap();
        assert_eq!(form.value().attr("action"), Some(action.as_str()));
        for save in parsed.select(&selector("button[value=save]")) {
            assert!(
                save.value().attr("formnovalidate").is_none(),
                "Save validates"
            );
        }
    }
}

#[tokio::test]
async fn single_autofocus_after_add_remove_and_each_move() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = list_setup(&list_definition()).await;
    abc(&setup).await;
    let html = sheet_page(&setup).await;
    assert!(autofocus(&html).is_empty(), "a plain view focuses nothing");
    for (op, focus) in [
        ("add-1", "row-1-3-0"),
        ("remove-1-1", "add-1"),
        ("up-1-2", "up-1-1"),
        ("up-1-1", "down-1-0"),
        ("down-1-0", "down-1-1"),
        ("down-1-1", "up-1-2"),
    ] {
        let (status, page) = setup.post("/private-sheets/1", clicked(&html, op)).await;
        assert_eq!(status, StatusCode::OK, "{op}");
        assert_eq!(autofocus(&page), [focus], "{op}");
    }
    // The tag list's first property is required text; an optional first property gets the
    // Set/Not set select instead (covered by the equipment row's own structure).
    let (_, tags) = setup
        .post("/private-sheets/1", clicked(&html, "add-2"))
        .await;
    assert_eq!(autofocus(&tags), ["row-2-0-0"]);
}

#[tokio::test]
async fn op_button_names_and_status_messages() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = list_setup(&list_definition()).await;
    abc(&setup).await;
    let html = sheet_page(&setup).await;
    let parsed = scraper::Html::parse_document(&html);
    let text = |id: &str| -> String {
        parsed
            .select(&selector(&format!("button#{id}")))
            .next()
            .unwrap()
            .text()
            .collect()
    };
    assert_eq!(text("add-1"), "Add Equipment row");
    assert_eq!(text("remove-1-1"), "Remove row 2 of Equipment");
    assert_eq!(text("up-1-1"), "Move row 2 of Equipment up");
    assert_eq!(text("down-1-1"), "Move row 2 of Equipment down");
    for (op, message) in [
        ("add-1", "Row added to Equipment. Not saved yet."),
        ("remove-1-2", "Row 3 of Equipment removed. Not saved yet."),
        (
            "up-1-2",
            "Row 3 of Equipment moved up to position 2. Not saved yet.",
        ),
        (
            "down-1-0",
            "Row 1 of Equipment moved down to position 2. Not saved yet.",
        ),
    ] {
        let (_, page) = setup.post("/private-sheets/1", clicked(&html, op)).await;
        assert_eq!(status_text(&page), message, "{op}");
    }
    // The focus target's accessible name carries the row's new position (plan N-G).
    let (_, moved) = setup
        .post("/private-sheets/1", clicked(&html, "up-1-2"))
        .await;
    let parsed = scraper::Html::parse_document(&moved);
    let focused = parsed.select(&selector("[autofocus]")).next().unwrap();
    assert_eq!(
        focused.text().collect::<String>(),
        "Move row 2 of Equipment up"
    );
}

#[tokio::test]
async fn create_page_adds_rows_then_creates() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = list_setup(&list_definition()).await;
    let path = create_path(&setup);
    let (_, page) = setup.get(&path).await;
    assert_eq!(
        get(&submitted_by(&page, None), "name"),
        "",
        "Name still empty"
    );
    let (status, page) = setup.post(&path, clicked(&page, "add-1")).await;
    assert_eq!(status, StatusCode::OK);
    let (status, page) = setup.post(&path, clicked(&page, "add-1")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(page.contains("New row · ID assigned on save"));
    let mut form = submitted(&page);
    for (key, value) in [
        ("name", "Created with rows"),
        ("value-label", "x"),
        ("row-1-0-0", "Rope"),
        ("row-1-0-1", "1"),
        ("row-1-0-4", "false"),
        ("row-1-1-0", "Torch"),
        ("row-1-1-1", "2"),
        ("row-1-1-4", "true"),
    ] {
        set(&mut form, key, value);
    }
    assert_eq!(setup.post(&path, form).await.0, StatusCode::SEE_OTHER);
    let saved = reopen(&setup, 1).await;
    let ids: Vec<&str> = saved.data["equipment"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["r1-1", "r1-2"]);
}

// ---- Step 5: history rows, change summary, derived values, escaping (plan D8, D9, D11) ----

async fn history_page(setup: &Setup, revision: i64) -> String {
    let (status, html) = setup
        .get(&format!("/private-sheets/1/history/{revision}"))
        .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "history {revision}: {}",
        &html[..html.len().min(3000)]
    );
    html
}

fn summary_section(html: &str) -> Option<String> {
    let page = scraper::Html::parse_document(html);
    page.select(&selector("section#change-summary"))
        .next()
        .map(|section| section.html())
}

fn summary_lines(html: &str) -> Vec<String> {
    let page = scraper::Html::parse_document(html);
    page.select(&selector("section#change-summary ul > li"))
        .map(|li| li.text().collect())
        .collect()
}

#[tokio::test]
async fn list_history_rows_have_stable_ids_and_no_ops() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = list_setup(&list_definition()).await;
    let created = abc(&setup).await;
    edit(
        &setup,
        &created,
        data(vec![item("r1-3", "C"), item("r1-1", "A")], vec![]),
    )
    .await
    .unwrap();
    for (revision, ids) in [(1, vec!["r1-1", "r1-2", "r1-3"]), (2, vec!["r1-3", "r1-1"])] {
        let html = history_page(&setup, revision).await;
        let page = scraper::Html::parse_document(&html);
        let legends: Vec<String> = page
            .select(&selector("fieldset.list-row > legend"))
            .map(|legend| legend.text().collect())
            .collect();
        let want: Vec<String> = ids
            .iter()
            .enumerate()
            .map(|(r, id)| format!("Row {} of {} · ID {id}", r + 1, ids.len()))
            .collect();
        assert_eq!(legends, want, "revision {revision}");
        assert_eq!(
            page.select(&selector("button")).count(),
            0,
            "no ops, no Save"
        );
        for control in page.select(&selector(
            "fieldset.list-row select, fieldset.list-row input:not([type=hidden])",
        )) {
            assert!(
                control.value().attr("disabled").is_some(),
                "read-only history"
            );
        }
    }
}

#[tokio::test]
async fn decoder_bound_history_renders_read_only() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = list_setup(&integer_lists_definition(3)).await;
    let mut snapshot = blank(&setup, 1).await;
    snapshot.sheet_id = 1;
    snapshot.data = integer_items(3);
    write(&setup, 1, snapshot, "create").await.unwrap();
    let html = history_page(&setup, 1).await;
    let page = scraper::Html::parse_document(&html);
    assert_eq!(page.select(&selector("fieldset.list-row")).count(), 3 * 256);
    assert_eq!(
        summary_lines(&html),
        ["Sheet created with 768 rows in 3 lists."]
    );
}

#[tokio::test]
async fn history_page_shows_change_summary_for_revision_n_against_n_minus_1() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = list_setup(&list_definition()).await;
    let first = abc(&setup).await;
    // Revision 2 removes C; revision 3 edits A's quantity and moves B to the top.
    let mut second_data = first.data.clone();
    second_data["equipment"].as_array_mut().unwrap().pop();
    let second = edit(&setup, &first, second_data).await.unwrap();
    let mut third_data = second.data.clone();
    third_data["equipment"].as_array_mut().unwrap().swap(0, 1);
    third_data["equipment"][1]["quantity"] = 7.into();
    edit(&setup, &second, third_data).await.unwrap();
    let html = history_page(&setup, 3).await;
    assert_eq!(
        summary_lines(&html),
        [
            "Row equipment/r1-1 of Equipment edited: Quantity equipment/r1-1/quantity.",
            "Row equipment/r1-2 of Equipment moved.",
        ],
        "N against N − 1 only (C's removal belongs to revision 2)"
    );
    assert_eq!(
        summary_lines(&history_page(&setup, 2).await),
        ["Row equipment/r1-3 of Equipment removed."]
    );
    // DOM structure (plan N-G).
    let page = scraper::Html::parse_document(&html);
    let section = page
        .select(&selector("section#change-summary"))
        .next()
        .expect("summary section");
    assert_eq!(
        section.value().attr("aria-labelledby"),
        Some("change-summary-heading")
    );
    let heading = section
        .select(&selector("h2#change-summary-heading"))
        .next()
        .unwrap();
    assert_eq!(heading.text().collect::<String>(), "Changes in revision 3");
    assert_eq!(section.select(&selector("ul")).count(), 1);
    let codes: Vec<String> = section
        .select(&selector("ul > li code"))
        .map(|code| code.text().collect())
        .collect();
    assert_eq!(
        codes,
        [
            "equipment/r1-1",
            "equipment/r1-1/quantity",
            "equipment/r1-2"
        ]
    );
    let link = section.select(&selector("a")).next().unwrap();
    assert_eq!(
        link.value().attr("href"),
        Some("/private-sheets/1/history/2")
    );
    // Revision 1 is a creation line; the current view has no summary.
    assert_eq!(
        summary_lines(&history_page(&setup, 1).await),
        ["Sheet created with 3 rows in 2 lists."]
    );
    assert!(summary_section(&sheet_page(&setup).await).is_none());
}

/// Replaces revision `revision`'s stored snapshot, keeping its serialization order (tests
/// only: the history is append-only, so the trigger is dropped and restored around the
/// update). `corrupt` stores a hash that does not match the content.
async fn tamper_history(
    setup: &Setup,
    revision: i64,
    corrupt: bool,
    edit: impl FnOnce(&mut PrivateSnapshot) + Send + 'static,
) {
    setup
        .store
        .run(move |db| {
            let tx = db.transaction()?;
            let content: String = tx.query_row(
                "SELECT snapshot FROM private_sheet_history WHERE sheet_id=1 AND revision=?1",
                [revision],
                |r| r.get(0),
            )?;
            let mut snapshot: PrivateSnapshot = serde_json::from_str(&content)?;
            edit(&mut snapshot);
            let content = serde_json::to_string(&snapshot)?;
            let hash = if corrupt {
                "0".repeat(64)
            } else {
                snapshots::hash(content.as_bytes())
            };
            let trigger: String = tx.query_row(
                "SELECT sql FROM sqlite_schema WHERE name='private_sheet_history_update'",
                [],
                |r| r.get(0),
            )?;
            tx.execute_batch("DROP TRIGGER private_sheet_history_update;")?;
            tx.execute(
                "UPDATE private_sheet_history SET snapshot=?1,hash=?2 WHERE sheet_id=1 AND revision=?3",
                params![content, hash, revision],
            )?;
            tx.execute_batch(&trigger)?;
            tx.commit()?;
            Ok(())
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn change_summary_unavailable_for_native_unsupported_and_invalid_previous() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let unavailable_revision = ["Change summary unavailable for this revision."];
    // A previous revision whose values fail the pin's validation.
    let setup = list_setup(&list_definition()).await;
    let first = abc(&setup).await;
    let second = edit(&setup, &first, first.data.clone()).await.unwrap();
    edit(&setup, &second, first.data.clone()).await.unwrap();
    tamper_history(&setup, 1, false, |snapshot| {
        snapshot.data["equipment"][0]["quantity"] = "not an integer".into();
    })
    .await;
    let html = history_page(&setup, 2).await;
    assert!(summary_lines(&html).is_empty());
    assert!(
        summary_section(&html)
            .unwrap()
            .contains(unavailable_revision[0]),
        "invalid values in N − 1"
    );
    assert!(
        html.contains("fieldset class=\"list-row\""),
        "revision N still renders"
    );
    // A previous revision whose stored hash does not match its content. (A previous revision
    // with another pin cannot sit under a valid revision N: the history view already refuses
    // N then, so the pin check of N − 1 is defence in depth.)
    tamper_history(&setup, 2, true, |_| {}).await;
    let html = history_page(&setup, 3).await;
    assert!(
        summary_section(&html)
            .unwrap()
            .contains(unavailable_revision[0]),
        "corrupt hash"
    );
    // Paired positive on the same sheet: revision 2 against an intact revision 1 is unavailable
    // only because revision 1 was tampered above; a fresh sheet shows a summary.
    let fresh = list_setup(&list_definition()).await;
    let first = abc(&fresh).await;
    edit(&fresh, &first, first.data.clone()).await.unwrap();
    assert!(
        summary_section(&history_page(&fresh, 2).await)
            .unwrap()
            .contains("No stored values changed."),
        "no changes, but available"
    );
    // A native sheet.
    let campaign = fresh
        .store
        .create_campaign(
            fresh.auth[1].clone(),
            "Native".into(),
            "mork-borg".into(),
            "it-2025".into(),
        )
        .await
        .unwrap();
    let mut native = super::native(campaign, 2);
    native.sheet_id = 2;
    super::insert_native(&fresh.store, fresh.auth[1].clone(), native).await;
    let (status, html) = fresh.get("/private-sheets/2/history/1").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        summary_section(&html)
            .unwrap()
            .contains("Change summary unavailable for this sheet."),
        "native"
    );
}

#[tokio::test]
async fn change_summary_unavailable_for_an_unsupported_runtime_pin() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = list_setup(&list_definition()).await;
    abc(&setup).await;
    setup
        .store
        .run(|db| {
            let tx = db.transaction()?;
            let (content, pin): (String, String) = tx.query_row(
                "SELECT snapshot,pin FROM private_sheets WHERE id=1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            let mut snapshot: PrivateSnapshot = serde_json::from_str(&content)?;
            if let snapshots::Pin::Runtime { schema, .. } = &mut snapshot.pin {
                *schema = 2;
            }
            let mut pin: snapshots::Pin = serde_json::from_str(&pin)?;
            if let snapshots::Pin::Runtime { schema, .. } = &mut pin {
                *schema = 2;
            }
            let content = serde_json::to_string(&snapshot)?;
            let hash = snapshots::hash(content.as_bytes());
            let triggers = ["private_sheets_update", "private_sheet_history_update"].map(|name| {
                tx.query_row("SELECT sql FROM sqlite_schema WHERE name=?1", [name], |r| {
                    r.get::<_, String>(0)
                })
                .unwrap()
            });
            tx.execute_batch(
                "DROP TRIGGER private_sheets_update; DROP TRIGGER private_sheet_history_update;",
            )?;
            tx.execute(
                "UPDATE private_sheets SET pin=?1,snapshot=?2,hash=?3",
                params![serde_json::to_string(&pin)?, content, hash],
            )?;
            tx.execute(
                "UPDATE private_sheet_history SET snapshot=?1,hash=?2",
                params![content, hash],
            )?;
            for sql in triggers {
                tx.execute_batch(&sql)?;
            }
            tx.commit()?;
            Ok(())
        })
        .await
        .unwrap();
    let html = history_page(&setup, 1).await;
    assert!(html.contains("Unsupported immutable interpretation pin"));
    assert!(
        summary_section(&html)
            .unwrap()
            .contains("Change summary unavailable for this sheet.")
    );
}

/// `lists` lists of `properties` integer properties whose labels are 100 `"` (the widest
/// escape).
fn quoted_lists_definition(lists: usize, properties: usize) -> Value {
    let label = "\"".repeat(100);
    let fields: Vec<Value> = (0..lists)
        .map(|l| {
            list(
                &format!("l{l}_{}", "a".repeat(61)),
                &label,
                (0..properties)
                    .map(|p| {
                        property(
                            &format!("p{p:02}_{}", "b".repeat(12)),
                            &label,
                            "integer",
                            false,
                        )
                    })
                    .collect(),
            )
        })
        .collect();
    let ids: Vec<Value> = fields.iter().map(|field| field["id"].clone()).collect();
    json!({
        "contract_version": 1, "game_id": "synthetic-game", "edition_id": "first",
        "definition_id": "quoted-lists", "title": "Quoted lists",
        "sources": [{"source_id": 1, "pdf_sha256": "0".repeat(64), "roles": ["sheet"]}],
        "fields": fields, "formula_profile": 1,
        "groups": [{"id": "main", "label": "Main", "fields": ids, "column_hint": "main"}],
    })
}

/// Plan D8 (N-E): 200 lines of at most 8 labels, one overflow line, and the section bound.
pub(super) const SUMMARY_LINE_BYTES: usize = 8_192;
pub(super) const SUMMARY_OVERFLOW_BYTES: usize = 32_768;
pub(super) const SUMMARY_FIXED_BYTES: usize = 4_096;
pub(super) const SUMMARY_BYTES: usize =
    200 * SUMMARY_LINE_BYTES + SUMMARY_OVERFLOW_BYTES + SUMMARY_FIXED_BYTES;

/// The raw bytes of the change-summary section as served.
fn raw_summary(html: &str) -> &str {
    let start = html
        .find("<section class=\"panel\" id=\"change-summary\"")
        .expect("summary section");
    let end = start + html[start..].find("</section>").unwrap() + "</section>".len();
    &html[start..end]
}

#[tokio::test]
async fn change_summary_large_diff_is_bounded() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    assert_eq!(SUMMARY_BYTES, 1_675_264);
    // Ten properties: 9 of 10 change in an edited row, so each line lists the maximum 8
    // labels plus "and 1 more", not a reused slot.
    let definition = quoted_lists_definition(3, 10);
    let setup = list_setup(&definition).await;
    let ids: Vec<String> = (0..3)
        .map(|l| definition["fields"][l]["id"].as_str().unwrap().to_owned())
        .collect();
    let properties: Vec<String> = (0..10)
        .map(|p| {
            definition["fields"][0]["items"]["properties"][p]["id"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    let rows = |count: usize, revision: i64, value: i64| -> Vec<Value> {
        (1..=count)
            .map(|k| {
                let mut item = json!({"id": format!("r{revision}-{k}")});
                for (p, property) in properties.iter().enumerate() {
                    item[property] = if p == 0 { 0.into() } else { value.into() };
                }
                item
            })
            .collect()
    };
    let mut first = blank(&setup, 1).await;
    first.sheet_id = 1;
    first.data = json!({ids[0].clone(): rows(256, 1, 0), ids[1].clone(): rows(20, 1, 0),
                        ids[2].clone(): rows(20, 1, 0)});
    let first = write(&setup, 1, first, "create").await.unwrap();
    // Lists 0 and 1: every row edited; list 2: every row replaced.
    let second = json!({ids[0].clone(): rows(256, 1, 1), ids[1].clone(): rows(20, 1, 1),
                        ids[2].clone(): rows(20, 2, 0)});
    edit(&setup, &first, second).await.unwrap();
    let response = super::router_request(
        &setup.app,
        &setup.auth[1],
        "/private-sheets/1/history/2",
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let html = String::from_utf8(
        axum::body::to_bytes(response.into_body(), 256 * 1024 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    let lines = summary_lines(&html);
    assert_eq!(lines.len(), 201, "200 lines and the overflow line");
    let page = scraper::Html::parse_document(&html);
    for li in page
        .select(&selector("section#change-summary ul > li"))
        .take(200)
    {
        assert_eq!(li.select(&selector("code")).count(), 9, "entry + 8 labels");
        assert!(li.text().collect::<String>().ends_with(" and 1 more."));
    }
    let label = "\"".repeat(100);
    let overflow = lines.last().unwrap();
    assert_eq!(
        overflow,
        &format!(
            "and 116 more changes. Totals: {label}: 0 added, 0 removed, 256 edited, 0 moved; \
             {label}: 0 added, 0 removed, 20 edited, 0 moved; \
             {label}: 20 added, 20 removed, 0 edited, 0 moved."
        )
    );
    let raw = raw_summary(&html);
    assert!(raw.len() <= SUMMARY_BYTES, "{} bytes", raw.len());
    for line in raw.split("<li>").skip(1).take(200) {
        assert!(line.len() <= SUMMARY_LINE_BYTES, "{} bytes", line.len());
    }
    let overflow_raw = raw.rsplit("<li>").next().unwrap();
    assert!(overflow_raw.len() <= SUMMARY_OVERFLOW_BYTES);
    assert!(
        raw.contains(&"&#34;".repeat(100)),
        "labels escaped at 5 bytes"
    );
}

#[tokio::test]
async fn derived_values_over_lists_update_after_save() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = list_setup(&formula_list_definition()).await;
    abc(&setup).await;
    let html = sheet_page(&setup).await;
    let derived = |html: &str| -> String {
        scraper::Html::parse_document(html)
            .select(&selector("dl.derived dd"))
            .next()
            .unwrap()
            .text()
            .collect()
    };
    assert_eq!(derived(&html), "3");
    let mut form = submitted(&html);
    set(&mut form, "row-1-0-1", "20");
    assert_eq!(
        setup.post("/private-sheets/1", form).await.0,
        StatusCode::SEE_OTHER
    );
    let html = sheet_page(&setup).await;
    assert_eq!(derived(&html), "22");
    assert!(html.contains("Advisory checks: 1 violated"));
    // History recomputes each revision's own result.
    assert_eq!(derived(&history_page(&setup, 1).await), "3");
}

/// Labels carrying markup, and a multiline property for `</textarea>` payloads.
fn hostile_definition() -> Value {
    let mut kind = property("kind", "<script>Kind", "enum", true);
    kind["options"] = json!([{"id": "a", "label": "<script>Option"}, {"id": "b", "label": "B"}]);
    let fields = vec![
        json!({"id": "label", "label": "Label", "control": "text", "nullable": false,
               "mapping": mapped()}),
        list(
            "gear",
            "<script>List",
            vec![
                property("name", "<script>Name", "text", false),
                property("story", "<script>Story", "multiline", false),
                kind,
            ],
        ),
    ];
    json!({
        "contract_version": 1, "game_id": "synthetic-game", "edition_id": "first",
        "definition_id": "hostile", "title": "Hostile",
        "sources": [{"source_id": 1, "pdf_sha256": "0".repeat(64), "roles": ["sheet"]}],
        "fields": fields, "formula_profile": 1,
        "groups": [{"id": "main", "label": "Main", "fields": ["label", "gear"],
                    "column_hint": "main"}],
    })
}

/// No injected element, and every payload reads back verbatim from its own context.
fn assert_inert(html: &str, context: &str, values: &[(&str, &str)]) {
    let page = scraper::Html::parse_document(html);
    for tag in ["script", "b", "i", "u"] {
        assert_eq!(
            page.select(&selector(tag)).count(),
            0,
            "{context}: <{tag}> injected"
        );
    }
    let form = submitted_by(html, None);
    for (key, want) in values {
        assert_eq!(get(&form, key), *want, "{context}: {key}");
    }
}

#[tokio::test]
async fn list_payloads_escaped_in_context() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = list_setup(&hostile_definition()).await;
    let text = "\"'><b>text</b>";
    let story = "</textarea><script>alert(1)</script>";
    let created = create(
        &setup,
        json!({"label": "x", "gear": [{"id": "r1-1", "name": text, "story": story}]}),
    )
    .await
    .unwrap();
    let saved = [("row-1-0-0", text), ("row-1-0-1", story)];
    let view = sheet_page(&setup).await;
    assert_inert(&view, "view", &saved);
    assert!(view.contains("&#60;script&#62;List"), "list label escaped");
    // Op page: a forged hidden ID, an invalid enum value and an invalid Set/Not set state are
    // echoed in attribute values; the status message carries the list label.
    let forged_id = "\"'><i>id</i>";
    let forged_enum = "\"'><u>enum</u>";
    let forged_state = "'\"><u>state</u>";
    let mut form = clicked(&view, "add-1");
    set(&mut form, "id-1-0", forged_id);
    set(&mut form, "row-1-0-2", forged_enum);
    set(&mut form, "set-1-0-2", forged_state);
    let (status, op) = setup.post("/private-sheets/1", form.clone()).await;
    assert_eq!(status, StatusCode::OK);
    let echoed = [
        ("id-1-0", forged_id),
        ("row-1-0-0", text),
        ("row-1-0-1", story),
        ("row-1-0-2", forged_enum),
        ("set-1-0-2", forged_state),
    ];
    assert_inert(&op, "op", &echoed);
    assert_eq!(
        status_text(&op),
        "Row added to <script>List. Not saved yet."
    );
    // 422: the same forged draft saved at the current revision.
    let mut save = form.clone();
    save.retain(|(key, _)| key != "op");
    let (status, rejected) = setup.post("/private-sheets/1", save.clone()).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_inert(&rejected, "422", &echoed);
    // 409: the same draft after a competing write.
    let second = edit(&setup, &created, created.data.clone()).await.unwrap();
    let (status, conflict) = setup.post("/private-sheets/1", save).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_inert(&conflict, "409", &echoed);
    // History and its change summary (labels carry markup).
    let mut changed = second.data.clone();
    changed["gear"][0]["name"] = "renamed".into();
    edit(&setup, &second, changed).await.unwrap();
    let history = history_page(&setup, 3).await;
    let page = scraper::Html::parse_document(&history);
    for tag in ["script", "b", "i", "u"] {
        assert_eq!(page.select(&selector(tag)).count(), 0, "history: <{tag}>");
    }
    assert_eq!(
        summary_lines(&history),
        ["Row gear/r1-1 of <script>List edited: <script>Name gear/r1-1/name."]
    );
    let textarea: String = page
        .select(&selector("textarea[name=\"row-1-0-1\"]"))
        .next()
        .unwrap()
        .text()
        .collect();
    assert_eq!(textarea, story, "history textarea");
    // Ops post to the generated same-origin form action; no button overrides it.
    let parsed = scraper::Html::parse_document(&view);
    assert_eq!(parsed.select(&selector("button[formaction]")).count(), 0);
    assert_eq!(
        parsed
            .select(&selector("form"))
            .next()
            .unwrap()
            .value()
            .attr("action"),
        Some("/private-sheets/1")
    );
}

/// The unpacked-gate list fixture (`list-definition.json`) publishes, round-trips through
/// draft operations and evaluates its list formulas, including one reading `item.id`.
#[tokio::test]
async fn source_fixture_list_definition_saves_and_evaluates() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let mut definition = crate::definitions::source_fixture::list_definition();
    definition["sources"] =
        json!([{"source_id": 1, "pdf_sha256": "0".repeat(64), "roles": ["sheet"]}]);
    definition["fields"][0]["mapping"] = mapped();
    let setup = list_setup(&definition).await;
    let path = create_path(&setup);
    let (_, page) = setup.get(&path).await;
    let (_, page) = setup.post(&path, clicked(&page, "add-1")).await;
    let (_, page) = setup.post(&path, clicked(&page, "add-1")).await;
    let mut form = submitted(&page);
    for (key, value) in [
        ("name", "Fixture"),
        ("value-name", "Fixture"),
        ("row-1-0-0", "Rope"),
        ("row-1-0-2", "2"),
        ("row-1-0-3", "3"),
        ("row-1-0-4", "false"),
        ("row-1-1-0", "Torch"),
        ("row-1-1-2", "1"),
        ("row-1-1-3", "5"),
        ("row-1-1-4", "true"),
    ] {
        set(&mut form, key, value);
    }
    assert_eq!(setup.post(&path, form).await.0, StatusCode::SEE_OTHER);
    let html = sheet_page(&setup).await;
    let derived: Vec<String> = scraper::Html::parse_document(&html)
        .select(&selector("dl.derived dd"))
        .map(|dd| dd.text().collect())
        .collect();
    assert_eq!(derived, ["11", "1"], "load 2*3 + 1*5 and r1-1 kept");
    assert!(html.contains("Advisory checks: 1 violated"), "load 11 > 10");
}

// ---- Step 7: large-form transport (plan D7) ----

/// Page-size bound of a sheet re-render (plan D7, RE3-1): fixed chrome, plus a bounded markup
/// per render unit (a control, option or row element with its label, escaped, including one
/// 64-character excerpt), plus each echoed byte once at the widest escape (`&#34;`, 5 bytes).
/// Derived fields and checks are not in these fixtures; their markup is bounded by the
/// formula layer's expression limits.
pub(super) const FIXED_PAGE_BYTES: usize = 64 * 1024;
pub(super) const UNIT_MARKUP_BYTES: usize = 2_048;

/// Render units of a formula-free list definition with `rows[f]` rows per list (plan D7).
fn render_units(definition: &Value, rows: &[usize]) -> usize {
    let choices = |item: &Value| match item["control"].as_str().unwrap() {
        "enum" => item["options"].as_array().unwrap().len() + 1,
        "boolean" => 3,
        _ => 0,
    };
    definition["fields"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
        .map(|(f, field)| {
            if field["control"] == "list" {
                let per_row: usize = field["items"]["properties"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|p| 1 + choices(p) + if p["optional"] == true { 2 } else { 0 })
                    .sum();
                4 + rows.get(f).copied().unwrap_or_default() * (4 + per_row)
            } else {
                1 + choices(field) + if field["nullable"] == true { 2 } else { 0 }
            }
        })
        .sum()
}

fn page_bound(units: usize) -> usize {
    FIXED_PAGE_BYTES + units * UNIT_MARKUP_BYTES + 5 * super::super::web::ECHO_BYTES
}

async fn big_post(setup: &Setup, path: &str, body: String) -> (StatusCode, String) {
    let response = raw_post(setup, path, body).await;
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024 * 1024)
        .await
        .unwrap();
    (status, String::from_utf8(bytes.to_vec()).unwrap())
}

/// A list-sheet edit form whose notes pad the URL-encoded body to exactly `size` bytes.
fn padded(revision: i64, size: usize) -> String {
    let base = serde_urlencoded::to_string(list_form(
        revision,
        vec![equipment_row(0, "r1-1", "Rope", "1", None, None, "false")],
    ))
    .unwrap();
    let without = base.replace("&notes=&", "&");
    let pad = size - without.len() - "&notes=".len();
    format!("{without}&notes={}", "a".repeat(pad))
}

#[tokio::test]
async fn sheet_post_accepts_5mib_and_413s_5mib_plus_1() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = list_setup(&list_definition()).await;
    create_rows(&setup, 1).await;
    let limit = super::super::web::SHEET_POST_BYTES;
    assert_eq!(limit, 5 * 1024 * 1024);
    let body = padded(1, limit);
    assert_eq!(body.len(), limit);
    let (status, _) = big_post(&setup, "/private-sheets/1", body).await;
    assert_ne!(status, StatusCode::PAYLOAD_TOO_LARGE, "exactly 5 MiB");
    let (status, _) = big_post(&setup, "/private-sheets/1", padded(1, limit + 1)).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    // The create POST has the same limit.
    let path = create_path(&setup);
    let (status, _) = big_post(&setup, &path, padded(0, limit + 1)).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "create");
    let (status, _) = big_post(&setup, &path, padded(0, limit)).await;
    assert_ne!(status, StatusCode::PAYLOAD_TOO_LARGE, "create at the limit");
}

#[tokio::test]
async fn non_sheet_posts_keep_8kib() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = list_setup(&list_definition()).await;
    let body = |size: usize| {
        let base = format!(
            "csrf={}&game_id=synthetic-game&edition_id=first&title=",
            "a".repeat(64)
        );
        format!("{base}{}", "t".repeat(size - base.len()))
    };
    let (status, _) = big_post(&setup, "/campaigns", body(8192)).await;
    assert_ne!(status, StatusCode::PAYLOAD_TOO_LARGE);
    let (status, _) = big_post(&setup, "/campaigns", body(8193)).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    let join = format!("/campaigns/{}/join", setup.campaign);
    let (status, _) = big_post(&setup, &join, format!("csrf={}", "a".repeat(8188))).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "join: 8,193 bytes");
}

#[tokio::test]
async fn unauthenticated_large_post_is_403_without_reading_the_body() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tower::ServiceExt;
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = list_setup(&list_definition()).await;
    create_rows(&setup, 1).await;
    for path in ["/private-sheets/1".to_owned(), create_path(&setup)] {
        let polled = std::sync::Arc::new(AtomicUsize::new(0));
        let counter = std::sync::Arc::clone(&polled);
        let chunks = futures_util::stream::iter((0..320).map(move |_| {
            counter.fetch_add(16 * 1024, Ordering::SeqCst);
            Ok::<_, std::io::Error>(axum::body::Bytes::from(vec![b'a'; 16 * 1024]))
        }));
        let request = axum::http::Request::builder()
            .uri(&path)
            .method("POST")
            .header("host", "127.0.0.1:3000")
            .header("origin", "http://127.0.0.1:3000")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(axum::body::Body::from_stream(chunks))
            .unwrap();
        let response = setup.app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{path}");
        let read = polled.load(Ordering::SeqCst);
        assert!(read <= 64 * 1024, "{path}: {read} bytes polled before 403");
    }
}

#[tokio::test]
async fn stored_over_1mib_is_422_with_draft() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = list_setup(&list_definition()).await;
    create_rows(&setup, 1).await;
    let quotes = "\"".repeat(2_400);
    let rows = (0..256)
        .map(|r| {
            equipment_row(
                r,
                if r == 0 { "r1-1" } else { "" },
                &quotes,
                "1",
                None,
                None,
                "false",
            )
        })
        .collect();
    let form = list_form(1, rows);
    let decoded: usize = form.iter().map(|(_, v)| v.len()).sum();
    assert!(
        decoded <= super::super::web::ECHO_BYTES,
        "within the echo cap"
    );
    let (status, html) = big_post(
        &setup,
        "/private-sheets/1",
        serde_urlencoded::to_string(form).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        status_text(&html).starts_with("No changes saved:"),
        "{}",
        status_text(&html)
    );
    assert_eq!(
        get(&submitted_by(&html, None), "row-1-255-0"),
        quotes,
        "draft kept"
    );
    assert_eq!(reopen(&setup, 1).await.revision, 1, "nothing saved");
}

fn alert_text(html: &str) -> String {
    scraper::Html::parse_document(html)
        .select(&selector("p[role=alert]"))
        .next()
        .map(|element| element.text().collect())
        .unwrap_or_default()
}

#[tokio::test]
async fn oversized_draft_is_not_echoed() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = list_setup(&list_definition()).await;
    create_rows(&setup, 1).await;
    let base = serde_urlencoded::to_string(list_form(
        1,
        vec![equipment_row(0, "r1-1", "Rope", "1", None, None, "false")],
    ))
    .unwrap()
    .replace("&notes=&", "&");
    let quotes = (super::super::web::SHEET_POST_BYTES - base.len() - "&notes=".len()) / 3;
    let body = format!("{base}&notes={}", "%22".repeat(quotes));
    assert!(body.len() <= super::super::web::SHEET_POST_BYTES);
    let (status, html) = big_post(&setup, "/private-sheets/1", body).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        alert_text(&html),
        "This draft is larger than the 1 MiB sheet limit and was not kept. Nothing was saved; copy large text before submitting."
    );
    assert!(
        !html.contains(&"&#34;".repeat(64)),
        "the draft was not echoed"
    );
    assert_eq!(
        get(&submitted_by(&html, None), "row-1-0-0"),
        "Item 1",
        "saved values shown"
    );
    let units = render_units(&list_definition(), &[0, 1, 1]);
    assert!(html.len() <= page_bound(units), "{} bytes", html.len());
    assert_eq!(reopen(&setup, 1).await.revision, 1);
}

#[tokio::test]
async fn one_mib_of_quotes_is_echoed_within_bound() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = list_setup(&list_definition()).await;
    create_rows(&setup, 2).await;
    // Every repeated (maximum-amplification) context gets 1,000 quotes: the hidden ID, an
    // invalid enum value, an invalid boolean and an invalid Set/Not set state, on 256 rows.
    let quotes = "\"".repeat(1_000);
    let rows: Vec<Vec<(String, String)>> = (0..256)
        .map(|r| {
            let mut row = equipment_row(r, &quotes, "n", "1", None, Some(&quotes), &quotes);
            set(&mut row, &format!("set-1-{r}-2"), &quotes);
            row
        })
        .collect();
    let mut form = list_form(1, rows);
    form.push(pair("op", "up-1-1"));
    let decoded: usize = form.iter().map(|(_, v)| v.len()).sum();
    assert!(decoded <= super::super::web::ECHO_BYTES, "{decoded}");
    assert!(decoded > 1_000_000);
    let (status, html) = big_post(
        &setup,
        "/private-sheets/1",
        serde_urlencoded::to_string(form).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    // Each value in full exactly once, plus one 64-character excerpt in its second context
    // (the page chrome, such as the pin JSON, has its own quotes).
    let chrome = sheet_page(&setup).await.matches("&#34;").count();
    let escaped = html.matches("&#34;").count();
    assert_eq!(escaped, chrome + 256 * 4 * (1_000 + 64));
    let excerpt = format!("Invalid submitted value: {}…", "&#34;".repeat(64));
    assert_eq!(
        html.matches(&excerpt).count(),
        256 * 2,
        "enum and boolean excerpts"
    );
    assert_eq!(
        html.matches(&format!("· ID {}…", "&#34;".repeat(64)))
            .count(),
        256,
        "legend ID excerpts"
    );
    let units = render_units(&list_definition(), &[0, 256, 0]);
    assert!(
        html.len() <= page_bound(units),
        "{} > {}",
        html.len(),
        page_bound(units)
    );
}

/// The worst-label sheet (labels of 100 `"`, 256 rows × 32 properties) stays within the
/// per-unit markup bound (no echo).
#[tokio::test]
async fn worst_label_page_markup_is_within_the_unit_bound() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let definition = quoted_lists_definition(1, 32);
    let setup = list_setup(&definition).await;
    let field = &definition["fields"][0];
    let items: Vec<Value> = (1..=256)
        .map(|k| {
            let mut item = json!({"id": format!("r1-{k}")});
            for p in field["items"]["properties"].as_array().unwrap() {
                item[p["id"].as_str().unwrap()] = 0.into();
            }
            item
        })
        .collect();
    let mut snapshot = blank(&setup, 1).await;
    snapshot.sheet_id = 1;
    snapshot.data = json!({field["id"].as_str().unwrap(): items});
    write(&setup, 1, snapshot, "create").await.unwrap();
    let response =
        super::router_request(&setup.app, &setup.auth[1], "/private-sheets/1", None).await;
    let html = axum::body::to_bytes(response.into_body(), 256 * 1024 * 1024)
        .await
        .unwrap();
    let units = render_units(&definition, &[256]);
    assert_eq!(units, 9_220);
    assert!(
        html.len() <= FIXED_PAGE_BYTES + units * UNIT_MARKUP_BYTES,
        "{} bytes for {units} units",
        html.len()
    );
    println!("worst-label view page: {} bytes, {units} units", html.len());
}

/// One stored note: address, op, raw, typed before and after (canonical JSON).
type SeedNote = (String, &'static str, String, String, String);

/// Creates sheet 1 with `data` as a retained (pre-M4) sheet, so list rows may carry any
/// valid 64-character ID, then writes `notes` 256 per revision (the table's limit) on
/// data-unchanged revisions, and returns the served sheet page.
async fn noted_page(definition: &Value, data: Value, notes: Vec<SeedNote>) -> String {
    let setup = list_setup(definition).await;
    legacy_create(&setup, 1, data).await;
    let version = setup.version;
    let auth = setup.auth[1].clone();
    setup
        .store
        .run_campaign(auth.clone(), move |db| {
            for batch in notes.chunks(256) {
                let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
                let pin = crate::definitions::private_pin::validate(
                    crate::definitions::private_pin::load(&tx, version)?,
                )?;
                let live = super::super::snapshots::load(&tx, 1)?;
                let written = super::super::snapshots::write(&tx, &auth, live, "edit", Some(&pin))?;
                for (address, op, raw, before, after) in batch {
                    tx.execute(
                        "INSERT INTO private_sheet_notes(sheet_id,revision,address,op,raw,before,after) VALUES(1,?1,?2,?3,?4,?5,?6)",
                        rusqlite::params![written.revision, address, op, raw, before, after],
                    )?;
                }
                tx.commit()?;
            }
            Ok(())
        })
        .await
        .unwrap();
    let response =
        super::router_request(&setup.app, &setup.auth[1], "/private-sheets/1", None).await;
    String::from_utf8(
        axum::body::to_bytes(response.into_body(), 256 * 1024 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}

fn within_unit_bound(html: &str, units: usize, label: &str) {
    assert!(
        html.len() <= FIXED_PAGE_BYTES + units * UNIT_MARKUP_BYTES,
        "{label}: {} bytes for {units} units",
        html.len()
    );
    println!(
        "worst-label page with {label}: {} bytes, {units} units (bound {})",
        html.len(),
        FIXED_PAGE_BYTES + units * UNIT_MARKUP_BYTES
    );
}

fn int(value: i64) -> String {
    format!("{{\"int\":\"{value}\"}}")
}

/// The typed note value of a number, or of unknown (`None`, slice 1.4).
fn typed(value: Option<i64>) -> String {
    value.map_or_else(|| r#"{"unknown":true}"#.to_owned(), int)
}

/// The stored value of a number, or `null` for unknown.
fn stored(value: Option<i64>) -> Value {
    value.map_or(Value::Null, Value::from)
}

/// One list with a 64-character ID and 32 properties with 64-character IDs and labels of
/// 100 `"`: all integers, or 31 integers and an enum (`with_enum`) whose options are a
/// 64-character ID and `b`.
fn wide_list_definition(with_enum: bool) -> Value {
    let label = "\"".repeat(100);
    let integers = if with_enum { 31 } else { 32 };
    let mut properties: Vec<Value> = (0..integers)
        .map(|p| {
            property(
                &format!("p{p:02}_{}", "b".repeat(60)),
                &label,
                "integer",
                false,
            )
        })
        .collect();
    if with_enum {
        let mut choice = property(&format!("p31_{}", "b".repeat(60)), &label, "enum", false);
        choice["options"] =
            json!([{"id": "o".repeat(64), "label": label}, {"id": "b", "label": label}]);
        properties.push(choice);
    }
    let fields = vec![list(&format!("l0_{}", "a".repeat(61)), &label, properties)];
    let ids: Vec<Value> = fields.iter().map(|field| field["id"].clone()).collect();
    json!({
        "contract_version": 1, "game_id": "synthetic-game", "edition_id": "first",
        "definition_id": "wide-lists", "title": "Wide lists",
        "sources": [{"source_id": 1, "pdf_sha256": "0".repeat(64), "roles": ["sheet"]}],
        "fields": fields, "formula_profile": 1,
        "groups": [{"id": "main", "label": "Main", "fields": ids, "column_hint": "main"}],
    })
}

/// The densest long sign-led sum: 63 one-digit terms (each shown as " − 1", 3 of its 6 bytes
/// a minus sign) and a 19-digit last term, 146 characters, 64 numbers; its value is
/// −9,223,372,036,854,775,063. Over `i64::MAX` before, the switch offers 744.
fn densest_sign_led() -> (String, i64, i64) {
    let raw = format!("{}-9223372036854775000", "-1".repeat(63));
    let value = super::super::entry::number(&raw).unwrap();
    assert_eq!(value, -9_223_372_036_854_775_063);
    (raw, i64::MAX, value)
}

/// The wide list with 256 rows: every integer cell holds `value` (`None`: unknown) with the
/// note `note` (op, raw, before); with `enum_notes` the list has its enum column, every enum
/// cell holding `b` and noted as changed from the 64-character option.
async fn wide_list_page(
    value: Option<i64>,
    note: (&'static str, &str, Option<i64>),
    enum_notes: bool,
) -> (String, usize) {
    let definition = wide_list_definition(enum_notes);
    let field = &definition["fields"][0];
    let list = field["id"].as_str().unwrap().to_owned();
    let properties: Vec<(String, bool)> = field["items"]["properties"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| (p["id"].as_str().unwrap().to_owned(), p["control"] == "enum"))
        .collect();
    let mut notes = Vec::new();
    let items: Vec<Value> = (1..=256)
        .map(|k| {
            // The longest row ID a retained row may carry: 64 characters.
            let host = format!("i{k:03}-{}", "x".repeat(59));
            let mut item = json!({"id": host});
            for (p, choice) in &properties {
                let address = format!("cell:{list}/{host}/{p}");
                if *choice {
                    item[p] = json!("b");
                    if enum_notes {
                        let before = format!("{{\"enum\":\"{}\"}}", "o".repeat(64));
                        let after = "{\"enum\":\"b\"}".to_owned();
                        notes.push((address, "literal", String::new(), before, after));
                    }
                } else {
                    item[p] = stored(value);
                    let (op, raw, before) = note;
                    notes.push((address, op, raw.to_owned(), typed(before), typed(value)));
                }
            }
            item
        })
        .collect();
    let html = noted_page(&definition, json!({list: items}), notes).await;
    (html, render_units(&definition, &[256]))
}

/// Slice 1.3 (review rounds 1 and 2, Sol Major): the switch, Undo and note markup keeps the
/// per-unit page bound in its worst case. 64-character list and property IDs, labels of 100
/// `"`, 256 rows; every integer cell shows the densest long sign-led sum (cut at a term
/// boundary), a relative offer and Undo with a 19-digit value before; every enum cell offers
/// Undo back to a 64-character option ID.
#[tokio::test]
async fn worst_label_page_with_every_switch_and_undo_is_within_the_unit_bound() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let (raw, before, value) = densest_sign_led();
    let (html, units) =
        wide_list_page(Some(value), ("sign_literal", &raw, Some(before)), true).await;
    assert_eq!(html.matches("form=\"assist-switch\"").count(), 31 * 256);
    assert_eq!(html.matches("form=\"assist-undo\"").count(), 32 * 256);
    assert_eq!(html.matches("class=\"entry-note\"").count(), 31 * 256);
    within_unit_bound(&html, units, "every long sign-led sum, switch and Undo");
    // Without the enum column (fewer units, so a tighter bound): every cell sign-led.
    let (html, units) =
        wide_list_page(Some(value), ("sign_literal", &raw, Some(before)), false).await;
    assert_eq!(units, 9_220);
    assert_eq!(html.matches("form=\"assist-switch\"").count(), 32 * 256);
    within_unit_bound(&html, units, "32 sign-led columns, switch and Undo");
}

/// The same for slice 1.2's plain sum note (`11+11…`, 64 numbers) with Undo on every cell.
#[tokio::test]
async fn worst_label_page_with_every_long_sum_is_within_the_unit_bound() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let raw = format!("1{}", "+1".repeat(63));
    let (html, units) = wide_list_page(Some(64), ("sum", &raw, Some(i64::MIN)), false).await;
    assert_eq!(units, 9_220);
    assert_eq!(html.matches("form=\"assist-undo\"").count(), 32 * 256);
    assert_eq!(html.matches("class=\"entry-note\"").count(), 32 * 256);
    within_unit_bound(&html, units, "every long sum and Undo");
}

/// The same for slice 1.2's scribble: 200 `"` (escaped five times wider) kept over every
/// integer cell. The scribble is echoed in full once, in the cell's control; its note is cut.
#[tokio::test]
async fn worst_label_page_with_every_long_scribble_is_within_the_unit_bound() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let raw = "\"".repeat(200);
    let (html, units) =
        wide_list_page(Some(i64::MIN), ("unresolved", &raw, Some(i64::MIN)), false).await;
    assert_eq!(units, 9_220);
    assert_eq!(html.matches("not understood").count(), 32 * 256);
    within_unit_bound(&html, units, "every long scribble");
}

/// 256 integer scalar fields with 64-character IDs and labels of 100 `"` (each charged one
/// unit), every one holding `value` (`None`: unknown) with the note (op, raw, before).
async fn wide_scalar_page(
    value: Option<i64>,
    note: (&'static str, &str, Option<i64>),
) -> (String, usize) {
    let label = "\"".repeat(100);
    let ids: Vec<String> = (0..256)
        .map(|f| format!("f{f:03}_{}", "x".repeat(59)))
        .collect();
    let fields: Vec<Value> = ids
        .iter()
        .map(|id| {
            json!({"id": id, "label": label, "control": "integer", "nullable": false,
                   "mapping": mapped()})
        })
        .collect();
    let definition = json!({
        "contract_version": 1, "game_id": "synthetic-game", "edition_id": "first",
        "definition_id": "wide-scalars", "title": "Wide scalars",
        "sources": [{"source_id": 1, "pdf_sha256": "0".repeat(64), "roles": ["sheet"]}],
        "fields": fields,
        "groups": [{"id": "main", "label": "Main", "fields": ids, "column_hint": "main"}],
    });
    let (op, raw, before) = note;
    let mut data = serde_json::Map::new();
    let mut notes = Vec::new();
    for id in &ids {
        data.insert(id.clone(), stored(value));
        notes.push((
            format!("field:{id}"),
            op,
            raw.to_owned(),
            typed(before),
            typed(value),
        ));
    }
    let html = noted_page(&definition, Value::Object(data), notes).await;
    let units = render_units(&definition, &[]);
    assert_eq!(units, 256);
    (html, units)
}

/// Review round 2 (Sol Major): scalar fields, each showing the densest long sign-led sum,
/// the offer and Undo.
#[tokio::test]
async fn worst_label_scalar_page_with_every_switch_and_undo_is_within_the_unit_bound() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let (raw, before, value) = densest_sign_led();
    let (html, units) = wide_scalar_page(Some(value), ("sign_literal", &raw, Some(before))).await;
    assert_eq!(html.matches("form=\"assist-switch\"").count(), 256);
    assert_eq!(html.matches("form=\"assist-undo\"").count(), 256);
    within_unit_bound(&html, units, "256 wide scalars with switch and Undo");
}

/// Review round 3 (Sonnet): scalar fields, each with a 200-`"` scribble kept over an
/// `i64::MIN` value; the scribble is echoed in full in the input.
#[tokio::test]
async fn worst_label_scalar_page_with_every_long_scribble_is_within_the_unit_bound() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let raw = "\"".repeat(200);
    let (html, units) =
        wide_scalar_page(Some(i64::MIN), ("unresolved", &raw, Some(i64::MIN))).await;
    assert_eq!(html.matches("not understood").count(), 256);
    assert_eq!(
        html.matches(&"&#34;".repeat(200)).count(),
        256,
        "echoed in full"
    );
    within_unit_bound(&html, units, "256 wide scalars with a long scribble");
}

/// Slice 1.4: unknown values add a "?" placeholder to empty integer controls and "?
/// (unknown)" notes. Worst cases: every integer cell erased to unknown with Undo back to a
/// 19-digit value (and every enum cell with Undo), and every integer cell unknown under a
/// 200-`"` scribble, on the wide list and on wide scalars.
#[tokio::test]
async fn worst_label_pages_with_every_unknown_are_within_the_unit_bound() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let erased = ("erased", "", Some(i64::MIN));
    let (html, units) = wide_list_page(None, erased, true).await;
    assert_eq!(html.matches("placeholder=\"?\"").count(), 31 * 256);
    assert_eq!(html.matches("unknown-hint").count(), 31 * 256 + 1);
    assert_eq!(html.matches("form=\"assist-undo\"").count(), 32 * 256);
    within_unit_bound(&html, units, "every value erased, with Undo");
    let (html, units) = wide_list_page(None, erased, false).await;
    assert_eq!(html.matches("form=\"assist-undo\"").count(), 32 * 256);
    within_unit_bound(&html, units, "32 erased columns, with Undo");
    let raw = "\"".repeat(200);
    let (html, units) = wide_list_page(None, ("unresolved", &raw, None), false).await;
    assert_eq!(html.matches("? (unknown) · ").count(), 32 * 256);
    within_unit_bound(&html, units, "every unknown with a long scribble");
    let (html, units) = wide_scalar_page(None, erased).await;
    assert_eq!(html.matches("placeholder=\"?\"").count(), 256);
    assert_eq!(html.matches("unknown-hint").count(), 256 + 1);
    assert_eq!(html.matches("form=\"assist-undo\"").count(), 256);
    within_unit_bound(&html, units, "256 wide scalars erased, with Undo");
    let (html, units) = wide_scalar_page(None, ("unresolved", &raw, None)).await;
    assert_eq!(html.matches("? (unknown) · ").count(), 256);
    within_unit_bound(
        &html,
        units,
        "256 wide scalars unknown with a long scribble",
    );
}

// ---- Step 8: the gate lift, the publication budget and end-to-end lists (plan D7) ----

/// List `l` of the budget fixture: 32 properties, of which `BUDGET_OPTIONAL[l]` leading ones
/// are optional integers, list 0's last property a required boolean, the rest integers.
const BUDGET_OPTIONAL: [usize; 3] = [3, 3, 2];

fn budget_property(l: usize, p: usize) -> Value {
    if l == 0 && p == 31 {
        property("p31", "P31", "boolean", false)
    } else {
        property(
            &format!("p{p}"),
            &format!("P{p}"),
            "integer",
            p < BUDGET_OPTIONAL[l],
        )
    }
}

/// Exactly `RENDER_UNITS` render units plus `extra` one-unit text fields (admission allows
/// 2,048 enum options in all, so the budget is reached through list rows): three lists of 32
/// properties, 12 + 256 × (45 + 42 + 40) = 32,524 units, plus 244 texts.
fn budget_definition(extra: usize) -> Value {
    let mut fields: Vec<Value> = (0..3)
        .map(|l| {
            list(
                &format!("l{l}"),
                &format!("List {l}"),
                (0..32).map(|p| budget_property(l, p)).collect(),
            )
        })
        .collect();
    for t in 0..244 + extra {
        fields.push(
            json!({"id": format!("t{t}"), "label": format!("T{t}"), "control": "text",
                           "nullable": false, "mapping": mapped()}),
        );
    }
    let ids: Vec<Value> = fields.iter().map(|field| field["id"].clone()).collect();
    json!({
        "contract_version": 1, "game_id": "synthetic-game", "edition_id": "first",
        "definition_id": "budget", "title": "Budget",
        "sources": [{"source_id": 1, "pdf_sha256": "0".repeat(64), "roles": ["sheet"]}],
        "fields": fields, "formula_profile": 1,
        "groups": [{"id": "main", "label": "Main", "fields": ids, "column_hint": "main"}],
    })
}

#[tokio::test]
async fn render_budget_boundary_publishes_and_plus_one_refuses() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let at = budget_definition(0);
    let pin = crate::definitions::private_pin::for_tests(&at);
    let counts = super::values::form_budget(pin.definition());
    assert_eq!(counts.units, super::values::RENDER_UNITS);
    let setup = list_setup(&at).await;
    assert!(setup.version > 0, "the boundary definition published");
    let mut over = budget_definition(1);
    over["definition_id"] = "budget-over".into();
    let draft = setup
        .store
        .save_definition_draft(serde_json::to_vec(&over).unwrap(), 0)
        .await
        .unwrap();
    let comparison = setup
        .store
        .compare_definition(draft.id, false)
        .await
        .unwrap();
    let error = setup
        .store
        .approve_definition(
            draft.id,
            false,
            crate::definitions::ApprovalRequest {
                comparison_hash: comparison.comparison_hash,
                actor: "Synthetic test".into(),
                rationale: "Synthetic original compared".into(),
                originals_compared: true,
            },
        )
        .await
        .unwrap_err();
    match error {
        crate::definitions::DefinitionError::Invalid(report) => {
            assert_eq!(report.diagnostics[0].code, "list-form-exceeds-limit");
        }
        other => panic!("want Invalid, got {other:?}"),
    }
}

/// Every node of a JSON value, counted like the decoder (one per value).
fn json_nodes(value: &Value) -> usize {
    1 + match value {
        Value::Array(items) => items.iter().map(json_nodes).sum(),
        Value::Object(map) => map.values().map(json_nodes).sum(),
        _ => 0,
    }
}

async fn timed_get(setup: &Setup, path: &str) -> (StatusCode, usize, std::time::Duration) {
    let started = std::time::Instant::now();
    let response = super::router_request(&setup.app, &setup.auth[2], path, None).await;
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 256 * 1024 * 1024)
        .await
        .unwrap();
    (status, bytes.len(), started.elapsed())
}

/// The full exact-budget form: every row present and values up to the stored cap.
fn budget_form(revision: i64, op: Option<&str>) -> Vec<(String, String)> {
    let mut form = vec![
        pair("csrf", "a".repeat(64)),
        pair("revision", revision.to_string()),
        pair("name", "Exact budget"),
        pair("notes", ""),
    ];
    for t in 0..244 {
        form.push(pair(format!("value-t{t}"), "x".repeat(1_200)));
    }
    for (f, optional) in BUDGET_OPTIONAL.iter().enumerate() {
        for r in 0..256 {
            form.push(pair(format!("id-{f}-{r}"), ""));
            for p in 0..32 {
                let value = if f == 0 && p == 31 {
                    "true".to_owned()
                } else {
                    i64::MIN.to_string()
                };
                form.push(pair(format!("row-{f}-{r}-{p}"), value));
                if p < *optional {
                    form.push(pair(format!("set-{f}-{r}-{p}"), "present"));
                }
            }
        }
    }
    if let Some(op) = op {
        form.push(pair("op", op));
    }
    form
}

/// Posts `form` as `auth[2]`, printing the status, body bytes and time (report-only).
async fn measured_post(
    setup: &Setup,
    path: &str,
    form: &[(String, String)],
    what: &str,
) -> (StatusCode, usize) {
    use tower::ServiceExt;
    let body = serde_urlencoded::to_string(form).unwrap();
    let request = axum::http::Request::builder()
        .uri(path)
        .method("POST")
        .header("host", "127.0.0.1:3000")
        .header("cookie", setup.auth[2].fixture_cookie())
        .header("origin", "http://127.0.0.1:3000")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(axum::body::Body::from(body.clone()))
        .unwrap();
    let started = std::time::Instant::now();
    let response = setup.app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 256 * 1024 * 1024)
        .await
        .unwrap()
        .len();
    println!(
        "{what}: {} pairs, {} body bytes -> {status}, {bytes} page bytes in {:?}",
        form.len(),
        body.len(),
        started.elapsed()
    );
    (status, bytes)
}

/// The exact-budget definition (test 45's boundary) with every row present and values up to
/// the stored cap saves through the form, reopens and freezes; its envelope stays within
/// `ENVELOPE_NODES`; its pages stay within the D7 and D8 bounds (sizes and times printed,
/// report-only).
#[tokio::test]
async fn exact_budget_sheet_saves_reopens_and_freezes() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = list_setup(&budget_definition(0)).await;
    join_player(&setup).await;
    let path = create_path(&setup);
    let bound = FIXED_PAGE_BYTES + super::values::RENDER_UNITS * UNIT_MARKUP_BYTES;
    let (status, bytes) = measured_post(
        &setup,
        &path,
        &budget_form(0, Some("up-0-1")),
        "exact-budget op",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(bytes <= bound + 5 * super::super::web::ECHO_BYTES);
    let mut invalid = budget_form(0, None);
    set(&mut invalid, "row-0-0-31", "maybe");
    let (status, bytes) = measured_post(&setup, &path, &invalid, "exact-budget 422").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(bytes <= bound + 5 * super::super::web::ECHO_BYTES);
    let (status, _) =
        measured_post(&setup, &path, &budget_form(0, None), "exact-budget save").await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let saved = reopen(&setup, 2).await;
    let content = serde_json::to_string(&saved).unwrap();
    assert!(content.len() <= snapshots::MAX_BYTES, "{}", content.len());
    let envelope =
        crate::definitions::decode::sheet_nodes(content.as_bytes()) - json_nodes(&saved.data);
    println!(
        "exact-budget snapshot: {} bytes, envelope nodes {envelope}",
        content.len()
    );
    assert!(envelope <= super::values::ENVELOPE_NODES, "{envelope}");
    let (status, bytes, time) = timed_get(&setup, "/private-sheets/1").await;
    assert_eq!(status, StatusCode::OK);
    println!("exact-budget GET: {bytes} bytes in {time:?}");
    assert!(bytes <= bound);
    // Test 34 rerun on this fixture: every row edited in 31 of 32 properties.
    let mut snapshot = saved.clone();
    for l in 0..3 {
        for item in snapshot.data[format!("l{l}")].as_array_mut().unwrap() {
            for p in 1..31 {
                item[format!("p{p}")] = 0.into();
            }
            item["p31"] = if l == 0 { false.into() } else { 0.into() };
        }
    }
    write(&setup, 2, snapshot, "edit").await.unwrap();
    let response = super::router_request(
        &setup.app,
        &setup.auth[2],
        "/private-sheets/1/history/2",
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let html = String::from_utf8(
        axum::body::to_bytes(response.into_body(), 256 * 1024 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    let summary = raw_summary(&html);
    println!(
        "exact-budget change summary: {} bytes, history page {} bytes",
        summary.len(),
        html.len()
    );
    assert!(summary.len() <= SUMMARY_BYTES);
    assert_eq!(summary_lines(&html).len(), 201);
    remove_player(&setup).await;
    let frozen = reopen(&setup, 2).await;
    assert_eq!((frozen.state.as_str(), frozen.revision), ("frozen", 3));
}

/// The Lantern Reach pack, published for real, created through the sheet form (its item IDs
/// become host IDs r1-k), shows the character's stored derived values.
#[tokio::test]
async fn lantern_reach_list_sheet_end_to_end() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let pack: Value =
        serde_json::from_slice(crate::definitions::formula_pack_tests::DEFINITION).unwrap();
    let mut definition = crate::definitions::formula_pack_tests::envelope(&pack);
    definition["sources"] =
        json!([{"source_id": 1, "pdf_sha256": "0".repeat(64), "roles": ["sheet"]}]);
    definition["fields"][0]["mapping"] = mapped();
    definition["edition_id"] = "first".into();
    definition["game_id"] = "synthetic-game".into();
    let setup = list_setup(&definition).await;
    let character: Value =
        serde_json::from_slice(crate::definitions::formula_pack_tests::CHARACTER).unwrap();
    let values = &character["values"];
    let pin = crate::definitions::private_pin::for_tests(&definition);
    let mut form = vec![
        pair("csrf", "a".repeat(64)),
        pair("revision", "0"),
        pair("name", "Bram Holt"),
        pair("notes", ""),
    ];
    let mut expected = Vec::new();
    for (f, field) in pin.definition().fields.iter().enumerate() {
        let value = &values[&field.id];
        if field.formula.is_some() {
            expected.push(match value {
                Value::Bool(true) => "True".to_owned(),
                Value::Bool(false) => "False".to_owned(),
                Value::String(text) => text.clone(),
                other => other.to_string(),
            });
        } else if field.control == crate::definitions::Control::List {
            for (r, item) in value.as_array().unwrap().iter().enumerate() {
                form.push(pair(format!("id-{f}-{r}"), ""));
                for (p, property) in super::values::properties(field).iter().enumerate() {
                    let raw = item.get(&property.id).map(|v| match v {
                        Value::String(text) => text.clone(),
                        other => other.to_string(),
                    });
                    form.push(pair(
                        format!("row-{f}-{r}-{p}"),
                        raw.clone().unwrap_or_default(),
                    ));
                    if property.optional {
                        form.push(pair(
                            format!("set-{f}-{r}-{p}"),
                            if raw.is_some() { "present" } else { "absent" },
                        ));
                    }
                }
            }
        } else {
            form.push(pair(
                format!("value-{}", field.id),
                match value {
                    Value::String(text) => text.clone(),
                    other => other.to_string(),
                },
            ));
        }
    }
    assert_eq!(
        setup.post(&create_path(&setup), form).await.0,
        StatusCode::SEE_OTHER
    );
    let saved = reopen(&setup, 1).await;
    let ids: Vec<&str> = saved.data["equipment"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["r1-1", "r1-2", "r1-3", "r1-4", "r1-5"]);
    let html = sheet_page(&setup).await;
    let derived: Vec<String> = scraper::Html::parse_document(&html)
        .select(&selector("dl.derived dd"))
        .map(|dd| dd.text().collect())
        .collect();
    assert_eq!(derived, expected, "derived results unchanged by the re-ID");
    assert!(html.contains("Advisory checks: 0 violated"));
}

/// Plan §6 (report-only, with a bound): a slow 5 MiB upload by a signed-in principal holds the
/// runtime (JOBS) permit, taken before the body, for at most the 5 s request timeout.
#[tokio::test]
async fn slow_5mib_upload_holds_the_runtime_permit_at_most_the_request_timeout() {
    use tower::ServiceExt;
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = list_setup(&list_definition()).await;
    create_rows(&setup, 1).await;
    // 5 MiB in 64 chunks, one every 100 ms: the body would take 6.4 s.
    let chunk = super::super::web::SHEET_POST_BYTES / 64;
    let chunks = futures_util::stream::unfold(0_usize, move |sent| async move {
        if sent == 64 {
            return None;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        Some((
            Ok::<_, std::io::Error>(axum::body::Bytes::from(vec![b'a'; chunk])),
            sent + 1,
        ))
    });
    let request = axum::http::Request::builder()
        .uri("/private-sheets/1")
        .method("POST")
        .header("host", "127.0.0.1:3000")
        .header("cookie", setup.auth[1].fixture_cookie())
        .header("origin", "http://127.0.0.1:3000")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(axum::body::Body::from_stream(chunks))
        .unwrap();
    let started = std::time::Instant::now();
    let upload = tokio::spawn(setup.app.clone().oneshot(request));
    let mut blocked = false;
    let held = loop {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        match crate::definitions::admit().ok() {
            Some(permit) if blocked => {
                drop(permit);
                break started.elapsed();
            }
            Some(permit) => drop(permit),
            None if started.elapsed() > std::time::Duration::from_secs(20) => {
                panic!("the runtime permit was never released")
            }
            None => blocked = true,
        }
    };
    let response = upload.await.unwrap().unwrap();
    println!(
        "slow 5 MiB upload: runtime permit held {held:?}, response {}",
        response.status()
    );
    assert!(held <= std::time::Duration::from_millis(5_600), "{held:?}");
}

// ---- Implementation-review fixes (2026-10-17) ----

/// Sol Major 1: a malformed submitted revision is echoed in full once (the hidden control);
/// the visible text shows a 64-character excerpt, so the page stays within the RE3-1 bound.
#[tokio::test]
async fn malformed_near_cap_revision_is_echoed_once() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = list_setup(&list_definition()).await;
    create_rows(&setup, 1).await;
    let chrome = sheet_page(&setup).await.matches("&#34;").count();
    let quotes = "\"".repeat(1_000_000);
    let mut form = list_form(
        1,
        vec![equipment_row(0, "r1-1", "Rope", "1", None, None, "false")],
    );
    set(&mut form, "revision", &quotes);
    let (status, html) = big_post(
        &setup,
        "/private-sheets/1",
        serde_urlencoded::to_string(form).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(reopen(&setup, 1).await.revision, 1, "nothing saved");
    assert_eq!(
        get(&submitted_by(&html, None), "revision"),
        quotes,
        "hidden value kept"
    );
    assert_eq!(html.matches("&#34;").count(), chrome + 1_000_000 + 64);
    let units = render_units(&list_definition(), &[0, 1, 0]);
    assert!(html.len() <= page_bound(units), "{} bytes", html.len());
}

/// Sol Major 2: values that start with one or two newlines survive every textarea round
/// trip (a parser drops the first newline after `<textarea>`): a draft op, a rejected save,
/// a saved reload and an unchanged second save.
#[tokio::test]
async fn leading_newlines_survive_ops_rejected_saves_and_reloads() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let mut definition = hostile_definition();
    definition["fields"].as_array_mut().unwrap().push(
        json!({"id": "essay", "label": "Essay", "control": "multiline",
                     "nullable": false, "mapping": mapped()}),
    );
    definition["groups"][0]["fields"]
        .as_array_mut()
        .unwrap()
        .push("essay".into());
    let setup = list_setup(&definition).await;
    let mut snapshot = blank(&setup, 1).await;
    snapshot.sheet_id = 1;
    snapshot.notes = "\nnotes".into();
    snapshot.data = json!({"label": "x", "essay": "\n\nscalar",
                           "gear": [{"id": "r1-1", "name": "n", "story": "\nrow"},
                                    {"id": "r1-2", "name": "m", "story": "\n\nsecond"}]});
    let created = write(&setup, 1, snapshot, "create").await.unwrap();
    let expect = |html: &str, context: &str| {
        let form = submitted_by(html, None);
        for (key, want) in [
            ("notes", "\nnotes"),
            ("value-essay", "\n\nscalar"),
            ("row-1-0-1", "\nrow"),
            ("row-1-1-1", "\n\nsecond"),
        ] {
            assert_eq!(get(&form, key), want, "{context}: {key}");
        }
    };
    let view = sheet_page(&setup).await;
    expect(&view, "saved reload");
    let (status, op) = setup
        .post("/private-sheets/1", clicked(&view, "add-1"))
        .await;
    assert_eq!(status, StatusCode::OK);
    expect(&op, "draft op");
    let mut rejected = submitted(&view);
    set(&mut rejected, "row-1-0-2", "not-an-option");
    set(&mut rejected, "set-1-0-2", "present");
    let (status, page) = setup.post("/private-sheets/1", rejected).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    expect(&page, "rejected save");
    // The unchanged page is a no-op: leading newlines compare equal (D-U).
    assert_eq!(
        setup.post("/private-sheets/1", submitted(&view)).await.0,
        StatusCode::OK
    );
    let saved = reopen(&setup, 1).await;
    assert_eq!(saved.revision, 1);
    assert_eq!(
        (saved.data, saved.notes),
        (created.data, created.notes),
        "unchanged save"
    );
    expect(&sheet_page(&setup).await, "second reload");
}

/// Third F1 (adjudicated: the limit is enforced on the body): a signed-in principal streaming
/// more than 5 MiB with no Content-Length gets 413.
#[tokio::test]
async fn streamed_body_over_5mib_without_length_is_413() {
    use tower::ServiceExt;
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = list_setup(&list_definition()).await;
    create_rows(&setup, 1).await;
    let chunk = 64 * 1024;
    let count = super::super::web::SHEET_POST_BYTES / chunk + 2;
    let chunks = futures_util::stream::iter(
        (0..count)
            .map(move |_| Ok::<_, std::io::Error>(axum::body::Bytes::from(vec![b'a'; chunk]))),
    );
    let request = axum::http::Request::builder()
        .uri("/private-sheets/1")
        .method("POST")
        .header("host", "127.0.0.1:3000")
        .header("cookie", setup.auth[1].fixture_cookie())
        .header("origin", "http://127.0.0.1:3000")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(axum::body::Body::from_stream(chunks))
        .unwrap();
    assert!(request.headers().get("content-length").is_none());
    let response = setup.app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

/// Third F4: on a list whose first property is optional, Add focuses that property's
/// Set/Not set select, and nothing else.
#[tokio::test]
async fn add_focuses_the_set_select_of_an_optional_first_property() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let mut definition = list_definition();
    definition["fields"][2]["items"]["properties"]
        .as_array_mut()
        .unwrap()
        .insert(0, property("note", "Note", "text", true));
    let setup = list_setup(&definition).await;
    let (_, page) = setup.get(&create_path(&setup)).await;
    let (status, added) = setup
        .post(&create_path(&setup), clicked(&page, "add-2"))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(autofocus(&added), ["set-2-0-0"]);
}

/// Third F3 calibration: the opposite row shape to the integer one, every property an
/// optional boolean (a value select and a Set/Not set select), stays within the unit bound.
/// Twenty properties keep the definition within the publication budget (31,748 units).
#[tokio::test]
async fn all_optional_worst_label_page_is_within_the_unit_bound() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let label = "\"".repeat(100);
    let properties: Vec<Value> = (0..20)
        .map(|p| {
            property(
                &format!("p{p:02}_{}", "b".repeat(12)),
                &label,
                "boolean",
                true,
            )
        })
        .collect();
    let mut definition = quoted_lists_definition(1, 20);
    definition["fields"][0]["items"]["properties"] = Value::Array(properties);
    let setup = list_setup(&definition).await;
    let field = &definition["fields"][0];
    let items: Vec<Value> = (1..=256)
        .map(|k| {
            let mut item = json!({"id": format!("r1-{k}")});
            for p in field["items"]["properties"].as_array().unwrap() {
                item[p["id"].as_str().unwrap()] = true.into();
            }
            item
        })
        .collect();
    let mut snapshot = blank(&setup, 1).await;
    snapshot.sheet_id = 1;
    snapshot.data = json!({field["id"].as_str().unwrap(): items});
    write(&setup, 1, snapshot, "create").await.unwrap();
    let response =
        super::router_request(&setup.app, &setup.auth[1], "/private-sheets/1", None).await;
    let html = axum::body::to_bytes(response.into_body(), 256 * 1024 * 1024)
        .await
        .unwrap();
    let units = render_units(&definition, &[256]);
    println!(
        "all-optional worst-label page: {} bytes, {units} units",
        html.len()
    );
    assert!(html.len() <= FIXED_PAGE_BYTES + units * UNIT_MARKUP_BYTES);
}

/// Re-review (Sonnet minor 1, Space Bunny N1): private NATIVE sheets keep leading newlines in
/// their textareas through a rejected save, a reload and an unchanged save (native sheets have
/// no draft operations).
#[tokio::test]
async fn native_sheet_textareas_keep_leading_newlines() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = list_setup(&list_definition()).await;
    let campaign = setup
        .store
        .create_campaign(
            setup.auth[1].clone(),
            "Native".into(),
            "mork-borg".into(),
            "it-2025".into(),
        )
        .await
        .unwrap();
    let mut native = super::native(campaign, 2);
    native.data["description"] = "\ndescription".into();
    native.data["weapon_one"] = "\n\nweapon".into();
    native.data["equipment"] = "\nequipment".into();
    let created = super::insert_native(&setup.store, setup.auth[1].clone(), native).await;
    let expect = |html: &str, context: &str| {
        let form = submitted_by(html, None);
        for (key, want) in [
            ("description", "\ndescription"),
            ("weapon_one", "\n\nweapon"),
            ("equipment", "\nequipment"),
        ] {
            assert_eq!(get(&form, key), want, "{context}: {key}");
        }
    };
    let view = sheet_page(&setup).await;
    expect(&view, "saved reload");
    let mut rejected = submitted(&view);
    set(&mut rejected, "agility", "x");
    let (status, page) = setup.post("/private-sheets/1", rejected).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    expect(&page, "rejected save");
    // The unchanged page is a no-op: leading newlines compare equal (D-U).
    assert_eq!(
        setup.post("/private-sheets/1", submitted(&view)).await.0,
        StatusCode::OK
    );
    let saved = reopen(&setup, 1).await;
    assert_eq!(saved.revision, 1);
    for key in ["description", "weapon_one", "equipment"] {
        assert_eq!(saved.data[key], created.data[key], "unchanged save: {key}");
    }
    expect(&sheet_page(&setup).await, "second reload");
}

/// Final review (Sol): native form submission serialises textarea line breaks as CR LF. The
/// transport normalises CR LF and CR to LF, so an unchanged browser Save stores the same
/// bytes and a `%0D%0A` body is stored with LF.
#[tokio::test]
async fn browser_crlf_submission_is_stored_as_lf() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let mut definition = hostile_definition();
    definition["fields"].as_array_mut().unwrap().push(
        json!({"id": "essay", "label": "Essay", "control": "multiline",
                     "nullable": false, "mapping": mapped()}),
    );
    definition["groups"][0]["fields"]
        .as_array_mut()
        .unwrap()
        .push("essay".into());
    let setup = list_setup(&definition).await;
    let mut snapshot = blank(&setup, 1).await;
    snapshot.sheet_id = 1;
    snapshot.notes = "\nnotes\nsecond line".into();
    snapshot.data = json!({"label": "x", "essay": "\n\nscalar\nmore",
                           "gear": [{"id": "r1-1", "name": "n", "story": "\nrow\nend"}]});
    let created = write(&setup, 1, snapshot, "create").await.unwrap();
    // What a browser sends for the unchanged page: textarea line breaks as CR LF.
    let view = sheet_page(&setup).await;
    let textareas = ["notes", "value-essay", "row-1-0-1"];
    let browser: Vec<(String, String)> = submitted(&view)
        .into_iter()
        .map(|(key, value)| {
            let value = if textareas.contains(&key.as_str()) {
                value.replace('\n', "\r\n")
            } else {
                value
            };
            (key, value)
        })
        .collect();
    assert!(
        serde_urlencoded::to_string(&browser)
            .unwrap()
            .contains("%0D%0A")
    );
    // Normalised to LF it equals the saved text, so nothing is written (D-U).
    assert_eq!(
        setup.post("/private-sheets/1", browser).await.0,
        StatusCode::OK
    );
    let saved = reopen(&setup, 1).await;
    assert_eq!(saved.revision, 1);
    assert_eq!(
        serde_json::to_string(&(&saved.data, &saved.notes)).unwrap(),
        serde_json::to_string(&(&created.data, &created.notes)).unwrap(),
        "an unchanged browser Save stores byte-identical values"
    );
    // A raw CR LF / lone CR body is stored with LF.
    let mut form = submitted(&sheet_page(&setup).await);
    set(&mut form, "value-essay", "a\r\nb\rc");
    assert_eq!(
        setup.post("/private-sheets/1", form).await.0,
        StatusCode::SEE_OTHER
    );
    assert_eq!(reopen(&setup, 1).await.data["essay"], "a\nb\nc");
    // Length limits count the normalised text: 20,000 CR LF pairs are 20,000 notes characters.
    let mut form = submitted(&sheet_page(&setup).await);
    set(&mut form, "notes", &"\r\n".repeat(20_000));
    assert_eq!(
        setup.post("/private-sheets/1", form).await.0,
        StatusCode::SEE_OTHER
    );
    assert_eq!(reopen(&setup, 1).await.notes, "\n".repeat(20_000));
}

/// Slice-1.2 review row 4: since an unchanged browser Save writes nothing (D-U), pin LF
/// normalisation on a changed save. The browser sends every textarea with CR LF and one
/// multiline cell edited: the edit is stored with LF and the untouched text is stored
/// byte-identical (red when the transport keeps CR).
#[tokio::test]
async fn browser_crlf_edit_is_stored_with_lf() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let mut definition = hostile_definition();
    definition["fields"].as_array_mut().unwrap().push(
        json!({"id": "essay", "label": "Essay", "control": "multiline",
                     "nullable": false, "mapping": mapped()}),
    );
    definition["groups"][0]["fields"]
        .as_array_mut()
        .unwrap()
        .push("essay".into());
    let setup = list_setup(&definition).await;
    let mut snapshot = blank(&setup, 1).await;
    snapshot.sheet_id = 1;
    snapshot.notes = "\nnotes\nsecond line".into();
    snapshot.data = json!({"label": "x", "essay": "\n\nscalar\nmore",
                           "gear": [{"id": "r1-1", "name": "n", "story": "\nrow\nend"}]});
    let created = write(&setup, 1, snapshot, "create").await.unwrap();
    let textareas = ["notes", "value-essay", "row-1-0-1"];
    let mut browser: Vec<(String, String)> = submitted(&sheet_page(&setup).await)
        .into_iter()
        .map(|(key, value)| {
            let value = if textareas.contains(&key.as_str()) {
                value.replace('\n', "\r\n")
            } else {
                value
            };
            (key, value)
        })
        .collect();
    set(&mut browser, "value-essay", "\r\nedited\r\nessay\r\n");
    assert!(
        serde_urlencoded::to_string(&browser)
            .unwrap()
            .contains("%0D%0A")
    );
    assert_eq!(
        setup.post("/private-sheets/1", browser).await.0,
        StatusCode::SEE_OTHER
    );
    let saved = reopen(&setup, 1).await;
    assert_eq!(saved.revision, 2);
    assert_eq!(saved.data["essay"], "\nedited\nessay\n");
    assert_eq!(saved.notes, created.notes, "untouched notes stored with LF");
    assert_eq!(
        saved.data["gear"], created.data["gear"],
        "untouched row text stored with LF"
    );
    let stored = serde_json::to_string(&(&saved.data, &saved.notes)).unwrap();
    assert!(!stored.contains("\\r"), "{stored}");
}

/// Last review (F8): a draft operation re-renders the normalised draft; no CR reaches the page.
#[tokio::test]
async fn draft_op_rerender_has_crlf_normalised() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = list_setup(&hostile_definition()).await;
    create(
        &setup,
        json!({"label": "x", "gear": [{"id": "r1-1", "name": "n", "story": "s"}]}),
    )
    .await
    .unwrap();
    let mut form = clicked(&sheet_page(&setup).await, "add-1");
    set(&mut form, "row-1-0-1", "\r\nfirst\r\nsecond\rthird");
    set(&mut form, "notes", "a\r\nb");
    let (status, html) = setup.post("/private-sheets/1", form).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!html.contains('\r'), "no CR in the re-render");
    let echoed = submitted_by(&html, None);
    assert_eq!(get(&echoed, "row-1-0-1"), "\nfirst\nsecond\nthird");
    assert_eq!(get(&echoed, "notes"), "a\nb");
}
