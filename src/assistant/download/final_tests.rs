//! Regression tests of the 2d-2 final round: a discard decides about a file from what is on disk
//! and in the records (never from memory), every discard that touches files holds the
//! model-work permit, and discard cleanup is tracked work the task answers around.
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

/// A row whose publication happened but was never recorded: the final file is in place, the
/// row still waits (a Cancel at the publication, or a crash before the commit).
async fn published_not_recorded(rig: &Rig) -> Row {
    let row = rig.confirm().await;
    let (released, permit) = counter();
    let release = rig.script.block_at(Fault::AfterVerify);
    rig.downloader.start(row.clone(), permit()).await.unwrap();
    wait_for("verified", || rig.script.hit_count(Fault::AfterVerify) > 0).await;
    assert!(rig.downloader.cancel(None).await);
    drop(release);
    wait_for("the permit", || released.load(Ordering::SeqCst) == 1).await;
    assert!(rig.model_file(&row).exists());
    assert_eq!(rig.rows.list().await.unwrap()[0].state, State::Confirmed);
    row
}

#[tokio::test]
async fn a_retry_after_a_failed_discard_still_removes_the_downloads_own_file() {
    let rig = Rig::new(6_000).await;
    let row = published_not_recorded(&rig).await;
    rig.script.fail_at(Fault::BeforeDiscard);
    assert!(
        rig.downloader
            .discard(row.id, Vec::new(), None)
            .await
            .is_err()
    );
    // Nothing was lost by the failure: the file and the record are still there.
    assert!(rig.model_file(&row).exists());
    assert_eq!(rig.rows.list().await.unwrap().len(), 1);
    assert_eq!(
        rig.downloader.discard(row.id, Vec::new(), None).await,
        Ok(Discarded::FileRemoved)
    );
    assert!(!rig.model_file(&row).exists());
    assert_eq!(rig.rows.list().await.unwrap().len(), 0);
}

#[tokio::test]
async fn an_unrelated_discard_first_does_not_forget_which_file_is_whose() {
    let rig = Rig::new(6_000).await;
    let row = published_not_recorded(&rig).await;
    // A nonexistent id, and another waiting download, are discarded first.
    assert_eq!(
        rig.downloader.discard(9999, Vec::new(), None).await,
        Ok(Discarded::Clean)
    );
    let other = rig
        .rows
        .insert(New {
            spec: SPEC.replace("granite-4.2-3b-Q4_K_M", "other"),
            file_name: "hf-other.gguf".into(),
            sha256: "c".repeat(64),
            size: 5,
            ..rig.new_row()
        })
        .await
        .unwrap();
    assert!(
        rig.downloader
            .discard(other.id, Vec::new(), None)
            .await
            .is_ok()
    );
    assert_eq!(
        rig.downloader.discard(row.id, Vec::new(), None).await,
        Ok(Discarded::FileRemoved)
    );
    assert!(!rig.model_file(&row).exists());
}

#[tokio::test]
async fn another_download_completing_first_does_not_orphan_the_file_of_the_first() {
    let rig = Rig::new(6_000).await;
    let first = published_not_recorded(&rig).await;
    // A second, different download runs to the end.
    let second_bytes = vec![5u8; 2_000];
    let content = Arc::clone(&rig.content);
    let served = second_bytes.clone();
    rig.hub.respond(move |seen| {
        if seen.path.contains("other") {
            Reply::file(&served, None)
        } else {
            Reply::file(&content, None)
        }
    });
    let second = rig
        .rows
        .insert(New {
            spec: SPEC.replace("granite-4.2-3b-Q4_K_M", "other"),
            file_name: "hf-second.gguf".into(),
            sha256: sha(&second_bytes),
            size: 2_000,
            ..rig.new_row()
        })
        .await
        .unwrap();
    // The first download's partial is gone (it was published), so nothing blocks the second.
    rig.downloader
        .start(second.clone(), permit())
        .await
        .unwrap();
    assert_eq!(finished(&rig.downloader).await, Phase::Done);
    assert_eq!(
        rig.downloader.discard(first.id, Vec::new(), None).await,
        Ok(Discarded::FileRemoved)
    );
    assert!(!rig.model_file(&first).exists());
    assert!(
        rig.dir.path().join("hf-second.gguf").exists(),
        "the second is untouched"
    );
}

#[tokio::test]
async fn a_file_that_is_shared_in_use_protected_or_not_verified_is_left_and_said_so() {
    // Shared with another record.
    let rig = Rig::new(6_000).await;
    let row = published_not_recorded(&rig).await;
    rig.rows
        .insert(New {
            spec: SPEC.replace("granite-4.2-3b-Q4_K_M", "twin"),
            ..rig.new_row()
        })
        .await
        .unwrap();
    assert_eq!(
        rig.downloader.discard(row.id, Vec::new(), None).await,
        Ok(Discarded::FileKept)
    );
    assert!(rig.model_file(&row).exists());
    // In use by the model setting.
    let rig = Rig::new(6_000).await;
    let row = published_not_recorded(&rig).await;
    rig.rows.in_use.lock().unwrap().push(row.file_name.clone());
    assert_eq!(
        rig.downloader.discard(row.id, Vec::new(), None).await,
        Ok(Discarded::FileKept)
    );
    assert!(rig.model_file(&row).exists());
    // Protected by the caller (a pinned default or the effective model).
    let rig = Rig::new(6_000).await;
    let row = published_not_recorded(&rig).await;
    assert_eq!(
        rig.downloader
            .discard(row.id, vec![row.file_name.clone()], None)
            .await,
        Ok(Discarded::FileKept)
    );
    assert!(rig.model_file(&row).exists());
    // Not exactly this download's: the same size with other bytes, or another size.
    for other in [vec![8u8; 6_000], vec![8u8; 10]] {
        let rig = Rig::new(6_000).await;
        let row = rig.confirm().await;
        std::fs::write(rig.model_file(&row), &other).unwrap();
        std::fs::set_permissions(
            rig.model_file(&row),
            std::os::unix::fs::PermissionsExt::from_mode(0o600),
        )
        .unwrap();
        assert_eq!(
            rig.downloader.discard(row.id, Vec::new(), None).await,
            Ok(Discarded::FileKept)
        );
        assert_eq!(std::fs::read(rig.model_file(&row)).unwrap(), other);
        assert_eq!(
            rig.rows.list().await.unwrap().len(),
            0,
            "the record still goes"
        );
    }
}

#[tokio::test]
async fn a_discard_holds_its_permit_until_the_cleanup_has_finished() {
    let rig = Rig::new(6_000).await;
    let row = rig.confirm().await;
    let (released, permit) = counter();
    let release = rig.script.block_at(Fault::BeforeDiscard);
    let discard = {
        let (downloader, id, permit) = (rig.downloader.clone(), row.id, permit());
        tokio::spawn(async move { downloader.discard(id, Vec::new(), Some(permit)).await })
    };
    wait_for("the cleanup", || {
        rig.script.hit_count(Fault::BeforeDiscard) > 0
    })
    .await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(released.load(Ordering::SeqCst), 0, "a Try must stay gated");
    drop(release);
    assert!(discard.await.unwrap().is_ok());
    wait_for("the permit", || released.load(Ordering::SeqCst) == 1).await;
}

#[tokio::test]
async fn a_shutdown_after_the_cleanup_entered_blocking_work_ends_the_task() {
    let rig = Rig::new(6_000).await;
    let row = rig.confirm().await;
    let (released, permit) = counter();
    let (released_waiting, permit_waiting) = counter();
    let release = rig.script.block_at(Fault::BeforeDiscard);
    let discard = {
        let (downloader, id, permit) = (rig.downloader.clone(), row.id, permit());
        tokio::spawn(async move { downloader.discard(id, Vec::new(), Some(permit)).await })
    };
    wait_for("the cleanup", || {
        rig.script.hit_count(Fault::BeforeDiscard) > 0
    })
    .await;
    // A second discard waits its turn behind the first.
    let waiting = {
        let (downloader, id, permit) = (rig.downloader.clone(), 4242, permit_waiting());
        tokio::spawn(async move { downloader.discard(id, Vec::new(), Some(permit)).await })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;
    // The task answers a shutdown although the cleanup is blocked in the disk.
    tokio::time::timeout(Duration::from_secs(2), rig.downloader.shutdown())
        .await
        .expect("the shutdown was not answered");
    assert_eq!(
        released.load(Ordering::SeqCst),
        0,
        "the cleanup keeps its permit"
    );
    // The waiting caller is told the task has ended; the running one still gets its answer.
    assert!(waiting.await.unwrap().is_err());
    assert_eq!(released_waiting.load(Ordering::SeqCst), 1);
    drop(release);
    assert!(discard.await.unwrap().is_ok());
    wait_for("the permit", || released.load(Ordering::SeqCst) == 1).await;
    assert_eq!(rig.rows.list().await.unwrap().len(), 0);
}
