use super::super::server::Health;
use super::*;
use crate::assistant::interpreter::Interpreter;
use std::sync::{Arc, Mutex};

/// A scripted server: per-token rates, a cache that remembers the last prompt, and a record
/// of every request.
#[derive(Default)]
pub(crate) struct Scripted {
    pub requests: Mutex<Vec<(String, u32, bool)>>,
    pub tokenized: Mutex<Vec<String>>,
    /// Timings answered to the samples, in order (priming gets its own).
    pub samples: Mutex<Vec<Timings>>,
    /// Tokens per filler, in order of `/tokenize` calls.
    pub token_counts: Mutex<Vec<usize>>,
    pub fail_priming: bool,
    /// How long each calibration sample takes (the paused clock).
    pub sample_delay: std::time::Duration,
}

#[async_trait::async_trait]
impl Server for Scripted {
    async fn health(&self) -> Health {
        Health::Ready
    }

    async fn tokens(&self, text: &str) -> Result<usize, Failure> {
        self.tokenized.lock().unwrap().push(text.to_owned());
        let mut counts = self.token_counts.lock().unwrap();
        Ok(if counts.is_empty() {
            text.len().div_ceil(3)
        } else {
            counts.remove(0)
        })
    }

    async fn plain(
        &self,
        user: &str,
        max_tokens: u32,
        ignore_eos: bool,
    ) -> Result<Timings, Failure> {
        self.requests
            .lock()
            .unwrap()
            .push((user.to_owned(), max_tokens, ignore_eos));
        if max_tokens == 1 {
            if self.fail_priming {
                return Err(Failure::Status(500));
            }
            return Ok(Timings {
                prompt_n: Some(1460.0),
                prompt_ms: Some(1300.0),
                cache_n: Some(0.0),
                predicted_n: Some(1.0),
                predicted_ms: Some(10.0),
            });
        }
        tokio::time::sleep(self.sample_delay).await;
        Ok(self.samples.lock().unwrap().remove(0))
    }

    async fn structured(
        &self,
        _: &str,
        _: &[String],
        _: u32,
    ) -> Result<super::super::server::Structured, Failure> {
        Err(Failure::Transport)
    }

    fn interpreter(&self) -> Arc<dyn Interpreter> {
        Arc::new(crate::assistant::interpreter::stub::StubInterpreter::default())
    }
}

#[test]
fn fillers_fill_the_cap_exactly_and_differ_from_the_first_byte() {
    for cap in [512, 768, 1024, 1500, 2048, 4096] {
        let zero = filler(cap, 0);
        let one = filler(cap, 1);
        assert_eq!(zero.len(), cap);
        assert_eq!(one.len(), cap);
        assert!(zero.is_ascii() && one.is_ascii());
        assert_ne!(zero.as_bytes()[0], one.as_bytes()[0]);
        // Context items as requests carry them.
        assert!(zero.contains(r#""type":"integer""#));
        assert!(serde_json::from_str::<serde_json::Value>(&zero).is_ok());
        // The padding stays under one item.
        assert!(zero.matches(" notes").count() < 22, "{zero}");
    }
}

fn timings(prompt_n: f64, cache_n: f64, predicted_n: f64) -> Timings {
    Timings {
        prompt_n: Some(prompt_n),
        prompt_ms: Some(prompt_n * 88.0),
        cache_n: Some(cache_n),
        predicted_n: Some(predicted_n),
        predicted_ms: Some(predicted_n * 213.0),
    }
}

#[tokio::test]
async fn priming_sends_the_minimal_envelope_with_one_token() {
    let server = Scripted::default();
    let primed = prime(&server).await.unwrap();
    // Priming's measurement is kept whole, not only the prefix's size.
    assert_eq!(primed.prefix_tokens, 1460.0);
    assert!(
        primed.prompt_n > 0.0 && primed.prompt_ms > 0.0,
        "{primed:?}"
    );
    let requests = server.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0], (priming_envelope(), 1, false));
    assert!(requests[0].0.starts_with("<data>\n"));
    let failing = Scripted {
        fail_priming: true,
        ..Scripted::default()
    };
    assert_eq!(
        prime(&failing).await,
        Err(Interrupted::Priming(Failure::Status(500)))
    );
    assert_eq!(
        Interrupted::Priming(Failure::Status(500)).describe(),
        "the priming request answered HTTP 500"
    );
    assert_eq!(
        Interrupted::Sample(Failure::Transport).describe(),
        "a calibration sample got no well-formed answer"
    );
}

#[tokio::test]
async fn calibration_sends_two_64_token_fillers_with_ignore_eos_and_derives_budgets() {
    let server = Scripted::default();
    *server.token_counts.lock().unwrap() = vec![293, 280];
    *server.samples.lock().unwrap() = vec![
        timings(170.0 + 293.0, 1290.0, 64.0),
        timings(170.0 + 280.0, 1290.0, 64.0),
    ];
    let calibration = calibrate(&server, Path::Cpu, 192, Primed::of(1460.0, 88.0), &|_| {})
        .await
        .unwrap();
    let requests = server.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    for (variant, (user, max_tokens, ignore_eos)) in requests.iter().enumerate() {
        assert_eq!(user, &filler(1024, variant));
        // The samples generate 64 tokens whatever reply length the budgets are derived for.
        assert_eq!(*max_tokens, 64);
        assert!(ignore_eos);
    }
    assert_eq!(
        *server.tokenized.lock().unwrap(),
        vec![filler(1024, 0), filler(1024, 1)]
    );
    assert_eq!(calibration.samples[0].filler_tokens, 293.0);
    assert_eq!(calibration.samples[1].filler_tokens, 280.0);
    assert!((calibration.density[0] - 1024.0 / 293.0).abs() < 1e-9);
    // N_cal is the smaller count.
    let derived = calibration.derived.unwrap();
    assert_eq!(derived.budgets.bounds.bytes, 1024);
    assert_eq!(derived.budgets.bounds.tokens, 280);
    assert_eq!(derived.budgets.max_tokens, 192);
}

#[tokio::test]
async fn the_estimate_is_g_times_the_owners_slowed_reply_length_not_the_samples_64_tokens() {
    // Hand values: g = 0.213 s, p = 0.088 s, no overhead beyond rounding. T = o + p × (t + N)
    // + g × L × (1 + 2.1e-4 × L / 2) with t = 170, N = 280, o = wall - prompt - generation.
    let derive_at = |max_tokens: u32| async move {
        let server = Scripted::default();
        *server.token_counts.lock().unwrap() = vec![293, 280];
        *server.samples.lock().unwrap() = vec![
            timings(170.0 + 293.0, 1290.0, 64.0),
            timings(170.0 + 280.0, 1290.0, 64.0),
        ];
        let calibration = calibrate(
            &server,
            Path::Cpu,
            max_tokens,
            Primed::of(1460.0, 88.0),
            &|_| {},
        )
        .await
        .unwrap();
        calibration.derived.unwrap()
    };
    let default = derive_at(1024).await;
    let long = derive_at(1536).await;
    // The second sample (t 170, N 280) is the larger: 0.088 × 450 + 0.213 × the slowed reply.
    let reply = |l: f64| l * (1.0 + 2.1e-4 * l / 2.0);
    assert!((default.estimate_s - (39.6 + 0.213 * reply(1024.0))).abs() < 1e-6);
    assert!((long.estimate_s - (39.6 + 0.213 * reply(1536.0))).abs() < 1e-6);
    assert!(long.budgets.run > default.budgets.run);
}

#[tokio::test(start_paused = true)]
async fn a_sample_over_480_s_stops_the_calibration_as_too_slow() {
    let server = Scripted {
        sample_delay: std::time::Duration::from_secs(481),
        ..Scripted::default()
    };
    *server.samples.lock().unwrap() = vec![timings(463.0, 1290.0, 64.0)];
    let began = tokio::time::Instant::now();
    assert_eq!(
        calibrate(&server, Path::Cpu, 1024, Primed::of(1460.0, 88.0), &|_| {})
            .await
            .err(),
        Some(Interrupted::SampleTimedOut { limit_s: 480 })
    );
    // The first sample alone is abandoned at its timeout; the second never starts.
    assert_eq!(began.elapsed(), std::time::Duration::from_secs(480));
    assert_eq!(server.requests.lock().unwrap().len(), 1);
    assert_eq!(
        Interrupted::SampleTimedOut { limit_s: 480 }.describe(),
        "a calibration sample took more than 480 s"
    );
    // Just under the timeout completes.
    let server = Scripted {
        sample_delay: std::time::Duration::from_secs(479),
        ..Scripted::default()
    };
    *server.samples.lock().unwrap() =
        vec![timings(463.0, 1290.0, 64.0), timings(463.0, 1290.0, 64.0)];
    assert!(
        calibrate(&server, Path::Cpu, 1024, Primed::of(1460.0, 88.0), &|_| {})
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn a_missing_timing_or_a_short_generation_derives_no_budgets() {
    let server = Scripted::default();
    let mut missing = timings(463.0, 1290.0, 64.0);
    missing.prompt_ms = None;
    *server.samples.lock().unwrap() = vec![timings(463.0, 1290.0, 64.0), missing];
    let calibration = calibrate(&server, Path::Cpu, 192, Primed::of(1460.0, 88.0), &|_| {})
        .await
        .unwrap();
    assert!(matches!(calibration.derived, Err(NoBudgets::Invalid(_))));
    let server = Scripted::default();
    *server.samples.lock().unwrap() =
        vec![timings(463.0, 1290.0, 64.0), timings(463.0, 1290.0, 100.0)];
    let calibration = calibrate(&server, Path::Cpu, 192, Primed::of(1460.0, 88.0), &|_| {})
        .await
        .unwrap();
    assert!(matches!(calibration.derived, Err(NoBudgets::Invalid(_))));
}

#[test]
fn no_filler_or_priming_envelope_carries_sheet_text() {
    // Only the shipped synthetic labels appear.
    let allowed: Vec<&str> = FIELDS.iter().map(|(_, label, _)| *label).collect();
    for variant in 0..2 {
        let text = filler(4096, variant);
        let items: Vec<serde_json::Value> = if variant == 0 {
            serde_json::from_str(&text).unwrap()
        } else {
            serde_json::from_str(&format!("[{text}]")).unwrap()
        };
        for item in items {
            let label = item["label"].as_str().unwrap();
            let base = label.split(" notes").next().unwrap();
            let base = base.strip_suffix(" no").unwrap_or(base);
            assert!(allowed.contains(&base), "{label}");
        }
    }
    assert!(priming_envelope().contains(r#""note":"took 1 damage""#));
}

#[tokio::test]
async fn calibration_reports_each_sample_before_it_runs() {
    let server = Scripted::default();
    *server.token_counts.lock().unwrap() = vec![293, 280];
    *server.samples.lock().unwrap() = vec![
        timings(170.0 + 293.0, 1290.0, 64.0),
        timings(170.0 + 280.0, 1290.0, 64.0),
    ];
    let seen = std::sync::Mutex::new(Vec::new());
    calibrate(&server, Path::Cpu, 1024, Primed::of(1460.0, 88.0), &|n| {
        seen.lock().unwrap().push(n);
    })
    .await
    .unwrap();
    assert_eq!(*seen.lock().unwrap(), vec![1, 2]);
}
