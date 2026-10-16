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
