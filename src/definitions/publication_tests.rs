use super::*;
/// The shared synthetic source fixture, also used by campaign route tests.
pub(crate) mod fixture {
    include!("../../tests/support/source_fixture.rs");
}

pub(super) fn assert_one(before: usize) {
    assert_eq!(
        SNAPSHOTS.with(std::cell::Cell::get) - before,
        1,
        "one evidence snapshot per approval"
    );
}

#[test]
fn snapshot_budget_counts_utf8_and_rejects_boundary_and_counter_overflow() {
    let data = "界";
    let bytes = encode(&data).unwrap().len() + 1;
    let mut used = SNAPSHOT_LIMIT - bytes;
    budget(&data, &mut used).unwrap();
    assert_eq!(used, SNAPSHOT_LIMIT);
    assert!(
        matches!(budget(&data, &mut used), Err(DefinitionError::Invalid(report)) if report.diagnostics[0].code == "snapshot-byte-limit")
    );
    let mut used = usize::MAX;
    assert!(
        matches!(budget(&data, &mut used), Err(DefinitionError::Invalid(report)) if report.diagnostics[0].code == "snapshot-byte-limit")
    );
}

#[tokio::test]
async fn approval_builds_one_evidence_snapshot_per_operation() {
    let _guard = super::super::TEST_RESOURCE_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    fixture::fixture_pair(dir.path()).unwrap();
    let store = Store::open(&dir.path().join("single.sqlite3")).unwrap();
    store
        .register_ruleset(crate::registry::RegisterRuleset {
            game_id: "synthetic-game".into(),
            game_label: "Synthetic game".into(),
            edition_id: "first".into(),
            edition_label: "First edition".into(),
        })
        .await
        .unwrap();
    store
        .import_source(
            &dir.path().join("unknown-bundle"),
            &dir.path().join("original.pdf"),
        )
        .await
        .unwrap();
    let source = store.sources().await.unwrap().remove(0);
    let page = store.source_page(source.pages[0].id).await.unwrap();
    let mut body: serde_json::Value =
        serde_json::from_str(include_str!("../../schemas/definition-example.json")).unwrap();
    body["sources"] =
        serde_json::json!([{"source_id":source.id,"pdf_sha256":source.pdf_hash,"roles":["sheet"]}]);
    body["fields"][0]["mapping"] = serde_json::json!({"state":"mapped","regions":[{"source_id":source.id,"page_id":page.id,"unit_id":page.units[0].id}]});
    let draft = store
        .save_definition_draft(serde_json::to_vec(&body).unwrap(), 0)
        .await
        .unwrap();
    let comparison = store.compare_definition(draft.id, false).await.unwrap();
    store
        .approve_definition(
            draft.id,
            false,
            ApprovalRequest {
                comparison_hash: comparison.comparison_hash,
                actor: "fixture".into(),
                rationale: "Synthetic".into(),
                originals_compared: true,
            },
        )
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_publication_candidate_keeps_admission_until_db_worker_finishes() {
    let _guard = super::super::TEST_RESOURCE_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("cancel.sqlite3")).unwrap();
    let (started, entered) = tokio::sync::oneshot::channel();
    let (release, unblock) = std::sync::mpsc::channel();
    let locked = store.clone();
    let lock = tokio::spawn(async move {
        locked
            .run(move |_| {
                started.send(()).unwrap();
                unblock.recv().unwrap();
                Ok(())
            })
            .await
    });
    entered.await.unwrap();
    let pending = store.clone();
    let (started, enqueued) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        use std::future::Future;
        let mut operation = Box::pin(candidate(&pending, "draft", 1));
        let mut started = Some(started);
        std::future::poll_fn(|cx| {
            let value = operation.as_mut().poll(cx);
            if let Some(sender) = started.take() {
                sender.send(()).unwrap();
            }
            value
        })
        .await
    });
    enqueued.await.unwrap();
    task.abort();
    assert!(matches!(task.await,Err(error) if error.is_cancelled()));
    assert!(matches!(admit(), Err(DefinitionError::Busy)));
    release.send(()).unwrap();
    lock.await.unwrap().unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if let Ok(permit) = admit() {
                drop(permit);
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

/// One registered pair, one imported source with sheet+core roles, and its first unit.
async fn formula_store(dir: &std::path::Path) -> (Store, serde_json::Value) {
    fixture::fixture_pair(dir).unwrap();
    let store = Store::open(&dir.join("formula.sqlite3")).unwrap();
    store
        .register_ruleset(crate::registry::RegisterRuleset {
            game_id: "synthetic-game".into(),
            game_label: "Synthetic game".into(),
            edition_id: "first".into(),
            edition_label: "First edition".into(),
        })
        .await
        .unwrap();
    store
        .import_source(&dir.join("unknown-bundle"), &dir.join("original.pdf"))
        .await
        .unwrap();
    let source = store.sources().await.unwrap().remove(0);
    let page = store.source_page(source.pages[0].id).await.unwrap();
    let region =
        serde_json::json!({"source_id":source.id,"page_id":page.id,"unit_id":page.units[0].id});
    let mut body: serde_json::Value = serde_json::from_str(include_str!(
        "../../schemas/definition-formula-example.json"
    ))
    .unwrap();
    body["sources"] = serde_json::json!([{"source_id":source.id,"pdf_sha256":source.pdf_hash,"roles":["sheet","core"]}]);
    body["fields"][0]["mapping"] = serde_json::json!({"state":"mapped","regions":[region]});
    body["fields"][4]["formula"]["evidence"] =
        serde_json::json!({"state":"mapped","regions":[region]});
    (store, body)
}

fn publication_code(error: &DefinitionError) -> &str {
    match error {
        DefinitionError::Invalid(report) => &report.diagnostics[0].code,
        _ => "other",
    }
}

async fn approve(store: &Store, id: i64) -> Result<PublishedDefinition, DefinitionError> {
    let comparison = store.compare_definition(id, false).await?;
    store
        .approve_definition(
            id,
            false,
            ApprovalRequest {
                comparison_hash: comparison.comparison_hash,
                actor: "fixture".into(),
                rationale: "Synthetic".into(),
                originals_compared: true,
            },
        )
        .await
}

#[tokio::test]
async fn list_definition_publishes() {
    let _guard = super::super::TEST_RESOURCE_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let (store, body) = formula_store(dir.path()).await;
    let draft = store
        .save_definition_draft(serde_json::to_vec(&body).unwrap(), 0)
        .await
        .unwrap();
    // M4 lifted the list gate: the list formula definition publishes, advisory.
    let published = approve(&store, draft.id).await.unwrap();
    assert_eq!(published.deterministic_rules, "formulas-advisory");
    assert!(
        published
            .definition
            .fields
            .iter()
            .any(|field| field.control == super::super::Control::List)
    );
    // Without the list field (and the formulas that read it) a formula definition publishes:
    // M3 evaluates it on view, save and history. Its results are advisory.
    let mut scalar = body.clone();
    scalar["fields"].as_array_mut().unwrap().remove(2);
    scalar["fields"][2]["formula"]["expr"] = "vitality + 1".into();
    scalar["groups"][0]["fields"] = serde_json::json!(["name", "vitality", "armor", "pace"]);
    scalar["constraints"][0]["expr"] = "pace > 0".into();
    let draft = store
        .save_definition_draft(serde_json::to_vec(&scalar).unwrap(), 1)
        .await
        .unwrap();
    let published = approve(&store, draft.id).await.unwrap();
    assert_eq!(published.deterministic_rules, "formulas-advisory");
    assert_eq!(published.unresolved_formulas, vec!["armor".to_owned()]);
    assert_eq!(
        published.unresolved_constraints,
        vec!["one-outer-armor".to_owned()]
    );
    assert!(published.definition.formula_profile.is_some());
}

#[tokio::test]
async fn formula_evidence_references_are_verified_like_field_mappings() {
    let _guard = super::super::TEST_RESOURCE_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let (store, mut body) = formula_store(dir.path()).await;
    let unit = body["fields"][4]["formula"]["evidence"]["regions"][0]["unit_id"]
        .as_i64()
        .unwrap();
    body["fields"][4]["formula"]["evidence"]["regions"][0]["unit_id"] = (unit + 1000).into();
    let error = store
        .save_definition_draft(serde_json::to_vec(&body).unwrap(), 0)
        .await
        .unwrap_err();
    assert_eq!(
        publication_code(&error),
        "evidence-unit-page-source-mismatch"
    );
    let DefinitionError::Invalid(report) = &error else {
        unreachable!()
    };
    assert_eq!(report.diagnostics[0].path, "/fields/4/formula/evidence");
}

/// Encoding sites: `definition_approvals.encoding CHECK(encoding=1)` (008), the provenance
/// trigger's fixed key set (009), the INSERT in `approve_definition` and the
/// `encoding != 1` gate in `verify_approval`. A tampered encoding-2 snapshot of a
/// formula-free definition must be rejected, not reinterpreted.
#[tokio::test]
async fn encoding_two_snapshots_are_rejected_for_formula_free_definitions() {
    let _guard = super::super::TEST_RESOURCE_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = formula_store(dir.path()).await;
    let source = store.sources().await.unwrap().remove(0);
    let page = store.source_page(source.pages[0].id).await.unwrap();
    let mut body: serde_json::Value =
        serde_json::from_str(include_str!("../../schemas/definition-example.json")).unwrap();
    body["sources"] =
        serde_json::json!([{"source_id":source.id,"pdf_sha256":source.pdf_hash,"roles":["sheet"]}]);
    body["fields"][0]["mapping"] = serde_json::json!({"state":"mapped","regions":[{"source_id":source.id,"page_id":page.id,"unit_id":page.units[0].id}]});
    let draft = store
        .save_definition_draft(serde_json::to_vec(&body).unwrap(), 0)
        .await
        .unwrap();
    let published = approve(&store, draft.id).await.unwrap();
    assert_eq!(published.deterministic_rules, "unsupported");
    assert!(store.definition_version(published.id).await.is_ok());
    // Tamper consistently (matching snapshot hash) so only the encoding gate can reject it.
    store
        .run(|db| {
            db.execute_batch(
                "PRAGMA ignore_check_constraints=ON;
                 DROP TRIGGER definition_approvals_update;
                 UPDATE definition_approvals SET encoding=2,
                   snapshot=json_set(snapshot,'$.encoding',2);",
            )?;
            let snapshot: String =
                db.query_row("SELECT snapshot FROM definition_approvals", [], |row| {
                    row.get(0)
                })?;
            let rehashed = hash(b"mimik-wiring-snapshot-v1\0", snapshot.as_bytes());
            db.execute(
                "UPDATE definition_approvals SET snapshot_hash=?1",
                [rehashed],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let error = store.definition_version(published.id).await.unwrap_err();
    assert_eq!(
        publication_code(&error),
        "stored-approval-snapshot-integrity"
    );
}

/// Adds a second text unit to the importable synthetic bundle, so evidence can cite a unit
/// that no field mapping uses.
fn add_second_unit(dir: &std::path::Path) {
    let bundle = dir.join("unknown-bundle");
    let mut raw: serde_json::Value =
        serde_json::from_slice(&std::fs::read(bundle.join("source.json")).unwrap()).unwrap();
    let polygon = serde_json::json!([[10, 60], [100, 60], [100, 90], [10, 90]]);
    let html = "<p>Pace equals six minus armor.</p>";
    raw["children"][0]["children"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({"id":"/page/0/Text/2","block_type":"Text","polygon":polygon,"html":html,"children":null}));
    let raw = serde_json::to_vec(&raw).unwrap();
    std::fs::write(bundle.join("source.json"), &raw).unwrap();
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(bundle.join("bundle.json")).unwrap()).unwrap();
    manifest["extraction_sha256"] = fixture::hash(&raw).into();
    manifest["pages"][0]["units"].as_array_mut().unwrap().push(
        serde_json::json!({"id":"/page/0/Text/2","kind":"Text","polygon":polygon,"html":html}),
    );
    std::fs::write(
        bundle.join("bundle.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
}

#[tokio::test]
async fn formula_evidence_units_are_captured_in_the_wiring_snapshot() {
    let _guard = super::super::TEST_RESOURCE_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    fixture::fixture_pair(dir.path()).unwrap();
    add_second_unit(dir.path());
    let store = Store::open(&dir.path().join("evidence.sqlite3")).unwrap();
    store
        .register_ruleset(crate::registry::RegisterRuleset {
            game_id: "synthetic-game".into(),
            game_label: "Synthetic game".into(),
            edition_id: "first".into(),
            edition_label: "First edition".into(),
        })
        .await
        .unwrap();
    store
        .import_source(
            &dir.path().join("unknown-bundle"),
            &dir.path().join("original.pdf"),
        )
        .await
        .unwrap();
    let source = store.sources().await.unwrap().remove(0);
    let page = store.source_page(source.pages[0].id).await.unwrap();
    assert_eq!(page.units.len(), 2);
    let (wired, cited) = (page.units[0].id, page.units[1].id);
    let mut body: serde_json::Value = serde_json::from_str(include_str!(
        "../../schemas/definition-formula-example.json"
    ))
    .unwrap();
    body["sources"] = serde_json::json!([{"source_id":source.id,"pdf_sha256":source.pdf_hash,"roles":["sheet","core"]}]);
    body["fields"][0]["mapping"] = serde_json::json!({"state":"mapped","regions":[{"source_id":source.id,"page_id":page.id,"unit_id":wired}]});
    body["constraints"][0]["evidence"] = serde_json::json!({"state":"mapped","regions":[{"source_id":source.id,"page_id":page.id,"unit_id":cited}]});
    let draft = store
        .save_definition_draft(serde_json::to_vec(&body).unwrap(), 0)
        .await
        .unwrap();
    let before = store.compare_definition(draft.id, false).await.unwrap();
    let captured = |comparison: &Comparison| {
        comparison
            .snapshot
            .units
            .iter()
            .find(|unit| unit.id == cited)
            .map(|unit| unit.revision)
    };
    assert_eq!(
        captured(&before),
        Some(1),
        "evidence-only unit must be captured"
    );
    store
        .review_source_unit(
            page.id,
            cited,
            crate::sources::UnitReview {
                revision: 1,
                state: "corrected".into(),
                correction: "Pace equals six minus armor (corrected).".into(),
                actor: "fixture".into(),
                rationale: "Synthetic correction".into(),
            },
        )
        .await
        .unwrap();
    let after = store.compare_definition(draft.id, false).await.unwrap();
    assert_eq!(captured(&after), Some(2));
    assert_ne!(before.comparison_hash, after.comparison_hash);
}

mod budget {
    use crate::campaigns::values::{ENVELOPE_NODES, FORM_LIMITS, FormCounts, form_budget, within};
    use crate::definitions::Definition;
    use serde_json::{Value, json};

    fn property(n: usize, control: &str, optional: bool) -> Value {
        let mut value = json!({"id": format!("p{n}"), "label": "P", "control": control,
                               "optional": optional});
        if control == "enum" {
            value["options"] = (0..128)
                .map(|o| json!({"id": format!("o{o}"), "label": "O"}))
                .collect();
        }
        value
    }

    fn scalar(n: usize, control: &str, nullable: bool, formula: bool) -> Value {
        let mut value = json!({"id": format!("s{n}"), "label": "S", "control": control,
                               "nullable": nullable,
                               "mapping": {"state": "unresolved", "reason": "x"}});
        if control == "enum" {
            value["options"] = (0..128)
                .map(|o| json!({"id": format!("o{o}"), "label": "O"}))
                .collect();
        }
        if formula {
            value["formula"] = json!({"expr": "1", "rule_ids": [],
                                      "evidence": {"state": "unresolved", "reason": "x"}});
        }
        value
    }

    /// Built by serde directly: these extremes exceed admission's size limits on purpose.
    fn definition(fields: Vec<Value>) -> Definition {
        serde_json::from_value(json!({
            "contract_version": 1, "game_id": "g", "edition_id": "e", "definition_id": "d",
            "title": "T", "sources": [], "fields": Value::Array(fields), "groups": [],
            "formula_profile": 1
        }))
        .unwrap()
    }

    fn list(n: usize, properties: Vec<Value>) -> Value {
        json!({"id": format!("l{n}"), "label": "L", "control": "list", "nullable": false,
               "mapping": {"state": "unresolved", "reason": "x"},
               "items": {"properties": Value::Array(properties)}})
    }

    #[test]
    fn form_budget_counts_match_arithmetic() {
        // A nullable enum of 128 options, a boolean, a derived integer and one list with a
        // required integer and an optional enum of 128 options.
        let counts = form_budget(&definition(vec![
            scalar(0, "enum", true, false),
            scalar(1, "boolean", false, false),
            scalar(2, "integer", false, true),
            list(
                0,
                vec![property(0, "integer", false), property(1, "enum", true)],
            ),
        ]));
        let per_row = 1 + (1 + 129 + 2);
        assert_eq!(
            counts,
            FormCounts {
                units: (1 + 129 + 2) + (1 + 3) + 1 + (4 + 256 * (4 + per_row)),
                pairs: 5 + 2 + 1 + 256 * (1 + 2 + 1),
                nodes: ENVELOPE_NODES + 1 + 2 + (1 + 256 * (2 + 2)),
            }
        );
    }

    #[test]
    fn within_refuses_each_count_with_test_limits() {
        let limits = FormCounts {
            units: 10,
            pairs: 20,
            nodes: 30,
        };
        assert!(within(limits, limits), "at every limit");
        for (counts, which) in [
            (
                FormCounts {
                    units: 11,
                    ..limits
                },
                "units",
            ),
            (
                FormCounts {
                    pairs: 21,
                    ..limits
                },
                "pairs",
            ),
            (
                FormCounts {
                    nodes: 31,
                    ..limits
                },
                "nodes",
            ),
        ] {
            assert!(!within(counts, limits), "{which} over its limit");
        }
        assert_eq!(FORM_LIMITS.units, 32_768);
        assert_eq!(FORM_LIMITS.pairs, 65_536);
        assert_eq!(FORM_LIMITS.nodes, 32_768);
    }

    #[test]
    fn pairs_and_nodes_are_dominated_by_render_units() {
        const { assert!(ENVELOPE_NODES < 514) };
        let lists = |control: &str, optional: bool| -> Vec<Value> {
            (0..32)
                .map(|l| list(l, (0..32).map(|p| property(p, control, optional)).collect()))
                .collect()
        };
        let extremes: Vec<(&str, Vec<Value>)> = vec![
            ("32 lists x 32 integers", lists("integer", false)),
            ("32 lists x 32 enums of 128", lists("enum", false)),
            ("32 lists x 32 optional integers", lists("integer", true)),
            (
                "one list of one integer",
                vec![list(0, vec![property(0, "integer", false)])],
            ),
            (
                "256 nullable enums",
                (0..256).map(|n| scalar(n, "enum", true, false)).collect(),
            ),
            (
                "256 texts",
                (0..256).map(|n| scalar(n, "text", false, false)).collect(),
            ),
            (
                "256 derived",
                (0..256)
                    .map(|n| scalar(n, "integer", false, true))
                    .collect(),
            ),
        ];
        for (name, fields) in extremes {
            let has_list = fields.iter().any(|field| field["control"] == "list");
            let counts = form_budget(&definition(fields));
            assert!(counts.pairs <= counts.units + 5, "{name}: {counts:?}");
            if has_list {
                assert!(counts.nodes < counts.units, "{name}: {counts:?}");
            } else {
                assert!(counts.nodes <= ENVELOPE_NODES + 257, "{name}: {counts:?}");
            }
        }
    }
}
