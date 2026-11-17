//! Catalogue publication (`docs/catalogues.md`; catalogue plan step 2, R1/R2/R5/R17–R20): the seal
//! and guard triggers, comparison tickets, append-only refusals, atomicity, races, the name
//! index and CLI redaction. All fixtures are synthetic.
use mimik::{
    Store,
    catalogues::{CatalogueComparison, CatalogueError, PublishRequest, PublishedCatalogue},
    registry::RegisterRuleset,
    sources::{PageReview, UnitReview},
};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};
use std::process::{Command, Output, Stdio};

#[path = "support/catalogue_fixture.rs"]
mod catalogue_fixture;
use catalogue_fixture::{Fixture, TEST_LOCK, assert_redacted, code, fixture, second_source};

const ACTOR: &str = "Operator zqxactor";
const RATIONALE: &str = "Synthetic zqxrationale";

fn request(comparison_hash: &str) -> PublishRequest {
    PublishRequest {
        comparison_hash: comparison_hash.into(),
        actor: ACTOR.into(),
        rationale: RATIONALE.into(),
    }
}

impl Fixture {
    /// Save `value` as the draft (create or next revision); returns the draft id.
    async fn draft(&self, value: &Value) -> i64 {
        let revision = self
            .db()
            .query_row(
                "SELECT coalesce(max(revision),0) FROM catalogue_drafts WHERE catalogue_id=?1",
                [value["catalogue_id"].as_str().unwrap()],
                |row| row.get::<_, i64>(0),
            )
            .unwrap();
        self.store
            .save_catalogue_draft(serde_json::to_vec(value).unwrap(), revision)
            .await
            .unwrap()
            .id
    }

    async fn compare(&self, id: i64) -> CatalogueComparison {
        self.store.compare_catalogue(id).await.unwrap()
    }

    /// Compare, then publish with the fresh ticket.
    async fn publish(&self, value: &Value) -> Result<PublishedCatalogue, CatalogueError> {
        let id = self.draft(value).await;
        let ticket = self.compare(id).await.comparison_hash;
        self.store.publish_catalogue(id, request(&ticket)).await
    }

    /// Row counts of every publication table, the name index included.
    fn published_rows(&self) -> [i64; 7] {
        [
            "catalogues",
            "catalogue_versions",
            "catalogue_entry_types",
            "catalogue_entries",
            "catalogue_entry_units",
            "catalogue_seals",
            "catalogue_names_fts",
        ]
        .map(|table| self.count(table))
    }

    fn cli_publish(&self, id: &str, ticket: &str) -> Output {
        self.cli(&[
            "publish-catalogue",
            "--id",
            id,
            "--comparison-hash",
            ticket,
            "--actor",
            ACTOR,
            "--rationale",
            RATIONALE,
        ])
    }

    async fn review_unit(&self, state: &str) {
        let revision: i64 = self
            .db()
            .query_row(
                "SELECT revision FROM source_units WHERE id=?1",
                [self.unit],
                |row| row.get(0),
            )
            .unwrap();
        let correction = if state == "corrected" {
            "Synthetic corrected text".into()
        } else {
            String::new()
        };
        self.store
            .review_source_unit(
                self.page,
                self.unit,
                UnitReview {
                    revision,
                    state: state.into(),
                    correction,
                    actor: "fixture-reviewer".into(),
                    rationale: "Synthetic review".into(),
                },
            )
            .await
            .unwrap();
    }

    async fn complete_page(&self, label: &str) {
        let revision: i64 = self
            .db()
            .query_row(
                "SELECT revision FROM source_pages WHERE id=?1",
                [self.page],
                |row| row.get(0),
            )
            .unwrap();
        self.store
            .complete_source_page(
                self.page,
                PageReview {
                    revision,
                    printed_label: label.into(),
                    actor: "fixture-reviewer".into(),
                    rationale: "Synthetic page review".into(),
                },
            )
            .await
            .unwrap();
    }
}

/// The fixture catalogue plus a second entry type and an entry type with no entries.
fn rich(fixture: &Fixture) -> Value {
    let mut value = fixture.catalogue();
    let types = value["entry_types"].as_array_mut().unwrap();
    types.push(json!({"id": "charm", "label": "Charm zqxlabel", "properties": []}));
    types.push(json!({"id": "empty", "label": "Empty zqxlabel", "properties": []}));
    value
}

fn output_json(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_redacted(output);
    serde_json::from_slice(&output.stdout).unwrap()
}

fn failed(output: &Output, needle: &str) {
    assert!(!output.status.success(), "unexpected success");
    assert_redacted(output);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains(needle), "expected {needle}: {stderr}");
}

fn error_text(result: rusqlite::Result<usize>) -> String {
    result
        .expect_err("statement unexpectedly succeeded")
        .to_string()
}

/// The member names of a JSON object, sorted: what is printed matters, not the order
/// (`serde_json` keeps document order, DEP-18).
fn keys(value: &Value) -> Vec<&str> {
    let mut keys: Vec<&str> = value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    keys
}

// ---------------------------------------------------------------------------------------
// Manual versions: rows derived from the stored content through SQLite's JSON functions,
// mutated by a test, inserted by hand. Library publication is the positive control.
// ---------------------------------------------------------------------------------------

type TypeRow = (String, i64, String, String);
type EntryRow = (
    String,
    i64,
    String,
    String,
    i64,
    String,
    Option<String>,
    String,
);
type UnitRow = (String, i64, i64, i64, i64);

#[derive(Clone, Default)]
struct Rows {
    types: Vec<TypeRow>,
    entries: Vec<EntryRow>,
    units: Vec<UnitRow>,
}

/// Insert an unsealed version of catalogue surrogate `catalogue` holding draft `draft`'s
/// current content; returns its id.
fn manual_version(db: &Connection, catalogue: i64, version: i64, draft: i64) -> i64 {
    db.execute(
        "INSERT INTO catalogue_versions(catalogue,game_id,edition_id,catalogue_id,version,title,content,content_hash,draft_id,draft_revision,comparison_hash,actor,rationale,admission_profile) SELECT ?1,game_id,edition_id,catalogue_id,?2,json_extract(content,'$.title'),content,content_hash,id,revision,?3,'fixture','fixture','catalogue-admission-1' FROM catalogue_drafts WHERE id=?4",
        params![catalogue, version, "c".repeat(64), draft],
    )
    .unwrap();
    db.last_insert_rowid()
}

fn surrogate(db: &Connection) -> i64 {
    if let Some(id) = db
        .query_row(
            "SELECT id FROM catalogues WHERE catalogue_id='spells'",
            [],
            |row| row.get(0),
        )
        .optional()
        .unwrap()
    {
        return id;
    }
    db.execute(
        "INSERT INTO catalogues(game_id,edition_id,catalogue_id) VALUES('synthetic-game','first','spells')",
        [],
    )
    .unwrap();
    db.last_insert_rowid()
}

fn content_rows(db: &Connection, version_id: i64) -> Rows {
    let mut rows = Rows::default();
    let mut query = db
        .prepare("SELECT t.value->>'$.id',t.key,t.value->>'$.label',t.value->'$.properties' FROM catalogue_versions v,json_each(v.content,'$.entry_types') t WHERE v.id=?1")
        .unwrap();
    rows.types = query
        .query_map([version_id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let mut query = db
        .prepare("SELECT e.value->>'$.key',e.key,e.value->>'$.type',e.value->>'$.name',coalesce(e.value->>'$.retired',0),CASE WHEN e.value->'$.description.owner_text' IS NULL THEN 'units' ELSE 'owner_text' END,e.value->>'$.description.owner_text',e.value->'$.properties' FROM catalogue_versions v,json_each(v.content,'$.entries') e WHERE v.id=?1")
        .unwrap();
    rows.entries = query
        .query_map([version_id], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
                r.get(6)?,
                r.get(7)?,
            ))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let mut query = db
        .prepare("SELECT e.value->>'$.key',u.key,u.value->>'$.unit_id',u.value->>'$.page_id',u.value->>'$.source_id' FROM catalogue_versions v,json_each(v.content,'$.entries') e,json_each(e.value,'$.description.units') u WHERE v.id=?1")
        .unwrap();
    rows.units = query
        .query_map([version_id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect();
    rows
}

fn insert_rows(db: &Connection, version_id: i64, rows: &Rows) -> rusqlite::Result<()> {
    for (type_id, ordinal, label, properties) in &rows.types {
        db.execute(
            "INSERT INTO catalogue_entry_types VALUES(?1,?2,?3,?4,?5)",
            params![version_id, type_id, ordinal, label, properties],
        )?;
    }
    for (key, ordinal, type_id, name, retired, kind, owner, properties) in &rows.entries {
        db.execute(
            "INSERT INTO catalogue_entries VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![
                version_id, key, ordinal, type_id, name, retired, kind, owner, properties
            ],
        )?;
    }
    for (key, ordinal, unit, page, source) in &rows.units {
        db.execute(
            "INSERT INTO catalogue_entry_units VALUES(?1,?2,?3,?4,?5,?6)",
            params![version_id, key, ordinal, unit, page, source],
        )?;
    }
    Ok(())
}

fn seal(db: &Connection, version_id: i64) -> rusqlite::Result<usize> {
    db.execute(
        "INSERT INTO catalogue_seals(version_id) VALUES(?1)",
        [version_id],
    )
}

/// Build a manual version of `draft` with `mutate` applied, then try to seal it. Rolled back.
fn manual_seal(
    db: &mut Connection,
    draft: i64,
    mutate: impl FnOnce(&mut Rows),
) -> Result<(), String> {
    let tx = db.transaction().unwrap();
    let catalogue = surrogate(&tx);
    let next: i64 = tx
        .query_row(
            "SELECT coalesce(max(version),0)+1 FROM catalogue_versions WHERE catalogue=?1",
            [catalogue],
            |row| row.get(0),
        )
        .unwrap();
    let version = manual_version(&tx, catalogue, next, draft);
    let mut rows = content_rows(&tx, version);
    mutate(&mut rows);
    insert_rows(&tx, version, &rows).map_err(|error| error.to_string())?;
    seal(&tx, version).map_err(|error| error.to_string())?;
    Ok(())
}

/// A two-unit, two-source fixture value (for unit and ordinal mutations).
async fn two_unit_value(fixture: &Fixture) -> Value {
    let (source_b, page_b, unit_b) = second_source(fixture).await;
    let mut value = rich(fixture);
    value["sources"]
        .as_array_mut()
        .unwrap()
        .push(json!({"source_id": source_b, "pdf_sha256": fixture.pdf_hash}));
    value["entries"][0]["description"]["units"] = json!([
        {"source_id": fixture.source, "page_id": fixture.page, "unit_id": fixture.unit},
        {"source_id": source_b, "page_id": page_b, "unit_id": unit_b}
    ]);
    value
}

type Mutation = (&'static str, fn(&mut Rows));

/// Each mutation of an otherwise exact manual version must make the seal refuse it with
/// `expected`; the unmutated rows (positive control) seal. Nothing is committed.
async fn assert_seal_refuses(expected: &str, cases: &[Mutation]) {
    let _guard = TEST_LOCK.lock().await;
    let fixture = fixture().await;
    let value = two_unit_value(&fixture).await;
    let draft = fixture.draft(&value).await;
    let mut db = fixture.db();
    manual_seal(&mut db, draft, |_| {}).unwrap();
    for (label, mutate) in cases {
        let error = manual_seal(&mut db, draft, mutate).expect_err(label);
        assert!(error.contains(expected), "{label}: {error}");
    }
    assert_eq!(fixture.published_rows(), [0; 7]);
}

#[tokio::test]
async fn seal_requires_entries_equal_to_the_content() {
    assert_seal_refuses(
        "Catalogue seal: entries differ from content",
        &[
            // Same-count substitution: keys a, c in place of a, b.
            ("same-count key substitution", |r| {
                r.entries[1].0 = "zqxkey-other".into();
            }),
            ("wrong entry type", |r| r.entries[0].2 = "charm".into()),
            ("wrong retired flag", |r| r.entries[1].4 = 0),
            ("wrong name", |r| r.entries[0].3 = "Other zqxname".into()),
            ("wrong owner text", |r| {
                r.entries[1].6 = Some("Other zqxowner".into());
            }),
            ("wrong properties", |r| {
                r.entries[0].7 = r#"{"zqxprop":"zqxvalue","level":2}"#.into();
            }),
            ("reordered entries", |r| {
                r.entries[0].1 = 1;
                r.entries[1].1 = 0;
            }),
            ("extra entry", |r| {
                let mut extra = r.entries[1].clone();
                extra.0 = "zqxkey-extra".into();
                extra.1 = 2;
                r.entries.push(extra);
            }),
            ("missing entry", |r| {
                r.entries.pop();
            }),
        ],
    )
    .await;
}

#[tokio::test]
async fn seal_requires_entry_types_equal_to_the_content() {
    assert_seal_refuses(
        "Catalogue seal: entry types differ from content",
        &[
            ("wrong type label", |r| {
                r.types[1].2 = "Other zqxlabel".into();
            }),
            ("wrong type properties", |r| {
                r.types[1].3 = r#"[{"id":"zqxprop","label":"Prop","control":"text"}]"#.into();
            }),
            ("extra entry type", |r| {
                r.types
                    .push(("extra".into(), 3, "Extra".into(), "[]".into()));
            }),
            ("missing entry type", |r| {
                r.types.pop();
            }),
        ],
    )
    .await;
}

#[tokio::test]
async fn seal_requires_entry_units_equal_to_the_content() {
    assert_seal_refuses(
        "Catalogue seal: entry units differ from content",
        &[
            ("wrong unit", |r| {
                let first = r.units[0].clone();
                r.units[1].2 = first.2;
                r.units[1].3 = first.3;
                r.units[1].4 = first.4;
            }),
            ("swapped unit ordinals", |r| {
                r.units[0].1 = 1;
                r.units[1].1 = 0;
            }),
            ("extra unit", |r| {
                let mut extra = r.units[0].clone();
                extra.1 = 2;
                r.units.push(extra);
            }),
            ("missing unit", |r| {
                r.units.pop();
            }),
        ],
    )
    .await;
}

#[tokio::test]
async fn seal_refuses_dropped_or_retyped_keys_and_dropped_types() {
    let _guard = TEST_LOCK.lock().await;
    let fixture = fixture().await;
    let base = rich(&fixture);
    fixture.publish(&base).await.unwrap();
    let mut db = fixture.db();
    let append = "Catalogue seal: append-only violated";
    let mut dropped_key = base.clone();
    dropped_key["entries"].as_array_mut().unwrap().pop();
    let mut retyped = base.clone();
    retyped["entries"][1]["type"] = "charm".into();
    retyped["entries"][1]["properties"] = json!({});
    let mut dropped_type = base.clone();
    dropped_type["entry_types"].as_array_mut().unwrap().pop();
    for (label, value) in [
        ("dropped key", &dropped_key),
        ("retyped key", &retyped),
        ("dropped empty type", &dropped_type),
    ] {
        let draft = fixture.draft(value).await;
        let error = manual_seal(&mut db, draft, |_| {}).expect_err(label);
        assert!(error.contains(append), "{label}: {error}");
    }
    // Adding entries and types, and retiring, is append-only.
    let mut grown = base.clone();
    grown["entry_types"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id": "added", "label": "Added", "properties": []}));
    grown["entries"][0]["retired"] = true.into();
    let draft = fixture.draft(&grown).await;
    manual_seal(&mut db, draft, |_| {}).unwrap();
}

#[tokio::test]
async fn seals_follow_version_order_and_versions_follow_their_draft() {
    let _guard = TEST_LOCK.lock().await;
    let fixture = fixture().await;
    let base = rich(&fixture);
    let first = fixture.publish(&base).await.unwrap();
    let draft = first.version.draft_id;
    let mut db = fixture.db();
    let catalogue = surrogate(&db);
    let order = "Catalogue seal: version order";
    // A seal for a version that does not exist, and a second seal of version 1.
    assert!(error_text(seal(&db, 999)).contains(order));
    let error = error_text(seal(&db, first.version.version_id));
    assert!(
        error.contains(order) || error.contains("Catalogue seal replacement rejected"),
        "{error}"
    );
    // Version numbers are contiguous and follow a sealed predecessor; content is the draft's.
    let provenance = "Catalogue version order or provenance rejected";
    let tx = db.transaction().unwrap();
    for version in [1, 3] {
        let error = tx
            .execute(
                "INSERT INTO catalogue_versions(catalogue,game_id,edition_id,catalogue_id,version,title,content,content_hash,draft_id,draft_revision,comparison_hash,actor,rationale,admission_profile) SELECT ?1,game_id,edition_id,catalogue_id,?2,json_extract(content,'$.title'),content,content_hash,id,revision,?3,'a','r','catalogue-admission-1' FROM catalogue_drafts WHERE id=?4",
                params![catalogue, version, "c".repeat(64), draft],
            )
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("replacement rejected") || error.contains(provenance),
            "{version}: {error}"
        );
    }
    let error = error_text(tx.execute(
        "INSERT INTO catalogue_versions(catalogue,game_id,edition_id,catalogue_id,version,title,content,content_hash,draft_id,draft_revision,comparison_hash,actor,rationale,admission_profile) SELECT ?1,game_id,edition_id,catalogue_id,2,'Other',json_set(content,'$.title','Other'),content_hash,id,revision,?2,'a','r','catalogue-admission-1' FROM catalogue_drafts WHERE id=?3",
        params![catalogue, "c".repeat(64), draft],
    ));
    assert!(error.contains(provenance), "{error}");
    manual_version(&tx, catalogue, 2, draft);
    let error = error_text(tx.execute(
        "INSERT INTO catalogue_versions(catalogue,game_id,edition_id,catalogue_id,version,title,content,content_hash,draft_id,draft_revision,comparison_hash,actor,rationale,admission_profile) SELECT ?1,game_id,edition_id,catalogue_id,3,json_extract(content,'$.title'),content,content_hash,id,revision,?2,'a','r','catalogue-admission-1' FROM catalogue_drafts WHERE id=?3",
        params![catalogue, "c".repeat(64), draft],
    ));
    assert!(error.contains(provenance), "unsealed predecessor: {error}");
    drop(tx);
    // The seal's own order check, reached by removing the insert-order trigger in this test
    // database (no product path creates out-of-order unsealed versions).
    db.execute_batch("DROP TRIGGER catalogue_versions_order")
        .unwrap();
    let tx = db.transaction().unwrap();
    let v2 = manual_version(&tx, catalogue, 2, draft);
    insert_rows(&tx, v2, &content_rows(&tx, v2)).unwrap();
    let v3 = manual_version(&tx, catalogue, 3, draft);
    insert_rows(&tx, v3, &content_rows(&tx, v3)).unwrap();
    assert!(
        error_text(seal(&tx, v3)).contains(order),
        "seal skipping version 2"
    );
    seal(&tx, v2).unwrap();
    seal(&tx, v3).unwrap();
    drop(tx);
}

#[tokio::test]
async fn published_rows_reject_update_delete_replace_and_sealed_inserts() {
    let _guard = TEST_LOCK.lock().await;
    let fixture = fixture().await;
    let value = two_unit_value(&fixture).await;
    let published = fixture.publish(&value).await.unwrap();
    let before = fixture.published_rows();
    let db = fixture.db();
    let tables = [
        (
            "catalogues",
            "catalogue_id='other'",
            "Catalogue identities are immutable",
            "Catalogue identities are retained",
        ),
        (
            "catalogue_versions",
            "actor='other'",
            "Catalogue versions are immutable",
            "Catalogue versions are retained",
        ),
        (
            "catalogue_entry_types",
            "label='other'",
            "Catalogue entry types are immutable",
            "Catalogue entry types are retained",
        ),
        (
            "catalogue_entries",
            "name='other'",
            "Catalogue entries are immutable",
            "Catalogue entries are retained",
        ),
        (
            "catalogue_entry_units",
            "ordinal=ordinal",
            "Catalogue entry units are immutable",
            "Catalogue entry units are retained",
        ),
        (
            "catalogue_seals",
            "created_at='other'",
            "Catalogue seals are immutable",
            "Catalogue seals are retained",
        ),
    ];
    for (table, set, updated, deleted) in tables {
        let error = error_text(db.execute(&format!("UPDATE {table} SET {set}"), []));
        assert!(error.contains(updated), "{table} update: {error}");
        let error = error_text(db.execute(&format!("DELETE FROM {table}"), []));
        assert!(error.contains(deleted), "{table} delete: {error}");
        let error = error_text(db.execute(
            &format!("INSERT OR REPLACE INTO {table} SELECT * FROM {table}"),
            [],
        ));
        // Several guards may refuse a replacement of a sealed row; which fires first is
        // SQLite's choice. The unsealed-version test checks each guard's own message.
        assert!(
            [
                "replacement rejected",
                "Catalogue version is sealed",
                "Catalogue seal: version order",
                "order or provenance rejected",
            ]
            .iter()
            .any(|message| error.contains(message)),
            "{table} replace: {error}"
        );
    }
    // Unique-key-only conflicts and fresh rows in a sealed version.
    let version = published.version.version_id;
    let sealed = "Catalogue version is sealed";
    for sql in [
        "INSERT INTO catalogue_entry_types VALUES(?1,'fresh',9,'Fresh','[]')",
        "INSERT OR REPLACE INTO catalogue_entry_types VALUES(?1,'fresh',0,'Fresh','[]')",
        "INSERT INTO catalogue_entries VALUES(?1,'fresh-key',9,'spell','Fresh',0,'owner_text','Fresh','{}')",
        "INSERT OR REPLACE INTO catalogue_entries VALUES(?1,'fresh-key',0,'spell','Fresh',0,'owner_text','Fresh','{}')",
        "INSERT INTO catalogue_entry_units SELECT version_id,key,9,unit_id,page_id,source_id FROM catalogue_entry_units WHERE version_id=?1 LIMIT 1",
    ] {
        let error = error_text(db.execute(sql, [version]));
        assert!(
            error.contains(sealed) || error.contains("replacement rejected"),
            "{sql}: {error}"
        );
    }
    let error = error_text(db.execute(
        "INSERT OR REPLACE INTO catalogues(id,game_id,edition_id,catalogue_id) SELECT id+1,game_id,edition_id,catalogue_id FROM catalogues",
        [],
    ));
    assert!(
        error.contains("Catalogue identity replacement rejected"),
        "{error}"
    );
    assert_eq!(fixture.published_rows(), before);
    let verified = output_json(&fixture.cli(&["verify-catalogues"]));
    assert_eq!(verified["status"], "ok");
}

#[tokio::test]
async fn replacement_guards_cover_every_key_of_an_unsealed_version() {
    let _guard = TEST_LOCK.lock().await;
    let fixture = fixture().await;
    let value = two_unit_value(&fixture).await;
    let draft = fixture.draft(&value).await;
    let mut db = fixture.db();
    // The version-order trigger also refuses any version replacement; it is dropped in this
    // test database so the replacement guard's own refusal is observable.
    db.execute_batch("DROP TRIGGER catalogue_versions_order")
        .unwrap();
    let tx = db.transaction().unwrap();
    let catalogue = surrogate(&tx);
    let version = manual_version(&tx, catalogue, 1, draft);
    let rows = content_rows(&tx, version);
    insert_rows(&tx, version, &rows).unwrap();
    for (sql, expected) in [
        (
            "INSERT OR REPLACE INTO catalogue_entry_types SELECT * FROM catalogue_entry_types WHERE version_id=?1",
            "Catalogue entry type replacement rejected",
        ),
        (
            "INSERT OR REPLACE INTO catalogue_entry_types SELECT version_id,'fresh',ordinal,label,properties FROM catalogue_entry_types WHERE version_id=?1 LIMIT 1",
            "Catalogue entry type replacement rejected",
        ),
        (
            "INSERT OR REPLACE INTO catalogue_entries SELECT * FROM catalogue_entries WHERE version_id=?1",
            "Catalogue entry replacement rejected",
        ),
        (
            "INSERT OR REPLACE INTO catalogue_entries SELECT version_id,'fresh-key',ordinal,type_id,name,retired,desc_kind,owner_text,properties FROM catalogue_entries WHERE version_id=?1 LIMIT 1",
            "Catalogue entry replacement rejected",
        ),
        (
            "INSERT OR REPLACE INTO catalogue_entry_units SELECT * FROM catalogue_entry_units WHERE version_id=?1",
            "Catalogue entry unit replacement rejected",
        ),
        (
            "INSERT OR REPLACE INTO catalogue_versions SELECT * FROM catalogue_versions WHERE id=?1",
            "Catalogue version replacement rejected",
        ),
    ] {
        let error = error_text(tx.execute(sql, [version]));
        assert!(error.contains(expected), "{sql}: {error}");
    }
    seal(&tx, version).unwrap();
    // A seal replacement also fails the seal's order check; drop that check here to observe
    // the replacement guard itself.
    tx.execute_batch("DROP TRIGGER catalogue_seals_check")
        .unwrap();
    let error = error_text(tx.execute(
        "INSERT OR REPLACE INTO catalogue_seals(version_id) VALUES(?1)",
        [version],
    ));
    assert!(
        error.contains("Catalogue seal replacement rejected"),
        "{error}"
    );
}

#[tokio::test]
async fn unit_rows_must_match_their_page_finalized_source_and_pair() {
    let _guard = TEST_LOCK.lock().await;
    let fixture = fixture().await;
    let (source_b, page_b, unit_b) = second_source(&fixture).await;
    // A finalized source of another pair.
    fixture
        .store
        .register_ruleset(RegisterRuleset {
            game_id: "synthetic-game".into(),
            game_label: "Synthetic game".into(),
            edition_id: "second".into(),
            edition_label: "Second edition".into(),
        })
        .await
        .unwrap();
    let other = fixture.root.join("other-pair-bundle");
    std::fs::create_dir(&other).unwrap();
    let first = fixture.root.join("unknown-bundle");
    for name in ["source.json", "page-1.png"] {
        std::fs::copy(first.join(name), other.join(name)).unwrap();
    }
    let mut manifest: Value =
        serde_json::from_slice(&std::fs::read(first.join("bundle.json")).unwrap()).unwrap();
    manifest["source_id"] = json!("synthetic-other-pair");
    manifest["edition_id"] = json!("second");
    manifest["edition"] = json!("Second edition");
    std::fs::write(
        other.join("bundle.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    fixture
        .store
        .import_source(&other, &fixture.root.join("original.pdf"))
        .await
        .unwrap();
    let db = fixture.db();
    let (source_c, page_c, unit_c): (i64, i64, i64) = db
        .query_row(
            "SELECT s.id,p.id,u.id FROM sources s JOIN source_pages p ON p.source_id=s.id JOIN source_units u ON u.page_id=p.id WHERE s.source_id='synthetic-other-pair'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    let finalized: bool = db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM source_finalizations WHERE source_id=?1)",
            [source_c],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        finalized,
        "the other-pair source must be finalized to isolate the pair check"
    );
    // An unfinalized source of the right pair.
    db.execute(
        "INSERT INTO sources(id,bundle_hash,source_id,title,system_id,edition,language,pdf_hash,pdf,raw_json,prepared_json,total_pages) VALUES(50,?1,'unfinalized','Unfinalized','synthetic-game','First','en',?2,x'',x'',?3,1)",
        params!["b".repeat(64), "a".repeat(64), json!({"contract_version": 1, "extraction_sha256": "0".repeat(64)}).to_string().as_bytes()],
    )
    .unwrap();
    db.execute(
        "INSERT INTO source_rulesets VALUES(50,'synthetic-game','first')",
        [],
    )
    .unwrap();
    db.execute(
        "INSERT INTO source_pages(id,source_id,pdf_page,png) VALUES(50,50,1,x'')",
        [],
    )
    .unwrap();
    db.execute("INSERT INTO source_units(id,page_id,block_id,kind,polygon,raw_html,raw_text) VALUES(50,50,'block','text','[[0,0],[1,0],[1,1],[0,1]]','','')", []).unwrap();
    drop(db);
    let draft = fixture.draft(&rich(&fixture)).await;
    let mut db = fixture.db();
    let tx = db.transaction().unwrap();
    let catalogue = surrogate(&tx);
    let version = manual_version(&tx, catalogue, 1, draft);
    insert_rows(&tx, version, &content_rows(&tx, version)).unwrap();
    let (a, pa, ua) = (fixture.source, fixture.page, fixture.unit);
    for (label, unit, page, source) in [
        ("unit of A with page and source B", ua, page_b, source_b),
        ("unit of A with page A and source B", ua, pa, source_b),
        ("unit of B with page and source A", unit_b, pa, a),
        ("unfinalized source of the pair", 50, 50, 50),
        ("finalized source of another pair", unit_c, page_c, source_c),
    ] {
        let error = error_text(tx.execute(
            "INSERT INTO catalogue_entry_units VALUES(?1,'zqxkey-charm',0,?2,?3,?4)",
            params![version, unit, page, source],
        ));
        assert!(
            error.contains("Catalogue unit, page, source or pair mismatch"),
            "{label}: {error}"
        );
    }
    tx.execute(
        "INSERT INTO catalogue_entry_units VALUES(?1,'zqxkey-charm',0,?2,?3,?4)",
        params![version, unit_b, page_b, source_b],
    )
    .unwrap();
}

// ---------------------------------------------------------------------------------------
// Library and CLI publication.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn cli_compare_publish_show_verify_and_rebuild_print_metadata_only() {
    let _guard = TEST_LOCK.lock().await;
    let fixture = fixture().await;
    let file = fixture.write("spells.json", &serde_json::to_vec(&rich(&fixture)).unwrap());
    let imported = output_json(&fixture.import(&file, 0));
    let id = imported["id"].as_i64().unwrap().to_string();
    let compared = output_json(&fixture.cli(&["compare-catalogue", "--id", &id]));
    assert_eq!(
        keys(&compared),
        [
            "badges",
            "catalogue_id",
            "changes",
            "comparison_hash",
            "content_hash",
            "counts",
            "draft_id",
            "draft_revision",
            "edition_id",
            "game_id",
            "latest_version",
            "publishable",
            "refusals",
            "units"
        ]
    );
    assert_eq!(compared["latest_version"], Value::Null);
    assert_eq!(compared["publishable"], true);
    assert_eq!(
        compared["units"],
        json!({"unreviewed": 1, "accepted": 0, "corrected": 0, "rejected": 0})
    );
    assert_eq!(
        compared["badges"],
        json!({"owner_written": 1, "reviewed": 0, "unchecked": 1, "unavailable": 0})
    );
    assert_eq!(compared["changes"]["added_entries"], 2);
    assert_eq!(compared["changes"]["added_entry_types"], 3);
    let ticket = compared["comparison_hash"].as_str().unwrap();
    let published = output_json(&fixture.cli(&[
        "publish-catalogue",
        "--id",
        &id,
        "--comparison-hash",
        ticket,
        "--actor",
        ACTOR,
        "--rationale",
        RATIONALE,
    ]));
    assert_eq!(
        keys(&published),
        [
            "catalogue_id",
            "content_hash",
            "counts",
            "created_at",
            "draft_id",
            "draft_revision",
            "edition_id",
            "game_id",
            "latest_version",
            "published",
            "status",
            "version",
            "version_id"
        ]
    );
    assert_eq!(
        (published["published"].clone(), published["version"].clone()),
        (json!(true), json!(1))
    );
    assert_eq!(published["content_hash"], imported["content_hash"]);
    let show = [
        "show-catalogue-version",
        "--game",
        "synthetic-game",
        "--edition",
        "first",
        "--catalogue",
        "spells",
    ];
    let shown = output_json(&fixture.cli(&show));
    assert_eq!(shown["version"], 1);
    assert_eq!(shown["counts"], imported["counts"]);
    let mut show_one = show.to_vec();
    show_one.extend(["--version", "1"]);
    assert_eq!(
        output_json(&fixture.cli(&show_one))["version_id"],
        published["version_id"]
    );
    let verified = output_json(&fixture.cli(&["verify-catalogues"]));
    assert_eq!(
        verified,
        json!({"catalogues": 1, "versions": 1, "index_rows": 2, "problems": [], "status": "ok"})
    );
    let rebuilt = output_json(&fixture.cli(&["rebuild-catalogue-index"]));
    assert_eq!(rebuilt, json!({"catalogues": 1, "index_rows": 2}));
}

#[tokio::test]
async fn cli_identical_content_publishes_nothing() {
    let _guard = TEST_LOCK.lock().await;
    let fixture = fixture().await;
    let id = fixture.draft(&rich(&fixture)).await.to_string();
    let ticket = output_json(&fixture.cli(&["compare-catalogue", "--id", &id]))["comparison_hash"]
        .as_str()
        .unwrap()
        .to_owned();
    let published = output_json(&fixture.cli_publish(&id, &ticket));
    let rows = fixture.published_rows();
    let again = output_json(&fixture.cli(&["compare-catalogue", "--id", &id]));
    assert_eq!(again["changes"]["identical"], true);
    assert_eq!(again["latest_version"], 1);
    let ticket = again["comparison_hash"].as_str().unwrap();
    let republished = output_json(&fixture.cli_publish(&id, ticket));
    assert_eq!(republished["published"], false);
    assert_eq!(republished["version_id"], published["version_id"]);
    assert_eq!(fixture.published_rows(), rows);
}

#[tokio::test]
async fn cli_failure_paths_are_redacted_and_publish_nothing() {
    let _guard = TEST_LOCK.lock().await;
    let fixture = fixture().await;
    let file = fixture.write("spells.json", &serde_json::to_vec(&rich(&fixture)).unwrap());
    let id = output_json(&fixture.import(&file, 0))["id"]
        .as_i64()
        .unwrap()
        .to_string();
    let ticket = output_json(&fixture.cli(&["compare-catalogue", "--id", &id]))["comparison_hash"]
        .as_str()
        .unwrap()
        .to_owned();
    let zeros = "0".repeat(64);
    let publish = |id: &str, hash: &str, actor: &str, rationale: &str| {
        fixture.cli(&[
            "publish-catalogue",
            "--id",
            id,
            "--comparison-hash",
            hash,
            "--actor",
            actor,
            "--rationale",
            rationale,
        ])
    };
    failed(
        &publish(&id, &zeros, ACTOR, RATIONALE),
        "comparison ticket is stale",
    );
    failed(
        &publish(&id, "zqxkey", ACTOR, RATIONALE),
        "comparison-hash-format",
    );
    failed(
        &publish(&id, &ticket.to_uppercase(), ACTOR, RATIONALE),
        "comparison-hash-format",
    );
    failed(&publish(&id, &ticket, " ", RATIONALE), "Catalogue rejected");
    failed(
        &publish(&id, &ticket, &"zqxactor".repeat(20), RATIONALE),
        "Catalogue rejected",
    );
    failed(&publish(&id, &ticket, ACTOR, ""), "Catalogue rejected");
    failed(&publish("999", &ticket, ACTOR, RATIONALE), "not found");
    failed(
        &publish("0", &ticket, ACTOR, RATIONALE),
        "positive-row-id-required",
    );
    failed(
        &fixture.cli(&["compare-catalogue", "--id", "999"]),
        "not found",
    );
    let show = |game: &str, catalogue: &str, version: &str| {
        fixture.cli(&[
            "show-catalogue-version",
            "--game",
            game,
            "--edition",
            "first",
            "--catalogue",
            catalogue,
            "--version",
            version,
        ])
    };
    failed(&show("synthetic-game", "spells", "1"), "not found");
    failed(&show("Zqxkey", "spells", "1"), "invalid-stable-id");
    failed(&show("synthetic-game", "zqxkey", "1"), "not found");
    failed(
        &show("synthetic-game", "spells", "0"),
        "positive-version-required",
    );
    assert_eq!(fixture.published_rows(), [0; 7]);
    // A rejected unit is refused with a static code.
    fixture.review_unit("rejected").await;
    let compared = output_json(&fixture.cli(&["compare-catalogue", "--id", &id]));
    assert_eq!(compared["publishable"], false);
    assert_eq!(compared["refusals"]["rejected_units"], 1);
    assert_eq!(compared["badges"]["unavailable"], 1);
    let ticket = compared["comparison_hash"].as_str().unwrap();
    failed(&publish(&id, ticket, ACTOR, RATIONALE), "rejected-units");
    assert_eq!(fixture.published_rows(), [0; 7]);
}

#[tokio::test]
async fn dropped_or_retyped_keys_and_types_are_refused() {
    let _guard = TEST_LOCK.lock().await;
    let fixture = fixture().await;
    let base = rich(&fixture);
    fixture.publish(&base).await.unwrap();
    let rows = fixture.published_rows();
    let mut dropped_key = base.clone();
    dropped_key["entries"].as_array_mut().unwrap().pop();
    let mut retyped = base.clone();
    retyped["entries"][1]["type"] = "charm".into();
    retyped["entries"][1]["properties"] = json!({});
    let mut dropped_type = base.clone();
    dropped_type["entry_types"].as_array_mut().unwrap().pop();
    for (value, refusal, expected) in [
        (&dropped_key, "dropped_entries", "dropped-entry-keys"),
        (&retyped, "retyped_entries", "retyped-entry-keys"),
        (&dropped_type, "dropped_entry_types", "dropped-entry-types"),
    ] {
        let id = fixture.draft(value).await;
        let comparison = fixture.compare(id).await;
        assert!(!comparison.publishable, "{expected}");
        let refusals = serde_json::to_value(comparison.refusals).unwrap();
        assert_eq!(refusals[refusal], 1, "{expected}");
        let error = fixture
            .store
            .publish_catalogue(id, request(&comparison.comparison_hash))
            .await
            .unwrap_err();
        assert_eq!(code(&error), expected);
        assert_eq!(fixture.published_rows(), rows);
    }
}

#[tokio::test]
async fn empty_types_are_retained_and_changes_are_counted() {
    let _guard = TEST_LOCK.lock().await;
    let fixture = fixture().await;
    let base = rich(&fixture);
    let first = fixture.publish(&base).await.unwrap();
    assert_eq!(first.version.version, 1);
    let mut next = base.clone();
    next["entries"][0]["retired"] = true.into();
    next["entries"][1]["retired"] = false.into();
    next["entries"][1]["description"]["owner_text"] = "Changed zqxowner text".into();
    next["entries"].as_array_mut().unwrap().push(json!({
        "key": "zqxkey-new", "type": "charm", "name": "New zqxname", "properties": {},
        "description": {"units": [{"source_id": fixture.source, "page_id": fixture.page, "unit_id": fixture.unit}]}
    }));
    let id = fixture.draft(&next).await;
    let comparison = fixture.compare(id).await;
    assert_eq!(comparison.latest_version, Some(1));
    assert_eq!(
        serde_json::to_value(comparison.changes).unwrap(),
        json!({"identical": false, "added_entries": 1, "added_entry_types": 0, "newly_retired": 1, "unretired": 1, "description_references_changed": 0, "owner_text_changed": 1})
    );
    let second = fixture
        .store
        .publish_catalogue(id, request(&comparison.comparison_hash))
        .await
        .unwrap();
    assert_eq!(
        (
            second.published,
            second.version.version,
            second.version.latest_version
        ),
        (true, 2, 2)
    );
    let db = fixture.db();
    let empty: i64 = db
        .query_row(
            "SELECT count(*) FROM catalogue_entry_types WHERE type_id='empty' AND version_id IN (?1,?2)",
            [first.version.version_id, second.version.version_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(empty, 2);
    // The name index holds only the latest version's names.
    let names: Vec<String> = db
        .prepare("SELECT key FROM catalogue_names_fts ORDER BY rowid")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(names, ["zqxkey-flare", "zqxkey-charm", "zqxkey-new"]);
    let shown = fixture
        .store
        .catalogue_version(
            "synthetic-game".into(),
            "first".into(),
            "spells".into(),
            Some(1),
        )
        .await
        .unwrap();
    assert_eq!((shown.version, shown.latest_version), (1, 2));
}

#[tokio::test]
async fn intervening_changes_stale_the_ticket() {
    let _guard = TEST_LOCK.lock().await;
    let fixture = fixture().await;
    let base = rich(&fixture);
    let id = fixture.draft(&base).await;
    let stale = |error: CatalogueError| {
        assert!(
            matches!(error, CatalogueError::StaleComparison),
            "{error:?}"
        );
    };
    // A unit review, a page completion and a page label change each advance a bound revision.
    let ticket = fixture.compare(id).await.comparison_hash;
    fixture.review_unit("accepted").await;
    stale(
        fixture
            .store
            .publish_catalogue(id, request(&ticket))
            .await
            .unwrap_err(),
    );
    let ticket = fixture.compare(id).await.comparison_hash;
    fixture.complete_page("p. 1").await;
    stale(
        fixture
            .store
            .publish_catalogue(id, request(&ticket))
            .await
            .unwrap_err(),
    );
    let ticket = fixture.compare(id).await.comparison_hash;
    fixture.complete_page("p. 2").await;
    stale(
        fixture
            .store
            .publish_catalogue(id, request(&ticket))
            .await
            .unwrap_err(),
    );
    let comparison = fixture.compare(id).await;
    assert_eq!(comparison.badges.reviewed, 1);
    // A new draft revision answers a conflict (the draft changed since it was loaded) or a
    // stale ticket.
    let ticket = comparison.comparison_hash;
    let mut changed = base.clone();
    changed["title"] = "Changed zqxtitle".into();
    fixture.draft(&changed).await;
    assert!(matches!(
        fixture
            .store
            .publish_catalogue(id, request(&ticket))
            .await
            .unwrap_err(),
        CatalogueError::StaleComparison | CatalogueError::Conflict
    ));
    // Another publication in between changes the latest version.
    let ticket = fixture.compare(id).await.comparison_hash;
    fixture
        .store
        .publish_catalogue(id, request(&ticket))
        .await
        .unwrap();
    let mut grown = changed.clone();
    grown["entry_types"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id": "later", "label": "Later", "properties": []}));
    fixture.draft(&grown).await;
    let ticket = fixture.compare(id).await.comparison_hash;
    // Another connection publishes a version between compare and publish.
    let mut other = fixture.db();
    {
        let tx = other.transaction().unwrap();
        let catalogue = surrogate(&tx);
        let version = manual_version(&tx, catalogue, 2, id);
        insert_rows(&tx, version, &content_rows(&tx, version)).unwrap();
        seal(&tx, version).unwrap();
        tx.commit().unwrap();
    }
    stale(
        fixture
            .store
            .publish_catalogue(id, request(&ticket))
            .await
            .unwrap_err(),
    );
    // The fresh ticket of identical content publishes nothing.
    let comparison = fixture.compare(id).await;
    assert!(comparison.changes.identical);
    let outcome = fixture
        .store
        .publish_catalogue(id, request(&comparison.comparison_hash))
        .await
        .unwrap();
    assert!(!outcome.published);
    assert_eq!(outcome.version.version, 2);
}

#[tokio::test]
async fn escaped_and_multilingual_text_seals() {
    let _guard = TEST_LOCK.lock().await;
    let fixture = fixture().await;
    let mut value = rich(&fixture);
    let tricky = "Quote \" backslash \\ slash / città mörk 𝔐 \u{05d0}";
    value["title"] = tricky.into();
    value["entries"][0]["name"] = tricky.into();
    value["entries"][0]["properties"]["zqxprop"] = format!("{tricky} </script>").into();
    value["entries"][1]["description"]["owner_text"] =
        format!("{tricky}\n\tline \u{200f}two").into();
    value["entry_types"][0]["label"] = tricky.into();
    let published = fixture.publish(&value).await.unwrap();
    assert!(published.published);
    let verified = fixture.store.verify_catalogues().await.unwrap();
    assert_eq!(verified.status, "ok", "{verified:?}");
}

#[tokio::test]
async fn name_index_is_checked_and_rebuilt() {
    let _guard = TEST_LOCK.lock().await;
    let fixture = fixture().await;
    let base = rich(&fixture);
    fixture.publish(&base).await.unwrap();
    // A second catalogue with colliding keys stays separate in the index.
    let mut items = base.clone();
    items["catalogue_id"] = "items".into();
    fixture.publish(&items).await.unwrap();
    let ok = fixture.store.verify_catalogues().await.unwrap();
    assert_eq!(
        (ok.catalogues, ok.versions, ok.index_rows, ok.status),
        (2, 2, 4, "ok")
    );
    let db = fixture.db();
    let rowids: Vec<(i64, i64)> = db
        .prepare("SELECT rowid,catalogue_row_id FROM catalogue_names_fts ORDER BY rowid")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(
        rowids,
        [
            (1 << 20, 1),
            ((1 << 20) + 1, 1),
            (2 << 20, 2),
            ((2 << 20) + 1, 2)
        ]
    );
    for tamper in [
        "DELETE FROM catalogue_names_fts WHERE rowid=(1<<20)",
        "INSERT INTO catalogue_names_fts(rowid,name,catalogue_row_id,key) VALUES(3<<20,'zqxname stray',3,'zqxkey-stray')",
        "UPDATE catalogue_names_fts SET name='zqxname renamed' WHERE rowid=(2<<20)",
        "UPDATE catalogue_names_fts SET catalogue_row_id=2 WHERE rowid=(1<<20)",
        "UPDATE catalogue_names_fts SET key='zqxkey-charm' WHERE rowid=(1<<20)",
    ] {
        db.execute(tamper, []).unwrap();
        let output = fixture.cli(&["verify-catalogues"]);
        failed(&output, "Catalogue integrity check failed");
        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            report["problems"],
            json!([{"catalogue": 0, "version": null, "code": "name-index-mismatch"}]),
            "{tamper}"
        );
        let rebuilt = output_json(&fixture.cli(&["rebuild-catalogue-index"]));
        assert_eq!(rebuilt, json!({"catalogues": 2, "index_rows": 4}));
        assert_eq!(
            fixture.store.verify_catalogues().await.unwrap().status,
            "ok",
            "{tamper}"
        );
    }
}

#[tokio::test]
async fn name_index_corruption_and_definition_drift_are_reported_and_repaired() {
    let _guard = TEST_LOCK.lock().await;
    let fixture = fixture().await;
    let base = rich(&fixture);
    fixture.publish(&base).await.unwrap();
    let mut items = base.clone();
    items["catalogue_id"] = "items".into();
    fixture.publish(&items).await.unwrap();
    let db = fixture.db();
    // FTS5's own integrity check: corrupt the index's shadow data.
    db.execute("DELETE FROM catalogue_names_fts_data WHERE id>10", [])
        .unwrap();
    let report = fixture.store.verify_catalogues().await.unwrap();
    assert!(
        report
            .problems
            .iter()
            .any(|p| p.code == "name-index-integrity-check-failed"),
        "{report:?}"
    );
    fixture.store.rebuild_catalogue_index().await.unwrap();
    assert_eq!(
        fixture.store.verify_catalogues().await.unwrap().status,
        "ok"
    );
    // A different index definition is reported too, and the rebuild restores the migrated one.
    db.execute_batch("DROP TABLE catalogue_names_fts; CREATE VIRTUAL TABLE catalogue_names_fts USING fts5(name, catalogue_row_id UNINDEXED, key UNINDEXED)").unwrap();
    let report = fixture.store.verify_catalogues().await.unwrap();
    assert!(
        report
            .problems
            .iter()
            .any(|p| p.code == "name-index-definition-mismatch"),
        "{report:?}"
    );
    fixture.store.rebuild_catalogue_index().await.unwrap();
    let definition: String = db
        .query_row(
            "SELECT sql FROM sqlite_master WHERE name='catalogue_names_fts'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let fresh = tempfile::tempdir().unwrap();
    drop(Store::open(&fresh.path().join("fresh.sqlite3")).unwrap());
    let migrated: String = Connection::open(fresh.path().join("fresh.sqlite3"))
        .unwrap()
        .query_row(
            "SELECT sql FROM sqlite_master WHERE name='catalogue_names_fts'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(definition, migrated);
    assert_eq!(
        fixture.store.verify_catalogues().await.unwrap().status,
        "ok"
    );
    // The index answers a name query joined back to the latest version of its catalogue.
    let hits: i64 = db
        .query_row(
            "SELECT count(*) FROM catalogue_names_fts CROSS JOIN catalogue_entries e ON e.key=catalogue_names_fts.key AND e.version_id=(SELECT max(v.id) FROM catalogue_versions v JOIN catalogue_seals s ON s.version_id=v.id WHERE v.catalogue=catalogue_names_fts.catalogue_row_id) WHERE catalogue_names_fts MATCH 'charm'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(hits, 2);
}

#[tokio::test]
async fn tampered_versions_fail_show_and_verify() {
    let _guard = TEST_LOCK.lock().await;
    let fixture = fixture().await;
    let published = fixture.publish(&rich(&fixture)).await.unwrap();
    let show = || {
        fixture.cli(&[
            "show-catalogue-version",
            "--game",
            "synthetic-game",
            "--edition",
            "first",
            "--catalogue",
            "spells",
        ])
    };
    output_json(&show());
    assert_eq!(published.version.version, 1);
    let db = fixture.db();
    // The immutability triggers are dropped in this test database only, to model storage
    // tampering that the integrity check must still detect.
    db.execute_batch(
        "DROP TRIGGER catalogue_versions_update; DROP TRIGGER catalogue_entries_update;",
    )
    .unwrap();
    for (tamper, restore, expected) in [
        (
            "UPDATE catalogue_versions SET content=json_set(content,'$.entries[0].name','Tampered zqxname')",
            "UPDATE catalogue_versions SET content=(SELECT content FROM catalogue_drafts WHERE id=1)",
            "content-hash-mismatch",
        ),
        (
            "UPDATE catalogue_entries SET name='Tampered zqxname' WHERE key='zqxkey-flare'",
            "UPDATE catalogue_entries SET name='Name zqxname' WHERE key='zqxkey-flare'",
            "projection-mismatch",
        ),
        (
            "UPDATE catalogue_entries SET retired=1-retired WHERE key='zqxkey-flare'",
            "UPDATE catalogue_entries SET retired=1-retired WHERE key='zqxkey-flare'",
            "projection-mismatch",
        ),
    ] {
        db.execute(tamper, []).unwrap();
        failed(&show(), expected);
        let output = fixture.cli(&["verify-catalogues"]);
        failed(&output, "Catalogue integrity check failed");
        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["problems"][0]["code"], expected, "{tamper}");
        assert_eq!(report["problems"][0]["version"], 1);
        db.execute(restore, []).unwrap();
        output_json(&show());
    }
}

#[tokio::test]
async fn crash_mid_publish_leaves_no_partial_or_sealed_version() {
    let _guard = TEST_LOCK.lock().await;
    let fixture = fixture().await;
    let id = fixture.draft(&rich(&fixture)).await;
    let ticket = fixture.compare(id).await.comparison_hash;
    let db = fixture.db();
    // A deliberately slow trigger (test database only) keeps the publish transaction open
    // after its first projection rows, so the process can be killed mid-publication.
    db.execute_batch("CREATE TRIGGER zz_slow AFTER INSERT ON catalogue_entries BEGIN SELECT count(*) FROM (WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM c WHERE x<20000000) SELECT x FROM c); END;").unwrap();
    drop(db);
    let mut child = Command::new(env!("CARGO_BIN_EXE_mimik"))
        .arg("--database")
        .arg(&fixture.database)
        .args([
            "publish-catalogue",
            "--id",
            &id.to_string(),
            "--comparison-hash",
            &ticket,
            "--actor",
            ACTOR,
            "--rationale",
            RATIONALE,
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let journal = fixture.database.with_extension("sqlite3-journal");
    let started = std::time::Instant::now();
    while !journal.exists() {
        assert!(
            started.elapsed() < std::time::Duration::from_secs(30),
            "publication never started writing"
        );
        assert!(
            child.try_wait().unwrap().is_none(),
            "publication finished before the crash"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    std::thread::sleep(std::time::Duration::from_millis(100));
    assert!(
        child.try_wait().unwrap().is_none(),
        "publication finished before the crash"
    );
    child.kill().unwrap();
    child.wait().unwrap();
    // Opening the database rolls back the hot journal.
    let store = Store::open(&fixture.database).unwrap();
    assert_eq!(fixture.published_rows(), [0; 7]);
    fixture.db().execute_batch("DROP TRIGGER zz_slow").unwrap();
    let report = store.verify_catalogues().await.unwrap();
    assert_eq!((report.versions, report.status), (0, "ok"));
    let ticket = store.compare_catalogue(id).await.unwrap().comparison_hash;
    let published = store.publish_catalogue(id, request(&ticket)).await.unwrap();
    assert_eq!(published.version.version, 1);
}

#[tokio::test]
async fn failing_seal_rolls_back_the_whole_publication() {
    let _guard = TEST_LOCK.lock().await;
    let fixture = fixture().await;
    let id = fixture.draft(&rich(&fixture)).await;
    let ticket = fixture.compare(id).await.comparison_hash;
    let db = fixture.db();
    db.execute_batch("CREATE TRIGGER zz_fail BEFORE INSERT ON catalogue_seals BEGIN SELECT RAISE(ABORT,'synthetic seal failure'); END;").unwrap();
    let error = fixture
        .store
        .publish_catalogue(id, request(&ticket))
        .await
        .unwrap_err();
    assert!(matches!(error, CatalogueError::Storage(_)), "{error:?}");
    assert_eq!(fixture.published_rows(), [0; 7]);
    db.execute_batch("DROP TRIGGER zz_fail").unwrap();
    assert!(
        fixture
            .store
            .publish_catalogue(id, request(&ticket))
            .await
            .unwrap()
            .published
    );
}

#[tokio::test]
async fn cross_process_publish_race_has_one_winner() {
    let _guard = TEST_LOCK.lock().await;
    let fixture = fixture().await;
    let id = fixture.draft(&rich(&fixture)).await.to_string();
    let ticket = fixture.compare(id.parse().unwrap()).await.comparison_hash;
    let threads: Vec<_> = (0..2)
        .map(|_| {
            let database = fixture.database.clone();
            let (id, ticket) = (id.clone(), ticket.clone());
            std::thread::spawn(move || {
                Command::new(env!("CARGO_BIN_EXE_mimik"))
                    .arg("--database")
                    .arg(database)
                    .args([
                        "publish-catalogue",
                        "--id",
                        &id,
                        "--comparison-hash",
                        &ticket,
                        "--actor",
                        ACTOR,
                        "--rationale",
                        RATIONALE,
                    ])
                    .output()
                    .unwrap()
            })
        })
        .collect();
    let outputs: Vec<Output> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    assert_eq!(outputs.iter().filter(|o| o.status.success()).count(), 1);
    for output in &outputs {
        assert_redacted(output);
        if !output.status.success() {
            // The loser sees the winner's version in its recomputed ticket; never a lock error.
            assert!(
                String::from_utf8_lossy(&output.stderr).contains("comparison ticket is stale"),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
    assert_eq!(fixture.count("catalogue_versions"), 1);
    assert_eq!(fixture.count("catalogue_seals"), 1);
}

// ---------------------------------------------------------------------------------------
// Step-2 review fixes (plan §12): verification gaps, admission profiles, index damage.
// ---------------------------------------------------------------------------------------

/// A tampering connection: foreign keys off, as an operator's raw `SQLite` shell would be.
fn tamper_db(fixture: &Fixture) -> Connection {
    let db = Connection::open(&fixture.database).unwrap();
    db.busy_timeout(std::time::Duration::from_secs(5)).unwrap();
    db.execute_batch("PRAGMA foreign_keys=OFF").unwrap();
    db
}

fn show_spells(fixture: &Fixture) -> Output {
    fixture.cli(&[
        "show-catalogue-version",
        "--game",
        "synthetic-game",
        "--edition",
        "first",
        "--catalogue",
        "spells",
    ])
}

/// Verify through the CLI; the problems as sorted `(catalogue, version, code)` triples.
fn verify_problems(fixture: &Fixture) -> Vec<(i64, Option<i64>, String)> {
    let output = fixture.cli(&["verify-catalogues"]);
    assert_redacted(&output);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|_| {
        panic!(
            "verify did not report: {}",
            String::from_utf8_lossy(&output.stderr)
        )
    });
    assert_eq!(
        output.status.success(),
        report["status"] == "ok",
        "{report}"
    );
    let mut problems: Vec<(i64, Option<i64>, String)> = report["problems"]
        .as_array()
        .unwrap()
        .iter()
        .map(|problem| {
            (
                problem["catalogue"].as_i64().unwrap(),
                problem["version"].as_i64(),
                problem["code"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    problems.sort();
    problems
}

/// An expected integrity problem: catalogue surrogate, version number, static code.
type Problem = (i64, Option<i64>, &'static str);

fn expected(problems: &[Problem]) -> Vec<(i64, Option<i64>, String)> {
    let mut problems: Vec<_> = problems
        .iter()
        .map(|(catalogue, version, code)| (*catalogue, *version, (*code).to_owned()))
        .collect();
    problems.sort();
    problems
}

#[tokio::test]
async fn orphan_unit_rows_fail_show_and_verify() {
    let _guard = TEST_LOCK.lock().await;
    let fixture = fixture().await;
    let published = fixture.publish(&rich(&fixture)).await.unwrap();
    output_json(&show_spells(&fixture));
    let db = tamper_db(&fixture);
    // Guards dropped in this test database only, to model storage tampering: a unit row
    // whose entry key does not exist in the version (a valid unit, page and source).
    db.execute_batch(
        "DROP TRIGGER catalogue_entry_units_sealed; DROP TRIGGER catalogue_entry_units_insert;",
    )
    .unwrap();
    db.execute(
        "INSERT INTO catalogue_entry_units VALUES(?1,'zqxkey-orphan',0,?2,?3,?4)",
        params![
            published.version.version_id,
            fixture.unit,
            fixture.page,
            fixture.source
        ],
    )
    .unwrap();
    failed(&show_spells(&fixture), "projection-mismatch");
    let problems = verify_problems(&fixture);
    assert!(
        problems.contains(&(1, Some(1), "projection-mismatch".into())),
        "{problems:?}"
    );
}

#[tokio::test]
async fn versions_record_their_admission_profile_and_verify_dispatches_on_it() {
    let _guard = TEST_LOCK.lock().await;
    let fixture = fixture().await;
    let published = fixture.publish(&rich(&fixture)).await.unwrap();
    let version_id = published.version.version_id;
    let db = fixture.db();
    let profile: String = db
        .query_row(
            "SELECT admission_profile FROM catalogue_versions WHERE id=?1",
            [version_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(profile, "catalogue-admission-1");
    // The CHECK admits only known profiles, and a version must name one.
    let copy = |profile: &str| {
        format!(
            "INSERT INTO catalogue_versions(catalogue,game_id,edition_id,catalogue_id,version,title,content,content_hash,draft_id,draft_revision,comparison_hash,actor,rationale,admission_profile) SELECT catalogue,game_id,edition_id,catalogue_id,2,title,content,content_hash,draft_id,draft_revision,comparison_hash,actor,rationale,{profile} FROM catalogue_versions WHERE id={version_id}"
        )
    };
    for profile in ["'catalogue-admission-2'", "''", "NULL"] {
        let error = error_text(db.execute(&copy(profile), []));
        assert!(error.contains("constraint failed"), "{profile}: {error}");
    }
    // Verification dispatches on the stored profile: a profile this build does not know
    // (allowed here by widening the CHECK in this test database only) is reported, not
    // re-admitted under the current rules.
    let tamper = tamper_db(&fixture);
    let schema_version: i64 = tamper
        .query_row("PRAGMA schema_version", [], |row| row.get(0))
        .unwrap();
    tamper
        .execute_batch(&format!(
            "PRAGMA writable_schema=ON; UPDATE sqlite_schema SET sql=replace(sql,'IN (''catalogue-admission-1'')','IN (''catalogue-admission-1'',''catalogue-admission-0'')') WHERE name='catalogue_versions'; PRAGMA schema_version={}; PRAGMA writable_schema=OFF;",
            schema_version + 1
        ))
        .unwrap();
    drop(tamper);
    let tamper = tamper_db(&fixture);
    tamper
        .execute_batch("DROP TRIGGER catalogue_versions_update; UPDATE catalogue_versions SET admission_profile='catalogue-admission-0';")
        .unwrap();
    failed(&show_spells(&fixture), "unknown-admission-profile");
    assert_eq!(
        verify_problems(&fixture),
        expected(&[(1, Some(1), "unknown-admission-profile")])
    );
    tamper
        .execute_batch("UPDATE catalogue_versions SET admission_profile='catalogue-admission-1';")
        .unwrap();
    output_json(&show_spells(&fixture));
    assert_eq!(verify_problems(&fixture), expected(&[]));
}

#[tokio::test]
async fn verify_reports_a_missing_or_reshaped_name_index_without_aborting() {
    let _guard = TEST_LOCK.lock().await;
    let fixture = fixture().await;
    fixture.publish(&rich(&fixture)).await.unwrap();
    let db = fixture.db();
    for (damage, code) in [
        ("DROP TABLE catalogue_names_fts", "name-index-missing"),
        (
            "DROP TABLE catalogue_names_fts; CREATE VIRTUAL TABLE catalogue_names_fts USING fts5(name, other UNINDEXED)",
            "name-index-definition-mismatch",
        ),
        (
            "DROP TABLE catalogue_names_fts; CREATE TABLE catalogue_names_fts(name TEXT)",
            "name-index-definition-mismatch",
        ),
    ] {
        db.execute_batch(damage).unwrap();
        let report = fixture.store.verify_catalogues().await.unwrap();
        assert_eq!(
            report
                .problems
                .iter()
                .map(|p| (p.catalogue, p.version, p.code))
                .collect::<Vec<_>>(),
            [(0, None, code)],
            "{damage}"
        );
        assert_eq!((report.versions, report.status), (1, "failed"));
        assert_eq!(verify_problems(&fixture), expected(&[(0, None, code)]));
        fixture.store.rebuild_catalogue_index().await.unwrap();
        assert_eq!(
            fixture.store.verify_catalogues().await.unwrap().status,
            "ok",
            "{damage}"
        );
    }
}

#[tokio::test]
async fn verify_reports_foreign_keys_sequence_seals_identity_and_extra_rows() {
    let _guard = TEST_LOCK.lock().await;
    let fixture = fixture().await;
    let base = rich(&fixture);
    fixture.publish(&base).await.unwrap();
    let mut next = base.clone();
    next["entries"].as_array_mut().unwrap().push(json!({
        "key": "zqxkey-added", "type": "charm", "name": "Added zqxname", "properties": {},
        "description": {"owner_text": "zqxowner added text"}}));
    let second = fixture.publish(&next).await.unwrap();
    let v2 = second.version.version_id;
    assert_eq!(verify_problems(&fixture), expected(&[]));
    let db = tamper_db(&fixture);
    db.execute_batch(
        "DROP TRIGGER catalogue_entry_types_sealed; DROP TRIGGER catalogue_entry_types_delete;
         DROP TRIGGER catalogue_entries_delete; DROP TRIGGER catalogue_seals_delete;
         DROP TRIGGER catalogue_versions_update; DROP TRIGGER catalogues_update;
         DROP TRIGGER catalogues_delete;",
    )
    .unwrap();
    let fk = (0, None, "foreign-key-violation");
    let cases: [(String, String, Vec<Problem>); 6] = [
        (
            "INSERT INTO catalogue_entry_types VALUES(1,'extra',9,'Extra','[]')".into(),
            "DELETE FROM catalogue_entry_types WHERE version_id=1 AND type_id='extra'".into(),
            vec![(1, Some(1), "projection-mismatch")],
        ),
        (
            "UPDATE catalogues SET catalogue_id='renamed'".into(),
            "UPDATE catalogues SET catalogue_id='spells'".into(),
            vec![
                fk,
                (1, Some(1), "version-identity-mismatch"),
                (1, Some(2), "version-identity-mismatch"),
            ],
        ),
        (
            format!("DELETE FROM catalogue_seals WHERE version_id={v2}"),
            format!("INSERT INTO catalogue_seals(version_id) VALUES({v2})"),
            // The derived name rows of version 2 no longer belong to the latest sealed version.
            vec![
                (1, Some(2), "version-not-sealed"),
                (0, None, "name-index-mismatch"),
                (0, None, "entry-names-mismatch"),
            ],
        ),
        (
            "INSERT INTO catalogue_entries VALUES(99,'orphan',0,'spell','Orphan',0,'owner_text','x','{}')".into(),
            "DELETE FROM catalogue_entries WHERE version_id=99".into(),
            vec![fk],
        ),
        (
            format!("UPDATE catalogue_versions SET version=3 WHERE id={v2}"),
            format!("UPDATE catalogue_versions SET version=2 WHERE id={v2}"),
            vec![(1, Some(3), "version-sequence-gap")],
        ),
        (
            "INSERT INTO catalogues(game_id,edition_id,catalogue_id) VALUES('synthetic-game','first','orphaned')".into(),
            "DELETE FROM catalogues WHERE catalogue_id='orphaned'".into(),
            vec![(2, None, "catalogue-without-versions")],
        ),
    ];
    for (tamper, restore, problems) in cases {
        db.execute_batch(&tamper).unwrap();
        assert_eq!(verify_problems(&fixture), expected(&problems), "{tamper}");
        db.execute_batch(&restore).unwrap();
        assert_eq!(verify_problems(&fixture), expected(&[]), "{restore}");
    }
}

#[tokio::test]
async fn identical_content_citing_a_now_rejected_unit_is_refused() {
    let _guard = TEST_LOCK.lock().await;
    let fixture = fixture().await;
    let base = rich(&fixture);
    fixture.publish(&base).await.unwrap();
    let rows = fixture.published_rows();
    fixture.review_unit("rejected").await;
    let id = fixture.draft(&base).await;
    let compared = fixture.compare(id).await;
    assert!(compared.changes.identical);
    assert_eq!(compared.refusals.rejected_units, 1);
    assert!(!compared.publishable);
    let error = fixture
        .store
        .publish_catalogue(id, request(&compared.comparison_hash))
        .await
        .unwrap_err();
    assert_eq!(code(&error), "rejected-units");
    assert_eq!(fixture.published_rows(), rows);
}

#[tokio::test]
async fn a_failed_replacement_publication_keeps_the_previous_index_rows() {
    let _guard = TEST_LOCK.lock().await;
    let fixture = fixture().await;
    let base = rich(&fixture);
    fixture.publish(&base).await.unwrap();
    let index = || -> Vec<(i64, String, i64, String)> {
        fixture
            .db()
            .prepare(
                "SELECT rowid,name,catalogue_row_id,key FROM catalogue_names_fts ORDER BY rowid",
            )
            .unwrap()
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    let before = index();
    let rows = fixture.published_rows();
    let mut next = base.clone();
    next["entries"][0]["name"] = "Renamed zqxname".into();
    let id = fixture.draft(&next).await;
    let ticket = fixture.compare(id).await.comparison_hash;
    fixture
        .db()
        .execute_batch("CREATE TRIGGER zz_fail BEFORE INSERT ON catalogue_seals BEGIN SELECT RAISE(ABORT,'synthetic seal failure'); END;")
        .unwrap();
    let error = fixture
        .store
        .publish_catalogue(id, request(&ticket))
        .await
        .unwrap_err();
    assert!(matches!(error, CatalogueError::Storage(_)), "{error:?}");
    assert_eq!(index(), before);
    assert_eq!(fixture.published_rows(), rows);
    fixture.db().execute_batch("DROP TRIGGER zz_fail").unwrap();
    assert_eq!(
        fixture.store.verify_catalogues().await.unwrap().status,
        "ok"
    );
}

#[tokio::test]
async fn reverse_key_order_entries_with_unsorted_references_publish_show_and_verify() {
    let _guard = TEST_LOCK.lock().await;
    let fixture = fixture().await;
    let (source_b, page_b, unit_b) = second_source(&fixture).await;
    assert!(unit_b > fixture.unit);
    let low =
        json!({"source_id": fixture.source, "page_id": fixture.page, "unit_id": fixture.unit});
    let high = json!({"source_id": source_b, "page_id": page_b, "unit_id": unit_b});
    let mut value = rich(&fixture);
    value["sources"]
        .as_array_mut()
        .unwrap()
        .push(json!({"source_id": source_b, "pdf_sha256": fixture.pdf_hash}));
    // Unit-backed entries in reverse lexical key order, each citing the higher unit id first;
    // an owner-written entry between them.
    let spell = |key: &str, description: Value| {
        json!({"key": key, "type": "spell", "name": "Name zqxname",
               "properties": {"zqxprop": "zqxvalue", "level": 1}, "description": description})
    };
    value["entries"] = json!([
        spell("zqxkey-zulu", json!({"units": [high, low]})),
        spell("zqxkey-mike", json!({"owner_text": "zqxowner text"})),
        spell("zqxkey-alpha", json!({"units": [high, low]})),
    ]);
    let first = fixture.publish(&value).await.unwrap();
    let rows: Vec<(String, i64, i64)> = fixture
        .db()
        .prepare("SELECT key,ordinal,unit_id FROM catalogue_entry_units WHERE version_id=?1 ORDER BY key,ordinal")
        .unwrap()
        .query_map([first.version.version_id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let (low_id, high_id) = (fixture.unit, unit_b);
    assert_eq!(
        rows,
        [
            ("zqxkey-alpha".into(), 0, high_id),
            ("zqxkey-alpha".into(), 1, low_id),
            ("zqxkey-zulu".into(), 0, high_id),
            ("zqxkey-zulu".into(), 1, low_id),
        ]
    );
    let shown = output_json(&show_spells(&fixture));
    assert_eq!(
        (&shown["version"], &shown["content_hash"]),
        (&json!(1), &json!(first.version.content_hash))
    );
    // An update that only reorders one entry's references.
    value["entries"][2]["description"]["units"] = json!([low, high]);
    let id = fixture.draft(&value).await;
    let compared = fixture.compare(id).await;
    assert_eq!(compared.changes.description_references_changed, 1);
    let second = fixture
        .store
        .publish_catalogue(id, request(&compared.comparison_hash))
        .await
        .unwrap();
    let shown = output_json(&show_spells(&fixture));
    assert_eq!(
        (&shown["version"], &shown["content_hash"]),
        (&json!(2), &json!(second.version.content_hash))
    );
    let verified = output_json(&fixture.cli(&["verify-catalogues"]));
    assert_eq!(
        verified,
        json!({"catalogues": 1, "versions": 2, "index_rows": 3, "problems": [], "status": "ok"})
    );
}

#[tokio::test]
async fn cli_reference_limit_rejection_is_redacted() {
    let _guard = TEST_LOCK.lock().await;
    let fixture = fixture().await;
    let mut value = fixture.catalogue();
    // 385 entries of 16 distinct units each: 6,160 references, over the 6,144 cap (plan
    // §14). Admission refuses the total before any source check.
    let units: Vec<Value> = (1..=16)
        .map(|unit| json!({"source_id": fixture.source, "page_id": fixture.page, "unit_id": unit}))
        .collect();
    value["entries"] = (0..385)
        .map(|entry| {
            json!({"key": format!("zqxkey-{entry}"), "type": "spell", "name": "Name zqxname",
                   "properties": {"zqxprop": "zqxvalue", "level": 1},
                   "description": {"units": units}})
        })
        .collect();
    let file = fixture.write("spells.json", &serde_json::to_vec(&value).unwrap());
    failed(&fixture.import(&file, 0), "unit-reference-limit");
    assert_eq!(fixture.drafts(), 0);
}

/// Assisted sheet step 3a: the normalised entry names behind linking on view. The seal replaces
/// a catalogue's rows (latest sealed version only), `verify-catalogues` recomputes them and
/// the marker, `rebuild-catalogue-index` repairs them, and store open rebuilds them when the
/// marker differs from the build's.
#[tokio::test]
async fn entry_names_follow_the_seal_and_are_verified_rebuilt_and_marked() {
    let _guard = TEST_LOCK.lock().await;
    let fixture = fixture().await;
    let names = || -> Vec<(i64, String, String)> {
        fixture
            .db()
            .prepare("SELECT version_id,entry_key,norm FROM catalogue_entry_names ORDER BY version_id,entry_key")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    let marker = || -> Option<String> {
        fixture
            .db()
            .query_row(
                "SELECT version FROM catalogue_name_fold WHERE id=1",
                [],
                |row| row.get(0),
            )
            .optional()
            .unwrap()
    };
    let base = rich(&fixture);
    fixture.publish(&base).await.unwrap();
    let first = names();
    assert_eq!(
        first,
        [
            (1, "zqxkey-charm".into(), "charm zqxname".into()),
            (1, "zqxkey-flare".into(), "name zqxname".into())
        ]
    );
    // A new sealed version replaces the rows: the index holds the latest version only.
    let mut next = base.clone();
    next["entries"][0]["name"] = "\u{c9}clair  STRASSE".into();
    fixture.publish(&next).await.unwrap();
    assert_eq!(
        names(),
        [
            (2, "zqxkey-charm".into(), "charm zqxname".into()),
            (2, "zqxkey-flare".into(), "eclair strasse".into())
        ]
    );
    let build_marker = marker().expect("the marker is written at open");
    assert_eq!(
        fixture.store.verify_catalogues().await.unwrap().status,
        "ok"
    );
    // Each tampering is reported by one code and repaired by the rebuild.
    for (tamper, code) in [
        (
            "DELETE FROM catalogue_entry_names WHERE entry_key='zqxkey-flare'",
            "entry-names-mismatch",
        ),
        (
            "INSERT INTO catalogue_entry_names VALUES (2,'stray','stray')",
            "entry-names-mismatch",
        ),
        (
            "UPDATE catalogue_entry_names SET norm='charm' WHERE entry_key='zqxkey-charm'",
            "entry-names-mismatch",
        ),
        (
            "UPDATE catalogue_entry_names SET version_id=1",
            "entry-names-mismatch",
        ),
        (
            "UPDATE catalogue_name_fold SET version='an older rule'",
            "entry-names-marker-mismatch",
        ),
    ] {
        fixture.db().execute(tamper, []).unwrap();
        let report = fixture.store.verify_catalogues().await.unwrap();
        assert_eq!(
            report.problems.iter().map(|p| p.code).collect::<Vec<_>>(),
            [code],
            "{tamper}"
        );
        fixture.store.rebuild_catalogue_index().await.unwrap();
        assert_eq!(
            fixture.store.verify_catalogues().await.unwrap().status,
            "ok",
            "{tamper}"
        );
        assert_eq!(marker().as_deref(), Some(build_marker.as_str()));
    }
}

/// A marker that differs from the build's rebuilds the rows when the store opens.
#[tokio::test]
async fn a_changed_name_marker_rebuilds_the_entry_names_at_store_open() {
    let _guard = TEST_LOCK.lock().await;
    let fixture = fixture().await;
    let names = || -> Vec<(i64, String)> {
        fixture
            .db()
            .prepare("SELECT version_id,entry_key FROM catalogue_entry_names ORDER BY version_id,entry_key")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    let marker = || -> String {
        fixture
            .db()
            .query_row(
                "SELECT version FROM catalogue_name_fold WHERE id=1",
                [],
                |row| row.get(0),
            )
            .unwrap()
    };
    fixture.publish(&rich(&fixture)).await.unwrap();
    let build_marker = marker();
    // A marker that differs from the build's rebuilds the rows when the store opens.
    fixture
        .db()
        .execute_batch("UPDATE catalogue_name_fold SET version='an older rule'; DELETE FROM catalogue_entry_names; INSERT INTO catalogue_entry_names VALUES (1,'old','old');")
        .unwrap();
    drop(Store::open(&fixture.database).unwrap());
    assert_eq!(
        names(),
        [
            (1, "zqxkey-charm".to_owned()),
            (1, "zqxkey-flare".to_owned())
        ]
    );
    assert_eq!(marker(), build_marker);
    assert_eq!(
        fixture.store.verify_catalogues().await.unwrap().status,
        "ok"
    );
    // A matching marker leaves the rows alone at open.
    fixture
        .db()
        .execute(
            "DELETE FROM catalogue_entry_names WHERE entry_key='zqxkey-charm'",
            [],
        )
        .unwrap();
    drop(Store::open(&fixture.database).unwrap());
    assert_eq!(names().len(), 1);
}
