use super::*;
use crate::assistant::jobs::Budgets;
use crate::assistant::request::Bounds;
use crate::assistant::runtime::budgets::{SAMPLE_TOKENS, Sample};

/// A sample at 88 ms per prompt token and 213 ms per generated token (the CPU container's).
fn sample() -> Sample {
    Sample {
        wall_ms: 88.0 * 393.0 + 213.0 * 64.0 + 1_000.0,
        prompt_n: 100.0 + 293.0,
        prompt_ms: 88.0 * 393.0,
        cached_n: 1290.0,
        predicted_n: f64::from(SAMPLE_TOKENS),
        predicted_ms: 213.0 * 64.0,
        filler_tokens: 293.0,
    }
}

pub(crate) fn make(path: Path) -> Inputs {
    let measured = Measured {
        path,
        max_tokens: 1024,
        prefix_tokens: 1460.0,
        samples: [sample(), sample()],
    };
    let mut derived = crate::assistant::runtime::budgets::derive(&measured).unwrap();
    // A long queue wait: most tests are about something else than Q.
    derived.budgets.queue_wait = Duration::from_secs(5_000);
    Inputs {
        measured,
        derived,
        primed: Primed::of(1460.0, 88.0),
        system_tokens: 1800.0,
        reply_tokens: 200,
        entry_mib: 120.0,
    }
}

#[test]
fn a_reminder_has_its_own_deadline_busy_term_and_extra_wait() {
    let inputs = make(Path::Cpu);
    let budgets = derive(&inputs).unwrap();
    // 88 ms x (1,800 + N(B)) of prompt, 200 tokens of reply at 213 ms, 1 s of overhead.
    let n = f64::from(u32::try_from(inputs.derived.budgets.bounds.tokens).unwrap());
    let prompt = 0.088 * (1800.0 + n);
    let reply = 0.213 * 200.0 * (1.0 + Path::Cpu.slowdown() * 100.0);
    assert!((budgets.estimate_s - (1.0 + prompt + reply)).abs() < 1e-6);
    assert_eq!(budgets.run.as_secs() % 5, 0);
    assert!(budgets.run.as_secs_f64() >= 1.25 * budgets.estimate_s);
    assert_eq!(budgets.busy, Duration::from_secs_f64(prompt.ceil()));
    // CPU: the cache entry's load, labelled as the dev host's constant.
    assert!(budgets.restore.dev_host_constant);
    assert!(budgets.restore.seconds < 1.0);
    assert!(budgets.extra_wait >= budgets.busy);
    // Never the interpretation's budgets.
    assert_ne!(budgets.run, inputs.derived.budgets.run);
}

#[test]
fn the_gpu_restore_is_the_entry_load_cost_and_the_fallback_reprime_is_priming_time() {
    let mut inputs = make(Path::Gpu);
    inputs.primed = Primed::of(1460.0, 10.0);
    inputs.entry_mib = 260.0;
    let budgets = derive(&inputs).unwrap();
    // 0.53 ms per MiB of entry plus 10 ms, a figure from the development host.
    assert!(budgets.restore.dev_host_constant);
    assert!((budgets.restore.seconds - (260.0 * 0.00053 + 0.010)).abs() < 1e-9);
    assert!(budgets.restore.seconds < 0.2);
    // The priming time is only the fallback's.
    assert!((budgets.reprime.as_secs_f64() - 14.6).abs() < 1e-6);
    // Without priming's time, the measured prompt rate times the prefix.
    inputs.primed.prompt_ms = f64::NAN;
    let budgets = derive(&inputs).unwrap();
    assert!((budgets.reprime.as_secs_f64() - 0.088 * 1460.0).abs() < 1e-6);
}

#[test]
fn a_deadline_over_the_ceiling_makes_reminders_unavailable_with_the_reason() {
    let mut inputs = make(Path::Cpu);
    inputs.reply_tokens = 2560;
    inputs.system_tokens = 1800.0;
    // 2,560 tokens at 213 ms and the slowdown is over 600 s / 1.25.
    let error = derive(&inputs).unwrap_err();
    assert!(
        matches!(error, Unavailable::OverCeiling { .. }),
        "{error:?}"
    );
    assert!(error.describe(Path::Cpu).contains("600 s ceiling"));
}

#[test]
fn a_reminder_that_could_make_a_scribble_reading_refused_is_unavailable() {
    let mut inputs = make(Path::Cpu);
    // The busy term plus the restore must be at most Q.
    inputs.derived.budgets = Budgets {
        queue_wait: Duration::from_secs(5),
        bounds: Bounds {
            bytes: 1024,
            tokens: 293,
        },
        ..inputs.derived.budgets
    };
    let error = derive(&inputs).unwrap_err();
    assert!(
        matches!(error, Unavailable::BeyondQueue { .. }),
        "{error:?}"
    );
    // At exactly Q it is available.
    let mut inputs = make(Path::Cpu);
    inputs.derived.budgets.queue_wait = Duration::from_secs(10_000);
    let needed =
        derive(&inputs).unwrap().extra_wait.as_secs() + inputs.derived.budgets.run.as_secs();
    inputs.derived.budgets.queue_wait = Duration::from_secs(needed);
    assert!(derive(&inputs).is_ok());
    inputs.derived.budgets.queue_wait = Duration::from_secs(needed - 1);
    assert!(derive(&inputs).is_err());
}

#[test]
fn a_restore_beyond_the_interpretations_margin_is_unavailable_on_both_paths() {
    for path in [Path::Cpu, Path::Gpu] {
        let mut inputs = make(path);
        inputs.entry_mib = 10_000_000.0;
        inputs.derived.budgets.queue_wait = Duration::from_secs(10_000);
        assert_eq!(derive(&inputs), Err(Unavailable::BeyondMargin), "{path:?}");
    }
}

#[test]
fn an_unavailable_reason_says_reminders_are_off_and_what_helps() {
    let error = Unavailable::OverCeiling { needed_s: 700 };
    let cpu = error.describe(Path::Cpu);
    assert!(
        cpu.starts_with("reminders are off for this generation"),
        "{cpu}"
    );
    assert!(
        cpu.contains("more threads") && cpu.contains("GPU path"),
        "{cpu}"
    );
    let gpu = error.describe(Path::Gpu);
    assert!(
        gpu.starts_with("reminders are off for this generation"),
        "{gpu}"
    );
    assert!(!gpu.contains("more threads"), "{gpu}");
}

#[test]
fn missing_measurements_are_unavailable_not_a_panic() {
    let mut inputs = make(Path::Cpu);
    inputs.system_tokens = f64::NAN;
    assert_eq!(derive(&inputs), Err(Unavailable::Unmeasured));
}

#[test]
fn a_reminder_that_does_not_fit_the_context_is_unavailable_with_the_reason() {
    let mut inputs = make(Path::Gpu);
    // The system message, N(B) and the reply bound must fit the context with the template's
    // margin; 8,192 is the context.
    let n = f64::from(u32::try_from(inputs.derived.budgets.bounds.tokens).unwrap());
    let reply = f64::from(inputs.reply_tokens);
    inputs.system_tokens = 8192.0 - n - reply - 64.0;
    assert!(!matches!(
        derive(&inputs),
        Err(Unavailable::BeyondContext { .. })
    ));
    inputs.system_tokens += 1.0;
    let error = derive(&inputs).unwrap_err();
    assert!(
        matches!(error, Unavailable::BeyondContext { .. }),
        "{error:?}"
    );
    assert!(error.describe(Path::Gpu).contains("context"));
}

#[test]
fn a_reminder_worse_than_a_running_interpretation_is_unavailable() {
    // A scribble reading right after a reminder may wait for the reminder's busy time plus the
    // restore; a running interpretation makes it wait R plus its busy term, no more is allowed.
    let mut inputs = make(Path::Gpu);
    inputs.derived.budgets.queue_wait = Duration::from_secs(10_000);
    let needed = derive(&inputs).unwrap().extra_wait.as_secs();
    inputs.derived.budgets.busy_after_expiry = Duration::ZERO;
    // R's margin over the estimate is not what this is about.
    inputs.derived.estimate_s = 0.0;
    inputs.derived.budgets.run = Duration::from_secs(needed);
    assert!(derive(&inputs).is_ok());
    inputs.derived.budgets.run = Duration::from_secs(needed - 1);
    let error = derive(&inputs).unwrap_err();
    assert!(
        matches!(error, Unavailable::WorseThanInterpretation { .. }),
        "{error:?}"
    );
    // Its busy term counts as the interpretation's.
    inputs.derived.budgets.busy_after_expiry = Duration::from_secs(1);
    assert!(derive(&inputs).is_ok());
}

#[test]
fn a_reading_behind_a_reminder_must_still_fit_q_after_the_reading_ahead_of_it() {
    // CPU: busy + restore + R (the one reading that may wait behind the reminder) at most Q.
    let mut inputs = make(Path::Cpu);
    inputs.derived.budgets.queue_wait = Duration::from_secs(10_000);
    inputs.derived.budgets.run = Duration::from_mins(50);
    let needed = derive(&inputs).unwrap().extra_wait.as_secs();
    inputs.derived.budgets.queue_wait = Duration::from_secs(needed + 3_000);
    assert!(derive(&inputs).is_ok());
    inputs.derived.budgets.queue_wait = Duration::from_secs(needed + 2_999);
    assert!(matches!(
        derive(&inputs),
        Err(Unavailable::BeyondQueue { .. })
    ));
    // GPU: R for each of the D readings that may wait.
    let mut inputs = make(Path::Gpu);
    inputs.derived.budgets.queue_wait = Duration::from_secs(100_000);
    inputs.derived.budgets.run = Duration::from_mins(50);
    inputs.derived.budgets.depth = 3;
    let needed = derive(&inputs).unwrap().extra_wait.as_secs();
    inputs.derived.budgets.queue_wait = Duration::from_secs(needed + 9_000);
    assert!(derive(&inputs).is_ok());
    inputs.derived.budgets.queue_wait = Duration::from_secs(needed + 8_999);
    assert!(matches!(
        derive(&inputs),
        Err(Unavailable::BeyondQueue { .. })
    ));
}

/// On the GPU path the fallback re-prime follows a reminder after a failed cache check, and
/// admission adds its time to the wait, so the availability checks count the longer of the
/// restore and the re-prime.
#[test]
fn the_gpu_availability_checks_count_the_fallback_reprime() {
    let mut inputs = make(Path::Gpu);
    inputs.primed = Primed::of(1460.0, 88.0);
    inputs.derived.budgets.queue_wait = Duration::from_secs(10_000);
    let open = derive(&inputs).unwrap();
    assert!(
        open.extra_wait.as_secs_f64() >= open.busy.as_secs_f64() + open.reprime.as_secs_f64() - 1.0
    );
    let ahead =
        inputs.derived.budgets.run.as_secs() * u64::try_from(inputs.derived.budgets.depth).unwrap();
    inputs.derived.budgets.queue_wait = Duration::from_secs(ahead + open.busy.as_secs() + 1);
    assert!(matches!(
        derive(&inputs),
        Err(Unavailable::BeyondQueue { .. })
    ));
    // The CPU path has no re-prime.
    let mut cpu = make(Path::Cpu);
    cpu.primed = Primed::of(1460.0, 88.0);
    cpu.derived.budgets.queue_wait = Duration::from_secs(10_000);
    let open = derive(&cpu).unwrap();
    assert!(open.extra_wait.as_secs_f64() < open.busy.as_secs_f64() + 2.0);
}
