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
        Ok(self.samples.lock().unwrap().remove(0))
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
    assert_eq!(prime(&server).await, Ok(1460.0));
    let requests = server.requests.lock().unwrap();
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
}

#[tokio::test]
async fn calibration_sends_two_fillers_with_ignore_eos_and_derives_budgets() {
    let server = Scripted::default();
    *server.token_counts.lock().unwrap() = vec![293, 280];
    *server.samples.lock().unwrap() = vec![
        timings(170.0 + 293.0, 1290.0, 192.0),
        timings(170.0 + 280.0, 1290.0, 192.0),
    ];
    let calibration = calibrate(&server, Path::Cpu, 192, 1460.0).await.unwrap();
    let requests = server.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    for (variant, (user, max_tokens, ignore_eos)) in requests.iter().enumerate() {
        assert_eq!(user, &filler(1024, variant));
        assert_eq!(*max_tokens, 192);
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
    assert_eq!(derived.budgets.bounds.bytes, 512);
    assert_eq!(derived.budgets.bounds.tokens, 140);
}

#[tokio::test]
async fn a_missing_timing_or_a_short_generation_derives_no_budgets() {
    let server = Scripted::default();
    let mut missing = timings(463.0, 1290.0, 192.0);
    missing.prompt_ms = None;
    *server.samples.lock().unwrap() = vec![timings(463.0, 1290.0, 192.0), missing];
    let calibration = calibrate(&server, Path::Cpu, 192, 1460.0).await.unwrap();
    assert!(matches!(calibration.derived, Err(NoBudgets::Invalid(_))));
    let server = Scripted::default();
    *server.samples.lock().unwrap() =
        vec![timings(463.0, 1290.0, 192.0), timings(463.0, 1290.0, 100.0)];
    let calibration = calibrate(&server, Path::Cpu, 192, 1460.0).await.unwrap();
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
