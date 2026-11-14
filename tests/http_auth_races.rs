#![cfg(feature = "auth-test-support")]

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
    response::Response,
};
use mimik::{
    AppError, Store,
    accounts::{Accounts, Summary},
    auth::SqliteSessions,
};
use std::{
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tower::ServiceExt;
use tower_sessions::{
    SessionStore,
    session::{Id, Record},
    session_store,
};
use zeroize::Zeroizing;

struct Rendezvous {
    accounts: Accounts,
    entered: tokio::sync::oneshot::Receiver<()>,
    dropped: tokio::sync::oneshot::Receiver<()>,
    release: std::sync::mpsc::Sender<()>,
}
fn rendezvous(accounts: &Accounts) -> Rendezvous {
    let (entered_tx, entered) = tokio::sync::oneshot::channel();
    let (dropped_tx, dropped) = tokio::sync::oneshot::channel();
    let (release, release_rx) = std::sync::mpsc::channel();
    let entered_tx = Mutex::new(Some(entered_tx));
    let dropped_tx = Mutex::new(Some(dropped_tx));
    let release_rx = Mutex::new(release_rx);
    let accounts = accounts.clone().with_auth_test_hooks(
        Arc::new(move || {
            entered_tx.lock().unwrap().take().unwrap().send(()).unwrap();
            release_rx
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(10))
                .unwrap();
        }),
        Arc::new(move || {
            if let Some(sender) = dropped_tx.lock().unwrap().take() {
                let _ = sender.send(());
            }
        }),
    );
    Rendezvous {
        accounts,
        entered,
        dropped,
        release,
    }
}
fn bytes() -> Zeroizing<Vec<u8>> {
    Zeroizing::new(b"synthetic password 123".to_vec())
}
async fn wait(receiver: tokio::sync::oneshot::Receiver<()>) {
    tokio::time::timeout(Duration::from_secs(5), receiver)
        .await
        .unwrap()
        .unwrap();
}

async fn post(app: &Router, cookie: &str, csrf: &str, user: &str) -> Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/login")
                .header(header::HOST, "127.0.0.1:3000")
                .header(header::ORIGIN, "http://127.0.0.1:3000")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(header::COOKIE, cookie)
                .body(Body::from(format!(
                    "login={user}&password=synthetic+password+123&csrf={csrf}"
                )))
                .unwrap(),
        )
        .await
        .unwrap()
}
async fn form(app: &Router) -> (String, String) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/auth/login")
                .header(header::HOST, "127.0.0.1:3000")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let cookie = response.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let text =
        String::from_utf8(to_bytes(response.into_body(), 8192).await.unwrap().to_vec()).unwrap();
    let token = text
        .split("name=\"csrf\" value=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap()
        .to_owned();
    (cookie, token)
}
async fn error(response: Response, status: StatusCode) {
    assert_eq!(response.status(), status);
    assert!(!response.headers().contains_key(header::SET_COOKIE));
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    let text = to_bytes(response.into_body(), 8192).await.unwrap();
    assert!(text.len() < 100);
    assert!(!String::from_utf8_lossy(&text).contains("password 123"));
}

#[derive(Clone)]
enum TransitionBoundary {
    Create,
    Save,
}

#[derive(Clone)]
struct ResetAtBoundary {
    inner: SqliteSessions,
    accounts: Accounts,
    account: Summary,
    output: std::path::PathBuf,
    boundary: TransitionBoundary,
}
impl std::fmt::Debug for ResetAtBoundary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ResetAtBoundary")
    }
}
#[async_trait::async_trait]
impl SessionStore for ResetAtBoundary {
    async fn create(&self, record: &mut Record) -> session_store::Result<()> {
        if record.data.contains_key("principal")
            && matches!(self.boundary, TransitionBoundary::Create)
        {
            self.accounts
                .issue_reset(self.account.id, self.account.epoch, &self.output)
                .await
                .map_err(|_| {
                    session_store::Error::Backend("Synthetic account transition failed.".into())
                })?;
        }
        self.inner.create(record).await
    }
    async fn save(&self, record: &Record) -> session_store::Result<()> {
        if record.data.contains_key("principal")
            && matches!(self.boundary, TransitionBoundary::Save)
        {
            self.accounts
                .issue_reset(self.account.id, self.account.epoch, &self.output)
                .await
                .map_err(|_| {
                    session_store::Error::Backend("Synthetic account transition failed.".into())
                })?;
        }
        self.inner.save(record).await
    }
    async fn load(&self, id: &Id) -> session_store::Result<Option<Record>> {
        self.inner.load(id).await
    }
    async fn delete(&self, id: &Id) -> session_store::Result<()> {
        self.inner.delete(id).await
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn authenticate_and_http_persistence_races_have_deterministic_rendezvous() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let password = dir.path().join("password");
    std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&password)
        .unwrap()
        .write_all(b"synthetic password 123")
        .unwrap();
    let store = Store::open(&dir.path().join("db")).unwrap();
    let accounts = Accounts::new(store.clone());
    let owner = accounts
        .bootstrap("owner".into(), "Owner".into(), &password)
        .await
        .unwrap();
    let token = dir.path().join("invite");
    accounts
        .invite("player".into(), "Player".into(), &token)
        .await
        .unwrap();
    let player = accounts.enroll(&token, &password).await.unwrap();
    verify_to_create_race(&store, &accounts, &player, dir.path()).await;
    second_save_epoch_race(&store, &accounts, &player, dir.path()).await;
    competing_logout_rotation_and_second_save(&store, &dir.path().join("db")).await;
    final_recheck_race(&accounts, &owner, dir.path()).await;
    cancelled_unknown_kdf_and_http_busy(&store, &accounts).await;
    hostile_profile_before_kdf(&store, &accounts, &player, dir.path()).await;
}

#[derive(Clone, Copy)]
enum RequestGate {
    Loaded,
    LoadedThenSave,
    PrincipalSave,
}

#[derive(Clone)]
struct CompetingRequestStore {
    inner: SqliteSessions,
    point: RequestGate,
    entered: Arc<Mutex<Option<tokio::sync::oneshot::Sender<Record>>>>,
    release: Arc<tokio::sync::Semaphore>,
}
impl std::fmt::Debug for CompetingRequestStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CompetingRequestStore")
    }
}
impl CompetingRequestStore {
    async fn rendezvous(&self, record: &Record) {
        let sender = self.entered.lock().unwrap().take();
        if let Some(sender) = sender {
            sender.send(record.clone()).unwrap();
            self.release.acquire().await.unwrap().forget();
        }
    }
}
#[async_trait::async_trait]
impl SessionStore for CompetingRequestStore {
    async fn create(&self, record: &mut Record) -> session_store::Result<()> {
        self.inner.create(record).await
    }
    async fn save(&self, record: &Record) -> session_store::Result<()> {
        if matches!(self.point, RequestGate::PrincipalSave) && record.data.contains_key("principal")
        {
            self.rendezvous(record).await;
        }
        self.inner.save(record).await
    }
    async fn load(&self, id: &Id) -> session_store::Result<Option<Record>> {
        let loaded = self.inner.load(id).await?;
        if !matches!(self.point, RequestGate::PrincipalSave)
            && let Some(record) = &loaded
        {
            self.rendezvous(record).await;
            if matches!(self.point, RequestGate::LoadedThenSave) {
                // A competing request persists the exact captured deadline,
                // not the library's default loaded-session expiry proposal.
                self.inner.save(record).await?;
            }
        }
        Ok(loaded)
    }
    async fn delete(&self, id: &Id) -> session_store::Result<()> {
        self.inner.delete(id).await
    }
}
async fn gated_router(
    store: &Store,
    point: RequestGate,
) -> (
    Router,
    tokio::sync::oneshot::Receiver<Record>,
    Arc<tokio::sync::Semaphore>,
) {
    let (sender, entered) = tokio::sync::oneshot::channel();
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let sessions = CompetingRequestStore {
        inner: SqliteSessions::new(store.clone()),
        point,
        entered: Arc::new(Mutex::new(Some(sender))),
        release: release.clone(),
    };
    (
        mimik::auth::app_with_test_sessions(store.clone(), sessions)
            .await
            .unwrap(),
        entered,
        release,
    )
}
async fn get_session(app: &Router, cookie: &str, path: &str) -> Response {
    app.clone()
        .oneshot(
            Request::builder()
                .uri(path)
                .header(header::HOST, "127.0.0.1:3000")
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}
async fn logout_request(app: &Router, record: &Record) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/logout")
                .header(header::HOST, "127.0.0.1:3000")
                .header(header::ORIGIN, "http://127.0.0.1:3000")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(header::COOKIE, format!("mimik_session={}", record.id))
                .body(Body::from(format!(
                    "csrf={}",
                    record.data["csrf"].as_str().unwrap()
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
}
async fn captured(receiver: tokio::sync::oneshot::Receiver<Record>) -> Record {
    tokio::time::timeout(Duration::from_secs(5), receiver)
        .await
        .unwrap()
        .unwrap()
}
async fn competing_logout_rotation_and_second_save(store: &Store, path: &std::path::Path) {
    let sessions = SqliteSessions::new(store.clone());
    let normal = mimik::auth::app_with_test_sessions(store.clone(), sessions.clone())
        .await
        .unwrap();
    let (cookie, _) = form(&normal).await;
    let (app, entered, release) = gated_router(store, RequestGate::LoadedThenSave).await;
    let old_cookie = cookie.clone();
    let task = tokio::spawn(async move { get_session(&app, &old_cookie, "/auth/login").await });
    let old = captured(entered).await;
    logout_request(&normal, &old).await;
    release.add_permits(1);
    error(task.await.unwrap(), StatusCode::INTERNAL_SERVER_ERROR).await;
    assert!(sessions.load(&old.id).await.unwrap().is_none());

    let (cookie, csrf) = form(&normal).await;
    let response = post(&normal, &cookie, &csrf, "owner").await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let cookie = response.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let old = sessions
        .load(&cookie.trim_start_matches("mimik_session=").parse().unwrap())
        .await
        .unwrap()
        .unwrap();
    let csrf = old.data["csrf"].as_str().unwrap().to_owned();
    let db = rusqlite::Connection::open(path).unwrap();
    let bound_before = db
        .query_row(
            "SELECT count(*) FROM auth_sessions WHERE account_id IS NOT NULL",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap();
    let (app, entered, release) = gated_router(store, RequestGate::Loaded).await;
    let old_cookie = cookie.clone();
    let task = tokio::spawn(async move { post(&app, &old_cookie, &csrf, "owner").await });
    let loaded = captured(entered).await;
    assert_eq!(loaded.id, old.id);
    logout_request(&normal, &loaded).await;
    release.add_permits(1);
    // The captured form was revoked: reject it, then require an actual fresh GET.
    error(task.await.unwrap(), StatusCode::FORBIDDEN).await;
    assert!(sessions.load(&old.id).await.unwrap().is_none());
    assert!(sessions.save(&old).await.is_err());
    let bound_after = db
        .query_row(
            "SELECT count(*) FROM auth_sessions WHERE account_id IS NOT NULL",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap();
    assert_eq!(bound_after, bound_before);
    let (fresh_cookie, fresh_csrf) = form(&normal).await;
    let response = post(&normal, &fresh_cookie, &fresh_csrf, "owner").await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let new_cookie = response.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    assert_ne!(cookie, new_cookie);
    assert_ne!(fresh_cookie, new_cookie);
    assert!(sessions.load(&old.id).await.unwrap().is_none());
    assert!(sessions.save(&old).await.is_err());
    assert_eq!(
        get_session(&normal, &new_cookie, "/").await.status(),
        StatusCode::OK
    );
    let current = sessions
        .load(
            &new_cookie
                .trim_start_matches("mimik_session=")
                .parse()
                .unwrap(),
        )
        .await
        .unwrap()
        .unwrap();
    let csrf = current.data["csrf"].as_str().unwrap().to_owned();
    let (app, entered, release) = gated_router(store, RequestGate::PrincipalSave).await;
    let task = tokio::spawn(async move { post(&app, &new_cookie, &csrf, "owner").await });
    let second_save = captured(entered).await;
    assert_ne!(second_save.id, current.id);
    assert!(sessions.load(&current.id).await.unwrap().is_none());
    logout_request(&normal, &second_save).await;
    release.add_permits(1);
    error(task.await.unwrap(), StatusCode::INTERNAL_SERVER_ERROR).await;
    for record in [old, current, second_save] {
        assert!(sessions.load(&record.id).await.unwrap().is_none());
        assert!(sessions.save(&record).await.is_err());
    }
}

async fn verify_to_create_race(
    store: &Store,
    accounts: &Accounts,
    player: &Summary,
    dir: &std::path::Path,
) {
    let sessions = ResetAtBoundary {
        inner: SqliteSessions::new(store.clone()),
        accounts: accounts.clone(),
        account: player.clone(),
        output: dir.join("between-verify-save"),
        boundary: TransitionBoundary::Create,
    };
    let app = mimik::auth::app_with_test_sessions(store.clone(), sessions)
        .await
        .unwrap();
    let (cookie, token) = form(&app).await;
    error(
        post(&app, &cookie, &token, "player").await,
        StatusCode::INTERNAL_SERVER_ERROR,
    )
    .await;
    let id = cookie.trim_start_matches("mimik_session=").parse().unwrap();
    assert!(
        SqliteSessions::new(store.clone())
            .load(&id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        accounts
            .current(player.id, player.epoch)
            .await
            .unwrap()
            .is_none()
    );
    let db = rusqlite::Connection::open(dir.join("db")).unwrap();
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM auth_sessions WHERE account_id IS NOT NULL",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
}

async fn second_save_epoch_race(
    store: &Store,
    accounts: &Accounts,
    player: &Summary,
    dir: &std::path::Path,
) {
    let current = accounts.show(player.id).await.unwrap();
    let sessions = ResetAtBoundary {
        inner: SqliteSessions::new(store.clone()),
        accounts: accounts.clone(),
        account: current.clone(),
        output: dir.join("between-create-second-save"),
        boundary: TransitionBoundary::Save,
    };
    let app = mimik::auth::app_with_test_sessions(store.clone(), sessions)
        .await
        .unwrap();
    let (cookie, token) = form(&app).await;
    error(
        post(&app, &cookie, &token, "player").await,
        StatusCode::INTERNAL_SERVER_ERROR,
    )
    .await;
    assert!(
        accounts
            .current(current.id, current.epoch)
            .await
            .unwrap()
            .is_none()
    );
    let db = rusqlite::Connection::open(dir.join("db")).unwrap();
    let id = db
        .query_row(
            "SELECT id FROM auth_sessions WHERE account_id=?1 AND epoch=?2 AND state='active'",
            rusqlite::params![current.id, current.epoch],
            |r| r.get::<_, String>(0),
        )
        .unwrap();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/auth/session")
                .header(header::HOST, "127.0.0.1:3000")
                .header(header::COOKIE, format!("mimik_session={id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    error(response, StatusCode::UNAUTHORIZED).await;
    assert!(
        SqliteSessions::new(store.clone())
            .load(&id.parse().unwrap())
            .await
            .unwrap()
            .is_none()
    );
}

async fn final_recheck_race(accounts: &Accounts, owner: &Summary, dir: &std::path::Path) {
    let gate = rendezvous(accounts);
    let worker = gate.accounts.clone();
    let task = tokio::spawn(async move { worker.authenticate("owner".into(), bytes()).await });
    wait(gate.entered).await;
    assert!(
        accounts
            .current(owner.id, owner.epoch)
            .await
            .unwrap()
            .is_some(),
        "KDF must not hold the DB lock"
    );
    accounts
        .issue_reset(owner.id, owner.epoch, &dir.join("owner-reset"))
        .await
        .unwrap();
    gate.release.send(()).unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap()
            .is_none()
    );
    wait(gate.dropped).await;
    let gate = rendezvous(accounts);
    let worker = gate.accounts.clone();
    let task = tokio::spawn(async move { worker.authenticate("owner".into(), bytes()).await });
    wait(gate.entered).await;
    accounts.disable(owner.id, owner.epoch + 1).await.unwrap();
    gate.release.send(()).unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap()
            .is_none()
    );
    wait(gate.dropped).await;
}

async fn cancelled_unknown_kdf_and_http_busy(store: &Store, accounts: &Accounts) {
    let app = mimik::auth::app(store.clone()).await.unwrap();
    let (cookie, token) = form(&app).await;
    let gate = rendezvous(accounts);
    let worker = gate.accounts.clone();
    let task = tokio::spawn(async move { worker.authenticate("unknown".into(), bytes()).await });
    wait(gate.entered).await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    for user in ["owner", "unknown"] {
        assert!(matches!(
            accounts.authenticate(user.into(), bytes()).await,
            Err(AppError::PasswordBusy)
        ));
    }
    error(
        post(&app, &cookie, &token, "unknown").await,
        StatusCode::SERVICE_UNAVAILABLE,
    )
    .await;
    accounts.require_owner_binding().await.unwrap();
    gate.release.send(()).unwrap();
    wait(gate.dropped).await;
    error(
        post(&app, &cookie, &token, "unknown").await,
        StatusCode::UNAUTHORIZED,
    )
    .await;
    error(
        post(&app, &cookie, &token, "owner").await,
        StatusCode::UNAUTHORIZED,
    )
    .await;
}

async fn hostile_profile_before_kdf(
    store: &Store,
    accounts: &Accounts,
    player: &Summary,
    dir: &std::path::Path,
) {
    let db = rusqlite::Connection::open(dir.join("db")).unwrap();
    let hostile = format!(
        "$argon2id$v=19$m=19456,t=2,p=1${}B${}",
        "A".repeat(21),
        "A".repeat(43)
    );
    db.execute(
        "UPDATE accounts SET epoch=epoch+1,verifier=?1 WHERE id=?2",
        rusqlite::params![hostile, player.id],
    )
    .unwrap();
    let count = Arc::new(AtomicUsize::new(0));
    let counter = count.clone();
    let accounts = accounts.clone().with_auth_test_hooks(
        Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        }),
        Arc::new(|| {}),
    );
    assert!(
        accounts
            .authenticate("player".into(), bytes())
            .await
            .is_err()
    );
    assert_eq!(count.load(Ordering::SeqCst), 0);
    let app = mimik::auth::app(store.clone()).await.unwrap();
    let (cookie, token) = form(&app).await;
    error(
        post(&app, &cookie, &token, "player").await,
        StatusCode::INTERNAL_SERVER_ERROR,
    )
    .await;
}
