//! The "Model" setting on `/admin/assistant` through the full auth router (`docs/model-runtime.md`,
//! "Owner model setting"): the grammar's refusals with the input kept, the trial, Switch, Keep and
//! Switch back stored in the database, rights, CSRF, strict forms, body limits and stale
//! trials, over a fake launcher (a fast one: real time passes here).
use super::*;
use crate::assistant::records::{Records, SqlRecords, Stored};
use crate::assistant::runtime::Path;
use crate::assistant::runtime::manager::tests::{Behaviour, FakeLauncher, Start};
use crate::assistant::runtime::manager::{self, Launcher, ModelSetup};
use crate::assistant::runtime::models::{Model, ModelFile};
use crate::assistant::{AssistantHandle, DEFAULT_MODEL_SHA256};
use axum::http::StatusCode;
use std::time::Duration;

fn csrf() -> String {
    "a".repeat(64)
}

fn form(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

async fn body(response: axum::response::Response) -> String {
    String::from_utf8(
        axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}

fn granite() -> Model {
    Model {
        spec: "local:granite-4.2-3b-Q4_K_M.gguf".into(),
        name: "granite-4.2-3b-Q4_K_M.gguf".into(),
        sha256: DEFAULT_MODEL_SHA256.into(),
        alias: "granite-4.2-3b-q4_k_m".into(),
        max_tokens: 1024,
        size: None,
    }
}

/// A server that is quick in real time.
fn quick() -> Behaviour {
    Behaviour {
        loading: 0,
        p: 0.0001,
        g: 0.0001,
        phrase_delay: Duration::from_millis(1),
        ..Behaviour::default()
    }
}

struct Models {
    _dir: tempfile::TempDir,
    store: Store,
    auth: Vec<RequestAuth>,
    app: axum::Router,
    launcher: Arc<FakeLauncher>,
    handle: manager::Handle,
}

/// The assistant over a quick fake launcher, the default Granite file active, `other.gguf`
/// (hash `b…b`) and `third.gguf` (hash `c…c`) in the models folder; `slow` makes each trial
/// phrase take that long.
async fn models(slow: Duration) -> Models {
    models_with(slow, Path::Cpu, false, Start::Ok(quick())).await
}

/// [`models`] on `path`; with `device_default` the active model is the device default and the
/// Qwen3.5-9B file is missing from the models folder.
async fn models_with(slow: Duration, path: Path, device_default: bool, first: Start) -> Models {
    models_older(slow, path, device_default, first, false).await
}

/// [`models_with`] whose active model's stored trial predates the prompt in force.
async fn models_older(
    slow: Duration,
    path: Path,
    device_default: bool,
    first: Start,
    older_prompt: bool,
) -> Models {
    let (dir, store, auth, _clock) = fixture_with_owner(1).await;
    let behaviour = |phrase_delay| Behaviour {
        phrase_delay,
        ..quick()
    };
    let launcher = FakeLauncher::scripted(
        std::iter::once(first)
            .chain((0..40).map(|_| Start::Ok(behaviour(slow))))
            .collect::<Vec<_>>(),
    );
    if device_default {
        launcher
            .absent
            .lock()
            .unwrap()
            .push("Qwen3.5-9B-Q4_K_M.gguf".into());
    }
    for (name, sha) in [("other.gguf", 'b'), ("third.gguf", 'c')] {
        launcher.files.lock().unwrap().insert(
            name.to_owned(),
            Ok(ModelFile {
                sha256: sha.to_string().repeat(64),
                size: 4 << 30,
            }),
        );
    }
    let handle = AssistantHandle::with_models(
        Arc::clone(&launcher) as Arc<dyn Launcher>,
        manager::Config {
            path,
            levels: manager::USED_LEVELS,
        },
        ModelSetup {
            active: granite(),
            previous: None,
            records: Some(Arc::new(SqlRecords(store.clone()))),
            from_owner_setting: false,
            device_default,
            older_prompt: crate::assistant::runtime::models::OlderPrompt {
                active: older_prompt,
                previous: false,
            },
        },
    );
    let runtime = handle.runtime.clone().unwrap();
    let app = crate::auth::app_with_options(
        store.clone(),
        crate::auth::AuthOptions {
            assistant: Some(handle),
            ..crate::auth::AuthOptions::default()
        },
    )
    .await
    .unwrap();
    let models = Models {
        _dir: dir,
        store,
        auth,
        app,
        launcher,
        handle: runtime,
    };
    models
        .until(|status| status.state == manager::State::Ready)
        .await;
    models
}

impl Models {
    async fn until(&self, done: impl Fn(&manager::Status) -> bool) -> manager::Status {
        for _ in 0..1000 {
            let status = self.handle.status();
            if done(&status) {
                return status;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("not reached: {:?}", self.handle.status());
    }

    async fn post(
        &self,
        who: usize,
        action: &str,
        pairs: &[(&str, &str)],
    ) -> axum::response::Response {
        router_request(
            &self.app,
            &self.auth[who],
            &format!("/admin/assistant/model/{action}"),
            Some(form(pairs)),
        )
        .await
    }

    async fn owner(&self, action: &str, pairs: &[(&str, &str)]) -> String {
        let response = self.post(0, action, pairs).await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER, "{action}");
        response.headers()["location"].to_str().unwrap().to_owned()
    }

    async fn page(&self) -> String {
        body(router_request(&self.app, &self.auth[0], "/admin/assistant", None).await).await
    }

    /// Tries `name` and waits for the trial to end and the previous model to serve again.
    async fn try_model(&self, name: &str) -> manager::Status {
        let spec = format!("local:{name}");
        assert_eq!(
            self.owner("try", &[("csrf", &csrf()), ("model", &spec)])
                .await,
            "/admin/assistant?notice=trying-model"
        );
        let status = self
            .until(|status| status.trial.as_ref().is_some_and(|t| !t.running()))
            .await;
        drop(status);
        self.until(|status| status.state == manager::State::Ready)
            .await
    }

    async fn count(&self, table: &'static str) -> i64 {
        self.store
            .run(move |db| {
                Ok(db.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))?)
            })
            .await
            .unwrap()
    }
}

fn trial_key(status: &manager::Status) -> String {
    match &status.trial {
        Some(crate::assistant::runtime::models::TrialState::Passed(report)) => {
            report.key.to_string()
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn the_model_field_refuses_bad_specs_naming_the_rule_and_keeps_the_input() {
    let _lock = crate::auth::test_resource_lock().await;
    let models = models(Duration::from_millis(1)).await;
    let too_long = format!("local:{}.gguf", "a".repeat(510));
    for (input, rule) in [
        ("local:../x.gguf", "one name, without"),
        ("local:/etc/x.gguf", "one name, without"),
        ("local:a/b.gguf", "one name, without"),
        ("local:a\\b.gguf", "one name, without"),
        ("local:.x.gguf", "must not start with"),
        ("local:CON.gguf", "Windows device name"),
        ("local:nul.gguf", "Windows device name"),
        ("local:x.bin", "must end with .gguf"),
        ("local: x.gguf", "white space"),
        ("local:x.gguf ", "white space"),
        ("local:a b.gguf", "white space"),
        ("local:\u{e9}.gguf", "must be ASCII"),
        (too_long.as_str(), "longer than 512 bytes"),
        ("hf:org/repo/m.gguf@main", "40 lowercase hex digits"),
        (
            "hf:org/m.gguf@c40945d71cd90f249a56985e8155551a9188dc30",
            "must be hf:",
        ),
        ("model.gguf", "must be local:"),
    ] {
        let response = models
            .post(0, "try", &[("csrf", &csrf()), ("model", input)])
            .await;
        assert_eq!(
            response.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "{input:?}"
        );
        let page = body(response).await;
        assert!(page.contains(rule), "{input:?}: {page}");
        // The field keeps what the owner typed (the page escapes it).
        let kept = format!("value=\"{}\"", input.replace('&', "&amp;"));
        assert!(page.contains(&kept), "{input:?}: {page}");
        assert!(page.contains("role=\"alert\""), "{input:?}");
    }
    // Nothing was hashed or started for any of them.
    assert!(models.launcher.hashed.lock().unwrap().is_empty());
    assert_eq!(models.handle.status().trial, None);
}

#[tokio::test]
async fn every_model_action_is_owner_only_csrf_checked_strict_and_body_bounded() {
    let _lock = crate::auth::test_resource_lock().await;
    let models = models(Duration::from_millis(1)).await;
    let key = format!("1.2.{}", "b".repeat(64));
    let valid: [(&str, Vec<(&str, String)>); 6] = [
        (
            "try",
            vec![("csrf", csrf()), ("model", "local:other.gguf".into())],
        ),
        ("cancel", vec![("csrf", csrf()), ("trial", "1".into())]),
        ("switch", vec![("csrf", csrf()), ("trial", key)]),
        ("keep", vec![("csrf", csrf())]),
        ("switch-back", vec![("csrf", csrf())]),
        (
            "length",
            vec![("csrf", csrf()), ("max_tokens", "1536".into())],
        ),
    ];
    for (action, pairs) in &valid {
        let borrowed = |pairs: &Vec<(&str, String)>| -> Vec<(String, String)> {
            pairs
                .iter()
                .map(|(k, v)| ((*k).to_owned(), v.clone()))
                .collect()
        };
        let send = |who: usize, pairs: Vec<(String, String)>| {
            let path = format!("/admin/assistant/model/{action}");
            let (app, auth) = (models.app.clone(), models.auth[who].clone());
            async move { router_request(&app, &auth, &path, Some(pairs)).await }
        };
        // A player (not the owner) is forbidden.
        assert_eq!(
            send(1, borrowed(pairs)).await.status(),
            StatusCode::FORBIDDEN,
            "{action}"
        );
        // A wrong CSRF token is forbidden.
        let mut wrong = borrowed(pairs);
        wrong[0].1 = "b".repeat(64);
        assert_eq!(
            send(0, wrong).await.status(),
            StatusCode::FORBIDDEN,
            "{action}"
        );
        // An extra key, a duplicated key and a missing key are refused.
        let mut extra = borrowed(pairs);
        extra.push(("extra".into(), "1".into()));
        assert_eq!(
            send(0, extra).await.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "{action}"
        );
        let mut duplicate = borrowed(pairs);
        duplicate.push(duplicate[0].clone());
        assert_eq!(
            send(0, duplicate).await.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "{action}"
        );
        assert_eq!(
            send(0, Vec::new()).await.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "{action}"
        );
        // A body over the limit is refused before it is read.
        let mut huge = borrowed(pairs);
        huge.push(("pad".into(), "x".repeat(5000)));
        assert_eq!(
            send(0, huge).await.status(),
            StatusCode::PAYLOAD_TOO_LARGE,
            "{action}"
        );
    }
    // A GET is not an action.
    let get = router_request(
        &models.app,
        &models.auth[0],
        "/admin/assistant/model/try",
        None,
    )
    .await;
    assert_eq!(get.status(), StatusCode::METHOD_NOT_ALLOWED);
    // None of it started anything.
    assert!(models.launcher.hashed.lock().unwrap().is_empty());
    assert_eq!(models.count("assistant_models").await, 0);
}

#[tokio::test]
async fn a_trial_switch_keep_and_switch_back_through_the_routes_are_stored() {
    let _lock = crate::auth::test_resource_lock().await;
    let models = models(Duration::from_millis(1)).await;
    let page = models.page().await;
    assert!(page.contains("<h2>Model</h2>"), "{page}");
    assert!(
        page.contains("contained only by its confinement: level 1 (Hakoniwa)"),
        "{page}"
    );
    assert!(
        page.contains("value=\"local:granite-4.2-3b-Q4_K_M.gguf\""),
        "{page}"
    );
    assert!(page.contains(DEFAULT_MODEL_SHA256), "{page}");
    let back = "action=\"/admin/assistant/model/switch-back\"";
    assert!(!page.contains(back), "no previous model yet");

    let status = models.try_model("other.gguf").await;
    let key = trial_key(&status);
    let page = models.page().await;
    // The result: device, times, speeds, max_tokens, B, R, D and Q, and the phrases.
    for shown in [
        "Trial 1 of local:other.gguf: passed",
        "Passed on the CPU",
        "Priming",
        "tokens/s",
        "<strong>1024</strong>",
        "Request bound B 1024 bytes",
        "run deadline R",
        "queue D",
        "took 6 damage",
        "ho perso 3 punti",
        "Expected value offered",
        "Keep the current model",
    ] {
        assert!(page.contains(shown), "{shown}: {page}");
    }
    assert!(
        page.contains(&format!("name=\"trial\" value=\"{key}\"")),
        "{page}"
    );
    // The previous model already serves again.
    assert_eq!(status.model.active, granite());
    // A trial stores nothing.
    assert_eq!(models.count("assistant_models").await, 0);

    // Switch: stored, restarted with the new model.
    assert_eq!(
        models
            .owner("switch", &[("csrf", &csrf()), ("trial", &key)])
            .await,
        "/admin/assistant?notice=switching"
    );
    let status = models
        .until(|s| s.model.active.spec == "local:other.gguf" && s.state == manager::State::Ready)
        .await;
    assert_eq!(status.model.previous, Some(granite()));
    assert_eq!(models.count("assistant_models").await, 2);
    let (active, previous, by): (String, String, i64) = models
        .store
        .run(|db| {
            Ok(db.query_row(
                "SELECT a.spec,p.spec,c.changed_by FROM assistant_model_choice c JOIN assistant_models a ON a.id=c.active_model JOIN assistant_models p ON p.id=c.previous_model",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )?)
        })
        .await
        .unwrap();
    assert_eq!(
        (active.as_str(), previous.as_str(), by),
        ("local:other.gguf", "local:granite-4.2-3b-Q4_K_M.gguf", 1)
    );
    let stored = SqlRecords(models.store.clone()).load().await.unwrap();
    let recorded = stored.active.unwrap();
    assert_eq!(recorded.model.sha256, "b".repeat(64));
    assert_eq!(recorded.model.max_tokens, 1024);
    assert!(
        recorded
            .trial
            .unwrap()
            .contains("\"measured_max_tokens\":1024")
    );
    let page = models.page().await;
    assert!(
        page.contains("Active: <strong>local:other.gguf</strong>"),
        "{page}"
    );
    assert!(
        page.contains("stored setting (the start flags are ignored)"),
        "{page}"
    );
    assert!(page.contains(back), "{page}");
    assert!(
        !page.contains("name=\"trial\""),
        "the switched trial is consumed"
    );

    // Switch back: the default again.
    assert_eq!(
        models.owner("switch-back", &[("csrf", &csrf())]).await,
        "/admin/assistant?notice=switching-back"
    );
    let status = models
        .until(|s| s.model.active.spec == granite().spec && s.state == manager::State::Ready)
        .await;
    assert_eq!(status.model.previous.unwrap().spec, "local:other.gguf");
    let stored = SqlRecords(models.store.clone()).load().await.unwrap();
    assert_eq!(stored.active.unwrap().model.sha256, DEFAULT_MODEL_SHA256);
}

#[tokio::test]
async fn keep_dismisses_a_finished_trial_and_nothing_restarts() {
    let _lock = crate::auth::test_resource_lock().await;
    let models = models(Duration::from_millis(1)).await;
    models.try_model("third.gguf").await;
    let generation = models.handle.status().generation;
    assert_eq!(
        models.owner("keep", &[("csrf", &csrf())]).await,
        "/admin/assistant?notice=kept"
    );
    assert_eq!(models.handle.status().trial, None);
    assert_eq!(
        models.owner("keep", &[("csrf", &csrf())]).await,
        "/admin/assistant?notice=nothing-to-keep"
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(models.handle.status().generation, generation);
}

#[tokio::test]
async fn a_second_trial_while_one_runs_is_refused_and_cancel_restores_the_previous_model() {
    let _lock = crate::auth::test_resource_lock().await;
    let models = models(Duration::from_millis(400)).await;
    assert_eq!(
        models
            .owner("try", &[("csrf", &csrf()), ("model", "local:other.gguf")])
            .await,
        "/admin/assistant?notice=trying-model"
    );
    assert_eq!(
        models
            .owner("try", &[("csrf", &csrf()), ("model", "local:third.gguf")])
            .await,
        "/admin/assistant?notice=trial-running"
    );
    // Players see "starting" meanwhile, and the page offers Cancel.
    models
        .until(|s| matches!(s.state, manager::State::Starting(_)) && s.trial.is_some())
        .await;
    let page = models.page().await;
    assert!(page.contains("<strong>Starting</strong>"), "{page}");
    assert!(
        page.contains("action=\"/admin/assistant/model/cancel\""),
        "{page}"
    );
    assert_eq!(*models.launcher.hashed.lock().unwrap(), ["other.gguf"]);
    // A page that showed another trial cancels nothing.
    assert_eq!(
        models
            .owner("cancel", &[("csrf", &csrf()), ("trial", "99")])
            .await,
        "/admin/assistant?notice=cancel-stale"
    );
    assert!(models.handle.status().trial.is_some_and(|t| t.running()));
    assert_eq!(
        models
            .owner("cancel", &[("csrf", &csrf()), ("trial", "1")])
            .await,
        "/admin/assistant?notice=trial-cancelled"
    );
    models
        .until(|s| {
            matches!(
                s.trial,
                Some(crate::assistant::runtime::models::TrialState::Cancelled { .. })
            ) && s.state == manager::State::Ready
        })
        .await;
    assert_eq!(
        models
            .owner("cancel", &[("csrf", &csrf()), ("trial", "1")])
            .await,
        "/admin/assistant?notice=no-trial-running"
    );
    assert_eq!(
        models.launcher.selected.lock().unwrap().last(),
        Some(&granite())
    );
}

#[tokio::test]
async fn a_stale_or_malformed_trial_key_changes_nothing() {
    let _lock = crate::auth::test_resource_lock().await;
    let models = models(Duration::from_millis(1)).await;
    let status = models.try_model("other.gguf").await;
    let key = trial_key(&status);
    let generation = status.generation;
    let parts: Vec<&str> = key.split('.').collect();
    let (number, trial_generation, sha) = (parts[0], parts[1], parts[2]);
    let older_generation = (trial_generation.parse::<u64>().unwrap() - 1).to_string();
    for stale in [
        format!("{number}.{trial_generation}.{}", "c".repeat(64)),
        format!("99.{trial_generation}.{sha}"),
        format!("{number}.{older_generation}.{sha}"),
        format!("{number}.{trial_generation}"),
        "not a key".to_owned(),
        String::new(),
    ] {
        assert_eq!(
            models
                .owner("switch", &[("csrf", &csrf()), ("trial", &stale)])
                .await,
            "/admin/assistant?notice=trial-stale",
            "{stale:?}"
        );
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(models.handle.status().generation, generation);
    assert_eq!(models.count("assistant_models").await, 0);
    // Another trial makes the first one's key stale.
    let newer = models.try_model("third.gguf").await;
    assert_ne!(trial_key(&newer), key);
    assert_eq!(
        models
            .owner("switch", &[("csrf", &csrf()), ("trial", &key)])
            .await,
        "/admin/assistant?notice=trial-stale"
    );
    // Without a previous model there is nothing to switch back to.
    assert_eq!(
        models.owner("switch-back", &[("csrf", &csrf())]).await,
        "/admin/assistant?notice=no-previous-model"
    );
    assert_eq!(models.count("assistant_models").await, 0);
}

#[tokio::test]
async fn a_session_revoked_during_a_trial_cannot_apply_its_result() {
    let _lock = crate::auth::test_resource_lock().await;
    let models = models(Duration::from_millis(300)).await;
    assert_eq!(
        models
            .owner("try", &[("csrf", &csrf()), ("model", "local:other.gguf")])
            .await,
        "/admin/assistant?notice=trying-model"
    );
    // The owner's session is revoked while the trial runs.
    models
        .store
        .run(|db| {
            db.execute("UPDATE accounts SET epoch=2 WHERE id=1", [])?;
            Ok(())
        })
        .await
        .unwrap();
    let status = models
        .until(|s| {
            s.trial.as_ref().is_some_and(|t| !t.running()) && s.state == manager::State::Ready
        })
        .await;
    let key = trial_key(&status);
    // The trial finished, but nothing can apply it: not Switch, not Keep, not Switch back.
    for (action, pairs) in [
        ("switch", vec![("csrf", csrf()), ("trial", key.clone())]),
        ("keep", vec![("csrf", csrf())]),
        ("switch-back", vec![("csrf", csrf())]),
    ] {
        let pairs: Vec<(&str, &str)> = pairs.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let response = models.post(0, action, &pairs).await;
        assert!(
            matches!(
                response.status(),
                StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
            ),
            "{action}: {}",
            response.status()
        );
    }
    assert_eq!(models.handle.status().model.active, granite());
    assert!(matches!(
        models.handle.status().trial,
        Some(crate::assistant::runtime::models::TrialState::Passed(_))
    ));
    assert_eq!(models.count("assistant_models").await, 0);
}

#[tokio::test]
async fn a_development_endpoint_has_no_model_form() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture_with_owner(1).await;
    let launcher = FakeLauncher::scripted([Start::Ok(quick())]);
    let handle = AssistantHandle::with_launcher(
        Arc::clone(&launcher) as Arc<dyn Launcher>,
        manager::Config {
            path: Path::Cpu,
            levels: manager::USED_LEVELS,
        },
    );
    let app = crate::auth::app_with_options(
        store,
        crate::auth::AuthOptions {
            assistant: Some(handle),
            ..crate::auth::AuthOptions::default()
        },
    )
    .await
    .unwrap();
    let page = body(router_request(&app, &auth[0], "/admin/assistant", None).await).await;
    assert!(page.contains("The model cannot be changed here"), "{page}");
    assert!(
        !page.contains("action=\"/admin/assistant/model/try\""),
        "{page}"
    );
    let response = router_request(
        &app,
        &auth[0],
        "/admin/assistant/model/try",
        Some(form(&[("csrf", &csrf()), ("model", "local:other.gguf")])),
    )
    .await;
    assert_eq!(
        response.headers()["location"],
        "/admin/assistant?notice=model-unsupported"
    );
    assert!(launcher.hashed.lock().unwrap().is_empty());
}

#[tokio::test]
async fn the_reply_length_route_checks_bounds_and_steps_stores_one_file_row_and_restarts_once() {
    use crate::assistant::model::reply_length::{DEFAULT, MAX, MIN, STEP};
    let _lock = crate::auth::test_resource_lock().await;
    let models = models(Duration::from_millis(1)).await;
    let settled = |tokens: u32| {
        move |s: &manager::Status| {
            s.model.active.max_tokens == tokens && s.state == manager::State::Ready && !s.restarting
        }
    };
    // The form shows the bounds from the constants and the active value.
    let page = models.page().await;
    for shown in [
        "action=\"/admin/assistant/model/length\"".to_owned(),
        format!(
            "name=\"max_tokens\" type=\"number\" min=\"{MIN}\" max=\"{MAX}\" step=\"{STEP}\" value=\"{DEFAULT}\""
        ),
        "does not run the five phrases".to_owned(),
        "a reply can take up to 15 s".to_owned(),
        "per token".to_owned(),
        // Q + R, not 2 × R: on the CPU at R 15 s the queue wait is 120 − 15 s plus the 1 s the
        // server stays busy after a cancelled request.
        "a player can wait up to about 121 s".to_owned(),
        "belongs to the model file".to_owned(),
    ] {
        assert!(page.contains(&shown), "{shown}: {page}");
    }
    // Crafted values change nothing: off the step, out of bounds, not a number.
    for crafted in [
        "1000",
        "255",
        "257",
        "0",
        "2688",
        "99999999999999",
        "-128",
        "abc",
        "",
        "1536.0",
        "1e3",
        " ",
        "0x600",
    ] {
        assert_eq!(
            models
                .owner("length", &[("csrf", &csrf()), ("max_tokens", crafted)])
                .await,
            "/admin/assistant?notice=length-invalid",
            "{crafted:?}"
        );
    }
    assert_eq!(
        models
            .owner("length", &[("csrf", &csrf()), ("max_tokens", "1024")])
            .await,
        "/admin/assistant?notice=length-unchanged"
    );
    assert_eq!(models.count("assistant_models").await, 0);
    assert_eq!(models.count("assistant_reply_lengths").await, 0);
    assert_eq!(models.launcher.starts.lock().unwrap().len(), 1);
    // A valid value: one row of the file's own table (no model row, no choice), and one
    // restart.
    assert_eq!(
        models
            .owner("length", &[("csrf", &csrf()), ("max_tokens", "1536")])
            .await,
        "/admin/assistant?notice=length-set"
    );
    let status = models.until(settled(1536)).await;
    assert_eq!(status.model.previous, None);
    assert_eq!(models.count("assistant_models").await, 0);
    assert_eq!(models.count("assistant_model_choice").await, 0);
    assert_eq!(models.count("assistant_reply_lengths").await, 1);
    assert_eq!(models.launcher.starts.lock().unwrap().len(), 2);
    let page = models.page().await;
    assert!(page.contains("value=\"1536\""), "{page}");
    assert!(page.contains("longest reply 1536 tokens"), "{page}");
    // Nothing was hashed or tried.
    assert!(models.launcher.hashed.lock().unwrap().is_empty());
    assert_eq!(models.handle.status().trial, None);
}

#[tokio::test]
async fn the_reply_length_is_one_row_per_file_and_leaves_the_model_rows_and_choice_alone() {
    let _lock = crate::auth::test_resource_lock().await;
    let models = models(Duration::from_millis(1)).await;
    let status = models.try_model("other.gguf").await;
    let key = trial_key(&status);
    models
        .owner("switch", &[("csrf", &csrf()), ("trial", &key)])
        .await;
    models
        .until(|s| s.model.active.spec == "local:other.gguf" && s.state == manager::State::Ready)
        .await;
    assert_eq!(models.count("assistant_models").await, 2);
    let hashed = models.launcher.hashed.lock().unwrap().len();
    let before = SqlRecords(models.store.clone()).load().await.unwrap();
    let settled = |tokens: u32| {
        move |s: &manager::Status| {
            s.model.active.max_tokens == tokens && s.state == manager::State::Ready && !s.restarting
        }
    };
    // Set 2048 on the switched model: its file gets a row; the model rows, the trial record and
    // the choice stay, and the five phrases are not run.
    assert_eq!(
        models
            .owner("length", &[("csrf", &csrf()), ("max_tokens", "2048")])
            .await,
        "/admin/assistant?notice=length-set"
    );
    models.until(settled(2048)).await;
    assert_eq!(models.count("assistant_models").await, 2);
    assert_eq!(models.count("assistant_reply_lengths").await, 1);
    let after = SqlRecords(models.store.clone()).load().await.unwrap();
    let (active, previous) = (after.active.unwrap(), after.previous.unwrap());
    let (was_active, was_previous) = (before.active.unwrap(), before.previous.unwrap());
    assert_eq!(active.trial, was_active.trial);
    assert_eq!(active.model.sha256, "b".repeat(64));
    assert_eq!(active.model.max_tokens, 2048);
    assert_eq!(
        previous, was_previous,
        "the other file keeps its own length"
    );
    assert_eq!(models.launcher.hashed.lock().unwrap().len(), hashed);
    // Setting it back changes the same row: still one row for the file.
    models
        .owner("length", &[("csrf", &csrf()), ("max_tokens", "1024")])
        .await;
    models.until(settled(1024)).await;
    assert_eq!(models.count("assistant_models").await, 2);
    assert_eq!(models.count("assistant_reply_lengths").await, 1);
}

#[tokio::test]
async fn the_stored_setting_beats_the_start_flags() {
    let _lock = crate::auth::test_resource_lock().await;
    let (dir, store, _auth, _clock) = fixture_with_owner(1).await;
    // A previous run switched to another model.
    let other = Model {
        spec: "local:other.gguf".into(),
        name: "other.gguf".into(),
        sha256: "b".repeat(64),
        alias: "other".into(),
        max_tokens: 1024,
        size: Some(1 << 30),
    };
    let records = SqlRecords(store.clone());
    let record = |model: Model| crate::assistant::records::Record { model, trial: None };
    records
        .activate(&record(other.clone()), Some(&record(granite())), 1)
        .await
        .unwrap();
    let handle = AssistantHandle::runtime(
        crate::assistant::RuntimeConfig {
            data_dir: dir.path().to_path_buf(),
            models_dir: None,
            runtime_dir: Some(dir.path().join("no-runtime")),
            model: Some(crate::assistant::DEFAULT_MODEL.to_owned()),
            model_sha256: None,
            model_alias: None,
            threads: Some(1),
            device: Some("cpu".into()),
        },
        store,
    )
    .await
    .unwrap();
    let status = handle.runtime.clone().unwrap().status();
    assert_eq!(status.model.active, other);
    assert_eq!(status.model.previous, Some(granite()));
    assert!(status.model.from_owner_setting);
    assert!(status.model.switchable);
    handle.shutdown().await;
}

#[tokio::test]
async fn an_explicit_9b_spec_without_a_hash_gets_the_9b_hash_not_granites() {
    let _lock = crate::auth::test_resource_lock().await;
    let (dir, store, _auth, _clock) = fixture_with_owner(1).await;
    let handle = AssistantHandle::runtime(
        default_config(dir.path(), Some(crate::assistant::GPU_DEFAULT_MODEL)),
        store,
    )
    .await
    .unwrap();
    let status = handle.runtime.clone().unwrap().status();
    assert!(!status.model.device_default.is_on());
    assert_eq!(
        status.model.active.sha256,
        crate::assistant::GPU_DEFAULT_MODEL_SHA256
    );
    assert_eq!(status.model.active.max_tokens, 1024);
    handle.shutdown().await;
}

#[tokio::test]
async fn the_page_says_why_the_device_default_is_granite() {
    let _lock = crate::auth::test_resource_lock().await;
    let first = || Start::Ok(quick());
    let models = models_with(Duration::from_millis(1), Path::Gpu, true, first()).await;
    let page = models.page().await;
    assert!(
        page.contains("Qwen3.5-9B not found or not readable in /srv/models; using Granite 4.2 3B."),
        "{page}"
    );
    assert!(page.contains("from the default for this device"), "{page}");
}

#[tokio::test]
async fn the_model_the_flags_name_starts_with_the_reply_length_stored_for_its_file() {
    let _lock = crate::auth::test_resource_lock().await;
    let (dir, store, _auth, _clock) = fixture_with_owner(1).await;
    let records = SqlRecords(store.clone());
    records
        .set_length(DEFAULT_MODEL_SHA256, Some(1536), 1)
        .await
        .unwrap();
    // The flag names the Granite file: it starts with the length its file has, and nothing
    // else is stored for it (no model row, no choice).
    let handle = AssistantHandle::runtime(
        default_config(dir.path(), Some(crate::assistant::DEFAULT_MODEL)),
        store.clone(),
    )
    .await
    .unwrap();
    let status = handle.runtime.clone().unwrap().status();
    assert_eq!(status.model.active.max_tokens, 1536);
    assert!(!status.model.device_default.is_on());
    assert_eq!(records.load().await.unwrap(), Stored::default());
    handle.shutdown().await;
}

#[tokio::test]
async fn the_page_says_when_the_default_9b_gave_way_to_granite_on_the_cpu() {
    let _lock = crate::auth::test_resource_lock().await;
    let failing = Start::Other("the model server exited before it was ready");
    let models = models_with(Duration::from_millis(1), Path::Gpu, true, failing).await;
    // The 9B file is present in this fixture's folder only if not marked absent; the GPU
    // start failed, so Granite runs on the CPU and the page says so.
    let page = models.page().await;
    assert!(
        page.contains("the GPU is not in use, so Granite 4.2 3B runs on the CPU."),
        "{page}"
    );
}

fn default_config(dir: &std::path::Path, model: Option<&str>) -> crate::assistant::RuntimeConfig {
    crate::assistant::RuntimeConfig {
        data_dir: dir.to_path_buf(),
        models_dir: None,
        runtime_dir: Some(dir.join("no-runtime")),
        model: model.map(str::to_owned),
        model_sha256: None,
        model_alias: None,
        threads: Some(1),
        device: Some("cpu".into()),
    }
}

#[tokio::test]
async fn only_the_absence_of_a_flag_and_of_a_stored_setting_leaves_the_device_default() {
    let _lock = crate::auth::test_resource_lock().await;
    let (dir, store, _auth, _clock) = fixture_with_owner(1).await;
    // No flag, nothing stored: the device default.
    let handle = AssistantHandle::runtime(default_config(dir.path(), None), store.clone())
        .await
        .unwrap();
    assert!(
        handle
            .runtime
            .clone()
            .unwrap()
            .status()
            .model
            .device_default
            .is_on()
    );
    handle.shutdown().await;
    // An explicit flag, even the Granite file itself, is never replaced.
    let handle = AssistantHandle::runtime(
        default_config(dir.path(), Some(crate::assistant::DEFAULT_MODEL)),
        store.clone(),
    )
    .await
    .unwrap();
    let status = handle.runtime.clone().unwrap().status();
    assert!(!status.model.device_default.is_on());
    assert_eq!(status.model.active, granite());
    handle.shutdown().await;
    // A hash or alias without a model would be silently dropped: refused.
    let mut config = default_config(dir.path(), None);
    config.model_sha256 = Some("b".repeat(64));
    let Err(why) = AssistantHandle::runtime(config, store.clone()).await else {
        panic!("a hash without a model must be refused")
    };
    assert!(why.contains("need --assistant-model"), "{why}");
    // The stored setting beats the default.
    let other = Model {
        spec: "local:other.gguf".into(),
        name: "other.gguf".into(),
        sha256: "b".repeat(64),
        alias: "other".into(),
        max_tokens: 1024,
        size: Some(1 << 30),
    };
    SqlRecords(store.clone())
        .activate(
            &crate::assistant::records::Record {
                model: other.clone(),
                trial: None,
            },
            None,
            1,
        )
        .await
        .unwrap();
    let handle = AssistantHandle::runtime(default_config(dir.path(), None), store)
        .await
        .unwrap();
    let status = handle.runtime.clone().unwrap().status();
    assert!(!status.model.device_default.is_on());
    assert_eq!(status.model.active, other);
    handle.shutdown().await;
}

#[tokio::test]
async fn the_9b_hash_mismatch_page_offers_try_the_gpu_again() {
    let _lock = crate::auth::test_resource_lock().await;
    let models = models_with(
        Duration::from_millis(1),
        Path::Gpu,
        true,
        Start::Ok(quick()),
    )
    .await;
    models.launcher.absent.lock().unwrap().clear();
    models
        .launcher
        .script
        .lock()
        .unwrap()
        .push_front(Start::Other(
            "the model file changed: it no longer has the SHA-256 recorded for it",
        ));
    let generation = models.handle.status().generation;
    assert_eq!(models.handle.restart(), manager::Restart::Accepted);
    models
        .until(|s| s.generation >= generation + 2 && s.state == manager::State::Ready)
        .await;
    let page = models.page().await;
    assert!(page.contains("does not match its pinned hash"), "{page}");
    assert!(!page.contains("not in use"), "{page}");
    assert!(
        page.contains("Granite 4.2 3B runs on the GPU instead of Qwen3.5-9B: "),
        "{page}"
    );
    assert!(
        page.contains("Try the GPU again checks the 9B file again."),
        "{page}"
    );
    assert!(
        page.contains("action=\"/admin/assistant/try-gpu\""),
        "{page}"
    );
    let response = router_request(
        &models.app,
        &models.auth[0],
        "/admin/assistant/try-gpu",
        Some(form(&[("csrf", &csrf())])),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    models
        .until(|s| {
            s.model.active.sha256 == crate::assistant::GPU_DEFAULT_MODEL_SHA256
                && s.state == manager::State::Ready
        })
        .await;
}

#[tokio::test]
async fn an_alias_the_stored_setting_would_refuse_is_refused_at_start() {
    let _lock = crate::auth::test_resource_lock().await;
    let (dir, store, _auth, _clock) = fixture_with_owner(1).await;
    let config = |alias: &str| crate::assistant::RuntimeConfig {
        data_dir: dir.path().to_path_buf(),
        models_dir: None,
        runtime_dir: Some(dir.path().join("no-runtime")),
        model: Some(crate::assistant::DEFAULT_MODEL.to_owned()),
        model_sha256: None,
        model_alias: Some(alias.to_owned()),
        threads: Some(1),
        device: Some("cpu".into()),
    };
    // Migration 022 allows 1 to 128 of [A-Za-z0-9._-]; any other alias would make every
    // Switch fail at its store.
    for refused in ["a b", "", "a/b", "\u{e9}", &"a".repeat(129)] {
        let Err(why) = AssistantHandle::runtime(config(refused), store.clone()).await else {
            panic!("{refused:?} must be refused at start")
        };
        assert!(why.contains("--assistant-model-alias"), "{why}");
    }
    let handle = AssistantHandle::runtime(config("granite.v2_x-1"), store)
        .await
        .unwrap();
    handle.shutdown().await;
}

#[tokio::test]
async fn the_page_says_when_the_measurement_predates_the_prompt() {
    let _lock = crate::auth::test_resource_lock().await;
    let older = models_older(
        Duration::from_millis(1),
        Path::Cpu,
        false,
        Start::Ok(quick()),
        true,
    )
    .await;
    let page = older.page().await;
    assert!(
        page.contains("measured with an older prompt") && page.contains("Try the model again"),
        "{page}"
    );
    // A model measured with the current prompt, or never, carries no such line.
    let current = models(Duration::from_millis(1)).await;
    assert!(!current.page().await.contains("older prompt"));
}

#[tokio::test]
async fn a_stored_trial_is_flagged_unless_it_ran_under_the_current_prompt() {
    let _lock = crate::auth::test_resource_lock().await;
    let other = || Model {
        spec: "local:other.gguf".into(),
        name: "other.gguf".into(),
        sha256: "b".repeat(64),
        alias: "other".into(),
        max_tokens: 1024,
        size: Some(1 << 30),
    };
    let current = serde_json::json!({"prefix_sha256": crate::assistant::contract::prefix_sha256(), "trial": 1}).to_string();
    // A trial that recorded no prompt (migration 022 rows before ASSIST-56) and one recorded
    // under another prompt are older; one under the current prompt and none at all are not.
    for (trial, older) in [
        (Some(r#"{"trial":1}"#.to_owned()), true),
        (
            Some(format!(r#"{{"prefix_sha256":"{}"}}"#, "0".repeat(64))),
            true,
        ),
        (Some(current), false),
        (None, false),
    ] {
        let (dir, store, _auth, _clock) = fixture_with_owner(1).await;
        SqlRecords(store.clone())
            .activate(
                &crate::assistant::records::Record {
                    model: other(),
                    trial: trial.clone(),
                },
                None,
                1,
            )
            .await
            .unwrap();
        let handle = AssistantHandle::runtime(default_config(dir.path(), None), store)
            .await
            .unwrap();
        let status = handle.runtime.clone().unwrap().status();
        assert_eq!(status.model.older_prompt.is_older(), older, "{trial:?}");
        handle.shutdown().await;
    }
}

/// Try and Set stay enabled while the model server restarts: a press is answered by the
/// server with the reason (`model-busy`), never silently ignored by a disabled button (the
/// owner saw a green Try that did nothing, 2026-11-16).
#[tokio::test]
async fn try_and_set_answer_with_a_notice_while_a_restart_runs() {
    let _lock = crate::auth::test_resource_lock().await;
    let models = models(Duration::from_millis(1)).await;
    *models.launcher.preflight_delay.lock().unwrap() = Some(Duration::from_secs(3600));
    let restart = router_request(
        &models.app,
        &models.auth[0],
        "/admin/assistant/restart",
        Some(form(&[("csrf", &csrf())])),
    )
    .await;
    assert_eq!(restart.status(), StatusCode::SEE_OTHER);
    models.until(|status| status.restarting).await;
    let page = models.page().await;
    assert!(page.contains("<button>Try</button>"), "{page}");
    assert!(page.contains("<button>Set</button>"), "{page}");
    assert_eq!(
        models
            .owner("try", &[("csrf", &csrf()), ("model", "local:other.gguf")])
            .await,
        "/admin/assistant?notice=model-busy"
    );
    assert_eq!(
        models
            .owner("length", &[("csrf", &csrf()), ("max_tokens", "1536")])
            .await,
        "/admin/assistant?notice=model-busy"
    );
    let page = body(
        router_request(
            &models.app,
            &models.auth[0],
            "/admin/assistant?notice=model-busy",
            None,
        )
        .await,
    )
    .await;
    assert!(page.contains("already being carried out"), "{page}");
    models.handle.shutdown().await;
}

/// `/admin/assistant/status.json`, parsed.
async fn status_json(models: &Models) -> serde_json::Value {
    let response = router_request(
        &models.app,
        &models.auth[0],
        "/admin/assistant/status.json",
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    serde_json::from_str(&body(response).await).unwrap()
}

/// The signature a page carries for its script.
fn page_signature(page: &str) -> String {
    let marker = "data-signature=\"";
    let at = page
        .find(marker)
        .unwrap_or_else(|| panic!("no signature in {page}"))
        + marker.len();
    let end = page[at..].find('"').unwrap();
    page[at..at + end].to_owned()
}

#[tokio::test]
async fn the_status_route_is_owner_only_never_cached_and_off_without_a_runtime() {
    let _lock = crate::auth::test_resource_lock().await;
    let models = models(Duration::from_millis(1)).await;
    let response = router_request(
        &models.app,
        &models.auth[0],
        "/admin/assistant/status.json",
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["cache-control"], "no-store");
    // A player is forbidden; a request with no session is not served either.
    let response = router_request(
        &models.app,
        &models.auth[1],
        "/admin/assistant/status.json",
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let signed_out = {
        use tower::ServiceExt;
        models
            .app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/admin/assistant/status.json")
                    .header("host", "127.0.0.1:3000")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
    };
    assert_eq!(signed_out.status(), StatusCode::UNAUTHORIZED);
    // The retired poller's route and script are gone.
    for gone in [
        "/admin/assistant/downloads.json",
        "/assets/assistant-downloads.js",
    ] {
        let response = router_request(&models.app, &models.auth[0], gone, None).await;
        assert!(!response.status().is_success(), "{gone}");
    }
    // Without an assistant there is nothing to follow.
    let (_dir, store, auth, _clock) = fixture_with_owner(1).await;
    let app = crate::auth::app_with_options(store, crate::auth::AuthOptions::default())
        .await
        .unwrap();
    let off = router_request(&app, &auth[0], "/admin/assistant/status.json", None).await;
    assert_eq!(off.status(), StatusCode::OK);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&body(off).await).unwrap(),
        serde_json::json!({"available": false})
    );
}

#[tokio::test]
async fn the_poll_interval_is_short_while_work_runs_long_when_idle_and_the_progress_follows() {
    let _lock = crate::auth::test_resource_lock().await;
    let models = models(Duration::from_millis(300)).await;
    let idle = status_json(&models).await;
    assert_eq!(idle["available"], true);
    assert_eq!(idle["poll_ms"], 15_000);
    assert_eq!(idle["progress"], serde_json::Value::Null);
    assert_eq!(
        models
            .owner("try", &[("csrf", &csrf()), ("model", "local:other.gguf")])
            .await,
        "/admin/assistant?notice=trying-model"
    );
    let busy = status_json(&models).await;
    assert_eq!(busy["poll_ms"], 2_000);
    let progress = &busy["progress"];
    assert!(
        progress["text"]
            .as_str()
            .is_some_and(|t| t.starts_with("Trial of local:other.gguf: step ")),
        "{busy}"
    );
    assert_eq!(progress["steps"], 13);
    assert!(progress["step"].as_u64().is_some_and(|step| step >= 1));
    // Nothing in the answer names a path, a URL or a secret.
    assert!(!busy.to_string().contains('/'), "{busy}");
    models
        .until(|status| {
            status.trial.as_ref().is_some_and(|t| !t.running())
                && status.state == manager::State::Ready
                && !status.model.pending
        })
        .await;
    let after = status_json(&models).await;
    assert_eq!(after["poll_ms"], 15_000);
    assert_eq!(after["progress"], serde_json::Value::Null);
    assert!(!after.to_string().contains('/'), "{after}");
}

#[tokio::test]
async fn the_signature_matches_the_pages_and_a_step_alone_never_changes_it() {
    let _lock = crate::auth::test_resource_lock().await;
    let models = models(Duration::from_millis(300)).await;
    let page = models.page().await;
    assert_eq!(
        page_signature(&page),
        status_json(&models).await["signature"].as_str().unwrap()
    );
    assert_eq!(
        models
            .owner("try", &[("csrf", &csrf()), ("model", "local:other.gguf")])
            .await,
        "/admin/assistant?notice=trying-model"
    );
    // From the first phrase on, the steps move and the signature does not.
    models
        .until(|s| {
            s.state
                .starting()
                .is_some_and(|p| p.phase == crate::assistant::runtime::progress::Phase::Phrase(1))
        })
        .await;
    let mut texts = std::collections::BTreeSet::new();
    let mut signatures = std::collections::BTreeSet::new();
    let mut inside = None;
    for _ in 0..100 {
        let state = status_json(&models).await;
        // Only the trial's own steps: its outcome is a change of state.
        let Some(text) = state["progress"]["text"]
            .as_str()
            .filter(|text| text.starts_with("Trial of"))
        else {
            break;
        };
        texts.insert(text.to_owned());
        signatures.insert(state["signature"].as_str().unwrap().to_owned());
        if inside.is_none() {
            let page = models.page().await;
            let again = status_json(&models).await;
            if again["progress"]["text"]
                .as_str()
                .is_some_and(|t| t.starts_with("Trial of"))
            {
                inside = Some(page_signature(&page));
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(texts.len() >= 3, "{texts:?}");
    assert_eq!(signatures.len(), 1, "{signatures:?}");
    // The page of a moment inside the trial carries that signature.
    assert!(inside.is_some_and(|signature| signatures.contains(&signature)));
}

#[tokio::test]
async fn the_page_shows_the_running_step_and_says_it_updates_itself() {
    let _lock = crate::auth::test_resource_lock().await;
    let models = models(Duration::from_millis(300)).await;
    // Idle: no progress block, the slow interval.
    let page = models.page().await;
    assert!(!page.contains("id=\"progress-text\""), "{page}");
    assert!(page.contains("data-poll-ms=\"15000\""), "{page}");
    assert!(
        page.contains("<script src=\"/assets/assistant-live.js\" defer></script>"),
        "{page}"
    );
    assert!(
        page.contains("<button type=\"button\" id=\"live-stop\" hidden>Stop updating</button>"),
        "{page}"
    );
    // A restart: the stop is its first step, in the State section.
    *models.launcher.start_delay.lock().unwrap() = Some(Duration::from_millis(400));
    let response = router_request(
        &models.app,
        &models.auth[0],
        "/admin/assistant/restart",
        Some(form(&[("csrf", &csrf())])),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let page = models.page().await;
    assert!(page.contains("<strong>Starting</strong>"), "{page}");
    assert!(
        page.contains("id=\"progress-text\" aria-live=\"polite\">Starting: step "),
        "{page}"
    );
    assert!(
        page.contains("<progress id=\"progress-bar\" aria-labelledby=\"progress-text\""),
        "{page}"
    );
    assert!(page.contains("data-poll-ms=\"2000\""), "{page}");
    assert!(
        page.contains("This page updates itself; without JavaScript, reload it"),
        "{page}"
    );
    models
        .until(|s| s.state == manager::State::Ready && !s.restarting)
        .await;
    *models.launcher.start_delay.lock().unwrap() = None;
    // A trial: its block replaces the old "Now:" line and the reload instruction.
    assert_eq!(
        models
            .owner("try", &[("csrf", &csrf()), ("model", "local:other.gguf")])
            .await,
        "/admin/assistant?notice=trying-model"
    );
    let page = models.page().await;
    assert!(page.contains("Trial of local:other.gguf: step "), "{page}");
    assert!(!page.contains("Now: "), "{page}");
    assert!(!page.contains("Reload this page to follow it"), "{page}");
    assert_eq!(page.matches("id=\"progress-text\"").count(), 1, "{page}");
    let notice = body(
        router_request(
            &models.app,
            &models.auth[0],
            "/admin/assistant?notice=trying-model",
            None,
        )
        .await,
    )
    .await;
    assert!(notice.contains("This page updates itself"), "{notice}");
    assert!(!notice.contains("Reload this page"), "{notice}");
    // The script sets text, value and max only.
    let script = body(
        router_request(
            &models.app,
            &models.auth[0],
            "/assets/assistant-live.js",
            None,
        )
        .await,
    )
    .await;
    assert!(script.contains("textContent") && !script.contains("innerHTML"));
}

#[tokio::test]
async fn a_reason_with_line_breaks_and_a_nul_gives_the_same_signature_in_the_page_and_the_answer() {
    let _lock = crate::auth::test_resource_lock().await;
    let models = models(Duration::from_millis(1)).await;
    *models.launcher.preflight.lock().unwrap() = Some("one\r\ntwo\rthree\0four".into());
    let response = router_request(
        &models.app,
        &models.auth[0],
        "/admin/assistant/restart",
        Some(form(&[("csrf", &csrf())])),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    models
        .until(|s| matches!(s.state, manager::State::Unavailable(_)))
        .await;
    let page = models.page().await;
    let signature = page_signature(&page);
    // Lowercase hex only: nothing an HTML parser would normalise in an attribute.
    assert!(
        signature.len() == 64
            && signature
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "{signature}"
    );
    assert_eq!(
        signature,
        status_json(&models).await["signature"].as_str().unwrap()
    );
}

async fn post_reminders(models: &Models, pairs: &[(&str, &str)]) -> String {
    let response = router_request(
        &models.app,
        &models.auth[0],
        "/admin/assistant/reminders",
        Some(form(pairs)),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    response.headers()["location"].to_str().unwrap().to_owned()
}

#[tokio::test]
async fn the_reminder_route_stores_the_choice_without_a_restart_and_the_page_shows_it() {
    let _lock = crate::auth::test_resource_lock().await;
    let models = models(Duration::from_millis(1)).await;
    let csrf = csrf();
    // The CPU path: off until chosen, no row.
    let page = models.page().await;
    assert!(
        page.contains("name=\"reminders\" value=\"off\" checked"),
        "{page}"
    );
    assert_eq!(models.count("assistant_reminders").await, 0);
    // Crafted values change nothing.
    for crafted in ["", "ON", "true", "1", "on "] {
        assert_eq!(
            post_reminders(&models, &[("csrf", &csrf), ("reminders", crafted)]).await,
            "/admin/assistant?notice=reminders-invalid",
            "{crafted:?}"
        );
    }
    assert_eq!(models.count("assistant_reminders").await, 0);
    // On: one row, no restart, and the page shows it as chosen.
    assert_eq!(
        post_reminders(&models, &[("csrf", &csrf), ("reminders", "on")]).await,
        "/admin/assistant?notice=reminders-on"
    );
    assert_eq!(models.count("assistant_reminders").await, 1);
    assert_eq!(models.launcher.starts.lock().unwrap().len(), 1);
    let page = models.page().await;
    assert!(
        page.contains("name=\"reminders\" value=\"on\" checked"),
        "{page}"
    );
    assert!(page.contains("you have chosen"), "{page}");
    assert_eq!(
        post_reminders(&models, &[("csrf", &csrf), ("reminders", "off")]).await,
        "/admin/assistant?notice=reminders-off"
    );
    assert_eq!(models.count("assistant_reminders").await, 1);
    // A request with a wrong CSRF token or an extra field is refused before anything changes.
    let bad = router_request(
        &models.app,
        &models.auth[0],
        "/admin/assistant/reminders",
        Some(form(&[("csrf", "wrong"), ("reminders", "on")])),
    )
    .await;
    assert_ne!(bad.status(), StatusCode::SEE_OTHER);
    let extra = router_request(
        &models.app,
        &models.auth[0],
        "/admin/assistant/reminders",
        Some(form(&[("csrf", &csrf), ("reminders", "on"), ("x", "1")])),
    )
    .await;
    assert_ne!(extra.status(), StatusCode::SEE_OTHER);
    let (enabled,): (i64,) = models
        .store
        .run(|db| Ok((db.query_row("SELECT enabled FROM assistant_reminders", [], |r| r.get(0))?,)))
        .await
        .unwrap();
    assert_eq!(enabled, 0);
}
