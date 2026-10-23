//! `/admin/assistant` through the full auth router: owner only, CSRF-checked Restart, no
//! second transition while one runs, a revoked session refused.
use super::*;
use crate::assistant::AssistantHandle;
use crate::assistant::runtime::manager::{self, Child, Fail, Launcher};
use crate::assistant::runtime::{Level, Path};
use axum::http::StatusCode;

/// A launcher whose preflight waits for a permit, then refuses: the runtime stays
/// "starting" while it waits, then "unavailable" with the reason.
struct Waiting(Arc<tokio::sync::Semaphore>, Arc<AtomicI64>);

#[async_trait::async_trait]
impl Launcher for Waiting {
    async fn preflight(&self, _: Path) -> Result<(), String> {
        self.1.fetch_add(1, Ordering::SeqCst);
        self.0.acquire().await.unwrap().forget();
        Err("test: no model server here".into())
    }
    async fn probe(&self, _: Path) -> Vec<(Level, Result<(), String>)> {
        Vec::new()
    }
    async fn start(&self, _: u64, _: Level, _: Path) -> Result<Arc<dyn Child>, Fail> {
        Err(Fail::Other("never".into()))
    }
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

fn restart_form(csrf: &str) -> Vec<(String, String)> {
    vec![("csrf".to_owned(), csrf.to_owned())]
}

#[tokio::test]
async fn the_assistant_page_is_owner_only_and_says_off_without_the_assistant() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture_with_owner(1).await;
    let app = crate::auth::app(store.clone()).await.unwrap();
    assert_eq!(
        router_request(&app, &auth[1], "/admin/assistant", None)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    let page = router_request(&app, &auth[0], "/admin/assistant", None).await;
    assert_eq!(page.status(), StatusCode::OK);
    let page = body(page).await;
    assert!(page.contains("<strong>Off</strong>"), "{page}");
    // No assistant, nothing to restart.
    assert_eq!(
        router_request(
            &app,
            &auth[0],
            "/admin/assistant/restart",
            Some(restart_form(&"a".repeat(64)))
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
}

struct Assisted {
    _dir: tempfile::TempDir,
    store: Store,
    auth: Vec<RequestAuth>,
    app: axum::Router,
    gate: Arc<tokio::sync::Semaphore>,
    preflights: Arc<AtomicI64>,
}

async fn assisted() -> Assisted {
    let (dir, store, auth, _clock) = fixture_with_owner(1).await;
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let preflights = Arc::new(AtomicI64::new(0));
    let handle = AssistantHandle::with_launcher(
        Arc::new(Waiting(Arc::clone(&gate), Arc::clone(&preflights))),
        manager::Config {
            path: Path::Cpu,
            max_tokens: 352,
            levels: manager::USED_LEVELS,
        },
    );
    let app = crate::auth::app_with_options(
        store.clone(),
        crate::auth::AuthOptions {
            assistant: Some(handle),
            ..crate::auth::AuthOptions::default()
        },
    )
    .await
    .unwrap();
    Assisted {
        _dir: dir,
        store,
        auth,
        app,
        gate,
        preflights,
    }
}

#[tokio::test]
async fn restart_is_owner_only_and_csrf_checked() {
    let _lock = crate::auth::test_resource_lock().await;
    let Assisted {
        _dir: _keep,
        app,
        auth,
        ..
    } = assisted().await;
    let page = body(router_request(&app, &auth[0], "/admin/assistant", None).await).await;
    assert!(page.contains("<strong>Starting</strong>"), "{page}");
    assert!(page.contains("llama.cpp b11609"), "{page}");
    assert_eq!(
        router_request(&app, &auth[1], "/admin/assistant", None)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        router_request(
            &app,
            &auth[1],
            "/admin/assistant/restart",
            Some(restart_form(&"a".repeat(64)))
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        router_request(
            &app,
            &auth[0],
            "/admin/assistant/restart",
            Some(restart_form(&"b".repeat(64)))
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn a_second_restart_while_one_runs_starts_nothing_and_a_revoked_session_is_refused() {
    let _lock = crate::auth::test_resource_lock().await;
    let Assisted {
        _dir: _keep,
        store,
        auth,
        app,
        gate,
        preflights,
    } = assisted().await;
    let csrf = "a".repeat(64);
    // The first preflight is still waiting; a Restart is accepted once, the second press
    // while it runs starts nothing.
    let first = router_request(
        &app,
        &auth[0],
        "/admin/assistant/restart",
        Some(restart_form(&csrf)),
    )
    .await;
    assert_eq!(first.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        first.headers()["location"],
        "/admin/assistant?notice=restarting"
    );
    let second = router_request(
        &app,
        &auth[0],
        "/admin/assistant/restart",
        Some(restart_form(&csrf)),
    )
    .await;
    assert_eq!(
        second.headers()["location"],
        "/admin/assistant?notice=already-restarting"
    );
    gate.add_permits(1);
    // The Restart runs once the first attempt ends: one more preflight, not two.
    for _ in 0..200 {
        if preflights.load(Ordering::SeqCst) >= 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    gate.add_permits(1);
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert_eq!(preflights.load(Ordering::SeqCst), 2);
    let page =
        body(router_request(&app, &auth[0], "/admin/assistant?notice=restarting", None).await)
            .await;
    assert!(page.contains("test: no model server here"), "{page}");
    assert!(page.contains("Restarting: the model server"));
    // A session revoked meanwhile is refused.
    store
        .run(|db| {
            db.execute("UPDATE accounts SET epoch=2 WHERE id=1", [])?;
            Ok(())
        })
        .await
        .unwrap();
    let refused = router_request(
        &app,
        &auth[0],
        "/admin/assistant/restart",
        Some(restart_form(&csrf)),
    )
    .await;
    assert!(
        matches!(
            refused.status(),
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
        ),
        "{}",
        refused.status()
    );
    assert_eq!(preflights.load(Ordering::SeqCst), 2);
}

/// A launcher with no usable GPU and no level 1: the GPU path fails at its probe, the CPU
/// path then has no level, and the runtime is unavailable on the CPU path. From the second
/// GPU probe on, each waits for a permit.
struct NoGpu {
    gate: Arc<tokio::sync::Semaphore>,
    gpu_probes: Arc<AtomicI64>,
}

#[async_trait::async_trait]
impl Launcher for NoGpu {
    async fn preflight(&self, _: Path) -> Result<(), String> {
        Ok(())
    }
    async fn probe(&self, path: Path) -> Vec<(Level, Result<(), String>)> {
        if path == Path::Cpu {
            return vec![(Level::Bubblewrap, Err("test: no bubblewrap".into()))];
        }
        if self.gpu_probes.fetch_add(1, Ordering::SeqCst) >= 1 {
            self.gate.acquire().await.unwrap().forget();
        }
        vec![(Level::Bubblewrap, Err("test: no hardware GPU".into()))]
    }
    async fn start(&self, _: u64, _: Level, _: Path) -> Result<Arc<dyn Child>, Fail> {
        Err(Fail::Other("never".into()))
    }
}

/// A launcher whose GPU probe passes with a device and whose start never ends: the runtime
/// stays on the GPU path, starting.
struct HangingGpu;

#[async_trait::async_trait]
impl Launcher for HangingGpu {
    async fn preflight(&self, _: Path) -> Result<(), String> {
        Ok(())
    }
    async fn probe(&self, _: Path) -> Vec<(Level, Result<(), String>)> {
        vec![(Level::Bubblewrap, Ok(()))]
    }
    fn device(&self) -> Option<manager::DeviceView> {
        Some(manager::DeviceView {
            pci: "0000:0c:00.0".into(),
            node: "renderD128".into(),
            name: "Vulkan0".into(),
            description: "AMD Radeon RX 6650 XT (RADV NAVI23)".into(),
            total_mib: 8176,
        })
    }
    async fn start(&self, _: u64, _: Level, _: Path) -> Result<Arc<dyn Child>, Fail> {
        std::future::pending().await
    }
}

#[tokio::test]
async fn the_gpu_state_says_sandboxed_with_graphics_driver_access_and_names_the_cpu_switch() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture_with_owner(1).await;
    let handle = AssistantHandle::with_launcher(
        Arc::new(HangingGpu),
        manager::Config {
            path: Path::Gpu,
            max_tokens: 352,
            levels: manager::USED_LEVELS,
        },
    );
    let app = crate::auth::app_with_options(
        store.clone(),
        crate::auth::AuthOptions {
            assistant: Some(handle),
            ..crate::auth::AuthOptions::default()
        },
    )
    .await
    .unwrap();
    let mut page = String::new();
    for _ in 0..200 {
        page = body(router_request(&app, &auth[0], "/admin/assistant", None).await).await;
        if page.contains("AMD Radeon RX 6650 XT") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(page.contains("Path: GPU"), "{page}");
    assert!(
        page.contains("Sandboxed, with graphics-driver access:")
            && page.contains("any ioctl on its one render node")
            && page.contains("<code>--assistant-device cpu</code>")
            && page.contains("no access to the user's files")
            && page.contains("read-only system libraries, system driver data")
            && page.contains("the card's sysfs attributes and the CPU topology")
            && !page.contains("no files beyond"),
        "{page}"
    );
}

/// The assistant page shows "Try the GPU again" disabled.
async fn assert_try_gpu_disabled(app: &axum::Router, auth: &crate::auth::RequestAuth) {
    let page = body(router_request(app, auth, "/admin/assistant", None).await).await;
    assert!(
        page.contains("action=\"/admin/assistant/try-gpu\"")
            && page.contains("<button disabled>Try the GPU again</button>"),
        "{page}"
    );
}

#[tokio::test]
async fn try_the_gpu_again_is_owner_only_csrf_checked_and_one_transition() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture_with_owner(1).await;
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let gpu_probes = Arc::new(AtomicI64::new(0));
    let handle = AssistantHandle::with_launcher(
        Arc::new(NoGpu {
            gate: Arc::clone(&gate),
            gpu_probes: Arc::clone(&gpu_probes),
        }),
        manager::Config {
            path: Path::Gpu,
            max_tokens: 352,
            levels: manager::USED_LEVELS,
        },
    );
    let app = crate::auth::app_with_options(
        store.clone(),
        crate::auth::AuthOptions {
            assistant: Some(handle),
            ..crate::auth::AuthOptions::default()
        },
    )
    .await
    .unwrap();
    let mut page = String::new();
    for _ in 0..200 {
        page = body(router_request(&app, &auth[0], "/admin/assistant", None).await).await;
        if page.contains("<strong>Unavailable</strong>") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(page.contains("Path: CPU"), "{page}");
    assert!(
        page.contains("The GPU is not in use: test: no hardware GPU"),
        "{page}"
    );
    assert!(
        page.contains("action=\"/admin/assistant/try-gpu\""),
        "{page}"
    );
    let csrf = "a".repeat(64);
    assert_eq!(
        router_request(
            &app,
            &auth[1],
            "/admin/assistant/try-gpu",
            Some(restart_form(&csrf))
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        router_request(
            &app,
            &auth[0],
            "/admin/assistant/try-gpu",
            Some(restart_form(&"b".repeat(64)))
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(gpu_probes.load(Ordering::SeqCst), 1);
    let first = router_request(
        &app,
        &auth[0],
        "/admin/assistant/try-gpu",
        Some(restart_form(&csrf)),
    )
    .await;
    assert_eq!(first.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        first.headers()["location"],
        "/admin/assistant?notice=trying-gpu"
    );
    // While that transition runs the button is disabled (a press would only be refused).
    assert_try_gpu_disabled(&app, &auth[0]).await;
    let second = router_request(
        &app,
        &auth[0],
        "/admin/assistant/try-gpu",
        Some(restart_form(&csrf)),
    )
    .await;
    assert_eq!(
        second.headers()["location"],
        "/admin/assistant?notice=already-restarting"
    );
    for _ in 0..200 {
        if gpu_probes.load(Ordering::SeqCst) >= 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    gate.add_permits(4);
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert_eq!(gpu_probes.load(Ordering::SeqCst), 2);
}
