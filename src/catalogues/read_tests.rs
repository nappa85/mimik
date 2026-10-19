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
            // The reader's search is interrupted: a query problem ("add more words").
            let searched = browse_within(db, &spells, &query(None, Some("echo"), None), Duration::ZERO);
            assert!(
                matches!(&searched, Err(BrowseFailure::Query(message)) if message.starts_with("Query too broad")),
                "{:?}",
                searched.err()
            );
            // A plain browse (no query) is interrupted too, but that is never the reader's
            // query problem (round-2 review, Space Bunny F2).
            let plain = browse_within(db, &spells, &query(None, None, None), Duration::ZERO);
            assert!(
                matches!(&plain, Err(BrowseFailure::Other(AppError::Invalid(_)))),
                "{:?}",
                plain.err()
            );
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

/// Round-2 measurement (Space Bunny F2): the worst admitted browse page, 50 unit-sourced
/// entries each citing 16 distinct units of 256 KiB (800 references, under the 6,144 cap),
/// so the badge probe scans 200 MiB of unit text. Run once in release with `--ignored`; the
/// result is recorded in plan §18.
#[tokio::test]
#[ignore = "measurement: run in release with --ignored --nocapture"]
async fn measure_the_worst_browse_page() {
    let _lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let (_dir, store) = store().await;
    let zeros = "0".repeat(64);
    let text = "a".repeat(256 * 1024);
    store
        .run(move |db| {
            let prepared = json!({"contract_version":1,"extraction_sha256":zeros}).to_string();
            db.execute("INSERT INTO sources(id,bundle_hash,source_id,title,system_id,edition,language,pdf_hash,pdf,raw_json,prepared_json,total_pages) VALUES(1,?1,'fixture','Synthetic manual','synthetic-game','First','en',?1,x'',x'',?2,50)", rusqlite::params![zeros, prepared.as_bytes()])?;
            db.execute("INSERT INTO source_rulesets VALUES(1,'synthetic-game','first')", [])?;
            for page in 1..=50_i64 {
                db.execute("INSERT INTO source_pages(id,source_id,pdf_page,png) VALUES(?1,1,?1,x'')", [page])?;
                for unit in 0..16_i64 {
                    db.execute("INSERT INTO source_units(id,page_id,block_id,kind,polygon,raw_html,raw_text) VALUES(?1,?2,?3,'text','[]','<p>x</p>',?4)", rusqlite::params![page * 16 + unit, page, format!("b{unit}"), text])?;
                }
            }
            crate::sources::search::ensure(db)?;
            for page in 1..=50_i64 {
                db.execute("INSERT INTO source_page_hashes VALUES(?1,1,?2)", rusqlite::params![page, zeros])?;
            }
            db.execute("INSERT INTO source_finalizations VALUES(1,?1,?1)", [&zeros])?;
            Ok(())
        })
        .await
        .unwrap();
    let entries: Vec<Value> = (1..=50_i64)
        .map(|page| {
            json!({"key": format!("entry-{page:02}"), "type": "spell", "name": format!("Entry {page:02}"),
                   "properties": {},
                   "description": {"units": (0..16_i64).map(|unit| json!({"source_id": 1, "page_id": page, "unit_id": page * 16 + unit})).collect::<Vec<_>>()}})
        })
        .collect();
    let mut value = catalogue("spells", &entries);
    value["sources"] = json!([{"source_id": 1, "pdf_sha256": "0".repeat(64)}]);
    publish(&store, &value, 0).await;
    let timings = store
        .run(|db| {
            let spells = linked(db, "spells");
            let mut timings = Vec::new();
            for q in [None, Some("entry"), None, Some("entry")] {
                let started = std::time::Instant::now();
                let page = browse(db, &spells, &query(None, q, None))?;
                assert_eq!(page.rows.len(), 50);
                assert!(
                    page.rows
                        .iter()
                        .all(|row| row.badge == super::Badge::Unchecked)
                );
                timings.push((q.is_some(), started.elapsed().as_secs_f64() * 1000.0));
            }
            Ok(timings)
        })
        .await
        .unwrap();
    println!("worst browse page (search, ms): {timings:?}");
}
