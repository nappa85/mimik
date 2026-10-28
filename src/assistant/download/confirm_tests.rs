//! Regression tests of the 2d-2 confirmation round: interrupts in cleanup paths, the permit held
//! until abandoned file work and discards have settled, a task that stays responsive while
//! abandoned work is blocked, publication ownership, discard failures, and the status written
//! only by the task.
use super::*;

fn counter() -> (Arc<AtomicUsize>, impl Fn() -> Permit) {
    let released = Arc::new(AtomicUsize::new(0));
    let again = Arc::clone(&released);
    (released, move || Box::new(Guard(Arc::clone(&again))))
}

async fn wait_for(what: &str, done: impl Fn() -> bool) {
    for _ in 0..1000 {
        if done() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("never reached: {what}");
}

/// A rig whose server sends other bytes of the right size, so the hash check fails.
async fn mismatching(len: usize) -> (Rig, Row) {
    let rig = Rig::new(len).await;
    let row = rig.confirm().await;
    rig.hub.respond(move |_| Reply::file(&vec![9u8; len], None));
    (rig, row)
}

#[tokio::test]
async fn a_cancel_during_the_cleanup_of_a_bad_download_is_honoured() {
    let (rig, row) = mismatching(5_000).await;
    let (released, permit) = counter();
    let _release = rig.script.block_at(Fault::BeforeCleanup);
    rig.downloader.start(row.clone(), permit()).await.unwrap();
    wait_for("the cleanup", || {
        rig.script.hit_count(Fault::BeforeCleanup) > 0
    })
    .await;
    assert!(rig.downloader.cancel(None).await);
    until(&rig.downloader, "cancelled", |phase| {
        *phase == Phase::Stopped(Stop::Cancelled)
    })
    .await;
    // The cleanup is still blocked on the disk: the gate is still held.
    assert_eq!(released.load(Ordering::SeqCst), 0);
    rig.script.release();
    wait_for("the permit", || released.load(Ordering::SeqCst) == 1).await;
    assert_eq!(rig.rows.list().await.unwrap().len(), 1);
    assert!(!rig.model_file(&row).exists());
}

#[tokio::test]
async fn a_discard_during_the_cleanup_of_a_bad_download_removes_the_row() {
    let (rig, row) = mismatching(5_000).await;
    let (released, permit) = counter();
    let _release = rig.script.block_at(Fault::BeforeCleanup);
    rig.downloader.start(row.clone(), permit()).await.unwrap();
    wait_for("the cleanup", || {
        rig.script.hit_count(Fault::BeforeCleanup) > 0
    })
    .await;
    let discard = {
        let (downloader, id) = (rig.downloader.clone(), row.id);
        tokio::spawn(async move { downloader.discard(id, Vec::new(), None).await })
    };
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        !discard.is_finished(),
        "the discard waits for the blocked cleanup"
    );
    assert_eq!(released.load(Ordering::SeqCst), 0);
    rig.script.release();
    assert!(discard.await.unwrap().is_ok());
    assert_eq!(rig.rows.list().await.unwrap().len(), 0);
    assert!(!rig.partial_file(&row).exists() && !rig.model_file(&row).exists());
    wait_for("the permit", || released.load(Ordering::SeqCst) == 1).await;
    assert_eq!(rig.downloader.status().phase, Phase::Idle);
}

#[tokio::test]
async fn a_shutdown_during_the_cleanup_of_a_bad_download_ends_the_task() {
    let (rig, row) = mismatching(5_000).await;
    let (released, permit) = counter();
    let _release = rig.script.block_at(Fault::BeforeCleanup);
    rig.downloader.start(row.clone(), permit()).await.unwrap();
    wait_for("the cleanup", || {
        rig.script.hit_count(Fault::BeforeCleanup) > 0
    })
    .await;
    // Accepted while the cleanup is still blocked.
    tokio::time::timeout(Duration::from_secs(2), rig.downloader.shutdown())
        .await
        .expect("the shutdown was dropped");
    assert_eq!(
        released.load(Ordering::SeqCst),
        0,
        "the gate outlives the cleanup"
    );
    rig.script.release();
    wait_for("the permit", || released.load(Ordering::SeqCst) == 1).await;
    // The actor has really ended: nothing answers any more.
    assert!(
        rig.downloader
            .discard(row.id, Vec::new(), None)
            .await
            .is_err()
    );
    assert!(!rig.downloader.cancel(None).await);
}

#[tokio::test]
async fn the_gate_is_held_while_a_cancelled_publication_is_still_in_flight() {
    let rig = Rig::new(6_000).await;
    let row = rig.confirm().await;
    let (released, permit) = counter();
    let _release = rig.script.block_at(Fault::AfterVerify);
    rig.downloader.start(row.clone(), permit()).await.unwrap();
    wait_for("verified", || rig.script.hit_count(Fault::AfterVerify) > 0).await;
    assert!(rig.downloader.cancel(None).await);
    until(&rig.downloader, "cancelled", |phase| {
        *phase == Phase::Stopped(Stop::Cancelled)
    })
    .await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        released.load(Ordering::SeqCst),
        0,
        "a concurrent Try must stay gated"
    );
    rig.script.release();
    wait_for("the permit", || released.load(Ordering::SeqCst) == 1).await;
    // The abandoned publication did finish, and the row is still waiting for the record.
    assert!(rig.model_file(&row).exists());
    assert_eq!(rig.rows.list().await.unwrap()[0].state, State::Confirmed);
}

#[tokio::test]
async fn the_gate_is_held_until_a_discard_has_removed_what_the_publication_made() {
    let rig = Rig::new(6_000).await;
    let row = rig.confirm().await;
    let (released, permit) = counter();
    let _release = rig.script.block_at(Fault::AfterVerify);
    rig.downloader.start(row.clone(), permit()).await.unwrap();
    wait_for("verified", || rig.script.hit_count(Fault::AfterVerify) > 0).await;
    let discard = {
        let (downloader, id) = (rig.downloader.clone(), row.id);
        tokio::spawn(async move { downloader.discard(id, Vec::new(), None).await })
    };
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(released.load(Ordering::SeqCst), 0);
    rig.script.release();
    assert!(discard.await.unwrap().is_ok());
    // By the time the gate opens, the file the publication made is gone.
    wait_for("the permit", || released.load(Ordering::SeqCst) == 1).await;
    assert!(!rig.model_file(&row).exists());
    assert_eq!(rig.rows.list().await.unwrap().len(), 0);
}

#[tokio::test]
async fn the_task_stays_responsive_when_a_resume_follows_a_cancel_over_a_blocked_write() {
    let rig = Rig::new(10_000).await;
    let row = rig.confirm().await;
    let (released, permit) = counter();
    let _release = rig.script.block_at(Fault::BeforeWrite);
    rig.downloader.start(row.clone(), permit()).await.unwrap();
    wait_for("a write", || rig.script.hit_count(Fault::BeforeWrite) > 0).await;
    assert!(rig.downloader.cancel(None).await);
    // The abandoned write is still blocked: a new transfer is refused, not awaited.
    let refused = tokio::time::timeout(
        Duration::from_secs(2),
        rig.downloader.resume(row.id, permit()),
    )
    .await
    .expect("the resume was not answered");
    assert_eq!(refused, Err(Refusal::Finishing));
    // And a shutdown is answered while the write is blocked.
    tokio::time::timeout(Duration::from_secs(2), rig.downloader.shutdown())
        .await
        .expect("the shutdown was not answered");
    rig.script.release();
    wait_for("the permits", || released.load(Ordering::SeqCst) == 2).await;
}

#[tokio::test]
async fn a_discard_after_a_cancelled_publication_leaves_no_orphan_file() {
    let rig = Rig::new(6_000).await;
    let row = rig.confirm().await;
    let (released, permit) = counter();
    let _release = rig.script.block_at(Fault::AfterVerify);
    rig.downloader.start(row.clone(), permit()).await.unwrap();
    wait_for("verified", || rig.script.hit_count(Fault::AfterVerify) > 0).await;
    assert!(rig.downloader.cancel(None).await);
    rig.script.release();
    wait_for("the permit", || released.load(Ordering::SeqCst) == 1).await;
    assert!(
        rig.model_file(&row).exists(),
        "the abandoned publication succeeded"
    );
    rig.downloader
        .discard(row.id, Vec::new(), None)
        .await
        .unwrap();
    assert_eq!(rig.rows.list().await.unwrap().len(), 0);
    assert!(!rig.model_file(&row).exists(), "an unreferenced final file");
}

/// How a running download's discard is made to fail.
#[derive(Clone, Copy, Debug)]
enum Failure {
    ListRows,
    Unlink,
    Hash,
    DeleteRow,
}

#[tokio::test]
async fn a_failed_discard_of_the_running_download_leaves_it_stopped_with_the_reason() {
    for failure in [
        Failure::ListRows,
        Failure::Unlink,
        Failure::Hash,
        Failure::DeleteRow,
    ] {
        let rig = Rig::new(6_000).await;
        let row = rig.confirm().await;
        let (released, permit) = counter();
        // Blocked after verification, so the publication (and the hash of the discard) count.
        let _release = rig.script.block_at(Fault::AfterVerify);
        rig.downloader.start(row.clone(), permit()).await.unwrap();
        wait_for("verified", || rig.script.hit_count(Fault::AfterVerify) > 0).await;
        let discard = {
            let (downloader, id) = (rig.downloader.clone(), row.id);
            tokio::spawn(async move { downloader.discard(id, Vec::new(), None).await })
        };
        tokio::time::sleep(Duration::from_millis(50)).await;
        match failure {
            Failure::ListRows => rig.rows.fail_list.store(true, Ordering::SeqCst),
            Failure::Unlink => rig.script.fail_at(Fault::BeforeDiscard),
            Failure::Hash => rig.script.fail_at(Fault::ReadError),
            Failure::DeleteRow => rig.rows.fail_delete.store(true, Ordering::SeqCst),
        }
        rig.script.release();
        let result = discard.await.unwrap();
        assert!(result.is_err(), "{failure:?}");
        let status = rig.downloader.status();
        assert!(
            matches!(status.phase, Phase::Stopped(Stop::Failed(_))),
            "{failure:?}: {status:?}"
        );
        wait_for("the permit", || released.load(Ordering::SeqCst) == 1).await;
    }
}

#[tokio::test]
async fn a_discard_of_a_row_that_is_already_gone_leaves_the_status_idle() {
    let rig = Rig::new(6_000).await;
    let row = rig.confirm().await;
    let (released, permit) = counter();
    let _release = rig.script.block_at(Fault::BeforeWrite);
    rig.downloader.start(row.clone(), permit()).await.unwrap();
    wait_for("a write", || rig.script.hit_count(Fault::BeforeWrite) > 0).await;
    let discard = {
        let (downloader, id) = (rig.downloader.clone(), row.id);
        tokio::spawn(async move { downloader.discard(id, Vec::new(), None).await })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;
    rig.rows.rows.lock().unwrap().clear();
    rig.script.release();
    assert!(discard.await.unwrap().is_ok());
    assert_eq!(rig.downloader.status().phase, Phase::Idle);
    wait_for("the permit", || released.load(Ordering::SeqCst) == 1).await;
}

#[tokio::test]
async fn cancel_matches_only_the_running_transfer_and_the_task_writes_the_status() {
    let rig = Rig::new(30_000).await;
    // Nothing runs: nothing matches.
    assert!(!rig.downloader.cancel(None).await);
    assert!(!rig.downloader.cancel(Some(1)).await);
    let row = rig.confirm().await;
    let content = Arc::clone(&rig.content);
    rig.hub.respond(move |_| Reply::Slow {
        status: 200,
        headers: Vec::new(),
        body: content.to_vec(),
        piece: 1000,
        gap: Duration::from_millis(30),
    });
    rig.downloader.start(row.clone(), permit()).await.unwrap();
    until(
        &rig.downloader,
        "bytes",
        |phase| matches!(phase, Phase::Running { received, .. } if *received > 0),
    )
    .await;
    // A stale id does not match and changes nothing.
    assert!(!rig.downloader.cancel(Some(row.id + 100)).await);
    assert!(matches!(
        rig.downloader.status().phase,
        Phase::Running { .. }
    ));
    assert!(rig.downloader.cancel(Some(row.id)).await);
    until(&rig.downloader, "cancelled", |phase| {
        *phase == Phase::Stopped(Stop::Cancelled)
    })
    .await;
    // A stopped download is not cancelled again.
    assert!(!rig.downloader.cancel(Some(row.id)).await);
    // Nor a finished one.
    let rig = Rig::new(2_000).await;
    let done = rig.confirm().await;
    rig.downloader.start(done.clone(), permit()).await.unwrap();
    assert_eq!(finished(&rig.downloader).await, Phase::Done);
    assert!(!rig.downloader.cancel(Some(done.id)).await);
    assert_eq!(rig.downloader.status().phase, Phase::Done);
}

#[tokio::test]
async fn reconciliation_leaves_a_row_alone_when_its_partial_is_a_link() {
    let mut rig = Rig::new(2_000).await;
    let first = rig.confirm().await;
    let other_bytes = vec![4u8; 1_000];
    let second = rig
        .rows
        .insert(New {
            spec: SPEC.replace("granite-4.2-3b-Q4_K_M", "second"),
            file_name: "hf-second.gguf".into(),
            sha256: sha(&other_bytes),
            size: 1_000,
            ..rig.new_row()
        })
        .await
        .unwrap();
    for (name, bytes) in [
        (&first.file_name, &*rig.content),
        (&second.file_name, &other_bytes),
    ] {
        let path = rig.dir.path().join(name);
        std::fs::write(&path, bytes).unwrap();
        std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o600))
            .unwrap();
    }
    // The first row's partial name is a symbolic link: its probe fails, so the row is left
    // for a later start; the other row is still reconciled.
    std::os::unix::fs::symlink(
        rig.dir.path().join(&second.file_name),
        rig.partial_file(&first),
    )
    .unwrap();
    rig.restart().await;
    let states: Vec<(i64, State)> = rig
        .rows
        .list()
        .await
        .unwrap()
        .iter()
        .map(|row| (row.id, row.state))
        .collect();
    assert!(states.contains(&(first.id, State::Confirmed)), "{states:?}");
    assert!(states.contains(&(second.id, State::Complete)), "{states:?}");
}

#[tokio::test]
async fn a_shutdown_at_the_last_step_is_accepted_while_blocked_and_the_row_heals_at_restart() {
    let mut rig = Rig::new(3_000).await;
    let row = rig.confirm().await;
    let (released, permit) = counter();
    let _release = rig.script.block_at(Fault::BeforeCommit);
    rig.downloader.start(row.clone(), permit()).await.unwrap();
    wait_for("the last step", || {
        rig.script.hit_count(Fault::BeforeCommit) > 0
    })
    .await;
    let shutdown = {
        let downloader = rig.downloader.clone();
        tokio::spawn(async move { downloader.shutdown().await })
    };
    // Accepted (answered) while the fault is still blocked.
    wait_for("the shutdown to be accepted", || shutdown.is_finished()).await;
    assert_eq!(released.load(Ordering::SeqCst), 0);
    rig.script.release();
    wait_for("the permit", || released.load(Ordering::SeqCst) == 1).await;
    assert_eq!(
        rig.downloader.status().phase,
        Phase::Stopped(Stop::Cancelled)
    );
    assert!(rig.model_file(&row).exists());
    assert_eq!(rig.rows.list().await.unwrap()[0].state, State::Confirmed);
    rig.restart().await;
    assert_eq!(rig.rows.list().await.unwrap()[0].state, State::Complete);
}
