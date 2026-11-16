//! Formula fixtures (plan §2 2c-2a, "budgets"): the 2b figures are illustrations, not
//! evidence. The samples generate 64 tokens; `max_tokens` is the owner's reply length.
use super::*;

/// A sample with per-token prompt time `p` and generation time `g` (seconds), overhead `o`
/// (seconds), tail `t`, filler tokens `n`, the 64 tokens a sample generates and `cached`
/// tokens read.
fn sample(
    prompt_s: f64,
    generation_s: f64,
    overhead_s: f64,
    tail: f64,
    filler: f64,
    cached: f64,
) -> Sample {
    let prompt_n = tail + filler;
    let predicted_n = f64::from(SAMPLE_TOKENS);
    let prompt_time = prompt_s * prompt_n * 1000.0;
    let generation_time = generation_s * predicted_n * 1000.0;
    Sample {
        wall_ms: prompt_time + generation_time + overhead_s * 1000.0,
        prompt_n,
        prompt_ms: prompt_time,
        cached_n: cached,
        predicted_n,
        predicted_ms: generation_time,
        filler_tokens: filler,
    }
}

fn measured(path: Path, max_tokens: u32, samples: [Sample; 2]) -> Measured {
    measured_with_prefix(path, max_tokens, 1460.0, samples)
}

fn measured_with_prefix(
    path: Path,
    max_tokens: u32,
    prefix_tokens: f64,
    samples: [Sample; 2],
) -> Measured {
    Measured {
        path,
        max_tokens,
        prefix_tokens,
        samples,
    }
}

/// The 2b container figures: p about 88 ms and g about 213 ms per token at 2 threads, o
/// about 1 s, about 3.5 bytes per token (293 tokens in 1 KiB).
fn container(t: f64) -> Sample {
    sample(0.088, 0.213, 1.0, t, 293.0, 1290.0)
}

/// A sample whose estimate is almost exactly `seconds` at a reply of 1,024 tokens (the
/// prompt term is a few hundred microseconds).
fn takes(seconds: f64) -> Sample {
    sample(1e-6, seconds / 1024.0, 0.0, 100.0, 293.0, 1290.0)
}

fn budgets(derived: &Derived) -> (usize, usize, u64, usize, u64) {
    let b = derived.budgets;
    (
        b.bounds.bytes,
        b.bounds.tokens,
        b.run.as_secs(),
        b.depth,
        b.queue_wait.as_secs(),
    )
}

#[test]
fn container_figures_with_a_192_token_reply_fit_the_largest_cpu_bound() {
    let derived = derive(&measured(
        Path::Cpu,
        192,
        [container(170.0), container(170.0)],
    ))
    .unwrap();
    // T = 1 + 0.088 × (170 + 293) + 0.213 × 192, R = 1.25 T up to 5 s; the total wait is
    // twice R, the queue the rest.
    assert!((derived.estimate_s - 82.64).abs() < 1e-9);
    assert_eq!(budgets(&derived), (1024, 293, 105, 1, 105));
}

#[test]
fn the_reply_length_enters_the_estimate_through_the_measured_speed() {
    // The same samples, two reply lengths: T grows by g × the extra tokens.
    let at = |max_tokens| {
        derive(&measured(
            Path::Cpu,
            max_tokens,
            [container(170.0), container(170.0)],
        ))
        .unwrap()
    };
    let (short, default) = (at(256), at(1024));
    for (derived, tokens) in [(short, 256.0), (default, 1024.0)] {
        let expected = 1.0 + 0.088 * (170.0 + 293.0) + 0.213 * tokens;
        assert!((derived.estimate_s - expected).abs() < 1e-9, "{tokens}");
    }
    // 1 + 40.744 + 218.112 = 259.856 s: R 325.
    assert_eq!(budgets(&default), (1024, 293, 325, 1, 325));
    // At 2,560 tokens B 1024 is 587.0 s (R 735 s), B 768 (N 219) 580.5 s (R 730 s) and B 512
    // (N 146) 574.1 s (R 720 s): all over the ceiling.
    assert_eq!(
        derive(&measured(
            Path::Cpu,
            2560,
            [container(170.0), container(170.0)]
        )),
        Err(NoBudgets::TooSlow {
            needed_s: 720,
            cap_s: 600
        })
    );
}

#[test]
fn container_figures_with_the_high_tail_and_a_long_reply_are_too_slow() {
    // 1 + 0.088 × (365 + 146) + 0.213 × 2560 = 591.1 s at B 512: R 740 s, over the ceiling.
    assert_eq!(
        derive(&measured(
            Path::Cpu,
            2560,
            [container(365.0), container(365.0)]
        )),
        Err(NoBudgets::TooSlow {
            needed_s: 740,
            cap_s: 600
        })
    );
}

#[test]
fn gpu_figures_give_the_largest_bound() {
    // About 1,100 prompt and 80 generated tokens per second on the RX 6650 XT.
    let gpu = sample(1.0 / 1100.0, 1.0 / 80.0, 0.3, 250.0, 1170.0, 1290.0);
    let derived = derive(&measured(Path::Gpu, 192, [gpu, gpu])).unwrap();
    assert_eq!(budgets(&derived), (4096, 1170, 15, 4, 60));
}

#[test]
fn a_gpu_measurement_picks_the_largest_bound_that_fits_the_ceiling() {
    // Reply of 2,560 tokens at 0.1 s each is 256 s before any prompt.
    let pick = |p| {
        let s = sample(p, 0.1, 0.3, 250.0, 1170.0, 1290.0);
        derive(&measured(Path::Gpu, 2560, [s, s]))
    };
    // B 4096: 0.3 + 0.2 × 1420 + 256 = 540.3 s, R 680 s (over); B 2048: 423.3 s, R 530 s.
    assert_eq!(budgets(&pick(0.2).unwrap()), (2048, 585, 530, 1, 530));
    // B 4096 and 2048 are over at p = 0.3; B 1024: 0.3 + 0.3 × 542 + 256 = 418.9 s, R 525 s.
    assert_eq!(budgets(&pick(0.3).unwrap()), (1024, 292, 525, 1, 525));
    // Slower generation: nothing fits, and the ceiling is the cap named.
    let slowest = sample(0.2, 0.3, 0.3, 250.0, 1170.0, 1290.0);
    assert!(matches!(
        derive(&measured(Path::Gpu, 2560, [slowest, slowest])),
        Err(NoBudgets::TooSlow { cap_s: 600, .. })
    ));
}

#[test]
fn a_cpu_measurement_that_fits_only_768_picks_it() {
    // B 1024: 1 + 0.28 × 393 + 384 = 495.04 s, R 620 s (over); B 768: 474.3 s, R 595 s.
    let s = sample(0.28, 0.15, 1.0, 100.0, 293.0, 1290.0);
    let derived = derive(&measured(Path::Cpu, 2560, [s, s])).unwrap();
    assert_eq!(budgets(&derived), (768, 219, 595, 1, 595));
}

#[test]
fn the_ceiling_is_600_seconds_on_both_paths() {
    // T just under and just over 480 s: R 600 s fits, R 605 s does not.
    for path in [Path::Cpu, Path::Gpu] {
        let fits = derive(&measured(path, 1024, [takes(479.0), takes(479.0)])).unwrap();
        assert_eq!(fits.budgets.run.as_secs(), 600, "{path:?}");
        assert_eq!(
            derive(&measured(path, 1024, [takes(481.0), takes(481.0)])),
            Err(NoBudgets::TooSlow {
                needed_s: 605,
                cap_s: 600
            }),
            "{path:?}"
        );
    }
}

#[test]
fn the_queue_follows_a_total_wait_of_the_larger_of_120_s_and_twice_r() {
    // (R, CPU queue, GPU depth, GPU queue): the CPU keeps one job and waits total - R; the
    // GPU admits (total - R) / R jobs, between 1 and 4.
    for (r, cpu_queue, depth, gpu_queue) in [
        (20_u32, 100_u64, 4_usize, 80_u64),
        (40, 80, 2, 80),
        (60, 60, 1, 60),
        (300, 300, 1, 300),
    ] {
        let seconds = f64::from(r) * 0.8 - 1.0;
        let cpu = derive(&measured(Path::Cpu, 1024, [takes(seconds), takes(seconds)])).unwrap();
        assert_eq!(cpu.budgets.run.as_secs(), u64::from(r));
        assert_eq!(cpu.budgets.depth, 1);
        assert_eq!(cpu.budgets.queue_wait.as_secs(), cpu_queue, "R {r}");
        let gpu = derive(&measured(Path::Gpu, 1024, [takes(seconds), takes(seconds)])).unwrap();
        assert_eq!(gpu.budgets.depth, depth, "R {r}");
        assert_eq!(gpu.budgets.queue_wait.as_secs(), gpu_queue, "R {r}");
    }
}

#[test]
fn the_request_the_reply_and_the_template_margin_must_fit_the_context() {
    // CTX_SIZE 8,192: prefix + N(B) + max_tokens + 64. Cached tokens follow the prefix.
    let s = |cached| sample(0.0001, 0.001, 0.0, 100.0, 293.0, cached);
    let at = |prefix, max_tokens| {
        derive(&measured_with_prefix(
            Path::Cpu,
            max_tokens,
            prefix,
            [s(prefix), s(prefix)],
        ))
    };
    // 5,300 + 293 + 2,560 + 64 is 8,217: B 1024 is refused, B 768 (N 219: 8,143) is chosen.
    assert_eq!(at(5300.0, 2560).unwrap().budgets.bounds.bytes, 768);
    // Exactly full: 8,192 = 5,275 + 293 + 2,560 + 64 fits at B 1024; one token more does not.
    assert_eq!(at(5275.0, 2560).unwrap().budgets.bounds.bytes, 1024);
    assert_eq!(at(5276.0, 2560).unwrap().budgets.bounds.bytes, 768);
    // B 512 (N 146) needs 5,300 + 146 + 2,560 + 64 = 8,070 at most: with 5,422 it is 8,192.
    assert_eq!(at(5422.0, 2560).unwrap().budgets.bounds.bytes, 512);
    // One more token and no bound fits: refused, never cut silently.
    assert_eq!(at(5423.0, 2560), Err(NoBudgets::ReplyTooLarge));
    assert_eq!(at(7000.0, 1024), Err(NoBudgets::ReplyTooLarge));
}

#[test]
fn unequal_filler_token_counts_use_the_smaller_in_both_estimates() {
    let a = container(170.0);
    let mut b = sample(0.088, 0.213, 1.0, 170.0, 280.0, 1290.0);
    // The second sample is slower in generation.
    let extra = b.predicted_ms * 0.01;
    b.predicted_ms += extra;
    b.wall_ms += extra;
    let derived = derive(&measured(Path::Cpu, 192, [a, b])).unwrap();
    // N(1024) is the smaller count, 280.
    assert_eq!(derived.budgets.bounds.tokens, 280);
    // Both estimates use it; the second is the larger.
    let t_a = 1.0 + 0.088 * (170.0 + 280.0) + 0.213 * 192.0;
    let t_b = 1.0 + 0.088 * (170.0 + 280.0) + 0.213 * 1.01 * 192.0;
    assert!(t_b > t_a);
    assert!(
        (derived.estimate_s - t_b).abs() < 1e-9,
        "{}",
        derived.estimate_s
    );
}

#[test]
fn the_estimate_is_the_larger_sample_per_candidate() {
    // One sample slower in prompt, the other slower in generation: for each candidate the
    // larger of the two decides, so the pair fits no better than either alone. Reply 1,024.
    let prompt_slow = sample(0.9, 0.15, 1.0, 100.0, 293.0, 1290.0);
    let generation_slow = sample(0.03, 0.30, 1.0, 100.0, 293.0, 1290.0);
    let alone = |s: Sample| derive(&measured(Path::Cpu, 1024, [s, s]));
    let pair = derive(&measured(Path::Cpu, 1024, [prompt_slow, generation_slow])).unwrap();
    let a = alone(prompt_slow).unwrap();
    let b = alone(generation_slow).unwrap();
    assert!(pair.budgets.bounds.bytes <= a.budgets.bounds.bytes.min(b.budgets.bounds.bytes));
    // The prompt-slow sample alone fits 768 (B 1024: 508.3 s, R 640 s), the generation-slow
    // one 1,024 (320 s), and the pair 768. Mixing the worst prompt rate of one with the worst
    // generation rate of the other would fit nothing: T is the larger sample per candidate,
    // not a worst-of-rates bound.
    assert_eq!(a.budgets.bounds.bytes, 768);
    assert_eq!(b.budgets.bounds.bytes, 1024);
    assert_eq!(pair.budgets.bounds.bytes, 768);
    let mixed = sample(0.9, 0.30, 1.0, 100.0, 293.0, 1290.0);
    assert!(matches!(alone(mixed), Err(NoBudgets::TooSlow { .. })));
}

#[test]
fn invalid_samples_fail_the_attempt() {
    let good = container(170.0);
    let mut cases = Vec::new();
    for field in 0..7 {
        for bad in [0.0, f64::NAN, f64::INFINITY, -1.0] {
            let mut s = good;
            let slot = match field {
                0 => &mut s.wall_ms,
                1 => &mut s.prompt_n,
                2 => &mut s.prompt_ms,
                3 => &mut s.cached_n,
                4 => &mut s.predicted_n,
                5 => &mut s.predicted_ms,
                _ => &mut s.filler_tokens,
            };
            *slot = bad;
            cases.push(s);
        }
    }
    // A sample must generate exactly 64 tokens, not the owner's reply length.
    for generated in [63.0, 65.0, 192.0] {
        let mut wrong = good;
        wrong.predicted_n = generated;
        cases.push(wrong);
    }
    let mut negative_tail = good;
    negative_tail.filler_tokens = negative_tail.prompt_n + 1.0;
    cases.push(negative_tail);
    let mut uncached = good;
    // 75 % of 1,460 is 1,095.
    uncached.cached_n = 1094.0;
    cases.push(uncached);
    for case in cases {
        assert!(
            matches!(
                derive(&measured(Path::Cpu, 192, [good, case])),
                Err(NoBudgets::Invalid(_))
            ),
            "{case:?}"
        );
        assert!(matches!(
            derive(&measured(Path::Cpu, 192, [case, good])),
            Err(NoBudgets::Invalid(_))
        ));
    }
    let mut exactly = good;
    exactly.cached_n = 1095.0;
    assert!(derive(&measured(Path::Cpu, 192, [good, exactly])).is_ok());
}

#[test]
fn run_deadlines_stay_within_floor_and_ceiling() {
    for path in [Path::Cpu, Path::Gpu] {
        for p in [0.0001, 0.001, 0.01, 0.05, 0.1] {
            for g in [0.001, 0.01, 0.1, 0.2, 0.4] {
                for max_tokens in [256, 1024, 2560] {
                    let s = sample(p, g, 0.5, 200.0, 293.0, 1290.0);
                    if let Ok(derived) = derive(&measured(path, max_tokens, [s, s])) {
                        let run = derived.budgets.run.as_secs();
                        assert!((15..=RUN_CEILING_S).contains(&run), "{run}");
                        assert_eq!(run % 5, 0);
                        let total = run + derived.budgets.queue_wait.as_secs();
                        assert!(total <= 120.max(2 * run), "{total}");
                        if path == Path::Cpu {
                            assert_eq!(derived.budgets.depth, 1);
                        } else {
                            assert!((1..=4).contains(&derived.budgets.depth));
                        }
                    }
                }
            }
        }
    }
    assert_eq!(run_deadline(0.001), 15);
    assert_eq!(run_deadline(12.0), 15);
    assert_eq!(run_deadline(12.1), 20);
    assert_eq!(run_deadline(f64::INFINITY) % 5, 0);
}
