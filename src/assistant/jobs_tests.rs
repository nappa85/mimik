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
        field_refs: true,
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
    assert_eq!(options[0].candidate.value, 99);
    assert_eq!(options[0].before, Some(84));
    assert_eq!(options[0].shown, "84 + 15");
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
    let mut other = proposal.clone();
    other.candidate.value = 70;
    assert!(!verify(job, 0, &bound, &other));
    let mut other = proposal.clone();
    other.before = Some(85);
    assert!(!verify(job, 0, &bound, &other));
    let mut other = proposal.clone();
    other.candidate.explanation = "heal 16".into();
    assert!(!verify(job, 0, &bound, &other));
    let mut other = proposal;
    other.candidate.refs.insert(
        "f1".into(),
        super::super::output::Supplied {
            address: "field:max_hp".into(),
            integer: true,
            value: Some(84),
        },
    );
    assert!(!verify(job, 0, &bound, &other));
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

/// Max HP as the context item `f1`, so a reply can reference it.
fn draft_with_max_hp(phrase: &str) -> Draft {
    let mut draft = draft(phrase);
    draft.items.push((
        super::super::contract::EnvelopeItem {
            alias: "f1".into(),
            id: "max_hp".into(),
            label: "Max HP".into(),
            kind: super::super::contract::ItemKind::Integer,
            value: serde_json::json!(84),
        },
        super::super::output::Supplied {
            address: "field:max_hp".into(),
            integer: true,
            value: Some(84),
        },
    ));
    draft
}

const HEAL_TO_MAX: &str = r#"{"kind":"options","options":[{"expr":{"op":"sub","l":{"ref":"f1"},"r":{"ref":"t"}},"explanation":"up to Max HP"}]}"#;

async fn answer_for(budgets: Budgets, reply: &str, phrase: &str) -> Answer {
    let stub = Arc::new(StubInterpreter::answering([Ok(reply.to_owned())]));
    let assistant = ready(stub, budgets);
    let origin = origin(1, "field:hp", 5);
    let registration = assistant.register(origin.sheet, origin.revision);
    let (id, _) = assistant
        .start(&registration, 1, origin, draft_with_max_hp(phrase))
        .unwrap();
    match until_done(&assistant, 1, 1, id).await {
        Poll::Done(answer) => answer,
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn the_default_model_never_offers_a_field_reference_and_says_to_enter_it_by_hand() {
    // The option references Max HP: with the policy off the whole reply is an abstention
    // that tells the player to enter the value by hand, and the breaker stays closed.
    let off = Budgets {
        field_refs: false,
        ..budgets()
    };
    assert_eq!(
        answer_for(off, HEAL_TO_MAX, "heal to max").await,
        Answer::EnterByHand
    );
    // Any other model keeps the option.
    let Answer::Options(options) = answer_for(budgets(), HEAL_TO_MAX, "heal to max").await else {
        panic!("a model allowed field references offers the option")
    };
    assert_eq!(options[0].candidate.value, 0);
    // A reply that references only the target is offered as before, with the policy off.
    let Answer::Options(options) = answer_for(off, HEAL, "heal 15").await else {
        panic!("a target-only option is offered")
    };
    assert_eq!(options[0].candidate.value, 99);
    // One field-referencing option abstains the whole reply, even beside a target-only one.
    let mixed = r#"{"kind":"options","options":[{"expr":{"op":"add","l":{"ref":"t"},"r":{"lit":15}},"explanation":"heal 15"},{"expr":{"op":"sub","l":{"ref":"f1"},"r":{"ref":"t"}},"explanation":"up to Max HP"}]}"#;
    assert_eq!(answer_for(off, mixed, "heal 15").await, Answer::EnterByHand);
    // A drop for another reason stays "nothing".
    let invented = r#"{"kind":"options","options":[{"expr":{"lit":915},"explanation":"x"}]}"#;
    assert_eq!(answer_for(off, invented, "heal 15").await, Answer::Nothing);
}
