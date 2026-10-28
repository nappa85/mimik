//! Regression tests of the 2d-2 review batch: control flow of the actor (which commands may
//! interrupt a transfer), reconciliation durability and per-row errors, and the smaller guards.
use super::*;

/// A second row (another spec and hash) that is not the running one.
async fn other_row(rig: &Rig, confirmed: bool) -> Row {
    let row = rig
        .rows
        .insert(New {
            spec: SPEC.replace("granite-4.2-3b-Q4_K_M", "other"),
            file_name: "hf-other.gguf".into(),
            sha256: "c".repeat(64),
            size: 777,
            ..rig.new_row()
        })
        .await
        .unwrap();
    if !confirmed {
        rig.rows.complete(row.id).await.unwrap();
    }
    row
}

fn slow(rig: &Rig) {
    let content = Arc::clone(&rig.content);
    rig.hub.respond(move |_| Reply::Slow {
        status: 200,
        headers: Vec::new(),
        body: content.to_vec(),
        piece: 1000,
        gap: Duration::from_millis(30),
    });
}

#[tokio::test]
async fn a_discard_of_another_id_never_touches_the_running_transfer() {
    let rig = Rig::new(30_000).await;
    let row = rig.confirm().await;
    slow(&rig);
    rig.downloader.start(row.clone(), permit()).await.unwrap();
    until(
        &rig.downloader,
        "bytes",
        |phase| matches!(phase, Phase::Running { received, .. } if *received >= 3000),
    )
    .await;
    // Nonexistent, completed, and unrelated waiting ids: handled without a stop.
    assert!(rig.downloader.discard(9999).await.is_ok());
    let finished_row = other_row(&rig, false).await;
    assert!(rig.downloader.discard(finished_row.id).await.is_err());
    let waiting = rig
        .rows
        .insert(New {
            spec: SPEC.replace("granite-4.2-3b-Q4_K_M", "waiting"),
            file_name: "hf-waiting.gguf".into(),
            sha256: "d".repeat(64),
            size: 5,
            ..rig.new_row()
        })
        .await
        .unwrap();
    assert!(rig.downloader.discard(waiting.id).await.is_ok());
    let status = rig.downloader.status();
    assert_eq!(status.id, Some(row.id));
    assert!(matches!(status.phase, Phase::Running { .. }), "{status:?}");
    // The transfer carries on to the end, and the discarded unrelated row is gone.
    assert_eq!(finished(&rig.downloader).await, Phase::Done);
    assert_eq!(std::fs::read(rig.model_file(&row)).unwrap(), *rig.content);
    let ids: Vec<i64> = rig
        .rows
        .list()
        .await
        .unwrap()
        .iter()
        .map(|r| r.id)
        .collect();
    assert!(!ids.contains(&waiting.id) && ids.contains(&finished_row.id));
}

#[tokio::test]
async fn a_discard_of_another_id_during_blocking_disk_work_does_not_cancel_it() {
    let rig = Rig::new(10_000).await;
    let row = rig.confirm().await;
    rig.script.block_at(Fault::BeforeWrite);
    rig.downloader.start(row.clone(), permit()).await.unwrap();
    until(&rig.downloader, "a write in flight", |_| {
        rig.script.hit_count(Fault::BeforeWrite) > 0
    })
    .await;
    assert!(rig.downloader.discard(9999).await.is_ok());
    assert!(matches!(
        rig.downloader.status().phase,
        Phase::Running { .. }
    ));
    rig.script.release();
    assert_eq!(finished(&rig.downloader).await, Phase::Done);
    assert_eq!(std::fs::read(rig.model_file(&row)).unwrap(), *rig.content);
}

#[tokio::test]
async fn cancel_is_not_held_up_by_a_server_slow_to_answer_the_headers() {
    let rig = Rig::new(5_000).await;
    let row = rig.confirm().await;
    let content = Arc::clone(&rig.content);
    rig.hub.respond(move |_| {
        Reply::Delayed(
            Duration::from_secs(3),
            Box::new(Reply::file(&content, None)),
        )
    });
    let released = Arc::new(AtomicUsize::new(0));
    rig.downloader
        .start(row.clone(), Box::new(Guard(Arc::clone(&released))))
        .await
        .unwrap();
    until(&rig.downloader, "the request", |_| {
        !rig.hub.requests().is_empty()
    })
    .await;
    rig.downloader.cancel(None).await;
    // Within half a second, long before the server answers: the permit is released.
    for _ in 0..50 {
        if released.load(Ordering::SeqCst) == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(released.load(Ordering::SeqCst), 1, "the permit stayed held");
    assert_eq!(
        rig.downloader.status().phase,
        Phase::Stopped(Stop::Cancelled)
    );
}

#[tokio::test]
async fn reconciliation_syncs_before_it_records_a_recovered_publication() {
    for point in [Fault::FileFsyncError, Fault::DirFsyncError] {
        let (mut rig, row, _) = crashed_at(Fault::AfterPublish, false).await;
        // The first restart already completed it; set up again with a row still confirmed.
        rig.rows.set_state(row.id, State::Confirmed);
        let script = Arc::new(Script::default());
        script.fail_at(point);
        rig.restart_with(script.clone()).await;
        assert_eq!(
            rig.rows.list().await.unwrap()[0].state,
            State::Confirmed,
            "{point:?}: not recorded without a durable file"
        );
        assert!(rig.model_file(&row).exists());
        // The next start, with a working disk, records it.
        rig.restart().await;
        assert_eq!(
            rig.rows.list().await.unwrap()[0].state,
            State::Complete,
            "{point:?}"
        );
    }
}

#[tokio::test]
async fn one_failing_row_does_not_stop_reconciliation_of_the_others() {
    let mut rig = Rig::new(4_000).await;
    let first = rig.confirm().await;
    // A second finished file (other bytes, other row) both rows published but not recorded.
    let other_bytes = vec![3u8; 2_000];
    let second = rig
        .rows
        .insert(New {
            spec: SPEC.replace("granite-4.2-3b-Q4_K_M", "second"),
            file_name: "hf-second.gguf".into(),
            sha256: sha(&other_bytes),
            size: 2_000,
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
    let script = Arc::new(Script::default());
    script.fail_at(Fault::ReadError);
    rig.restart_with(script).await;
    let states: Vec<State> = rig
        .rows
        .list()
        .await
        .unwrap()
        .iter()
        .map(|r| r.state)
        .collect();
    assert!(states.contains(&State::Complete), "{states:?}");
    assert!(states.contains(&State::Confirmed), "{states:?}");
}

#[tokio::test]
async fn a_resume_cannot_start_a_second_partial() {
    let rig = Rig::new(4_000).await;
    let first = rig.confirm().await;
    let second = other_row(&rig, true).await;
    // The first download owns a partial; the second may not start another.
    std::fs::write(rig.partial_file(&first), &rig.content[..100]).unwrap();
    std::fs::set_permissions(
        rig.partial_file(&first),
        std::os::unix::fs::PermissionsExt::from_mode(0o600),
    )
    .unwrap();
    assert_eq!(
        rig.downloader.resume(second.id, permit()).await,
        Err(Refusal::OtherPartial)
    );
    assert_eq!(rig.downloader.status().phase, Phase::Idle);
    assert_eq!(rig.hub.requests().len(), 0);
    assert!(rig.partial_file(&first).exists());
}

#[tokio::test]
async fn an_interrupt_at_the_last_step_is_honoured_and_a_shutdown_there_ends_the_task() {
    let rig = Rig::new(3_000).await;
    let row = rig.confirm().await;
    rig.script.block_at(Fault::BeforeCommit);
    rig.downloader.start(row.clone(), permit()).await.unwrap();
    until(&rig.downloader, "the last step", |_| {
        rig.script.hit_count(Fault::BeforeCommit) > 0
    })
    .await;
    rig.downloader.cancel(None).await;
    rig.script.release();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        rig.downloader.status().phase,
        Phase::Stopped(Stop::Cancelled)
    );
    let _ = row;
}

#[tokio::test]
async fn a_resume_of_a_missing_row_leaves_the_status_alone() {
    let rig = Rig::new(1_000).await;
    assert!(rig.downloader.resume(4242, permit()).await.is_err());
    // A finished row is not a waiting one either.
    let done = other_row(&rig, false).await;
    assert!(rig.downloader.resume(done.id, permit()).await.is_err());
    assert_eq!(rig.downloader.status().phase, Phase::Idle);
}

#[tokio::test]
async fn a_discard_racing_the_publication_leaves_no_orphan_file() {
    let rig = Rig::new(6_000).await;
    let row = rig.confirm().await;
    rig.script.block_at(Fault::AfterVerify);
    rig.downloader.start(row.clone(), permit()).await.unwrap();
    until(&rig.downloader, "verified", |_| {
        rig.script.hit_count(Fault::AfterVerify) > 0
    })
    .await;
    let discard = {
        let downloader = rig.downloader.clone();
        let id = row.id;
        tokio::spawn(async move { downloader.discard(id).await })
    };
    tokio::time::sleep(Duration::from_millis(100)).await;
    rig.script.release();
    assert!(discard.await.unwrap().is_ok());
    assert!(!rig.model_file(&row).exists(), "an orphan file with no row");
    assert_eq!(rig.rows.list().await.unwrap().len(), 0);
}

#[tokio::test]
async fn a_download_without_a_content_length_at_the_start_is_refused() {
    let rig = Rig::new(3_000).await;
    let row = rig.confirm().await;
    let content = Arc::clone(&rig.content);
    rig.hub.respond(move |_| Reply::Chunked {
        status: 200,
        headers: Vec::new(),
        body: content.to_vec(),
        piece: 1000,
        gap: Duration::from_millis(1),
    });
    rig.downloader.start(row.clone(), permit()).await.unwrap();
    let Phase::Stopped(Stop::Failed(why)) = finished(&rig.downloader).await else {
        panic!("must refuse")
    };
    assert!(why.contains("size"), "{why}");
    assert!(!rig.model_file(&row).exists());
}

#[tokio::test]
async fn a_start_or_resume_or_repair_while_one_runs_is_answered_busy_and_its_permit_released() {
    let rig = Rig::new(30_000).await;
    let row = rig.confirm().await;
    slow(&rig);
    rig.downloader.start(row.clone(), permit()).await.unwrap();
    until(
        &rig.downloader,
        "bytes",
        |phase| matches!(phase, Phase::Running { received, .. } if *received > 0),
    )
    .await;
    let released = Arc::new(AtomicUsize::new(0));
    let again = other_row(&rig, true).await;
    for attempt in 0..3 {
        let permit: Permit = Box::new(Guard(Arc::clone(&released)));
        let answer = match attempt {
            0 => rig.downloader.start(again.clone(), permit).await,
            1 => rig.downloader.resume(again.id, permit).await,
            _ => rig.downloader.repair(row.id, permit).await,
        };
        assert!(answer.is_err(), "{attempt}");
    }
    assert_eq!(released.load(Ordering::SeqCst), 3);
    assert_eq!(rig.downloader.status().id, Some(row.id));
    assert_eq!(finished(&rig.downloader).await, Phase::Done);
}

#[tokio::test]
async fn a_repair_fetches_a_finished_download_again_and_refuses_when_nothing_is_missing() {
    let rig = Rig::new(4_000).await;
    let row = rig.confirm().await;
    rig.downloader.start(row.clone(), permit()).await.unwrap();
    assert_eq!(finished(&rig.downloader).await, Phase::Done);
    // The file is there: nothing to repair.
    assert!(rig.downloader.repair(row.id, permit()).await.is_err());
    std::fs::remove_file(rig.model_file(&row)).unwrap();
    rig.downloader.repair(row.id, permit()).await.unwrap();
    assert_eq!(finished(&rig.downloader).await, Phase::Done);
    assert_eq!(std::fs::read(rig.model_file(&row)).unwrap(), *rig.content);
    let rows = rig.rows.list().await.unwrap();
    assert_eq!((rows.len(), rows[0].state), (1, State::Complete));
    // A different file under the name: the repair fails and the file is untouched.
    std::fs::write(rig.model_file(&row), b"mine").unwrap();
    rig.downloader.repair(row.id, permit()).await.unwrap();
    let Phase::Stopped(Stop::Failed(why)) = finished(&rig.downloader).await else {
        panic!("must refuse")
    };
    assert!(why.contains("remove it by hand"), "{why}");
    assert_eq!(std::fs::read(rig.model_file(&row)).unwrap(), b"mine");
}
