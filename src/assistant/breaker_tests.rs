use super::*;

fn fail(breaker: &mut Breaker, ticket: Ticket, at: Instant) {
    assert_ne!(breaker.admit(ticket, at, None), Admission::Refused);
    breaker.record(ticket, Outcome::Failure { generation: 1 }, at);
}

#[test]
fn three_failures_open_it_and_two_do_not() {
    let start = Instant::now();
    let mut breaker = Breaker::new();
    fail(&mut breaker, 1, start);
    fail(&mut breaker, 2, start);
    assert!(breaker.is_closed());
    fail(&mut breaker, 3, start);
    assert!(!breaker.is_closed());
    assert_eq!(breaker.admit(4, start, None), Admission::Refused);
    assert!(breaker.refuses(start + Duration::from_secs(119)));
    assert!(!breaker.refuses(start + OPEN_FOR));
}

#[test]
fn mixed_sequences_do_not_open_it() {
    let start = Instant::now();
    let mut breaker = Breaker::new();
    fail(&mut breaker, 1, start);
    fail(&mut breaker, 2, start);
    breaker.record(3, Outcome::Success, start);
    fail(&mut breaker, 4, start);
    fail(&mut breaker, 5, start);
    assert!(breaker.is_closed());
}

#[test]
fn dropped_or_nonsense_answers_and_neutral_ends_never_count() {
    let start = Instant::now();
    let mut breaker = Breaker::new();
    // A completed 2xx exchange is a success whatever its content (every option dropped).
    for ticket in 0..100 {
        breaker.admit(ticket, start, None);
        breaker.record(ticket, Outcome::Success, start);
    }
    // Supersession, cancellation, queue-wait and run-deadline expiries.
    for ticket in 100..110 {
        breaker.admit(ticket, start, None);
        breaker.record(ticket, Outcome::Neutral, start);
    }
    assert!(breaker.is_closed());
    assert_eq!(breaker.counted(), 0);
}

fn opened() -> (Breaker, Instant) {
    let start = Instant::now();
    let mut breaker = Breaker::new();
    for ticket in 1..=3 {
        fail(&mut breaker, ticket, start);
    }
    (breaker, start + OPEN_FOR)
}

#[test]
fn one_half_open_probe_while_others_are_refused() {
    let (mut breaker, due) = opened();
    assert_eq!(breaker.admit(10, due, None), Admission::Probe);
    assert_eq!(breaker.admit(11, due, None), Admission::Refused);
    assert!(breaker.refuses(due));
    assert!(breaker.may_run(10));
    assert!(!breaker.may_run(11));
}

#[test]
fn probe_success_closes_and_failure_reopens() {
    let (mut breaker, due) = opened();
    breaker.admit(10, due, None);
    breaker.record(10, Outcome::Success, due);
    assert!(breaker.is_closed());
    let (mut breaker, due) = opened();
    breaker.admit(10, due, None);
    breaker.record(10, Outcome::Failure { generation: 1 }, due);
    assert_eq!(breaker.admit(11, due, None), Admission::Refused);
    assert_eq!(breaker.admit(11, due + OPEN_FOR, None), Admission::Probe);
}

#[test]
fn a_cancelled_probe_leaves_it_half_open_for_the_next_job() {
    let (mut breaker, due) = opened();
    breaker.admit(10, due, None);
    breaker.record(10, Outcome::Neutral, due);
    assert!(!breaker.is_closed());
    assert!(!breaker.refuses(due));
    assert_eq!(breaker.admit(11, due, None), Admission::Probe);
}

#[test]
fn a_start_superseding_the_probe_becomes_the_probe() {
    let (mut breaker, due) = opened();
    breaker.admit(10, due, None);
    assert_eq!(breaker.admit(11, due, Some(10)), Admission::Probe);
    assert!(breaker.may_run(11));
    assert!(!breaker.may_run(10));
    // The superseded probe's neutral end does not clear the newcomer.
    breaker.record(10, Outcome::Neutral, due);
    assert!(breaker.may_run(11));
}

#[test]
fn an_exit_within_two_seconds_withdraws_the_count_that_opened_it() {
    let start = Instant::now();
    let mut breaker = Breaker::new();
    fail(&mut breaker, 1, start);
    fail(&mut breaker, 2, start);
    fail(&mut breaker, 3, start + Duration::from_secs(10));
    assert!(!breaker.is_closed());
    breaker.generation_ended(1, start + Duration::from_secs(11));
    assert!(breaker.is_closed());
    // The two older counts stay (they are older than 2 s at the exit).
    assert_eq!(breaker.counted(), 2);
}

#[test]
fn an_exit_after_two_seconds_or_of_another_generation_keeps_the_count() {
    let start = Instant::now();
    let mut breaker = Breaker::new();
    fail(&mut breaker, 1, start);
    breaker.generation_ended(1, start + Duration::from_secs(3));
    assert_eq!(breaker.counted(), 1);
    breaker.generation_ended(2, start);
    assert_eq!(breaker.counted(), 1);
}

#[test]
fn a_withdrawn_probe_failure_goes_back_to_half_open() {
    let (mut breaker, due) = opened();
    breaker.admit(10, due, None);
    breaker.record(10, Outcome::Failure { generation: 1 }, due);
    breaker.generation_ended(1, due + Duration::from_secs(1));
    assert_eq!(
        breaker.admit(11, due + Duration::from_secs(1), None),
        Admission::Probe
    );
}

#[test]
fn reset_closes_it() {
    let (mut breaker, due) = opened();
    breaker.reset();
    assert!(breaker.is_closed());
    assert_eq!(breaker.admit(1, due, None), Admission::Go);
}
