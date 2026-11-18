use super::*;
use crate::assistant::runtime::budgets::{SAMPLE_TOKENS, Sample};
use crate::assistant::runtime::launch::entry_mib;

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
        system_tokens: 1800.0,
        reply_tokens: 200,
        entry_mib: 120.0,
        reminder_entry_mib: 100.0,
    }
}

/// A calibration sample at `p_ms` per prompt token and `g_ms` per generated token whose
/// filler is `filler` tokens (a tail of 100 tokens beyond it, 0.5 s of overhead).
fn measured_sample(p_ms: f64, g_ms: f64, filler: f64, cached: f64) -> Sample {
    let prompt_n = filler + 100.0;
    Sample {
        wall_ms: p_ms * prompt_n + g_ms * 64.0 + 500.0,
        prompt_n,
        prompt_ms: p_ms * prompt_n,
        cached_n: cached,
        predicted_n: f64::from(SAMPLE_TOKENS),
        predicted_ms: g_ms * 64.0,
        filler_tokens: filler,
    }
}

/// The development host as 3c-1b measured it (`docs/results.md`): the GPU (5.17 ms per prompt
/// token, B 4,096, N(B) 1,128, R 105 s, D 1, Q 111 s) or the CPU (about 42 ms per prompt
/// token, 130 ms per token, N(B) 280).
fn dev_host(path: Path, system_tokens: f64) -> Inputs {
    let (p, g, filler, prefix, cached) = match path {
        Path::Gpu => (5.17, 72.0, 1128.0, 1810.0, 1810.0),
        Path::Cpu => (42.0, 130.0, 280.0, 1460.0, 1400.0),
    };
    let samples = [
        measured_sample(p, g, filler, cached),
        measured_sample(p, g, filler + 5.0, cached),
    ];
    let measured = Measured {
        path,
        max_tokens: 1024,
        prefix_tokens: prefix,
        samples,
    };
    let derived = crate::assistant::runtime::budgets::derive(&measured).unwrap();
    let (sha, size) = match path {
        Path::Gpu => (
            crate::assistant::model::GPU_DEFAULT_MODEL_SHA256,
            5_680_522_464,
        ),
        Path::Cpu => (crate::assistant::model::DEFAULT_MODEL_SHA256, 2_244_011_552),
    };
    let n = f64::from(u32::try_from(derived.budgets.bounds.tokens).unwrap());
    let reply = 900.0;
    Inputs {
        measured,
        derived,
        system_tokens,
        reply_tokens: 900,
        entry_mib: entry_mib(sha, size, prefix + n + 1024.0),
        reminder_entry_mib: entry_mib(sha, size, system_tokens + n + reply),
    }
}

#[test]
fn a_reminder_has_its_own_deadline_and_two_busy_terms() {
    let inputs = make(Path::Cpu);
    let budgets = derive(&inputs).unwrap();
    // 88 ms x (1,800 + N(B)) of prompt, 200 tokens of reply at 213 ms, 1 s of overhead.
    let n = f64::from(u32::try_from(inputs.derived.budgets.bounds.tokens).unwrap());
    let prompt = 0.088 * (1800.0 + n);
    let reply = 0.213 * 200.0 * (1.0 + Path::Cpu.slowdown() * 100.0);
    assert!((budgets.estimate_s - (1.0 + prompt + reply)).abs() < 1e-6);
    assert_eq!(budgets.run.as_secs() % 5, 0);
    assert!(budgets.run.as_secs_f64() >= 1.25 * budgets.estimate_s);
    // Unverified: the whole prompt and the interpretation's restore, rounded up.
    let restore = 120.0 * RESTORE_S_PER_MIB;
    assert_eq!(
        budgets.busy_unverified,
        Duration::from_secs_f64((prompt + restore).ceil())
    );
    // Verified: the reminder's own restore, the variable part with the template's tail, and
    // the interpretation's restore.
    let variable = 0.088 * (n + TEMPLATE_MARGIN_TOKENS);
    let verified = 100.0 * RESTORE_S_PER_MIB + variable + restore;
    assert_eq!(
        budgets.busy_verified,
        Duration::from_secs_f64(verified.ceil())
    );
    assert!(budgets.busy_verified < budgets.busy_unverified);
    // CPU: the cache entry's load, labelled as the dev host's constant.
    assert!(budgets.restore.dev_host_constant);
    assert!(budgets.restore.seconds < 1.0);
    assert_eq!(budgets.system_tokens, 1800);
    // Never the interpretation's budgets.
    assert_ne!(budgets.run, inputs.derived.budgets.run);
}

#[test]
fn the_gpu_restore_is_the_entry_load_cost() {
    let mut inputs = make(Path::Gpu);
    inputs.entry_mib = 260.0;
    let budgets = derive(&inputs).unwrap();
    // 0.53 ms per MiB of entry plus 10 ms, a figure from the development host.
    assert!(budgets.restore.dev_host_constant);
    assert!((budgets.restore.seconds - (260.0 * 0.00053 + 0.010)).abs() < 1e-9);
    assert!(budgets.restore.seconds < 0.2);
}

/// The dev host's GPU, which the 3c-1b live run found unavailable ("could wait 129 s, more than
/// the 111 s it may"): with the real cost shown as an extra wait it is available, the verified
/// extra wait is at most 7 s and the unverified one stays within a running interpretation's.
#[test]
fn the_dev_hosts_gpu_has_reminders_with_a_small_verified_extra_wait() {
    let inputs = dev_host(Path::Gpu, 1820.0);
    // The figures the live run saw: B 4,096, N(B) 1,128, R 105 s, D 1, Q 111 s.
    assert_eq!(inputs.derived.budgets.bounds.tokens, 1128);
    assert_eq!(inputs.derived.budgets.run.as_secs(), 105);
    assert_eq!(inputs.derived.budgets.depth, 1);
    assert!((111..=112).contains(&inputs.derived.budgets.queue_wait.as_secs()));
    let budgets = derive(&inputs).unwrap();
    assert!(
        budgets.busy_verified.as_secs() <= 7,
        "{:?}",
        budgets.busy_verified
    );
    let y = budgets.busy_unverified.as_secs();
    assert!((13..=16).contains(&y), "{y}");
    let interpretation =
        (inputs.derived.budgets.run + inputs.derived.budgets.busy_after_expiry).as_secs();
    assert!(y <= interpretation);
}

/// The dev host's CPU: the whole reminder prompt about 68 s, shown as the unverified extra wait.
#[test]
fn the_dev_hosts_cpu_has_reminders_with_the_whole_prompt_as_the_unverified_term() {
    let inputs = dev_host(Path::Cpu, 1410.0);
    let budgets = derive(&inputs).unwrap();
    let y = budgets.busy_unverified.as_secs();
    assert!((64..=75).contains(&y), "{y}");
    assert!(budgets.busy_verified.as_secs() < 20);
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

/// A queue wait shorter than a reminder's busy term no longer makes reminders unavailable:
/// the term is added to a reading's allowance and shown.
#[test]
fn a_short_queue_wait_does_not_make_reminders_unavailable() {
    let mut inputs = make(Path::Cpu);
    inputs.derived.budgets.queue_wait = Duration::from_secs(5);
    assert!(derive(&inputs).is_ok());
    let mut inputs = make(Path::Gpu);
    inputs.derived.budgets.queue_wait = Duration::from_secs(5);
    assert!(derive(&inputs).is_ok());
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
fn an_unavailable_reason_says_what_helps_on_the_cpu() {
    let error = Unavailable::OverCeiling { needed_s: 700 };
    let cpu = error.describe(Path::Cpu);
    assert!(!cpu.starts_with("reminders are off"), "{cpu}");
    assert!(
        cpu.contains("more threads") && cpu.contains("GPU path"),
        "{cpu}"
    );
    let gpu = error.describe(Path::Gpu);
    assert!(!gpu.starts_with("reminders are off"), "{gpu}");
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
    // A scribble reading right after a cancelled, unverified reminder may wait for its whole
    // prompt and the restore; a running interpretation makes it wait R plus its busy term, no
    // more is allowed.
    let mut inputs = make(Path::Gpu);
    inputs.derived.budgets.queue_wait = Duration::from_secs(10_000);
    let needed = derive(&inputs).unwrap().busy_unverified.as_secs();
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
