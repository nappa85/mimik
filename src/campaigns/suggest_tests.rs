//! Assisted sheet step 4: the suggest route and its confinement (plan item 7 to 9), through
//! the real router. Synthetic catalogue names only.
use super::{
    catalogue_member_tests::{entry, gear, published, world},
    formula_tests::Setup,
    interpret_route_tests::send,
    list_tests::legacy_create,
};
use crate::campaigns::web::suggest_web::{IN_MATCH, SUGGEST, forget_facts};
use axum::http::StatusCode;
use serde_json::{Value, json};
use std::sync::Arc;

async fn suggest(
    setup: &Setup,
    path_and_query: &str,
) -> (StatusCode, Value, axum::http::HeaderMap) {
    send(
        setup,
        "GET",
        &format!("/private-sheets/1/suggest/{path_and_query}"),
        None,
        true,
    )
    .await
}

fn keys(answer: &Value) -> Vec<String> {
    answer["suggestions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["key"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn names_are_suggested_by_prefix_substring_and_on_request_by_typo() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = world(None, &[("Shield", None)]).await;
    forget_facts();
    let (status, answer, headers) = suggest(&setup, "1/0?q=sw").await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer["status"], "ok");
    assert_eq!(keys(&answer), ["sword-a", "sword-b"]);
    assert_eq!(answer["more"], false);
    assert_eq!(answer["cut"], false);
    // Duplicates (same norm) are flagged, with their type and a hint from the entry text.
    let first = &answer["suggestions"][0];
    assert_eq!(first["name"], "Sword");
    assert_eq!(first["type"], "Weapon");
    assert_eq!(first["duplicate"], true);
    assert!(
        first["hint"]
            .as_str()
            .unwrap()
            .starts_with("Synthetic text of sword-a"),
        "{first}"
    );
    assert_eq!(answer["suggestions"][1]["type"], "Tool");
    // The strict sheet policy and no caching.
    assert_eq!(
        headers["content-security-policy"],
        crate::campaigns::web::SHEET_CSP
    );
    assert_eq!(headers["cache-control"], "no-store");
    // Substring tier, folding (ß, case), retired entries never.
    assert_eq!(keys(&suggest(&setup, "1/0?q=ope").await.1), ["rope"]);
    assert_eq!(keys(&suggest(&setup, "1/0?q=STRASSE").await.1), ["strasse"]);
    assert_eq!(
        keys(&suggest(&setup, "1/0?q=old").await.1),
        Vec::<String>::new()
    );
    // The typo pass only on request.
    assert_eq!(
        keys(&suggest(&setup, "1/0?q=swrod").await.1),
        Vec::<String>::new()
    );
    let (_, typo, _) = suggest(&setup, "1/0?q=swrod&typo=1").await;
    assert_eq!(keys(&typo), ["sword-a", "sword-b"]);
}

#[tokio::test]
async fn the_member_s_entry_type_filters_every_tier() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = world(Some("weapon"), &[("Shield", None)]).await;
    for query in ["sw", "ord", "swrod&typo=1"] {
        let (status, answer, _) = suggest(&setup, &format!("1/0?q={query}")).await;
        assert_eq!(status, StatusCode::OK, "{answer}");
        assert_eq!(keys(&answer), ["sword-a"], "{query}");
        assert_eq!(answer["suggestions"][0]["duplicate"], false);
    }
    assert_eq!(
        keys(&suggest(&setup, "1/0?q=shi").await.1),
        Vec::<String>::new()
    );
}

#[tokio::test]
async fn malformed_requests_are_400_before_anything_is_looked_up() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = world(None, &[("Shield", None)]).await;
    let long_name = "a".repeat(65);
    let bytes = "a".repeat(257);
    let raw = format!("1/0?q=sw&x={}", "a".repeat(1100));
    let cases = [
        "1/0?q=s",       // one scalar
        "1/0?q=%20s%20", // one after trimming
        &format!("1/0?q={long_name}"),
        &format!("1/0?q={bytes}"),
        "1/0?q=%EF%B7%BA%EF%B7%BA%EF%B7%BA%EF%B7%BA", // folds to 72 scalars
        "1/0?q=%E2%80%8B%E2%80%8B",                   // folds to nothing
        "1/0?q=sw&typo=1&typo=1",                     // repeated
        "1/0?q=sw&q=sw",
        "1/0?q=sw&typo=2",
        "1/0?q=sw&other=1",
        "1/0?q=sw&typo=1&extra",
        "1/0?typo=1", // no q
        "1/0",
        "1/0?q=sw&typo=0",
        "1/0?q=ab&typo=1", // two scalars: too short for the typo pass
        &raw,
        "01/0?q=sw",
        "1/00?q=sw",
        "x/0?q=sw",
    ];
    for case in cases {
        let (status, _, _) = suggest(&setup, case).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{case}");
    }
    // Only the exact JSON Accept is served.
    let (status, _, _) = send(
        &setup,
        "GET",
        "/private-sheets/1/suggest/1/0?q=sw",
        None,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn every_refusal_is_the_one_uniform_403_json_that_survives_the_auth_layer() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = world(None, &[("Shield", None)]).await;
    let uniform = json!({"status": "unavailable"});
    for path in [
        "1/2?q=sw", // a property of the list that has no catalogue member (the key)
        "1/1?q=sw", // quantity
        "0/0?q=sw", // a scalar field, not a list
        "9/0?q=sw", // forged field
        "1/9?q=sw", // forged property
    ] {
        let (status, answer, headers) = suggest(&setup, path).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}");
        assert_eq!(answer, uniform, "{path}");
        assert_eq!(headers["content-type"], "application/json");
    }
    // An unknown sheet, and a player who may not read this sheet (account 3 is no member).
    let (status, answer, _) = send(
        &setup,
        "GET",
        "/private-sheets/99/suggest/1/0?q=sw",
        None,
        true,
    )
    .await;
    assert_eq!((status, answer), (StatusCode::FORBIDDEN, uniform.clone()));
    let stranger = Setup {
        auth: vec![setup.auth[0].clone(), setup.auth[2].clone()],
        ..clone_setup(&setup)
    };
    let (status, answer, _) = suggest(&stranger, "1/0?q=sw").await;
    assert_eq!((status, answer), (StatusCode::FORBIDDEN, uniform));
}

fn clone_setup(setup: &Setup) -> Setup {
    Setup {
        dir: tempfile::tempdir().unwrap(),
        store: setup.store.clone(),
        auth: setup.auth.clone(),
        app: setup.app.clone(),
        campaign: setup.campaign,
        version: setup.version,
    }
}

#[tokio::test]
async fn the_pool_answers_429_at_saturation_with_its_retry_after() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = world(None, &[("Shield", None)]).await;
    let held: Vec<_> = (0..4)
        .map(|_| SUGGEST.clone().try_acquire_owned().unwrap())
        .collect();
    let (status, answer, headers) = suggest(&setup, "1/0?q=sw").await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(answer, json!({"status": "busy"}));
    assert_eq!(headers["retry-after"], "1");
    drop(held);
    assert_eq!(suggest(&setup, "1/0?q=sw").await.0, StatusCode::OK);
}

#[tokio::test]
async fn a_pin_facts_hit_needs_no_runtime_permit_and_a_miss_answers_503_when_it_is_busy() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = world(None, &[("Shield", None)]).await;
    forget_facts();
    assert_eq!(
        suggest(&setup, "1/0?q=sw").await.0,
        StatusCode::OK,
        "warm the facts"
    );
    let jobs = crate::definitions::admit().unwrap();
    let (status, answer, _) = suggest(&setup, "1/0?q=sw").await;
    assert_eq!(status, StatusCode::OK, "a hit runs without JOBS: {answer}");
    forget_facts();
    let (status, answer, headers) = suggest(&setup, "1/0?q=sw").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(answer, json!({"status": "busy"}));
    assert_eq!(headers["retry-after"], "1");
    drop(jobs);
    assert_eq!(suggest(&setup, "1/0?q=sw").await.0, StatusCode::OK);
}

#[tokio::test]
async fn an_aborted_request_keeps_its_permit_until_its_matching_ends() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = Arc::new(world(None, &[("Shield", None)]).await);
    let (reached, reached_rx) = std::sync::mpsc::channel::<()>();
    let (release, release_rx) = std::sync::mpsc::channel::<()>();
    *IN_MATCH.lock().unwrap() = Some(Arc::new({
        let reached = std::sync::Mutex::new(reached);
        let release_rx = std::sync::Mutex::new(release_rx);
        move || {
            reached.lock().unwrap().send(()).unwrap();
            let _ = release_rx.lock().unwrap().recv();
        }
    }));
    let free = SUGGEST.available_permits();
    let request = tokio::spawn({
        let setup = setup.clone();
        async move { suggest(&setup, "1/0?q=swrod&typo=1").await.0 }
    });
    tokio::task::spawn_blocking(move || reached_rx.recv().unwrap())
        .await
        .unwrap();
    assert_eq!(SUGGEST.available_permits(), free - 1);
    request.abort();
    let _ = request.await;
    // The request is gone but its blocking work is not: the permit is still held.
    assert_eq!(SUGGEST.available_permits(), free - 1);
    release.send(()).unwrap();
    for _ in 0..200 {
        if SUGGEST.available_permits() == free {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("the permit was never released");
}

#[tokio::test]
async fn a_version_sealed_between_requests_is_served_from_its_own_cache_key() {
    use crate::catalogues::PublishRequest;
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = world(None, &[("Shield", None)]).await;
    assert_eq!(
        keys(&suggest(&setup, "1/0?q=lan").await.1),
        Vec::<String>::new()
    );
    let mut next = gear();
    next["entries"]
        .as_array_mut()
        .unwrap()
        .push(entry("lantern", "tool", "Lantern", false));
    let draft = setup
        .store
        .save_catalogue_draft(serde_json::to_vec(&next).unwrap(), 1)
        .await
        .unwrap();
    let comparison = setup.store.compare_catalogue(draft.id).await.unwrap();
    setup
        .store
        .publish_catalogue(
            draft.id,
            PublishRequest {
                comparison_hash: comparison.comparison_hash,
                actor: "Synthetic operator".into(),
                rationale: "Synthetic second version".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(keys(&suggest(&setup, "1/0?q=lan").await.1), ["lantern"]);
}

#[tokio::test]
async fn two_sheets_on_different_pins_get_their_own_pin_facts() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    // Pin 1 types the name property as weapon; pin 2 (another definition version of the same
    // catalogue) as tool: the same (f, p) answers differently once each pin's facts are cached.
    let setup = published(
        &super::catalogue_member_tests::definition(Some("weapon")),
        &gear(),
    )
    .await;
    legacy_create(&setup, 1, json!({"label": "Sheet", "equipment": []})).await;
    forget_facts();
    assert_eq!(keys(&suggest(&setup, "1/0?q=sw").await.1), ["sword-a"]);
    let second = {
        let definition = super::catalogue_member_tests::definition(Some("tool"));
        let draft = setup
            .store
            .save_definition_draft(serde_json::to_vec(&definition).unwrap(), 1)
            .await
            .unwrap();
        let comparison = setup
            .store
            .compare_definition(draft.id, false)
            .await
            .unwrap();
        setup
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
            .unwrap()
            .id
    };
    assert_ne!(second, setup.version);
    // Sheet 2 is pinned to the second version.
    let on_second = Setup {
        version: second,
        ..clone_setup(&setup)
    };
    super::list_tests::legacy_create_id(&on_second, 1, 2, json!({"label": "Two", "equipment": []}))
        .await;
    let (status, answer, _) = send(
        &setup,
        "GET",
        "/private-sheets/2/suggest/1/0?q=sw",
        None,
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(keys(&answer), ["sword-b"]);
    assert_eq!(keys(&suggest(&setup, "1/0?q=sw").await.1), ["sword-a"]);
}

#[tokio::test]
async fn names_with_markup_quotes_and_right_to_left_letters_travel_as_json_text() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let mut catalogue = gear();
    let hostile = "Zap <img src=x> \"q\" & \u{5d0}\u{5d1}\u{5d2}";
    catalogue["entries"]
        .as_array_mut()
        .unwrap()
        .push(entry("zap", "tool", hostile, false));
    let setup = published(&super::catalogue_member_tests::definition(None), &catalogue).await;
    legacy_create(&setup, 1, json!({"label": "Sheet", "equipment": []})).await;
    let (status, answer, headers) = suggest(&setup, "1/0?q=zap").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(answer["suggestions"][0]["name"], hostile);
    assert_eq!(headers["content-type"], "application/json");
    assert_eq!(headers["x-content-type-options"], "nosniff");
}

/// p50 and p95 of 1,000 warm requests per mix against a catalogue at the entry cap (6,144
/// entries of up to 100 scalars, shared prefixes, mixed scripts, several types), through the
/// router and the real auth layer (`tools/suggest_profile.py` times the release binary over
/// HTTP; this is the in-process twin). Ignored: run with
/// `cargo test --release -- --ignored --nocapture measure_the_worst_suggest_request`.
/// The 50 ms gate applies to release builds only.
#[tokio::test]
#[ignore = "measurement, not a gate for debug runs"]
async fn measure_the_worst_suggest_request() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let scripts = [
        "Lantern",
        "Longsword",
        "Lamp",
        "\u{424}\u{43e}\u{43d}\u{430}\u{440}\u{44c}",
        "\u{d55c}\u{ae00}",
        "\u{9577}\u{5263}",
    ];
    let mut catalogue = gear();
    catalogue["entry_types"] = json!([
        {"id": "tool", "label": "Tool", "properties": []},
        {"id": "weapon", "label": "Weapon", "properties": []},
    ]);
    let entries: Vec<Value> = (0..6144)
        .map(|n| {
            let base = scripts[n % scripts.len()];
            let name: String = if n % 7 == 0 {
                format!("{base} of the long winding road beyond the silver mountains and the sleeping sea {n}")
                    .chars()
                    .take(100)
                    .collect()
            } else if n % 3 == 1 {
                // Short names, so many norms fall within 2 scalars of the typo query.
                format!("{} {}", ["Rope", "Lamp", "Flask", "Oil"][n % 4], n % 100)
            } else {
                format!("{base} {} mark {}", n / 6, n % 97)
            };
            entry(&format!("e{n}"), if n % 2 == 0 { "tool" } else { "weapon" }, &name, false)
        })
        .collect();
    catalogue["entries"] = Value::Array(entries);
    let setup = published(&super::catalogue_member_tests::definition(None), &catalogue).await;
    legacy_create(&setup, 1, json!({"label": "Sheet", "equipment": []})).await;
    let mut gate = Vec::new();
    for (mix, query) in [
        ("prefix", "la"),
        ("substring", "nter"),
        ("typo", "qzxwvu&typo=1"),
    ] {
        let path = format!("1/0?q={query}");
        let started = std::time::Instant::now();
        assert_eq!(suggest(&setup, &path).await.0, StatusCode::OK);
        let cold = started.elapsed();
        let mut warm = Vec::with_capacity(1000);
        for _ in 0..1000 {
            let started = std::time::Instant::now();
            assert_eq!(suggest(&setup, &path).await.0, StatusCode::OK);
            warm.push(started.elapsed());
        }
        warm.sort();
        let (p50, p95) = (warm[499], warm[949]);
        println!("{mix}: cold {cold:?}, p50 {p50:?}, p95 {p95:?}");
        gate.push((mix, p95));
    }
    if !cfg!(debug_assertions) {
        for (mix, p95) in gate {
            assert!(
                p95 <= std::time::Duration::from_millis(50),
                "{mix}: p95 {p95:?}"
            );
        }
    }
}

/// Rewrites sheet 1's stored pin (and kind) behind the sheet triggers, which refuse such rows
/// at creation: what damage or an older build could leave behind.
async fn tamper(setup: &Setup, pin: String, kind: &'static str) {
    setup
        .store
        .run(move |db| {
            db.execute_batch("DROP TRIGGER private_sheets_update")?;
            db.execute(
                "UPDATE private_sheets SET pin=?1,kind=?2 WHERE id=1",
                rusqlite::params![pin, kind],
            )?;
            Ok(())
        })
        .await
        .unwrap();
}

async fn pin_of(setup: &Setup) -> String {
    setup
        .store
        .run(|db| {
            Ok(
                db.query_row("SELECT pin FROM private_sheets WHERE id=1", [], |r| {
                    r.get(0)
                })?,
            )
        })
        .await
        .unwrap()
}

#[tokio::test]
async fn native_unsupported_and_corrupt_pins_are_the_uniform_403_on_suggest_and_which() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    for variant in ["native", "unsupported", "corrupt"] {
        let setup = world(None, &[("Sword", None)]).await;
        forget_facts();
        assert_eq!(
            suggest(&setup, "1/0?q=sw").await.0,
            StatusCode::OK,
            "{variant}: before"
        );
        let pin = pin_of(&setup).await;
        match variant {
            "native" => {
                let native = r#"{"type":"native","adapter":"mork-borg-it-2025","schema":1,"calculation_version":1}"#;
                tamper(&setup, native.to_owned(), "native").await;
            }
            "unsupported" => {
                tamper(
                    &setup,
                    pin.replace("\"schema\":1", "\"schema\":2"),
                    "runtime",
                )
                .await;
            }
            // Valid JSON of no pin shape (the column is read with JSON functions).
            _ => tamper(&setup, r#"{"type":"bogus"}"#.to_owned(), "runtime").await,
        }
        let (status, answer, _) = suggest(&setup, "1/0?q=sw").await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{variant}");
        assert_eq!(answer, json!({"status": "unavailable"}), "{variant}");
        let (status, html) = setup
            .get("/private-sheets/1/cells/1/0/0/which?base=1")
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{variant}");
        assert!(
            html.contains("can\u{2019}t be listed from here"),
            "{variant}"
        );
    }
}

#[tokio::test]
async fn a_catalogue_without_a_sealed_version_is_the_uniform_403_and_shows_no_warning() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let _runtime_lock = crate::definitions::TEST_RESOURCE_LOCK.lock().await;
    let setup = world(None, &[("Sword", None), ("Zzyzx unknown", None)]).await;
    assert_eq!(suggest(&setup, "1/0?q=sw").await.0, StatusCode::OK);
    let (_, html) = setup.get("/private-sheets/1").await;
    assert_eq!(html.matches("class=\"entry-match\"").count(), 2);
    setup
        .store
        .run(|db| {
            db.execute_batch("DROP TRIGGER catalogue_seals_delete; DELETE FROM catalogue_seals;")?;
            Ok(())
        })
        .await
        .unwrap();
    let (status, answer, _) = suggest(&setup, "1/0?q=sw").await;
    assert_eq!(
        (status, answer),
        (StatusCode::FORBIDDEN, json!({"status": "unavailable"}))
    );
    let (status, _) = setup
        .get("/private-sheets/1/cells/1/0/0/which?base=1")
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // The page still renders its rows, with no warning and no link: never "Not in the
    // catalogue" for a catalogue that could not be read.
    let (status, html) = setup.get("/private-sheets/1").await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("Zzyzx unknown"));
    assert_eq!(html.matches("class=\"entry-match\"").count(), 0);
    assert_eq!(html.matches("class=\"entry-link\"").count(), 0);
    assert!(!html.contains("Not in the catalogue"));
}
