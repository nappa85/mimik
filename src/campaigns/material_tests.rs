//! Synthetic material acceptance; no user corpus is accessed.
use super::*;
use axum::http::StatusCode;

async fn suggestion_fixture(
    store: &Store,
    kind: &str,
    target: crate::sources::SuggestionTarget,
) -> i64 {
    store
        .insert_source_suggestion(crate::sources::SourceSuggestion {
            source: 1,
            bundle_hash: "a".repeat(64),
            origin: "tool:fixture".into(),
            kind: kind.into(),
            target,
            suggested_text: (kind != "reject").then(|| "stored suggestion".into()),
            rationale: "Compare synthetic original".into(),
            report_id: None,
        })
        .await
        .unwrap()
}
fn suggestion_unit(unit: i64) -> crate::sources::SuggestionTarget {
    crate::sources::SuggestionTarget::Unit {
        target: crate::sources::SuggestionUnit { unit, revision: 1 },
    }
}

#[tokio::test]
async fn material_review_unreviewed_comparison_uses_extraction() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    finalize_material(&store).await;
    let id = suggestion_fixture(&store, "correction", suggestion_unit(1)).await;
    let advice = store.material_advice(auth[0].clone(), id).await.unwrap();
    assert_eq!(advice.lookup, "Fixture\n");
}

#[tokio::test]
async fn material_review_truncated_lookup_forces_labelled_preview() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    store
        .run(|db| {
            replace_unit(db, "unit-1", &"x".repeat(21000))?;
            Ok(())
        })
        .await
        .unwrap();
    finalize_material(&store).await;
    store
        .material_review_unit(
            auth[0].clone(),
            auth[0].csrf_fixture(),
            1,
            1,
            crate::sources::UnitReview {
                revision: 1,
                state: "accepted".into(),
                correction: String::new(),
                actor: String::new(),
                rationale: "Checked".into(),
            },
        )
        .await
        .unwrap();
    let id = store
        .insert_source_suggestion(crate::sources::SourceSuggestion {
            source: 1,
            bundle_hash: "a".repeat(64),
            origin: "tool:fixture".into(),
            kind: "correction".into(),
            target: crate::sources::SuggestionTarget::Unit {
                target: crate::sources::SuggestionUnit {
                    unit: 1,
                    revision: 2,
                },
            },
            suggested_text: Some("x".repeat(20000)),
            rationale: "Prefix only".into(),
            report_id: None,
        })
        .await
        .unwrap();
    let advice = store.material_advice(auth[0].clone(), id).await.unwrap();
    assert!(advice.lookup_truncated);
    assert!(
        advice.difference.fallback,
        "a truncated prefix is not a complete difference"
    );
    let app = crate::auth::app(store.clone()).await.unwrap();
    let html = body(router_get(&app, &auth[0], "/admin/materials/pages/1").await).await;
    assert!(html.contains("Comparison text truncated"));
}

#[tokio::test]
async fn material_review_single_unit_has_no_batch_operation() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    store
        .material_review_unit(
            auth[0].clone(),
            auth[0].csrf_fixture(),
            1,
            1,
            crate::sources::UnitReview {
                revision: 1,
                state: "accepted".into(),
                correction: String::new(),
                actor: String::new(),
                rationale: "Checked".into(),
            },
        )
        .await
        .unwrap();
    store
        .run(|db| {
            let operation: Option<String> = db.query_row(
                "SELECT operation_id FROM source_review_events WHERE unit_id=1",
                [],
                |r| r.get(0),
            )?;
            assert!(operation.is_none(), "single-unit review is not a batch");
            Ok(())
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn material_suggestion_accept_stored_edit_stale_and_dismissal_exclusion() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    finalize_material(&store).await;
    let first = suggestion_fixture(&store, "correction", suggestion_unit(1)).await;
    let competing = suggestion_fixture(&store, "correction", suggestion_unit(1)).await;
    assert!(
        store
            .material_accept(
                auth[1].clone(),
                auth[1].csrf_fixture(),
                first,
                "Checked".into(),
                None
            )
            .await
            .is_err()
    );
    assert!(
        store
            .material_accept(
                auth[0].clone(),
                auth[0].stale_csrf_fixture(),
                first,
                "Checked".into(),
                None
            )
            .await
            .is_err()
    );
    assert!(
        store
            .material_accept(
                auth[0].clone(),
                auth[1].csrf_fixture(),
                first,
                "Cross-session admission must be rejected".into(),
                None,
            )
            .await
            .is_err(),
        "a valid CSRF admission from another session is not Owner authority"
    );
    store
        .material_accept(
            auth[0].clone(),
            auth[0].csrf_fixture(),
            first,
            "Checked".into(),
            None,
        )
        .await
        .unwrap();
    let data = store.material_advice(auth[0].clone(), first).await.unwrap();
    assert_eq!(data.state, "applied");
    assert_eq!(
        store
            .material_advice(auth[0].clone(), competing)
            .await
            .unwrap()
            .state,
        "stale"
    );
    assert!(matches!(
        store
            .material_accept(
                auth[0].clone(),
                auth[0].csrf_fixture(),
                competing,
                "Fresh form revision cannot rescue old advice".into(),
                None
            )
            .await,
        Err(AppError::Conflict)
    ));
    assert!(matches!(
        store
            .material_dismiss(
                auth[0].clone(),
                auth[0].csrf_fixture(),
                first,
                String::new()
            )
            .await,
        Err(AppError::Conflict)
    ));
    store
        .run(|db| {
            assert_eq!(
                db.query_row("SELECT correction FROM source_units WHERE id=1", [], |r| {
                    r.get::<_, String>(0)
                })?,
                "stored suggestion"
            );
            Ok(())
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn material_suggestion_edits_dismissal_and_note() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    finalize_material(&store).await;
    let edited = suggestion_fixture(&store, "correction", suggestion_unit(2)).await;
    store
        .material_accept(
            auth[0].clone(),
            auth[0].csrf_fixture(),
            edited,
            "Checked edit".into(),
            Some("owner edit".into()),
        )
        .await
        .unwrap();
    let dismissed = suggestion_fixture(&store, "correction", suggestion_unit(3)).await;
    let before = store.source_page(1).await.unwrap();
    store
        .material_dismiss(
            auth[0].clone(),
            auth[0].csrf_fixture(),
            dismissed,
            "Advice unsupported".into(),
        )
        .await
        .unwrap();
    assert!(matches!(
        store
            .material_dismiss(
                auth[0].clone(),
                auth[0].csrf_fixture(),
                dismissed,
                String::new()
            )
            .await,
        Err(AppError::Conflict)
    ));
    assert!(matches!(
        store
            .material_accept(
                auth[0].clone(),
                auth[0].csrf_fixture(),
                dismissed,
                "Checked".into(),
                None
            )
            .await,
        Err(AppError::Conflict)
    ));
    let after = store.source_page(1).await.unwrap();
    assert_eq!(before.revision, after.revision);
    assert_eq!(before.events.len(), after.events.len());
    assert_eq!(before.complete, after.complete);
    let note = suggestion_fixture(&store, "note", suggestion_unit(4)).await;
    assert!(matches!(
        store
            .material_accept(
                auth[0].clone(),
                auth[0].csrf_fixture(),
                note,
                "Checked".into(),
                None
            )
            .await,
        Err(AppError::Invalid(_))
    ));
    store
        .run(move |db| {
            assert!(db.query_row(
                "SELECT edited FROM source_suggestion_applications WHERE suggestion_id=?1",
                [edited],
                |r| r.get::<_, bool>(0)
            )?);
            assert_eq!(
                db.query_row("SELECT count(*) FROM source_web_events", [], |r| r
                    .get::<_, i64>(0))?,
                1
            );
            assert_eq!(
                db.query_row(
                    "SELECT count(*) FROM source_suggestion_web_events",
                    [],
                    |r| r.get::<_, i64>(0)
                )?,
                1
            );
            Ok(())
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn material_merge_long_decimal_rectangle_and_stale_second_target() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    // Replace unreviewed fixture rows before finalization; artifacts are never mutated.
    store.run(|db| {
        db.execute("DELETE FROM source_units WHERE id IN (1,2)",[])?;
        for id in 1..=2 {db.execute("INSERT INTO source_units(id,page_id,block_id,kind,polygon,raw_html,raw_text) VALUES(?1,1,?2,'Text','[[1.2345678901234567,2.3456789012345678],[100.987654321098765,2.3456789012345678],[100.987654321098765,40.876543210987654],[1.2345678901234567,40.876543210987654]]','<p>Fixture</p>','Fixture')",params![id,format!("unit-{id}")])?;}
        crate::sources::search::ensure(db)?;Ok(())
    }).await.unwrap();
    finalize_material(&store).await;
    let target = crate::sources::SuggestionTarget::Units {
        targets: (1..=2)
            .map(|unit| crate::sources::SuggestionUnit { unit, revision: 1 })
            .collect(),
    };
    let merge = suggestion_fixture(&store, "merge", target.clone()).await;
    store
        .material_accept(
            auth[0].clone(),
            auth[0].csrf_fixture(),
            merge,
            "Read combined original region".into(),
            None,
        )
        .await
        .unwrap();
    store
        .run(move |db| {
            assert_eq!(
                db.query_row(
                    "SELECT count(*) FROM source_units WHERE origin='owner' AND state='corrected'",
                    [],
                    |r| r.get::<_, i64>(0)
                )?,
                1
            );
            assert_eq!(
                db.query_row("SELECT count(*) FROM source_web_events", [], |r| r
                    .get::<_, i64>(0))?,
                3
            );
            assert_eq!(
                db.query_row(
                    "SELECT count(DISTINCT operation_id) FROM source_review_events",
                    [],
                    |r| r.get::<_, i64>(0)
                )?,
                1
            );
            Ok(())
        })
        .await
        .unwrap();
    let target = crate::sources::SuggestionTarget::Units {
        targets: vec![
            crate::sources::SuggestionUnit {
                unit: 3,
                revision: 1,
            },
            crate::sources::SuggestionUnit {
                unit: 2,
                revision: 1,
            },
        ],
    };
    let stale = suggestion_fixture(&store, "merge", target).await;
    let before = store.source_page(1).await.unwrap();
    assert!(matches!(
        store
            .material_accept(
                auth[0].clone(),
                auth[0].csrf_fixture(),
                stale,
                "Checked".into(),
                None
            )
            .await,
        Err(AppError::Conflict)
    ));
    let after = store.source_page(1).await.unwrap();
    assert_eq!(before.units.len(), after.units.len());
    assert_eq!(before.revision, after.revision);
    assert_eq!(before.events.len(), after.events.len());
}

#[tokio::test]
async fn material_suggestion_accept_dismiss_serialized_store_calls_have_one_winner() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    finalize_material(&store).await;
    let id = suggestion_fixture(&store, "correction", suggestion_unit(1)).await;
    let (accept, dismiss) = tokio::join!(
        store.material_accept(
            auth[0].clone(),
            auth[0].csrf_fixture(),
            id,
            "Checked".into(),
            None
        ),
        store.material_dismiss(auth[0].clone(), auth[0].csrf_fixture(), id, String::new())
    );
    assert_ne!(accept.is_ok(), dismiss.is_ok());
    store.run(move|db| { assert_eq!(db.query_row("SELECT (SELECT count(*) FROM source_suggestion_applications WHERE suggestion_id=?1)+(SELECT count(*) FROM source_suggestion_dismissals WHERE suggestion_id=?1)",[id],|r|r.get::<_,i64>(0))?,1);Ok(()) }).await.unwrap();
}

#[tokio::test]
async fn material_review_response_bounds_and_labels() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    finalize_material(&store).await;
    for unit in 1..=50 {
        store
            .material_review_unit(
                auth[0].clone(),
                auth[0].csrf_fixture(),
                1,
                unit,
                crate::sources::UnitReview {
                    revision: 1,
                    state: "corrected".into(),
                    correction: "🦀".repeat(20000),
                    actor: String::new(),
                    rationale: "Checked original".into(),
                },
            )
            .await
            .unwrap();
    }
    for unit in 1..=50 {
        for _ in 0..21 {
            store
                .insert_source_suggestion(crate::sources::SourceSuggestion {
                    source: 1,
                    bundle_hash: "a".repeat(64),
                    origin: "tool:fixture".into(),
                    kind: "correction".into(),
                    target: crate::sources::SuggestionTarget::Unit {
                        target: crate::sources::SuggestionUnit { unit, revision: 2 },
                    },
                    suggested_text: Some("界".repeat(20000)),
                    rationale: "Compare original".into(),
                    report_id: None,
                })
                .await
                .unwrap();
        }
    }
    let app = crate::auth::app(store.clone()).await.unwrap();
    let response = router_get(&app, &auth[0], "/admin/materials/pages/1").await;
    assert_eq!(response.status(), StatusCode::OK);
    let html = body(response).await;
    let parsed = scraper::Html::parse_document(&html);
    let differences = scraper::Selector::parse("[data-difference]").unwrap();
    assert!(parsed.select(&differences).count() <= 100);
    let corrections =
        scraper::Selector::parse("[data-form=unit] textarea[name=correction]").unwrap();
    let form_bytes: usize = parsed
        .select(&corrections)
        .map(|field| field.text().collect::<String>().len())
        .sum();
    assert!(
        form_bytes <= 2 * 1024 * 1024,
        "correction forms must also respect the text budget: {form_bytes}"
    );
    assert!(
        html.contains("Lookup text preview"),
        "budget truncation must be labelled"
    );
    assert!(html.contains("/admin/materials/suggestions/"));
    let screen = store
        .material_screen(auth[0].clone(), 1, (1..=50).collect(), 0)
        .await
        .unwrap();
    let mut bytes = 0;
    for card in screen.cards {
        if card.preview {
            assert!(
                card.lookup.chars().count() <= 2000,
                "lookup previews must stop at 2,000 characters"
            );
        }
        bytes += card.lookup.len() * 2;
        if !card.form_omitted {
            bytes += 80000;
        }
        assert_eq!(card.advice.len(), 20);
        assert!(card.next.is_some());
        for advice in card.advice {
            bytes += advice.difference.old.len() + advice.difference.new.len();
            bytes += advice
                .difference
                .segments
                .iter()
                .map(|s| s.text.len())
                .sum::<usize>();
        }
    }
    assert!(bytes <= 2 * 1024 * 1024, "{bytes}");
}

#[tokio::test]
async fn material_review_worst_case_merge_response() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    store.run(|db| {
        for id in 52..=1024 {
            db.execute("INSERT INTO source_units(id,page_id,block_id,kind,polygon,raw_html,raw_text) VALUES(?1,1,?2,'Text','[[1,1],[20,1],[20,20],[1,20]]','<p>Fixture</p>','Fixture')", params![id,format!("unit-{id}")])?;
        }
        crate::sources::search::ensure(db)?;
        Ok(())
    }).await.unwrap();
    finalize_material(&store).await;
    for _ in 0..20 {
        store
            .insert_source_suggestion(crate::sources::SourceSuggestion {
                source: 1,
                bundle_hash: "a".repeat(64),
                origin: "tool:fixture".into(),
                kind: "merge".into(),
                target: crate::sources::SuggestionTarget::Units {
                    targets: (1..=1024)
                        .map(|unit| crate::sources::SuggestionUnit { unit, revision: 1 })
                        .collect(),
                },
                suggested_text: Some("🦀".repeat(20000)),
                rationale: "🦀".repeat(5000),
                report_id: None,
            })
            .await
            .unwrap();
    }
    let app = crate::auth::app(store.clone()).await.unwrap();
    let start = std::time::Instant::now();
    let response = router_get(&app, &auth[0], "/admin/materials/pages/1").await;
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "worst case must complete inside the request window"
    );
    let html = body(response).await;
    let elapsed = start.elapsed();
    eprintln!(
        "worst-case merge: {:?}, {} rendered bytes",
        elapsed,
        html.len()
    );
    assert!(elapsed < std::time::Duration::from_secs(5));
    assert!(
        html.len() <= 2 * 1024 * 1024,
        "{} rendered bytes",
        html.len()
    );
    tokio::time::timeout(
        std::time::Duration::from_millis(250),
        store.material_recheck(auth[0].clone()),
    )
    .await
    .expect("store must be available immediately after rendering")
    .unwrap();
    check_large_merge_detail(app, store, auth).await;
}

async fn check_large_merge_detail(app: axum::Router, store: Store, auth: Vec<RequestAuth>) {
    let detail = body(router_get(&app, &auth[0], "/admin/materials/suggestions/1").await).await;
    eprintln!("1,024-target detail GET: {} bytes", detail.len());
    assert!(detail.len() <= 256 * 1024);
    let draft = "🦀".repeat(20000);
    let response = document_at(
        &app,
        &auth[0],
        "/admin/materials/suggestions/1/accept",
        review_form(&[("edit", "yes"), ("edited_text", &draft), ("rationale", "")]),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let detail = body(response).await;
    eprintln!("1,024-target detail 422: {} bytes", detail.len());
    assert!(detail.len() <= 256 * 1024);
    assert_eq!(
        field_value(&detail, "[data-form=accept] textarea[name=edited_text]"),
        draft
    );
    store
        .material_review_unit(
            auth[0].clone(),
            auth[0].csrf_fixture(),
            1,
            1,
            crate::sources::UnitReview {
                revision: 1,
                state: "accepted".into(),
                correction: String::new(),
                actor: String::new(),
                rationale: "Checked".into(),
            },
        )
        .await
        .unwrap();
    let response = document_at(
        &app,
        &auth[0],
        "/admin/materials/suggestions/1/accept",
        review_form(&[
            ("edit", "yes"),
            ("edited_text", &draft),
            ("rationale", "Checked"),
        ]),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let detail = body(response).await;
    eprintln!("1,024-target detail 409: {} bytes", detail.len());
    assert!(detail.len() <= 256 * 1024);
    assert_eq!(
        field_value(&detail, "[data-form=accept] textarea[name=edited_text]"),
        draft
    );
}

#[tokio::test]
async fn material_suggestion_detail_rendered_bound_and_intact_oversized_draft() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    finalize_material(&store).await;
    store
        .material_review_unit(
            auth[0].clone(),
            auth[0].csrf_fixture(),
            1,
            1,
            crate::sources::UnitReview {
                revision: 1,
                state: "corrected".into(),
                correction: "🦀".repeat(20000),
                actor: String::new(),
                rationale: "Checked".into(),
            },
        )
        .await
        .unwrap();
    let id = store
        .insert_source_suggestion(crate::sources::SourceSuggestion {
            source: 1,
            bundle_hash: "a".repeat(64),
            origin: "tool:fixture".into(),
            kind: "correction".into(),
            target: crate::sources::SuggestionTarget::Unit {
                target: crate::sources::SuggestionUnit {
                    unit: 1,
                    revision: 2,
                },
            },
            suggested_text: Some("🐙".repeat(20000)),
            rationale: "🦀".repeat(5000),
            report_id: None,
        })
        .await
        .unwrap();
    let app = crate::auth::app(store.clone()).await.unwrap();
    let html = body(
        router_get(
            &app,
            &auth[0],
            &format!("/admin/materials/suggestions/{id}"),
        )
        .await,
    )
    .await;
    eprintln!("detail GET: {} bytes", html.len());
    assert!(html.len() <= 256 * 1024, "{} rendered bytes", html.len());
    for draft in ["🦀".repeat(20000), "\"".repeat(100_000)] {
        let response = document_at(
            &app,
            &auth[0],
            &format!("/admin/materials/suggestions/{id}/accept"),
            review_form(&[("edit", "yes"), ("edited_text", &draft), ("rationale", "")]),
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let html = body(response).await;
        let bound = if draft.len() > 80000 {
            2 * 1024 * 1024
        } else {
            256 * 1024
        };
        eprintln!(
            "detail 422: {} bytes (draft {} bytes)",
            html.len(),
            draft.len()
        );
        assert!(html.len() <= bound);
        assert_eq!(
            field_value(&html, "[data-form=accept] textarea[name=edited_text]"),
            draft
        );
    }
}

#[tokio::test]
async fn material_merge_cards_have_unique_anchors() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    finalize_material(&store).await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    let merge = suggestion_fixture(
        &store,
        "merge",
        crate::sources::SuggestionTarget::Units {
            targets: vec![
                crate::sources::SuggestionUnit {
                    unit: 50,
                    revision: 1,
                },
                crate::sources::SuggestionUnit {
                    unit: 51,
                    revision: 1,
                },
            ],
        },
    )
    .await;
    let html = body(router_get(&app, &auth[0], "/admin/materials/pages/1?units=49").await).await;
    let parsed = scraper::Html::parse_document(&html);
    let selector = scraper::Selector::parse(&format!("[data-suggestion='{merge}']")).unwrap();
    let anchors: Vec<_> = parsed
        .select(&selector)
        .map(|e| e.value().id().unwrap().to_owned())
        .collect();
    assert_eq!(anchors.len(), 2);
    assert_ne!(anchors[0], anchors[1], "merge cards need distinct DOM ids");
}

#[tokio::test]
async fn material_suggestion_detail_labels_truncated_lookup_and_preserves_newlines() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    finalize_material(&store).await;
    store
        .material_review_unit(
            auth[0].clone(),
            auth[0].csrf_fixture(),
            1,
            1,
            crate::sources::UnitReview {
                revision: 1,
                state: "corrected".into(),
                correction: "x".repeat(20000),
                actor: String::new(),
                rationale: "Checked".into(),
            },
        )
        .await
        .unwrap();
    store
        .material_review_unit(
            auth[0].clone(),
            auth[0].csrf_fixture(),
            1,
            2,
            crate::sources::UnitReview {
                revision: 1,
                state: "corrected".into(),
                correction: "y".repeat(20000),
                actor: String::new(),
                rationale: "Checked".into(),
            },
        )
        .await
        .unwrap();
    let merge = suggestion_fixture(
        &store,
        "merge",
        crate::sources::SuggestionTarget::Units {
            targets: (1..=2)
                .map(|unit| crate::sources::SuggestionUnit { unit, revision: 2 })
                .collect(),
        },
    )
    .await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    let html = body(
        router_get(
            &app,
            &auth[0],
            &format!("/admin/materials/suggestions/{merge}"),
        )
        .await,
    )
    .await;
    assert!(html.contains("Lookup text is truncated"));
    let correction = suggestion_fixture(&store, "correction", suggestion_unit(3)).await;
    let response = document_at(
        &app,
        &auth[0],
        &format!("/admin/materials/suggestions/{correction}/accept"),
        vec![
            ("csrf".into(), "a".repeat(64)),
            ("rationale".into(), "\nkept rationale".into()),
            ("edit".into(), "yes".into()),
            ("edited_text".into(), "\n".into()),
        ],
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let html = body(response).await;
    let parsed = scraper::Html::parse_document(&html);
    for (name, expected) in [("rationale", "\nkept rationale"), ("edited_text", "\n")] {
        let selector =
            scraper::Selector::parse(&format!("#accept-{correction} textarea[name={name}]"))
                .unwrap();
        assert_eq!(
            parsed
                .select(&selector)
                .next()
                .unwrap()
                .text()
                .collect::<String>(),
            expected
        );
    }
}

fn review_form(fields: &[(&str, &str)]) -> Vec<(String, String)> {
    std::iter::once(("csrf".into(), "a".repeat(64)))
        .chain(
            fields
                .iter()
                .map(|(key, value)| ((*key).into(), (*value).into())),
        )
        .collect()
}

fn field_value(html: &str, selector: &str) -> String {
    scraper::Html::parse_document(html)
        .select(&scraper::Selector::parse(selector).unwrap())
        .next()
        .expect("draft field")
        .text()
        .collect()
}

#[tokio::test]
async fn material_owner_and_primary_drafts_on_conflict_and_validation() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    finalize_material(&store).await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    for (revision, polygon, status) in [
        ("0", "[[1,1],[20,1],[20,20],[1,20]]", StatusCode::CONFLICT),
        ("1", "not a polygon", StatusCode::UNPROCESSABLE_ENTITY),
    ] {
        let response = document_at(
            &app,
            &auth[0],
            "/admin/materials/pages/1/owner-units",
            review_form(&[
                ("revision", revision),
                ("polygon", polygon),
                ("text", "\n<owner draft>"),
                ("rationale", "\nchecked original"),
            ]),
        )
        .await;
        assert_eq!(response.status(), status);
        let html = body(response).await;
        assert_eq!(
            field_value(&html, "[data-form=owner] textarea[name=text]"),
            "\n<owner draft>"
        );
        assert_eq!(
            field_value(&html, "[data-form=owner] textarea[name=rationale]"),
            "\nchecked original"
        );
        assert!(
            html.contains("?units=0#add-text"),
            "link to the matching draft panel"
        );
    }
    for (event, rationale, status) in [
        ("0", "\nprimary draft", StatusCode::CONFLICT),
        ("1", "\n", StatusCode::UNPROCESSABLE_ENTITY),
    ] {
        let response = document_at(
            &app,
            &auth[0],
            "/admin/materials/pages/1/primary",
            review_form(&[
                ("source", "1"),
                ("previous", "1"),
                ("event", event),
                ("rationale", rationale),
            ]),
        )
        .await;
        assert_eq!(response.status(), status);
        let html = body(response).await;
        assert_eq!(
            field_value(&html, "[data-form=primary] textarea"),
            rationale
        );
        assert!(html.contains("?units=0#primary"));
    }
}

#[tokio::test]
async fn material_primary_selection_list_drafts_keep_tokens_and_lf() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    finalize_material(&store).await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    for (token, rationale, expected, status) in [
        ("0:0", "kept", "kept", StatusCode::CONFLICT),
        (
            "1:0",
            "\nkept\r\nprimary",
            "\nkept\nprimary",
            StatusCode::CONFLICT,
        ),
        ("1:1", "\n", "\n", StatusCode::UNPROCESSABLE_ENTITY),
    ] {
        let response = document_at(
            &app,
            &auth[0],
            "/admin/materials/sources/1/primary",
            review_form(&[("after", "0"), ("page_1", token), ("rationale", rationale)]),
        )
        .await;
        assert_eq!(response.status(), status);
        let html = body(response).await;
        assert_eq!(field_value(&html, "#primary-pages textarea"), expected);
        assert!(html.contains(&format!("value=\"{token}\" checked")));
        assert!(
            html.contains("this reading is primary"),
            "submitted tokens must not replace current state"
        );
    }
}

#[tokio::test]
async fn material_card_links_preserve_unsaved_text() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    finalize_material(&store).await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    let html = body(router_get(&app, &auth[0], "/admin/materials/pages/1").await).await;
    assert!(html.contains("href=\"#unit-1\">Next unreviewed unit"));
}

#[tokio::test]
async fn material_progressive_controls_expose_keyboard_drawing_and_zoom_state() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    finalize_material(&store).await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    let html = body(router_get(&app, &auth[0], "/admin/materials/pages/1").await).await;
    let parsed = scraper::Html::parse_document(&html);
    assert!(
        parsed
            .select(&scraper::Selector::parse("label[hidden] > #review-shortcuts").unwrap())
            .next()
            .is_some(),
        "shortcut switch is unavailable until enhancement runs"
    );
    for button in parsed.select(&scraper::Selector::parse("[data-zoom]").unwrap()) {
        assert!(button.value().attr("aria-pressed").is_some());
    }
    assert!(html.contains("id=\"keyboard-region\""));
}

#[tokio::test]
async fn material_committed_suggestion_does_not_reload_detail() {
    let _lock = crate::auth::test_resource_lock().await;
    let (dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    finalize_material(&store).await;
    let id = suggestion_fixture(&store, "correction", suggestion_unit(1)).await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    let path = dir.path().join("private.sqlite3");
    *crate::sources::materials::AFTER_SUGGESTION_COMMIT
        .lock()
        .unwrap() = Some(Box::new(move || {
        rusqlite::Connection::open(&path)
            .unwrap()
            .execute("DROP TABLE source_search_text", [])
            .unwrap();
    }));
    let response = document_at(
        &app,
        &auth[0],
        &format!("/admin/materials/suggestions/{id}/accept"),
        review_form(&[("rationale", "Checked")]),
    )
    .await;
    assert_eq!(
        response.status(),
        StatusCode::SEE_OTHER,
        "a committed decision cannot become a detail-read error"
    );
}

#[tokio::test]
async fn material_primary_write_does_not_load_unit_editor() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    store
        .run(|db| {
            replace_unit(db, "unit-1", &"x".repeat(65537))?;
            Ok(())
        })
        .await
        .unwrap();
    finalize_material(&store).await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    let response = document_at(
        &app,
        &auth[0],
        "/admin/materials/pages/1/primary",
        review_form(&[
            ("source", "1"),
            ("previous", "1"),
            ("event", "1"),
            ("rationale", "Checked"),
        ]),
    )
    .await;
    assert_eq!(
        response.status(),
        StatusCode::SEE_OTHER,
        "primary selection does not require rendering unit content"
    );
}

#[tokio::test]
async fn material_missing_geometry_omits_overlay_and_drawing() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    let html = body(router_get(&app, &auth[0], "/admin/materials/pages/1").await).await;
    assert!(!html.contains("id=\"review-overlay\""));
    assert!(!html.contains("id=\"draw-region\""));
    assert!(html.contains("Page geometry unavailable"));
}

#[tokio::test]
async fn material_region_overlay_includes_notes_beyond_current_notes_page() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    finalize_material(&store).await;
    let mut last = 0;
    for _ in 0..21 {
        last = suggestion_fixture(
            &store,
            "missing_text",
            crate::sources::SuggestionTarget::Region {
                page: 1,
                polygon: [[1.0, 1.0], [10.0, 1.0], [10.0, 10.0], [1.0, 10.0]],
            },
        )
        .await;
    }
    let app = crate::auth::app(store.clone()).await.unwrap();
    let html = body(router_get(&app, &auth[0], "/admin/materials/pages/1").await).await;
    assert!(
        html.contains(&format!("data-overlay-suggestion=\"{last}\"")),
        "region geometry is independent of the 20-row notes cursor"
    );
}

#[tokio::test]
async fn material_suggestion_handler_drafts_authority_and_stored_text() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    finalize_material(&store).await;
    let id = suggestion_fixture(&store, "correction", suggestion_unit(51)).await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    let path = format!("/admin/materials/suggestions/{id}/accept");
    assert_suggestion_authority(&app, &auth, &path).await;
    let response = document_at(
        &app,
        &auth[0],
        &path,
        review_form(&[("rationale", "Checked"), ("suggested_text", "forged")]),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let response = document_at(
        &app,
        &auth[0],
        &path,
        review_form(&[
            ("rationale", "\nkept acceptance"),
            ("edit", "yes"),
            ("edited_text", ""),
        ]),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let html = body(response).await;
    assert_eq!(
        field_value(&html, "[data-form=accept] textarea[name=rationale]"),
        "\nkept acceptance"
    );
    assert!(html.contains("?units=50#unit-51"));
    assert_missing_edit_choice_keeps_draft(&app, &auth[0], &path).await;
    let response = document_at(
        &app,
        &auth[0],
        &path,
        review_form(&[
            ("rationale", "Checked"),
            ("edit", "yes"),
            ("edited_text", "Owner edit must be retained"),
        ]),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert!(
        response.headers()["location"]
            .to_str()
            .unwrap()
            .contains("?units=50#unit-51")
    );
    let response = document_at(
        &app,
        &auth[0],
        &path,
        review_form(&[("rationale", "\nconflict draft")]),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(
        field_value(
            &body(response).await,
            "[data-form=accept] textarea[name=rationale]"
        ),
        "\nconflict draft"
    );
    let dismissal = format!("/admin/materials/suggestions/{id}/dismiss");
    let response = document_at(
        &app,
        &auth[0],
        &dismissal,
        review_form(&[("reason", "\ndismiss conflict")]),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(
        field_value(&body(response).await, "[data-form=dismiss] textarea"),
        "\ndismiss conflict"
    );
    let id = suggestion_fixture(&store, "correction", suggestion_unit(1)).await;
    let response = document_at(
        &app,
        &auth[0],
        &format!("/admin/materials/suggestions/{id}/dismiss"),
        review_form(&[("reason", "\ninvalid\0reason")]),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body(response).await.contains("invalid\0reason"));
    assert_eq!(
        store.source_page(1).await.unwrap().units[50].correction,
        "Owner edit must be retained"
    );
}

async fn assert_suggestion_authority(app: &axum::Router, auth: &[RequestAuth], path: &str) {
    for (who, token) in [(&auth[1], "a".repeat(64)), (&auth[0], "wrong".into())] {
        let response = document_at(
            app,
            who,
            path,
            vec![
                ("csrf".into(), token),
                ("rationale".into(), "Checked".into()),
            ],
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
}

async fn assert_missing_edit_choice_keeps_draft(
    app: &axum::Router,
    auth: &RequestAuth,
    path: &str,
) {
    let response = document_at(
        app,
        auth,
        path,
        review_form(&[
            ("rationale", "Checked"),
            ("edited_text", "Owner edit must be retained"),
        ]),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        field_value(
            &body(response).await,
            "[data-form=accept] textarea[name=edited_text]"
        ),
        "Owner edit must be retained"
    );
}

#[tokio::test]
async fn material_page_notes_missing_text_and_rejection() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    finalize_material(&store).await;
    let missing = suggestion_fixture(
        &store,
        "missing_text",
        crate::sources::SuggestionTarget::Region {
            page: 1,
            polygon: [[1.0, 1.0], [20.0, 1.0], [20.0, 20.0], [1.0, 20.0]],
        },
    )
    .await;
    for _ in 0..21 {
        suggestion_fixture(
            &store,
            "note",
            crate::sources::SuggestionTarget::Page {
                page: 1,
                block_id: Some("raw-container".into()),
            },
        )
        .await;
    }
    let screen = store
        .material_screen(auth[0].clone(), 1, vec![1], 0)
        .await
        .unwrap();
    assert_eq!(screen.notes.len(), 20);
    let next = screen.notes_next.unwrap();
    assert!(
        screen
            .overlay
            .iter()
            .any(|region| region.suggestion == missing)
    );
    let screen = store
        .material_screen(auth[0].clone(), 1, vec![1], next)
        .await
        .unwrap();
    assert_eq!(screen.notes.len(), 2);
    assert!(screen.notes_next.is_none());
    let app = crate::auth::app(store.clone()).await.unwrap();
    let response = document_at(
        &app,
        &auth[0],
        &format!("/admin/materials/suggestions/{missing}/accept"),
        review_form(&[("rationale", "Read omitted text")]),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert!(
        response.headers()["location"]
            .to_str()
            .unwrap()
            .contains("notes=0#page-notes")
    );
    let advice = store
        .material_advice(auth[0].clone(), missing)
        .await
        .unwrap();
    assert_eq!(advice.state, "applied");
    let reject = suggestion_fixture(&store, "reject", suggestion_unit(1)).await;
    store
        .material_accept(
            auth[0].clone(),
            auth[0].csrf_fixture(),
            reject,
            "Not evidence".into(),
            None,
        )
        .await
        .unwrap();
    let page = store
        .material_page(auth[0].clone(), 1, 50, 0)
        .await
        .unwrap();
    assert_eq!(
        page.page.units.last().unwrap().correction,
        "stored suggestion"
    );
    assert_eq!(
        store.source_page(1).await.unwrap().units[0].state,
        "rejected"
    );
    assert_review_counts(&store, 1, 2, 2).await;
}

#[tokio::test]
async fn material_merge_drafts_stale_targets_and_zero_area() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    store
        .run(|db| {
            db.execute(
                "UPDATE source_units SET polygon='[[1,1],[20,1],[20,20],[1,20]]' WHERE id IN (1,2)",
                [],
            )?;
            db.execute(
                "UPDATE source_units SET polygon='[[1,1],[1,1],[1,1],[1,1]]' WHERE id IN (3,4)",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    finalize_material(&store).await;
    let merge_target = |first| crate::sources::SuggestionTarget::Units {
        targets: (first..first + 2)
            .map(|unit| crate::sources::SuggestionUnit { unit, revision: 1 })
            .collect(),
    };
    let merge = suggestion_fixture(&store, "merge", merge_target(1)).await;
    let zero = suggestion_fixture(&store, "merge", merge_target(3)).await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    let response = document_at(
        &app,
        &auth[0],
        &format!("/admin/materials/suggestions/{merge}/accept"),
        review_form(&[
            ("rationale", "\n"),
            ("edit", "yes"),
            ("edited_text", "\nmerge draft"),
        ]),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        field_value(
            &body(response).await,
            "[data-form=accept] textarea[name=edited_text]"
        ),
        "\nmerge draft"
    );
    let response = document_at(
        &app,
        &auth[0],
        &format!("/admin/materials/suggestions/{zero}/accept"),
        review_form(&[("rationale", "Zero area draft")]),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body(response).await.contains("Zero area draft"));
    store
        .material_review_unit(
            auth[0].clone(),
            auth[0].csrf_fixture(),
            1,
            2,
            crate::sources::UnitReview {
                revision: 1,
                state: "accepted".into(),
                correction: String::new(),
                actor: String::new(),
                rationale: "Unrelated Owner decision".into(),
            },
        )
        .await
        .unwrap();
    let response = document_at(
        &app,
        &auth[0],
        &format!("/admin/materials/suggestions/{merge}/accept"),
        review_form(&[
            ("rationale", "Retained merge rationale"),
            ("edit", "yes"),
            ("edited_text", "Retained merge text"),
        ]),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let html = body(response).await;
    assert!(html.contains("Retained merge rationale"));
    assert_eq!(
        field_value(&html, "[data-form=accept] textarea[name=edited_text]"),
        "Retained merge text"
    );
    assert_review_counts(&store, 0, 1, 0).await;
}

async fn assert_review_counts(store: &Store, owners: i64, events: i64, applications: i64) {
    store
        .run(move |db| {
            for (table, expected) in [
                ("source_units WHERE origin='owner'", owners),
                ("source_review_events", events),
                ("source_web_events", events),
                ("source_suggestion_applications", applications),
            ] {
                assert_eq!(
                    db.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r
                        .get::<_, i64>(0))?,
                    expected,
                    "{table}"
                );
            }
            Ok(())
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn material_large_page_omits_unit_overlay_but_keeps_missing_regions_and_drawing() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    store.run(|db| {
        db.execute_batch("WITH RECURSIVE ids(id) AS (VALUES(52) UNION ALL SELECT id+1 FROM ids WHERE id<2001) INSERT INTO source_units(id,page_id,block_id,kind,polygon,raw_html,raw_text) SELECT id,1,'unit-'||id,'Text','[[1,1],[20,1],[20,20],[1,20]]','<p>Fixture</p>','Fixture' FROM ids;")?;
        Ok(())
    }).await.unwrap();
    finalize_material(&store).await;
    let missing = suggestion_fixture(
        &store,
        "missing_text",
        crate::sources::SuggestionTarget::Region {
            page: 1,
            polygon: [[1.0, 1.0], [20.0, 1.0], [20.0, 20.0], [1.0, 20.0]],
        },
    )
    .await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    let response = router_get(&app, &auth[0], "/admin/materials/pages/1").await;
    assert_eq!(response.status(), StatusCode::OK);
    let html = body(response).await;
    let parsed = scraper::Html::parse_document(&html);
    assert!(html.contains("Unit overlay omitted"));
    assert_eq!(
        parsed
            .select(&scraper::Selector::parse("[data-overlay-unit]").unwrap())
            .count(),
        0
    );
    assert!(html.contains(&format!("data-overlay-suggestion=\"{missing}\"")));
    assert!(html.contains("id=\"drawn-region\""));
}

async fn seed(store: &Store) {
    store.run(|db|{
        db.execute("INSERT INTO sources(id,bundle_hash,source_id,title,system_id,edition,language,pdf_hash,pdf,raw_json,prepared_json,total_pages) VALUES(1,'fixture','fixture','Material <title>','mork-borg','Fixture edition','en',?1,x'',x'',x'',2)",["a".repeat(64)])?;
        db.execute("INSERT INTO source_pages(id,source_id,pdf_page,png) VALUES(1,1,1,x'89504e47')",[])?;
        for id in 1..=51 {db.execute("INSERT INTO source_units(id,page_id,block_id,kind,polygon,raw_html,raw_text) VALUES(?1,1,?2,'Text','[]','<p>Fixture</p><script>bad()</script>','Fixture')",params![id,format!("unit-{id}")])?;}
        // Raw fixture rows get derived search text exactly as a legacy upgrade would.
        crate::sources::search::ensure(db)?;
        Ok(())
    }).await.unwrap();
}

async fn finalize_material(store: &Store) {
    store.run(|db| {
        let prepared=serde_json::json!({"contract_version":1,"extraction_sha256":"b".repeat(64),"pages":[{"pdf_page":1,"width_points":200,"height_points":300}]});
        db.execute("UPDATE sources SET bundle_hash=?1,prepared_json=?2 WHERE id=1",params!["a".repeat(64),serde_json::to_vec(&prepared)?])?;
        db.execute("INSERT INTO source_page_hashes VALUES(1,1,?1)",["c".repeat(64)])?;
        db.execute("INSERT INTO source_finalizations VALUES(1,?1,?2)",params!["a".repeat(64),"b".repeat(64)])?;
        crate::sources::documents::register_import(db,1)?;
        Ok(())
    }).await.unwrap();
}

#[tokio::test]
async fn material_owner_insert_and_primary_selection_have_authenticated_sidecars() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    finalize_material(&store).await;
    let unit = crate::sources::OwnerUnit {
        revision: 1,
        polygon: [[1.0, 1.0], [20.0, 1.0], [20.0, 20.0], [1.0, 20.0]],
        text: "Owner transcription".into(),
        actor: "forged".into(),
        rationale: "Read original".into(),
    };
    let event = store
        .material_add_owner_unit(auth[0].clone(), auth[0].csrf_fixture(), 1, unit.clone())
        .await
        .unwrap();
    let primary = store
        .material_set_primary_source(
            auth[0].clone(),
            auth[0].csrf_fixture(),
            1,
            store.source_primary_state(1).await.unwrap(),
            1,
            "Confirmed reading".into(),
        )
        .await
        .unwrap()[0];
    store.run(move |db| {
        assert_eq!(db.query_row("SELECT e.actor,w.actor_id FROM source_review_events e JOIN source_web_events w ON w.event_id=e.id WHERE e.id=?1",[event],|r|Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?)))?,("account:1".into(),1));
        assert_eq!(db.query_row("SELECT e.actor,w.actor_id FROM source_primary_events e JOIN source_primary_web_events w ON w.event_id=e.id WHERE e.id=?1",[primary],|r|Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?)))?,("account:1".into(),1));
        db.execute_batch("CREATE TRIGGER owner_audit_fault BEFORE INSERT ON source_web_events BEGIN SELECT RAISE(ABORT,'fixture fault'); END;")?;
        Ok(())
    }).await.unwrap();
    let mut unit = unit;
    unit.revision = 3;
    assert!(
        store
            .material_add_owner_unit(auth[0].clone(), auth[0].csrf_fixture(), 1, unit)
            .await
            .is_err()
    );
    assert_eq!(store.source_page(1).await.unwrap().units.len(), 52);
    assert_eq!(store.source_page(1).await.unwrap().revision, 3);
}

#[tokio::test]
async fn material_atomic_batch_attributes_in_order_and_sidecar_failure_rolls_back_all() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    let decisions = |revision| {
        (1..=2)
            .map(|unit| crate::sources::UnitDecision {
                page: 1,
                unit,
                review: crate::sources::UnitReview {
                    revision,
                    state: "accepted".into(),
                    correction: String::new(),
                    actor: "forged".into(),
                    rationale: String::new(),
                },
            })
            .collect()
    };
    store
        .material_review_units_atomic(
            auth[0].clone(),
            auth[0].csrf_fixture(),
            decisions(1),
            "Checked both units".into(),
        )
        .await
        .unwrap();
    store.run(|db| {
        assert_eq!(db.query_row("SELECT count(*) FROM source_web_events w JOIN source_review_events e ON e.id=w.event_id WHERE e.operation_id LIKE 'batch-%' AND e.actor='account:1'",[],|r|r.get::<_,i64>(0))?,2);
        assert_eq!(db.query_row("SELECT count(DISTINCT operation_id) FROM source_review_events",[],|r|r.get::<_,i64>(0))?,1);
        db.execute_batch("CREATE TRIGGER second_batch_sidecar_fault BEFORE INSERT ON source_web_events WHEN (SELECT unit_id FROM source_review_events WHERE id=NEW.event_id)=2 BEGIN SELECT RAISE(ABORT,'fixture fault'); END;")?;
        Ok(())
    }).await.unwrap();
    assert!(
        store
            .material_review_units_atomic(
                auth[0].clone(),
                auth[0].csrf_fixture(),
                decisions(2),
                "Recheck both".into()
            )
            .await
            .is_err()
    );
    store
        .run(|db| {
            assert_eq!(
                db.query_row("SELECT count(*) FROM source_review_events", [], |r| r
                    .get::<_, i64>(0))?,
                2
            );
            assert_eq!(
                db.query_row("SELECT count(*) FROM source_web_events", [], |r| r
                    .get::<_, i64>(0))?,
                2
            );
            assert_eq!(
                db.query_row("SELECT revision FROM source_pages WHERE id=1", [], |r| r
                    .get::<_, i64>(0))?,
                3
            );
            assert_eq!(
                db.query_row(
                    "SELECT sum(revision) FROM source_units WHERE id IN (1,2)",
                    [],
                    |r| r.get::<_, i64>(0)
                )?,
                4
            );
            Ok(())
        })
        .await
        .unwrap();
}
/// Raw HTML is immutable (migration trigger); fixtures replace the unreviewed unit instead.
fn replace_unit(db: &rusqlite::Connection, block: &str, html: &str) -> Result<(), AppError> {
    db.execute(
        "DELETE FROM source_units WHERE id=1 AND state='unreviewed'",
        [],
    )?;
    db.execute("INSERT INTO source_units(id,page_id,block_id,kind,polygon,raw_html,raw_text) VALUES(1,1,?1,'Text','[]',?2,'Fixture')", params![block, html])?;
    crate::sources::search::ensure(db)?;
    Ok(())
}
fn decision(revision: i64) -> Vec<(String, String)> {
    vec![
        ("csrf".into(), "a".repeat(64)),
        ("revision".into(), revision.to_string()),
        ("state".into(), "corrected".into()),
        ("correction".into(), "<unsaved correction>".into()),
        ("rationale".into(), "<checked original>".into()),
    ]
}
async fn body(response: axum::response::Response) -> String {
    String::from_utf8(
        axum::body::to_bytes(response.into_body(), 16 * 1024 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}

async fn document_post(
    app: &axum::Router,
    auth: &RequestAuth,
    form: Vec<(String, String)>,
) -> axum::response::Response {
    document_at(app, auth, "/admin/materials/pages/1/units/1", form).await
}
async fn document_at(
    app: &axum::Router,
    auth: &RequestAuth,
    path: &str,
    form: Vec<(String, String)>,
) -> axum::response::Response {
    use tower::ServiceExt;
    app.clone()
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri(path)
                .header("host", "127.0.0.1:3000")
                .header("origin", "http://127.0.0.1:3000")
                .header("accept", "text/html")
                .header("sec-fetch-mode", "navigate")
                .header("cookie", auth.fixture_cookie())
                .header("content-type", "application/x-www-form-urlencoded")
                .body(axum::body::Body::from(
                    serde_urlencoded::to_string(form).unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn material_document_conflict_validation_and_nul_retain_escaped_drafts() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    for (revision, state, status) in [
        (0, "corrected", StatusCode::CONFLICT),
        (1, "invalid", StatusCode::UNPROCESSABLE_ENTITY),
    ] {
        let mut form = decision(revision);
        form[2].1 = state.into();
        let response = document_post(&app, &auth[0], form).await;
        assert_eq!(response.status(), status);
        let html = body(response).await;
        assert!(html.contains("Your draft was not saved"), "{html}");
        assert!(html.contains(&format!("Submitted revision {revision}")));
        assert!(!html.contains("<unsaved correction>"));
        assert!(
            html.contains("&#60;unsaved correction&#62;")
                || html.contains("&lt;unsaved correction&gt;")
        );
        assert!(html.contains("checked original"));
        assert_eq!(
            scraper::Html::parse_document(&html)
                .select(
                    &scraper::Selector::parse(
                        "[data-form=unit] select[name=state] option[selected]"
                    )
                    .unwrap()
                )
                .next()
                .expect("submitted decision state")
                .value()
                .attr("value"),
            Some(state)
        );
    }
    let mut form = decision(1);
    form[3].1 = "\0retained NUL correction".into();
    let response = document_post(&app, &auth[0], form).await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let html = body(response).await;
    assert!(html.contains("retained NUL correction"));
    assert!(html.contains("Submitted revision 1"));
    for revision in [0, 1] {
        let form = vec![
            ("csrf".into(), "a".repeat(64)),
            ("revision".into(), revision.to_string()),
            ("printed_label".into(), "<printed draft>".into()),
            ("rationale".into(), "<whole page rationale>".into()),
        ];
        let response = document_at(&app, &auth[0], "/admin/materials/pages/1", form).await;
        assert_eq!(
            response.status(),
            if revision == 0 {
                StatusCode::CONFLICT
            } else {
                StatusCode::UNPROCESSABLE_ENTITY
            }
        );
        let html = body(response).await;
        assert!(html.contains("Your draft was not saved"));
        assert!(html.contains(&format!("Submitted page revision {revision}")));
        assert!(html.contains("whole page rationale"));
        assert!(!html.contains("value=\"<printed draft>\""));
    }
}

#[tokio::test]
async fn material_completion_checks_offscreen_units_and_invalidates_after_review() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    for id in 1..=50 {
        store
            .material_review_unit(
                auth[0].clone(),
                auth[0].csrf_fixture(),
                1,
                id,
                crate::sources::UnitReview {
                    revision: 1,
                    state: "accepted".into(),
                    correction: String::new(),
                    actor: String::new(),
                    rationale: "Compared synthetic original".into(),
                },
            )
            .await
            .unwrap();
    }
    let complete = |revision| crate::sources::PageReview {
        revision,
        printed_label: "1".into(),
        actor: String::new(),
        rationale: "Whole page omissions checked".into(),
    };
    assert!(matches!(
        store
            .material_complete(auth[0].clone(), auth[0].csrf_fixture(), 1, complete(51))
            .await,
        Err(AppError::Invalid(_))
    ));
    store
        .material_review_unit(
            auth[0].clone(),
            auth[0].csrf_fixture(),
            1,
            51,
            crate::sources::UnitReview {
                revision: 1,
                state: "rejected".into(),
                correction: String::new(),
                actor: String::new(),
                rationale: "Compared last unit".into(),
            },
        )
        .await
        .unwrap();
    store
        .material_complete(auth[0].clone(), auth[0].csrf_fixture(), 1, complete(52))
        .await
        .unwrap();
    let page = store
        .material_page(auth[0].clone(), 1, 50, 50)
        .await
        .unwrap();
    assert!(page.page.complete);
    assert_eq!(page.page.revision, 53);
    assert_eq!(page.page.events.len(), 2);
    assert_eq!(page.history_next, None);
    store
        .material_review_unit(
            auth[0].clone(),
            auth[0].csrf_fixture(),
            1,
            1,
            crate::sources::UnitReview {
                revision: 2,
                state: "corrected".into(),
                correction: "Recompared source".into(),
                actor: String::new(),
                rationale: "Changed source decision".into(),
            },
        )
        .await
        .unwrap();
    let page = store.material_page(auth[0].clone(), 1, 0, 0).await.unwrap();
    assert!(!page.page.complete);
    assert_eq!(page.page.revision, 54);
    assert_eq!(page.history_next, Some(50));
}

#[tokio::test]
async fn material_final_admission_failure_rolls_back_unit_event_and_sidecar() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, clock) = fixture().await;
    seed(&store).await;
    store.run(move|db|{db.create_scalar_function("material_test_expire",0,rusqlite::functions::FunctionFlags::SQLITE_UTF8,move |_|{clock.store(1000,Ordering::SeqCst);Ok(1)})?;db.execute_batch("CREATE TRIGGER material_final_expiry AFTER INSERT ON source_web_events BEGIN SELECT material_test_expire(); END;")?;Ok(())}).await.unwrap();
    let result = store
        .material_review_unit(
            auth[0].clone(),
            auth[0].csrf_fixture(),
            1,
            1,
            crate::sources::UnitReview {
                revision: 1,
                state: "accepted".into(),
                correction: String::new(),
                rationale: "Compared".into(),
                actor: String::new(),
            },
        )
        .await;
    assert!(matches!(result, Err(AppError::NotFound)));
    store
        .run(|db| {
            for table in ["source_review_events", "source_web_events"] {
                assert_eq!(
                    db.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r
                        .get::<_, i64>(0))?,
                    0
                );
            }
            assert_eq!(
                db.query_row("SELECT revision FROM source_units WHERE id=1", [], |r| r
                    .get::<_, i64>(0))?,
                1
            );
            assert_eq!(
                db.query_row("SELECT revision FROM source_pages WHERE id=1", [], |r| r
                    .get::<_, i64>(0))?,
                1
            );
            Ok(())
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn material_crop_uses_fresh_authority_after_cpu_work() {
    let _lock = crate::auth::test_resource_lock().await;
    let (dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, 4, 4);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .write_header()
            .unwrap()
            .write_image_data(&[0; 48])
            .unwrap();
    }
    store
        .run(move |db| {
            db.execute("UPDATE source_pages SET png=?1 WHERE id=1", [bytes])?;
            db.execute(
                "UPDATE sources SET prepared_json=?1 WHERE id=1",
                [br#"{"pages":[{"pdf_page":1,"width_points":4,"height_points":4}]}"#.to_vec()],
            )?;
            db.execute(
                "UPDATE source_units SET polygon='[[0,0],[4,0],[4,4],[0,4]]' WHERE id=1",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let app = crate::auth::app(store.clone()).await.unwrap();
    assert_eq!(
        router_request(
            &app,
            &auth[0],
            "/admin/materials/pages/1/units/1/image",
            None
        )
        .await
        .status(),
        StatusCode::OK
    );
    let path = dir.path().join("private.sqlite3");
    *crate::sources::materials::AFTER_CROP.lock().unwrap() = Some(Box::new(move || {
        let db = rusqlite::Connection::open(&path).unwrap();
        crate::store::register_auth_clock(&db, Arc::new(|| Ok(100))).unwrap();
        db.execute(
            "UPDATE auth_sessions SET state='revoked',payload=NULL WHERE account_id=1",
            [],
        )
        .unwrap();
    }));
    let response = router_request(
        &app,
        &auth[0],
        "/admin/materials/pages/1/units/1/image",
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_ne!(
        response
            .headers()
            .get("content-type")
            .map(axum::http::HeaderValue::as_bytes),
        Some(b"image/png".as_slice())
    );
}

#[tokio::test]
async fn material_schema14_upgrade_preserves_source_account_and_private_history() {
    let _lock = crate::auth::test_resource_lock().await;
    let (dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    let campaign = store
        .create_campaign(
            auth[1].clone(),
            "Retained campaign".into(),
            "mork-borg".into(),
            "it-2025".into(),
        )
        .await
        .unwrap();
    let saved = insert_native(&store, auth[1].clone(), native(campaign, 2)).await;
    crate::accounts::Accounts::new(store.clone())
        .browser_invite(
            auth[0].clone(),
            "retained".into(),
            "Retained account".into(),
            0,
        )
        .await
        .unwrap();
    store
        .review_source_unit(
            1,
            1,
            crate::sources::UnitReview {
                revision: 1,
                state: "corrected".into(),
                correction: "Retained correction".into(),
                actor: "local reviewer".into(),
                rationale: "Retained rationale".into(),
            },
        )
        .await
        .unwrap();
    store.run(|db|{assert_eq!(db.query_row("SELECT count(*) FROM source_web_events",[],|r|r.get::<_,i64>(0))?,0);crate::sources::search::drop_schema(db)?;db.execute_batch("DROP TABLE registry_web_events; DROP TABLE registry_events; DROP TABLE source_web_events; DROP INDEX source_review_page_history; PRAGMA user_version=14;")?;db.execute_batch(crate::catalogues::DROP_SCHEMA)?;db.execute_batch(crate::campaigns::DROP_NOTES_SCHEMA)?;db.execute_batch(crate::assistant::records::DROP_SCHEMA)?;Ok(())}).await.unwrap();
    let reopened = Store::open(&dir.path().join("private.sqlite3")).unwrap();
    reopened
        .run(|db| {
            crate::store::register_auth_clock(db, Arc::new(|| Ok(100)))?;
            assert_eq!(
                db.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))?,
                crate::store::SCHEMA_VERSION
            );
            assert_eq!(
                db.query_row("SELECT count(*) FROM account_web_events", [], |r| r
                    .get::<_, i64>(0))?,
                1
            );
            assert_eq!(
                db.query_row("SELECT actor FROM source_review_events", [], |r| r
                    .get::<_, String>(0))?,
                "local reviewer"
            );
            Ok(())
        })
        .await
        .unwrap();
    let data = reopened
        .material_page(auth[0].clone(), 1, 0, 0)
        .await
        .unwrap();
    assert_eq!(data.page.units[0].correction, "Retained correction");
    assert_eq!(data.page.units[0].revision, 2);
    let (view, permit) = reopened
        .private_read(
            auth[1].clone(),
            1,
            None,
            crate::definitions::admit().unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_string(&view.snapshot).unwrap(),
        serde_json::to_string(&saved).unwrap()
    );
    drop(permit);
}

#[tokio::test]
async fn material_directory_cursors_are_independent_and_exhaustive() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    store.run(|db|{
        for id in 2..=51{
            db.execute("INSERT INTO sources SELECT ?1,?2,source_id,title,system_id,edition,language,pdf_hash,pdf,raw_json,prepared_json,total_pages FROM sources WHERE id=1",params![id,format!("synthetic-{id}")])?;
            db.execute("INSERT INTO source_pages(id,source_id,pdf_page,png) VALUES(?1,1,?1,x'')",[id])?;
        }Ok(())
    }).await.unwrap();
    let first = store.material_sources(auth[0].clone(), 0).await.unwrap();
    assert_eq!(first.rows.len(), 50);
    assert_eq!(first.next, Some(50));
    assert_eq!(first.rows[0].imported, 51);
    let last = store.material_sources(auth[0].clone(), 50).await.unwrap();
    assert_eq!(last.rows[0].id, 51);
    assert_eq!(last.next, None);
    let first = store.material_pages(auth[0].clone(), 1, 0).await.unwrap();
    assert_eq!(first.rows.len(), 50);
    assert_eq!(first.next, Some(50));
    let last = store.material_pages(auth[0].clone(), 1, 50).await.unwrap();
    assert_eq!(last.rows[0].id, 51);
    assert_eq!(last.next, None);
}

#[tokio::test]
async fn material_maximum_metadata_and_history_byte_guards() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    store
        .run(|db| {
            db.execute(
                "UPDATE sources SET title=?1,edition=?2 WHERE id=1",
                params!["x".repeat(200), "🦀".repeat(100)],
            )?;
            replace_unit(db, &"x".repeat(512), &"x".repeat(65536))?;
            Ok(())
        })
        .await
        .unwrap();
    let data = store.material_page(auth[0].clone(), 1, 0, 0).await.unwrap();
    assert_eq!(data.page.edition.len(), 400);
    assert_eq!(data.page.units[0].block_id.len(), 512);
    assert!(store.material_sources(auth[0].clone(), 0).await.is_ok());
    store.run(|db|{db.execute("UPDATE source_units SET state='accepted',revision=revision+1 WHERE id=1",[])?;db.execute("INSERT INTO source_review_events(page_id,unit_id,actor,rationale,state,correction,previous_revision) VALUES(1,1,'local',?1,'accepted','',1)",[format!("\0{}","x".repeat(20000))])?;Ok(())}).await.unwrap();
    assert!(store.material_page(auth[0].clone(), 1, 0, 0).await.is_err());
}

#[tokio::test]
async fn material_authenticated_decision_invalidates_immutable_published_evidence() {
    let _lock = crate::auth::test_resource_lock().await;
    let _runtime = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    let version = published_fixture(&store).await;
    let before = store.definition_version(version).await.unwrap();
    assert_eq!(
        before.wiring_status,
        crate::definitions::WiringStatus::Current
    );
    store
        .material_review_unit(
            auth[0].clone(),
            auth[0].csrf_fixture(),
            1,
            1,
            crate::sources::UnitReview {
                revision: 1,
                state: "corrected".into(),
                correction: "Source-compared correction".into(),
                rationale: "Rechecked synthetic original".into(),
                actor: String::new(),
            },
        )
        .await
        .unwrap();
    let after = store.definition_version(version).await.unwrap();
    assert_eq!(after.wiring_status, crate::definitions::WiringStatus::Stale);
    assert_eq!(before.approval_id, after.approval_id);
    assert_eq!(before.content_hash, after.content_hash);
}

#[tokio::test]
async fn material_missing_wrong_and_revoked_csrf_cannot_change_either_review_kind() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    for path in [
        "/admin/materials/pages/1",
        "/admin/materials/pages/1/units/1",
    ] {
        let form = if path.ends_with("units/1") {
            decision(1)
        } else {
            vec![
                ("csrf".into(), "a".repeat(64)),
                ("revision".into(), "1".into()),
                ("printed_label".into(), "1".into()),
                ("rationale".into(), "Compared original".into()),
            ]
        };
        let mut missing = form.clone();
        missing.retain(|r| r.0 != "csrf");
        assert_eq!(
            router_request(&app, &auth[0], path, Some(missing))
                .await
                .status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        let mut wrong = form;
        wrong.iter_mut().find(|r| r.0 == "csrf").unwrap().1 = "wrong".into();
        assert_eq!(
            router_request(&app, &auth[0], path, Some(wrong))
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
    }
    store
        .run(|db| {
            db.execute(
                "UPDATE auth_sessions SET state='revoked',payload=NULL WHERE account_id=1",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    assert_eq!(
        router_request(
            &app,
            &auth[0],
            "/admin/materials/pages/1/units/1",
            Some(decision(1))
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    store
        .run(|db| {
            assert_eq!(
                db.query_row("SELECT count(*) FROM source_review_events", [], |r| r
                    .get::<_, i64>(0))?,
                0
            );
            assert_eq!(
                db.query_row("SELECT revision FROM source_pages WHERE id=1", [], |r| r
                    .get::<_, i64>(0))?,
                1
            );
            Ok(())
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn material_router_owner_matrix_unicode_limit_and_atomic_attribution() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture_with_owner(101).await;
    seed(&store).await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    for path in [
        "/admin/materials",
        "/admin/materials/sources/1",
        "/admin/materials/pages/1",
        "/admin/materials/pages/1/image",
        "/admin/materials/pages/1/units/1/image",
    ] {
        assert_eq!(
            router_request(&app, &auth[1], path, None).await.status(),
            StatusCode::FORBIDDEN
        );
    }
    let page = body(router_request(&app, &auth[0], "/admin/materials/pages/1", None).await).await;
    assert!(page.contains("Material &#60;title&#62;") || page.contains("Material &lt;title&gt;"));
    assert!(!page.contains("<script>bad()"));
    assert!(page.contains("Next units"));
    let mut large = decision(1);
    large[3].1 = "🦀".repeat(20000);
    large[4].1 = "🦀".repeat(5000);
    assert_eq!(
        router_request(
            &app,
            &auth[0],
            "/admin/materials/pages/1/units/1",
            Some(large)
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    store
        .run(|db| {
            assert_eq!(
                db.query_row(
                    "SELECT actor FROM source_review_events WHERE id=1",
                    [],
                    |r| r.get::<_, String>(0)
                )?,
                "account:101"
            );
            assert_eq!(
                db.query_row(
                    "SELECT actor_id FROM source_web_events WHERE event_id=1",
                    [],
                    |r| r.get::<_, i64>(0)
                )?,
                101
            );
            for sql in [
                "DELETE FROM source_web_events",
                "UPDATE source_web_events SET actor_epoch=2",
                "INSERT OR REPLACE INTO source_web_events SELECT * FROM source_web_events",
            ] {
                assert!(db.execute(sql, []).is_err());
            }
            Ok(())
        })
        .await
        .unwrap();
    let mut hostile = decision(2);
    hostile.push(("actor".into(), "someone else".into()));
    assert_eq!(
        router_request(
            &app,
            &auth[0],
            "/admin/materials/pages/1/units/1",
            Some(hostile)
        )
        .await
        .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let mut excessive = decision(2);
    excessive[3].1 = "x".repeat(330_000);
    assert_eq!(
        router_request(
            &app,
            &auth[0],
            "/admin/materials/pages/1/units/1",
            Some(excessive)
        )
        .await
        .status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
    assert_only_first_material_decision_was_saved(&store).await;
}

async fn assert_only_first_material_decision_was_saved(store: &Store) {
    store
        .run(|db| {
            assert_eq!(
                db.query_row("SELECT revision FROM source_units WHERE id=1", [], |r| r
                    .get::<_, i64>(0))?,
                2
            );
            assert_eq!(
                db.query_row("SELECT revision FROM source_pages WHERE id=1", [], |r| r
                    .get::<_, i64>(0))?,
                2
            );
            for table in ["source_review_events", "source_web_events"] {
                assert_eq!(
                    db.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r
                        .get::<_, i64>(0))?,
                    1
                );
            }
            Ok(())
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn material_queued_expiry_and_revocation_reject_both_mutation_kinds() {
    let _lock = crate::auth::test_resource_lock().await;
    for complete in [false, true] {
        for revoke in [false, true] {
            let (_dir, store, auth, clock) = fixture().await;
            seed(&store).await;
            let (entered, waiting) = tokio::sync::oneshot::channel();
            let (release, released) = std::sync::mpsc::channel();
            let locking = store.clone();
            let lock = tokio::spawn(async move {
                locking.run(move|db|{entered.send(()).unwrap();released.recv_timeout(std::time::Duration::from_secs(5)).unwrap();if revoke{db.execute("UPDATE auth_sessions SET state='revoked',payload=NULL WHERE account_id=1",[])?;}Ok(())}).await
            });
            waiting.await.unwrap();
            let saving = store.clone();
            let captured = auth[0].clone();
            let write = tokio::spawn(async move {
                if complete {
                    saving
                        .material_complete(
                            captured.clone(),
                            captured.csrf_fixture(),
                            1,
                            crate::sources::PageReview {
                                revision: 1,
                                printed_label: "1".into(),
                                rationale: "Checked".into(),
                                actor: String::new(),
                            },
                        )
                        .await
                } else {
                    saving
                        .material_review_unit(
                            captured.clone(),
                            captured.csrf_fixture(),
                            1,
                            1,
                            crate::sources::UnitReview {
                                revision: 1,
                                state: "accepted".into(),
                                correction: String::new(),
                                rationale: "Checked".into(),
                                actor: String::new(),
                            },
                        )
                        .await
                }
            });
            tokio::time::timeout(std::time::Duration::from_secs(5), async {
                while crate::store::AUTH_WORKERS.available_permits() == 16 {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            if !revoke {
                clock.store(1000, Ordering::SeqCst);
            }
            release.send(()).unwrap();
            lock.await.unwrap().unwrap();
            assert!(matches!(write.await.unwrap(), Err(AppError::NotFound)));
            store
                .run(|db| {
                    assert_eq!(
                        db.query_row("SELECT revision FROM source_pages WHERE id=1", [], |r| r
                            .get::<_, i64>(0))?,
                        1
                    );
                    assert_eq!(
                        db.query_row("SELECT count(*) FROM source_web_events", [], |r| r
                            .get::<_, i64>(0))?,
                        0
                    );
                    Ok(())
                })
                .await
                .unwrap();
        }
    }
}

#[tokio::test]
async fn material_registry_ids_are_byte_bounded_including_nul() {
    let _lock = crate::auth::test_resource_lock().await;
    for (game, edition) in [
        ("\0".to_owned() + &"g".repeat(65), "edition".into()),
        ("game".into(), "\0".to_owned() + &"e".repeat(65)),
    ] {
        let (_dir, store, auth, _clock) = fixture().await;
        seed(&store).await;
        store
            .run(move |db| {
                db.execute("INSERT INTO registry_games VALUES(?1,'Synthetic')", [&game])?;
                db.execute(
                    "INSERT INTO registry_editions VALUES(?1,?2,'Synthetic',0)",
                    params![game, edition],
                )?;
                db.execute(
                    "INSERT INTO source_rulesets VALUES(1,?1,?2)",
                    params![game, edition],
                )?;
                Ok(())
            })
            .await
            .unwrap();
        assert!(
            store.material_sources(auth[0].clone(), 0).await.is_err(),
            "oversized present registry identity must fail"
        );
    }
}

#[tokio::test]
async fn material_nul_validation_is_structured_and_does_not_mutate() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    for nul in ["\0content", "con\0tent"] {
        for field in 0..2 {
            let review = crate::sources::UnitReview {
                revision: 1,
                state: "corrected".into(),
                correction: if field == 0 {
                    nul.into()
                } else {
                    "good".into()
                },
                rationale: if field == 1 {
                    nul.into()
                } else {
                    "checked".into()
                },
                actor: String::new(),
            };
            assert!(matches!(
                store
                    .material_review_unit(auth[0].clone(), auth[0].csrf_fixture(), 1, 1, review)
                    .await,
                Err(AppError::Invalid(_))
            ));
        }
        assert!(matches!(
            store
                .material_complete(
                    auth[0].clone(),
                    auth[0].csrf_fixture(),
                    1,
                    crate::sources::PageReview {
                        revision: 1,
                        printed_label: nul.into(),
                        rationale: "checked".into(),
                        actor: String::new()
                    }
                )
                .await,
            Err(AppError::Invalid(_))
        ));
    }
    store
        .run(|db| {
            assert_eq!(
                db.query_row("SELECT revision FROM source_units WHERE id=1", [], |r| r
                    .get::<_, i64>(0))?,
                1
            );
            assert_eq!(
                db.query_row("SELECT count(*) FROM source_review_events", [], |r| r
                    .get::<_, i64>(0))?,
                0
            );
            Ok(())
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn material_region_guards_report_storage_type_and_polygon_overflow() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    for sql in [
        "UPDATE source_pages SET png='text' WHERE id=1",
        "UPDATE sources SET prepared_json='text' WHERE id=1",
    ] {
        store
            .run(move |db| {
                db.execute(sql, [])?;
                Ok(())
            })
            .await
            .unwrap();
        let result = store
            .material_region_data(
                auth[0].clone(),
                1,
                1,
                crate::sources::region::admit().unwrap(),
            )
            .await;
        assert!(
            matches!(result, Err(AppError::Invalid(_))),
            "invalid stored region type must fail before crop"
        );
        store
            .run(|db| {
                db.execute("UPDATE source_pages SET png=x'89504e47' WHERE id=1", [])?;
                db.execute("UPDATE sources SET prepared_json=x'' WHERE id=1", [])?;
                Ok(())
            })
            .await
            .unwrap();
    }
    store
        .run(|db| {
            db.execute(
                "UPDATE source_units SET polygon=?1 WHERE id=1",
                [format!("\0{}", "x".repeat(1024))],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    assert!(matches!(
        store
            .material_region_data(
                auth[0].clone(),
                1,
                1,
                crate::sources::region::admit().unwrap()
            )
            .await,
        Err(AppError::Invalid(_))
    ));
}

#[tokio::test]
async fn material_pagination_completeness_guard_and_audit_rollback() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    let data = store.material_page(auth[0].clone(), 1, 0, 0).await.unwrap();
    assert_eq!(data.page.units.len(), 50);
    assert_eq!(data.units_next, Some(50));
    let data = store
        .material_page(auth[0].clone(), 1, 50, 0)
        .await
        .unwrap();
    assert_eq!(data.page.units[0].id, 51);
    assert_eq!(data.units_next, None);
    assert!(
        store
            .material_page(auth[0].clone(), 1, -1, 0)
            .await
            .is_err()
    );
    assert!(
        store
            .material_complete(
                auth[0].clone(),
                auth[0].csrf_fixture(),
                1,
                crate::sources::PageReview {
                    revision: 1,
                    printed_label: "1".into(),
                    rationale: "Full page checked".into(),
                    actor: String::new()
                }
            )
            .await
            .is_err()
    );
    store.run(|db|{db.execute_batch("CREATE TRIGGER material_audit_fault BEFORE INSERT ON source_web_events BEGIN SELECT RAISE(ABORT,'fixture fault'); END;")?;Ok(())}).await.unwrap();
    let review = crate::sources::UnitReview {
        revision: 1,
        state: "accepted".into(),
        correction: String::new(),
        rationale: "Compared original".into(),
        actor: "forged".into(),
    };
    assert!(
        store
            .material_review_unit(auth[0].clone(), auth[0].csrf_fixture(), 1, 1, review)
            .await
            .is_err()
    );
    store
        .run(|db| {
            assert_eq!(
                db.query_row("SELECT revision FROM source_units WHERE id=1", [], |r| r
                    .get::<_, i64>(0))?,
                1
            );
            assert_eq!(
                db.query_row("SELECT count(*) FROM source_review_events", [], |r| r
                    .get::<_, i64>(0))?,
                0
            );
            Ok(())
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn material_byte_guards_csrf_and_revocation_fail_closed() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, clock) = fixture().await;
    seed(&store).await;
    store
        .run(|db| {
            replace_unit(db, "unit-1", &format!("\0{}", "x".repeat(65536)))?;
            Ok(())
        })
        .await
        .unwrap();
    assert!(store.material_page(auth[0].clone(), 1, 0, 0).await.is_err());
    store
        .run(|db| {
            replace_unit(db, "unit-1", &"x".repeat(65536))?;
            Ok(())
        })
        .await
        .unwrap();
    assert!(store.material_page(auth[0].clone(), 1, 0, 0).await.is_ok());
    let app = crate::auth::app(store.clone()).await.unwrap();
    let mut form = decision(1);
    form[0].1 = "wrong".into();
    assert_eq!(
        router_request(
            &app,
            &auth[0],
            "/admin/materials/pages/1/units/1",
            Some(form)
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    clock.store(1000, Ordering::SeqCst);
    assert!(store.material_image(auth[0].clone(), 1).await.is_err());
    assert!(store.material_recheck(auth[0].clone()).await.is_err());
}

#[tokio::test]
async fn material_review_path_maintains_the_search_index() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    let fts = |store: &Store| {
        let store = store.clone();
        async move {
            store
                .run(|db| {
                    let mut query = db.prepare(
                        "SELECT rowid,body FROM source_search_fts WHERE rowid=1 ORDER BY rowid",
                    )?;
                    let rows = query
                        .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
                        .collect::<Result<Vec<_>, _>>()?;
                    Ok(rows)
                })
                .await
                .unwrap()
        }
    };
    assert_eq!(fts(&store).await, Vec::<(i64, String)>::new());
    for (revision, state, correction, expected) in [
        (1, "accepted", "", Some("Fixture\n")),
        (2, "corrected", "Owner correction", Some("Owner correction")),
        (3, "rejected", "", None),
    ] {
        store
            .material_review_unit(
                auth[0].clone(),
                auth[0].csrf_fixture(),
                1,
                1,
                crate::sources::UnitReview {
                    revision,
                    state: state.into(),
                    correction: correction.into(),
                    actor: String::new(),
                    rationale: "Compared synthetic original".into(),
                },
            )
            .await
            .unwrap();
        assert_eq!(
            fts(&store).await,
            expected
                .map(|body| vec![(1, body.to_owned())])
                .unwrap_or_default()
        );
    }
}

/// Re-review (Space Bunny N2): a stored correction and a rejected draft's rationale keep their
/// leading newlines on the material page (HTML parsing drops the first newline after
/// `<textarea>`; the template emits a sacrificial one).
#[tokio::test]
async fn material_textareas_keep_leading_newlines() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    let mut form = decision(1);
    form[3].1 = "\ncorrected text".into();
    form[4].1 = "\n\nchecked original".into();
    let response = document_post(&app, &auth[0], form.clone()).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let textarea = |html: &str, name: &str| -> String {
        scraper::Html::parse_document(html)
            .select(&scraper::Selector::parse(&format!("textarea[name={name}]")).unwrap())
            .next()
            .unwrap()
            .text()
            .collect()
    };
    let page = body(router_get(&app, &auth[0], "/admin/materials/pages/1").await).await;
    assert_eq!(
        textarea(&page, "correction"),
        "\ncorrected text",
        "stored correction"
    );
    // A stale resubmission re-renders the rejected draft's rationale.
    let response = document_post(&app, &auth[0], form).await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let html = body(response).await;
    assert_eq!(
        textarea(&html, "rationale"),
        "\n\nchecked original",
        "draft rationale"
    );
}

/// Re-review (Space Bunny N6): the submitted revision is an integer, so a malformed one is
/// refused before any page renders and can never be echoed twice.
#[tokio::test]
async fn material_malformed_revision_is_not_echoed() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    let mut form = decision(1);
    form[1].1 = "\"".repeat(10_000);
    let response = document_post(&app, &auth[0], form).await;
    assert!(response.status().is_client_error(), "{}", response.status());
    let html = body(response).await;
    assert!(
        !html.contains("Your draft was not saved"),
        "no draft re-render"
    );
    assert!(
        html.matches("&#34;").count() < 64,
        "the revision is not echoed"
    );
}

async fn router_get(
    app: &axum::Router,
    auth: &RequestAuth,
    path: &str,
) -> axum::response::Response {
    use tower::ServiceExt;
    app.clone()
        .oneshot(
            axum::http::Request::builder()
                .uri(path)
                .header("host", "127.0.0.1:3000")
                .header("accept", "text/html")
                .header("sec-fetch-mode", "navigate")
                .header("cookie", auth.fixture_cookie())
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

/// Final review (Space Bunny F2): the page-completeness rationale of a rejected draft keeps its
/// leading newlines (the `Some(form)` branch of the completion textarea).
#[tokio::test]
async fn material_completion_rationale_keeps_leading_newlines() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    let form = vec![
        ("csrf".into(), "a".repeat(64)),
        ("revision".into(), "0".into()),
        ("printed_label".into(), "1".into()),
        ("rationale".into(), "\n\nwhole page checked".into()),
    ];
    let response = document_at(&app, &auth[0], "/admin/materials/pages/1", form).await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let html = body(response).await;
    let page = scraper::Html::parse_document(&html);
    let rationale: String = page
        .select(
            &scraper::Selector::parse(
                "form[action=\"/admin/materials/pages/1\"] textarea[name=rationale]",
            )
            .unwrap(),
        )
        .next()
        .expect("completion rationale")
        .text()
        .collect();
    assert_eq!(rationale, "\n\nwhole page checked");
}

/// Last review (Space Bunny F1, F4; Sonnet): material review normalises browser CR LF before
/// validation (20,000 typed correction characters and 5,000 rationale characters accepted,
/// stored with LF); the history `<pre>` keeps a leading newline; the empty completion textarea
/// (the `else` branch) is empty in a browser.
#[tokio::test]
async fn material_crlf_at_the_limit_is_stored_as_lf_and_shown_intact() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    let page = body(router_get(&app, &auth[0], "/admin/materials/pages/1").await).await;
    let parsed = scraper::Html::parse_document(&page);
    let completion: String = parsed
        .select(
            &scraper::Selector::parse(
                "form[action=\"/admin/materials/pages/1\"] textarea[name=rationale]",
            )
            .unwrap(),
        )
        .next()
        .expect("empty completion rationale")
        .text()
        .collect();
    assert_eq!(completion, "", "the else branch is empty in a browser");
    let correction = "\nx".repeat(10_000);
    let rationale = "y\n".repeat(2_500);
    let mut form = decision(1);
    form[3].1 = correction.replace('\n', "\r\n");
    form[4].1 = rationale.replace('\n', "\r\n");
    let response = document_post(&app, &auth[0], form).await;
    assert_eq!(
        response.status(),
        StatusCode::SEE_OTHER,
        "typed characters within limits"
    );
    let stored: (String, String) = store
        .run(|db| {
            Ok(db.query_row(
                "SELECT u.correction,e.rationale FROM source_units u JOIN source_review_events e ON e.unit_id=u.id WHERE u.id=1 ORDER BY e.id DESC LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?)
        })
        .await
        .unwrap();
    assert_eq!(stored, (correction.clone(), rationale), "stored with LF");
    let page = body(router_get(&app, &auth[0], "/admin/materials/pages/1").await).await;
    let history: String = scraper::Html::parse_document(&page)
        .select(&scraper::Selector::parse("pre.evidence-text").unwrap())
        .next()
        .expect("history correction")
        .text()
        .collect();
    assert_eq!(
        history, correction,
        "history <pre> keeps the leading newline"
    );
}
