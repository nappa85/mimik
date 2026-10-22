use super::super::contract::{ItemKind, TARGET_ALIAS};
use super::super::interpreter::stub::StubInterpreter;
use super::*;
use serde_json::json;

fn draft(items: usize, label: &str) -> Draft {
    Draft {
        phrase: "took 15 damage".into(),
        target: EnvelopeTarget {
            alias: TARGET_ALIAS,
            id: "hp".into(),
            label: Some(label.into()),
            value: Some(84),
        },
        target_address: "field:hp".into(),
        items: (1..=items)
            .map(|n| {
                (
                    EnvelopeItem {
                        alias: format!("f{n}"),
                        id: format!("skill_{n:02}"),
                        label: format!("Skill number {n:02}"),
                        kind: ItemKind::Integer,
                        value: json!(n),
                    },
                    Supplied {
                        address: format!("field:skill_{n:02}"),
                        integer: true,
                        value: Some(i64::try_from(n).unwrap()),
                    },
                )
            })
            .collect(),
    }
}

fn wide() -> Bounds {
    Bounds {
        bytes: 4096,
        tokens: usize::MAX,
    }
}

#[tokio::test]
async fn the_dynamic_part_stays_within_each_byte_bound_dropping_whole_items() {
    let stub = StubInterpreter::default();
    let full = draft(60, "Current HP");
    for bytes in [4096, 2048, 1024, 768, 512] {
        let rendered = render(
            &full,
            Bounds {
                bytes,
                tokens: usize::MAX,
            },
            &stub,
        )
        .await
        .unwrap();
        assert!(rendered.dynamic.len() <= bytes, "{bytes}");
        // One more item would not have fitted.
        if kept(&rendered) < full.items.len() {
            let more = envelope(&full.content(kept(&rendered) + 1, true));
            assert!(more.len() > bytes, "{bytes}");
        }
        // Kept items are whole and the dropped ones absent.
        let inner: Value = serde_json::from_str(
            &rendered.dynamic["<data>\n".len()..rendered.dynamic.len() - "\n</data>".len()],
        )
        .unwrap();
        assert_eq!(inner["context"].as_array().unwrap().len(), kept(&rendered));
        assert_eq!(rendered.checks.supplied.len(), kept(&rendered) + 1);
        assert!(rendered.dynamic.contains(r#""label":"Current HP""#));
    }
}

use serde_json::Value;

/// Context items kept: the supplied aliases besides the target.
fn kept(rendered: &Rendered) -> usize {
    rendered.checks.supplied.len() - 1
}

#[tokio::test]
async fn the_token_bound_drops_items_the_byte_bound_kept() {
    let stub = StubInterpreter::default();
    let full = draft(10, "Current HP");
    let by_bytes = render(&full, wide(), &stub).await.unwrap();
    assert_eq!(kept(&by_bytes), 10);
    // One token per byte: the token bound is the tighter one.
    *stub.token_ratio.lock().unwrap() = Some(1);
    let limit = by_bytes.dynamic.len() - 1;
    let by_tokens = render(
        &full,
        Bounds {
            bytes: 4096,
            tokens: limit,
        },
        &stub,
    )
    .await
    .unwrap();
    assert_eq!(kept(&by_tokens), 9);
    assert!(by_tokens.dynamic.len() <= limit);
    // Every tokenized text was a whole rendering within the byte bound.
    for text in stub.tokenized.lock().unwrap().iter() {
        assert!(text.len() <= 4096 && text.starts_with("<data>\n") && text.ends_with("\n</data>"));
    }
}

#[tokio::test]
async fn the_label_goes_before_the_phrase_id_or_value() {
    let stub = StubInterpreter::default();
    let long_label = "L".repeat(100);
    let full = draft(3, &long_label);
    let bare = envelope(&full.content(0, false)).len();
    let rendered = render(
        &full,
        Bounds {
            bytes: bare,
            tokens: usize::MAX,
        },
        &stub,
    )
    .await
    .unwrap();
    assert_eq!(kept(&rendered), 0);
    assert!(rendered.dynamic.contains("took 15 damage"));
    assert!(rendered.dynamic.contains(r#""id":"hp""#));
    assert!(rendered.dynamic.contains(r#""value":84"#));
    assert!(!rendered.dynamic.contains(&long_label));
}

#[tokio::test]
async fn a_phrase_and_target_over_a_bound_are_too_long_without_a_completion() {
    let stub = StubInterpreter::default();
    let mut full = draft(3, "Current HP");
    full.phrase = "x".repeat(600);
    assert_eq!(
        render(
            &full,
            Bounds {
                bytes: 512,
                tokens: usize::MAX
            },
            &stub
        )
        .await
        .unwrap_err(),
        NotRendered::TooLong
    );
    // The token bound alone can also make it too long.
    assert_eq!(
        render(
            &draft(0, "Current HP"),
            Bounds {
                bytes: 4096,
                tokens: 3
            },
            &stub
        )
        .await
        .unwrap_err(),
        NotRendered::TooLong
    );
    assert!(stub.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn aliases_are_request_local() {
    let stub = StubInterpreter::default();
    let mut first = draft(2, "Current HP");
    let mut second = draft(2, "Current HP");
    // The same field as the second item in one request and the first in another.
    second.items.remove(0);
    second.items[0].0.alias = "f1".into();
    first.items.truncate(2);
    let one = render(&first, wide(), &stub).await.unwrap();
    let two = render(&second, wide(), &stub).await.unwrap();
    assert_eq!(one.checks.supplied["f2"].address, "field:skill_02");
    assert_eq!(two.checks.supplied["f1"].address, "field:skill_02");
}
