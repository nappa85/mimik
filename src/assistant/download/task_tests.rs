use super::*;
use crate::assistant::download::client::SHIPPED_HOSTS;
use crate::assistant::download::disk::{Faults, NoFaults};
use crate::assistant::download::fake_hub::{FakeHub, Reply, Seen};
use crate::assistant::download::rows::{New, memory::Memory};
use sha2::{Digest, Sha256};
use std::sync::Condvar;
use std::sync::atomic::AtomicUsize;

const SPEC: &str = "hf:ibm-granite/granite-4.2-3b-GGUF/granite-4.2-3b-Q4_K_M.gguf@c40945d71cd90f249a56985e8155551a9188dc30";

/// Fails or blocks at chosen points.
#[derive(Default)]
struct Script {
    fail: Mutex<Option<Fault>>,
    block: Mutex<Option<Fault>>,
    open: Mutex<bool>,
    gate: Condvar,
    hits: Mutex<Vec<Fault>>,
}

/// Releases a blocked fault point when dropped.
struct Release(Arc<Script>);

impl Drop for Release {
    fn drop(&mut self) {
        self.0.release();
    }
}

impl Script {
    fn fail_at(&self, point: Fault) {
        *self.fail.lock().unwrap() = Some(point);
    }

    /// Blocks at `point` until released. The guard releases when dropped, so a test that
    /// fails before its own release does not leave a disk thread blocked forever.
    fn block_at(self: &Arc<Self>, point: Fault) -> Release {
        *self.block.lock().unwrap() = Some(point);
        *self.open.lock().unwrap() = false;
        Release(Arc::clone(self))
    }

    fn release(&self) {
        *self.block.lock().unwrap() = None;
        *self.open.lock().unwrap() = true;
        self.gate.notify_all();
    }

    fn hit_count(&self, point: Fault) -> usize {
        self.hits
            .lock()
            .unwrap()
            .iter()
            .filter(|p| **p == point)
            .count()
    }
}

impl Faults for Script {
    fn hit(&self, point: Fault) -> std::io::Result<()> {
        self.hits.lock().unwrap().push(point);
        if *self.block.lock().unwrap() == Some(point) {
            let mut open = self.open.lock().unwrap();
            while !*open {
                open = self.gate.wait(open).unwrap();
            }
        }
        let mut fail = self.fail.lock().unwrap();
        if *fail == Some(point) {
            *fail = None;
            return Err(std::io::Error::other("injected"));
        }
        Ok(())
    }
}

fn config() -> Config {
    Config {
        flush: 1000,
        floor_bytes: 1,
        floor_window: Duration::from_secs(60),
        reserve: 0,
        chunk: 1000,
    }
}

struct Rig {
    dir: tempfile::TempDir,
    hub: FakeHub,
    rows: Arc<Memory>,
    script: Arc<Script>,
    content: Arc<Vec<u8>>,
    disk: Arc<Disk>,
    downloader: Downloader,
}

fn serve(content: Arc<Vec<u8>>) -> impl FnMut(&Seen) -> Reply + Send + 'static {
    move |seen| {
        if seen.path.contains("/resolve/") {
            Reply::file(
                &content,
                seen.headers.get("range").and_then(|v| v.to_str().ok()),
            )
        } else {
            Reply::status(404)
        }
    }
}

fn content(len: usize) -> Arc<Vec<u8>> {
    Arc::new((0..len).map(|i| u8::try_from(i % 251).unwrap()).collect())
}

fn sha(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

impl Rig {
    async fn new(len: usize) -> Self {
        Self::with(len, config()).await
    }

    async fn with(len: usize, config: Config) -> Self {
        let content = content(len);
        let names: Vec<&str> = SHIPPED_HOSTS.to_vec();
        let hub = FakeHub::start(&names, serve(Arc::clone(&content))).await;
        let dir = tempfile::tempdir().unwrap();
        std::fs::set_permissions(
            dir.path(),
            std::os::unix::fs::PermissionsExt::from_mode(0o700),
        )
        .unwrap();
        let script = Arc::new(Script::default());
        let disk = Arc::new(Disk::open(dir.path(), script.clone()).unwrap());
        let rows = Arc::new(Memory::default());
        let downloader = Downloader::spawn(
            Client::new(&hub.endpoints()).unwrap(),
            Arc::clone(&disk),
            rows.clone(),
            config,
        )
        .await
        .unwrap();
        Self {
            dir,
            hub,
            rows,
            script,
            content,
            disk,
            downloader,
        }
    }

    fn new_row(&self) -> New {
        New {
            spec: SPEC.to_owned(),
            file_name: format!("hf-{}.gguf", sha(&self.content)),
            sha256: sha(&self.content),
            size: self.content.len() as u64,
            licence_id: Some("apache-2.0".into()),
            licence_path: None,
            licence_sha256: None,
            confirmed_by: 1,
        }
    }

    async fn confirm(&self) -> Row {
        self.rows.insert(self.new_row()).await.unwrap()
    }

    fn model_file(&self, row: &Row) -> std::path::PathBuf {
        self.dir.path().join(&row.file_name)
    }

    fn partial_file(&self, row: &Row) -> std::path::PathBuf {
        self.dir.path().join(".partial").join(&row.sha256)
    }

    fn range_requests(&self) -> Vec<String> {
        self.hub
            .requests()
            .iter()
            .filter_map(|seen| {
                seen.headers
                    .get("range")
                    .map(|v| v.to_str().unwrap().to_owned())
            })
            .collect()
    }

    /// A restart: the old task ends, the disk is reopened without faults, and the new task
    /// reconciles before accepting anything.
    async fn restart(&mut self) {
        self.restart_with(Arc::new(NoFaults)).await;
    }

    /// [`Self::restart`] over a disk with `faults`.
    async fn restart_with(&mut self, faults: Arc<dyn Faults>) {
        self.downloader.shutdown().await;
        self.disk = Arc::new(Disk::open(self.dir.path(), faults).unwrap());
        self.downloader = Downloader::spawn(
            Client::new(&self.hub.endpoints()).unwrap(),
            Arc::clone(&self.disk),
            self.rows.clone(),
            config(),
        )
        .await
        .unwrap();
    }
}

async fn until(downloader: &Downloader, what: &str, done: impl Fn(&Phase) -> bool) -> Status {
    for _ in 0..1000 {
        let status = downloader.status();
        if done(&status.phase) {
            return status;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("never reached: {what}; status {:?}", downloader.status());
}

async fn finished(downloader: &Downloader) -> Phase {
    until(downloader, "a finished phase", |phase| {
        matches!(phase, Phase::Done | Phase::Stopped(_) | Phase::Idle)
    })
    .await
    .phase
}

struct Guard(Arc<AtomicUsize>);

impl Drop for Guard {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

/// Resumes `row` once the abandoned file work of a cancel has finished (until then the task
/// answers that it is still finishing).
async fn resume_when_free(rig: &Rig, row: &Row) {
    for _ in 0..400 {
        match rig.downloader.resume(row.id, permit()).await {
            Ok(()) => return,
            Err(Refusal::Finishing) => tokio::time::sleep(Duration::from_millis(10)).await,
            Err(other) => panic!("{other}"),
        }
    }
    panic!("the task never finished");
}

fn permit() -> Permit {
    Box::new(())
}

#[tokio::test]
async fn a_download_is_received_verified_published_and_recorded() {
    let rig = Rig::new(10_000).await;
    let row = rig.confirm().await;
    rig.downloader.start(row.clone(), permit()).await.unwrap();
    assert_eq!(finished(&rig.downloader).await, Phase::Done);
    assert_eq!(std::fs::read(rig.model_file(&row)).unwrap(), *rig.content);
    assert!(!rig.partial_file(&row).exists());
    let rows = rig.rows.list().await.unwrap();
    assert_eq!(rows[0].state, State::Complete);
    // One request, no Range, the redirect-free fake answered the whole file.
    assert_eq!(rig.hub.requests().len(), 1);
    assert_eq!(rig.range_requests().len(), 0);
    // The files have the modes the plan names.
    let mode = std::fs::metadata(rig.model_file(&row)).unwrap();
    assert_eq!(std::os::unix::fs::MetadataExt::mode(&mode) & 0o022, 0);
}

#[tokio::test]
async fn progress_is_visible_and_the_permit_is_released_at_the_end() {
    let rig = Rig::new(4_000).await;
    rig.hub.respond({
        let content = Arc::clone(&rig.content);
        move |_| Reply::Slow {
            status: 200,
            headers: Vec::new(),
            body: content.to_vec(),
            piece: 500,
            gap: Duration::from_millis(30),
        }
    });
    let counter = Arc::new(AtomicUsize::new(0));
    let row = rig.confirm().await;
    rig.downloader
        .start(row, Box::new(Guard(Arc::clone(&counter))))
        .await
        .unwrap();
    let middle = until(
        &rig.downloader,
        "some bytes",
        |phase| matches!(phase, Phase::Running { received, .. } if *received > 0),
    )
    .await;
    assert!(matches!(middle.phase, Phase::Running { total: 4_000, .. }));
    assert_eq!(finished(&rig.downloader).await, Phase::Done);
    assert_eq!(
        counter.load(Ordering::SeqCst),
        1,
        "the permit was dropped once"
    );
}

#[tokio::test]
async fn cancel_keeps_the_partial_and_resume_asks_for_the_rest_with_range() {
    let rig = Rig::new(20_000).await;
    let row = rig.confirm().await;
    rig.hub.respond({
        let content = Arc::clone(&rig.content);
        move |seen| {
            if seen.headers.get("range").is_some() {
                Reply::file(
                    &content,
                    seen.headers.get("range").and_then(|v| v.to_str().ok()),
                )
            } else {
                Reply::Slow {
                    status: 200,
                    headers: Vec::new(),
                    body: content.to_vec(),
                    piece: 1000,
                    gap: Duration::from_millis(40),
                }
            }
        }
    });
    rig.downloader.start(row.clone(), permit()).await.unwrap();
    until(
        &rig.downloader,
        "5000 bytes",
        |phase| matches!(phase, Phase::Running { received, .. } if *received >= 5000),
    )
    .await;
    rig.downloader.cancel(None).await;
    assert_eq!(
        finished(&rig.downloader).await,
        Phase::Stopped(Stop::Cancelled)
    );
    let kept = std::fs::metadata(rig.partial_file(&row)).unwrap().len();
    assert!(kept > 0 && kept < 20_000, "{kept}");
    rig.downloader.resume(row.id, permit()).await.unwrap();
    assert_eq!(finished(&rig.downloader).await, Phase::Done);
    assert_eq!(std::fs::read(rig.model_file(&row)).unwrap(), *rig.content);
    // The resume asked for bytes from the partial's length, cut back by one chunk of 1000.
    let ranges = rig.range_requests();
    assert_eq!(ranges.len(), 1, "{ranges:?}");
    let from: u64 = ranges[0]
        .strip_prefix("bytes=")
        .and_then(|rest| rest.strip_suffix('-'))
        .unwrap()
        .parse()
        .unwrap();
    assert!(
        from <= kept && from.is_multiple_of(1000) && kept - from <= 2000,
        "{from} {kept}"
    );
}

#[tokio::test]
async fn a_server_that_does_not_honour_range_makes_the_download_restart_from_zero_once() {
    for answer in ["full", "wrong-start", "wrong-total", "malformed"] {
        let rig = Rig::new(6_000).await;
        let row = rig.confirm().await;
        // A partial of 3000 valid bytes from an earlier attempt (cut back to 2000 on resume).
        std::fs::write(rig.partial_file(&row), &rig.content[..3000]).unwrap();
        std::fs::set_permissions(
            rig.partial_file(&row),
            std::os::unix::fs::PermissionsExt::from_mode(0o600),
        )
        .unwrap();
        let content = Arc::clone(&rig.content);
        rig.hub.respond(move |seen| {
            let range = seen.headers.get("range").and_then(|v| v.to_str().ok());
            match (range, answer) {
                (None, _) | (Some(_), "full") => Reply::file(&content, None),
                (Some(_), "wrong-start") => Reply::Full {
                    status: 206,
                    headers: vec![("content-range", "bytes 0-5999/6000".into())],
                    body: content.to_vec(),
                },
                (Some(_), "wrong-total") => Reply::Full {
                    status: 206,
                    headers: vec![("content-range", "bytes 2000-5999/7000".into())],
                    body: content[2000..].to_vec(),
                },
                _ => Reply::Full {
                    status: 206,
                    headers: vec![("content-range", "garbage".into())],
                    body: content[2000..].to_vec(),
                },
            }
        });
        rig.downloader.start(row.clone(), permit()).await.unwrap();
        assert_eq!(finished(&rig.downloader).await, Phase::Done, "{answer}");
        assert_eq!(
            std::fs::read(rig.model_file(&row)).unwrap(),
            *rig.content,
            "{answer}"
        );
        let ranges = rig.range_requests();
        assert_eq!(ranges.len(), 1, "{answer}: one ranged try, then from zero");
        assert_eq!(rig.hub.requests().len(), 2, "{answer}");
    }
}

#[tokio::test]
async fn a_body_longer_than_the_size_fails_and_never_becomes_a_model() {
    let rig = Rig::new(5_000).await;
    let row = rig.confirm().await;
    let mut longer = rig.content.to_vec();
    longer.extend_from_slice(&[7; 3000]);
    rig.hub.respond(move |_| Reply::Slow {
        status: 200,
        headers: Vec::new(),
        body: longer.clone(),
        piece: 700,
        gap: Duration::from_millis(1),
    });
    rig.downloader.start(row.clone(), permit()).await.unwrap();
    let Phase::Stopped(Stop::Failed(why)) = finished(&rig.downloader).await else {
        panic!("must fail")
    };
    assert!(why.contains("size differs"), "{why}");
    assert!(!rig.model_file(&row).exists());
    assert!(
        std::fs::metadata(rig.partial_file(&row)).is_ok_and(|m| m.len() == 0),
        "no byte of a wrong-sized answer is kept"
    );
    // A size announced larger than the confirmed one is refused before any byte is written.
    let rig = Rig::new(5_000).await;
    let row = rig.confirm().await;
    rig.hub.respond(|_| Reply::file(&vec![1u8; 9_000], None));
    rig.downloader.start(row.clone(), permit()).await.unwrap();
    assert!(matches!(
        finished(&rig.downloader).await,
        Phase::Stopped(Stop::Failed(_))
    ));
    assert!(!rig.model_file(&row).exists());
}

#[tokio::test]
async fn a_hash_mismatch_deletes_the_file_and_leaves_no_model() {
    let rig = Rig::new(5_000).await;
    let mut row = rig.confirm().await;
    // The server sends other bytes of the right size.
    rig.hub.respond(|_| Reply::file(&vec![9u8; 5_000], None));
    rig.downloader.start(row.clone(), permit()).await.unwrap();
    let Phase::Stopped(Stop::Failed(why)) = finished(&rig.downloader).await else {
        panic!("must fail")
    };
    assert!(why.contains("differs"), "{why}");
    assert!(!rig.model_file(&row).exists());
    assert!(!rig.partial_file(&row).exists());
    row.state = State::Confirmed;
    assert_eq!(rig.rows.list().await.unwrap()[0].state, State::Confirmed);
}

/// Stops the download at `point` (a crash or an error), then restarts Mimik and returns the
/// rig after reconciliation.
async fn crashed_at(point: Fault, forced_link: bool) -> (Rig, Row, Phase) {
    let mut rig = Rig::new(8_000).await;
    rig.disk.no_rename2.store(forced_link, Ordering::SeqCst);
    let row = rig.confirm().await;
    rig.script.fail_at(point);
    rig.downloader.start(row.clone(), permit()).await.unwrap();
    let phase = finished(&rig.downloader).await;
    rig.restart().await;
    (rig, row, phase)
}

#[tokio::test]
async fn a_crash_before_the_file_is_published_leaves_a_partial_that_verifies_without_a_request() {
    for point in [
        Fault::AfterWrite,
        Fault::FileFsyncError,
        Fault::ReadError,
        Fault::AfterVerify,
    ] {
        let (rig, row, phase) = crashed_at(point, false).await;
        assert!(
            matches!(phase, Phase::Stopped(Stop::Failed(_))),
            "{point:?}"
        );
        assert!(
            !rig.model_file(&row).exists(),
            "{point:?}: nothing published"
        );
        assert_eq!(
            rig.rows.list().await.unwrap()[0].state,
            State::Confirmed,
            "{point:?}"
        );
        assert_eq!(
            std::fs::metadata(rig.partial_file(&row)).unwrap().len(),
            8_000,
            "{point:?}: the whole partial is kept"
        );
        let before = rig.hub.requests().len();
        rig.downloader.resume(row.id, permit()).await.unwrap();
        assert_eq!(finished(&rig.downloader).await, Phase::Done, "{point:?}");
        assert_eq!(
            rig.hub.requests().len(),
            before,
            "{point:?}: verification only, no request"
        );
        assert_eq!(std::fs::read(rig.model_file(&row)).unwrap(), *rig.content);
    }
}

#[tokio::test]
async fn a_directory_sync_error_rolls_the_publication_back() {
    for point in [Fault::DirFsyncError, Fault::BetweenDirSyncs] {
        for forced_link in [false, true] {
            let (rig, row, phase) = crashed_at(point, forced_link).await;
            assert!(
                matches!(phase, Phase::Stopped(Stop::Failed(_))),
                "{point:?}"
            );
            assert!(
                !rig.model_file(&row).exists(),
                "{point:?} link {forced_link}: not published"
            );
            assert!(
                rig.partial_file(&row).exists(),
                "{point:?}: the partial is back"
            );
            assert_eq!(rig.rows.list().await.unwrap()[0].state, State::Confirmed);
        }
    }
}

#[tokio::test]
async fn a_crash_after_publication_is_completed_by_reconciliation_at_start() {
    for forced_link in [false, true] {
        for point in [Fault::AfterPublish, Fault::BeforeCommit] {
            let (rig, row, phase) = crashed_at(point, forced_link).await;
            assert!(
                matches!(phase, Phase::Stopped(Stop::Failed(_))),
                "{point:?}"
            );
            assert_eq!(std::fs::read(rig.model_file(&row)).unwrap(), *rig.content);
            assert!(
                !rig.partial_file(&row).exists(),
                "{point:?}: partial name gone"
            );
            assert_eq!(
                rig.rows.list().await.unwrap()[0].state,
                State::Complete,
                "{point:?} link {forced_link}"
            );
        }
    }
}

#[tokio::test]
async fn a_database_failure_after_publication_is_reported_and_healed_at_restart() {
    let mut rig = Rig::new(3_000).await;
    let row = rig.confirm().await;
    rig.rows.fail_complete.store(true, Ordering::SeqCst);
    rig.downloader.start(row.clone(), permit()).await.unwrap();
    let Phase::Stopped(Stop::Failed(why)) = finished(&rig.downloader).await else {
        panic!("must report")
    };
    assert!(why.contains("restarting Mimik"), "{why}");
    assert!(rig.model_file(&row).exists());
    rig.rows.fail_complete.store(false, Ordering::SeqCst);
    rig.restart().await;
    assert_eq!(rig.rows.list().await.unwrap()[0].state, State::Complete);
}

#[tokio::test]
async fn a_link_cut_between_link_and_unlink_is_recognised_by_its_second_link() {
    let (rig, row, _) = crashed_at(Fault::AfterLink, true).await;
    // After the restart: the partial name is gone, one link remains, the row is complete.
    assert!(!rig.partial_file(&row).exists());
    assert_eq!(
        std::os::unix::fs::MetadataExt::nlink(&std::fs::metadata(rig.model_file(&row)).unwrap()),
        1
    );
    assert_eq!(rig.rows.list().await.unwrap()[0].state, State::Complete);
    // The same cut after the unlink (before the syncs) needs no recovery of names.
    let (rig, row, _) = crashed_at(Fault::AfterUnlink, true).await;
    assert!(!rig.partial_file(&row).exists());
    assert_eq!(rig.rows.list().await.unwrap()[0].state, State::Complete);
}

#[tokio::test]
async fn reconciliation_sweeps_before_any_command_and_keeps_what_a_download_owns() {
    let rig = Rig::new(2_000).await;
    let row = rig.confirm().await;
    let partial = rig.dir.path().join(".partial");
    let write = |name: &str, bytes: &[u8]| {
        let path = partial.join(name);
        std::fs::write(&path, bytes).unwrap();
        std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o600))
            .unwrap();
    };
    write("orphan", b"left over");
    write(&row.sha256, &vec![0u8; 5_000]); // larger than its row's size
    let other = rig
        .rows
        .insert(New {
            spec: SPEC.replace("granite-4.2-3b-Q4_K_M", "other"),
            sha256: "b".repeat(64),
            file_name: "hf-b.gguf".into(),
            ..rig.new_row()
        })
        .await
        .unwrap();
    write(&other.sha256, &[1u8; 100]); // a resumable partial
    let mut rig = rig;
    rig.restart().await;
    let mut names = rig.disk.partials().unwrap();
    names.sort();
    assert_eq!(
        names,
        std::slice::from_ref(&other.sha256),
        "the orphan and the oversize partial went"
    );
}

#[tokio::test]
async fn a_blocked_disk_write_never_blocks_cancel_or_the_runtime() {
    let rig = Rig::new(10_000).await;
    let row = rig.confirm().await;
    let _release = rig.script.block_at(Fault::BeforeWrite);
    rig.downloader.start(row.clone(), permit()).await.unwrap();
    until(&rig.downloader, "a write in flight", |_| {
        rig.script.hit_count(Fault::BeforeWrite) > 0
    })
    .await;
    // The runtime is alive: a timer fires, and Cancel is reflected at once.
    let ticks = tokio::time::timeout(Duration::from_millis(200), async {
        tokio::time::sleep(Duration::from_millis(50)).await;
    })
    .await;
    assert!(ticks.is_ok());
    assert!(rig.downloader.cancel(None).await);
    until(&rig.downloader, "cancelled", |phase| {
        *phase == Phase::Stopped(Stop::Cancelled)
    })
    .await;
    // Once the disk answers again the abandoned write finishes and a resume completes.
    rig.script.release();
    resume_when_free(&rig, &row).await;
    assert_eq!(finished(&rig.downloader).await, Phase::Done);
    assert_eq!(std::fs::read(rig.model_file(&row)).unwrap(), *rig.content);
}

#[tokio::test]
async fn a_cancel_during_verification_stops_the_hash_and_keeps_the_whole_partial() {
    let rig = Rig::new(10_000).await;
    let row = rig.confirm().await;
    let _release = rig.script.block_at(Fault::ReadError);
    rig.downloader.start(row.clone(), permit()).await.unwrap();
    until(&rig.downloader, "verifying", |phase| {
        *phase == Phase::Verifying
    })
    .await;
    assert!(rig.downloader.cancel(None).await);
    until(&rig.downloader, "cancelled", |phase| {
        *phase == Phase::Stopped(Stop::Cancelled)
    })
    .await;
    rig.script.release();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!rig.model_file(&row).exists());
    assert_eq!(
        std::fs::metadata(rig.partial_file(&row)).unwrap().len(),
        10_000
    );
    let before = rig.hub.requests().len();
    resume_when_free(&rig, &row).await;
    assert_eq!(finished(&rig.downloader).await, Phase::Done);
    assert_eq!(rig.hub.requests().len(), before, "verification only");
}

#[tokio::test]
async fn discard_while_running_stops_it_and_removes_the_partial_and_the_row() {
    let rig = Rig::new(20_000).await;
    let row = rig.confirm().await;
    rig.hub.respond({
        let content = Arc::clone(&rig.content);
        move |_| Reply::Slow {
            status: 200,
            headers: Vec::new(),
            body: content.to_vec(),
            piece: 1000,
            gap: Duration::from_millis(30),
        }
    });
    rig.downloader.start(row.clone(), permit()).await.unwrap();
    until(
        &rig.downloader,
        "bytes",
        |phase| matches!(phase, Phase::Running { received, .. } if *received >= 3000),
    )
    .await;
    rig.downloader
        .discard(row.id, Vec::new(), None)
        .await
        .unwrap();
    assert_eq!(rig.rows.list().await.unwrap().len(), 0);
    assert!(!rig.partial_file(&row).exists());
    assert!(!rig.model_file(&row).exists());
    assert_eq!(rig.downloader.status().phase, Phase::Idle);
    // A finished download is deleted, not discarded.
    let rig = Rig::new(2_000).await;
    let row = rig.confirm().await;
    rig.downloader.start(row.clone(), permit()).await.unwrap();
    assert_eq!(finished(&rig.downloader).await, Phase::Done);
    assert!(
        rig.downloader
            .discard(row.id, Vec::new(), None)
            .await
            .is_err()
    );
    assert!(rig.model_file(&row).exists());
}

#[tokio::test]
async fn a_stalled_server_below_the_floor_stops_the_download_for_a_resume() {
    let cfg = Config {
        floor_bytes: 10_000,
        floor_window: Duration::from_millis(300),
        ..config()
    };
    let rig = Rig::with(20_000, cfg).await;
    let row = rig.confirm().await;
    rig.hub.respond(|_| Reply::Stall {
        status: 200,
        headers: Vec::new(),
        first: vec![0; 10],
        declared: Some(20_000),
    });
    rig.downloader.start(row.clone(), permit()).await.unwrap();
    assert_eq!(
        finished(&rig.downloader).await,
        Phase::Stopped(Stop::Stalled)
    );
    assert!(!rig.model_file(&row).exists());
}

#[tokio::test]
async fn free_space_is_checked_before_any_request_and_again_on_resume() {
    let (low, rows, hub_requests);
    {
        let rig = Rig::new(10_000).await;
        let disk = Arc::new(
            Disk::open(rig.dir.path(), Arc::new(NoFaults))
                .unwrap()
                .with_space(Arc::new(|_| 9_999)),
        );
        let downloader = Downloader::spawn(
            Client::new(&rig.hub.endpoints()).unwrap(),
            disk,
            rig.rows.clone(),
            Config {
                reserve: 0,
                ..config()
            },
        )
        .await
        .unwrap();
        let row = rig.confirm().await;
        downloader.start(row.clone(), permit()).await.unwrap();
        low = finished(&downloader).await;
        // A failed start leaves the row and a partial for a retry.
        rows = rig.rows.list().await.unwrap()[0].state;
        hub_requests = rig.hub.requests().len();
        assert!(rig.partial_file(&row).exists());
    }
    let Phase::Stopped(Stop::Failed(why)) = low else {
        panic!("must fail")
    };
    assert!(why.contains("not enough free disk space"), "{why}");
    assert_eq!((rows, hub_requests), (State::Confirmed, 0));
}

#[tokio::test]
async fn an_unsafe_partial_stops_the_download_and_is_left_untouched() {
    let rig = Rig::new(3_000).await;
    let row = rig.confirm().await;
    let target = rig.dir.path().join("precious");
    std::fs::write(&target, b"precious").unwrap();
    std::os::unix::fs::symlink(&target, rig.partial_file(&row)).unwrap();
    rig.downloader.start(row.clone(), permit()).await.unwrap();
    assert!(matches!(
        finished(&rig.downloader).await,
        Phase::Stopped(Stop::Failed(_))
    ));
    assert_eq!(std::fs::read(&target).unwrap(), b"precious");
    assert_eq!(rig.hub.requests().len(), 0);
}

#[tokio::test]
async fn a_file_that_already_has_the_hash_is_adopted_and_another_with_the_name_is_never_touched() {
    // Same bytes under the name: adopted without a request.
    let rig = Rig::new(4_000).await;
    let row = rig.confirm().await;
    std::fs::write(rig.model_file(&row), &*rig.content).unwrap();
    std::fs::set_permissions(
        rig.model_file(&row),
        std::os::unix::fs::PermissionsExt::from_mode(0o600),
    )
    .unwrap();
    rig.downloader.start(row.clone(), permit()).await.unwrap();
    assert_eq!(finished(&rig.downloader).await, Phase::Done);
    assert_eq!(rig.hub.requests().len(), 0);
    assert_eq!(rig.rows.list().await.unwrap()[0].state, State::Complete);
    // Other bytes of the same size, and another size: refused, the user's file untouched.
    for other in [vec![5u8; 4_000], vec![5u8; 10]] {
        let rig = Rig::new(4_000).await;
        let row = rig.confirm().await;
        std::fs::write(rig.model_file(&row), &other).unwrap();
        std::fs::set_permissions(
            rig.model_file(&row),
            std::os::unix::fs::PermissionsExt::from_mode(0o600),
        )
        .unwrap();
        rig.downloader.start(row.clone(), permit()).await.unwrap();
        let Phase::Stopped(Stop::Failed(why)) = finished(&rig.downloader).await else {
            panic!("must refuse")
        };
        assert!(why.contains("remove it by hand"), "{why}");
        assert_eq!(std::fs::read(rig.model_file(&row)).unwrap(), other);
        assert_eq!(rig.hub.requests().len(), 0);
    }
}

#[tokio::test]
async fn delete_keeps_what_is_protected_in_use_or_shared_and_removes_the_rest() {
    let rig = Rig::new(2_000).await;
    let row = rig.confirm().await;
    rig.downloader.start(row.clone(), permit()).await.unwrap();
    assert_eq!(finished(&rig.downloader).await, Phase::Done);
    // Protected (a pinned default or the effective model), then in the stored choice.
    let protect = vec![row.file_name.clone()];
    assert!(rig.downloader.delete(row.id, &protect).await.is_err());
    rig.rows.in_use.lock().unwrap().push(row.file_name.clone());
    assert!(rig.downloader.delete(row.id, &[]).await.is_err());
    assert!(rig.model_file(&row).exists());
    rig.rows.in_use.lock().unwrap().clear();
    // Another download row names the same file: only this row goes.
    let twin = rig
        .rows
        .insert(New {
            spec: SPEC.replace("granite-4.2-3b-Q4_K_M", "twin"),
            ..rig.new_row()
        })
        .await
        .unwrap();
    rig.downloader.delete(row.id, &[]).await.unwrap();
    assert!(
        rig.model_file(&row).exists(),
        "the twin still refers to the file"
    );
    assert_eq!(rig.rows.list().await.unwrap().len(), 1);
    // A confirmed (unfinished) download is not deleted this way.
    assert!(rig.downloader.delete(twin.id, &[]).await.is_err());
    rig.rows.complete(twin.id).await.unwrap();
    rig.downloader.delete(twin.id, &[]).await.unwrap();
    assert!(!rig.model_file(&row).exists());
    assert_eq!(rig.rows.list().await.unwrap().len(), 0);
}

#[tokio::test]
async fn shutdown_stops_the_transfer_and_keeps_the_partial() {
    let rig = Rig::new(20_000).await;
    let row = rig.confirm().await;
    rig.hub.respond({
        let content = Arc::clone(&rig.content);
        move |_| Reply::Slow {
            status: 200,
            headers: Vec::new(),
            body: content.to_vec(),
            piece: 1000,
            gap: Duration::from_millis(30),
        }
    });
    rig.downloader.start(row.clone(), permit()).await.unwrap();
    until(
        &rig.downloader,
        "bytes",
        |phase| matches!(phase, Phase::Running { received, .. } if *received >= 3000),
    )
    .await;
    rig.downloader.shutdown().await;
    assert!(rig.partial_file(&row).exists());
    assert!(!rig.model_file(&row).exists());
}

#[path = "fixes_tests.rs"]
mod fixes_tests;

#[path = "confirm_tests.rs"]
mod confirm_tests;

#[path = "final_tests.rs"]
mod final_tests;
