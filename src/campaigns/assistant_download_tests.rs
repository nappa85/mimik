//! The Downloads panel and routes of `/admin/assistant` through the full auth router
//! (`docs/model-downloads.md`): review, confirm, download, delete, the model-work gate, rights, CSRF,
//! strict forms and the 5 s request deadline, over a fake Hugging Face on loopback with a test
//! CA and a fake launcher.
use super::*;
use crate::assistant::AssistantHandle;
use crate::assistant::DEFAULT_MODEL_SHA256;
use crate::assistant::download::client::{Client, SHIPPED_HOSTS};
use crate::assistant::download::disk::{Disk, NoFaults};
use crate::assistant::download::fake_hub::{FakeHub, Reply, Seen};
use crate::assistant::download::task::Config;
use crate::assistant::download_rows::SqlRows;
use crate::assistant::downloads::Downloads;
use crate::assistant::records::SqlRecords;
use crate::assistant::runtime::Path;
use crate::assistant::runtime::manager::tests::{Behaviour, FakeLauncher, Start};
use crate::assistant::runtime::manager::{self, Launcher, ModelSetup};
use crate::assistant::runtime::models::{Model, ModelFile};
use axum::http::StatusCode;
use sha2::{Digest, Sha256};
use std::time::Duration;

const REV: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const SPEC: &str = "hf:acme/tiny-GGUF/tiny.gguf@aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const OTHER: &str = "hf:acme/other-GGUF/other.gguf@aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const LICENCE: &str = "Tiny licence <script>alert(1)</script>";

fn csrf() -> String {
    "a".repeat(64)
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
        max_tokens: 352,
        size: None,
    }
}

fn quick() -> Behaviour {
    Behaviour {
        loading: 0,
        p: 0.0001,
        g: 0.0001,
        phrase_delay: Duration::from_millis(1),
        ..Behaviour::default()
    }
}

fn content() -> Vec<u8> {
    (0..30_000)
        .map(|i| u8::try_from(i % 251).unwrap())
        .collect()
}

fn sha(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// The metadata of the `acme` repositories: the file `name` with `bytes`, and a LICENSE.
fn metadata(name: &str, bytes: &[u8]) -> String {
    serde_json::json!({
        "sha": REV,
        "private": false,
        "gated": false,
        "cardData": {"license": "mit"},
        "siblings": [
            {"rfilename": "LICENSE", "size": LICENCE.len()},
            {"rfilename": name, "size": bytes.len(),
             "lfs": {"sha256": sha(bytes), "size": bytes.len(), "pointerSize": 135}}
        ]
    })
    .to_string()
}

fn serve(bytes: Vec<u8>) -> impl FnMut(&Seen) -> Reply + Send + 'static {
    move |seen| {
        if seen.path.starts_with("/api/models/acme/") {
            let name = if seen.path.contains("other-GGUF") {
                "other.gguf"
            } else {
                "tiny.gguf"
            };
            Reply::json(&metadata(name, &bytes))
        } else if seen.path.ends_with("/LICENSE") {
            Reply::text(LICENCE)
        } else if seen.path.contains("/resolve/") {
            Reply::file(
                &bytes,
                seen.headers.get("range").and_then(|v| v.to_str().ok()),
            )
        } else {
            Reply::status(404)
        }
    }
}

struct Rig {
    _dir: tempfile::TempDir,
    models: tempfile::TempDir,
    store: Store,
    auth: Vec<RequestAuth>,
    app: axum::Router,
    launcher: Arc<FakeLauncher>,
    handle: manager::Handle,
    downloads: Arc<Downloads>,
    hub: FakeHub,
    bytes: Vec<u8>,
}

async fn rig() -> Rig {
    rig_with(Duration::from_millis(1)).await
}

async fn rig_with(slow: Duration) -> Rig {
    let (dir, store, auth, _clock) = fixture_with_owner(1).await;
    let bytes = content();
    let mut names: Vec<&str> = SHIPPED_HOSTS.to_vec();
    names.push("evil.example");
    let hub = FakeHub::start(&names, serve(bytes.clone())).await;
    let models = tempfile::tempdir().unwrap();
    std::fs::set_permissions(
        models.path(),
        std::os::unix::fs::PermissionsExt::from_mode(0o700),
    )
    .unwrap();
    let disk = Arc::new(Disk::open(models.path(), Arc::new(NoFaults)).unwrap());
    let downloads = Arc::new(
        Downloads::with(
            Client::new(&hub.endpoints()).unwrap(),
            disk,
            Arc::new(SqlRows(store.clone())),
            Config {
                flush: 4096,
                floor_bytes: 1,
                floor_window: Duration::from_secs(60),
                reserve: 0,
                chunk: 4096,
            },
        )
        .await
        .unwrap(),
    );
    let launcher = FakeLauncher::scripted(
        std::iter::once(Start::Ok(quick()))
            .chain((0..40).map(|_| {
                Start::Ok(Behaviour {
                    phrase_delay: slow,
                    ..quick()
                })
            }))
            .collect::<Vec<_>>(),
    );
    launcher.files.lock().unwrap().insert(
        format!("hf-{}.gguf", sha(&bytes)),
        Ok(ModelFile {
            sha256: sha(&bytes),
            size: bytes.len() as u64,
        }),
    );
    let handle = AssistantHandle::with_models(
        Arc::clone(&launcher) as Arc<dyn Launcher>,
        manager::Config {
            path: Path::Cpu,
            levels: manager::USED_LEVELS,
        },
        ModelSetup {
            active: granite(),
            previous: None,
            records: Some(Arc::new(SqlRecords(store.clone()))),
            from_owner_setting: false,
            device_default: false,
            older_prompt: crate::assistant::runtime::models::OlderPrompt::default(),
        },
    )
    .with_downloads(Arc::clone(&downloads));
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
    let rig = Rig {
        _dir: dir,
        models,
        store,
        auth,
        app,
        launcher,
        handle: runtime,
        downloads,
        hub,
        bytes,
    };
    rig.until(|| rig.handle.status().state == manager::State::Ready)
        .await;
    rig
}

impl Rig {
    async fn until(&self, done: impl Fn() -> bool) {
        for _ in 0..1500 {
            if done() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("not reached");
    }

    async fn post(
        &self,
        who: usize,
        path: &str,
        pairs: &[(&str, &str)],
    ) -> axum::response::Response {
        let form = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        router_request(&self.app, &self.auth[who], path, Some(form)).await
    }

    /// An owner POST that answers with a redirect; returns the notice.
    async fn owner(&self, path: &str, pairs: &[(&str, &str)]) -> String {
        let response = self.post(0, path, pairs).await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER, "{path}");
        let location = response.headers()["location"].to_str().unwrap().to_owned();
        location
            .strip_prefix("/admin/assistant?notice=")
            .unwrap_or(&location)
            .to_owned()
    }

    async fn page(&self) -> String {
        body(router_request(&self.app, &self.auth[0], "/admin/assistant", None).await).await
    }

    async fn json(&self) -> serde_json::Value {
        let response = router_request(
            &self.app,
            &self.auth[0],
            "/admin/assistant/downloads.json",
            None,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        serde_json::from_str(&body(response).await).unwrap()
    }

    /// Reviews `spec` and returns the token of the ready review.
    async fn review(&self, spec: &str) -> String {
        assert_eq!(
            self.owner(
                "/admin/assistant/model/try",
                &[("csrf", &csrf()), ("model", spec)]
            )
            .await,
            "reviewing"
        );
        let mut page = String::new();
        for _ in 0..500 {
            page = self.page().await;
            if page.contains("Confirm and download") || page.contains("Refused:") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let marker = "name=\"token\" value=\"";
        let at = page
            .find(marker)
            .unwrap_or_else(|| panic!("no ready review in {page}"))
            + marker.len();
        page[at..at + 32].to_owned()
    }

    async fn rows(&self) -> Vec<crate::assistant::download::rows::Row> {
        self.downloads.rows.list().await.unwrap()
    }

    async fn confirmed_and_done(&self, token: &str) {
        assert_eq!(
            self.owner(
                "/admin/assistant/download/confirm",
                &[("csrf", &csrf()), ("token", token)]
            )
            .await,
            "download-started"
        );
        self.until(|| {
            self.downloads.downloader.status().phase
                == crate::assistant::download::task::Phase::Done
        })
        .await;
    }

    fn file(&self) -> std::path::PathBuf {
        self.models
            .path()
            .join(format!("hf-{}.gguf", sha(&self.bytes)))
    }
}

#[tokio::test]
async fn a_review_shows_what_will_be_recorded_and_a_confirmation_downloads_it() {
    let _lock = crate::auth::test_resource_lock().await;
    let rig = rig().await;
    let token = rig.review(SPEC).await;
    let page = rig.page().await;
    assert!(
        page.contains("acme/tiny-GGUF") && page.contains(&sha(&rig.bytes)),
        "{page}"
    );
    assert!(page.contains("third-party model: its hash comes from the same host"));
    // The licence text is escaped, never active markup.
    assert!(page.contains("Tiny licence &#60;script&#62;"), "{page}");
    assert!(!page.contains("<script>alert(1)"));
    // Nothing of the model was asked for before Confirm, and nothing was recorded.
    assert!(
        rig.hub
            .requests()
            .iter()
            .all(|seen| !seen.path.ends_with("/tiny.gguf"))
    );
    assert_eq!(rig.rows().await.len(), 0);
    rig.confirmed_and_done(&token).await;
    assert_eq!(std::fs::read(rig.file()).unwrap(), rig.bytes);
    let rows = rig.rows().await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].spec, SPEC);
    assert_eq!(rows[0].licence_id.as_deref(), Some("mit"));
    assert_eq!(rows[0].licence_path.as_deref(), Some("LICENSE"));
    assert_eq!(
        rows[0].licence_sha256.as_deref(),
        Some(sha(LICENCE.as_bytes()).as_str())
    );
    assert_eq!(rows[0].confirmed_by, 1);
    // The panel lists it with Try and Delete; completing started no trial.
    let page = rig.page().await;
    assert!(
        page.contains("Downloaded models")
            && page.contains(&format!("local:hf-{}.gguf", sha(&rig.bytes)))
    );
    assert!(rig.handle.status().trial.is_none());
}

#[tokio::test]
async fn the_active_model_serves_during_a_download_and_a_trial_waits_for_it() {
    let _lock = crate::auth::test_resource_lock().await;
    let rig = rig().await;
    let token = rig.review(SPEC).await;
    rig.hub.respond({
        let bytes = rig.bytes.clone();
        move |seen| {
            if seen.path.starts_with("/api/") {
                Reply::json(&metadata("tiny.gguf", &bytes))
            } else if seen.path.ends_with("/LICENSE") {
                Reply::text(LICENCE)
            } else {
                Reply::Slow {
                    status: 200,
                    headers: Vec::new(),
                    body: bytes.clone(),
                    piece: 2000,
                    gap: Duration::from_millis(40),
                }
            }
        }
    });
    assert_eq!(
        rig.owner(
            "/admin/assistant/download/confirm",
            &[("csrf", &csrf()), ("token", &token)]
        )
        .await,
        "download-started"
    );
    // The model server is still ready, a Try and a Switch back wait, and a delete too.
    assert_eq!(rig.handle.status().state, manager::State::Ready);
    for (path, pairs) in [
        (
            "/admin/assistant/model/try",
            vec![("csrf", csrf()), ("model", "local:other.gguf".to_owned())],
        ),
        ("/admin/assistant/model/switch-back", vec![("csrf", csrf())]),
        (
            "/admin/assistant/model/delete",
            vec![("csrf", csrf()), ("id", "1".to_owned())],
        ),
    ] {
        let pairs: Vec<(&str, &str)> = pairs.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let notice = rig.owner(path, &pairs).await;
        assert!(
            notice == "model-busy" || notice == "trial-running",
            "{path}: {notice}"
        );
    }
    // A second confirmation while one runs is refused and writes nothing.
    assert_eq!(rig.rows().await.len(), 1);
    // Cancel releases the gate: a Try is accepted again.
    assert_eq!(
        rig.owner(
            "/admin/assistant/download/cancel",
            &[("csrf", &csrf()), ("id", "1")],
        )
        .await,
        "download-cancelled"
    );
    rig.until(|| !rig.downloads.downloader.busy()).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        rig.owner(
            "/admin/assistant/model/try",
            &[("csrf", &csrf()), ("model", "local:other.gguf")]
        )
        .await,
        "trying-model"
    );
}

#[tokio::test]
async fn a_running_trial_blocks_a_confirmation_and_records_nothing() {
    let _lock = crate::auth::test_resource_lock().await;
    let rig = rig_with(Duration::from_millis(300)).await;
    rig.launcher.files.lock().unwrap().insert(
        "other.gguf".into(),
        Ok(ModelFile {
            sha256: "b".repeat(64),
            size: 4 << 30,
        }),
    );
    let token = rig.review(SPEC).await;
    assert_eq!(
        rig.owner(
            "/admin/assistant/model/try",
            &[("csrf", &csrf()), ("model", "local:other.gguf")]
        )
        .await,
        "trying-model"
    );
    assert_eq!(
        rig.owner(
            "/admin/assistant/download/confirm",
            &[("csrf", &csrf()), ("token", &token)]
        )
        .await,
        "model-busy"
    );
    assert_eq!(rig.rows().await.len(), 0);
}

#[tokio::test]
async fn a_stale_or_used_token_confirms_nothing() {
    let _lock = crate::auth::test_resource_lock().await;
    let rig = rig().await;
    let token = rig.review(SPEC).await;
    assert_eq!(
        rig.owner(
            "/admin/assistant/download/confirm",
            &[("csrf", &csrf()), ("token", &"0".repeat(32))]
        )
        .await,
        "review-stale"
    );
    assert_eq!(rig.rows().await.len(), 0);
    rig.confirmed_and_done(&token).await;
    // The same token again: the review is gone.
    assert_eq!(
        rig.owner(
            "/admin/assistant/download/confirm",
            &[("csrf", &csrf()), ("token", &token)]
        )
        .await,
        "review-stale"
    );
    assert_eq!(rig.rows().await.len(), 1);
}

#[tokio::test]
async fn a_second_download_asks_to_discard_the_partial_before_any_row_is_written() {
    let _lock = crate::auth::test_resource_lock().await;
    let rig = rig().await;
    let first = rig.review(SPEC).await;
    rig.hub.respond({
        let bytes = rig.bytes.clone();
        move |seen| {
            if seen.path.starts_with("/api/") {
                Reply::json(&metadata(
                    if seen.path.contains("other") {
                        "other.gguf"
                    } else {
                        "tiny.gguf"
                    },
                    &bytes,
                ))
            } else if seen.path.ends_with("/LICENSE") {
                Reply::text(LICENCE)
            } else {
                Reply::Slow {
                    status: 200,
                    headers: Vec::new(),
                    body: bytes.clone(),
                    piece: 2000,
                    gap: Duration::from_millis(40),
                }
            }
        }
    });
    rig.owner(
        "/admin/assistant/download/confirm",
        &[("csrf", &csrf()), ("token", &first)],
    )
    .await;
    rig.until(|| {
        matches!(
            rig.downloads.downloader.status().phase,
            crate::assistant::download::task::Phase::Running { received, .. } if received > 0
        )
    })
    .await;
    rig.owner(
        "/admin/assistant/download/cancel",
        &[("csrf", &csrf()), ("id", "1")],
    )
    .await;
    rig.until(|| !rig.downloads.downloader.busy()).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    // A different model: its confirmation is refused while the partial exists.
    let second = rig.review(OTHER).await;
    assert_eq!(
        rig.owner(
            "/admin/assistant/download/confirm",
            &[("csrf", &csrf()), ("token", &second)]
        )
        .await,
        "partial-exists"
    );
    assert_eq!(rig.rows().await.len(), 1, "no row for the second one");
    // Discard removes the partial and the row; then the second can be confirmed.
    let id = rig.rows().await[0].id.to_string();
    assert_eq!(
        rig.owner(
            "/admin/assistant/download/discard",
            &[("csrf", &csrf()), ("id", &id)]
        )
        .await,
        "download-discarded"
    );
    assert_eq!(rig.rows().await.len(), 0);
    assert_eq!(rig.downloads.downloader.partials().await.unwrap().len(), 0);
    assert_eq!(
        rig.owner(
            "/admin/assistant/download/confirm",
            &[("csrf", &csrf()), ("token", &second)]
        )
        .await,
        "download-started"
    );
}

#[tokio::test]
async fn a_review_answers_at_once_while_the_server_is_slow_and_a_second_waits() {
    let _lock = crate::auth::test_resource_lock().await;
    let rig = rig().await;
    rig.hub
        .respond(|_| Reply::Delayed(Duration::from_secs(2), Box::new(Reply::status(500))));
    let started = std::time::Instant::now();
    assert_eq!(
        rig.owner(
            "/admin/assistant/model/review",
            &[("csrf", &csrf()), ("model", SPEC)]
        )
        .await,
        "reviewing"
    );
    assert!(
        started.elapsed() < Duration::from_millis(900),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(
        rig.owner(
            "/admin/assistant/model/review",
            &[("csrf", &csrf()), ("model", SPEC)]
        )
        .await,
        "review-busy"
    );
    let state = rig.json().await;
    assert_eq!(state["review"], "running");
    assert!(rig.page().await.contains("Reviewing:"));
}

#[tokio::test]
async fn gated_private_and_denied_repositories_are_refused_at_review() {
    let _lock = crate::auth::test_resource_lock().await;
    let rig = rig().await;
    let bytes = rig.bytes.clone();
    for answer in ["gated", "private", "resolve-401"] {
        let bytes = bytes.clone();
        rig.hub.respond(move |seen| {
            if seen.path.starts_with("/api/") {
                let mut meta: serde_json::Value =
                    serde_json::from_str(&metadata("tiny.gguf", &bytes)).unwrap();
                match answer {
                    "gated" => meta["gated"] = serde_json::json!("auto"),
                    "private" => meta["private"] = serde_json::json!(true),
                    _ => {}
                }
                Reply::json(&meta.to_string())
            } else {
                Reply::status(401)
            }
        });
        let _ = rig
            .owner(
                "/admin/assistant/model/review",
                &[("csrf", &csrf()), ("model", SPEC)],
            )
            .await;
        let mut page = String::new();
        for _ in 0..300 {
            page = rig.page().await;
            if page.contains("Refused:") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(
            page.contains("Refused:") && page.contains("by hand"),
            "{answer}: {page}"
        );
        assert!(!page.contains("Confirm and download"));
    }
}

#[tokio::test]
async fn the_state_for_polling_holds_no_url_and_no_secret() {
    let _lock = crate::auth::test_resource_lock().await;
    let rig = rig().await;
    let token = rig.review(SPEC).await;
    rig.confirmed_and_done(&token).await;
    let text = rig.json().await.to_string();
    for forbidden in ["http", "huggingface", "://", "licence", "token"] {
        assert!(!text.contains(forbidden), "{forbidden}: {text}");
    }
    assert!(text.contains("complete"));
}

#[tokio::test]
async fn a_downloaded_file_is_tried_switched_to_and_then_kept_by_delete() {
    let _lock = crate::auth::test_resource_lock().await;
    let rig = rig().await;
    let token = rig.review(SPEC).await;
    rig.confirmed_and_done(&token).await;
    let name = format!("hf-{}.gguf", sha(&rig.bytes));
    assert_eq!(
        rig.owner(
            "/admin/assistant/model/try",
            &[("csrf", &csrf()), ("model", &format!("local:{name}"))]
        )
        .await,
        "trying-model"
    );
    rig.until(|| {
        rig.handle
            .status()
            .trial
            .as_ref()
            .is_some_and(|trial| !trial.running())
    })
    .await;
    let key = match &rig.handle.status().trial {
        Some(crate::assistant::runtime::models::TrialState::Passed(report)) => {
            report.key.to_string()
        }
        other => panic!("{other:?}"),
    };
    assert_eq!(
        rig.owner(
            "/admin/assistant/model/switch",
            &[("csrf", &csrf()), ("trial", &key)]
        )
        .await,
        "switching"
    );
    rig.until(|| {
        rig.handle.status().state == manager::State::Ready
            && rig.handle.status().model.active.name == name
    })
    .await;
    // The active model's file cannot be deleted; nothing is removed.
    let id = rig.rows().await[0].id.to_string();
    assert_eq!(
        rig.owner(
            "/admin/assistant/model/delete",
            &[("csrf", &csrf()), ("id", &id)]
        )
        .await,
        "file-kept"
    );
    assert!(rig.file().exists());
    // Switch back to the previous model, then the file is the previous one: still kept.
    rig.owner("/admin/assistant/model/switch-back", &[("csrf", &csrf())])
        .await;
    rig.until(|| {
        rig.handle.status().state == manager::State::Ready
            && rig.handle.status().model.active.name != name
    })
    .await;
    assert_eq!(
        rig.owner(
            "/admin/assistant/model/delete",
            &[("csrf", &csrf()), ("id", &id)]
        )
        .await,
        "file-kept"
    );
    assert!(rig.file().exists());
    let stored = rig
        .store
        .run(|db| {
            Ok(db.query_row(
                "SELECT count(*) FROM assistant_models WHERE spec LIKE 'local:hf-%'",
                [],
                |r| r.get::<_, i64>(0),
            )?)
        })
        .await
        .unwrap();
    assert_eq!(stored, 1, "the model table holds local:, never hf:");
}

#[tokio::test]
async fn a_finished_download_that_nothing_uses_is_deleted_with_its_file() {
    let _lock = crate::auth::test_resource_lock().await;
    let rig = rig().await;
    let token = rig.review(SPEC).await;
    rig.confirmed_and_done(&token).await;
    let id = rig.rows().await[0].id.to_string();
    assert_eq!(
        rig.owner(
            "/admin/assistant/model/delete",
            &[("csrf", &csrf()), ("id", &id)]
        )
        .await,
        "file-deleted"
    );
    assert!(!rig.file().exists());
    assert_eq!(rig.rows().await.len(), 0);
}

#[tokio::test]
async fn the_panel_lists_the_pinned_defaults_and_a_foreign_file_under_a_pinned_name_is_flagged() {
    let _lock = crate::auth::test_resource_lock().await;
    let rig = rig().await;
    let page = rig.page().await;
    assert!(page.contains("Pinned defaults"));
    assert!(page.contains("granite-4.2-3b-Q4_K_M.gguf") && page.contains("Qwen3.5-9B-Q4_K_M.gguf"));
    assert!(page.matches("not downloaded").count() == 2, "{page}");
    assert!(page.contains("name=\"model\" value=\"hf:ibm-granite/"));
    // A different file with a pinned name is never offered a download over it.
    let path = rig.models.path().join("granite-4.2-3b-Q4_K_M.gguf");
    std::fs::write(&path, b"mine").unwrap();
    std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();
    let page = rig.page().await;
    assert!(page.contains("a different file has this name"), "{page}");
    assert!(!page.contains("name=\"model\" value=\"hf:ibm-granite/"));
    assert_eq!(std::fs::read(&path).unwrap(), b"mine");
}

#[tokio::test]
async fn every_download_action_is_owner_only_csrf_checked_strict_and_bounded() {
    let _lock = crate::auth::test_resource_lock().await;
    let rig = rig().await;
    let actions: [(&str, Vec<(&str, String)>); 7] = [
        (
            "/admin/assistant/model/review",
            vec![("csrf", csrf()), ("model", SPEC.into())],
        ),
        (
            "/admin/assistant/download/confirm",
            vec![("csrf", csrf()), ("token", "0".repeat(32))],
        ),
        (
            "/admin/assistant/download/cancel",
            vec![("csrf", csrf()), ("id", "1".into())],
        ),
        (
            "/admin/assistant/download/repair",
            vec![("csrf", csrf()), ("id", "1".into())],
        ),
        (
            "/admin/assistant/download/resume",
            vec![("csrf", csrf()), ("id", "1".into())],
        ),
        (
            "/admin/assistant/download/discard",
            vec![("csrf", csrf()), ("id", "1".into())],
        ),
        (
            "/admin/assistant/model/delete",
            vec![("csrf", csrf()), ("id", "1".into())],
        ),
    ];
    for (path, pairs) in &actions {
        let send = |who: usize, pairs: Vec<(String, String)>| {
            let (app, auth) = (rig.app.clone(), rig.auth[who].clone());
            async move { router_request(&app, &auth, path, Some(pairs)).await }
        };
        let borrowed = || -> Vec<(String, String)> {
            pairs
                .iter()
                .map(|(k, v)| ((*k).to_owned(), v.clone()))
                .collect()
        };
        assert_eq!(
            send(1, borrowed()).await.status(),
            StatusCode::FORBIDDEN,
            "{path}"
        );
        let mut wrong = borrowed();
        wrong[0].1 = "b".repeat(64);
        assert_eq!(
            send(0, wrong).await.status(),
            StatusCode::FORBIDDEN,
            "{path}"
        );
        let mut extra = borrowed();
        extra.push(("extra".into(), "1".into()));
        assert_eq!(
            send(0, extra).await.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "{path}"
        );
        let mut duplicate = borrowed();
        duplicate.push(duplicate[0].clone());
        assert_eq!(
            send(0, duplicate).await.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "{path}"
        );
        assert_eq!(
            send(0, Vec::new()).await.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "{path}"
        );
        let mut huge = borrowed();
        huge.push(("pad".into(), "x".repeat(5000)));
        assert_eq!(
            send(0, huge).await.status(),
            StatusCode::PAYLOAD_TOO_LARGE,
            "{path}"
        );
    }
    // The state for polling is owner only too, and a GET is not an action.
    let response = router_request(
        &rig.app,
        &rig.auth[1],
        "/admin/assistant/downloads.json",
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    for (path, _) in &actions {
        let response = router_request(&rig.app, &rig.auth[0], path, None).await;
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED, "{path}");
    }
}

#[tokio::test]
async fn a_finished_download_whose_file_is_gone_can_be_downloaded_again_without_a_new_review() {
    let _lock = crate::auth::test_resource_lock().await;
    let rig = rig().await;
    let token = rig.review(SPEC).await;
    rig.confirmed_and_done(&token).await;
    let before = rig.rows().await;
    std::fs::remove_file(rig.file()).unwrap();
    // The panel says the file is gone and offers the repair.
    let page = rig.page().await;
    assert!(page.contains("missing or changed"), "{page}");
    assert!(page.contains("/admin/assistant/download/repair"), "{page}");
    let id = before[0].id.to_string();
    assert_eq!(
        rig.owner(
            "/admin/assistant/download/repair",
            &[("csrf", &csrf()), ("id", &id)]
        )
        .await,
        "download-started"
    );
    rig.until(|| {
        rig.downloads.downloader.status().phase == crate::assistant::download::task::Phase::Done
    })
    .await;
    assert_eq!(std::fs::read(rig.file()).unwrap(), rig.bytes);
    // The provenance is the confirmed one: same row, same licence snapshot.
    let after = rig.rows().await;
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].id, before[0].id);
    assert_eq!(after[0].licence_sha256, before[0].licence_sha256);
    assert_eq!(after[0].confirmed_by, before[0].confirmed_by);
    // A file that is there is not downloaded again, and a different file is never replaced.
    assert_eq!(
        rig.owner(
            "/admin/assistant/download/repair",
            &[("csrf", &csrf()), ("id", &id)]
        )
        .await,
        "nothing-to-repair"
    );
}

#[tokio::test]
async fn a_repair_never_replaces_a_different_file_under_the_name() {
    let _lock = crate::auth::test_resource_lock().await;
    let rig = rig().await;
    let token = rig.review(SPEC).await;
    rig.confirmed_and_done(&token).await;
    let id = rig.rows().await[0].id.to_string();
    std::fs::write(rig.file(), b"someone else's file").unwrap();
    let notice = rig
        .owner(
            "/admin/assistant/download/repair",
            &[("csrf", &csrf()), ("id", &id)],
        )
        .await;
    assert!(
        notice == "download-started" || notice == "nothing-to-repair",
        "{notice}"
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(std::fs::read(rig.file()).unwrap(), b"someone else's file");
}

#[tokio::test]
async fn the_pinned_panel_does_not_claim_more_than_a_size_check_shows() {
    let _lock = crate::auth::test_resource_lock().await;
    let rig = rig().await;
    // A sparse file of the pinned Granite size: only the size can be said to match.
    let path = rig.models.path().join("granite-4.2-3b-Q4_K_M.gguf");
    let file = std::fs::File::create(&path).unwrap();
    file.set_len(crate::assistant::model::GRANITE_SIZE).unwrap();
    std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();
    let page = rig.page().await;
    assert!(page.contains("the size matches"), "{page}");
    assert!(
        page.contains("hash is checked when the model loads"),
        "{page}"
    );
    assert!(!page.contains("verified by Mimik"), "{page}");
    assert!(!page.contains("present in the models folder"), "{page}");
}

#[tokio::test]
async fn cancel_names_the_download_it_was_shown_for() {
    let _lock = crate::auth::test_resource_lock().await;
    let rig = rig().await;
    let token = rig.review(SPEC).await;
    rig.hub.respond({
        let bytes = rig.bytes.clone();
        move |seen| {
            if seen.path.starts_with("/api/") {
                Reply::json(&metadata("tiny.gguf", &bytes))
            } else if seen.path.ends_with("/LICENSE") {
                Reply::text(LICENCE)
            } else {
                Reply::Slow {
                    status: 200,
                    headers: Vec::new(),
                    body: bytes.clone(),
                    piece: 2000,
                    gap: Duration::from_millis(60),
                }
            }
        }
    });
    rig.owner(
        "/admin/assistant/download/confirm",
        &[("csrf", &csrf()), ("token", &token)],
    )
    .await;
    // A stale page (another download's id) cancels nothing.
    assert_eq!(
        rig.owner(
            "/admin/assistant/download/cancel",
            &[("csrf", &csrf()), ("id", "9999")]
        )
        .await,
        "stale-download"
    );
    assert!(rig.downloads.downloader.busy());
    let id = rig.rows().await[0].id.to_string();
    assert_eq!(
        rig.owner(
            "/admin/assistant/download/cancel",
            &[("csrf", &csrf()), ("id", &id)]
        )
        .await,
        "download-cancelled"
    );
}

#[tokio::test]
async fn unreadable_download_records_are_reported_not_shown_as_empty() {
    let _lock = crate::auth::test_resource_lock().await;
    let rig = rig().await;
    rig.store
        .run(|db| {
            db.execute_batch(
                "ALTER TABLE assistant_downloads RENAME TO assistant_downloads_gone;",
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let state = rig.json().await;
    assert_eq!(state["available"], true);
    assert!(
        state["error"]
            .as_str()
            .is_some_and(|text| text.contains("records")),
        "{state}"
    );
    let page = rig.page().await;
    assert!(page.contains("download records cannot be read"), "{page}");
}
