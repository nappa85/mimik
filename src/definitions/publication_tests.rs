use super::*;
mod fixture {
    use crate as mimik;
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
