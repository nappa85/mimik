//! Catalogue plan §3 step 3: browse order and cursor (R13), quoted name search (§12),
//! colliding keys (R20) and the search time budget, against published owner-written
//! catalogues of a synthetic pair.
use super::{BrowseFailure, BrowseQuery, Linked, browse, browse_within, latest_sealed};
use crate::{AppError, Store, catalogues::PublishRequest};
use serde_json::{Value, json};
use std::time::Duration;

fn owner(key: &str, entry_type: &str, name: &str) -> Value {
    json!({"key": key, "type": entry_type, "name": name, "properties": {},
           "description": {"owner_text": "Synthetic owner text."}})
}

fn catalogue(id: &str, entries: &[Value]) -> Value {
    json!({"contract_version": 1, "game_id": "synthetic-game", "edition_id": "first",
           "catalogue_id": id, "title": format!("Synthetic {id}"), "sources": [],
           "entry_types": [{"id": "spell", "label": "Spell", "properties": []},
                           {"id": "charm", "label": "Charm", "properties": []}],
           "entries": entries})
}

async fn store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("read.sqlite3")).unwrap();
    store
        .register_ruleset(crate::registry::RegisterRuleset {
            game_id: "synthetic-game".into(),
            game_label: "Synthetic game".into(),
            edition_id: "first".into(),
            edition_label: "First edition".into(),
        })
        .await
        .unwrap();
    (dir, store)
}

async fn publish(store: &Store, value: &Value, expected_revision: i64) {
    let draft = store
        .save_catalogue_draft(serde_json::to_vec(value).unwrap(), expected_revision)
        .await
        .unwrap();
    let comparison = store.compare_catalogue(draft.id).await.unwrap();
    store
        .publish_catalogue(
            draft.id,
            PublishRequest {
                comparison_hash: comparison.comparison_hash,
                actor: "Synthetic operator".into(),
                rationale: "Synthetic catalogue".into(),
            },
        )
        .await
        .unwrap();
}

fn linked(db: &rusqlite::Connection, id: &str) -> Linked {
    latest_sealed(db, "synthetic-game", "first", id)
        .unwrap()
        .unwrap()
}

fn query(entry_type: Option<&str>, q: Option<&str>, after: Option<&str>) -> BrowseQuery {
    BrowseQuery {
        entry_type: entry_type.map(Into::into),
        q: q.map(Into::into),
        after: after.map(Into::into),
    }
}

fn keys(
    db: &rusqlite::Connection,
    catalogue: &str,
    query: &BrowseQuery,
) -> Result<(Vec<String>, bool), BrowseFailure> {
    let page = browse(db, &linked(db, catalogue), query)?;
    Ok((
        page.rows.into_iter().map(|row| row.key).collect(),
        page.more,
    ))
}

#[tokio::test]
async fn browse_orders_by_name_then_key_with_an_exclusive_cursor() {
    let _lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let (_dir, store) = store().await;
    // Sixty entries over three duplicate names, keys in scrambled author order.
    let mut expected: Vec<(String, String, String)> = (0..60)
        .map(|n| {
            let name = ["Echo", "Alpha", "Zulu"][(n * 7) % 3].to_owned();
            let entry_type = if n % 2 == 0 { "spell" } else { "charm" };
            (
                name,
                format!("k{:02}", (n * 37) % 60),
                entry_type.to_owned(),
            )
        })
        .collect();
    let entries: Vec<Value> = expected
        .iter()
        .map(|(name, key, entry_type)| owner(key, entry_type, name))
        .collect();
    publish(&store, &catalogue("spells", &entries), 0).await;
    expected.sort();
    let order: Vec<String> = expected.iter().map(|(_, key, _)| key.clone()).collect();
    store
        .run(move |db| {
            let (first, more) = keys(db, "spells", &query(None, None, None))?;
            assert_eq!(first, order[..50]);
            assert!(more);
            let (second, more) = keys(db, "spells", &query(None, None, Some(&first[49])))?;
            assert_eq!(second, order[50..]);
            assert!(!more);
            // An unknown cursor key restarts from the beginning.
            let (restart, _) = keys(db, "spells", &query(None, None, Some("nothing")))?;
            assert_eq!(restart, first);
            // A type filter keeps (name, key) order; a cursor key of another type seeks from
            // that key's (name, key).
            let charms: Vec<String> = expected
                .iter()
                .filter(|(_, _, entry_type)| entry_type == "charm")
                .map(|(_, key, _)| key.clone())
                .collect();
            let (filtered, more) = keys(db, "spells", &query(Some("charm"), None, None))?;
            assert_eq!(filtered, charms);
            assert!(!more);
            let spell = expected
                .iter()
                .find(|(name, _, entry_type)| name == "Echo" && entry_type == "spell")
                .unwrap();
            let seek: Vec<String> = expected
                .iter()
                .filter(|(name, key, entry_type)| {
                    entry_type == "charm" && (name, key) > (&spell.0, &spell.1)
                })
                .map(|(_, key, _)| key.clone())
                .collect();
            let (after_spell, _) = keys(db, "spells", &query(Some("charm"), None, Some(&spell.1)))?;
            assert_eq!(after_spell, seek);
            assert!(matches!(
                keys(db, "spells", &query(Some("potion"), None, None)),
                Err(BrowseFailure::Other(AppError::NotFound))
            ));
            Ok(())
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn name_search_quotes_every_token_and_filters_without_ranking() {
    let _lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let (_dir, store) = store().await;
    publish(
        &store,
        &catalogue(
            "spells",
            &[
                owner("flare", "spell", "Lantern Flare"),
                owner("or-ward", "spell", "Lantern OR Ward"),
                owner("ward-ward", "spell", "Ward Ward Ward"),
                owner("alpha-ward", "charm", "Alpha Ward"),
                owner("near", "charm", "Near the Column"),
            ],
        ),
        0,
    )
    .await;
    store
        .run(|db| {
            let found = |q: &str| keys(db, "spells", &query(None, Some(q), None));
            assert_eq!(found("lantern")?.0, ["flare", "or-ward"]);
            assert_eq!(found("lan")?.0, ["flare", "or-ward"], "three-letter prefix");
            // Order is (name, key) even where bm25 would rank "Ward Ward Ward" first.
            assert_eq!(found("ward")?.0, ["alpha-ward", "or-ward", "ward-ward"]);
            // Operators are quoted tokens: "OR" must occur in the name.
            assert_eq!(found("lantern OR ward")?.0, ["or-ward"]);
            // A column filter is a phrase of its tokens, never a filter.
            assert!(found("name:lantern")?.0.is_empty());
            assert!(found("NEAR(lantern ward)")?.0.is_empty());
            assert_eq!(found("near column")?.0, ["near"]);
            assert_eq!(found("\"lantern")?.0, ["flare", "or-ward"]);
            assert_eq!(found("\"lantern flare\"")?.0, ["flare"]);
            assert_eq!(found("ward")?.0, found("WARD")?.0);
            // Type filter and search together.
            assert_eq!(
                keys(db, "spells", &query(Some("charm"), Some("ward"), None))?.0,
                ["alpha-ward"]
            );
            for (q, message) in [
                ("* - ^", "Search text needs at least one word"),
                (&"w ".repeat(17), "Search text supports at most 16"),
                ("ward\u{7}", "Search text must not contain control"),
            ] {
                assert!(
                    matches!(found(q), Err(BrowseFailure::Query(text)) if text.starts_with(message)),
                    "{q:?}"
                );
            }
            assert_eq!(found(&"w ".repeat(16))?.0, Vec::<String>::new());
            Ok(())
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn colliding_keys_across_catalogues_never_leak_and_only_the_latest_names_match() {
    let _lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let (_dir, store) = store().await;
    publish(
        &store,
        &catalogue("spells", &[owner("lantern", "spell", "Spell Lantern")]),
        0,
    )
    .await;
    publish(
        &store,
        &catalogue("items", &[owner("lantern", "charm", "Brass Lantern")]),
        0,
    )
    .await;
    store
        .run(|db| {
            let names = |catalogue: &str, q: Option<&str>| -> Result<Vec<String>, AppError> {
                Ok(browse(db, &linked(db, catalogue), &query(None, q, None))?
                    .rows
                    .into_iter()
                    .map(|row| row.name)
                    .collect())
            };
            assert_eq!(names("spells", Some("lantern"))?, ["Spell Lantern"]);
            assert!(names("spells", Some("brass"))?.is_empty());
            assert_eq!(names("items", Some("lantern"))?, ["Brass Lantern"]);
            assert_eq!(names("items", None)?, ["Brass Lantern"]);
            Ok(())
        })
        .await
        .unwrap();
    // A second version renames the entry: only its name matches now.
    publish(
        &store,
        &catalogue("spells", &[owner("lantern", "spell", "Spell Beacon")]),
        1,
    )
    .await;
    store
        .run(|db| {
            let spells = linked(db, "spells");
            let page = |q: &str| browse(db, &spells, &query(None, Some(q), None));
            assert!(page("lantern")?.rows.is_empty());
            assert_eq!(page("beacon")?.rows[0].name, "Spell Beacon");
            Ok(())
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn browse_is_interrupted_past_its_time_budget() {
    let _lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let (_dir, store) = store().await;
    let entries: Vec<Value> = (0..1_500)
        .map(|n| owner(&format!("echo-{n}"), "spell", &format!("Echo {n}")))
        .collect();
    publish(&store, &catalogue("spells", &entries), 0).await;
    let result = store
        .run(|db| {
            let spells = linked(db, "spells");
            let within = browse_within(db, &spells, &query(None, Some("echo"), None), super::QUERY_BUDGET)?;
            assert_eq!(within.rows.len(), 50);
            assert!(within.more);
            for q in [Some("echo"), None] {
                let interrupted = browse_within(db, &spells, &query(None, q, None), Duration::ZERO);
                assert!(
                    matches!(&interrupted, Err(BrowseFailure::Query(message)) if message.starts_with("Query too broad")),
                    "{q:?}: {:?}",
                    interrupted.err()
                );
            }
            // The handler is removed afterwards: a statement well past the check interval runs.
            Ok(db.query_row("WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM c WHERE x<100000) SELECT count(*) FROM c", [], |row| row.get::<_, i64>(0))?)
        })
        .await
        .unwrap();
    assert_eq!(result, 100_000);
}

#[tokio::test]
async fn index_identity_damage_is_never_presented_as_a_query_problem() {
    let _lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let (_dir, store) = store().await;
    publish(
        &store,
        &catalogue("spells", &[owner("lantern", "spell", "Spell Lantern")]),
        0,
    )
    .await;
    store
        .run(|db| {
            let mut damaged = linked(db, "spells");
            damaged.surrogate = i64::MAX;
            let failure = browse(db, &damaged, &query(None, Some("lantern"), None));
            assert!(
                matches!(
                    failure,
                    Err(BrowseFailure::Other(AppError::Invalid(
                        "Catalogue index identity out of range."
                    )))
                ),
                "{:?}",
                failure.err()
            );
            // A real query problem is still one.
            assert!(matches!(
                browse(db, &damaged, &query(None, Some("*"), None)),
                Err(BrowseFailure::Query(_))
            ));
            Ok(())
        })
        .await
        .unwrap();
}
