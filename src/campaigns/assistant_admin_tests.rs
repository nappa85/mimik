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

fn restart_form(csrf: &str) -> Option<Vec<(String, String)>> {
    Some(vec![("csrf".into(), csrf.into())])
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
        router_request(&app, &auth[0], "/admin/assistant/restart", restart_form(&"a".repeat(64)))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn restart_is_owner_only_csrf_checked_and_never_doubled() {
    let _lock = crate::auth::test_resource_lock().await;
    let (_dir, store, auth, _clock) = fixture_with_owner(1).await;
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let preflights = Arc::new(AtomicI64::new(0));
    let handle = AssistantHandle::with_launcher(
        Arc::new(Waiting(Arc::clone(&gate), Arc::clone(&preflights))),
        manager::Config {
            path: Path::Cpu,
            max_tokens: 352,
        },
    );
    let app = crate::auth::app_with_options(
        store.clone(),
        crate::auth::AuthOptions {
            assistant: Some(handle.clone()),
            ..crate::auth::AuthOptions::default()
        },
    )
    .await
    .unwrap();
    let csrf = "a".repeat(64);
    let page = body(router_request(&app, &auth[0], "/admin/assistant", None).await).await;
    assert!(page.contains("<strong>Starting</strong>"), "{page}");
    assert!(page.contains("Level") || page.contains("No level is active"));
    // Not the owner; a wrong token.
    assert_eq!(
        router_request(&app, &auth[1], "/admin/assistant/restart", restart_form(&csrf))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        router_request(&app, &auth[0], "/admin/assistant/restart", restart_form(&"b".repeat(64)))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    // The first preflight is still waiting; a Restart is accepted once, the second press
    // while it runs starts nothing.
    let first = router_request(&app, &auth[0], "/admin/assistant/restart", restart_form(&csrf)).await;
    assert_eq!(first.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        first.headers()["location"],
        "/admin/assistant?notice=restarting"
    );
    let second = router_request(&app, &auth[0], "/admin/assistant/restart", restart_form(&csrf)).await;
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
    let page = body(router_request(&app, &auth[0], "/admin/assistant?notice=restarting", None).await).await;
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
    let refused = router_request(&app, &auth[0], "/admin/assistant/restart", restart_form(&csrf)).await;
    assert!(
        matches!(refused.status(), StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN),
        "{}",
        refused.status()
    );
    assert_eq!(preflights.load(Ordering::SeqCst), 2);
}
