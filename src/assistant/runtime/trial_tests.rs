use super::super::server::{Health, Structured, Timings};
use super::*;
use crate::assistant::interpreter::{Failure, Interpreter};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// A server that answers the phrases from a script.
#[derive(Default)]
struct Script {
    replies: Mutex<VecDeque<Result<Structured, Failure>>>,
    delay: Mutex<Option<Duration>>,
    /// The dynamic part, the `ref` IDs and `max_tokens` of each phrase sent.
    seen: Mutex<Vec<(String, Vec<String>, u32)>>,
}

#[async_trait::async_trait]
impl Server for Script {
    async fn health(&self) -> Health {
        Health::Ready
    }

    async fn tokens(&self, _: &str) -> Result<usize, Failure> {
        Err(Failure::Transport)
    }

    async fn plain(&self, _: &str, _: u32, _: bool) -> Result<Timings, Failure> {
        Err(Failure::Transport)
    }

    async fn structured(
        &self,
        user: &str,
        ref_ids: &[String],
        max_tokens: u32,
    ) -> Result<Structured, Failure> {
        self.seen
            .lock()
            .unwrap()
            .push((user.to_owned(), ref_ids.to_vec(), max_tokens));
        let delay = *self.delay.lock().unwrap();
        if let Some(delay) = delay {
            tokio::time::sleep(delay).await;
        }
        self.replies
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Err(Failure::Transport))
    }

    fn interpreter(&self) -> Arc<dyn Interpreter> {
        Arc::new(crate::assistant::interpreter::stub::StubInterpreter::default())
    }
}

fn structured(expression: &str, predicted_n: f64) -> Structured {
    Structured {
        text: format!(
            r#"{{"kind":"options","options":[{{"expr":{expression},"explanation":"x"}}]}}"#
        ),
        truncated: false,
        predicted_n: Some(predicted_n),
    }
}

/// A reply per phrase that reaches its expected value.
fn right_answers(predicted: [f64; 5]) -> VecDeque<Result<Structured, Failure>> {
    let t = r#"{"ref":"t"}"#;
    let op = |op: &str, n: i64| format!(r#"{{"op":"{op}","l":{t},"r":{{"lit":{n}}}}}"#);
    VecDeque::from([
        Ok(structured(&op("sub", 6), predicted[0])),
        Ok(structured(&op("add", 4), predicted[1])),
        Ok(structured(
            &format!(
                r#"{{"op":"sub","l":{t},"r":{{"op":"div","round":"floor","l":{t},"r":{{"lit":2}}}}}}"#
            ),
            predicted[2],
        )),
        Ok(structured(&op("sub", 15), predicted[3])),
        Ok(structured(&op("sub", 3), predicted[4])),
    ])
}

#[test]
fn the_phrases_are_synthetic_small_and_have_reachable_expected_values() {
    assert_eq!(PHRASES.len(), 5);
    for phrase in &PHRASES {
        let (dynamic, ref_ids, checks) = phrase.request();
        // The reply schema allows `t` and exactly the IDs the phrase sends.
        let sent: Vec<&str> = phrase.context.iter().map(|(id, _, _)| *id).collect();
        assert_eq!(ref_ids, sent, "{}", phrase.note);
        assert!(!dynamic.contains("\"value\":24"), "{}", phrase.note);
        assert!(!dynamic.contains("\"alias\":\"f"), "{}", phrase.note);
        assert!(
            checks
                .supplied
                .keys()
                .all(|name| name == "t" || sent.contains(&name.as_str()))
        );
        // Every phrase fits the smallest bound of any path.
        assert!(dynamic.len() <= 1024, "{}: {}", phrase.note, dynamic.len());
        assert!(dynamic.starts_with("<data>\n") && dynamic.ends_with("\n</data>"));
        assert_eq!(checks.phrase, phrase.note);
        assert_eq!(checks.target_value, Some(phrase.target.2));
        assert_eq!(checks.supplied.len(), 1 + phrase.context.len());
        assert_ne!(phrase.expected.len(), 0);
        // A fixed, non-player text: the target and context come from this table only.
        assert!(dynamic.contains(phrase.note));
    }
    // The arithmetic the expected values stand for.
    let expect = |n: usize| PHRASES[n].expected;
    assert_eq!(expect(0), [20 - 6]);
    assert_eq!(expect(1), [11 + 4]);
    // Half of 9 rounded down (4) and up (5), taken off 9.
    assert_eq!(expect(2), [9 - 4, 9 - 5]);
    assert_eq!(expect(3), [120 - 15]);
    assert_eq!(expect(4), [17 - 3]);
}

#[tokio::test]
async fn right_replies_pass_with_their_tokens_and_the_expected_value_offered() {
    let script = Script::default();
    *script.replies.lock().unwrap() = right_answers([60.0, 61.0, 90.0, 62.0, 63.0]);
    let progress = Mutex::new(Vec::new());
    let results = run(&script, Duration::from_secs(180), &|n| {
        progress.lock().unwrap().push(n);
    })
    .await
    .unwrap();
    assert_eq!(*progress.lock().unwrap(), [1, 2, 3, 4, 5]);
    assert_eq!(results.len(), 5);
    assert!(results.iter().all(|r| r.expected_offered), "{results:?}");
    assert_eq!(largest_predicted(&results), 90);
    // Every phrase went out at the measurement cap with its own dynamic part.
    let seen = script.seen.lock().unwrap();
    assert!(seen.iter().all(|(_, _, max_tokens)| *max_tokens == 384));
    assert_eq!(seen.len(), 5);
}

#[tokio::test]
async fn a_wrong_but_valid_value_still_passes_and_says_the_expected_one_was_not_offered() {
    let script = Script::default();
    let mut answers = right_answers([60.0; 5]);
    answers[0] = Ok(structured(r#"{"lit":6}"#, 40.0));
    *script.replies.lock().unwrap() = answers;
    let results = run(&script, Duration::from_secs(180), &|_| {})
        .await
        .unwrap();
    assert!(!results[0].expected_offered);
    assert!(results[1].expected_offered);
}

#[tokio::test]
async fn a_truncated_or_invalid_reply_or_a_failed_exchange_fails_the_trial() {
    for (bad, expected) in [
        (
            Ok(Structured {
                text: r#"{"kind":"opt"#.into(),
                truncated: true,
                predicted_n: Some(384.0),
            }),
            PhraseFailure::Truncated {
                note: "took 6 damage",
            },
        ),
        (
            Ok(Structured {
                text: r#"{"kind":"options"}"#.into(),
                truncated: false,
                predicted_n: Some(10.0),
            }),
            PhraseFailure::Invalid {
                note: "took 6 damage",
            },
        ),
        (
            Ok(Structured {
                text: "not json".into(),
                truncated: false,
                predicted_n: Some(10.0),
            }),
            PhraseFailure::Invalid {
                note: "took 6 damage",
            },
        ),
        (
            Err(Failure::Status(500)),
            PhraseFailure::Exchange {
                note: "took 6 damage",
                what: "the server answered HTTP 500".into(),
            },
        ),
        (
            Err(Failure::Transport),
            PhraseFailure::Exchange {
                note: "took 6 damage",
                what: "no well-formed answer".into(),
            },
        ),
        (
            Ok(Structured {
                text: r#"{"kind":"options","options":[]}"#.into(),
                truncated: false,
                predicted_n: None,
            }),
            PhraseFailure::Exchange {
                note: "took 6 damage",
                what: "the server did not report the generated tokens".into(),
            },
        ),
    ] {
        let script = Script::default();
        *script.replies.lock().unwrap() = VecDeque::from([bad]);
        let failure = run(&script, Duration::from_secs(180), &|_| {})
            .await
            .unwrap_err();
        assert_eq!(failure, expected);
        assert_ne!(failure.describe(), "");
        // The first failure ends the run.
        assert_eq!(script.seen.lock().unwrap().len(), 1);
    }
}

#[tokio::test(start_paused = true)]
async fn a_phrase_past_its_deadline_fails_naming_the_deadline() {
    let script = Script::default();
    *script.replies.lock().unwrap() = right_answers([60.0; 5]);
    *script.delay.lock().unwrap() = Some(Duration::from_secs(200));
    let failure = run(&script, Duration::from_secs(180), &|_| {})
        .await
        .unwrap_err();
    assert_eq!(
        failure,
        PhraseFailure::Deadline {
            note: "took 6 damage",
            seconds: 180
        }
    );
    assert!(failure.describe().contains("180 s"));
}

#[test]
fn the_largest_predicted_n_rounds_up_and_stops_at_the_cap() {
    let result = |predicted_n| PhraseResult {
        note: "x",
        predicted_n,
        seconds: 1.0,
        expected_offered: true,
    };
    assert_eq!(
        largest_predicted(&[result(12.0), result(263.0), result(40.0)]),
        263
    );
    assert_eq!(largest_predicted(&[result(262.2)]), 263);
    assert_eq!(largest_predicted(&[result(900.0)]), 384);
    assert_eq!(largest_predicted(&[]), 0);
}
