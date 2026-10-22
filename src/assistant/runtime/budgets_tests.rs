//! Formula fixtures (plan §2 2c-2a, "budgets"): the 2b figures are illustrations, not
//! evidence.
use super::*;

/// A sample with per-token prompt time `p` and generation time `g` (seconds), overhead `o`
/// (seconds), tail `t`, filler tokens `n`, `max_tokens` generated and `cached` tokens read.
fn sample(p: f64, g: f64, o: f64, t: f64, n: f64, max_tokens: u32, cached: f64) -> Sample {
    let prompt_n = t + n;
    let predicted_n = f64::from(max_tokens);
    let prompt_ms = p * prompt_n * 1000.0;
    let predicted_ms = g * predicted_n * 1000.0;
    Sample {
        wall_ms: prompt_ms + predicted_ms + o * 1000.0,
        prompt_n,
        prompt_ms,
        cached_n: cached,
        predicted_n,
        predicted_ms,
        filler_tokens: n,
    }
}

fn measured(path: Path, max_tokens: u32, samples: [Sample; 2]) -> Measured {
    Measured {
        path,
        max_tokens,
        prefix_tokens: 1460.0,
        samples,
    }
}

/// The 2b container figures: p about 88 ms and g about 213 ms per token at 2 threads, o
/// about 1 s, about 3.5 bytes per token (293 tokens in 1 KiB), 2b's 192 tokens.
fn container(t: f64) -> Sample {
    sample(0.088, 0.213, 1.0, t, 293.0, 192, 1290.0)
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
fn container_figures_with_the_low_tail_fit_only_512() {
    let derived = derive(&measured(Path::Cpu, 192, [container(170.0), container(170.0)])).unwrap();
    assert_eq!(budgets(&derived), (512, 146, 90, 1, 30));
    // T = 1 + 0.088 × (170 + 146) + 0.213 × 192.
    assert!((derived.estimate_s - 69.704).abs() < 1e-9);
}

#[test]
fn container_figures_with_the_high_tail_are_too_slow() {
    assert_eq!(
        derive(&measured(
            Path::Cpu,
            192,
            [container(365.0), container(365.0)]
        )),
        Err(NoBudgets::TooSlow {
            needed_s: 110,
            cap_s: 90
        })
    );
}

#[test]
fn gpu_figures_give_the_largest_bound() {
    // About 1,100 prompt and 80 generated tokens per second on the RX 6650 XT.
    let gpu = sample(1.0 / 1100.0, 1.0 / 80.0, 0.3, 250.0, 1170.0, 192, 1290.0);
    let derived = derive(&measured(Path::Gpu, 192, [gpu, gpu])).unwrap();
    assert_eq!(budgets(&derived), (4096, 1170, 15, 4, 60));
}

#[test]
fn a_gpu_measurement_that_fits_only_2048_picks_it_and_one_that_fits_none_has_no_budgets() {
    let slow = sample(0.025, 0.1, 0.3, 250.0, 1170.0, 192, 1290.0);
    let derived = derive(&measured(Path::Gpu, 192, [slow, slow])).unwrap();
    assert_eq!(budgets(&derived), (2048, 585, 55, 1, 55));
    let slower = sample(0.04, 0.1, 0.3, 250.0, 1170.0, 192, 1290.0);
    let derived = derive(&measured(Path::Gpu, 192, [slower, slower])).unwrap();
    assert_eq!(budgets(&derived).0, 1024);
    let slowest = sample(0.2, 0.2, 0.3, 250.0, 1170.0, 192, 1290.0);
    assert!(matches!(
        derive(&measured(Path::Gpu, 192, [slowest, slowest])),
        Err(NoBudgets::TooSlow { cap_s: 60, .. })
    ));
}

#[test]
fn a_cpu_measurement_that_fits_only_768_picks_it() {
    let s = sample(0.05, 0.15, 1.0, 100.0, 293.0, 352, 1290.0);
    let derived = derive(&measured(Path::Cpu, 352, [s, s])).unwrap();
    assert_eq!(budgets(&derived), (768, 219, 90, 1, 30));
}

#[test]
fn unequal_filler_token_counts_use_the_smaller_in_both_estimates() {
    let a = container(170.0);
    let mut b = sample(0.088, 0.213, 1.0, 170.0, 280.0, 192, 1290.0);
    // The second sample is slower in generation.
    let extra = b.predicted_ms * 0.01;
    b.predicted_ms += extra;
    b.wall_ms += extra;
    let derived = derive(&measured(Path::Cpu, 192, [a, b])).unwrap();
    assert_eq!(derived.budgets.bounds.tokens, 140);
    // Both estimates use N(512) = 140; the second is the larger.
    let t_a = 1.0 + 0.088 * (170.0 + 140.0) + 0.213 * 192.0;
    let t_b = 1.0 + 0.088 * (170.0 + 140.0) + 0.213 * 1.01 * 192.0;
    assert!(t_b > t_a);
    assert!((derived.estimate_s - t_b).abs() < 1e-9, "{}", derived.estimate_s);
}

#[test]
fn the_estimate_is_the_larger_sample_per_candidate() {
    // One sample slower in prompt, the other slower in generation: for each candidate the
    // larger of the two decides, so the pair fits no better than either alone.
    let prompt_slow = sample(0.12, 0.15, 1.0, 100.0, 293.0, 192, 1290.0);
    let generation_slow = sample(0.03, 0.30, 1.0, 100.0, 293.0, 192, 1290.0);
    let alone = |s: Sample| derive(&measured(Path::Cpu, 192, [s, s]));
    let pair = derive(&measured(Path::Cpu, 192, [prompt_slow, generation_slow])).unwrap();
    let a = alone(prompt_slow).unwrap();
    let b = alone(generation_slow).unwrap();
    assert!(pair.budgets.bounds.bytes <= a.budgets.bounds.bytes.min(b.budgets.bounds.bytes));
    // The prompt-slow sample alone fits 768, the generation-slow one 1,024, and the pair
    // 768. Mixing the worst prompt rate of one with the worst generation rate of the other
    // would fit nothing: T is the larger sample per candidate, not a worst-of-rates bound.
    assert_eq!(a.budgets.bounds.bytes, 768);
    assert_eq!(b.budgets.bounds.bytes, 1024);
    assert_eq!(pair.budgets.bounds.bytes, 768);
    let mixed = sample(0.12, 0.30, 1.0, 100.0, 293.0, 192, 1290.0);
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
    let mut short = good;
    short.predicted_n = 191.0;
    cases.push(short);
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
fn run_deadlines_stay_within_floor_and_caps() {
    for path in [Path::Cpu, Path::Gpu] {
        for p in [0.0001, 0.001, 0.01, 0.05, 0.1] {
            for g in [0.001, 0.01, 0.1, 0.2] {
                let s = sample(p, g, 0.5, 200.0, 293.0, 256, 1290.0);
                if let Ok(derived) = derive(&measured(path, 256, [s, s])) {
                    let run = derived.budgets.run.as_secs();
                    assert!((15..=path.run_cap()).contains(&run), "{run}");
                    assert_eq!(run % 5, 0);
                    let total = run + derived.budgets.queue_wait.as_secs();
                    assert!(total <= 120, "{total}");
                    if path == Path::Cpu {
                        assert_eq!(derived.budgets.depth, 1);
                    } else {
                        assert!((1..=4).contains(&derived.budgets.depth));
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
