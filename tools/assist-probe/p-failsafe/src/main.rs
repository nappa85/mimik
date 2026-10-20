//! failsafe 1.3.0 against the §1.4 breaker: F1 to F5 with the crate's mock clock.
use std::time::Duration;

use failsafe::futures::CircuitBreaker as FutureBreaker;
use failsafe::{CircuitBreaker, Config, FailurePredicate, backoff, clock, failure_policy};
use serde_json::json;

fn breaker() -> failsafe::StateMachine<failure_policy::ConsecutiveFailures<backoff::Constant>, ()> {
    Config::new()
        .failure_policy(failure_policy::consecutive_failures(3, backoff::constant(Duration::from_secs(120))))
        .build()
}

#[derive(Debug)]
enum E {
    Transport,
    Neutral,
}

struct OnlyTransport;
impl FailurePredicate<E> for OnlyTransport {
    fn is_err(&self, err: &E) -> bool {
        matches!(err, E::Transport)
    }
}

fn main() {
    let out = clock::freeze(|time| {
        let b = breaker();
        // F2: 2 failures keep it closed, the 3rd opens it.
        b.on_error();
        b.on_error();
        let closed_after_two = b.is_call_permitted();
        b.on_error();
        let open_after_three = !b.is_call_permitted();
        // Mixed: failure, success, failure.
        let m = breaker();
        m.on_error();
        m.on_error();
        m.on_success();
        m.on_error();
        m.on_error();
        let mixed_closed = m.is_call_permitted();
        // F2 backoff and F3 half-open: after 2 min, how many concurrent probes are admitted?
        time.advance(Duration::from_secs(119));
        let still_open_at_119 = !b.is_call_permitted();
        time.advance(Duration::from_secs(2));
        let first = b.is_call_permitted();
        let second = b.is_call_permitted();
        let third = b.is_call_permitted();
        // F1/F5 through the future API: a neutral error with a predicate.
        let f = breaker();
        f.on_error();
        f.on_error();
        let r: Result<(), failsafe::Error<E>> = futures_executor::block_on(
            FutureBreaker::call_with(&f, OnlyTransport, async { Err::<(), E>(E::Neutral) }));
        f.on_error();
        let neutral_reset_count = f.is_call_permitted(); // true means the neutral error reset it
        // F5: a dropped (never completed) future in half-open records nothing.
        let h = breaker();
        h.on_error();
        h.on_error();
        h.on_error();
        time.advance(Duration::from_secs(121));
        {
            let fut = FutureBreaker::call_with(&h, OnlyTransport, futures_pending());
            drop(fut);
        }
        let after_drop_permitted = h.is_call_permitted();
        let _ = r;
        json!({
            "f2_closed_after_two_failures": closed_after_two,
            "f2_open_after_three": open_after_three,
            "f2_mixed_failure_success_failure_stays_closed": mixed_closed,
            "f2_open_until_backoff": still_open_at_119,
            "f3_half_open_permits": [first, second, third],
            "f3_single_probe": first && !second && !third,
            "f1_neutral_error_via_future_counts_as_success": neutral_reset_count,
            "f5_dropped_future_permitted_after": after_drop_permitted,
            "f4_mock_clock": "failsafe::clock::freeze (thread-local)",
        })
    });
    println!("{}", serde_json::to_string_pretty(&out).unwrap());
}

fn futures_pending() -> impl std::future::Future<Output = Result<(), E>> {
    std::future::pending()
}
