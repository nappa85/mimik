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
async fn material_final_all_review_reads_have_deadlines() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    finalize_material(&store).await;
    let id = suggestion_fixture(&store, "correction", suggestion_unit(1)).await;
    let mut missing = Vec::new();
    for label in ["page", "cursor", "list", "detail"] {
        store
            .force_read_deadline(label, std::time::Duration::ZERO)
            .await;
        let result = match label {
            "page" => store
                .material_page(auth[0].clone(), 1, 0, 0)
                .await
                .map(|_| ()),
            "cursor" => store
                .material_unit_cursor(auth[0].clone(), 1, 1)
                .await
                .map(|_| ()),
            "list" => store
                .material_advice_list(auth[0].clone(), 1, 1, 0)
                .await
                .map(|_| ()),
            _ => store.material_advice(auth[0].clone(), id).await.map(|_| ()),
        };
        store.clear_forced_read_deadline().await;
        eprintln!("{label} forced expiry: {result:?}");
        if !matches!(result, Err(AppError::Invalid(message)) if message.contains("time limit")) {
            missing.push(label);
        }
        store
            .material_page(auth[0].clone(), 1, 0, 0)
            .await
            .expect("handler must be cleared for next request");
    }
    assert!(
        missing.is_empty(),
        "reads missing clear deadline errors: {missing:?}"
    );
    let app = crate::auth::app(store.clone()).await.unwrap();
    for (label, path) in [
        ("page", "/admin/materials/pages/1".to_owned()),
        (
            "cursor",
            "/admin/materials/pages/1/units/1/suggestions".to_owned(),
        ),
        (
            "list",
            "/admin/materials/pages/1/units/1/suggestions".to_owned(),
        ),
        ("detail", format!("/admin/materials/suggestions/{id}")),
    ] {
        store
            .force_read_deadline(label, std::time::Duration::ZERO)
            .await;
        assert_eq!(
            router_get(&app, &auth[0], &path).await.status(),
            StatusCode::REQUEST_TIMEOUT,
            "{label}"
        );
        assert_eq!(
            router_get(&app, &auth[0], &path).await.status(),
            StatusCode::OK,
            "next {label} request"
        );
    }
}

#[tokio::test]
async fn material_final_deadline_lookup_is_not_a_text_preview() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    finalize_material(&store).await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    store
        .force_read_deadline("screen", std::time::Duration::ZERO)
        .await;
    let response = router_get(&app, &auth[0], "/admin/materials/pages/1").await;
    assert_eq!(response.status(), StatusCode::OK);
    let html = body(response).await;
    assert!(html.contains("Lookup not read (time limit)"));
    assert!(!html.contains("response text budget reached"));
    let parsed = scraper::Html::parse_document(&html);
    assert_eq!(
        parsed
            .select(&scraper::Selector::parse("#lookup-panel pre").unwrap())
            .count(),
        0
    );
    assert_eq!(
        router_get(&app, &auth[0], "/admin/materials/pages/1")
            .await
            .status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn material_final_empty_edit_is_retained_and_not_applied() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    finalize_material(&store).await;
    let id = suggestion_fixture(&store, "correction", suggestion_unit(1)).await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    let response = document_at(
        &app,
        &auth[0],
        &format!("/admin/materials/suggestions/{id}/accept"),
        review_form(&[
            ("edit", "suggested"),
            ("edited_text", ""),
            ("rationale", "Checked"),
        ]),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        field_value(&body(response).await, "textarea[name=edited_text]"),
        ""
    );
    assert_eq!(
        store
            .material_advice(auth[0].clone(), id)
            .await
            .unwrap()
            .state,
        "open"
    );
}

#[tokio::test]
async fn material_final_unchanged_cr_suggestion_is_accepted() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    finalize_material(&store).await;
    let id = store
        .insert_source_suggestion(crate::sources::SourceSuggestion {
            source: 1,
            bundle_hash: "a".repeat(64),
            origin: "tool:fixture".into(),
            kind: "correction".into(),
            target: suggestion_unit(1),
            suggested_text: Some("one\rtwo\r\nthree".into()),
            rationale: "Checked".into(),
            report_id: None,
        })
        .await
        .unwrap();
    let app = crate::auth::app(store.clone()).await.unwrap();
    let response = document_at(
        &app,
        &auth[0],
        &format!("/admin/materials/suggestions/{id}/accept"),
        review_form(&[
            ("edit", "suggested"),
            ("edited_text", "one\r\ntwo\r\nthree"),
            ("rationale", "Checked"),
        ]),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        store
            .material_advice(auth[0].clone(), id)
            .await
            .unwrap()
            .state,
        "applied"
    );
}

#[tokio::test]
async fn material_final_invalid_state_copies_fit_budget() {
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
                    correction: "\"".repeat(20000),
                    actor: String::new(),
                    rationale: "Checked".into(),
                },
            )
            .await
            .unwrap();
    }
    let app = crate::auth::app(store).await.unwrap();
    let invalid = "\"".repeat(108_000);
    let response = document_at(
        &app,
        &auth[0],
        "/admin/materials/pages/1/units/1",
        review_form(&[
            ("revision", "2"),
            ("state", &invalid),
            ("correction", ""),
            ("rationale", "Checked"),
        ]),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let html = body(response).await;
    let parsed = scraper::Html::parse_document(&html);
    let bytes = html
        .split_once("<section id=\"review-list\"")
        .unwrap()
        .1
        .split_once("<section class=\"panel\" id=\"add-text\"")
        .unwrap()
        .0
        .len();
    eprintln!(
        "invalid-state budgeted sections: {bytes}; full page: {}",
        html.len()
    );
    // The submitted invalid decision is shown in full; no radio claims to be chosen.
    assert!(html.contains("Submitted invalid decision: &#34;&#34;&#34;"));
    assert!(
        parsed
            .select(&scraper::Selector::parse("#unit-1 option[selected]:not([value=''])").unwrap())
            .next()
            .is_none()
    );
    assert!(bytes <= 2 * 1024 * 1024, "budgeted sections: {bytes}");
}

#[tokio::test]
async fn material_final_maximum_suggestion_markup_fits_budget() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    finalize_material(&store).await;
    store.run(|db| {
        let tx = db.transaction()?;
        for unit in 1..=50 {
            for _ in 0..20 {
                tx.execute("INSERT INTO source_suggestions(source_id,bundle_hash,origin,kind,targets,suggested_text,rationale) VALUES(1,?1,?2,'correction',?3,'x','Checked')", params!["a".repeat(64), "\"".repeat(100), serde_json::to_string(&suggestion_unit(unit))?])?;
            }
        }
        for _ in 0..20 {
            tx.execute("INSERT INTO source_suggestions(source_id,bundle_hash,origin,kind,targets,suggested_text,rationale) VALUES(1,?1,?2,'missing_text',?3,'x','Checked')", params!["a".repeat(64), "\"".repeat(100), serde_json::json!({"type":"region","page":1,"polygon":[[1,1],[20,1],[20,20],[1,20]]}).to_string()])?;
        }
        tx.commit()?;
        Ok(())
    }).await.unwrap();
    let app = crate::auth::app(store).await.unwrap();
    let response = router_get(&app, &auth[0], "/admin/materials/pages/1").await;
    assert_eq!(response.status(), StatusCode::OK);
    let html = body(response).await;
    assert!(
        !html.contains("Screen read deadline reached"),
        "fixture must exercise row rendering"
    );
    let parsed = scraper::Html::parse_document(&html);
    let bytes = html
        .split_once("<section id=\"review-list\"")
        .unwrap()
        .1
        .split_once("<section class=\"panel\" id=\"add-text\"")
        .unwrap()
        .0
        .len();
    let rows = parsed
        .select(&scraper::Selector::parse(".suggestion").unwrap())
        .count();
    eprintln!(
        "maximum suggestion markup: {rows} rows, {bytes} budgeted bytes, {} full bytes",
        html.len()
    );
    assert_eq!(rows, 32, "fixed charges must cap the screen at 32 rows");
    assert_eq!(
        parsed
            .select(&scraper::Selector::parse("#page-notes .suggestion").unwrap())
            .count(),
        20,
        "unit advice must not starve the independent notes cursor"
    );
    assert!(bytes <= 2 * 1024 * 1024, "budgeted sections: {bytes}");
}

#[tokio::test]
async fn material_confirmation_detail_worker_and_authority_recheck() {
    let _lock = crate::auth::test_resource_lock().await;
    let (dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    finalize_material(&store).await;
    let id = suggestion_fixture(&store, "correction", suggestion_unit(1)).await;
    let app = crate::auth::app(store).await.unwrap();
    let executor = std::thread::current().id();
    let (send, receive) = std::sync::mpsc::channel();
    let path = dir.path().join("private.sqlite3");
    *crate::sources::materials::DETAIL_COMPARISON.lock().unwrap() = Some(Box::new(move || {
        send.send(std::thread::current().id()).unwrap();
        let db = rusqlite::Connection::open(&path).unwrap();
        crate::store::register_auth_clock(&db, Arc::new(|| Ok(100))).unwrap();
        db.execute(
            "UPDATE auth_sessions SET state='revoked',payload=NULL WHERE account_id=1",
            [],
        )
        .unwrap();
    }));
    let response = router_get(
        &app,
        &auth[0],
        &format!("/admin/materials/suggestions/{id}"),
    )
    .await;
    assert_ne!(
        receive.try_recv().unwrap(),
        executor,
        "comparison must run off the async executor"
    );
    assert_eq!(
        response.status(),
        StatusCode::FORBIDDEN,
        "authority must be rechecked after comparison"
    );
}

#[tokio::test]
async fn material_confirmation_keyboard_box_warns_of_replacement() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    finalize_material(&store).await;
    let app = crate::auth::app(store).await.unwrap();
    let html = body(router_get(&app, &auth[0], "/admin/materials/pages/1").await).await;
    let parsed = scraper::Html::parse_document(&html);
    let button = parsed
        .select(&scraper::Selector::parse("#keyboard-region").unwrap())
        .next()
        .unwrap();
    assert!(
        button
            .text()
            .collect::<String>()
            .contains("replaces entered polygon")
    );
}

#[tokio::test]
async fn material_confirmation_missing_primary_is_forbidden() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    finalize_material(&store).await;
    let app = crate::auth::app(store).await.unwrap();
    let response = document_at(
        &app,
        &auth[0],
        "/admin/materials/pages/999/primary",
        review_form(&[
            ("source", "1"),
            ("previous", "0"),
            ("event", "0"),
            ("rationale", "Checked"),
        ]),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn material_confirmation_suggested_keeps_changed_edit() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    finalize_material(&store).await;
    let id = suggestion_fixture(&store, "correction", suggestion_unit(1)).await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    let response = document_at(
        &app,
        &auth[0],
        &format!("/admin/materials/suggestions/{id}/accept"),
        review_form(&[
            ("edit", "suggested"),
            ("edited_text", "Owner changed < & \""),
            ("rationale", "Checked"),
        ]),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        field_value(&body(response).await, "textarea[name=edited_text]"),
        "Owner changed < & \""
    );
    assert_eq!(
        store
            .material_advice(auth[0].clone(), id)
            .await
            .unwrap()
            .state,
        "open"
    );
}

#[tokio::test]
async fn material_confirmation_suggested_truncation_is_not_lookup_truncation() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    finalize_material(&store).await;
    let id = store
        .insert_source_suggestion(crate::sources::SourceSuggestion {
            source: 1,
            bundle_hash: "a".repeat(64),
            origin: "tool:fixture".into(),
            kind: "correction".into(),
            target: suggestion_unit(1),
            suggested_text: Some("\"".repeat(20000)),
            rationale: "x".repeat(5000),
            report_id: None,
        })
        .await
        .unwrap();
    let app = crate::auth::app(store.clone()).await.unwrap();
    let (rows, _) = store
        .material_advice_list(auth[0].clone(), 1, 1, 0)
        .await
        .unwrap();
    let advice = &rows[0];
    assert!(
        !advice.lookup_truncated,
        "suggestion length must not label lookup evidence as truncated"
    );
    let html = body(
        router_get(
            &app,
            &auth[0],
            &format!("/admin/materials/suggestions/{id}"),
        )
        .await,
    )
    .await;
    assert!(advice.suggested_truncated);
    assert!(html.contains("Suggested text is truncated"));
    assert_eq!(
        field_value(&html, "textarea[name=edited_text]"),
        "\"".repeat(20000)
    );
    assert!(!html.contains("Lookup text is truncated"));
}

#[tokio::test]
async fn material_confirmation_compact_follows_card_order() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    finalize_material(&store).await;
    suggestion_fixture(
        &store,
        "merge",
        crate::sources::SuggestionTarget::Units {
            targets: vec![
                crate::sources::SuggestionUnit {
                    unit: 1,
                    revision: 1,
                },
                crate::sources::SuggestionUnit {
                    unit: 2,
                    revision: 1,
                },
            ],
        },
    )
    .await;
    let screen = store
        .material_screen(auth[0].clone(), 1, vec![2, 1], 0)
        .await
        .unwrap();
    assert!(
        !screen.cards[0].advice[0].compact,
        "first rendered card owns the complete copy"
    );
    assert!(screen.cards[1].advice[0].compact);
}

#[tokio::test]
async fn material_confirmation_escaped_screen_sections_fit_budget() {
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
                    correction: match unit % 3 {
                        0 => "\"",
                        1 => "&",
                        _ => "<",
                    }
                    .repeat(20000),
                    actor: String::new(),
                    rationale: "Checked".into(),
                },
            )
            .await
            .unwrap();
    }
    let app = crate::auth::app(store).await.unwrap();
    let html = body(router_get(&app, &auth[0], "/admin/materials/pages/1").await).await;
    let parsed = scraper::Html::parse_document(&html);
    let bytes: usize = parsed
        .select(
            &scraper::Selector::parse("#review-list pre.evidence-text, [data-form=unit] textarea")
                .unwrap(),
        )
        .map(|node| node.html().len())
        .sum();
    eprintln!(
        "escape-heavy budgeted sections: {bytes} bytes; full page: {} bytes",
        html.len()
    );
    assert!(bytes <= 2 * 1024 * 1024, "escaped sections: {bytes}");
}

#[tokio::test]
async fn material_confirmation_distinct_merge_read_budget() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    // A performance bound on the real 250 ms read deadline: keep the production deadline.
    store.clear_read_deadlines().await;
    seed(&store).await;
    store.run(|db| {
        db.execute_batch("WITH RECURSIVE ids(id) AS (VALUES(52) UNION ALL SELECT id+1 FROM ids WHERE id<1073) INSERT INTO source_units(id,page_id,block_id,kind,polygon,raw_html,raw_text) SELECT id,1,'unit-'||id,'Text','[[1,1],[20,1],[20,20],[1,20]]','<p>Fixture</p>','Fixture' FROM ids;")?;
        db.execute("INSERT INTO source_pages(id,source_id,pdf_page,png) VALUES(2,1,2,x'')", [])?;
        db.execute("INSERT INTO source_units(id,page_id,block_id,kind,polygon,raw_html,raw_text) VALUES(1074,2,'other','Text','[[1,1],[20,1],[20,20],[1,20]]','<p>Other</p>','Other')", [])?;
        crate::sources::search::ensure(db)?;
        Ok(())
    }).await.unwrap();
    store.run(|db| {
        let prepared = serde_json::json!({"contract_version":1,"extraction_sha256":"b".repeat(64),"pages":[{"pdf_page":1,"width_points":200,"height_points":300},{"pdf_page":2,"width_points":200,"height_points":300}]});
        db.execute("UPDATE sources SET bundle_hash=?1,prepared_json=?2 WHERE id=1", params!["a".repeat(64),serde_json::to_vec(&prepared)?])?;
        for page in 1..=2 { db.execute("INSERT INTO source_page_hashes VALUES(?1,1,?2)",params![page,"c".repeat(64)])?; }
        db.execute("INSERT INTO source_finalizations VALUES(1,?1,?2)",params!["a".repeat(64),"b".repeat(64)])?;
        crate::sources::documents::register_import(db,1)?;
        let tx = db.transaction()?;
        for card in 1..=50 {
            let units = std::iter::once(card).chain(51..=1073).map(|unit| crate::sources::SuggestionUnit { unit, revision: 1 }).collect();
            let json = serde_json::to_string(&crate::sources::SuggestionTarget::Units { targets: units })?;
            for _ in 0..20 {
                tx.execute("INSERT INTO source_suggestions(source_id,bundle_hash,origin,kind,targets,suggested_text,rationale) VALUES(1,?1,'tool:fixture','merge',?2,'Combined','Checked')", params!["a".repeat(64),json])?;
            }
        }
        for _ in 0..1000 {
            tx.execute("INSERT INTO source_suggestions(source_id,bundle_hash,origin,kind,targets,suggested_text,rationale) VALUES(1,?1,'tool:fixture','correction',?2,'Other','Checked')", params!["a".repeat(64),serde_json::to_string(&suggestion_unit(1074))?])?;
        }
        tx.commit()?;
        Ok(())
    }).await.unwrap();
    let start = std::time::Instant::now();
    let screen = store
        .material_screen(auth[0].clone(), 1, (1..=50).collect(), 0)
        .await
        .unwrap();
    let elapsed = start.elapsed();
    let truncated = screen
        .cards
        .iter()
        .flat_map(|card| &card.advice)
        .filter(|row| row.lookup_truncated)
        .count();
    eprintln!("distinct merge screen reads: {elapsed:?}; truncated lookups: {truncated}");
    assert!(
        elapsed < std::time::Duration::from_secs(2),
        "screen must release the mutex well inside the request window: {elapsed:?}"
    );
    assert!(
        truncated > 0 || screen.cards.is_empty(),
        "1,024,000 target rows must exhaust the screen lookup-row budget"
    );
    tokio::time::timeout(
        std::time::Duration::from_millis(250),
        store.material_recheck(auth[0].clone()),
    )
    .await
    .expect("deadline handler must be cleared and store available")
    .unwrap();
    let app = crate::auth::app(store.clone()).await.unwrap();
    let start = std::time::Instant::now();
    let response = router_get(&app, &auth[0], "/admin/materials/pages/1").await;
    assert_eq!(response.status(), StatusCode::OK);
    let html = body(response).await;
    eprintln!(
        "distinct merge rendered response: {:?}; {} bytes",
        start.elapsed(),
        html.len()
    );
    assert!(start.elapsed() < std::time::Duration::from_secs(2));
    assert!(html.len() <= 2 * 1024 * 1024);
    assert!(
        html.contains("Screen read deadline reached")
            || html.contains("lookup text omitted or shortened")
    );
    let draft = "\"&<".repeat(20000);
    let response = document_at(
        &app,
        &auth[0],
        "/admin/materials/pages/1/units/1",
        review_form(&[
            ("revision", "1"),
            ("state", "corrected"),
            ("correction", &draft),
            ("rationale", ""),
        ]),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        field_value(
            &body(response).await,
            "[data-form=unit] textarea[name=correction]"
        ),
        draft
    );
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
    // Checks the production path (reduced screen after the 250 ms read deadline) inside the
    // request window, so it keeps the production deadline.
    store.clear_read_deadlines().await;
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
                .header("sec-fetch-dest", "document")
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
        let parsed = scraper::Html::parse_document(&html);
        let checked = parsed
            .select(
                &scraper::Selector::parse("#unit-1 select[name=state] option[selected]").unwrap(),
            )
            .map(|option| option.value().attr("value").unwrap())
            .collect::<Vec<_>>();
        if state == "invalid" {
            assert_eq!(checked, [""]);
            assert!(html.contains("Submitted invalid decision: invalid."));
        } else {
            assert_eq!(checked, [state]);
        }
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
                .header("sec-fetch-dest", "document")
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

/// Part 3 browser check: every Owner material page, the review screen first, carries the strict
/// sheet policy (scripts and styles from this origin only, Trusted Types with no policy).
/// Refusals can be HTML error documents rebuilt by `normalize` without the marker.
#[tokio::test]
async fn material_pages_carry_the_strict_policy() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, 4, 6);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .write_header()
            .unwrap()
            .write_image_data(&[0; 72])
            .unwrap();
    }
    store
        .run(move |db| {
            db.execute("UPDATE source_pages SET png=?1 WHERE id=1", [bytes])?;
            db.execute(
                "UPDATE source_units SET polygon='[[0,0],[200,0],[200,300],[0,300]]' WHERE id=1",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    finalize_material(&store).await;
    let suggestion = suggestion_fixture(&store, "correction", suggestion_unit(1)).await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    for path in [
        "/admin/materials",
        "/admin/materials/sources/1",
        "/admin/materials/pages/1",
        &format!("/admin/materials/suggestions/{suggestion}"),
        "/admin/materials/pages/1/image",
        "/admin/materials/pages/1/units/1/image",
        "/assets/source-review.js",
        "/assets/source-regions.js",
    ] {
        let response = router_get(&app, &auth[0], path).await;
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        assert_eq!(
            response.headers()[axum::http::header::CONTENT_SECURITY_POLICY],
            crate::campaigns::web::SHEET_CSP,
            "{path}"
        );
    }
    for (revision, status) in [
        (1, StatusCode::SEE_OTHER),
        (1, StatusCode::CONFLICT),
        (2, StatusCode::UNPROCESSABLE_ENTITY),
    ] {
        let mut fields = decision(revision);
        if status == StatusCode::UNPROCESSABLE_ENTITY {
            fields[4].1.clear();
        }
        let response = document_post(&app, &auth[0], fields).await;
        assert_eq!(response.status(), status);
        assert_eq!(
            response.headers()[axum::http::header::CONTENT_SECURITY_POLICY],
            crate::campaigns::web::SHEET_CSP,
            "{status}"
        );
    }
}

#[tokio::test]
async fn material_refusals_and_login_redirect_keep_default_policy() {
    use tower::ServiceExt;

    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    let app = crate::auth::app(store).await.unwrap();
    for (actor, path, status) in [
        (&auth[1], "/admin/materials/pages/1", StatusCode::FORBIDDEN),
        (
            &auth[0],
            "/admin/materials/not-a-route",
            // The shipping auth router deliberately makes unknown routes uniform refusals.
            StatusCode::FORBIDDEN,
        ),
    ] {
        let response = router_get(&app, actor, path).await;
        assert_eq!(response.status(), status);
        assert_eq!(
            response.headers()[axum::http::header::CONTENT_SECURITY_POLICY],
            "frame-ancestors 'none'"
        );
        assert!(
            response.headers()[axum::http::header::CONTENT_TYPE]
                .to_str()
                .unwrap()
                .starts_with("text/html")
        );
    }
    let response = app
        .oneshot(
            axum::http::Request::builder()
                .uri("/admin/materials/pages/1")
                .header("host", "127.0.0.1:3000")
                .header("accept", "text/html")
                .header("sec-fetch-mode", "navigate")
                .header("sec-fetch-dest", "document")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert!(
        response.headers()[axum::http::header::LOCATION]
            .to_str()
            .unwrap()
            .starts_with("/auth/login")
    );
    assert_eq!(
        response.headers()[axum::http::header::CONTENT_SECURITY_POLICY],
        "frame-ancestors 'none'"
    );
}

/// A slow machine (every review read of one store hitting its deadline) must not reach a
/// concurrent test's store: the overrides are keyed to the store's database file, and a
/// store without overrides gets a deadline that cannot expire under load.
#[tokio::test]
async fn review_read_deadline_overrides_are_scoped_to_their_store() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_slow_dir, slow, slow_auth, _slow_clock) = fixture().await;
    let (_dir, store, auth, _clock) = fixture().await;
    for store in [&slow, &store] {
        seed(store).await;
        finalize_material(store).await;
    }
    slow.update_read_deadlines(|hooks| {
        hooks.default = Some(std::time::Duration::ZERO);
        hooks.force_expiry = true;
    })
    .await;
    // A pending one-shot override of the same label on the other store is not consumed here.
    store
        .force_read_deadline("screen", std::time::Duration::ZERO)
        .await;
    let slow_app = crate::auth::app(slow.clone()).await.unwrap();
    let app = crate::auth::app(store.clone()).await.unwrap();
    // Every read of the slow store expires (the reduced fallback too), on every request.
    for _ in 0..2 {
        let status = router_get(&slow_app, &slow_auth[0], "/admin/materials/pages/1")
            .await
            .status();
        assert_eq!(status, StatusCode::REQUEST_TIMEOUT, "slow store");
    }
    // The other store's one-shot override fires exactly once, on its own store.
    let html = body(router_get(&app, &auth[0], "/admin/materials/pages/1").await).await;
    assert!(
        html.contains("Lookup not read (time limit)"),
        "own one-shot"
    );
    let html = body(router_get(&app, &auth[0], "/admin/materials/pages/1").await).await;
    assert!(
        !html.contains("Lookup not read (time limit)"),
        "one-shot spent"
    );
    slow.update_read_deadlines(|hooks| {
        hooks.default = Some(std::time::Duration::from_mins(10));
        hooks.force_expiry = false;
    })
    .await;
    let response = router_get(&slow_app, &slow_auth[0], "/admin/materials/pages/1").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        !body(response)
            .await
            .contains("Lookup not read (time limit)")
    );
}

fn accept_form(revision: i64, rationale: &str) -> Vec<(String, String)> {
    review_form(&[
        ("revision", &revision.to_string()),
        ("state", "accepted"),
        ("correction", ""),
        ("rationale", rationale),
    ])
}

fn location(response: &axum::response::Response) -> String {
    response
        .headers()
        .get(axum::http::header::LOCATION)
        .expect("redirect")
        .to_str()
        .unwrap()
        .to_owned()
}

/// Accept units 2..=51 in one batch so only unit 1 is left to review.
async fn accept_all_but_first(store: &Store, auth: &RequestAuth) {
    let decisions = (2..=51)
        .map(|unit| crate::sources::UnitDecision {
            page: 1,
            unit,
            review: crate::sources::UnitReview {
                revision: 1,
                state: "accepted".into(),
                correction: String::new(),
                actor: String::new(),
                rationale: String::new(),
            },
        })
        .collect();
    store
        .material_review_units_atomic(
            auth.clone(),
            auth.csrf_fixture(),
            decisions,
            "Checked".into(),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn material_accept_as_extracted_needs_no_rationale_but_keeps_the_audit_event() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    let response = document_at(
        &app,
        &auth[0],
        "/admin/materials/pages/1/units/1",
        accept_form(1, ""),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    store
        .run(|db| {
            let (rationale, actor, state): (String, String, String) = db.query_row(
                "SELECT rationale,actor,state FROM source_review_events WHERE unit_id=1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )?;
            assert_eq!((rationale.as_str(), state.as_str()), ("", "accepted"));
            assert!(actor.starts_with("account:"));
            let audited: i64 = db.query_row(
                "SELECT count(*) FROM source_web_events w JOIN source_review_events e ON e.id=w.event_id WHERE e.unit_id=1",
                [],
                |r| r.get(0),
            )?;
            assert_eq!(audited, 1, "the decision keeps its authenticated audit event");
            Ok(())
        })
        .await
        .unwrap();
    // Corrections and rejections still need a rationale.
    for (state, correction) in [("corrected", "Fixed"), ("rejected", "")] {
        let response = document_at(
            &app,
            &auth[0],
            "/admin/materials/pages/1/units/2",
            review_form(&[
                ("revision", "1"),
                ("state", state),
                ("correction", correction),
                ("rationale", "  "),
            ]),
        )
        .await;
        assert_eq!(
            response.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "{state}"
        );
    }
    let html = body(router_get(&app, &auth[0], "/admin/materials/pages/1").await).await;
    assert!(
        !scraper::Html::parse_document(&html)
            .select(
                &scraper::Selector::parse("[data-form=unit] textarea[name=rationale][required]")
                    .unwrap()
            )
            .any(|_| true),
        "the rationale field is not browser-required"
    );
}

#[tokio::test]
async fn material_save_confirms_and_lands_on_the_next_unreviewed_unit() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    let response = document_at(
        &app,
        &auth[0],
        "/admin/materials/pages/1/units/1",
        accept_form(1, ""),
    )
    .await;
    let target = location(&response);
    assert_eq!(
        target,
        "/admin/materials/pages/1?saved=accepted&saved_unit=1&to=2&then=unit#unit-2"
    );
    let html = body(router_get(&app, &auth[0], &target).await).await;
    assert!(
        html.contains("Saved unit 1: accepted as extracted."),
        "{html}"
    );
    let parsed = scraper::Html::parse_document(&html);
    assert!(
        parsed
            .select(&scraper::Selector::parse("#unit-2 strong").unwrap())
            .next()
            .is_some(),
        "the landing card repeats the confirmation"
    );
    // Unknown words are refused, not echoed.
    for forged in ["saved=%3Cb%3E", "saved=accepted&saved_unit=1&then=%3Cb%3E"] {
        let path = format!("/admin/materials/pages/1?{forged}");
        let response = router_get(&app, &auth[0], &path).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{forged}");
    }
    // A well-formed link that claims a save that did not happen shows no notice.
    for false_link in [
        "saved=rejected&saved_unit=1&then=unit",
        "saved=accepted&saved_unit=30&then=unit",
        "saved=accepted&saved_unit=1&then=done",
        "saved=accepted&saved_unit=999&then=unit",
    ] {
        let path = format!("/admin/materials/pages/1?{false_link}");
        let response = router_get(&app, &auth[0], &path).await;
        assert_eq!(response.status(), StatusCode::OK, "{false_link}");
        assert!(!body(response).await.contains("Saved unit"), "{false_link}");
    }
}

#[tokio::test]
async fn material_save_moves_to_the_next_page_or_the_summary_when_done() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    accept_all_but_first(&store, &auth[0]).await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    let response = document_at(
        &app,
        &auth[0],
        "/admin/materials/pages/1/units/1",
        accept_form(1, ""),
    )
    .await;
    assert_eq!(
        location(&response),
        "/admin/materials/pages/1?saved=accepted&saved_unit=1&then=done#complete"
    );
    let html = body(router_get(&app, &auth[0], &location(&response)).await).await;
    assert!(html.contains("No unit is left to review"));

    // With a second page that still has an unreviewed unit, the save moves there.
    store
        .run(|db| {
            db.execute("INSERT INTO source_pages(id,source_id,pdf_page,png) VALUES(2,1,2,x'89504e47')", [])?;
            db.execute("INSERT INTO source_units(id,page_id,block_id,kind,polygon,raw_html,raw_text) VALUES(100,2,'p2','Text','[]','<p>Two</p>','Two')", [])?;
            Ok(())
        })
        .await
        .unwrap();
    let next = store
        .material_next_after_review(auth[0].clone(), 1, 1)
        .await
        .unwrap();
    assert!(matches!(
        next,
        crate::sources::materials::ReviewNext::Page(2)
    ));
}

#[tokio::test]
async fn material_overlay_boxes_carry_their_review_state_and_a_legend() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    store
        .run(|db| {
            db.execute(
                "UPDATE source_units SET polygon='[[1,1],[20,1],[20,20],[1,20]]' WHERE id<=4",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    finalize_material(&store).await;
    for (unit, state, correction) in [
        (2, "accepted", ""),
        (3, "corrected", "Fixed"),
        (4, "rejected", ""),
    ] {
        store
            .material_review_unit(
                auth[0].clone(),
                auth[0].csrf_fixture(),
                1,
                unit,
                crate::sources::UnitReview {
                    revision: 1,
                    state: state.into(),
                    correction: correction.into(),
                    actor: String::new(),
                    rationale: "Checked".into(),
                },
            )
            .await
            .unwrap();
    }
    let app = crate::auth::app(store.clone()).await.unwrap();
    let html = body(router_get(&app, &auth[0], "/admin/materials/pages/1").await).await;
    let parsed = scraper::Html::parse_document(&html);
    for (unit, state) in [
        (1, "unreviewed"),
        (2, "accepted"),
        (3, "corrected"),
        (4, "rejected"),
    ] {
        let polygon = parsed
            .select(
                &scraper::Selector::parse(&format!("polygon[data-overlay-unit=\"{unit}\"]"))
                    .unwrap(),
            )
            .next()
            .unwrap_or_else(|| panic!("unit {unit} box"));
        assert!(
            polygon
                .value()
                .classes()
                .any(|c| c == format!("state-{state}")),
            "unit {unit}"
        );
        assert!(html.contains(&format!("aria-label=\"Review unit {unit} ({state})\"")));
    }
    for label in [
        "Unreviewed (dashed blue)",
        "Accepted (solid green)",
        "Corrected (dotted orange)",
        "Rejected (dash-dot red)",
    ] {
        assert!(html.contains(label), "legend {label}");
    }
}

#[tokio::test]
async fn material_primary_pages_checkbox_is_labelled_and_explained() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    finalize_material(&store).await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    let response = router_get(&app, &auth[0], "/admin/materials/sources/1").await;
    assert_eq!(response.status(), StatusCode::OK);
    let html = body(response).await;
    let parsed = scraper::Html::parse_document(&html);
    let labels: Vec<String> = parsed
        .select(&scraper::Selector::parse("label:has(input[type=checkbox])").unwrap())
        .map(|label| label.text().collect())
        .collect();
    assert_eq!(labels.len(), 1, "one page row: {labels:?}");
    assert!(
        labels
            .iter()
            .all(|text| text.contains("primary reading for PDF page")),
        "{labels:?}"
    );
    assert!(html.contains("Ticking alone changes nothing."));
}

#[tokio::test]
async fn material_save_lookup_failure_is_unknown_not_finished() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    accept_all_but_first(&store, &auth[0]).await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    store
        .force_read_deadline("next", std::time::Duration::ZERO)
        .await;
    let response = document_at(
        &app,
        &auth[0],
        "/admin/materials/pages/1/units/1",
        accept_form(1, ""),
    )
    .await;
    let target = location(&response);
    assert_eq!(
        target,
        "/admin/materials/pages/1?saved=accepted&saved_unit=1&then=unknown#unit-1"
    );
    let html = body(router_get(&app, &auth[0], &target).await).await;
    assert!(html.contains("The next unit could not be found in time"));
    assert!(!html.contains("No unit is left to review"));
}

#[tokio::test]
async fn material_save_redirects_with_card_cursor_and_to_the_next_page() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    // Units 2..=50 are accepted, so after unit 1 the next unreviewed unit is #51 (position 51).
    let decisions = (2..=50)
        .map(|unit| crate::sources::UnitDecision {
            page: 1,
            unit,
            review: crate::sources::UnitReview {
                revision: 1,
                state: "accepted".into(),
                correction: String::new(),
                actor: String::new(),
                rationale: String::new(),
            },
        })
        .collect();
    store
        .material_review_units_atomic(
            auth[0].clone(),
            auth[0].csrf_fixture(),
            decisions,
            "  ".into(),
        )
        .await
        .unwrap();
    store
        .run(|db| {
            let empty: i64 = db.query_row(
                "SELECT count(*) FROM source_review_events WHERE state='accepted' AND rationale=''",
                [],
                |r| r.get(0),
            )?;
            assert_eq!(
                empty, 49,
                "every event of an all-accepted batch stores an empty rationale"
            );
            Ok(())
        })
        .await
        .unwrap();
    let response = document_at(
        &app,
        &auth[0],
        "/admin/materials/pages/1/units/1",
        accept_form(1, " \n "),
    )
    .await;
    assert_eq!(
        location(&response),
        "/admin/materials/pages/1?units=50&saved=accepted&saved_unit=1&to=51&then=unit#unit-51"
    );
    store
        .run(|db| {
            let rationale: String = db.query_row(
                "SELECT rationale FROM source_review_events WHERE unit_id=1",
                [],
                |r| r.get(0),
            )?;
            assert_eq!(
                rationale, "",
                "whitespace-only accepted rationale is stored empty"
            );
            Ok(())
        })
        .await
        .unwrap();
    let html = body(router_get(&app, &auth[0], &location(&response)).await).await;
    assert!(html.contains("No rationale given."));
}

#[tokio::test]
async fn material_save_moves_to_the_next_page_with_a_true_notice() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    accept_all_but_first(&store, &auth[0]).await;
    store
        .run(|db| {
            db.execute("INSERT INTO source_pages(id,source_id,pdf_page,png) VALUES(2,1,2,x'89504e47')", [])?;
            db.execute("INSERT INTO source_units(id,page_id,block_id,kind,polygon,raw_html,raw_text) VALUES(100,2,'p2','Text','[]','<p>Two</p>','Two')", [])?;
            Ok(())
        })
        .await
        .unwrap();
    let app = crate::auth::app(store.clone()).await.unwrap();
    let response = document_at(
        &app,
        &auth[0],
        "/admin/materials/pages/1/units/1",
        accept_form(1, ""),
    )
    .await;
    let target = location(&response);
    assert_eq!(
        target,
        "/admin/materials/pages/2?saved=accepted&saved_unit=1&then=page#saved-notice"
    );
    let html = body(router_get(&app, &auth[0], &target).await).await;
    assert!(html.contains("Saved unit 1: accepted as extracted."));
    assert!(html.contains("this is the next page with units to review"));
    // The same link is false once its unit is not in that state.
    let false_link = "/admin/materials/pages/2?saved=rejected&saved_unit=1&then=page";
    assert!(
        !body(router_get(&app, &auth[0], false_link).await)
            .await
            .contains("Saved unit")
    );
}
#[tokio::test]
async fn material_mixed_batch_needs_a_rationale() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    let decision = |unit, state: &str, correction: &str| crate::sources::UnitDecision {
        page: 1,
        unit,
        review: crate::sources::UnitReview {
            revision: 1,
            state: state.into(),
            correction: correction.into(),
            actor: String::new(),
            rationale: String::new(),
        },
    };
    let result = store
        .material_review_units_atomic(
            auth[0].clone(),
            auth[0].csrf_fixture(),
            vec![
                decision(1, "accepted", ""),
                decision(2, "corrected", "Fixed"),
            ],
            " ".into(),
        )
        .await;
    assert!(matches!(result, Err(crate::AppError::Invalid(_))));
}

// ---- Owner tables, refused-save errors and per-decision fields (review-screen usability) ----

const BOX1: &str = "[[10,10],[50,10],[50,100],[10,100]]";
const BOX2: &str = "[[60,20],[120,20],[120,90],[60,90]]";

async fn table_fixture() -> (tempfile::TempDir, Store, Vec<RequestAuth>) {
    table_fixture_with("").await
}

/// Units 1 and 2 get real boxes; `extra` is SQL run before the source is finalized (raw rows are
/// immutable afterwards).
async fn table_fixture_with(extra: &'static str) -> (tempfile::TempDir, Store, Vec<RequestAuth>) {
    let (dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    store
        .run(move |db| {
            db.execute_batch(extra)?;
            for (id, polygon) in [(1, BOX1), (2, BOX2)] {
                db.execute(
                    "UPDATE source_units SET polygon=?1 WHERE id=?2",
                    params![polygon, id],
                )?;
            }
            Ok(())
        })
        .await
        .unwrap();
    finalize_material(&store).await;
    (dir, store, auth)
}

fn grid(rows: &[&[&str]]) -> Vec<Vec<String>> {
    rows.iter()
        .map(|row| row.iter().map(|c| (*c).to_owned()).collect())
        .collect()
}

fn table_of(units: &[(i64, i64)], cells: Vec<Vec<String>>) -> crate::sources::OwnerTable {
    crate::sources::OwnerTable {
        revision: 1,
        sources: units
            .iter()
            .map(|(unit, revision)| crate::sources::TableSource {
                unit: *unit,
                revision: *revision,
            })
            .collect(),
        cells,
        header: false,
        actor: "forged".into(),
        rationale: "Columns split by the extractor".into(),
    }
}

async fn counts(store: &Store) -> (i64, i64, i64) {
    store
        .run(|db| {
            Ok((
                db.query_row("SELECT count(*) FROM source_units", [], |r| r.get(0))?,
                db.query_row("SELECT count(*) FROM source_review_events", [], |r| {
                    r.get(0)
                })?,
                db.query_row("SELECT revision FROM source_pages WHERE id=1", [], |r| {
                    r.get(0)
                })?,
            ))
        })
        .await
        .unwrap()
}

#[tokio::test]
async fn material_table_creation_unions_boxes_escapes_cells_and_rejects_originals_atomically() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) = table_fixture().await;
    let raw_before: Vec<String> = store
        .run(|db| {
            Ok(db
                .prepare("SELECT raw_html FROM source_units WHERE id IN (1,2) ORDER BY id")?
                .query_map([], |r| r.get(0))?
                .collect::<Result<_, _>>()?)
        })
        .await
        .unwrap();
    let (unit, events) = store
        .material_create_table(
            auth[0].clone(),
            auth[0].csrf_fixture(),
            1,
            table_of(
                &[(1, 1), (2, 1)],
                grid(&[
                    &["Name", "<script>bad()</script>"],
                    &["Sword", "d6 & \"d8\""],
                ]),
            ),
        )
        .await
        .unwrap();
    assert_eq!(events.len(), 2);
    store
        .run(move |db| {
            let (kind, origin, block, state, revision, polygon, html): (String, String, String, String, i64, String, String) = db.query_row(
                "SELECT kind,origin,block_id,state,revision,polygon,raw_html FROM source_units WHERE id=?1",
                [unit],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)),
            )?;
            assert_eq!((kind.as_str(), origin.as_str(), block.as_str()), ("Table", "owner", "owner:1"));
            assert_eq!((state.as_str(), revision), ("unreviewed", 1));
            let polygon: Vec<[f64; 2]> = serde_json::from_str(&polygon).unwrap();
            assert_eq!(polygon, [[10.0, 10.0], [120.0, 10.0], [120.0, 100.0], [10.0, 100.0]]);
            assert_eq!(
                html,
                "<table><tbody><tr><td>Name</td><td>&lt;script&gt;bad()&lt;/script&gt;</td></tr><tr><td>Sword</td><td>d6 &amp; &quot;d8&quot;</td></tr></tbody></table>"
            );
            assert!(!crate::sources::sanitized(&html).unwrap().contains("<script"));
            // Search text exists and carries the cell text, not markup.
            let search: String = db.query_row("SELECT search_text FROM source_search_text WHERE unit_id=?1", [unit], |r| r.get(0))?;
            assert!(search.contains("Sword") && !search.contains("<table"));
            // Originals: rejected in one batch with the rationale; raw extraction untouched.
            let rejected: i64 = db.query_row(
                "SELECT count(*) FROM source_units WHERE id IN (1,2) AND state='rejected' AND revision=2",
                [],
                |r| r.get(0),
            )?;
            assert_eq!(rejected, 2);
            let batch: i64 = db.query_row(
                "SELECT count(DISTINCT operation_id) FROM source_review_events WHERE operation_id LIKE 'batch-%' AND state='rejected' AND rationale='Columns split by the extractor' AND actor='account:1'",
                [],
                |r| r.get(0),
            )?;
            assert_eq!(batch, 1);
            let audited: i64 = db.query_row(
                "SELECT count(*) FROM source_web_events w JOIN source_review_events e ON e.id=w.event_id WHERE e.operation_id IS NOT NULL",
                [],
                |r| r.get(0),
            )?;
            assert_eq!(audited, 2);
            let raw: Vec<String> = db
                .prepare("SELECT raw_html FROM source_units WHERE id IN (1,2) ORDER BY id")?
                .query_map([], |r| r.get(0))?
                .collect::<Result<_, _>>()?;
            assert_eq!(raw, raw_before);
            let page: (i64, i64) = db.query_row("SELECT complete,revision FROM source_pages WHERE id=1", [], |r| Ok((r.get(0)?, r.get(1)?)))?;
            assert_eq!(page.0, 0);
            let (raw_text, links): (String, i64) = db.query_row("SELECT u.raw_text,(SELECT count(*) FROM source_owner_tables t WHERE t.unit_id=u.id) FROM source_units u WHERE u.id=?1", [unit], |r| Ok((r.get(0)?, r.get(1)?)))?;
            assert_eq!(raw_text, "Name | <script>bad()</script>\nSword | d6 & \"d8\"");
            assert_eq!(links, 1);
            assert!(db.execute("UPDATE source_owner_tables SET operation_id='x'", []).is_err());
            assert!(db.execute("DELETE FROM source_owner_tables", []).is_err());
            Ok(())
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn material_table_stale_unit_page_or_fault_rolls_back_everything() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) = table_fixture().await;
    let before = counts(&store).await;
    let cells = || grid(&[&["a", "b"]]);
    let mut stale_page = table_of(&[(1, 1), (2, 1)], cells());
    stale_page.revision = 0;
    for (table, expected) in [
        (table_of(&[(1, 1), (2, 9)], cells()), "Conflict"),
        (stale_page, "Conflict"),
    ] {
        let error = store
            .material_create_table(auth[0].clone(), auth[0].csrf_fixture(), 1, table)
            .await
            .unwrap_err();
        assert!(matches!(error, AppError::Conflict), "{expected}: {error:?}");
        assert_eq!(counts(&store).await, before);
    }
    // The second audit row fails: nothing of the first rejection, nor the table unit, stays.
    store
        .run(|db| {
            db.execute_batch("CREATE TRIGGER table_audit_fault BEFORE INSERT ON source_web_events WHEN (SELECT count(*) FROM source_web_events)>=1 BEGIN SELECT RAISE(ABORT,'fixture fault'); END;")?;
            Ok(())
        })
        .await
        .unwrap();
    assert!(
        store
            .material_create_table(
                auth[0].clone(),
                auth[0].csrf_fixture(),
                1,
                table_of(&[(1, 1), (2, 1)], cells())
            )
            .await
            .is_err()
    );
    assert_eq!(counts(&store).await, before);
}

#[tokio::test]
async fn material_table_bounds_page_boundary_and_identity_are_enforced() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) = table_fixture_with(
        "UPDATE source_units SET polygon='[[150,200],[400,200],[400,500],[150,500]]' WHERE id=4; UPDATE source_units SET polygon='[[5,5],[5,5],[5,5],[5,5]]' WHERE id=5;",
    )
    .await;
    let before = counts(&store).await;
    let create = |table| {
        let store = store.clone();
        let auth = auth[0].clone();
        async move {
            store
                .material_create_table(auth.clone(), auth.csrf_fixture(), 1, table)
                .await
        }
    };
    let wide: Vec<Vec<String>> = vec![vec!["x".into(); 17]];
    let tall: Vec<Vec<String>> = vec![vec!["x".into()]; 201];
    let long = grid(&[&[&"x".repeat(1001)]]);
    for cells in [
        wide,
        tall,
        long,
        grid(&[&["", ""]]),
        grid(&[&["a", "b"], &["c"]]),
        grid(&[&["a\0"]]),
        vec![],
    ] {
        let error = create(table_of(&[(1, 1)], cells)).await.unwrap_err();
        assert!(matches!(error, AppError::Invalid(_)), "{error:?}");
    }
    let mut blank = table_of(&[(1, 1)], grid(&[&["a"]]));
    blank.rationale = "  ".into();
    assert!(matches!(create(blank).await, Err(AppError::Invalid(_))));
    let many: Vec<(i64, i64)> = (1..=17).map(|u| (u, 1)).collect();
    assert!(matches!(
        create(table_of(&many, grid(&[&["a"]]))).await,
        Err(AppError::Invalid(_))
    ));
    assert!(matches!(
        create(table_of(&[], grid(&[&["a"]]))).await,
        Err(AppError::Invalid(_))
    ));
    assert!(matches!(
        create(table_of(&[(1, 1), (1, 1)], grid(&[&["a"]]))).await,
        Err(AppError::Invalid(_))
    ));
    assert!(matches!(
        create(table_of(&[(999, 1)], grid(&[&["a"]]))).await,
        Err(AppError::SourceNotFound)
    ));
    // A unit without a usable region cannot define a box.
    assert!(matches!(
        create(table_of(&[(3, 1)], grid(&[&["a"]]))).await,
        Err(AppError::Invalid(_))
    ));
    assert_eq!(counts(&store).await, before);
    // Units reaching past the page are clamped to it (unit 4); a box with no area is refused (unit 5).
    assert!(matches!(
        create(table_of(&[(5, 1)], grid(&[&["a"]]))).await,
        Err(AppError::Invalid(_))
    ));
    let (unit, _) = create(table_of(&[(4, 1)], grid(&[&["a"]]))).await.unwrap();
    let polygon: String = store
        .run(move |db| {
            Ok(db.query_row(
                "SELECT polygon FROM source_units WHERE id=?1",
                [unit],
                |r| r.get(0),
            )?)
        })
        .await
        .unwrap();
    let polygon: Vec<[f64; 2]> = serde_json::from_str(&polygon).unwrap();
    assert_eq!(
        polygon,
        [
            [150.0, 200.0],
            [200.0, 200.0],
            [200.0, 300.0],
            [150.0, 300.0]
        ]
    );
}

#[tokio::test]
async fn material_migration_028_allows_only_text_and_table_owner_units() {
    let _lock = crate::auth::test_resource_lock().await;
    let (dir, store, _auth) = table_fixture().await;
    let insert = |kind: &'static str, id: i64, state: &'static str, revision: i64| {
        let store = store.clone();
        async move {
            store
                .run(move |db| {
                    Ok(db.execute(
                        "INSERT INTO source_units(page_id,block_id,kind,polygon,raw_html,raw_text,origin,state,revision) VALUES(1,?1,?2,'[[1,1],[20,1],[20,20],[1,20]]','<p>x</p>','x','owner',?3,?4)",
                        params![format!("owner:{id}"), kind, state, revision],
                    ).map_err(|e| e.to_string()))
                })
                .await
                .unwrap()
        }
    };
    assert!(insert("Table", 1, "unreviewed", 1).await.is_ok());
    assert!(insert("Text", 2, "unreviewed", 1).await.is_ok());
    for (kind, state, revision) in [
        ("Form", "unreviewed", 1),
        ("Caption", "unreviewed", 1),
        ("Table", "corrected", 1),
        ("Table", "unreviewed", 2),
    ] {
        let error = insert(kind, 3, state, revision).await.unwrap_err();
        assert!(
            error.contains("Invalid owner unit identity"),
            "{kind} {state} {revision}: {error}"
        );
    }
    // Extracted units cannot use the owner namespace, and a Table cannot appear on an unfinalized page.
    let error = store
        .run(|db| Ok(db.execute("INSERT INTO source_units(page_id,block_id,kind,polygon,raw_html,raw_text) VALUES(1,'owner:9','Table','[]','<p>x</p>','x')", []).map_err(|e| e.to_string())))
        .await
        .unwrap()
        .unwrap_err();
    assert!(error.contains("Invalid owner unit identity"), "{error}");
    // Upgrade path: a database at schema 27 (Text only) gains Table when reopened.
    let path = dir.path().join("private.sqlite3");
    store
        .run(|db| {
            let old = include_str!("../../migrations/024_source_documents.sql");
            let start = old
                .find("CREATE TRIGGER source_unit_origin_insert")
                .unwrap();
            let end = old[start..].find("END;").unwrap() + start + 4;
            db.execute_batch(&format!(
                "DROP TABLE source_owner_tables; DROP TRIGGER source_unit_origin_insert; {}",
                &old[start..end]
            ))?;
            db.execute_batch("PRAGMA user_version=27;")?;
            Ok(())
        })
        .await
        .unwrap();
    assert!(insert("Table", 4, "unreviewed", 1).await.is_err());
    let upgraded = Store::open(&path).unwrap();
    let ok = upgraded
        .run(|db| {
            assert_eq!(db.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))?, crate::SCHEMA_VERSION);
            Ok(db.execute("INSERT INTO source_units(page_id,block_id,kind,polygon,raw_html,raw_text,origin) VALUES(1,'owner:4','Table','[[1,1],[20,1],[20,20],[1,20]]','<p>x</p>','x','owner')", []).is_ok())
        })
        .await
        .unwrap();
    assert!(ok);
}

#[tokio::test]
async fn material_table_form_prefills_columns_and_saves_through_the_router() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) = table_fixture_with(
        "UPDATE source_units SET raw_text='Name'||char(10)||'Sword'||char(10)||char(10)||'Bow' WHERE id=1; UPDATE source_units SET raw_text='Damage | a\\b'||char(10)||'d6' WHERE id=2;",
    )
    .await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    // The page offers a checkbox per card that targets the selection form.
    let page = body(router_get(&app, &auth[0], "/admin/materials/pages/1").await).await;
    let parsed = scraper::Html::parse_document(&page);
    let boxes =
        scraper::Selector::parse("input[type=checkbox][name=unit][form=table-select]").unwrap();
    assert!(parsed.select(&boxes).count() >= 2);
    let form = scraper::Selector::parse("form#table-select").unwrap();
    assert_eq!(
        parsed.select(&form).next().unwrap().value().attr("action"),
        Some("/admin/materials/pages/1/tables/new")
    );
    // The reading-order grid: one column per unit, short columns padded, `|` and `\` escaped.
    let response = router_get(
        &app,
        &auth[0],
        "/admin/materials/pages/1/tables/new?unit=2&unit=1",
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let html = body(response).await;
    assert_eq!(
        field_value(&html, "textarea[name=cells]"),
        "| Name | Damage \\| a\\\\b |\n| Sword | d6 |\n| Bow |  |"
    );
    let hidden =
        scraper::Selector::parse("form[data-form=table] input[type=hidden][name=unit]").unwrap();
    let units: Vec<_> = scraper::Html::parse_document(&html)
        .select(&hidden)
        .map(|e| e.value().attr("value").unwrap().to_owned())
        .collect();
    assert_eq!(units, ["1:1", "2:1"]);
    for bad in ["", "?unit=x", "?unit=1&unit=1", "?other=1"] {
        let response = router_get(
            &app,
            &auth[0],
            &format!("/admin/materials/pages/1/tables/new{bad}"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY, "{bad}");
        assert!(body(response).await.contains("table-error"));
    }
}

#[tokio::test]
async fn material_table_refused_save_shows_error_then_saves_and_renders_the_table() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) = table_fixture().await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    // Refused save: red error, focus target, draft kept.
    let post = |rationale: &'static str, revision: &'static str| {
        let app = app.clone();
        let auth = auth[0].clone();
        async move {
            document_at(
                &app,
                &auth,
                "/admin/materials/pages/1/tables",
                vec![
                    ("csrf".into(), "a".repeat(64)),
                    ("revision".into(), revision.into()),
                    ("unit".into(), "1:1".into()),
                    ("unit".into(), "2:1".into()),
                    ("cells".into(), "a | <b>\r\nc | d".into()),
                    ("rationale".into(), rationale.into()),
                ],
            )
            .await
        }
    };
    let refused = post("", "1").await;
    assert_eq!(refused.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let html = body(refused).await;
    let parsed = scraper::Html::parse_document(&html);
    let alert = scraper::Selector::parse("#table-error[role=alert][data-save-error]").unwrap();
    assert!(parsed.select(&alert).next().is_some(), "{html}");
    let invalid = scraper::Selector::parse("textarea[name=rationale][aria-invalid=true]").unwrap();
    assert!(parsed.select(&invalid).next().is_some());
    assert_eq!(field_value(&html, "textarea[name=cells]"), "a | <b>\nc | d");
    assert_eq!(counts(&store).await.0, 51);
    assert_eq!(post("checked", "0").await.status(), StatusCode::CONFLICT);
    let saved = post("Columns split", "1").await;
    assert_eq!(saved.status(), StatusCode::SEE_OTHER);
    let location = saved.headers()["location"].to_str().unwrap().to_owned();
    assert!(
        location.starts_with("/admin/materials/pages/1?units=") && location.contains("#unit-"),
        "{location}"
    );
    let (kind, html): (String, String) = store
        .run(|db| {
            Ok(db.query_row(
                "SELECT kind,raw_html FROM source_units WHERE origin='owner'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?)
        })
        .await
        .unwrap();
    assert_eq!(kind, "Table");
    assert!(html.contains("<td>&lt;b&gt;</td>"), "{html}");
    // The page renders the new table inside its card.
    let page = body(router_get(&app, &auth[0], &location).await).await;
    assert!(page.contains("<table>") && page.contains("<td>c</td>"));
}

const SHAPE_UNITS: &str = "UPDATE source_units SET raw_text='d6 Result'||char(10)||'1 Rats'||char(10)||'2 Goblins'||char(10)||'3 Ogres' WHERE id=1; UPDATE source_units SET raw_text='4 Wolves'||char(10)||'5 Bandits'||char(10)||'6 A dragon' WHERE id=2; UPDATE source_units SET polygon='[[55,30],[58,30],[58,40],[55,40]]' WHERE id=3; UPDATE source_units SET polygon='[[150,150],[160,150],[160,160],[150,160]]' WHERE id=4;";

async fn table_post(
    app: &axum::Router,
    auth: &RequestAuth,
    fields: Vec<(String, String)>,
) -> axum::response::Response {
    document_at(app, auth, "/admin/materials/pages/1/tables", fields).await
}

fn table_fields(units: &[&str], cells: &str, rationale: &str) -> Vec<(String, String)> {
    let mut fields = vec![
        ("csrf".to_owned(), "a".repeat(64)),
        ("revision".to_owned(), "1".to_owned()),
    ];
    fields.extend(units.iter().map(|u| ("unit".to_owned(), (*u).to_owned())));
    fields.push(("cells".to_owned(), cells.to_owned()));
    fields.push(("rationale".to_owned(), rationale.to_owned()));
    fields
}

#[tokio::test]
async fn material_table_form_reads_shapes_and_the_header_round_trips_to_search() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) = table_fixture_with(SHAPE_UNITS).await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    let get = |query: &'static str| {
        let (app, auth) = (app.clone(), auth[0].clone());
        async move {
            router_get(
                &app,
                &auth,
                &format!("/admin/materials/pages/1/tables/new?{query}"),
            )
            .await
        }
    };
    let response = get("unit=2&unit=1").await;
    assert_eq!(response.status(), StatusCode::OK);
    let html = body(response).await;
    assert_eq!(field_value(&html, "textarea[name=cells]"), HEADED_CELLS);
    let parsed = scraper::Html::parse_document(&html);
    let count = |css: &str| {
        parsed
            .select(&scraper::Selector::parse(css).unwrap())
            .count()
    };
    assert_eq!(
        count("input[type=checkbox][name=header][value='1'][checked]"),
        1
    );
    assert_eq!(count("#table-problems"), 0);
    // The "Read as" form is a separate GET form with only the unit ids and the shape buttons.
    assert_eq!(count("form#table-read-as[method=get]"), 1);
    assert_eq!(count("form#table-read-as input[type=hidden][name=unit]"), 2);
    assert_eq!(
        count(
            "form#table-read-as input[name=csrf], form#table-read-as textarea, form#table-read-as input[name=cells]"
        ),
        0
    );
    assert!(count("form#table-read-as button[name=shape]") >= 2);
    assert_eq!(
        count("form#table-read-as button[name=shape][value=roll][aria-pressed=true]"),
        1
    );
    assert!(html.contains("Roll table (100)"), "{html}");
    // The units whose boxes lie inside the union of the selected boxes are named (unit 3),
    // others are not (unit 4).
    assert!(html.contains("id=\"table-overlap\""), "{html}");
    assert!(
        html.contains("unit 3") && !html.contains("unit 4,") && !html.contains("unit 4 "),
        "{html}"
    );
    // The same units read as plain columns: one column per unit, no header.
    let plain = body(get("unit=1&unit=2&shape=plain").await).await;
    assert_eq!(
        field_value(&plain, "textarea[name=cells]")
            .lines()
            .next()
            .unwrap(),
        "| d6 Result | 4 Wolves |"
    );
    assert!(
        scraper::Html::parse_document(&plain)
            .select(&scraper::Selector::parse("input[name=header][checked]").unwrap())
            .next()
            .is_none()
    );
    // Unknown, repeated or extra parameters are refused.
    for bad in [
        "unit=1&shape=nope",
        "unit=1&shape=roll&shape=plain",
        "unit=1&csrf=x",
        "unit=1&cells=x",
    ] {
        assert_eq!(
            get(bad).await.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "{bad}"
        );
    }
}

const HEADED_CELLS: &str = "| d6 | Result |\n| 1 | Rats |\n| 2 | Goblins |\n| 3 | Ogres |\n| 4 | Wolves |\n| 5 | Bandits |\n| 6 | A dragon |";

#[tokio::test]
async fn material_table_header_saves_thead_and_round_trips_through_sanitiser_and_search() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) = table_fixture().await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    // Saving with the header ticked stores thead/th; the sanitiser keeps them and the search text
    // and the stored plain text have the same one-line-per-row form.
    let saved = table_post(&app, &auth[0], {
        let mut fields = table_fields(&["1:1", "2:1"], HEADED_CELLS, "checked");
        fields.push(("header".into(), "1".into()));
        fields
    })
    .await;
    assert_eq!(saved.status(), StatusCode::SEE_OTHER);
    let location = saved.headers()["location"].to_str().unwrap().to_owned();
    let (html, raw_text, search): (String, String, String) = store
        .run(|db| {
            Ok(db.query_row(
                "SELECT u.raw_html,u.raw_text,t.search_text FROM source_units u JOIN source_search_text t ON t.unit_id=u.id WHERE u.origin='owner'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )?)
        })
        .await
        .unwrap();
    assert!(
        html.starts_with("<table><thead><tr><th>d6</th><th>Result</th></tr></thead><tbody><tr><td>1</td><td>Rats</td></tr>"),
        "{html}"
    );
    let clean = crate::sources::sanitized(&html).unwrap();
    assert_eq!(clean, html, "thead and th survive the sanitiser");
    let plain_text = crate::sources::tables::plain(
        &crate::sources::tables::parse_grid(HEADED_CELLS, true).unwrap(),
    );
    assert_eq!(raw_text, plain_text);
    assert_eq!(search.trim_end(), plain_text);
    let page = body(router_get(&app, &auth[0], &location).await).await;
    assert!(
        page.contains("<th>d6</th>"),
        "the card shows the header cells"
    );
}

#[tokio::test]
async fn material_table_without_the_header_box_stores_plain_cells() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) = table_fixture().await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    let saved = table_post(
        &app,
        &auth[0],
        table_fields(&["1:1"], "a | b\nc | d", "checked"),
    )
    .await;
    assert_eq!(saved.status(), StatusCode::SEE_OTHER);
    let html: String = store
        .run(|db| {
            Ok(db.query_row(
                "SELECT raw_html FROM source_units WHERE origin='owner'",
                [],
                |r| r.get(0),
            )?)
        })
        .await
        .unwrap();
    assert!(
        !html.contains("<th>") && !html.contains("<thead>"),
        "{html}"
    );
    // Any other value of the header field is a malformed form.
    let mut fields = table_fields(&["1:1"], "a | b", "checked");
    fields.push(("header".into(), "yes".into()));
    assert_eq!(
        table_post(&app, &auth[0], fields).await.status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
}

const TWENTY_UNITS: &str = "UPDATE source_units SET polygon='[[10,10],[50,10],[50,100],[10,100]]', raw_text=id||' Result '||id WHERE id<=20;";

#[tokio::test]
async fn material_table_saves_a_twenty_unit_roll_table() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) = table_fixture_with(TWENTY_UNITS).await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    let query: Vec<String> = (1..=20).map(|u| format!("unit={u}")).collect();
    let html = body(
        router_get(
            &app,
            &auth[0],
            &format!("/admin/materials/pages/1/tables/new?{}", query.join("&")),
        )
        .await,
    )
    .await;
    let cells = field_value(&html, "textarea[name=cells]");
    assert_eq!(cells.lines().count(), 20);
    assert_eq!(cells.lines().nth(19).unwrap(), "| 20 | Result 20 |");
    assert!(html.contains("Roll table (100)"));
    let units: Vec<String> = (1..=20).map(|u| format!("{u}:1")).collect();
    let refs: Vec<&str> = units.iter().map(String::as_str).collect();
    let before = counts(&store).await;
    let saved = table_post(&app, &auth[0], table_fields(&refs, &cells, "checked")).await;
    assert_eq!(saved.status(), StatusCode::SEE_OTHER);
    let after = counts(&store).await;
    assert_eq!((after.0, after.1), (before.0 + 1, before.1 + 20));
}

const TWO_HUNDRED_UNITS: &str = "WITH RECURSIVE n(i) AS (SELECT 52 UNION ALL SELECT i+1 FROM n WHERE i<200) INSERT INTO source_units(id,page_id,block_id,kind,polygon,raw_html,raw_text) SELECT i,1,'unit-'||i,'Text','[]','<p>x</p>','x' FROM n; UPDATE source_units SET raw_text='Item '||id||': gold '||id WHERE id<=200;";

#[tokio::test]
async fn material_table_prefill_reads_two_hundred_units_and_refuses_plain_above_sixteen() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) = table_fixture_with(TWO_HUNDRED_UNITS).await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    let query: Vec<String> = (1..=200).map(|u| format!("unit={u}")).collect();
    let response = router_get(
        &app,
        &auth[0],
        &format!("/admin/materials/pages/1/tables/new?{}", query.join("&")),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let html = body(response).await;
    let cells = field_value(&html, "textarea[name=cells]");
    assert_eq!(cells.lines().count(), 200);
    assert_eq!(cells.lines().next().unwrap(), "| Item 1 | gold 1 |");
    assert!(
        !html.contains("Plain columns"),
        "plain columns is not offered above 16 units"
    );
    // Asking for plain columns above 16 units says why there is no draft.
    let seventeen: Vec<String> = (1..=17).map(|u| format!("unit={u}")).collect();
    let plain = body(
        router_get(
            &app,
            &auth[0],
            &format!(
                "/admin/materials/pages/1/tables/new?{}&shape=plain",
                seventeen.join("&")
            ),
        )
        .await,
    )
    .await;
    assert!(plain.contains("More than 16 columns"), "{plain}");
    assert_eq!(field_value(&plain, "textarea[name=cells]").trim(), "");
}

#[tokio::test]
async fn material_table_post_accepts_the_largest_non_ascii_grid_within_the_body_limit() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) = table_fixture().await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    // 19 rows of 1,000 three-byte characters: just under the 20,000 character bound, the
    // longest rationale, and every byte percent-encoded: about 230 KB of the 320 KiB limit.
    let cells = vec![format!("| {} |", "\u{8a9e}".repeat(1_000)); 19].join("\n");
    let rationale = "\u{8a9e}".repeat(5_000);
    let fields = table_fields(&["1:1"], &cells, &rationale);
    let encoded = serde_urlencoded::to_string(&fields).unwrap();
    assert!(
        encoded.len() < 320 * 1024 && encoded.len() > 200 * 1024,
        "{}",
        encoded.len()
    );
    let saved = table_post(&app, &auth[0], fields).await;
    assert_eq!(saved.status(), StatusCode::SEE_OTHER);
    // A body over the limit is refused as a whole, with the too-large status.
    let before = counts(&store).await;
    let huge = table_fields(&["1:1"], &"x".repeat(330 * 1024), "r");
    let refused = table_post(&app, &auth[0], huge).await;
    assert_eq!(refused.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(counts(&store).await, before);
}

#[tokio::test]
async fn material_refused_unit_save_shows_error_beside_unit_summary_and_focus_target() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    let mut form = decision(1);
    form[3].1 = "fixed".into();
    form[4].1 = String::new();
    let response = document_post(&app, &auth[0], form).await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let html = body(response).await;
    let parsed = scraper::Html::parse_document(&html);
    let select = |css: &str| scraper::Selector::parse(css).unwrap();
    let error = parsed
        .select(&select(
            "#unit-1 #unit-1-error[role=alert][data-save-error]",
        ))
        .next()
        .expect("error inside unit 1");
    assert!(error.text().collect::<String>().contains("not saved"));
    assert!(
        parsed
            .select(&select(".error-summary a[href='#unit-1-error']"))
            .next()
            .is_some()
    );
    assert!(html.contains("A save failed"));
    assert!(
        parsed
            .select(&select(
                "#unit-1 textarea[name=rationale][aria-invalid=true]"
            ))
            .next()
            .is_some()
    );
    assert!(
        parsed
            .select(&select(
                "#unit-1 textarea[name=correction][aria-invalid=true]"
            ))
            .next()
            .is_none()
    );
    // Only the unit that failed carries an error.
    assert!(
        parsed
            .select(&select("#unit-2 [data-save-error]"))
            .next()
            .is_none()
    );
    let css = include_str!("../../assets/app.css");
    assert!(css.contains(".field-error") && css.contains("aria-invalid"));
    assert!(include_str!("../../assets/source-review.js").contains("[data-save-error]"));
}

#[tokio::test]
async fn material_unit_form_shows_only_the_fields_each_decision_needs() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    let html = body(router_get(&app, &auth[0], "/admin/materials/pages/1").await).await;
    let parsed = scraper::Html::parse_document(&html);
    let select = |css: &str| scraper::Selector::parse(css).unwrap();
    let form = parsed
        .select(&select("#unit-1 form[data-form=unit]"))
        .next()
        .unwrap();
    // One dropdown, on the heading row, tied to the form by id; no radios.
    assert!(
        form.select(&select("input[type=radio]")).next().is_none(),
        "no radios any more"
    );
    let form_id = form.value().attr("id").expect("the form has an id");
    let dropdown = parsed
        .select(&select("#unit-1 .unit-head select[name=state]"))
        .next()
        .expect("the decision dropdown is on the heading row");
    assert_eq!(dropdown.value().attr("form"), Some(form_id));
    assert!(dropdown.value().attr("required").is_some());
    let values: Vec<_> = dropdown
        .select(&select("option"))
        .map(|r| r.value().attr("value").unwrap())
        .collect();
    assert_eq!(values, ["", "accepted", "corrected", "rejected"]);
    assert_eq!(parsed.select(&select("#unit-1 select")).count(), 1);
    // Accepted has no field of its own: both text fields live in containers that only
    // `corrected` / `rejected` reveal, and the rationale label says SRC-4 requires it.
    assert!(
        form.select(&select(
            "[data-decision-fields=corrected] textarea[name=correction]"
        ))
        .next()
        .is_some()
    );
    assert!(
        form.select(&select(
            "[data-decision-fields~=rejected] textarea[name=rationale]"
        ))
        .next()
        .is_some()
    );
    assert_eq!(form.select(&select("textarea")).count(), 2);
    assert!(form.html().contains("SRC-4"));
    assert!(
        !form.select(&select("textarea[required]")).any(|_| true),
        "hidden fields must not block submit"
    );
    // Without script every field shows (nothing is hidden by the sheet); the script hides the
    // fields the chosen decision does not use.
    let css = include_str!("../../assets/app.css");
    assert!(!css.contains(".decision-fields { display: none; }"));
    assert!(!css.contains(":checked ~ .fields-"));
    let js = include_str!("../../assets/source-review.js");
    assert!(js.contains("data-decision-fields") && js.contains("select[name=state]"));
}

#[tokio::test]
async fn material_unused_decision_fields_are_ignored_and_a_corrected_unit_preselects() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    let select = |css: &str| scraper::Selector::parse(css).unwrap();
    // Fields that the chosen decision does not use are ignored, not refused or stored.
    for (state, unit) in [("accepted", 1), ("rejected", 2)] {
        let response = document_at(
            &app,
            &auth[0],
            &format!("/admin/materials/pages/1/units/{unit}"),
            review_form(&[
                ("revision", "1"),
                ("state", state),
                ("correction", "stale hidden text"),
                ("rationale", "stale hidden rationale"),
            ]),
        )
        .await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER, "{state}");
    }
    let stored: Vec<(String, String, String)> = store
        .run(|db| {
            Ok(db
                .prepare(
                    "SELECT state,correction,rationale FROM source_review_events ORDER BY unit_id",
                )?
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect::<Result<_, _>>()?)
        })
        .await
        .unwrap();
    assert_eq!(
        stored,
        [
            ("accepted".into(), String::new(), String::new()),
            (
                "rejected".into(),
                String::new(),
                "stale hidden rationale".into()
            )
        ]
    );
    // A corrected unit pre-selects its option on the next view.
    let response = document_at(
        &app,
        &auth[0],
        "/admin/materials/pages/1/units/3",
        review_form(&[
            ("revision", "1"),
            ("state", "corrected"),
            ("correction", "fixed"),
            ("rationale", "checked"),
        ]),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let html = body(router_get(&app, &auth[0], "/admin/materials/pages/1?units=2").await).await;
    assert!(
        scraper::Html::parse_document(&html)
            .select(&select("#unit-3 option[value=corrected][selected]"))
            .next()
            .is_some()
    );
}

const PAGE_TWO: &str = "INSERT INTO source_pages(id,source_id,pdf_page,png) VALUES(2,1,2,x'89504e47'); INSERT INTO source_units(id,page_id,block_id,kind,polygon,raw_html,raw_text) VALUES(900,2,'p2','Text','[[1,1],[5,1],[5,5],[1,5]]','<p>x</p>','x');";

#[tokio::test]
async fn material_table_history_notice_reselect_and_accepted_lookup_end_to_end() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) = table_fixture().await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    let (unit, _) = store
        .material_create_table(
            auth[0].clone(),
            auth[0].csrf_fixture(),
            1,
            table_of(
                &[(1, 1), (2, 1)],
                grid(&[&["Name", "Damage"], &["Sword", "d6"]]),
            ),
        )
        .await
        .unwrap();
    let page = body(router_get(&app, &auth[0], "/admin/materials/pages/1").await).await;
    assert!(
        page.contains(&format!("Table unit {unit} created from units 1, 2.")),
        "{page}"
    );
    // The confirmation appears only for a true link.
    let link = format!("/admin/materials/pages/1?units=50&table={unit}");
    assert!(
        body(router_get(&app, &auth[0], &link).await)
            .await
            .contains(&format!("Created table unit {unit}"))
    );
    assert!(
        !body(router_get(&app, &auth[0], "/admin/materials/pages/1?table=1").await)
            .await
            .contains("Created table unit")
    );
    // Re-selecting the table prefills plain rows, never box drawing.
    let again = body(
        router_get(
            &app,
            &auth[0],
            &format!("/admin/materials/pages/1/tables/new?unit={unit}"),
        )
        .await,
    )
    .await;
    assert_eq!(
        field_value(&again, "textarea[name=cells]"),
        "| Name \\| Damage |\n| Sword \\| d6 |"
    );
    // Accepting makes the table evidence: lookup text and the search index carry its cells.
    store
        .material_review_unit(
            auth[0].clone(),
            auth[0].csrf_fixture(),
            1,
            unit,
            crate::sources::UnitReview {
                revision: 1,
                state: "accepted".into(),
                correction: String::new(),
                actor: String::new(),
                rationale: String::new(),
            },
        )
        .await
        .unwrap();
    let page = body(router_get(&app, &auth[0], "/admin/materials/pages/1?units=50").await).await;
    let lookup = field_value(&page, &format!("pre[data-lookup-unit='{unit}']"));
    assert!(
        lookup.contains("Sword") && lookup.contains("d6"),
        "{lookup}"
    );
    let hits: i64 = store
        .run(|db| {
            Ok(db.query_row(
                "SELECT count(*) FROM source_search_fts WHERE source_search_fts MATCH 'Sword'",
                [],
                |r| r.get(0),
            )?)
        })
        .await
        .unwrap();
    assert!(hits >= 1);
}

#[tokio::test]
async fn material_table_refuses_other_page_units_and_reports_bounds() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    store
        .run(|db| Ok(db.execute_batch(PAGE_TWO)?))
        .await
        .unwrap();
    // Finalize as `finalize_material` does, with the second page's hash after the first's.
    store
        .run(|db| {
            let prepared = serde_json::json!({"contract_version":1,"extraction_sha256":"b".repeat(64),"pages":[{"pdf_page":1,"width_points":200,"height_points":300},{"pdf_page":2,"width_points":200,"height_points":300}]});
            db.execute("UPDATE sources SET bundle_hash=?1,prepared_json=?2 WHERE id=1", params!["a".repeat(64), serde_json::to_vec(&prepared)?])?;
            db.execute("INSERT INTO source_page_hashes VALUES(1,1,?1)", ["c".repeat(64)])?;
            db.execute("INSERT INTO source_page_hashes VALUES(2,1,?1)", ["d".repeat(64)])?;
            db.execute("INSERT INTO source_finalizations VALUES(1,?1,?2)", params!["a".repeat(64), "b".repeat(64)])?;
            crate::sources::documents::register_import(db, 1)?;
            Ok(())
        })
        .await
        .unwrap();
    let app = crate::auth::app(store.clone()).await.unwrap();
    // A unit of another page of the same source is refused on GET and POST.
    let response = router_get(
        &app,
        &auth[0],
        "/admin/materials/pages/1/tables/new?unit=1&unit=900",
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let response = document_at(
        &app,
        &auth[0],
        "/admin/materials/pages/1/tables",
        review_form(&[
            ("revision", "1"),
            ("unit", "900:1"),
            ("cells", "a"),
            ("rationale", "r"),
        ]),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let before = counts(&store).await;
    // More than 200 units on GET.
    let many: String = (1..=201)
        .map(|u| format!("unit={u}"))
        .collect::<Vec<_>>()
        .join("&");
    let response = router_get(
        &app,
        &auth[0],
        &format!("/admin/materials/pages/1/tables/new?{many}"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body(response).await.contains("Choose 1-200"));
    // Stored-size bounds: 64 KiB of HTML and 20,000 characters of text.
    let tall_text: Vec<Vec<String>> = vec![vec!["x".repeat(1000)]; 21];
    assert!(matches!(
        store
            .material_create_table(
                auth[0].clone(),
                auth[0].csrf_fixture(),
                1,
                table_of(&[(1, 1)], tall_text)
            )
            .await,
        Err(AppError::Invalid(_))
    ));
    let big_html: Vec<Vec<String>> = vec![vec!["&".repeat(1000); 14]; 4]; // 56 cells * 5000 bytes
    assert!(crate::sources::tables::html(&big_html, false).len() > 65_536);
    assert!(matches!(
        store
            .material_create_table(
                auth[0].clone(),
                auth[0].csrf_fixture(),
                1,
                table_of(&[(1, 1)], big_html)
            )
            .await,
        Err(AppError::Invalid(_))
    ));
    assert_eq!(counts(&store).await, before);
}

// ---- Review flow: create-table bar, re-decided units, review list, sticky image, next page ----

fn css(selector: &str) -> scraper::Selector {
    scraper::Selector::parse(selector).unwrap()
}

#[tokio::test]
async fn material_create_table_bar_and_no_script_buttons() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    let html = body(router_get(&app, &auth[0], "/admin/materials/pages/1").await).await;
    let parsed = scraper::Html::parse_document(&html);
    // The bar lives in the selection form, is hidden until the script shows it, and has a count,
    // a submit button and a clear button.
    let bar = parsed
        .select(&css("form#table-select #table-bar"))
        .next()
        .expect("table bar inside the selection form");
    assert!(bar.value().attr("hidden").is_some());
    assert!(bar.select(&css("[data-selected-count]")).next().is_some());
    let submit: String = bar
        .select(&css("button[type=submit]"))
        .next()
        .unwrap()
        .text()
        .collect();
    assert_eq!(submit.trim(), "Create table");
    let clear: String = bar
        .select(&css("button[type=button][data-clear-selection]"))
        .next()
        .unwrap()
        .text()
        .collect();
    assert_eq!(clear.trim(), "Clear");
    // Without script: today's button above the list plus one after the card list.
    let buttons = parsed
        .select(&css("[data-no-script-table]"))
        .filter(|b| {
            b.text()
                .collect::<String>()
                .contains("Create table from selected units")
        })
        .count();
    assert_eq!(buttons, 2, "one before and one after the cards");
    let after = parsed.select(&css("button[form=table-select]")).count();
    assert!(after >= 1, "the trailing button targets the selection form");
    // Each card says what the tick does.
    assert!(
        parsed
            .select(&css("#unit-1 .select-unit .tick-hint"))
            .next()
            .is_some()
    );
    // The GET contract is unchanged.
    let form = parsed.select(&css("form#table-select")).next().unwrap();
    assert_eq!(form.value().attr("method"), Some("get"));
    assert_eq!(
        form.value().attr("action"),
        Some("/admin/materials/pages/1/tables/new")
    );
    let js = include_str!("../../assets/source-review.js");
    assert!(js.contains("table-bar") && js.contains("units selected"));
    assert!(js.contains("has-table-bar") && js.contains("data-selected-announce"));
    assert!(
        parsed
            .select(&css(
                "form#table-select p.sr-only[aria-live=polite][data-selected-announce]"
            ))
            .next()
            .is_some(),
        "an always-present live region outside the hidden bar"
    );
    let sheet = include_str!("../../assets/app.css");
    assert!(sheet.contains(".table-bar") && sheet.contains("position: fixed"));
    assert!(
        sheet.contains(".has-table-bar { scroll-padding-bottom")
            && sheet.contains(".has-table-bar body { padding-bottom")
    );
    assert!(
        html.contains("buttons above and below the cards")
            && html.contains("applies to the cards shown")
    );
}

#[tokio::test]
async fn material_re_decided_unit_stays_while_first_decisions_advance() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    // First decision: still goes to the next unreviewed unit.
    let response = document_at(
        &app,
        &auth[0],
        "/admin/materials/pages/1/units/1",
        accept_form(1, ""),
    )
    .await;
    assert_eq!(
        location(&response),
        "/admin/materials/pages/1?saved=accepted&saved_unit=1&to=2&then=unit#unit-2"
    );
    // Correcting the accepted unit stays on that unit.
    let response = document_at(
        &app,
        &auth[0],
        "/admin/materials/pages/1/units/1",
        review_form(&[
            ("revision", "2"),
            ("state", "corrected"),
            ("correction", "Fixed"),
            ("rationale", "Checked"),
        ]),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let target = location(&response);
    assert_eq!(
        target,
        "/admin/materials/pages/1?saved=corrected&saved_unit=1&to=1&then=stay#unit-1"
    );
    let html = body(router_get(&app, &auth[0], &target).await).await;
    assert!(
        html.contains("Saved unit 1: corrected with source evidence."),
        "{html}"
    );
    assert!(html.contains("Staying on this unit"));
    assert!(
        scraper::Html::parse_document(&html)
            .select(&css("#unit-1 strong"))
            .next()
            .is_some(),
        "the card repeats the confirmation"
    );
    // A forged stay link for a unit in another state shows no notice.
    let false_link = "/admin/materials/pages/1?saved=rejected&saved_unit=1&to=1&then=stay";
    assert!(
        !body(router_get(&app, &auth[0], false_link).await)
            .await
            .contains("Saved unit")
    );
    // A deep unit (outside the default 50-card window) keeps its window cursor when re-decided.
    let response = document_at(
        &app,
        &auth[0],
        "/admin/materials/pages/1/units/51",
        accept_form(1, ""),
    )
    .await;
    assert!(location(&response).contains("then=unit") || location(&response).contains("then=done"));
    let response = document_at(
        &app,
        &auth[0],
        "/admin/materials/pages/1/units/51",
        review_form(&[
            ("revision", "2"),
            ("state", "rejected"),
            ("correction", ""),
            ("rationale", "No"),
        ]),
    )
    .await;
    assert_eq!(
        location(&response),
        "/admin/materials/pages/1?units=50&saved=rejected&saved_unit=51&to=51&then=stay#unit-51"
    );
}

async fn review_list_fixture() -> (tempfile::TempDir, Store, Vec<RequestAuth>) {
    let (dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    store
        .run(|db| {
            // Page ids equal PDF pages 2..=60; page 2 has 120 unreviewed units, the others one.
            db.execute_batch(
                "WITH RECURSIVE n(i) AS (SELECT 2 UNION ALL SELECT i+1 FROM n WHERE i<60) INSERT INTO source_pages(id,source_id,pdf_page,png) SELECT i,1,i,x'89504e47' FROM n;
                 WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i<120) INSERT INTO source_units(id,page_id,block_id,kind,polygon,raw_html,raw_text) SELECT 2000+i,2,'t'||i,'Text','[]','<p>x</p>','Dense '||i FROM n;
                 WITH RECURSIVE n(i) AS (SELECT 3 UNION ALL SELECT i+1 FROM n WHERE i<60) INSERT INTO source_units(id,page_id,block_id,kind,polygon,raw_html,raw_text) SELECT 3000+i,i,'s'||i,'Table','[]','<p>x</p>','Row <b>'||i FROM n;
                 INSERT INTO sources(id,bundle_hash,source_id,title,system_id,edition,language,pdf_hash,pdf,raw_json,prepared_json,total_pages) VALUES(2,'other','other','Other','mork-borg','Other edition','en',hex(zeroblob(32)),x'',x'',x'',1);
                 INSERT INTO source_pages(id,source_id,pdf_page,png) VALUES(100,2,1,x'89504e47');
                 INSERT INTO source_units(id,page_id,block_id,kind,polygon,raw_html,raw_text) VALUES(5000,100,'o','Text','[]','<p>x</p>','Foreign unit');",
            )?;
            crate::sources::search::ensure(db)?;
            Ok(())
        })
        .await
        .unwrap();
    (dir, store, auth)
}

#[tokio::test]
async fn material_review_list_groups_unreviewed_units_by_page_within_the_source() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) = review_list_fixture().await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    // Review unit 1 and unit 3003 so they leave the list.
    for (page, unit) in [(1, 1), (3, 3003)] {
        let response = document_at(
            &app,
            &auth[0],
            &format!("/admin/materials/pages/{page}/units/{unit}"),
            accept_form(1, ""),
        )
        .await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
    }
    let response = router_get(&app, &auth[0], "/admin/materials/sources/1/review").await;
    assert_eq!(response.status(), StatusCode::OK);
    let html = body(response).await;
    let parsed = scraper::Html::parse_document(&html);
    let groups: Vec<_> = parsed.select(&css("section[data-review-page]")).collect();
    assert_eq!(groups.len(), 10, "the first request lists 10 pages");
    let first = groups[0];
    assert_eq!(first.value().attr("data-review-page"), Some("1"));
    let text: String = first.text().collect();
    assert!(
        text.contains("PDF page 1") && text.contains("50 of 51 units unreviewed"),
        "{text}"
    );
    let links: Vec<_> = first
        .select(&css("a[data-unit-link]"))
        .map(|a| a.value().attr("href").unwrap())
        .collect();
    assert_eq!(links.len(), 50);
    assert_eq!(links[0], "/admin/materials/pages/1#unit-2");
    assert_eq!(
        *links.last().unwrap(),
        "/admin/materials/pages/1?units=50#unit-51"
    );
    // Units outside the default card window link with their window cursor.
    // Page 3 has no unreviewed unit left and is not listed; the foreign source never appears.
    assert!(!html.contains("data-review-page=\"3\""));
    assert!(!html.contains("Foreign unit") && !html.contains("unit-5000"));
    // The per-page listing is bounded and says what is left.
    let dense = groups[1];
    assert_eq!(dense.select(&css("a[data-unit-link]")).count(), 100);
    assert!(dense.text().collect::<String>().contains("20 more"));
    // "More" opens the page at the first unlisted unit (the 101st card of page 2).
    assert_eq!(
        dense
            .select(&css("a[data-more-link]"))
            .next()
            .and_then(|a| a.value().attr("href")),
        Some("/admin/materials/pages/2?units=100#unit-2101")
    );
    // Unit text is escaped.
    assert!(!html.contains("<b>"));
    // Paging by PDF page with one lookahead.
    let next = parsed
        .select(&css("a[data-next-review]"))
        .next()
        .expect("next link");
    assert_eq!(
        next.value().attr("href"),
        Some("/admin/materials/sources/1/review?after=11")
    );
    let second =
        body(router_get(&app, &auth[0], "/admin/materials/sources/1/review?after=11").await).await;
    let parsed = scraper::Html::parse_document(&second);
    let pages: Vec<_> = parsed
        .select(&css("section[data-review-page]"))
        .map(|s| s.value().attr("data-review-page").unwrap())
        .collect();
    assert_eq!(pages.len(), 10);
    assert_eq!(pages[0], "12");
    assert!(parsed.select(&css("a[data-next-review]")).next().is_some());
    let last =
        body(router_get(&app, &auth[0], "/admin/materials/sources/1/review?after=51").await).await;
    let parsed = scraper::Html::parse_document(&last);
    assert_eq!(parsed.select(&css("section[data-review-page]")).count(), 9);
    assert!(parsed.select(&css("a[data-next-review]")).next().is_none());
    // Bad cursor and unknown source are refused.
    assert_eq!(
        router_get(&app, &auth[0], "/admin/materials/sources/1/review?after=-1")
            .await
            .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        router_get(&app, &auth[0], "/admin/materials/sources/99/review")
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    // The other source lists only its own unit.
    let other = body(router_get(&app, &auth[0], "/admin/materials/sources/2/review").await).await;
    assert!(other.contains("Foreign unit") && !other.contains("Dense 1"));
    // Linked from the source and page screens.
    let source = body(router_get(&app, &auth[0], "/admin/materials/sources/1").await).await;
    assert!(source.contains("href=\"/admin/materials/sources/1/review\""));
    let page = body(router_get(&app, &auth[0], "/admin/materials/pages/1").await).await;
    assert!(page.contains("href=\"/admin/materials/sources/1/review\""));
}

#[tokio::test]
async fn material_review_list_of_a_fully_reviewed_source_says_so() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    accept_all_but_first(&store, &auth[0]).await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    let response = document_at(
        &app,
        &auth[0],
        "/admin/materials/pages/1/units/1",
        accept_form(1, ""),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let html = body(router_get(&app, &auth[0], "/admin/materials/sources/1/review").await).await;
    assert!(html.contains("No unit is waiting for review"));
    assert!(!html.contains("data-review-page"));
}

#[tokio::test]
async fn material_page_image_is_sticky_beside_the_cards() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    let html = body(router_get(&app, &auth[0], "/admin/materials/pages/1").await).await;
    let parsed = scraper::Html::parse_document(&html);
    assert!(
        parsed
            .select(&css(
                ".evidence-columns > aside.source-original.sticky-original"
            ))
            .next()
            .is_some()
    );
    let sheet = include_str!("../../assets/app.css");
    let rule = sheet
        .lines()
        .find(|line| line.contains(".sticky-original") && line.contains("position: sticky"))
        .expect("a sticky rule");
    assert!(rule.contains("max-height"), "{rule}");
    assert!(
        sheet.contains("@media (max-width: 700px)")
            && sheet.contains(".sticky-original { position: static")
    );
}

#[tokio::test]
async fn material_next_incomplete_page_reaches_pages_without_units_to_review() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    store
        .run(|db| {
            // Page 2 has no units (needs only a completeness check), page 3 has an unreviewed unit,
            // page 4 has only a reviewed unit. Later pages that must be skipped: a complete page 5 and an
            // incomplete page of another source.
            db.execute_batch(
                "INSERT INTO source_pages(id,source_id,pdf_page,png) VALUES(2,1,2,x'89504e47'),(3,1,3,x'89504e47'),(4,1,4,x'89504e47');
                 INSERT INTO source_pages(id,source_id,pdf_page,png,complete) VALUES(5,1,5,x'89504e47',1);
                 INSERT INTO sources(id,bundle_hash,source_id,title,system_id,edition,language,pdf_hash,pdf,raw_json,prepared_json,total_pages) VALUES(2,'other','other','Other','mork-borg','Other edition','en',hex(zeroblob(32)),x'',x'',x'',1);
                 INSERT INTO source_pages(id,source_id,pdf_page,png) VALUES(100,2,6,x'89504e47');
                 INSERT INTO source_units(id,page_id,block_id,kind,polygon,raw_html,raw_text) VALUES(900,3,'a','Text','[]','<p>x</p>','x'),(901,4,'b','Text','[]','<p>x</p>','x');",
            )?;
            crate::sources::search::ensure(db)?;
            Ok(())
        })
        .await
        .unwrap();
    let app = crate::auth::app(store.clone()).await.unwrap();
    let response = document_at(
        &app,
        &auth[0],
        "/admin/materials/pages/4/units/901",
        accept_form(1, ""),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let link = |html: &str, text: &str| {
        scraper::Html::parse_document(html)
            .select(&css("nav a"))
            .find(|a| a.text().collect::<String>() == text)
            .map(|a| a.value().attr("href").unwrap().to_owned())
    };
    let one = body(router_get(&app, &auth[0], "/admin/materials/pages/1").await).await;
    assert_eq!(
        link(&one, "Next page with units to review").as_deref(),
        Some("/admin/materials/pages/3")
    );
    assert_eq!(
        link(&one, "Next page to confirm complete").as_deref(),
        Some("/admin/materials/pages/2")
    );
    let two = body(router_get(&app, &auth[0], "/admin/materials/pages/2").await).await;
    assert_eq!(
        link(&two, "Next page to confirm complete").as_deref(),
        Some("/admin/materials/pages/4")
    );
    let four = body(router_get(&app, &auth[0], "/admin/materials/pages/4").await).await;
    assert_eq!(link(&four, "Next page to confirm complete"), None);
    assert_eq!(link(&four, "Next page with units to review"), None);
}

#[tokio::test]
async fn material_review_list_response_fits_the_html_budget_with_adversarial_text() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture().await;
    seed(&store).await;
    store
        .run(|db| {
            // Pages 2..=11 with 101 units each: maximum-length kinds of `&` and 80-character
            // snippets of quotes (escaped to five bytes each) or four-byte characters.
            db.execute_batch(
                "WITH RECURSIVE n(i) AS (SELECT 2 UNION ALL SELECT i+1 FROM n WHERE i<11) INSERT INTO source_pages(id,source_id,pdf_page,png) SELECT i,1,i,x'89504e47' FROM n;
                 WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i<101), pg(p) AS (SELECT 2 UNION ALL SELECT p+1 FROM pg WHERE p<11)
                 INSERT INTO source_units(id,page_id,block_id,kind,polygon,raw_html,raw_text)
                 SELECT 10000+p*1000+i,p,'b'||p||'-'||i,replace(hex(zeroblob(50)),'0','&'),'[]','<p>x</p>',
                        CASE WHEN p%2=0 THEN replace(hex(zeroblob(40)),'0','\"') ELSE replace(hex(zeroblob(40)),'0','😀') END
                 FROM n,pg;",
            )?;
            crate::sources::search::ensure(db)?;
            Ok(())
        })
        .await
        .unwrap();
    let app = crate::auth::app(store.clone()).await.unwrap();
    let response = router_get(&app, &auth[0], "/admin/materials/sources/1/review").await;
    assert_eq!(response.status(), StatusCode::OK);
    let html = body(response).await;
    let links = html.matches("data-unit-link").count();
    assert!(
        links > 900 && links <= 1000,
        "a response lists at most 1,000 units: {links}"
    );
    assert!(
        html.len() <= 2 * 1024 * 1024,
        "worst-case review list: {} bytes",
        html.len()
    );
    assert!(html.contains("data-next-review"));
}

#[tokio::test]
async fn material_review_list_after_a_read_deadline_shows_counts_and_the_next_link() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth) = review_list_fixture().await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    store
        .force_read_deadline("review_list", std::time::Duration::ZERO)
        .await;
    let response = router_get(&app, &auth[0], "/admin/materials/sources/1/review").await;
    assert_eq!(response.status(), StatusCode::OK);
    let html = body(response).await;
    assert!(html.contains("Read deadline reached"), "{html}");
    assert!(html.contains("229 unreviewed unit(s) on 60 page(s)"));
    assert!(!html.contains("data-review-page"));
    assert!(html.contains("review?after=10"));
    // The next request is complete again.
    let html = body(router_get(&app, &auth[0], "/admin/materials/sources/1/review").await).await;
    assert!(!html.contains("Read deadline reached") && html.contains("data-review-page"));
}
