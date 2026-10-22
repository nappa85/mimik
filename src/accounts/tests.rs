use super::*;
use std::{
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    sync::{
        Mutex,
        atomic::{AtomicI64, AtomicUsize, Ordering},
        mpsc,
    },
    time::Duration,
};
use std::{
    io::Read,
    os::unix::net::{UnixListener, UnixStream},
    path::PathBuf,
    process::{Child, Command},
};

async fn readiness(listener: &UnixListener, timeout: Duration) -> std::io::Result<UnixStream> {
    listener.set_nonblocking(true)?;
    let listener = tokio::net::UnixListener::from_std(listener.try_clone()?)?;
    tokio::time::timeout(timeout, async {
        let (socket, _) = listener.accept().await?;
        let mut ready = [0];
        loop {
            socket.readable().await?;
            match socket.try_read(&mut ready) {
                Ok(1) => break,
                Ok(_) => return Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof)),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => return Err(error),
            }
        }
        if ready != *b"R" {
            return Err(std::io::Error::other("invalid child readiness"));
        }
        let socket = socket.into_std()?;
        socket.set_nonblocking(false)?;
        socket.set_read_timeout(Some(timeout))?;
        socket.set_write_timeout(Some(timeout))?;
        Ok(socket)
    })
    .await
    .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "child readiness timed out"))?
}

#[derive(Default)]
struct Children(Vec<Child>);

impl Drop for Children {
    fn drop(&mut self) {
        for child in &mut self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Children {
    fn finish(&mut self) {
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        for child in &mut self.0 {
            loop {
                if let Some(status) = child.try_wait().unwrap() {
                    assert!(status.success());
                    break;
                }
                assert!(std::time::Instant::now() < deadline, "child exit timed out");
                std::thread::yield_now();
            }
        }
        self.0.clear();
    }
}

#[test]
fn exited_child_readiness_is_bounded() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("missing.sock");
    let listener = UnixListener::bind(&path).unwrap();
    let child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "accounts::tests::process_redemption_worker"])
        .env_remove("MIMIK_TEST_ACCOUNT_DB")
        .spawn()
        .unwrap();
    let mut children = Children(vec![child]);
    children.finish();
    let (done_tx, done_rx) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let result = runtime.block_on(readiness(&listener, Duration::from_millis(100)));
        done_tx.send(result.map(|_| ())).unwrap();
    });
    let result = done_rx.recv_timeout(Duration::from_secs(1));
    if result.is_err() {
        // Rescue the old blocking implementation so the failing-first test itself
        // terminates and does not strand a test worker.
        let mut rescue = UnixStream::connect(&path).unwrap();
        rescue.write_all(b"R").unwrap();
    }
    thread.join().unwrap();
    assert!(
        matches!(result, Ok(Err(ref error)) if error.kind() == std::io::ErrorKind::TimedOut),
        "missing child readiness did not return a bounded timeout: {result:?}"
    );
}

#[tokio::test]
async fn connected_child_without_ready_byte_is_bounded() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("silent.sock");
    let listener = UnixListener::bind(&path).unwrap();
    let _silent_peer = UnixStream::connect(&path).unwrap();
    let result = readiness(&listener, Duration::from_millis(100)).await;
    assert!(matches!(result, Err(ref error) if error.kind() == std::io::ErrorKind::TimedOut));
}

#[tokio::test]
async fn rendezvous_failure_cleanup_reaps_waiting_child() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cleanup.sock");
    let listener = UnixListener::bind(&path).unwrap();
    let child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "accounts::tests::waiting_child_worker"])
        .env("MIMIK_TEST_WAITING_CHILD_SOCKET", &path)
        .spawn()
        .unwrap();
    let children = Children(vec![child]);
    let mut socket = readiness(&listener, Duration::from_secs(20)).await.unwrap();
    // Simulate parent failure before releasing the barrier. Drop must terminate
    // and reap the child rather than leave its gate read alive until timeout.
    drop(children);
    let mut byte = [0];
    assert_eq!(socket.read(&mut byte).unwrap(), 0);
}

#[test]
fn waiting_child_worker() {
    let Some(path) = std::env::var_os("MIMIK_TEST_WAITING_CHILD_SOCKET") else {
        return;
    };
    let mut socket = UnixStream::connect(path).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(20)))
        .unwrap();
    socket
        .set_write_timeout(Some(Duration::from_secs(20)))
        .unwrap();
    socket.write_all(b"R").unwrap();
    let mut go = [0];
    socket.read_exact(&mut go).unwrap();
    panic!("parent should kill waiting child before releasing gate");
}

fn input(path: &Path, bytes: &[u8]) {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    file.write_all(bytes).unwrap();
}

type Gate = (
    Arc<dyn Fn() + Send + Sync>,
    mpsc::Receiver<()>,
    mpsc::Sender<()>,
);
fn gate() -> Gate {
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let release_rx = Mutex::new(release_rx);
    (
        Arc::new(move || {
            entered_tx.send(()).unwrap();
            release_rx
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(20))
                .unwrap();
        }),
        entered_rx,
        release_tx,
    )
}

type Snapshot = (i64, String, i64, Vec<(Vec<u8>, String)>);
fn snapshot(db: &Connection) -> Snapshot {
    (
        db.query_row("SELECT count(*) FROM accounts", [], |r| r.get(0))
            .unwrap(),
        db.query_row("SELECT coalesce(group_concat(epoch,','),'') FROM (SELECT epoch FROM accounts ORDER BY id)", [], |r| {
            r.get(0)
        })
        .unwrap(),
        db.query_row("SELECT count(*) FROM account_events", [], |r| r.get(0))
            .unwrap(),
        db.prepare("SELECT digest,state FROM account_credentials ORDER BY digest")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap(),
    )
}

// Share the auth resource lock with browser redemption tests; production KDF
// admission remains deliberately nonqueued. Races use channel acknowledgements.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn deterministic_time_filesystem_and_password_races() {
    let _auth_lock = crate::auth::test_resource_lock().await;
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let path = dir.path().join("db");
    let password = dir.path().join("password");
    input(&password, b"synthetic password 123");
    let alternate = dir.path().join("alternate");
    input(&alternate, b"different password 123");
    let store = Store::open(&path).unwrap();
    let db = Connection::open(&path).unwrap();
    let now = Arc::new(AtomicI64::new(1_000_000));
    let mut accounts = Accounts::new(store.clone());
    let clock = now.clone();
    accounts.clock = Arc::new(move || Ok(clock.load(Ordering::SeqCst)));
    let owner = accounts
        .bootstrap("owner".into(), "Owner".into(), &password)
        .await
        .unwrap();
    assert!(
        !accounts
            .verify_password(owner.id, &alternate)
            .await
            .unwrap()
    );

    sync_failures(&accounts, &dir, &owner, &db).await;
    let player = enrollment_races(&accounts, &dir, &password, &owner, &now).await;
    reset_races(&accounts, &dir, &alternate, &player, &now, &db).await;
    verification_races(&accounts, &dir, &password).await;
    cancellation(&accounts, &owner, &password).await;
    cancellation_handoff(&dir, &password).await;
    unsupported_and_private(&accounts, &dir, &owner, &password, &db).await;
    rejection_never_runs_kdf(&dir, &password).await;
    phc_rejection_never_runs_kdf().await;
    post_sync_sql_failure(&dir, &password).await;
    process_races(&dir, &password).await;
}

async fn rejection_never_runs_kdf(dir: &tempfile::TempDir, password: &Path) {
    let path = dir.path().join("admission.sqlite3");
    let mut accounts = Accounts::new(Store::open(&path).unwrap());
    let now = Arc::new(AtomicI64::new(1_000_000));
    let clock = now.clone();
    accounts.clock = Arc::new(move || Ok(clock.load(Ordering::SeqCst)));
    accounts
        .bootstrap("owner".into(), "Owner".into(), password)
        .await
        .unwrap();
    let db = Connection::open(path).unwrap();
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    accounts.work_gate = Some(Arc::new(move || {
        counter.fetch_add(1, Ordering::SeqCst);
    }));
    for purpose in ["enroll", "reset"] {
        let token = dir.path().join(format!("admission-{purpose}"));
        let reserved = accounts
            .invite(format!("admission-{purpose}"), "Player".into(), &token)
            .await
            .unwrap();
        let token = if purpose == "reset" {
            let player = accounts.enroll(&token, password).await.unwrap();
            let reset = dir.path().join("admission-reset-issued");
            accounts
                .issue_reset(player.id, player.epoch, &reset)
                .await
                .unwrap();
            reset
        } else {
            token
        };
        let start = hits.load(Ordering::SeqCst);
        let before = snapshot(&db);
        invalid_credential_preflight(&accounts, dir.path(), password, purpose, &token, &now).await;
        assert_eq!(
            hits.load(Ordering::SeqCst),
            start,
            "{purpose} preflight reached KDF"
        );
        assert_eq!(snapshot(&db), before);
        let replacement = dir.path().join(format!("replacement-{purpose}"));
        if purpose == "enroll" {
            accounts
                .invite(reserved.login, "Player".into(), &replacement)
                .await
                .unwrap();
        } else {
            let current = accounts.show(reserved.id).await.unwrap();
            accounts
                .issue_reset(current.id, current.epoch, &replacement)
                .await
                .unwrap();
        }
        let before = snapshot(&db);
        assert!(accounts.redeem(purpose, &token, password).await.is_err());
        assert_eq!(hits.load(Ordering::SeqCst), start);
        assert_eq!(snapshot(&db), before);
        // Positive control proves that the counter is connected to actual work.
        let current = accounts
            .redeem(purpose, &replacement, password)
            .await
            .unwrap();
        assert_eq!(hits.load(Ordering::SeqCst), start + 1);
        let before = snapshot(&db);
        assert!(
            accounts
                .redeem(purpose, &replacement, password)
                .await
                .is_err()
        );
        assert_eq!(
            hits.load(Ordering::SeqCst),
            start + 1,
            "consumed {purpose} reached KDF"
        );
        assert_eq!(snapshot(&db), before);
        let disabled = dir.path().join(format!("disabled-{purpose}"));
        let issued = accounts
            .issue_reset(current.id, current.epoch, &disabled)
            .await
            .unwrap();
        accounts.disable(current.id, issued.epoch).await.unwrap();
        let before = snapshot(&db);
        assert!(accounts.complete_reset(&disabled, password).await.is_err());
        assert_eq!(hits.load(Ordering::SeqCst), start + 1);
        assert_eq!(snapshot(&db), before);
    }
}

async fn invalid_credential_preflight(
    accounts: &Accounts,
    dir: &Path,
    password: &Path,
    purpose: &'static str,
    token: &Path,
    now: &AtomicI64,
) {
    // The other purpose must be rejected even with a valid active credential.
    let other = if purpose == "enroll" {
        "reset"
    } else {
        "enroll"
    };
    assert!(accounts.redeem(other, token, password).await.is_err());
    now.store(999_999, Ordering::SeqCst);
    assert!(accounts.redeem(purpose, token, password).await.is_err());
    now.store(
        1_000_000 + if purpose == "enroll" { 86_400 } else { 3600 },
        Ordering::SeqCst,
    );
    assert!(accounts.redeem(purpose, token, password).await.is_err());
    now.store(1_000_000, Ordering::SeqCst);
    for (index, malformed) in [
        vec![b'g'; 64],
        vec![b'a'; 63],
        vec![b'A'; 64],
        vec![b'0'; 64],
        vec![b'0'; 65],
    ]
    .iter()
    .enumerate()
    {
        let invalid = dir.join(format!("invalid-{purpose}-{index}"));
        input(&invalid, malformed);
        assert!(accounts.redeem(purpose, &invalid, password).await.is_err());
    }
}

async fn phc_rejection_never_runs_kdf() {
    let canonical = "$argon2id$v=19$m=19456,t=2,p=1$AAAAAAAAAAAAAAAAAAAAAA$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
    let hits = Arc::new(AtomicUsize::new(0));
    let invalid = [
        canonical.replace("m=19456", "m=19456,m=19456"),
        canonical.replace("p=1", "p=1,keyid=AAAA"),
        canonical.replace("p=1", "p=1,data=AAAA"),
        format!(
            "$argon2id$v=19$m=19456,t=2,p=1${}${}",
            "A".repeat(21),
            "A".repeat(44)
        ),
        format!(
            "$argon2id$v=19$m=19456,t=2,p=1${}${}",
            "A".repeat(23),
            "A".repeat(42)
        ),
        canonical.replace("AAAAAA$", "AAAAAB$"),
        format!("{}B", &canonical[..96]),
    ];
    for verifier in invalid {
        let mut permit = crypto::Permit::acquire().unwrap();
        let counter = hits.clone();
        permit.gate = Some(Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        }));
        assert!(
            crypto::verify(
                Zeroizing::new(b"synthetic password 123".to_vec()),
                verifier,
                permit
            )
            .await
            .is_err()
        );
        assert_eq!(hits.load(Ordering::SeqCst), 0, "hostile PHC reached KDF");
    }
    let mut permit = crypto::Permit::acquire().unwrap();
    let counter = hits.clone();
    permit.gate = Some(Arc::new(move || {
        counter.fetch_add(1, Ordering::SeqCst);
    }));
    let (matched, permit) = crypto::verify(
        Zeroizing::new(b"synthetic password 123".to_vec()),
        canonical.into(),
        permit,
    )
    .await
    .unwrap();
    assert!(!matched);
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    drop(permit);
}

async fn post_sync_sql_failure(dir: &tempfile::TempDir, password: &Path) {
    let path = dir.path().join("post-sync.sqlite3");
    let mut accounts = Accounts::new(Store::open(&path).unwrap());
    let owner = accounts
        .bootstrap("owner".into(), "Owner".into(), password)
        .await
        .unwrap();
    let db = Connection::open(&path).unwrap();
    accounts
        .invite(
            "reserved".into(),
            "Reserved".into(),
            &dir.path().join("sql-reserved"),
        )
        .await
        .unwrap();
    db.execute_batch("CREATE TRIGGER injected_event_failure BEFORE INSERT ON account_events BEGIN SELECT RAISE(ABORT,'injected post-sync SQL failure'); END;").unwrap();
    for operation in ["invite", "reissue", "reset"] {
        let orphan = dir.path().join(format!("sql-orphan-{operation}"));
        let before = snapshot(&db);
        let verifier: String = db
            .query_row(
                "SELECT verifier FROM accounts WHERE id=?1",
                [owner.id],
                |r| r.get(0),
            )
            .unwrap();
        let error = match operation {
            "invite" => {
                accounts
                    .invite("failed".into(), "Failed".into(), &orphan)
                    .await
            }
            "reissue" => {
                accounts
                    .invite("reserved".into(), "Reserved".into(), &orphan)
                    .await
            }
            _ => accounts.issue_reset(owner.id, owner.epoch, &orphan).await,
        }
        .unwrap_err();
        assert!(error.to_string().contains("injected post-sync SQL failure"));
        assert_eq!(snapshot(&db), before);
        assert_eq!(
            db.query_row(
                "SELECT verifier FROM accounts WHERE id=?1",
                [owner.id],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            verifier
        );
        assert_eq!(files::read(&orphan, 64).unwrap().len(), 64);
        let hits = Arc::new(AtomicUsize::new(0));
        let counter = hits.clone();
        accounts.work_gate = Some(Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        }));
        assert!(accounts.enroll(&orphan, password).await.is_err());
        assert!(accounts.complete_reset(&orphan, password).await.is_err());
        assert_eq!(hits.load(Ordering::SeqCst), 0);
        assert_eq!(snapshot(&db), before);
    }
    db.execute("DROP TRIGGER injected_event_failure", [])
        .unwrap();
    assert!(accounts.verify_password(owner.id, password).await.unwrap());
}

async fn sync_failures(
    accounts: &Accounts,
    dir: &tempfile::TempDir,
    owner: &Summary,
    db: &Connection,
) {
    accounts
        .invite(
            "reserved".into(),
            "Reserved".into(),
            &dir.path().join("reserved-token"),
        )
        .await
        .unwrap();
    for fault in [files::SyncFault::File, files::SyncFault::Directory] {
        let mut broken = accounts.clone();
        broken.sync_fault = fault;
        let before = snapshot(db);
        let destination = dir.path().join(format!("fault-{}", before.2));
        // Each injection uses a new directory to keep abandoned files explicit.
        let private = tempfile::tempdir_in(dir.path()).unwrap();
        fs::set_permissions(private.path(), fs::Permissions::from_mode(0o700)).unwrap();
        assert!(
            broken
                .invite(
                    "failed".into(),
                    "Failed".into(),
                    &private.path().join("token")
                )
                .await
                .is_err()
        );
        assert_eq!(snapshot(db), before);
        let error = broken
            .invite(
                "reserved".into(),
                "Reserved".into(),
                &private.path().join("reissue"),
            )
            .await
            .unwrap_err();
        assert!(
            matches!(error, AppError::SourceIo(_)),
            "expected injected reissue sync failure: {error:?}"
        );
        assert_eq!(snapshot(db), before);
        let error = broken
            .issue_reset(owner.id, owner.epoch, &destination)
            .await
            .unwrap_err();
        assert!(
            matches!(error, AppError::SourceIo(_)),
            "expected injected sync failure: {error:?}"
        );
        assert_eq!(snapshot(db), before);
        // Leave the orphan on disk and reject collision on the next issuance.
        assert!(
            accounts
                .issue_reset(owner.id, owner.epoch, &destination)
                .await
                .is_err()
        );
        fs::rename(&destination, private.path().join("orphan")).unwrap();
    }
}

async fn enrollment_races(
    accounts: &Accounts,
    dir: &tempfile::TempDir,
    password: &Path,
    owner: &Summary,
    now: &AtomicI64,
) -> Summary {
    let invite1 = dir.path().join("invite1");
    let reserved = accounts
        .invite("player".into(), "Player".into(), &invite1)
        .await
        .unwrap();
    now.store(1_000_000 + 86_400, Ordering::SeqCst);
    assert!(accounts.enroll(&invite1, password).await.is_err());
    assert_eq!(accounts.show(reserved.id).await.unwrap().epoch, 1);
    let invite2 = dir.path().join("invite2");
    let reissued = accounts
        .invite("player".into(), "Player".into(), &invite2)
        .await
        .unwrap();
    assert_eq!(reissued.epoch, 2);
    assert!(accounts.enroll(&invite1, password).await.is_err());
    now.store(1_000_000, Ordering::SeqCst);
    assert!(
        accounts.enroll(&invite2, password).await.is_err(),
        "future-issued token must fail"
    );
    now.store(1_086_400, Ordering::SeqCst);

    let (work_gate, entered, release) = gate();
    let mut blocked = accounts.clone();
    blocked.work_gate = Some(work_gate);
    let token = invite2.clone();
    let pw = password.to_owned();
    let task = tokio::spawn(async move { blocked.enroll(&token, &pw).await });
    entered.recv_timeout(Duration::from_secs(20)).unwrap();
    assert!(
        accounts.verify_password(owner.id, password).await.is_err(),
        "global permit must reject without queuing"
    );
    // The existing shared Store remains usable while Argon2's worker is blocked.
    assert_eq!(accounts.show(reserved.id).await.unwrap().epoch, 2);
    let invite3 = dir.path().join("invite3");
    accounts
        .invite("player".into(), "Player".into(), &invite3)
        .await
        .unwrap();
    release.send(()).unwrap();
    assert!(
        task.await.unwrap().is_err(),
        "reissue during hashing must reject old commit"
    );
    let (work_gate, entered, release) = gate();
    let mut blocked = accounts.clone();
    blocked.work_gate = Some(work_gate);
    let token = invite3.clone();
    let pw = password.to_owned();
    let task = tokio::spawn(async move { blocked.enroll(&token, &pw).await });
    entered.recv_timeout(Duration::from_secs(20)).unwrap();
    now.fetch_add(86_400, Ordering::SeqCst);
    release.send(()).unwrap();
    assert!(
        task.await.unwrap().is_err(),
        "expiry during enrollment hashing must reject commit"
    );
    assert_eq!(accounts.show(reserved.id).await.unwrap().epoch, 3);
    let invite4 = dir.path().join("invite4");
    accounts
        .invite("player".into(), "Player".into(), &invite4)
        .await
        .unwrap();
    let player = accounts.enroll(&invite4, password).await.unwrap();
    assert_eq!(player.epoch, 5);
    player
}

async fn reset_races(
    accounts: &Accounts,
    dir: &tempfile::TempDir,
    alternate: &Path,
    player: &Summary,
    now: &AtomicI64,
    db: &Connection,
) {
    for operation in ["expiry", "reissue", "disable"] {
        let token = dir.path().join(format!("reset-{operation}"));
        let current = accounts.show(player.id).await.unwrap();
        let issued = accounts
            .issue_reset(player.id, current.epoch, &token)
            .await
            .unwrap();
        let (work_gate, entered, release) = gate();
        let mut blocked = accounts.clone();
        blocked.work_gate = Some(work_gate);
        let pw = alternate.to_owned();
        let task = tokio::spawn(async move { blocked.complete_reset(&token, &pw).await });
        entered.recv_timeout(Duration::from_secs(20)).unwrap();
        match operation {
            "expiry" => now.fetch_add(3600, Ordering::SeqCst),
            "reissue" => {
                accounts
                    .issue_reset(
                        player.id,
                        issued.epoch,
                        &dir.path().join("superseding-reset"),
                    )
                    .await
                    .unwrap();
                0
            }
            _ => {
                accounts.disable(player.id, issued.epoch).await.unwrap();
                0
            }
        };
        let before = snapshot(db);
        release.send(()).unwrap();
        assert!(task.await.unwrap().is_err());
        assert_eq!(snapshot(db), before, "rejected reset changed database");
        assert!(
            !accounts
                .verify_password(player.id, alternate)
                .await
                .unwrap()
        );
    }
}

async fn verification_races(accounts: &Accounts, dir: &tempfile::TempDir, password: &Path) {
    // Reset issuance and disable each invalidate an in-flight successful check.
    for disable in [false, true] {
        let token = dir.path().join(format!("check-invite-{disable}"));
        let login = if disable {
            "check-disable"
        } else {
            "check-reset"
        };
        accounts
            .invite(login.into(), login.into(), &token)
            .await
            .unwrap();
        let enrolled = accounts.enroll(&token, password).await.unwrap();
        let (work_gate, entered, release) = gate();
        let mut blocked = accounts.clone();
        blocked.work_gate = Some(work_gate);
        let pw = password.to_owned();
        let task = tokio::spawn(async move { blocked.verify_password(enrolled.id, &pw).await });
        entered.recv_timeout(Duration::from_secs(20)).unwrap();
        if disable {
            accounts.disable(enrolled.id, enrolled.epoch).await.unwrap();
        } else {
            accounts
                .issue_reset(
                    enrolled.id,
                    enrolled.epoch,
                    &dir.path().join("check-reset-token"),
                )
                .await
                .unwrap();
        }
        release.send(()).unwrap();
        assert!(!task.await.unwrap().unwrap());
    }
}

async fn cancellation(accounts: &Accounts, owner: &Summary, password: &Path) {
    // Cancellation cannot release the permit while a detached blocking worker lives.
    let (work_gate, entered, release) = gate();
    let (dropped_tx, dropped_rx) = mpsc::channel();
    let mut blocked = accounts.clone();
    blocked.work_gate = Some(work_gate);
    blocked.work_dropped = Some(Arc::new(move || {
        dropped_tx.send(()).unwrap();
    }));
    let pw = password.to_owned();
    let id = owner.id;
    let task = tokio::spawn(async move { blocked.verify_password(id, &pw).await });
    entered.recv_timeout(Duration::from_secs(20)).unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert!(accounts.verify_password(owner.id, password).await.is_err());
    release.send(()).unwrap();
    dropped_rx.recv_timeout(Duration::from_secs(20)).unwrap();
    assert!(accounts.verify_password(owner.id, password).await.unwrap());
}

async fn unsupported_and_private(
    accounts: &Accounts,
    dir: &tempfile::TempDir,
    owner: &Summary,
    password: &Path,
    db: &Connection,
) {
    // Malformed stored PHC of admitted SQL size never reaches the KDF gate.
    db.execute(
        "UPDATE accounts SET verifier=substr(verifier,1,96)||'!',epoch=epoch+1 WHERE id=?1",
        [owner.id],
    )
    .unwrap();
    let mut guarded = accounts.clone();
    guarded.work_gate = Some(Arc::new(|| {
        panic!("unsupported PHC reached expensive work")
    }));
    assert!(guarded.verify_password(owner.id, password).await.is_err());
    let private = dir.path().join("not-private");
    fs::create_dir(&private).unwrap();
    fs::set_permissions(&private, fs::Permissions::from_mode(0o755)).unwrap();
    let before = snapshot(db);
    assert!(
        accounts
            .invite("unsafe".into(), "Unsafe".into(), &private.join("token"))
            .await
            .is_err()
    );
    assert_eq!(snapshot(db), before);
}

async fn cancellation_handoff(dir: &tempfile::TempDir, password: &Path) {
    let mut accounts = Accounts::new(Store::open(&dir.path().join("handoff.sqlite3")).unwrap());
    let (commit_gate, entered, release) = gate();
    accounts.clock = Arc::new(move || {
        commit_gate();
        Ok(1_000_000)
    });
    let (dropped_tx, dropped_rx) = mpsc::channel();
    accounts.work_dropped = Some(Arc::new(move || {
        dropped_tx.send(()).unwrap();
    }));
    let check = accounts.clone();
    let pw = password.to_owned();
    let task = tokio::spawn(async move {
        accounts
            .bootstrap("owner".into(), "Owner".into(), &pw)
            .await
    });
    entered.recv_timeout(Duration::from_secs(20)).unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert!(
        check.verify_password(1, password).await.is_err(),
        "cancelled DB worker must retain handoff permit"
    );
    release.send(()).unwrap();
    dropped_rx.recv_timeout(Duration::from_secs(20)).unwrap();
    assert_eq!(check.show(1).await.unwrap().epoch, 1);
}

async fn process_races(dir: &tempfile::TempDir, password: &Path) {
    // Independent processes both cross preflight before either finishes KDF.
    let reset_password = dir.path().join("process-reset-password");
    input(&reset_password, b"new process password 123");
    for action in ["bootstrap", "enroll", "reset"] {
        let process_db = dir.path().join(format!("process-{action}.sqlite3"));
        let process_accounts = Accounts::new(Store::open(&process_db).unwrap());
        let token = dir.path().join(format!("process-{action}.token"));
        let mut reset_target = None;
        if action != "bootstrap" {
            process_accounts
                .bootstrap("owner".into(), "Owner".into(), password)
                .await
                .unwrap();
            process_accounts
                .invite("player".into(), "Player".into(), &token)
                .await
                .unwrap();
            if action == "reset" {
                let player = process_accounts.enroll(&token, password).await.unwrap();
                reset_target = Some(
                    process_accounts
                        .issue_reset(
                            player.id,
                            player.epoch,
                            &dir.path().join("process-reset-new.token"),
                        )
                        .await
                        .unwrap(),
                );
            }
        }
        let token = if action == "reset" {
            dir.path().join("process-reset-new.token")
        } else {
            token
        };
        let connection = Connection::open(&process_db).unwrap();
        let before = snapshot(&connection);
        let old_verifier: Option<String> = reset_target.as_ref().map(|target| {
            connection
                .query_row(
                    "SELECT verifier FROM accounts WHERE id=?1",
                    [target.id],
                    |r| r.get(0),
                )
                .unwrap()
        });
        let socket_path = dir.path().join(format!("process-{action}.sock"));
        run_process_race(
            &process_db,
            &token,
            if action == "reset" {
                &reset_password
            } else {
                password
            },
            &socket_path,
            action,
        )
        .await;
        let process_db = Connection::open(&process_db).unwrap();
        assert_eq!(
            process_db
                .query_row("SELECT count(*) FROM account_owner", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
        if action == "enroll" {
            assert_eq!(
                process_db
                    .query_row(
                        "SELECT count(*) FROM account_credentials WHERE state='consumed'",
                        [],
                        |r| r.get::<_, i64>(0)
                    )
                    .unwrap(),
                1
            );
        }
        if let Some(target) = reset_target {
            check_process_reset(
                &process_accounts,
                &process_db,
                &target,
                &token,
                old_verifier,
                before.2,
                (password, &reset_password),
            )
            .await;
        }
    }
}

async fn run_process_race(
    db: &Path,
    token: &Path,
    password: &Path,
    socket_path: &Path,
    action: &str,
) {
    let listener = UnixListener::bind(socket_path).unwrap();
    let mut children = Children::default();
    for _ in 0..2 {
        children.0.push(
            Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "accounts::tests::process_redemption_worker",
                    "--nocapture",
                ])
                .env("MIMIK_TEST_ACCOUNT_DB", db)
                .env("MIMIK_TEST_ACCOUNT_PASSWORD", password)
                .env("MIMIK_TEST_ACCOUNT_TOKEN", token)
                .env("MIMIK_TEST_ACCOUNT_GATE", socket_path)
                .env("MIMIK_TEST_ACCOUNT_ACTION", action)
                .spawn()
                .unwrap(),
        );
    }
    let mut connections = Vec::new();
    for _ in 0..2 {
        connections.push(readiness(&listener, Duration::from_secs(20)).await.unwrap());
    }
    for socket in &mut connections {
        socket.write_all(b"G").unwrap();
    }
    let mut winners = 0;
    for socket in &mut connections {
        let mut outcome = [0];
        socket.read_exact(&mut outcome).unwrap();
        assert!(outcome == *b"W" || outcome == *b"L");
        winners += usize::from(outcome == *b"W");
    }
    children.finish();
    assert_eq!(
        winners, 1,
        "independent process {action} must have exactly one winner"
    );
}

async fn check_process_reset(
    accounts: &Accounts,
    db: &Connection,
    target: &Summary,
    token: &Path,
    old_verifier: Option<String>,
    previous_events: i64,
    passwords: (&Path, &Path),
) {
    let current = accounts.show(target.id).await.unwrap();
    assert_eq!(current.epoch, target.epoch + 1);
    assert_eq!(current.state, "enabled");
    assert_eq!(snapshot(db).2, previous_events + 1);
    let (consumed, epoch): (String, i64) = db
        .query_row(
            "SELECT state,epoch FROM account_credentials WHERE digest=?1",
            [token_digest(token).unwrap().as_slice()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(consumed, "consumed");
    assert_eq!(epoch, target.epoch);
    assert_eq!(db.query_row("SELECT count(*) FROM account_credentials WHERE account_id=?1 AND purpose='reset' AND state='consumed'", [target.id], |r| r.get::<_, i64>(0)).unwrap(), 1);
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM account_credentials WHERE account_id=?1 AND state='active'",
            [target.id],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    let verifier: String = db
        .query_row(
            "SELECT verifier FROM accounts WHERE id=?1",
            [target.id],
            |r| r.get(0),
        )
        .unwrap();
    assert_ne!(Some(verifier), old_verifier);
    assert_eq!(db.query_row("SELECT count(*) FROM account_events WHERE account_id=?1 AND epoch=?2 AND action='reset_complete' AND actor='credential_redemption'", params![target.id,current.epoch], |r| r.get::<_, i64>(0)).unwrap(), 1);
    assert!(
        accounts
            .verify_password(target.id, passwords.1)
            .await
            .unwrap()
    );
    assert!(
        !accounts
            .verify_password(target.id, passwords.0)
            .await
            .unwrap()
    );
}

#[test]
fn process_redemption_worker() {
    let Some(db) = std::env::var_os("MIMIK_TEST_ACCOUNT_DB") else {
        return;
    };
    let password = PathBuf::from(std::env::var_os("MIMIK_TEST_ACCOUNT_PASSWORD").unwrap());
    let token = PathBuf::from(std::env::var_os("MIMIK_TEST_ACCOUNT_TOKEN").unwrap());
    let socket = UnixStream::connect(std::env::var_os("MIMIK_TEST_ACCOUNT_GATE").unwrap()).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(20)))
        .unwrap();
    socket
        .set_write_timeout(Some(Duration::from_secs(20)))
        .unwrap();
    let socket = Arc::new(Mutex::new(socket));
    let gate_socket = socket.clone();
    let mut accounts = Accounts::new(Store::open(Path::new(&db)).unwrap());
    accounts.work_gate = Some(Arc::new(move || {
        let mut socket = gate_socket.lock().unwrap();
        socket.write_all(b"R").unwrap();
        let mut go = [0];
        socket.read_exact(&mut go).unwrap();
        assert_eq!(go, [b'G']);
    }));
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let result = runtime.block_on(async {
        match std::env::var("MIMIK_TEST_ACCOUNT_ACTION").unwrap().as_str() {
            "bootstrap" => {
                accounts
                    .bootstrap("owner".into(), "Owner".into(), &password)
                    .await
            }
            "enroll" => accounts.enroll(&token, &password).await,
            "reset" => accounts.complete_reset(&token, &password).await,
            _ => panic!("unknown process action"),
        }
    });
    socket
        .lock()
        .unwrap()
        .write_all(if result.is_ok() { b"W" } else { b"L" })
        .unwrap();
}

#[test]
fn exact_password_and_descriptor_admission() {
    for bytes in [
        vec![b'a'; 11],
        vec![b'a'; 129],
        vec![0xff; 20],
        b"long password 123\n".to_vec(),
        vec![0; 20],
    ] {
        assert!(crypto::password(&bytes).is_err());
    }
    assert!(crypto::password("😀".repeat(128).as_bytes()).is_ok());
    assert!(crypto::password(b"abcdefghijkl").is_ok());
    assert!(crypto::password_for(b"abcdefghijklmn", true).is_err());
    assert!(crypto::password_for(b"abcdefghijklmno", true).is_ok());
    assert!(crypto::password(b"  exact password 123  ").is_ok());
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("input");
    input(&path, b"exact password 123");
    let link = dir.path().join("link");
    std::os::unix::fs::symlink(&path, &link).unwrap();
    assert!(files::read(&link, 512).is_err());
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
    assert!(files::read(&path, 512).is_err());
    assert!(files::read(dir.path(), 512).is_err());
    let fifo = dir.path().join("fifo");
    rustix::fs::mknodat(
        rustix::fs::CWD,
        &fifo,
        rustix::fs::FileType::Fifo,
        rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
        0,
    )
    .unwrap();
    assert!(files::read(&fifo, 512).is_err());
    let oversized = dir.path().join("oversized");
    input(&oversized, &[b'a'; 513]);
    assert!(files::read(&oversized, 512).is_err());
    assert!(files::write(&link, b"token", files::SyncFault::None).is_err());
    let canonical = "$argon2id$v=19$m=19456,t=2,p=1$AAAAAAAAAAAAAAAAAAAAAA$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
    assert!(crypto::admit(canonical).is_ok());
    for unsupported in [
        canonical.replace("19456", "99999"),
        canonical.replace("argon2id", "argon2i"),
        canonical.replace("v=19", "v=16"),
        canonical.replace("m=19456,t=2,p=1", "t=2,m=19456,p=1"),
        canonical.replace("p=1", "p=2"),
        format!("{canonical}="),
        canonical.replace("$AAAA", "$!!!!"),
    ] {
        assert!(crypto::admit(&unsupported).is_err());
    }
    let mut children = Children::default();
    children.0.push(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "accounts::tests::descriptor_umask_worker",
                "--nocapture",
            ])
            .env("MIMIK_TEST_DESCRIPTOR_WORKER", "1")
            .spawn()
            .unwrap(),
    );
    children.finish();
}

#[test]
fn descriptor_umask_worker() {
    if std::env::var_os("MIMIK_TEST_DESCRIPTOR_WORKER").is_none() {
        return;
    }
    // umask is process-global: only mutate it in this isolated child.
    rustix::process::umask(rustix::fs::Mode::from_raw_mode(0o777));
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let output = dir.path().join("token");
    files::write(&output, &[b'0'; 64], files::SyncFault::None).unwrap();
    assert_eq!(
        fs::metadata(&output).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(files::read(&output, 64).unwrap().len(), 64);
    if rustix::process::geteuid().is_root() {
        let descriptor = OpenOptions::new().read(true).open(&output).unwrap();
        rustix::fs::fchown(&descriptor, Some(rustix::process::Uid::from_raw(1)), None).unwrap();
        assert!(
            files::read(&output, 64).is_err(),
            "wrong-owner input descriptor accepted"
        );
        // Parent descriptor ownership is checked before output creation too.
        let directory = OpenOptions::new().read(true).open(dir.path()).unwrap();
        rustix::fs::fchown(&directory, Some(rustix::process::Uid::from_raw(1)), None).unwrap();
        assert!(
            files::write(
                &dir.path().join("rejected"),
                b"token",
                files::SyncFault::None
            )
            .is_err()
        );
        assert!(!dir.path().join("rejected").exists());
    }
}

#[tokio::test]
async fn sql_guard_constraints_and_checked_epoch_exhaustion() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("exhausted.sqlite3");
    let store = Store::open(&path).unwrap();
    let db = Connection::open(&path).unwrap();
    db.execute_batch("PRAGMA foreign_keys=ON").unwrap();
    let canonical = "$argon2id$v=19$m=19456,t=2,p=1$AAAAAAAAAAAAAAAAAAAAAA$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
    db.execute("INSERT INTO accounts(id,login,display_name,state,epoch,verifier) VALUES(1,'owner','Owner','enabled',1,?1)",[canonical]).unwrap();
    db.execute("INSERT INTO account_owner VALUES(1,1)", [])
        .unwrap();
    // Simulate an already exhausted historic account without billions of updates;
    // the actual production update guard remains installed for every assertion.
    db.execute("DROP TRIGGER accounts_insert", []).unwrap();
    db.execute("INSERT INTO accounts(id,login,display_name,state,epoch,verifier) VALUES(2,'exhausted','Exhausted','enabled',9223372036854775807,?1)",[canonical]).unwrap();
    db.execute("INSERT INTO accounts(id,login,display_name,state,epoch) VALUES(3,'pending','Pending','pending',1)",[]).unwrap();
    for sql in [
        "UPDATE accounts SET epoch=epoch+1 WHERE id=2",
        "UPDATE accounts SET epoch=1.5 WHERE id=1",
        "UPDATE accounts SET epoch=0 WHERE id=1",
        "UPDATE accounts SET id=4,epoch=2 WHERE id=3",
        "INSERT INTO account_credentials VALUES(zeroblob(31),3,'enroll',1,100,86500,'active')",
        "INSERT INTO account_credentials VALUES(zeroblob(32),3,'reset',1,100,3700,'active')",
        "INSERT INTO account_credentials VALUES(zeroblob(32),3,'enroll',2,100,86500,'active')",
        "INSERT INTO account_credentials VALUES(zeroblob(32),3,'enroll',1,100,86501,'active')",
        "INSERT INTO account_credentials VALUES(zeroblob(32),3,'enroll',1,0,86400,'active')",
        "INSERT INTO account_credentials VALUES(zeroblob(32),999,'enroll',1,100,86500,'active')",
        "INSERT INTO account_events(account_id,epoch,occurred_at,actor,action) VALUES(1,1,100,'credential_redemption','disable')",
    ] {
        assert!(db.execute(sql, []).is_err(), "constraint accepted {sql}");
    }
    db.execute(
        "INSERT INTO account_credentials VALUES(zeroblob(32),3,'enroll',1,100,86500,'active')",
        [],
    )
    .unwrap();
    for sql in [
        "INSERT OR REPLACE INTO account_credentials VALUES(zeroblob(32),3,'enroll',1,100,86500,'active')",
        "UPDATE account_credentials SET purpose='reset',expires_at=3700 WHERE account_id=3",
        "UPDATE account_credentials SET digest=zeroblob(32),epoch=2 WHERE account_id=3",
        "UPDATE account_credentials SET state='active' WHERE account_id=3",
    ] {
        assert!(
            db.execute(sql, []).is_err(),
            "credential guard accepted {sql}"
        );
    }
    db.execute("UPDATE account_credentials SET state='revoked'", [])
        .unwrap();
    assert!(
        db.execute("UPDATE account_credentials SET state='active'", [])
            .is_err()
    );
    let before = snapshot(&db);
    let accounts = Accounts::new(store);
    assert!(accounts.disable(2, i64::MAX).await.is_err());
    assert!(
        accounts
            .issue_reset(2, i64::MAX, &dir.path().join("never-created"))
            .await
            .is_err()
    );
    assert_eq!(snapshot(&db), before);
    assert!(!dir.path().join("never-created").exists());
}
