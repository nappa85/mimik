use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use mimik::{
    AppError, app,
    mork_advice::{Context, Truth, evaluate},
    mork_borg::Sheet,
};
use mimik::{CharacterInput, Store, mork_borg::EDITION};
use tower::ServiceExt;

fn confirmed(revision: i64) -> Context {
    Context {
        revision,
        base: Truth::Yes,
        can_act: Truth::Yes,
        action_remaining: Truth::Yes,
        movement_remaining: Truth::Yes,
        melee_reach: Truth::Yes,
        ranged_ready: Truth::Yes,
        ranged_in_range: Truth::Yes,
        incoming_attack: Truth::Yes,
        scroll_available: Truth::Yes,
        blocking_armor: Truth::No,
        zweihander: Truth::No,
        disoriented: Truth::No,
        route_clear: Truth::Yes,
        power_uses: "3".into(),
        goal: String::new(),
    }
}

#[test]
fn missing_evidence_scope_and_broken_state_never_manufacture_targets() {
    let sheet = Sheet::default();
    for (saved, mut context) in [(false, confirmed(1)), (true, Context::default())] {
        context.revision = 1;
        let advice = evaluate(&sheet, saved, &context).unwrap();
        assert!(
            advice
                .hints
                .iter()
                .all(|hint| hint.target.is_none() && hint.status == "needs context")
        );
    }
    let broken = Sheet { hp: 0, ..sheet };
    let advice = evaluate(&broken, true, &confirmed(1)).unwrap();
    assert!(advice.hints.iter().all(|hint| hint.target.is_none()));
    assert!(
        advice
            .notes
            .iter()
            .any(|note| note.contains("broken outcome"))
    );
}

#[test]
fn ordinary_action_budget_does_not_block_movement_or_triggered_defense() {
    let mut context = confirmed(1);
    context.action_remaining = Truth::No;
    let advice = evaluate(&Sheet::default(), true, &context).unwrap();
    for hint in &advice.hints {
        if ["melee", "ranged", "power"].contains(&hint.key) {
            assert_eq!(hint.status, "blocked standard path");
        } else {
            assert_eq!(hint.status, "standard check supported");
        }
    }
    let defense = advice
        .hints
        .iter()
        .find(|hint| hint.key == "defense")
        .unwrap();
    assert_eq!(defense.target, Some(12));
    assert!(defense.explanation.contains("immediate free attack"));
    context.action_remaining = Truth::Yes;
    context.movement_remaining = Truth::No;
    let advice = evaluate(&Sheet::default(), true, &context).unwrap();
    assert_eq!(advice.hints[0].status, "standard check supported");
    assert_eq!(advice.hints[3].status, "blocked standard path");
}

#[test]
fn disorientation_cannot_endorse_a_normal_power_success_path() {
    for state in [Truth::Unknown, Truth::Yes] {
        let context = Context {
            disoriented: state,
            ..confirmed(1)
        };
        let advice = evaluate(&Sheet::default(), true, &context).unwrap();
        let power = advice
            .hints
            .iter()
            .find(|hint| hint.key == "power")
            .unwrap();
        assert_eq!(power.status, "needs context");
        assert_eq!(power.target, None);
        assert!(power.explanation.contains("Failure loses d2 HP"));
        assert!(
            power
                .explanation
                .contains("Success activates the power and spends one daily use")
        );
        if state == Truth::Yes {
            assert!(
                power
                    .unknowns
                    .iter()
                    .any(|reason| reason.contains("fail catastrophically"))
            );
        }
    }
    let context = Context {
        power_uses: "0".into(),
        ..confirmed(1)
    };
    let advice = evaluate(&Sheet::default(), true, &context).unwrap();
    assert_eq!(advice.hints[2].status, "blocked standard path");
}

#[test]
fn confirmed_power_blockers_take_precedence_over_disorientation_unknowns() {
    for state in [Truth::Unknown, Truth::Yes] {
        for context in [
            Context {
                action_remaining: Truth::No,
                disoriented: state,
                ..confirmed(1)
            },
            Context {
                power_uses: "0".into(),
                disoriented: state,
                ..confirmed(1)
            },
        ] {
            let advice = evaluate(&Sheet::default(), true, &context).unwrap();
            let power = &advice.hints[2];
            assert_eq!(power.status, "blocked standard path");
            assert!(!power.blockers.is_empty());
            assert!(!power.unknowns.is_empty());
            assert_eq!(power.target, None);
            assert_eq!(power.modifier, None);
        }
    }
}

#[test]
fn actual_armor_and_original_damaged_tier_are_not_silently_mixed() {
    let sheet = Sheet {
        worn_armor: 3,
        protection: 0,
        strength: -3,
        standard_items: 6,
        ..Sheet::default()
    };
    let mut context = confirmed(1);
    let conflicting = evaluate(&sheet, true, &context).unwrap();
    assert!(conflicting.hints.iter().all(|hint| hint.target.is_none()));
    context.blocking_armor = Truth::Yes;
    let advice = evaluate(&sheet, true, &context).unwrap();
    assert_eq!(advice.hints[4].target, Some(16));
    assert_eq!(advice.hints[2].status, "blocked standard path");
    let excessive = Sheet {
        standard_items: 11,
        ..sheet
    };
    assert!(
        evaluate(&excessive, true, &context)
            .unwrap()
            .hints
            .iter()
            .all(|hint| hint.target.is_none())
    );
}

#[tokio::test]
async fn advice_is_read_only_and_stale_drafts_are_preserved_and_escaped() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("advice.sqlite3")).unwrap();
    let created = store.create(character()).await.unwrap();
    let (_, mut sheet, _) = store.mork_borg_sheet(created.id).await.unwrap();
    sheet.max_hp = 5;
    let (current, _) = store.save_mork_borg_sheet(created.id, sheet).await.unwrap();
    let (_, advice) = store
        .mork_advice(created.id, &confirmed(current.input.revision))
        .await
        .unwrap();
    assert_eq!(advice.revision, current.input.revision);
    assert_eq!(
        store.get(created.id).await.unwrap().input.revision,
        current.input.revision
    );
    assert!(store.sources().await.unwrap().is_empty());
    assert!(matches!(
        store
            .mork_advice(created.id, &confirmed(current.input.revision - 1))
            .await,
        Err(AppError::Conflict)
    ));
    let response = app(store.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/characters/{}/context", created.id))
                .header("host", "localhost:3000")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(
                    "revision=1&goal=%3Cscript%3Eunsafe%3C%2Fscript%3E",
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let html = String::from_utf8(
        to_bytes(response.into_body(), 100_000)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(html.contains("name=\"revision\" value=\"1\""));
    assert!(!html.contains("<script>unsafe"));
    assert!(!html.contains("Saved-sheet snapshot"));
    assert_eq!(
        store.get(created.id).await.unwrap().input.revision,
        current.input.revision
    );
}

#[test]
fn death_conclusions_require_confirmed_scope_and_unknown_power_facts_stay_unknown() {
    let dead = Sheet {
        hp: -1,
        ..Sheet::default()
    };
    let unknown = evaluate(&dead, true, &Context::default()).unwrap();
    assert!(
        unknown
            .hints
            .iter()
            .all(|hint| hint.status == "needs context")
    );
    let base = evaluate(&dead, true, &confirmed(1)).unwrap();
    assert!(
        base.hints
            .iter()
            .take(3)
            .all(|hint| hint.status == "blocked standard path" && hint.target.is_none())
    );
    assert!(
        base.hints
            .iter()
            .skip(3)
            .all(|hint| hint.status == "needs context")
    );
    let unknown_armor = Context {
        blocking_armor: Truth::Unknown,
        ..confirmed(1)
    };
    let advice = evaluate(&Sheet::default(), true, &unknown_armor).unwrap();
    assert_eq!(advice.hints[2].target, None);
    assert_eq!(advice.hints[4].target, None);
    for count in ["-1", "bad", "1001"] {
        assert!(
            evaluate(
                &Sheet::default(),
                true,
                &Context {
                    power_uses: count.into(),
                    ..confirmed(1)
                }
            )
            .is_err()
        );
    }
}

#[tokio::test]
async fn wrong_edition_malformed_facts_and_stat_override_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("inputs.sqlite3")).unwrap();
    let mut input = character();
    input.edition = "another edition".into();
    let wrong = store.create(input).await.unwrap();
    assert!(matches!(
        store.mork_advice(wrong.id, &confirmed(1)).await,
        Err(AppError::Invalid(_))
    ));
    let correct = store.create(character()).await.unwrap();
    let router = app(store.clone());
    for body in ["revision=1&base=invalid", "revision=1&strength=6"] {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/characters/{}/context", correct.id))
                    .header("host", "localhost:3000")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }
    assert_eq!(store.get(correct.id).await.unwrap().input.revision, 1);
}

fn character() -> CharacterInput {
    CharacterInput {
        name: "Context fixture".into(),
        system_id: "mork-borg".into(),
        edition: EDITION.into(),
        notes: String::new(),
        revision: 0,
    }
}

#[tokio::test]
async fn sheet_identity_and_values_are_a_coherent_snapshot_during_other_connection_writes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("snapshot.sqlite3");
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("PRAGMA journal_mode=WAL;").unwrap();
    drop(db);
    let reader = Store::open(&path).unwrap();
    let writer = Store::open(&path).unwrap();
    let created = writer.create(character()).await.unwrap();
    let (_, mut sheet, _) = writer.mork_borg_sheet(created.id).await.unwrap();
    sheet.max_hp = 10_000;
    sheet.hp = 2;
    writer
        .save_mork_borg_sheet(created.id, sheet)
        .await
        .unwrap();
    let id = created.id;
    let writing = tokio::spawn(async move {
        for _ in 0..600 {
            let (_, mut sheet, _) = writer.mork_borg_sheet(id).await.unwrap();
            sheet.hp = i32::try_from(sheet.revision + 1).unwrap();
            writer.save_mork_borg_sheet(id, sheet).await.unwrap();
            tokio::task::yield_now().await;
        }
    });
    let mut mismatch = None;
    for _ in 0..1_200 {
        let (character, sheet, saved) = reader.mork_borg_sheet(id).await.unwrap();
        assert!(saved);
        if character.input.revision != i64::from(sheet.hp) {
            mismatch = Some((character.input.revision, sheet.hp));
            break;
        }
        tokio::task::yield_now().await;
    }
    writing.await.unwrap();
    assert_eq!(
        mismatch, None,
        "Mixed identity/stats snapshot under actual concurrent writes"
    );
}
