use super::super::budgets::{SAMPLE_TOKENS, Sample};
use super::super::calibration::Primed;
use super::super::server::{Health, Structured, Timings};
use super::*;
use crate::assistant::interpreter::stub::StubInterpreter;
use crate::assistant::interpreter::{Failure, Interpreter};
use std::sync::{Arc, Mutex};

/// A server that only tokenizes: the system message and the largest reply get these counts.
struct Tokenizer {
    counts: Mutex<Vec<Result<usize, Failure>>>,
    seen: Mutex<Vec<usize>>,
}

#[async_trait::async_trait]
impl Server for Tokenizer {
    async fn health(&self) -> Health {
        Health::Ready
    }
    async fn tokens(&self, text: &str) -> Result<usize, Failure> {
        self.seen.lock().unwrap().push(text.len());
        self.counts.lock().unwrap().remove(0)
    }
    async fn plain(&self, _: &str, _: u32, _: bool) -> Result<Timings, Failure> {
        unreachable!("planning sends no completion")
    }
    async fn structured(&self, _: &str, _: &[String], _: u32) -> Result<Structured, Failure> {
        unreachable!("planning sends no completion")
    }
    fn interpreter(&self) -> Arc<dyn Interpreter> {
        Arc::new(StubInterpreter::default())
    }
}

fn tokenizer(counts: Vec<Result<usize, Failure>>) -> Tokenizer {
    Tokenizer {
        counts: Mutex::new(counts),
        seen: Mutex::default(),
    }
}

fn sample(filler: f64) -> Sample {
    Sample {
        wall_ms: 88.0 * (170.0 + filler) + 213.0 * 64.0 + 1000.0,
        prompt_n: 170.0 + filler,
        prompt_ms: 88.0 * (170.0 + filler),
        cached_n: 1290.0,
        predicted_n: f64::from(SAMPLE_TOKENS),
        predicted_ms: 213.0 * 64.0,
        filler_tokens: filler,
    }
}

fn calibration(path: Path) -> Calibration {
    let measured = Measured {
        path,
        max_tokens: 1024,
        prefix_tokens: 1460.0,
        samples: [sample(293.0), sample(280.0)],
    };
    let mut derived = super::super::budgets::derive(&measured);
    // A long queue wait: these tests are about the tokenizer's figures, not about Q.
    if let Ok(derived) = &mut derived {
        derived.budgets.queue_wait = std::time::Duration::from_secs(5_000);
    }
    Calibration {
        prefix_tokens: 1460.0,
        primed: Primed::of(1460.0, 88.0),
        samples: measured.samples,
        density: [3.5, 3.6],
        derived,
    }
}

#[tokio::test]
async fn the_system_message_and_the_largest_reply_are_tokenized_and_give_budgets() {
    let server = tokenizer(vec![Ok(1800), Ok(900)]);
    let reminders = plan(
        &server,
        &calibration(Path::Cpu),
        1024,
        Path::Cpu,
        ("", 2_244_011_552),
    )
    .await;
    let Reminders::Available(budgets) = reminders else {
        panic!("{reminders:?}");
    };
    assert_eq!(budgets.reply_tokens, 900);
    assert!(budgets.restore.dev_host_constant);
    // The two texts tokenized, in this order.
    assert_eq!(
        *server.seen.lock().unwrap(),
        [REMINDER_PREFIX.len(), largest_reminder_reply().len()]
    );
}

#[tokio::test]
async fn a_failed_tokenize_makes_reminders_unavailable_not_the_generation() {
    for counts in [
        vec![Err(Failure::Status(500))],
        vec![Ok(1800), Err(Failure::Transport)],
    ] {
        let server = tokenizer(counts);
        let reminders = plan(&server, &calibration(Path::Cpu), 1024, Path::Cpu, ("", 0)).await;
        assert!(
            matches!(&reminders, Reminders::Unavailable(why) if why.contains("could not be measured")),
            "{reminders:?}"
        );
    }
}

#[tokio::test]
async fn a_deadline_over_the_ceiling_gives_the_reason_for_the_page() {
    // A reply bound of 2,560 tokens at 213 ms and the slowdown cannot fit 600 s.
    let server = tokenizer(vec![Ok(1800), Ok(2560)]);
    let reminders = plan(&server, &calibration(Path::Cpu), 1024, Path::Cpu, ("", 0)).await;
    assert!(
        matches!(&reminders, Reminders::Unavailable(why) if why.contains("600 s ceiling")),
        "{reminders:?}"
    );
}

#[tokio::test]
async fn a_generation_without_budgets_has_no_reminders() {
    let mut bare = calibration(Path::Cpu);
    bare.derived = Err(super::super::budgets::NoBudgets::ReplyTooLarge);
    let server = tokenizer(vec![]);
    let reminders = plan(&server, &bare, 1024, Path::Cpu, ("", 0)).await;
    assert!(matches!(reminders, Reminders::Unavailable(_)));
    assert!(server.seen.lock().unwrap().is_empty());
}

/// The GPU restore is sized from the shipped model's measured entry (Qwen3.5-9B: 32 KiB per token
/// and five 50.25 MiB states), recognised by its pinned SHA-256.
#[tokio::test]
async fn the_gpu_restore_is_sized_from_the_measured_entry_of_the_shipped_model() {
    use crate::assistant::model::GPU_DEFAULT_MODEL_SHA256;
    let server = tokenizer(vec![Ok(1800), Ok(900)]);
    let calibration = calibration(Path::Gpu);
    let reminders = plan(
        &server,
        &calibration,
        1024,
        Path::Gpu,
        (GPU_DEFAULT_MODEL_SHA256, 5_680_522_464),
    )
    .await;
    let Reminders::Available(budgets) = reminders else {
        panic!("{reminders:?}");
    };
    let entry = calibration.prefix_tokens * 32.0 / 1024.0 + 5.0 * 50.25;
    let expected = entry * 0.00053 + 0.010;
    assert!(
        (budgets.restore.seconds - expected).abs() < 1e-9,
        "{} against {expected}",
        budgets.restore.seconds
    );
}
