use super::super::contract::{EnvelopeTarget, TARGET_ALIAS};
use super::super::interpreter::Failure;
use super::super::interpreter::stub::StubInterpreter;
use super::*;
use std::sync::atomic::Ordering;
use tokio::sync::Semaphore;

const HEAL: &str = r#"{"kind":"options","options":[{"expr":{"op":"add","l":{"ref":"t"},"r":{"lit":15}},"explanation":"heal 15"}]}"#;

fn budgets() -> Budgets {
    Budgets {
        bounds: Bounds {
            bytes: 1024,
            tokens: 384,
        },
        run: Duration::from_secs(85),
        depth: 1,
        queue_wait: Duration::from_secs(35),
        max_tokens: 352,
        busy_after_expiry: Duration::ZERO,
    }
}

fn draft(phrase: &str) -> Draft {
    Draft {
        phrase: phrase.into(),
        target: EnvelopeTarget {
            alias: TARGET_ALIAS,
            id: "hp".into(),
            label: Some("Current HP".into()),
            value: Some(84),
        },
        target_address: "field:hp".into(),
        items: Vec::new(),
    }
}

fn origin(sheet: i64, target: &str, revision: i64) -> Origin {
    Origin {
        sheet,
        target: target.into(),
        revision,
        kind: Kind::Interpretation,
    }
}

/// A ready assistant over `stub`, with its worker running.
fn ready(stub: Arc<StubInterpreter>, budgets: Budgets) -> Arc<Assistant> {
    let assistant = Assistant::new();
    // Tests never depend on real time beating the grace: a short one where failures must
    // count, a long one where the exit is published first.
    assistant.set_transport_grace(Duration::from_millis(10));
    assistant.event(RuntimeEvent::GenerationStarted { generation: 1 });
    assistant.event(RuntimeEvent::Ready {
        generation: 1,
        budgets,
        interpreter: stub,
        path: Path::Cpu,
        reminders: Reminders::Unavailable("test".into()),
    });
    tokio::spawn(Arc::clone(&assistant).work());
    assistant
}

fn gated(answers: usize) -> (Arc<StubInterpreter>, Arc<Semaphore>) {
    let stub = Arc::new(StubInterpreter::answering(
        (0..answers).map(|_| Ok(HEAL.to_owned())),
    ));
    let gate = Arc::new(Semaphore::new(0));
    *stub.gate.lock().unwrap() = Some(Arc::clone(&gate));
    (stub, gate)
}

fn start(
    assistant: &Arc<Assistant>,
    account: i64,
    origin: Origin,
) -> Result<(JobId, bool), Refusal> {
    let registration = assistant.register(origin.sheet, origin.revision);
    assistant.start(&registration, account, origin, draft("heal 15"))
}

async fn settle() {
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
}

async fn until_done(assistant: &Assistant, account: i64, sheet: i64, id: JobId) -> Poll {
    for _ in 0..200 {
        let poll = assistant.poll(account, sheet, id);
        if matches!(poll, Poll::Done(_) | Poll::Missing) {
            return poll;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assistant.poll(account, sheet, id)
}

#[tokio::test]
async fn a_job_runs_and_offers_its_option() {
    let stub = Arc::new(StubInterpreter::answering([Ok(HEAL.to_owned())]));
    let assistant = ready(Arc::clone(&stub), budgets());
    let (id, repeat) = start(&assistant, 1, origin(1, "field:hp", 5)).unwrap();
    assert!(!repeat);
    let Poll::Done(Answer::Options(options)) = until_done(&assistant, 1, 1, id).await else {
        panic!()
    };
    let [change] = options[0].changes.as_slice() else {
        panic!("an interpretation option is one change")
    };
    assert_eq!(change.address, "field:hp");
    assert_eq!(change.after(), NoteValue::Int(99));
    assert_eq!(change.before, NoteValue::Int(84));
    assert_eq!(change.shown(), "Current HP (84) + 15");
    assert_eq!(options[0].explanation, "heal 15");
    assert!(assistant.proposal(1, 1, id, 0).is_some());
    assert!(assistant.proposal(1, 1, id, 1).is_none());
    // Another account's or sheet's job is not found.
    assert_eq!(assistant.poll(2, 1, id), Poll::Missing);
    assert_eq!(assistant.poll(1, 2, id), Poll::Missing);
    assert!(assistant.proposal(2, 1, id, 0).is_none());
    assert!(assistant.proposal(1, 2, id, 0).is_none());
    assert_eq!(stub.requests.lock().unwrap()[0].max_tokens, 352);
}

#[tokio::test]
async fn a_repeated_start_returns_the_same_job_and_cancels_nothing() {
    let (stub, gate) = gated(1);
    let assistant = ready(stub, budgets());
    let (first, _) = start(&assistant, 1, origin(1, "field:hp", 5)).unwrap();
    settle().await;
    let (second, repeat) = start(&assistant, 1, origin(1, "field:hp", 5)).unwrap();
    assert_eq!(first, second);
    assert!(repeat);
    assert_eq!(assistant.poll(1, 1, first), Poll::Running);
    gate.add_permits(1);
    assert!(matches!(
        until_done(&assistant, 1, 1, first).await,
        Poll::Done(Answer::Options(_))
    ));
    // A finished job with applicable options still counts as a repeat.
    assert_eq!(
        start(&assistant, 1, origin(1, "field:hp", 5)).unwrap(),
        (first, true)
    );
}

#[tokio::test]
async fn a_newer_job_of_the_account_supersedes_and_an_older_one_is_refused() {
    let (stub, gate) = gated(2);
    let assistant = ready(stub, budgets());
    // B's start first (revision 7), then A's older one (revision 6): A is refused.
    let (b, _) = start(&assistant, 1, origin(1, "field:max_hp", 7)).unwrap();
    settle().await;
    assert_eq!(
        start(&assistant, 1, origin(1, "field:hp", 6)),
        Err(Refusal::Conflict)
    );
    assert_eq!(assistant.poll(1, 1, b), Poll::Running);
    // A newer one on another target supersedes B.
    let (c, _) = start(&assistant, 1, origin(1, "field:hp", 8)).unwrap();
    assert_eq!(assistant.poll(1, 1, b), Poll::Done(Answer::Superseded));
    // Two targets written in one revision: the later start supersedes.
    let (d, _) = start(&assistant, 1, origin(1, "field:luck", 8)).unwrap();
    assert_eq!(assistant.poll(1, 1, c), Poll::Done(Answer::Superseded));
    gate.add_permits(2);
    assert!(matches!(
        until_done(&assistant, 1, 1, d).await,
        Poll::Done(Answer::Options(_))
    ));
}

#[tokio::test]
async fn a_start_on_a_second_sheet_while_a_job_is_live_is_busy() {
    let (stub, gate) = gated(1);
    let assistant = ready(stub, budgets());
    let (first, _) = start(&assistant, 1, origin(1, "field:hp", 5)).unwrap();
    assert_eq!(
        start(&assistant, 1, origin(2, "field:hp", 9)),
        Err(Refusal::Busy)
    );
    gate.add_permits(1);
    assert!(matches!(
        until_done(&assistant, 1, 1, first).await,
        Poll::Done(Answer::Options(_))
    ));
    assert!(start(&assistant, 1, origin(2, "field:hp", 9)).is_ok());
}

#[tokio::test(start_paused = true)]
async fn busy_admission_follows_the_queue_budget() {
    let (stub, gate) = gated(3);
    let assistant = ready(stub, budgets());
    let (running, _) = start(&assistant, 1, origin(1, "field:hp", 5)).unwrap();
    settle().await;
    assert_eq!(assistant.poll(1, 1, running), Poll::Running);
    // More than Q (35 s) of the running job's 85 s left: busy, and nothing is cancelled.
    tokio::time::advance(Duration::from_secs(49)).await;
    assert_eq!(
        start(&assistant, 2, origin(2, "field:hp", 5)),
        Err(Refusal::Busy)
    );
    assert_eq!(assistant.poll(1, 1, running), Poll::Running);
    // Less than Q left: queued.
    tokio::time::advance(Duration::from_secs(2)).await;
    let (queued, _) = start(&assistant, 2, origin(2, "field:hp", 5)).unwrap();
    assert_eq!(assistant.poll(2, 2, queued), Poll::Queued);
    // A second waiting job beyond D (1): busy.
    assert_eq!(
        start(&assistant, 3, origin(3, "field:hp", 5)),
        Err(Refusal::Busy)
    );
    // The account's own newer start supersedes its queued job although the queue is full.
    let (replacement, _) = start(&assistant, 2, origin(2, "field:hp", 6)).unwrap();
    assert_eq!(assistant.poll(2, 2, queued), Poll::Done(Answer::Superseded));
    assert_eq!(assistant.poll(2, 2, replacement), Poll::Queued);
    assert_eq!(assistant.poll(1, 1, running), Poll::Running);
    gate.add_permits(3);
}

#[tokio::test(start_paused = true)]
async fn queue_wait_and_run_deadline_expire_neutrally() {
    let (stub, gate) = gated(2);
    let mut long = budgets();
    long.queue_wait = Duration::from_secs(120);
    let assistant = ready(stub, long);
    let (running, _) = start(&assistant, 1, origin(1, "field:hp", 5)).unwrap();
    settle().await;
    let (waiting, _) = start(&assistant, 2, origin(2, "field:hp", 5)).unwrap();
    // The running job reaches its 85 s run deadline: "ask again".
    tokio::time::advance(Duration::from_secs(86)).await;
    settle().await;
    assert_eq!(assistant.poll(1, 1, running), Poll::Done(Answer::AskAgain));
    // The waiting job then runs, within its queue wait.
    settle().await;
    assert_eq!(assistant.poll(2, 2, waiting), Poll::Running);
    tokio::time::advance(Duration::from_secs(86)).await;
    settle().await;
    assert_eq!(assistant.poll(2, 2, waiting), Poll::Done(Answer::AskAgain));
    assert!(assistant.breaker_closed());
    gate.add_permits(2);
}

#[tokio::test(start_paused = true)]
async fn a_job_past_its_queue_wait_never_runs() {
    let stub = Arc::new(StubInterpreter::answering([Ok(HEAL.to_owned())]));
    let assistant = Assistant::new();
    assistant.event(RuntimeEvent::GenerationStarted { generation: 1 });
    assistant.event(RuntimeEvent::Ready {
        generation: 1,
        budgets: budgets(),
        interpreter: Arc::clone(&stub) as Arc<dyn Interpreter>,
        path: Path::Cpu,
        reminders: Reminders::Unavailable("test".into()),
    });
    // The worker is late: the job waits past Q (35 s) before it is picked up.
    let (job, _) = start(&assistant, 1, origin(1, "field:hp", 5)).unwrap();
    tokio::time::advance(Duration::from_secs(36)).await;
    tokio::spawn(Arc::clone(&assistant).work());
    settle().await;
    assert_eq!(assistant.poll(1, 1, job), Poll::Done(Answer::AskAgain));
    assert!(stub.requests.lock().unwrap().is_empty());
    assert!(assistant.breaker_closed());
}

#[tokio::test]
async fn a_note_written_after_the_database_check_refuses_the_start() {
    let stub = Arc::new(StubInterpreter::default());
    let assistant = ready(stub, budgets());
    let registration = assistant.register(1, 5);
    // The save commits between the start's database check and its enqueue.
    assistant.note_written(1, 6, &["field:hp".to_owned()]);
    assert_eq!(
        assistant.start(&registration, 1, origin(1, "field:hp", 5), draft("heal 15")),
        Err(Refusal::Conflict)
    );
}

#[tokio::test]
async fn a_registered_old_origin_keeps_the_entries_it_needs() {
    let stub = Arc::new(StubInterpreter::default());
    let assistant = ready(stub, budgets());
    let registration = assistant.register(1, 5);
    // Many writes on the sheet meanwhile, with nothing else registered or live.
    assistant.note_written(1, 6, &["field:hp".to_owned()]);
    for revision in 7..50 {
        assistant.note_written(1, revision, &["field:luck".to_owned()]);
    }
    assert_eq!(
        assistant.start(&registration, 1, origin(1, "field:hp", 5), draft("heal 15")),
        Err(Refusal::Conflict)
    );
    drop(registration);
    // Once nothing needs it, the sheet's entry goes.
    assert!(!assistant.table().writes.contains_key(&1));
}

#[tokio::test]
async fn a_later_note_write_cancels_the_job_or_drops_its_options() {
    let (stub, gate) = gated(2);
    let assistant = ready(stub, budgets());
    let (running, _) = start(&assistant, 1, origin(1, "field:hp", 5)).unwrap();
    settle().await;
    // Scribble A then scribble B (or a typed value): the running job is cancelled.
    assistant.note_written(1, 6, &["field:hp".to_owned()]);
    assert_eq!(assistant.poll(1, 1, running), Poll::Done(Answer::Stale));
    // Its late answer is discarded.
    gate.add_permits(1);
    settle().await;
    assert_eq!(assistant.poll(1, 1, running), Poll::Done(Answer::Stale));
    // A finished job loses its options on another edit of its target.
    let (done, _) = start(&assistant, 1, origin(1, "field:hp", 6)).unwrap();
    gate.add_permits(1);
    assert!(matches!(
        until_done(&assistant, 1, 1, done).await,
        Poll::Done(Answer::Options(_))
    ));
    assistant.note_written(1, 7, &["field:hp".to_owned()]);
    assert_eq!(assistant.poll(1, 1, done), Poll::Done(Answer::Stale));
    assert!(assistant.proposal(1, 1, done, 0).is_none());
}

#[tokio::test]
async fn an_unrelated_write_keeps_the_options_and_a_row_removal_drops_its_cells() {
    let stub = Arc::new(StubInterpreter::answering([
        Ok(HEAL.to_owned()),
        Ok(HEAL.to_owned()),
    ]));
    let assistant = ready(stub, budgets());
    let (job, _) = start(&assistant, 1, origin(1, "field:hp", 5)).unwrap();
    assert!(matches!(
        until_done(&assistant, 1, 1, job).await,
        Poll::Done(Answer::Options(_))
    ));
    assistant.note_written(1, 6, &["field:luck".to_owned()]);
    assert!(assistant.proposal(1, 1, job, 0).is_some());
    let (cell, _) = start(&assistant, 1, origin(1, "cell:gear/r1-1/weight", 7)).unwrap();
    assert!(matches!(
        until_done(&assistant, 1, 1, cell).await,
        Poll::Done(Answer::Options(_))
    ));
    assistant.note_written(1, 8, &["row:gear/r1-10".to_owned()]);
    assert!(assistant.proposal(1, 1, cell, 0).is_some());
    assistant.note_written(1, 9, &["row:gear/r1-1".to_owned()]);
    assert!(assistant.proposal(1, 1, cell, 0).is_none());
}

#[tokio::test(start_paused = true)]
async fn options_expire_after_ten_minutes() {
    let stub = Arc::new(StubInterpreter::answering([Ok(HEAL.to_owned())]));
    let assistant = ready(stub, budgets());
    let (job, _) = start(&assistant, 1, origin(1, "field:hp", 5)).unwrap();
    assert!(matches!(
        until_done(&assistant, 1, 1, job).await,
        Poll::Done(Answer::Options(_))
    ));
    tokio::time::advance(Duration::from_secs(599)).await;
    assert!(assistant.proposal(1, 1, job, 0).is_some());
    tokio::time::advance(Duration::from_secs(2)).await;
    assert!(assistant.proposal(1, 1, job, 0).is_none());
    assert_eq!(assistant.poll(1, 1, job), Poll::Done(Answer::AskAgain));
    // An expired job no longer counts as a repeat: the start goes on as a new job.
    let (again, repeat) = start(&assistant, 1, origin(1, "field:hp", 5)).unwrap();
    assert_ne!(again, job);
    assert!(!repeat);
}

#[tokio::test]
async fn sixty_four_records_evict_the_oldest_finished_never_a_live_one() {
    let answers: Vec<_> = (0..200).map(|_| Ok(HEAL.to_owned())).collect();
    let stub = Arc::new(StubInterpreter::answering(answers));
    let mut wide = budgets();
    wide.depth = 100;
    wide.queue_wait = Duration::from_secs(100_000);
    let assistant = ready(stub, wide);
    let mut first = None;
    for account in 0..MAX_RECORDS {
        let account = i64::try_from(account).unwrap();
        let (id, _) = start(&assistant, account, origin(account, "field:hp", 5)).unwrap();
        until_done(&assistant, account, account, id).await;
        first.get_or_insert(id);
    }
    assert_eq!(assistant.records(), MAX_RECORDS);
    let (_, _) = start(&assistant, 1000, origin(1000, "field:hp", 5)).unwrap();
    assert_eq!(assistant.records(), MAX_RECORDS);
    // The evicted job is not found: the route answers "ask again".
    assert_eq!(assistant.poll(0, 0, first.unwrap()), Poll::Missing);
}

#[tokio::test]
async fn sixty_four_live_jobs_make_a_start_busy() {
    let (stub, gate) = gated(100);
    let mut wide = budgets();
    wide.depth = 100;
    wide.queue_wait = Duration::from_secs(100_000);
    let assistant = ready(stub, wide);
    for account in 0..MAX_RECORDS {
        let account = i64::try_from(account).unwrap();
        start(&assistant, account, origin(account, "field:hp", 5)).unwrap();
    }
    assert_eq!(
        start(&assistant, 1000, origin(1000, "field:hp", 5)),
        Err(Refusal::Busy)
    );
    gate.add_permits(100);
}

#[tokio::test]
async fn a_generation_change_ends_every_job_neutrally() {
    let (stub, gate) = gated(2);
    let mut long = budgets();
    long.queue_wait = Duration::from_secs(120);
    let assistant = ready(Arc::clone(&stub), long);
    let (running, _) = start(&assistant, 1, origin(1, "field:hp", 5)).unwrap();
    settle().await;
    let (queued, _) = start(&assistant, 2, origin(2, "field:hp", 5)).unwrap();
    assistant.event(RuntimeEvent::GenerationEnded {
        generation: 1,
        reason: EndReason::Exited,
    });
    assert_eq!(assistant.poll(1, 1, running), Poll::Done(Answer::Restarted));
    assert_eq!(assistant.poll(2, 2, queued), Poll::Done(Answer::Restarted));
    assert_eq!(
        start(&assistant, 3, origin(3, "field:hp", 5)),
        Err(Refusal::Runtime(RuntimeState::Starting))
    );
    gate.add_permits(2);
    assert!(assistant.breaker_closed());
}

#[tokio::test]
async fn a_transport_error_before_the_generation_ends_resolves_as_restarted_and_neutral() {
    // The child's death closes the connection first; its exit is published afterwards. The
    // job must not be settled by the transport error alone.
    let stub = Arc::new(StubInterpreter::answering([Err(Failure::Transport)]));
    let assistant = ready(stub, budgets());
    assistant.set_transport_grace(Duration::from_secs(3600));
    let (job, _) = start(&assistant, 1, origin(1, "field:hp", 5)).unwrap();
    settle().await;
    assert!(
        !matches!(assistant.poll(1, 1, job), Poll::Done(_)),
        "the job was settled by the transport error before the exit was published"
    );
    assistant.event(RuntimeEvent::GenerationEnded {
        generation: 1,
        reason: EndReason::Exited,
    });
    assert_eq!(assistant.poll(1, 1, job), Poll::Done(Answer::Restarted));
    settle().await;
    assert!(assistant.breaker_closed());
}

#[tokio::test]
async fn a_transport_error_on_a_generation_that_stays_alive_counts_after_the_grace() {
    let stub = Arc::new(StubInterpreter::answering([Err(Failure::Transport)]));
    let assistant = ready(stub, budgets());
    let (job, _) = start(&assistant, 1, origin(1, "field:hp", 5)).unwrap();
    assert_eq!(
        until_done(&assistant, 1, 1, job).await,
        Poll::Done(Answer::Unavailable)
    );
}

#[tokio::test]
async fn an_http_status_or_a_tokenizer_failure_counts_at_once_without_the_grace() {
    for tokenizer in [false, true] {
        let stub = Arc::new(StubInterpreter::answering([Err(Failure::Status(500))]));
        if tokenizer {
            *stub.tokenize_fails.lock().unwrap() = Some(Failure::Transport);
        }
        let assistant = ready(stub, budgets());
        // A grace that would never end in a test: only an immediate answer passes.
        assistant.set_transport_grace(Duration::from_secs(3600));
        let (job, _) = start(&assistant, 1, origin(1, "field:hp", 5)).unwrap();
        assert_eq!(
            until_done(&assistant, 1, 1, job).await,
            Poll::Done(Answer::Unavailable),
            "tokenizer: {tokenizer}"
        );
    }
}

#[tokio::test]
async fn a_supersession_ends_the_transport_grace_at_once() {
    let stub = Arc::new(StubInterpreter::answering([
        Err(Failure::Transport),
        Ok(HEAL.to_owned()),
    ]));
    let assistant = ready(stub, budgets());
    assistant.set_transport_grace(Duration::from_secs(3600));
    let (first, _) = start(&assistant, 1, origin(1, "field:hp", 5)).unwrap();
    settle().await;
    assert!(!matches!(assistant.poll(1, 1, first), Poll::Done(_)));
    // A newer job of the account replaces it: the worker is free for it at once.
    let (second, _) = start(&assistant, 1, origin(1, "field:hp", 6)).unwrap();
    assert_eq!(assistant.poll(1, 1, first), Poll::Done(Answer::Superseded));
    assert!(matches!(
        until_done(&assistant, 1, 1, second).await,
        Poll::Done(Answer::Options(_))
    ));
}

#[tokio::test]
async fn transport_failures_open_the_breaker_and_a_start_reports_starting_first() {
    let stub = Arc::new(StubInterpreter::answering(
        (0..3).map(|_| Err(Failure::Status(500))),
    ));
    let assistant = ready(stub, budgets());
    for account in 0..3 {
        let (id, _) = start(&assistant, account, origin(account, "field:hp", 5)).unwrap();
        assert_eq!(
            until_done(&assistant, account, account, id).await,
            Poll::Done(Answer::Unavailable)
        );
    }
    assert!(!assistant.breaker_closed());
    assert_eq!(
        start(&assistant, 9, origin(9, "field:hp", 5)),
        Err(Refusal::Unavailable)
    );
    // While starting, the runtime state comes before the open breaker.
    assistant.event(RuntimeEvent::GenerationEnded {
        generation: 1,
        reason: EndReason::Restart,
    });
    assert_eq!(
        start(&assistant, 9, origin(9, "field:hp", 5)),
        Err(Refusal::Runtime(RuntimeState::Starting))
    );
}

#[tokio::test]
async fn a_failure_then_the_exit_within_two_seconds_is_withdrawn() {
    let stub = Arc::new(StubInterpreter::answering(
        (0..3).map(|_| Err(Failure::Transport)),
    ));
    let assistant = ready(stub, budgets());
    for account in 0..3 {
        let (id, _) = start(&assistant, account, origin(account, "field:hp", 5)).unwrap();
        until_done(&assistant, account, account, id).await;
    }
    assert!(!assistant.breaker_closed());
    assistant.event(RuntimeEvent::GenerationEnded {
        generation: 1,
        reason: EndReason::Exited,
    });
    assert!(assistant.breaker_closed());
}

#[tokio::test]
async fn only_restart_or_app_start_resets_the_breaker() {
    let stub = Arc::new(StubInterpreter::answering(
        (0..3).map(|_| Err(Failure::Status(500))),
    ));
    let assistant = ready(stub, budgets());
    for account in 0..3 {
        let (id, _) = start(&assistant, account, origin(account, "field:hp", 5)).unwrap();
        until_done(&assistant, account, account, id).await;
    }
    // A new child (a new generation) leaves it open.
    assistant.event(RuntimeEvent::GenerationStarted { generation: 2 });
    assert!(!assistant.breaker_closed());
    assistant.reset_breaker();
    assert!(assistant.breaker_closed());
}

#[tokio::test]
async fn too_long_sends_no_completion_and_counts_nothing() {
    let stub = Arc::new(StubInterpreter::default());
    let assistant = ready(Arc::clone(&stub), budgets());
    let registration = assistant.register(1, 5);
    let (id, _) = assistant
        .start(
            &registration,
            1,
            origin(1, "field:hp", 5),
            draft(&"x".repeat(2000)),
        )
        .unwrap();
    assert_eq!(
        until_done(&assistant, 1, 1, id).await,
        Poll::Done(Answer::TooLong)
    );
    assert!(stub.requests.lock().unwrap().is_empty());
    assert!(assistant.breaker_closed());
}

#[tokio::test]
async fn dropped_and_nonsense_answers_are_successes() {
    let answers = (0..100).map(|n| {
        Ok(if n % 2 == 0 {
            "nonsense".to_owned()
        } else {
            r#"{"kind":"options","options":[{"expr":{"lit":915},"explanation":"x"}]}"#.to_owned()
        })
    });
    let stub = Arc::new(StubInterpreter::answering(answers));
    let assistant = ready(stub, budgets());
    for account in 0..100 {
        let (id, _) = start(&assistant, account, origin(1, "field:hp", 5)).unwrap();
        assert_eq!(
            until_done(&assistant, account, 1, id).await,
            Poll::Done(Answer::Nothing)
        );
    }
    assert!(assistant.breaker_closed());
}

#[tokio::test]
async fn cancelling_a_running_job_closes_its_connection() {
    use super::super::fake_server::{self, Reply};
    use super::super::interpreter::RigInterpreter;
    use super::super::transport::{self, Endpoint};
    let (url, shared) = fake_server::tcp().await;
    shared.push(Reply::Slow(Duration::from_millis(300), HEAL.into()));
    shared.push(Reply::Content(HEAL.into()));
    let interpreter = RigInterpreter::new(
        transport::Bounded::direct(Endpoint::Tcp(transport::tcp(&url, false).unwrap())),
        "dummy-token-not-a-secret".into(),
        "test".into(),
    );
    let assistant = ready_with(Arc::new(interpreter));
    let (first, _) = start(&assistant, 1, origin(1, "field:hp", 5)).unwrap();
    for _ in 0..100 {
        if shared.open.load(Ordering::SeqCst) == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let (second, _) = start(&assistant, 1, origin(1, "field:hp", 6)).unwrap();
    assert_eq!(assistant.poll(1, 1, first), Poll::Done(Answer::Superseded));
    assert!(matches!(
        until_done(&assistant, 1, 1, second).await,
        Poll::Done(Answer::Options(_))
    ));
    assert_eq!(shared.disconnected.load(Ordering::SeqCst), 1);
    assert_eq!(shared.open.load(Ordering::SeqCst), 0);
    // The fixed prefix was byte-identical in both requests.
    let systems: Vec<String> = shared
        .seen()
        .iter()
        .filter(|seen| seen.path == "/v1/chat/completions")
        .map(|seen| {
            serde_json::from_slice::<serde_json::Value>(&seen.body).unwrap()["messages"][0]["content"]
                .to_string()
        })
        .collect();
    assert_eq!(systems.len(), 2);
    assert_eq!(systems[0], systems[1]);
}

fn ready_with(interpreter: Arc<dyn Interpreter>) -> Arc<Assistant> {
    let assistant = Assistant::new();
    assistant.event(RuntimeEvent::GenerationStarted { generation: 1 });
    assistant.event(RuntimeEvent::Ready {
        generation: 1,
        budgets: budgets(),
        interpreter,
        path: Path::Cpu,
        reminders: Reminders::Unavailable("test".into()),
    });
    tokio::spawn(Arc::clone(&assistant).work());
    assistant
}

#[tokio::test]
async fn the_content_hash_detects_a_stored_proposal_that_no_longer_matches() {
    let stub = Arc::new(StubInterpreter::answering([Ok(HEAL.to_owned())]));
    let assistant = ready(stub, budgets());
    let (job, _) = start(&assistant, 1, origin(1, "field:hp", 5)).unwrap();
    until_done(&assistant, 1, 1, job).await;
    let (bound, proposal) = assistant.proposal(1, 1, job, 0).unwrap();
    assert!(verify(job, 0, &bound, &proposal));
    // Another option index, job, origin, value, value before, explanation or reference.
    assert!(!verify(job, 1, &bound, &proposal));
    assert!(!verify(JobId([0; 16]), 0, &bound, &proposal));
    let moved = Origin {
        revision: 6,
        ..bound.clone()
    };
    assert!(!verify(job, 0, &moved, &proposal));
    let tampered = |edit: &dyn Fn(&mut Proposal)| {
        let mut other = proposal.clone();
        edit(&mut other);
        !verify(job, 0, &bound, &other)
    };
    let expr = |other: &mut Proposal| match &mut other.changes[0].edit {
        Edit::Expr { candidate, .. } => candidate.clone(),
        _ => panic!("an expression"),
    };
    assert!(tampered(&|other| {
        let mut candidate = expr(other);
        candidate.value = 70;
        other.changes[0].edit = Edit::Expr {
            candidate,
            shown: "x".into(),
        };
    }));
    assert!(tampered(
        &|other| other.changes[0].before = NoteValue::Int(85)
    ));
    assert!(tampered(
        &|other| other.changes[0].address = "field:max_hp".into()
    ));
    assert!(tampered(&|other| other.explanation = "heal 16".into()));
    assert!(tampered(&|other| {
        let mut candidate = expr(other);
        candidate.refs.insert(
            "f1".into(),
            super::super::output::Supplied {
                address: "field:max_hp".into(),
                integer: true,
                value: Some(84),
            },
        );
        other.changes[0].edit = Edit::Expr {
            candidate,
            shown: "x".into(),
        };
    }));
    // Another kind of job over the same origin.
    let reminder = Origin {
        kind: Kind::Reminder,
        ..bound.clone()
    };
    assert!(!verify(job, 0, &reminder, &proposal));
    // A change added or removed.
    assert!(tampered(&|other| {
        let extra = other.changes[0].clone();
        other.changes.push(Change {
            address: "field:armour".into(),
            ..extra
        });
    }));
    assert!(tampered(&|other| other.changes.clear()));
}

fn lit(value: i64) -> Candidate {
    Candidate {
        expr: super::super::output::Node::Lit { lit: value },
        explanation: String::new(),
        value,
        refs: BTreeMap::new(),
    }
}

fn int_change(address: &str, before: NoteValue, value: i64) -> Change {
    Change {
        address: address.into(),
        before,
        edit: Edit::Expr {
            candidate: lit(value),
            shown: value.to_string(),
        },
    }
}

fn reminder(target: &str) -> Origin {
    Origin {
        kind: Kind::Reminder,
        ..origin(1, target, 5)
    }
}

/// The hash tells apart an enum change and a boolean change that differ only in their value
/// after, and a reference's alias; a value that cannot be encoded drops the option.
#[tokio::test]
async fn the_hash_covers_every_after_and_an_unencodable_value_drops_the_option() {
    let assistant = Assistant::new();
    let origin = reminder("field:hp");
    let id = JobId([7; 16]);
    let enum_change = |after: &str| Change {
        address: "field:stance".into(),
        before: NoteValue::Enum("guard".into()),
        edit: Edit::Enum(after.into()),
    };
    let one = bind(id, 0, &origin, vec![enum_change("a")], "why".into()).unwrap();
    let other = bind(id, 0, &origin, vec![enum_change("b")], "why".into()).unwrap();
    assert_ne!(one.hash, other.hash, "enum after");
    let flag = |after: bool| Change {
        address: "field:inspired".into(),
        before: NoteValue::Bool(!after),
        edit: Edit::Bool(after),
    };
    let unknown_flag = |after: bool| Change {
        address: "field:inspired".into(),
        before: NoteValue::Unknown,
        edit: Edit::Bool(after),
    };
    let yes = bind(id, 0, &origin, vec![unknown_flag(true)], "why".into()).unwrap();
    let no = bind(id, 0, &origin, vec![unknown_flag(false)], "why".into()).unwrap();
    assert_ne!(yes.hash, no.hash, "bool after alone");
    let on = bind(id, 0, &origin, vec![flag(true)], "why".into()).unwrap();
    let mut off = on.clone();
    off.changes[0].edit = Edit::Bool(false);
    assert!(!verify(id, 0, &origin, &off), "bool after");
    // A reference's alias, with the same address and value.
    let with_ref = |alias: &str| {
        let mut change = int_change("field:hp", NoteValue::Int(84), 70);
        let Edit::Expr { candidate, .. } = &mut change.edit else {
            unreachable!()
        };
        candidate.refs.insert(
            alias.into(),
            super::super::output::Supplied {
                address: "field:max_hp".into(),
                integer: true,
                value: Some(84),
            },
        );
        bind(id, 0, &origin, vec![change], "why".into())
            .unwrap()
            .hash
    };
    assert_ne!(with_ref("f1"), with_ref("f2"), "alias");
    // An enum ID too long to encode as a note value has no hash: no option.
    let long = "x".repeat(100);
    assert!(bind(id, 0, &origin, vec![enum_change(&long)], "why".into()).is_none());
    let _ = &assistant;
}

/// The read set is in the content hash (tag v3): a changed value, address, a row's name or
/// the set's size fails `verify`, and interpretations carry an empty set.
#[tokio::test]
async fn the_read_set_is_in_the_content_hash() {
    use super::super::proposal::{Read, bind_with_reads};
    let origin = reminder("field:hp");
    let id = JobId([7; 16]);
    let change = || int_change("field:hp", NoteValue::Int(84), 70);
    let read = |address: &str, value: &str| Read {
        address: address.into(),
        value: value.into(),
    };
    let base = vec![read("field:max_hp", "i:84"), read("row:abc", "Rope")];
    let proposal =
        bind_with_reads(id, 0, &origin, vec![change()], base.clone(), "why".into()).unwrap();
    assert!(verify(id, 0, &origin, &proposal));
    let tampered = |reads: Vec<Read>| {
        let mut other = proposal.clone();
        other.reads = reads;
        verify(id, 0, &origin, &other)
    };
    assert!(!tampered(vec![
        read("field:max_hp", "i:85"),
        read("row:abc", "Rope")
    ]));
    assert!(!tampered(vec![
        read("field:max_hp", "i:84"),
        read("row:abc", "Lamp")
    ]));
    assert!(!tampered(vec![
        read("field:max_hp", "i:84"),
        read("row:abd", "Rope")
    ]));
    assert!(!tampered(vec![read("field:max_hp", "i:84")]));
    assert!(!tampered(Vec::new()));
    let none = bind(id, 0, &origin, vec![change()], "why".into()).unwrap();
    assert_eq!(none.reads, Vec::new());
    assert_ne!(none.hash, proposal.hash);
}

/// How many options of `options` a test-built job keeps.
fn kept(assistant: &Assistant, options: Vec<(Vec<Change>, String)>) -> usize {
    let job = assistant.offer(1, reminder("field:hp"), options);
    (0..4)
        .take_while(|n| assistant.proposal(1, 1, job, *n).is_some())
        .count()
}

/// Plan 3b, "Proposal model": one to four changes of distinct integer, enum or boolean
/// targets; a duplicate target or a fifth change drops the option, and so does a change equal
/// to the current value, one that does not fit its value before, or a row add or remove.
#[tokio::test]
async fn a_proposal_has_one_to_four_changes_of_distinct_targets() {
    let assistant = Assistant::new();
    let four = vec![
        int_change("field:hp", NoteValue::Int(84), 70),
        int_change("field:armour", NoteValue::Int(-1), 2),
        Change {
            address: "field:stance".into(),
            before: NoteValue::Enum("guard".into()),
            edit: Edit::Enum("brace".into()),
        },
        Change {
            address: "cell:equipment/r1-1/broken".into(),
            before: NoteValue::Absent,
            edit: Edit::Bool(true),
        },
    ];
    let more = |changes: &[Change]| (changes.to_vec(), "why".to_owned());
    assert_eq!(kept(&assistant, vec![more(&four)]), 1, "four changes");
    assert_eq!(kept(&assistant, vec![more(&four[..1])]), 1, "one change");
    // A fifth change.
    let mut five = four.clone();
    five.push(int_change("field:luck", NoteValue::Unknown, 3));
    assert_eq!(kept(&assistant, vec![more(&five)]), 0, "five changes");
    // A duplicate target.
    let mut twice = four[..2].to_vec();
    twice.push(int_change("field:hp", NoteValue::Int(84), 60));
    assert_eq!(
        kept(&assistant, vec![more(&twice)]),
        0,
        "a duplicate target"
    );
    // No change, or a change equal to the current value.
    assert_eq!(kept(&assistant, vec![more(&[])]), 0);
    let same = vec![int_change("field:hp", NoteValue::Int(84), 84)];
    assert_eq!(kept(&assistant, vec![more(&same)]), 0, "equal to the value");
    // An edit that does not fit the value before, and a row address.
    let misfit = vec![Change {
        address: "field:hp".into(),
        before: NoteValue::Int(84),
        edit: Edit::Bool(true),
    }];
    assert_eq!(
        kept(&assistant, vec![more(&misfit)]),
        0,
        "a boolean over a number"
    );
    let row = vec![int_change("row:equipment/r1-1", NoteValue::Unknown, 1)];
    assert_eq!(
        kept(&assistant, vec![more(&row)]),
        0,
        "a row is not a target"
    );
    // A dropped option leaves the others, indexed among those kept.
    assert_eq!(kept(&assistant, vec![more(&five), more(&four)]), 1);
    let job = assistant.offer(1, reminder("field:hp"), vec![more(&five), more(&four)]);
    let (bound, option) = assistant.proposal(1, 1, job, 0).unwrap();
    assert!(verify(job, 0, &bound, &option));
    assert_eq!(option.changes.len(), 4);
}
#[tokio::test]
async fn an_unavailable_runtime_is_reported_before_anything_else() {
    let stub = Arc::new(StubInterpreter::default());
    let assistant = ready(stub, budgets());
    assistant.event(RuntimeEvent::GenerationEnded {
        generation: 1,
        reason: EndReason::Unavailable("too slow on this machine".into()),
    });
    assert_eq!(
        start(&assistant, 1, origin(1, "field:hp", 5)),
        Err(Refusal::Runtime(RuntimeState::Unavailable(
            "too slow on this machine".into()
        )))
    );
    assert_eq!(assistant.budgets(), None);
}

/// A ready assistant without a worker (jobs stay queued) whose breaker three counted
/// failures opened and whose open time has passed: the next start is the half-open probe.
async fn half_open_without_worker() -> Arc<Assistant> {
    let assistant = Assistant::new();
    assistant.event(RuntimeEvent::GenerationStarted { generation: 1 });
    assistant.event(RuntimeEvent::Ready {
        generation: 1,
        budgets: budgets(),
        interpreter: Arc::new(StubInterpreter::default()),
        path: Path::Cpu,
        reminders: Reminders::Unavailable("test".into()),
    });
    let now = Instant::now().into_std();
    for ticket in 1000..1003 {
        assistant.breaker().admit(ticket, now, None);
        assistant
            .breaker()
            .record(ticket, breaker::Outcome::Failure { generation: 1 }, now);
    }
    tokio::time::advance(breaker::OPEN_FOR + Duration::from_secs(1)).await;
    assistant
}

#[tokio::test(start_paused = true)]
async fn a_queued_probe_ended_by_a_note_write_leaves_the_breaker_half_open() {
    let assistant = half_open_without_worker().await;
    let (probe, _) = start(&assistant, 1, origin(1, "field:hp", 5)).unwrap();
    assert_eq!(
        start(&assistant, 2, origin(2, "field:hp", 5)),
        Err(Refusal::Unavailable)
    );
    assistant.note_written(1, 6, &["field:hp".to_owned()]);
    assert_eq!(assistant.poll(1, 1, probe), Poll::Done(Answer::Stale));
    // The next job becomes the probe instead of the breaker refusing until a restart.
    assert!(start(&assistant, 2, origin(2, "field:hp", 5)).is_ok());
}

#[tokio::test(start_paused = true)]
async fn a_queued_probe_ended_by_a_generation_change_leaves_the_breaker_half_open() {
    let assistant = half_open_without_worker().await;
    start(&assistant, 1, origin(1, "field:hp", 5)).unwrap();
    assistant.event(RuntimeEvent::GenerationEnded {
        generation: 1,
        reason: EndReason::Restart,
    });
    assistant.event(RuntimeEvent::GenerationStarted { generation: 2 });
    assistant.event(RuntimeEvent::Ready {
        generation: 2,
        budgets: budgets(),
        interpreter: Arc::new(StubInterpreter::default()),
        path: Path::Cpu,
        reminders: Reminders::Unavailable("test".into()),
    });
    assert!(start(&assistant, 2, origin(2, "field:hp", 5)).is_ok());
}

#[tokio::test(start_paused = true)]
async fn a_superseded_queued_probe_hands_the_probe_to_its_successor() {
    let assistant = half_open_without_worker().await;
    start(&assistant, 1, origin(1, "field:hp", 5)).unwrap();
    let (successor, _) = start(&assistant, 1, origin(1, "field:hp", 6)).unwrap();
    // The successor is the probe: another account is still refused.
    assert_eq!(
        start(&assistant, 2, origin(2, "field:hp", 5)),
        Err(Refusal::Unavailable)
    );
    assistant.note_written(1, 7, &["field:hp".to_owned()]);
    assert_eq!(assistant.poll(1, 1, successor), Poll::Done(Answer::Stale));
    assert!(start(&assistant, 2, origin(2, "field:hp", 5)).is_ok());
}

#[tokio::test(start_paused = true)]
async fn a_queued_probe_past_its_queue_wait_leaves_the_breaker_half_open() {
    let assistant = half_open_without_worker().await;
    let (probe, _) = start(&assistant, 1, origin(1, "field:hp", 5)).unwrap();
    tokio::time::advance(Duration::from_secs(36)).await;
    tokio::spawn(Arc::clone(&assistant).work());
    settle().await;
    assert_eq!(assistant.poll(1, 1, probe), Poll::Done(Answer::AskAgain));
    assert!(start(&assistant, 2, origin(2, "field:hp", 5)).is_ok());
}

#[tokio::test]
async fn a_row_removal_between_the_database_check_and_the_enqueue_refuses_a_cell_start() {
    let stub = Arc::new(StubInterpreter::default());
    let assistant = ready(stub, budgets());
    let registration = assistant.register(1, 5);
    assistant.note_written(1, 6, &["row:gear/r1-1".to_owned()]);
    assert_eq!(
        assistant.start(
            &registration,
            1,
            origin(1, "cell:gear/r1-1/weight", 5),
            draft("heal 15")
        ),
        Err(Refusal::Conflict)
    );
    // Another row's removal does not.
    assistant.note_written(1, 7, &["row:gear/r1-10".to_owned()]);
    assert!(
        assistant
            .start(
                &registration,
                1,
                origin(1, "cell:gear/r1-2/weight", 5),
                draft("heal 15")
            )
            .is_ok()
    );
}

#[tokio::test(start_paused = true)]
async fn a_deadline_expiry_keeps_the_server_busy_for_its_prompt_term() {
    // The 2c-1 cascade: a request cancelled at its deadline may still be in prompt evaluation; the
    // next job waits that long, and admission counts it.
    let (stub, gate) = gated(3);
    let mut busy = budgets();
    busy.queue_wait = Duration::from_secs(120);
    busy.busy_after_expiry = Duration::from_secs(30);
    let assistant = ready(stub, busy);
    let (first, _) = start(&assistant, 1, origin(1, "field:hp", 5)).unwrap();
    settle().await;
    let (second, _) = start(&assistant, 2, origin(2, "field:hp", 5)).unwrap();
    tokio::time::advance(Duration::from_secs(86)).await;
    settle().await;
    assert_eq!(assistant.poll(1, 1, first), Poll::Done(Answer::AskAgain));
    // The server is presumed busy for 30 s after the expiry.
    tokio::time::advance(Duration::from_secs(10)).await;
    settle().await;
    assert_eq!(assistant.poll(2, 2, second), Poll::Queued);
    // A start now must wait the busy rest (about 19 s) plus the second job's R (85 s) plus
    // its own: within Q = 120 s only without the busy term, so it is refused.
    let refused = start(&assistant, 3, origin(3, "field:hp", 5));
    assert_eq!(refused, Err(Refusal::Busy));
    tokio::time::advance(Duration::from_secs(21)).await;
    settle().await;
    assert_eq!(assistant.poll(2, 2, second), Poll::Running);
    gate.add_permits(3);
}

#[tokio::test(start_paused = true)]
async fn a_job_queued_behind_one_that_expires_still_starts_within_the_derived_queue_wait() {
    // The CPU figures `budgets::derive` gives for R 50 s and a prompt term of 30 s: the total
    // wait is max(120, 2 × 50) = 120, Q = (120 − 50) + 30 = 100 s; without the busy slack Q
    // would be 70 s. A job queued at once behind a running one waits R for it to expire and
    // the busy time after: 80 s.
    for (queue, starts) in [(100, true), (70, false)] {
        let (stub, gate) = gated(3);
        let mut derived = budgets();
        derived.run = Duration::from_secs(50);
        derived.queue_wait = Duration::from_secs(queue);
        derived.busy_after_expiry = Duration::from_secs(30);
        let assistant = ready(stub, derived);
        let (first, _) = start(&assistant, 1, origin(1, "field:hp", 5)).unwrap();
        settle().await;
        let (second, _) = start(&assistant, 2, origin(2, "field:hp", 5)).unwrap();
        tokio::time::advance(Duration::from_secs(51)).await;
        settle().await;
        assert_eq!(assistant.poll(1, 1, first), Poll::Done(Answer::AskAgain));
        assert_eq!(assistant.poll(2, 2, second), Poll::Queued);
        // 50 s + 30 s after it was queued the second job runs, or, with the old Q, it has
        // already been ended as too old.
        tokio::time::advance(Duration::from_secs(30)).await;
        settle().await;
        let expected = if starts {
            Poll::Running
        } else {
            Poll::Done(Answer::AskAgain)
        };
        assert_eq!(assistant.poll(2, 2, second), expected, "Q {queue}");
        gate.add_permits(3);
    }
}

#[tokio::test(start_paused = true)]
async fn on_the_gpu_two_queued_jobs_behind_expiring_ones_still_start_within_the_derived_queue_wait()
{
    // GPU figures from `budgets::derive`: D 2, R 30 s, a prompt term of 20 s, Q = D × (R +
    // busy) = 100 s. Two requests ahead are dropped at their deadlines, each leaving the server
    // busy for 20 s, so the third job starts at 30 + 20 + 30 + 20 = 100 s.
    let (stub, gate) = gated(4);
    let mut derived = budgets();
    derived.run = Duration::from_secs(30);
    derived.depth = 2;
    derived.queue_wait = Duration::from_secs(100);
    derived.busy_after_expiry = Duration::from_secs(20);
    let assistant = ready(stub, derived);
    let (first, _) = start(&assistant, 1, origin(1, "field:hp", 5)).unwrap();
    settle().await;
    let (second, _) = start(&assistant, 2, origin(2, "field:hp", 5)).unwrap();
    tokio::time::advance(Duration::from_secs(5)).await;
    let (third, _) = start(&assistant, 3, origin(3, "field:hp", 5)).unwrap();
    tokio::time::advance(Duration::from_secs(26)).await;
    settle().await;
    assert_eq!(assistant.poll(1, 1, first), Poll::Done(Answer::AskAgain));
    // Busy until 50 s; then the second job runs, and is dropped at 80 s.
    tokio::time::advance(Duration::from_secs(20)).await;
    settle().await;
    assert_eq!(assistant.poll(2, 2, second), Poll::Running);
    tokio::time::advance(Duration::from_secs(30)).await;
    settle().await;
    assert_eq!(assistant.poll(2, 2, second), Poll::Done(Answer::AskAgain));
    assert_eq!(assistant.poll(3, 3, third), Poll::Queued);
    // The third was queued at 5 s and starts at 100 s, inside its Q (105 s).
    tokio::time::advance(Duration::from_secs(20)).await;
    settle().await;
    assert_eq!(assistant.poll(3, 3, third), Poll::Running);
    gate.add_permits(4);
}

#[tokio::test(start_paused = true)]
async fn expiries_and_completed_exchanges_are_signalled_to_the_runtime() {
    let stub = Arc::new(StubInterpreter::answering([Ok(HEAL.to_owned())]));
    let gate = Arc::new(Semaphore::new(0));
    *stub.gate.lock().unwrap() = Some(Arc::clone(&gate));
    let assistant = ready(stub, budgets());
    let (sender, mut signals) = tokio::sync::mpsc::unbounded_channel();
    assistant.subscribe(sender);
    let (_first, _) = start(&assistant, 1, origin(1, "field:hp", 5)).unwrap();
    settle().await;
    tokio::time::advance(Duration::from_secs(86)).await;
    settle().await;
    assert_eq!(signals.try_recv(), Ok(Signal::Expired { generation: 1 }));
    gate.add_permits(1);
    let (_second, _) = start(&assistant, 1, origin(1, "field:hp", 6)).unwrap();
    for _ in 0..20 {
        settle().await;
        tokio::time::advance(Duration::from_millis(10)).await;
    }
    assert_eq!(signals.try_recv(), Ok(Signal::Completed { generation: 1 }));
}

#[tokio::test(start_paused = true)]
async fn a_superseded_running_job_also_keeps_the_server_busy() {
    let (stub, gate) = gated(3);
    let mut busy = budgets();
    busy.queue_wait = Duration::from_secs(120);
    busy.busy_after_expiry = Duration::from_secs(30);
    let assistant = ready(stub, busy);
    let (first, _) = start(&assistant, 1, origin(1, "field:hp", 5)).unwrap();
    settle().await;
    assert_eq!(assistant.poll(1, 1, first), Poll::Running);
    // A newer note of the same account supersedes the running job: its request is dropped.
    let (second, _) = start(&assistant, 1, origin(1, "field:hp", 6)).unwrap();
    settle().await;
    assert_eq!(assistant.poll(1, 1, first), Poll::Done(Answer::Superseded));
    tokio::time::advance(Duration::from_secs(10)).await;
    settle().await;
    assert_eq!(assistant.poll(1, 1, second), Poll::Queued);
    tokio::time::advance(Duration::from_secs(21)).await;
    settle().await;
    assert_eq!(assistant.poll(1, 1, second), Poll::Running);
    gate.add_permits(3);
}

#[tokio::test(start_paused = true)]
async fn a_job_superseded_as_its_deadline_fires_still_keeps_the_server_busy() {
    let (stub, gate) = gated(3);
    let mut busy = budgets();
    busy.queue_wait = Duration::from_secs(120);
    busy.busy_after_expiry = Duration::from_secs(30);
    let assistant = ready(stub, busy);
    let (first, _) = start(&assistant, 1, origin(1, "field:hp", 5)).unwrap();
    settle().await;
    assert_eq!(assistant.poll(1, 1, first), Poll::Running);
    tokio::time::advance(budgets().run.saturating_sub(Duration::from_millis(1))).await;
    settle().await;
    // The supersession and the run deadline reach the worker together.
    let (second, _) = start(&assistant, 1, origin(1, "field:hp", 6)).unwrap();
    tokio::time::advance(Duration::from_millis(1)).await;
    settle().await;
    tokio::time::advance(Duration::from_secs(10)).await;
    settle().await;
    assert_eq!(assistant.poll(1, 1, second), Poll::Queued);
    gate.add_permits(3);
}

/// The reminder setting (ASSIST-60): unset is on for the GPU path and off for the CPU path, a
/// stored choice wins, and a generation whose measurements do not allow reminders, or a
/// runtime that is not ready, has none whatever is stored.
#[tokio::test]
async fn the_effective_reminder_setting_follows_the_stored_choice_the_path_and_the_budgets() {
    use super::super::runtime::reminder_budgets::{ReminderBudgets, Restore};
    let allowed = Reminders::Available(ReminderBudgets {
        run: Duration::from_secs(60),
        estimate_s: 40.0,
        busy: Duration::from_secs(20),
        restore: Restore {
            seconds: 0.1,
            dev_host_constant: true,
        },
        reply_tokens: 500,
        prefix_tokens: 1500,
        extra_wait: Duration::from_secs(21),
    });
    let ready = |assistant: &Arc<Assistant>, generation, path, reminders| {
        assistant.event(RuntimeEvent::GenerationStarted { generation });
        assistant.event(RuntimeEvent::Ready {
            generation,
            budgets: budgets(),
            interpreter: Arc::new(StubInterpreter::default()),
            path,
            reminders,
        });
    };
    let assistant = Assistant::new();
    // Not ready: none, even when stored on.
    assistant.set_reminder_choice(Some(true));
    assert!(!assistant.reminders_effective());
    assistant.set_reminder_choice(None);
    ready(&assistant, 1, Path::Cpu, allowed.clone());
    assert!(
        !assistant.reminders_effective(),
        "unset on the CPU path is off"
    );
    assistant.set_reminder_choice(Some(true));
    assert!(assistant.reminders_effective());
    assistant.set_reminder_choice(Some(false));
    assert!(!assistant.reminders_effective());
    // The GPU path: unset is on, an explicit off wins.
    ready(&assistant, 2, Path::Gpu, allowed.clone());
    assistant.set_reminder_choice(None);
    assert!(
        assistant.reminders_effective(),
        "unset on the GPU path is on"
    );
    assistant.set_reminder_choice(Some(false));
    assert!(!assistant.reminders_effective());
    // Unavailable for the generation: none whatever is stored, and the budgets are absent.
    ready(
        &assistant,
        3,
        Path::Gpu,
        Reminders::Unavailable("over the ceiling".into()),
    );
    assistant.set_reminder_choice(Some(true));
    assert!(!assistant.reminders_effective());
    assert!(assistant.reminder_budgets().is_none());
    // A generation that ends takes them away.
    ready(&assistant, 4, Path::Gpu, allowed);
    assert!(assistant.reminders_effective());
    assistant.event(RuntimeEvent::GenerationEnded {
        generation: 4,
        reason: EndReason::Exited,
    });
    assert!(!assistant.reminders_effective());
    assert!(assistant.reminder_generation().is_none());
}

// --- Reminder records (plan 3c-1, "Scheduling") ---------------------------------------------

fn reminder_origin(sheet: i64, target: &str, revision: i64) -> Origin {
    Origin {
        kind: Kind::Reminder,
        ..origin(sheet, target, revision)
    }
}

fn reminder_draft() -> ReminderDraft {
    use super::super::contract::{ItemKind, ReminderDecrease, ReminderField, ReminderRow};
    use super::super::reminder::{FieldDraft, RowDraft};
    ReminderDraft {
        request: ReminderRequest {
            decrease: ReminderDecrease {
                id: "hp".into(),
                label: "Current HP".into(),
                before: 99,
                after: 84,
                phrase: "-15".into(),
            },
            address: "field:hp".into(),
            fields: vec![FieldDraft {
                item: ReminderField {
                    id: "max_hp".into(),
                    label: "Max HP".into(),
                    kind: ItemKind::Integer,
                    value: Some(serde_json::Value::from(120)),
                },
                address: "field:max_hp".into(),
                options: vec![],
            }],
            rows: vec![RowDraft {
                row: ReminderRow {
                    id: "r1".into(),
                    list: "Equipment".into(),
                    name: "Antidote".into(),
                },
                address: "row:equipment/r1-1".into(),
            }],
        },
    }
}

fn start_reminder(
    assistant: &Arc<Assistant>,
    account: i64,
    origin: Origin,
) -> Result<JobId, ReminderRefusal> {
    let registration = assistant.register(origin.sheet, origin.revision);
    assistant.start_reminder(&registration, account, origin, reminder_draft())
}

fn reminders_ready(on: bool) -> Arc<Assistant> {
    use super::super::runtime::reminder_budgets::{ReminderBudgets, Restore};
    let assistant = Assistant::new();
    assistant.event(RuntimeEvent::GenerationStarted { generation: 1 });
    assistant.event(RuntimeEvent::Ready {
        generation: 1,
        budgets: budgets(),
        interpreter: Arc::new(StubInterpreter::default()),
        path: Path::Gpu,
        reminders: Reminders::Available(ReminderBudgets {
            run: Duration::from_secs(60),
            estimate_s: 40.0,
            busy: Duration::from_secs(20),
            restore: Restore {
                seconds: 0.1,
                dev_host_constant: false,
            },
            reply_tokens: 500,
            prefix_tokens: 1500,
            extra_wait: Duration::from_secs(21),
        }),
    });
    assistant.set_reminder_choice(Some(on));
    assistant
}

/// Reminders have their own records: they take no interpretation account slot, are not
/// repeat-detected against interpretations, and a newer reminder of the account supersedes
/// the older one.
#[tokio::test]
async fn a_reminder_takes_no_interpretation_slot_and_a_newer_one_supersedes_the_older() {
    let assistant = reminders_ready(true);
    let (_stub, _gate) = gated(0);
    let first = start_reminder(&assistant, 1, reminder_origin(1, "field:hp", 5)).unwrap();
    // The same account may still start an interpretation, on another sheet too.
    assert!(start(&assistant, 1, origin(2, "field:hp", 9)).is_ok());
    assert_eq!(
        assistant.poll_reminder_for_tests(1, 1, first),
        Some(Poll::Queued)
    );
    let second = start_reminder(&assistant, 1, reminder_origin(1, "field:gold", 6)).unwrap();
    assert_ne!(first, second);
    assert_eq!(
        assistant.poll(1, 1, first),
        Poll::Done(Answer::Superseded),
        "poll looks in both tables"
    );
    assert_eq!(assistant.poll(1, 1, second), Poll::Queued);
    // Another account's poll never sees it.
    assert_eq!(assistant.poll(2, 1, second), Poll::Missing);
}

/// 8 live reminders at most, one per account; 429 beyond; 32 records, a start evicts the
/// oldest finished one.
#[tokio::test]
async fn reminders_are_limited_to_eight_live_and_thirty_two_records() {
    let assistant = reminders_ready(true);
    for account in 1..=8 {
        start_reminder(&assistant, account, reminder_origin(account, "field:hp", 1)).unwrap();
    }
    assert_eq!(
        start_reminder(&assistant, 9, reminder_origin(9, "field:hp", 1)),
        Err(ReminderRefusal::Busy)
    );
    // The same account's superseding start still fits (as if the old one were gone).
    assert!(start_reminder(&assistant, 3, reminder_origin(3, "field:gold", 2)).is_ok());
    // Superseded records are finished: they fill the table to 32 and then the oldest goes.
    for n in 0..40 {
        start_reminder(&assistant, 3, reminder_origin(3, "field:hp", 10 + n)).unwrap();
    }
    assert!(assistant.reminder_records() <= 32);
}

/// Off, unavailable, not ready or the breaker not closed: no reminder starts.
#[tokio::test]
async fn a_reminder_needs_the_setting_on_a_ready_generation_and_a_closed_breaker() {
    let off = reminders_ready(false);
    assert_eq!(
        start_reminder(&off, 1, reminder_origin(1, "field:hp", 1)),
        Err(ReminderRefusal::NotAvailable)
    );
    let cold = Assistant::new();
    cold.set_reminder_choice(Some(true));
    assert_eq!(
        start_reminder(&cold, 1, reminder_origin(1, "field:hp", 1)),
        Err(ReminderRefusal::NotAvailable)
    );
    let on = reminders_ready(true);
    // Three counted failures open the breaker: a reminder is refused, and so is the probe.
    for ticket in 100..103 {
        on.breaker_failure_for_tests(ticket);
    }
    assert!(!on.breaker_closed());
    assert_eq!(
        start_reminder(&on, 1, reminder_origin(1, "field:hp", 1)),
        Err(ReminderRefusal::BreakerNotClosed)
    );
}

/// A note write on the decrease's field, a generation change, or a newer note ends reminders
/// silently; a start under a newer note conflicts.
#[tokio::test]
async fn the_note_hook_and_a_generation_change_end_reminders_and_a_newer_note_conflicts() {
    let assistant = reminders_ready(true);
    let id = start_reminder(&assistant, 1, reminder_origin(1, "field:hp", 5)).unwrap();
    assistant.note_written(1, 6, &["field:hp".to_owned()]);
    assert_eq!(assistant.poll(1, 1, id), Poll::Done(Answer::Stale));
    // A start whose registration was taken before the write finds the newer note.
    let held = assistant.register(1, 5);
    assistant.note_written(1, 7, &["field:hp".to_owned()]);
    assert_eq!(
        assistant.start_reminder(
            &held,
            1,
            reminder_origin(1, "field:hp", 5),
            reminder_draft()
        ),
        Err(ReminderRefusal::Conflict)
    );
    let id = start_reminder(&assistant, 1, reminder_origin(1, "field:hp", 7)).unwrap();
    assistant.event(RuntimeEvent::GenerationEnded {
        generation: 1,
        reason: EndReason::Exited,
    });
    assert_eq!(assistant.poll(1, 1, id), Poll::Done(Answer::Restarted));
}

// --- Reminder worker (plan 3c-1, "Scheduling") ----------------------------------------------

const REMINDER_REPLY: &str = r#"{"relevant":true,"row":"r1","hint":"The antidote may help","option":{"changes":[{"ref":"hp","expr":{"op":"add","l":{"ref":"hp"},"r":{"lit":15}}}],"explanation":"heal 15"}}"#;

/// A ready assistant with reminders on, its worker running, over `stub`: a reminder run
/// deadline of 60 s, `busy` the busy term and `restore` seconds of re-priming.
fn reminders_running(stub: Arc<StubInterpreter>, path: Path, busy_s: u64) -> Arc<Assistant> {
    reminders_queue(stub, path, (busy_s, 35))
}

/// [`reminders_running`] with the interpretation queue wait Q in seconds.
fn reminders_queue(
    stub: Arc<StubInterpreter>,
    path: Path,
    (busy_s, queue_s): (u64, u64),
) -> Arc<Assistant> {
    use super::super::runtime::reminder_budgets::{ReminderBudgets, Restore};
    let assistant = Assistant::new();
    assistant.set_transport_grace(Duration::from_millis(10));
    assistant.event(RuntimeEvent::GenerationStarted { generation: 1 });
    let mut interpretation = budgets();
    interpretation.queue_wait = Duration::from_secs(queue_s);
    assistant.event(RuntimeEvent::Ready {
        generation: 1,
        budgets: interpretation,
        interpreter: stub,
        path,
        reminders: Reminders::Available(ReminderBudgets {
            run: Duration::from_secs(60),
            estimate_s: 40.0,
            busy: Duration::from_secs(busy_s),
            restore: Restore {
                seconds: 5.0,
                dev_host_constant: false,
            },
            reply_tokens: 500,
            prefix_tokens: 1500,
            extra_wait: Duration::from_secs(21),
        }),
    });
    assistant.set_reminder_choice(Some(true));
    tokio::spawn(Arc::clone(&assistant).work());
    assistant
}

fn kinds(stub: &StubInterpreter) -> Vec<&'static str> {
    stub.requests
        .lock()
        .unwrap()
        .iter()
        .map(|request| match (&request.kind, request.max_tokens) {
            (RequestKind::Reminder { .. }, _) => "reminder",
            (RequestKind::Interpretation, 1) => "reprime",
            (RequestKind::Interpretation, _) => "interpretation",
        })
        .collect()
}

#[tokio::test]
async fn a_reminder_runs_and_offers_its_hint_row_and_option_with_the_read_set() {
    let stub = Arc::new(StubInterpreter::answering([Ok(REMINDER_REPLY.to_owned())]));
    let assistant = reminders_running(Arc::clone(&stub), Path::Cpu, 20);
    let id = start_reminder(&assistant, 1, reminder_origin(1, "field:hp", 5)).unwrap();
    let Poll::Done(Answer::Reminder { hint, row, options }) =
        until_done(&assistant, 1, 1, id).await
    else {
        panic!()
    };
    assert_eq!(hint, "The antidote may help");
    assert_eq!(row.as_deref(), Some("Antidote"));
    let [option] = options.as_slice() else {
        panic!()
    };
    assert_eq!(option.changes[0].after(), NoteValue::Int(99));
    let reads: Vec<_> = option
        .reads
        .iter()
        .map(|read| read.address.as_str())
        .collect();
    assert_eq!(reads, ["field:hp", "field:max_hp", "row:equipment/r1-1"]);
    assert_eq!(kinds(&stub), ["reminder"]);
    // The option applies through the same lookup as an interpretation's.
    assert!(assistant.proposal(1, 1, id, 0).is_some());
    // Options live ten minutes.
    assistant.expire_for_tests(id);
    assert_eq!(assistant.poll(1, 1, id), Poll::Done(Answer::AskAgain));
    assert!(assistant.proposal(1, 1, id, 0).is_none());
}

#[tokio::test]
async fn a_reminder_runs_only_when_no_interpretation_is_queued() {
    let (stub, gate) = gated(3);
    stub.answers.lock().unwrap().clear();
    stub.answers.lock().unwrap().extend([
        Ok(HEAL.to_owned()),
        Ok(HEAL.to_owned()),
        Ok(REMINDER_REPLY.to_owned()),
    ]);
    let assistant = reminders_queue(Arc::clone(&stub), Path::Cpu, (20, 200));
    let (first, _) = start(&assistant, 1, origin(1, "field:hp", 5)).unwrap();
    settle().await;
    // The worker is busy with the first interpretation; a reminder queues, then a second
    // interpretation: the interpretation goes first.
    let reminder = start_reminder(&assistant, 3, reminder_origin(3, "field:hp", 5)).unwrap();
    let (second, _) = start(&assistant, 2, origin(2, "field:hp", 5)).unwrap();
    gate.add_permits(3);
    assert!(matches!(
        until_done(&assistant, 3, 3, reminder).await,
        Poll::Done(_)
    ));
    assert!(matches!(
        assistant.poll(2, 2, second),
        Poll::Done(Answer::Options(_))
    ));
    assert!(matches!(assistant.poll(1, 1, first), Poll::Done(_)));
    assert_eq!(
        kinds(&stub),
        ["interpretation", "interpretation", "reminder"]
    );
}

#[tokio::test(start_paused = true)]
async fn an_admitted_interpretation_cancels_a_running_reminder_and_its_busy_term_counts() {
    let (stub, gate) = gated(3);
    let assistant = reminders_running(Arc::clone(&stub), Path::Cpu, 20);
    let reminder = start_reminder(&assistant, 1, reminder_origin(1, "field:hp", 5)).unwrap();
    settle().await;
    assert_eq!(assistant.poll(1, 1, reminder), Poll::Running);
    let (job, _) = start(&assistant, 2, origin(2, "field:hp", 5)).unwrap();
    settle().await;
    assert_eq!(
        assistant.poll(1, 1, reminder),
        Poll::Done(Answer::Superseded)
    );
    // The server may still hold the reminder's prompt: 20 s before the interpretation runs.
    tokio::time::advance(Duration::from_secs(10)).await;
    settle().await;
    assert_eq!(assistant.poll(2, 2, job), Poll::Queued);
    tokio::time::advance(Duration::from_secs(11)).await;
    settle().await;
    assert_eq!(assistant.poll(2, 2, job), Poll::Running);
    gate.add_permits(3);

    // A busy term past Q (35 s) refuses the interpretation: the reminder keeps running.
    let (stub, gate) = gated(3);
    let assistant = reminders_running(stub, Path::Cpu, 40);
    let reminder = start_reminder(&assistant, 1, reminder_origin(1, "field:hp", 5)).unwrap();
    settle().await;
    assert_eq!(
        start(&assistant, 2, origin(2, "field:hp", 5)),
        Err(Refusal::Busy)
    );
    assert_eq!(assistant.poll(1, 1, reminder), Poll::Running);
    gate.add_permits(3);
}

#[tokio::test(start_paused = true)]
async fn a_reminder_expiry_is_neutral_never_hung_and_keeps_the_server_busy() {
    let (stub, gate) = gated(3);
    let assistant = reminders_running(Arc::clone(&stub), Path::Cpu, 20);
    let (sender, mut signals) = tokio::sync::mpsc::unbounded_channel();
    assistant.subscribe(sender);
    for round in 0..3 {
        let id = start_reminder(&assistant, 1, reminder_origin(1, "field:hp", 5 + round)).unwrap();
        settle().await;
        assert_eq!(assistant.poll(1, 1, id), Poll::Running);
        tokio::time::advance(Duration::from_secs(61)).await;
        settle().await;
        assert_eq!(assistant.poll(1, 1, id), Poll::Done(Answer::AskAgain));
        assert!(signals.try_recv().is_err(), "no Expired signal");
        // The next job waits for the reminder prompt's busy term.
        tokio::time::advance(Duration::from_secs(21)).await;
        settle().await;
    }
    assert!(assistant.breaker_closed());
    gate.add_permits(3);
}

#[tokio::test]
async fn reminder_replies_and_failures_reach_the_breaker_as_specified() {
    // Invalid replies are `Nothing` and uncounted, but the exchange completed.
    let stub = Arc::new(StubInterpreter::answering((0..4).map(|n| {
        Ok([
            "{",
            "",
            "not json",
            r#"{"relevant":false,"row":null,"hint":"x","option":null}"#,
        ][n]
            .to_owned())
    })));
    let assistant = reminders_running(Arc::clone(&stub), Path::Cpu, 20);
    let (sender, mut signals) = tokio::sync::mpsc::unbounded_channel();
    assistant.subscribe(sender);
    for round in 0..4 {
        let id = start_reminder(&assistant, 1, reminder_origin(1, "field:hp", 5 + round)).unwrap();
        assert_eq!(
            until_done(&assistant, 1, 1, id).await,
            Poll::Done(Answer::Nothing)
        );
        assert_eq!(signals.try_recv(), Ok(Signal::Completed { generation: 1 }));
    }
    assert!(assistant.breaker_closed());
    // HTTP and transport failures count: three open the breaker.
    let stub = Arc::new(StubInterpreter::answering([
        Err(Failure::Status(500)),
        Err(Failure::Status(503)),
        Err(Failure::Transport),
    ]));
    let assistant = reminders_running(stub, Path::Cpu, 20);
    for round in 0..3 {
        assert!(assistant.breaker_closed(), "{round}");
        let id = start_reminder(&assistant, 1, reminder_origin(1, "field:hp", 5 + round)).unwrap();
        assert_eq!(
            until_done(&assistant, 1, 1, id).await,
            Poll::Done(Answer::Unavailable)
        );
    }
    assert!(!assistant.breaker_closed());
}

/// On the GPU path every started reminder is followed by a re-priming of the interpretation
/// prefix before the next job, and the server is busy until that request completes; the CPU
/// path (host-RAM prompt cache) sends none.
#[tokio::test]
async fn the_gpu_path_reprimes_after_every_reminder_and_the_cpu_path_does_not() {
    let (stub, gate) = gated(0);
    stub.answers.lock().unwrap().extend([
        Ok(REMINDER_REPLY.to_owned()),
        Ok(HEAL.to_owned()),
        Ok(HEAL.to_owned()),
    ]);
    let assistant = reminders_running(Arc::clone(&stub), Path::Gpu, 20);
    let id = start_reminder(&assistant, 1, reminder_origin(1, "field:hp", 5)).unwrap();
    gate.add_permits(1);
    assert!(matches!(
        until_done(&assistant, 1, 1, id).await,
        Poll::Done(_)
    ));
    // The re-priming is in flight (waiting at the gate); an interpretation waits behind it.
    let (job, _) = start(&assistant, 2, origin(2, "field:hp", 5)).unwrap();
    for _ in 0..20 {
        settle().await;
    }
    assert_eq!(kinds(&stub), ["reminder", "reprime"]);
    assert_eq!(assistant.poll(2, 2, job), Poll::Queued);
    gate.add_permits(2);
    assert!(matches!(
        until_done(&assistant, 2, 2, job).await,
        Poll::Done(_)
    ));
    assert_eq!(kinds(&stub), ["reminder", "reprime", "interpretation"]);

    let stub = Arc::new(StubInterpreter::answering([Ok(REMINDER_REPLY.to_owned())]));
    let assistant = reminders_running(Arc::clone(&stub), Path::Cpu, 20);
    let id = start_reminder(&assistant, 1, reminder_origin(1, "field:hp", 5)).unwrap();
    assert!(matches!(
        until_done(&assistant, 1, 1, id).await,
        Poll::Done(_)
    ));
    for _ in 0..20 {
        settle().await;
    }
    assert_eq!(kinds(&stub), ["reminder"]);
}

// --- Review fixes (plan 3c-1, "Budgets" and "Scheduling") -----------------------------------

/// A scribble reading is never refused because of a reminder (plan 3c-1): with D = 1 on the CPU
/// path, a second account's reading is admitted while the first waits behind the cancelled
/// reminder's busy time, as it would behind a running interpretation; a third one is refused.
#[tokio::test(start_paused = true)]
async fn the_first_job_queued_behind_a_reminders_busy_time_does_not_count_toward_d() {
    let (stub, gate) = gated(4);
    let assistant = reminders_queue(Arc::clone(&stub), Path::Cpu, (20, 200));
    start_reminder(&assistant, 1, reminder_origin(1, "field:hp", 5)).unwrap();
    settle().await;
    let (first, _) = start(&assistant, 2, origin(2, "field:hp", 5)).unwrap();
    settle().await;
    assert_eq!(
        assistant.poll(2, 2, first),
        Poll::Queued,
        "behind the busy time"
    );
    assert!(
        start(&assistant, 3, origin(3, "field:hp", 5)).is_ok(),
        "one may wait behind the busy time, as behind a running reading"
    );
    assert_eq!(
        start(&assistant, 4, origin(4, "field:hp", 5)),
        Err(Refusal::Busy),
        "D still bounds the rest"
    );
    gate.add_permits(4);
}

/// The busy term is set when the admission cancels a running reminder, in the same step, so
/// the next admission already counts it.
#[tokio::test(start_paused = true)]
async fn cancelling_a_running_reminder_sets_the_busy_time_at_once() {
    let (stub, gate) = gated(2);
    let assistant = reminders_running(Arc::clone(&stub), Path::Cpu, 20);
    start_reminder(&assistant, 1, reminder_origin(1, "field:hp", 5)).unwrap();
    settle().await;
    assert_eq!(assistant.busy_for_tests(), Duration::ZERO);
    start(&assistant, 2, origin(2, "field:hp", 5)).unwrap();
    // No yield: the worker has not seen the cancel yet.
    assert_eq!(assistant.busy_for_tests(), Duration::from_secs(20));
    gate.add_permits(2);
}

/// On the GPU path the re-prime is installed when the reminder starts, not after it returns.
#[tokio::test(start_paused = true)]
async fn the_reprime_is_installed_when_the_reminder_starts() {
    let (stub, gate) = gated(0);
    stub.answers
        .lock()
        .unwrap()
        .extend([Ok(REMINDER_REPLY.to_owned()), Ok(HEAL.to_owned())]);
    let assistant = reminders_running(Arc::clone(&stub), Path::Gpu, 20);
    start_reminder(&assistant, 1, reminder_origin(1, "field:hp", 5)).unwrap();
    settle().await;
    assert!(
        assistant.reprime_pending_for_tests(),
        "installed while it runs"
    );
    gate.add_permits(2);
    settle().await;
    assert_eq!(assistant.busy_for_tests(), Duration::ZERO);
}

/// A reminder that never reached the server (its tokenizer call failed) leaves the slot's
/// cache alone: no re-prime.
#[tokio::test(start_paused = true)]
async fn a_reminder_that_never_reached_the_server_is_not_followed_by_a_reprime() {
    let stub = Arc::new(StubInterpreter::default());
    *stub.tokenize_fails.lock().unwrap() = Some(Failure::Status(500));
    let assistant = reminders_running(Arc::clone(&stub), Path::Gpu, 20);
    let id = start_reminder(&assistant, 1, reminder_origin(1, "field:hp", 5)).unwrap();
    assert!(matches!(
        until_done(&assistant, 1, 1, id).await,
        Poll::Done(_)
    ));
    for _ in 0..3 {
        settle().await;
    }
    assert!(!assistant.reprime_pending_for_tests());
    assert!(kinds(&stub).is_empty(), "{:?}", kinds(&stub));
}

/// CPU path, runtime detection (plan 3c-0): the first interpretation after a reminder must have
/// read at least 75 % of the prefix from the cache. Too little, no figure at all, an expiry or a
/// transport failure make reminders unavailable for the rest of the generation, with the
/// reason; a good figure changes nothing; the GPU path is not checked.
#[tokio::test(start_paused = true)]
async fn a_cache_that_did_not_restore_the_prefix_makes_reminders_unavailable() {
    #[derive(Clone, Copy, Debug)]
    enum Case {
        Cached(Option<u64>),
        Expires,
        Transport,
    }
    for (case, path, stays) in [
        (Case::Cached(Some(100)), Path::Cpu, false),
        (Case::Cached(None), Path::Cpu, false),
        (Case::Expires, Path::Cpu, false),
        (Case::Transport, Path::Cpu, false),
        (Case::Cached(Some(1200)), Path::Cpu, true),
        (Case::Cached(Some(100)), Path::Gpu, true),
        (Case::Cached(None), Path::Gpu, true),
    ] {
        let stub = Arc::new(StubInterpreter::answering([
            Ok(REMINDER_REPLY.to_owned()),
            match case {
                Case::Transport => Err(Failure::Transport),
                _ => Ok(HEAL.to_owned()),
            },
        ]));
        let assistant = reminders_running(Arc::clone(&stub), path, 20);
        let id = start_reminder(&assistant, 1, reminder_origin(1, "field:hp", 5)).unwrap();
        assert!(matches!(
            until_done(&assistant, 1, 1, id).await,
            Poll::Done(_)
        ));
        match case {
            Case::Cached(cached) => *stub.cached_tokens.lock().unwrap() = cached,
            Case::Expires => stub
                .delays
                .lock()
                .unwrap()
                .push_back(Duration::from_secs(100)),
            Case::Transport => {}
        }
        tokio::time::advance(Duration::from_secs(30)).await;
        settle().await;
        let (job, _) = start(&assistant, 2, origin(2, "field:hp", 5)).unwrap();
        for _ in 0..40 {
            tokio::time::advance(Duration::from_secs(5)).await;
            settle().await;
        }
        assert!(matches!(assistant.poll(2, 2, job), Poll::Done(_)));
        assert_eq!(assistant.reminders_effective(), stays, "{case:?} {path:?}");
        if !stays {
            let Some((_, Reminders::Unavailable(why))) = assistant.reminder_generation() else {
                panic!("the page states the reason")
            };
            assert!(why.contains("cache"), "{why}");
            assert_eq!(
                start_reminder(&assistant, 1, reminder_origin(1, "field:hp", 6)),
                Err(ReminderRefusal::NotAvailable)
            );
            // A new generation measures again.
            assistant.event(RuntimeEvent::GenerationEnded {
                generation: 1,
                reason: EndReason::Exited,
            });
            assert!(assistant.reminder_generation().is_none());
        }
    }
}

/// Reminders queued when the cache check turns them off do not run.
#[tokio::test(start_paused = true)]
async fn a_queued_reminder_does_not_run_after_the_cache_check_turned_reminders_off() {
    let (stub, gate) = gated(0);
    stub.answers.lock().unwrap().extend([
        Ok(REMINDER_REPLY.to_owned()),
        Ok(HEAL.to_owned()),
        Ok(REMINDER_REPLY.to_owned()),
    ]);
    let assistant = reminders_running(Arc::clone(&stub), Path::Cpu, 20);
    start_reminder(&assistant, 1, reminder_origin(1, "field:hp", 5)).unwrap();
    gate.add_permits(1);
    settle().await;
    *stub.cached_tokens.lock().unwrap() = Some(10);
    let (job, _) = start(&assistant, 2, origin(2, "field:hp", 5)).unwrap();
    // Queued while the interpretation is yet to finish.
    let queued = start_reminder(&assistant, 3, reminder_origin(3, "field:hp", 5)).unwrap();
    gate.add_permits(5);
    for _ in 0..10 {
        tokio::time::advance(Duration::from_secs(5)).await;
        settle().await;
    }
    assert!(matches!(assistant.poll(2, 2, job), Poll::Done(_)));
    assert_eq!(
        assistant.poll(3, 3, queued),
        Poll::Done(Answer::Unavailable)
    );
    assert_eq!(kinds(&stub), ["reminder", "interpretation"]);
}

/// An interpretation right after a reminder does not expire when the server first spends the
/// prefix's time, with R and the restore taken from the reminder budgets of a calibrated
/// generation: GPU, the re-prime (prefix-sized) which the worker awaits before the job starts;
/// CPU, the cache restore inside the request, covered by R's margin. A request that is slower by
/// more than the margin does expire.
#[tokio::test(start_paused = true)]
async fn an_interpretation_after_a_reminder_does_not_expire_on_either_path() {
    use super::super::runtime::reminder_budgets::{derive, tests::make};
    for path in [Path::Gpu, Path::Cpu] {
        let inputs = make(path);
        let allowed = derive(&inputs).unwrap();
        let (r, estimate) = (inputs.derived.budgets.run, inputs.derived.estimate_s);
        let restore = Duration::from_secs_f64(allowed.restore.seconds);
        let normal = Duration::from_secs_f64(estimate);
        let margin = r.as_secs_f64() - estimate;
        if path == Path::Cpu {
            assert!(
                allowed.restore.seconds <= margin,
                "derive keeps the CPU restore in R"
            );
        }
        for over in [false, true] {
            let stub = Arc::new(StubInterpreter::answering([
                Ok(REMINDER_REPLY.to_owned()),
                Ok(HEAL.to_owned()),
                Ok(HEAL.to_owned()),
            ]));
            let assistant = Assistant::new();
            assistant.set_transport_grace(Duration::from_millis(10));
            assistant.event(RuntimeEvent::GenerationStarted { generation: 1 });
            assistant.event(RuntimeEvent::Ready {
                generation: 1,
                budgets: inputs.derived.budgets,
                interpreter: Arc::clone(&stub) as _,
                path,
                reminders: Reminders::Available(allowed),
            });
            assistant.set_reminder_choice(Some(true));
            tokio::spawn(Arc::clone(&assistant).work());
            let slower = Duration::from_secs_f64(margin + 5.0);
            // The reminder takes no time; then, in order, the requests after it. GPU: the
            // re-prime takes the restore time, the interpretation its normal time. CPU: the
            // interpretation's request carries the restore.
            let mut delays = vec![Duration::ZERO];
            match (path, over) {
                (Path::Gpu, _) => delays.extend([restore, normal]),
                (Path::Cpu, false) => delays.push(normal + restore),
                (Path::Cpu, true) => delays.push(normal + slower),
            }
            stub.delays.lock().unwrap().extend(delays);
            *stub.cached_tokens.lock().unwrap() = Some(1500);
            let id = start_reminder(&assistant, 1, reminder_origin(1, "field:hp", 5)).unwrap();
            assert!(matches!(
                until_done(&assistant, 1, 1, id).await,
                Poll::Done(_)
            ));
            let (job, _) = start(&assistant, 2, origin(2, "field:hp", 5)).unwrap();
            // Within the options' ten minutes after the job's end.
            for _ in 0..160 {
                tokio::time::advance(Duration::from_secs(5)).await;
                settle().await;
            }
            let expected_expiry = path == Path::Cpu && over;
            let poll = assistant.poll(2, 2, job);
            if expected_expiry {
                assert_eq!(poll, Poll::Done(Answer::AskAgain), "{path:?} over");
            } else {
                assert!(
                    matches!(poll, Poll::Done(Answer::Options(_))),
                    "{path:?}: {poll:?}"
                );
            }
        }
    }
}

/// A scribble reading behind a reminder's busy time waits that time and the reading ahead of it:
/// the time arithmetic counts every job waiting, though the first does not count toward D.
#[tokio::test(start_paused = true)]
async fn admission_counts_the_busy_time_and_the_job_ahead_in_the_wait() {
    // R is 85 s, the reminder busy term 20 s: the second reading waits 105 s.
    for (queue_s, admitted) in [(105, true), (104, false)] {
        let (stub, gate) = gated(4);
        let assistant = reminders_queue(Arc::clone(&stub), Path::Cpu, (20, queue_s));
        start_reminder(&assistant, 1, reminder_origin(1, "field:hp", 5)).unwrap();
        settle().await;
        start(&assistant, 2, origin(2, "field:hp", 5)).unwrap();
        assert_eq!(
            start(&assistant, 3, origin(3, "field:hp", 5)).is_ok(),
            admitted,
            "Q {queue_s}"
        );
        gate.add_permits(4);
    }
}

/// The re-prime is presumed to keep the server busy for the restore time the Ready rules count,
/// no longer; the worker awaits it anyway, so a job queued behind it starts only once it is done.
#[tokio::test(start_paused = true)]
async fn the_reprime_is_presumed_busy_for_the_restore_time_and_awaited() {
    let (stub, gate) = gated(0);
    stub.answers.lock().unwrap().extend([
        Ok(REMINDER_REPLY.to_owned()),
        Ok(HEAL.to_owned()),
        Ok(HEAL.to_owned()),
    ]);
    let assistant = reminders_running(Arc::clone(&stub), Path::Gpu, 20);
    start_reminder(&assistant, 1, reminder_origin(1, "field:hp", 5)).unwrap();
    gate.add_permits(1);
    settle().await;
    assert_eq!(kinds(&stub), ["reminder", "reprime"]);
    // The restore is 5 s (`reminders_queue`): not the 2 x 5 + 10 s bound of the wait.
    assert_eq!(assistant.busy_for_tests(), Duration::from_secs(5));
    let (job, _) = start(&assistant, 2, origin(2, "field:hp", 5)).unwrap();
    tokio::time::advance(Duration::from_secs(8)).await;
    settle().await;
    assert_eq!(
        assistant.poll(2, 2, job),
        Poll::Queued,
        "awaits the re-prime"
    );
    gate.add_permits(2);
    settle().await;
    assert!(matches!(
        until_done(&assistant, 2, 2, job).await,
        Poll::Done(_)
    ));
}

/// Third prompt kinds beside interpretations: a cancelled reminder then an interpretation, a
/// reminder of two accounts, and a reminder between interpretations leave the schedule and the
/// busy term as specified (no live server: the stub's request order).
#[tokio::test(start_paused = true)]
async fn mixed_prompt_kinds_keep_their_order_and_busy_terms() {
    // A cancelled reminder, then an interpretation: the interpretation waits the busy term.
    let (stub, gate) = gated(3);
    let assistant = reminders_running(Arc::clone(&stub), Path::Cpu, 20);
    start_reminder(&assistant, 1, reminder_origin(1, "field:hp", 5)).unwrap();
    settle().await;
    let (job, _) = start(&assistant, 2, origin(2, "field:hp", 5)).unwrap();
    settle().await;
    tokio::time::advance(Duration::from_secs(19)).await;
    settle().await;
    assert_eq!(kinds(&stub), ["reminder"]);
    tokio::time::advance(Duration::from_secs(2)).await;
    gate.add_permits(3);
    settle().await;
    assert_eq!(kinds(&stub), ["reminder", "interpretation"]);
    assert!(matches!(
        assistant.poll(2, 2, job),
        Poll::Done(_) | Poll::Running
    ));

    // Reminders of two accounts run one after the other, each followed on the GPU path by one
    // re-prime, and an interpretation queued meanwhile goes first.
    let (stub, gate) = gated(0);
    stub.answers.lock().unwrap().extend([
        Ok(REMINDER_REPLY.to_owned()),
        Ok(HEAL.to_owned()),
        Ok(REMINDER_REPLY.to_owned()),
        Ok(HEAL.to_owned()),
    ]);
    let assistant = reminders_running(Arc::clone(&stub), Path::Gpu, 20);
    start_reminder(&assistant, 1, reminder_origin(1, "field:hp", 5)).unwrap();
    start_reminder(&assistant, 2, reminder_origin(2, "field:hp", 5)).unwrap();
    gate.add_permits(10);
    for _ in 0..4 {
        tokio::time::advance(Duration::from_secs(30)).await;
        settle().await;
    }
    assert_eq!(kinds(&stub), ["reminder", "reprime", "reminder", "reprime"]);
}
