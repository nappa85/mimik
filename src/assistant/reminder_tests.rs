use super::super::interpreter::stub::StubInterpreter;
use super::*;

fn request() -> ReminderRequest {
    let field = |id: &str, kind, value: Option<Value>, options: &[&str]| FieldDraft {
        item: ReminderField {
            id: id.into(),
            label: id.to_uppercase(),
            kind,
            value,
        },
        address: format!("field:{id}"),
        options: options.iter().map(|option| (*option).to_owned()).collect(),
    };
    ReminderRequest {
        decrease: ReminderDecrease {
            id: "hp".into(),
            label: "HP".into(),
            before: 80,
            after: 60,
            phrase: "-20 poison".into(),
        },
        address: "field:hp".into(),
        fields: vec![
            field("max_hp", ItemKind::Integer, Some(Value::from(100)), &[]),
            field(
                "stance",
                ItemKind::Enum,
                Some(Value::from("calm")),
                &["calm", "wary"],
            ),
            field("gold", ItemKind::Integer, None, &[]),
        ],
        rows: vec![
            RowDraft {
                row: ReminderRow {
                    id: "r1".into(),
                    list: "Equipment".into(),
                    name: "Antidote".into(),
                },
                address: "row:equipment/r1-1".into(),
            },
            RowDraft {
                row: ReminderRow {
                    id: "r2".into(),
                    list: "Equipment".into(),
                    name: "Rope".into(),
                },
                address: "row:equipment/r1-2".into(),
            },
        ],
    }
}

fn bounds(bytes: usize, tokens: usize) -> Bounds {
    Bounds { bytes, tokens }
}

/// Rows go first (the last first), then fields (the last first); the decrease never. The
/// stub counts one token per 3 bytes, so the token bound alone decides here.
#[tokio::test]
async fn rendering_drops_rows_then_fields_and_never_the_decrease() {
    let request = request();
    let stub = StubInterpreter::default();
    let full = render(&request, bounds(4096, 10_000), 300, &stub)
        .await
        .unwrap();
    assert_eq!(full.kept.rows.len(), 2);
    assert_eq!(full.kept.fields.len(), 3);
    let tokens = |dynamic: &str| dynamic.len().div_ceil(3);
    // One token short of everything: the last row goes.
    let one_row = render(
        &request,
        bounds(4096, tokens(&full.completion.dynamic) - 1),
        300,
        &stub,
    )
    .await
    .unwrap();
    assert_eq!(one_row.kept.rows.len(), 1);
    assert_eq!(one_row.kept.rows[0].row.id, "r1");
    assert_eq!(one_row.kept.fields.len(), 3);
    // Room for the decrease and two fields only: rows are gone before any field.
    let no_rows = request.kept(2, 0);
    let two = render(
        &request,
        bounds(4096, tokens(&reminder_envelope(&no_rows.envelope()))),
        300,
        &stub,
    )
    .await
    .unwrap();
    assert!(two.kept.rows.is_empty());
    assert_eq!(
        two.kept
            .fields
            .iter()
            .map(|f| f.item.id.as_str())
            .collect::<Vec<_>>(),
        ["max_hp", "stance"]
    );
    // Only the decrease fits.
    let alone = render(
        &request,
        bounds(
            4096,
            tokens(&reminder_envelope(&request.kept(0, 0).envelope())),
        ),
        300,
        &stub,
    )
    .await
    .unwrap();
    assert!(alone.kept.fields.is_empty() && alone.kept.rows.is_empty());
    // It does not fit alone: nothing is rendered.
    assert!(matches!(
        render(&request, bounds(4096, 1), 300, &stub).await,
        Err(NotRendered::TooLong)
    ));
    assert!(matches!(
        render(&request, bounds(10, 10_000), 300, &stub).await,
        Err(NotRendered::TooLong)
    ));
}

/// The read set holds every value sent, with row names by their typed address, and the
/// completion names the row IDs, the change targets and the expression references.
#[tokio::test]
async fn the_rendered_request_carries_the_read_set_and_the_per_request_ids() {
    let stub = StubInterpreter::default();
    let rendered = render(&request(), bounds(4096, 10_000), 300, &stub)
        .await
        .unwrap();
    let reads: Vec<_> = rendered
        .reads
        .iter()
        .map(|read| (read.address.as_str(), read.value.as_str()))
        .collect();
    assert_eq!(
        reads,
        [
            ("field:hp", r#"{"int":"60"}"#),
            ("field:max_hp", r#"{"int":"100"}"#),
            ("field:stance", r#"{"enum":"calm"}"#),
            ("field:gold", r#"{"unknown":true}"#),
            ("row:equipment/r1-1", "Antidote"),
            ("row:equipment/r1-2", "Rope"),
        ]
    );
    assert_eq!(rendered.completion.max_tokens, 300);
    assert_eq!(rendered.completion.ref_ids, ["hp", "max_hp", "gold"]);
    assert_eq!(
        rendered.completion.kind,
        RequestKind::Reminder {
            rows: vec!["r1".into(), "r2".into()],
            targets: vec!["hp".into(), "max_hp".into(), "stance".into(), "gold".into()],
        }
    );
    assert!(rendered.completion.dynamic.contains("Antidote"));
}

fn reading(text: &str) -> Result<Reading, Invalid> {
    read(text, &request())
}

const OPTION_HEAL: &str = r#"{"relevant":true,"row":"r1","hint":"An antidote may help","option":{"changes":[{"ref":"hp","expr":{"op":"add","l":{"ref":"hp"},"r":{"lit":20}}},{"ref":"stance","enum":"wary"}],"explanation":"restore the 20 lost"}}"#;

#[test]
fn a_valid_reply_gives_the_hint_the_row_name_and_the_bound_option() {
    let Ok(Reading::Hint { hint, row, option }) = reading(OPTION_HEAL) else {
        panic!("{:?}", reading(OPTION_HEAL))
    };
    assert_eq!(hint, "An antidote may help");
    assert_eq!(row.as_deref(), Some("Antidote"));
    let (changes, explanation) = option.expect("the option passes");
    assert_eq!(explanation, "restore the 20 lost");
    assert_eq!(changes[0].address, "field:hp");
    assert_eq!(changes[0].before, NoteValue::Int(60));
    assert_eq!(changes[0].after(), NoteValue::Int(80));
    assert_eq!(changes[0].shown(), "HP (60) + 20");
    assert_eq!(changes[1].before, NoteValue::Enum("calm".into()));
    assert_eq!(changes[1].after(), NoteValue::Enum("wary".into()));
}

#[test]
fn irrelevant_truncated_and_invalid_replies_show_nothing() {
    assert_eq!(
        reading(r#"{"relevant":false,"row":null,"hint":"x","option":null}"#),
        Ok(Reading::Nothing)
    );
    assert_eq!(reading(&OPTION_HEAL[..60]), Err(Invalid::Json));
    // An unknown row or target, an extra member, a hint over 120 characters.
    for bad in [
        r#"{"relevant":true,"row":"r9","hint":"x","option":null}"#.to_owned(),
        r#"{"relevant":true,"row":null,"hint":"x","option":{"changes":[{"ref":"nope","enum":"a"}],"explanation":"e"}}"#.to_owned(),
        r#"{"relevant":true,"row":null,"hint":"x","option":null,"more":1}"#.to_owned(),
        format!(r#"{{"relevant":true,"row":null,"hint":"{}","option":null}}"#, "h".repeat(121)),
    ] {
        assert_eq!(reading(&bad), Err(Invalid::Schema), "{bad}");
    }
}

fn option_with(changes: &str) -> String {
    format!(
        r#"{{"relevant":true,"row":null,"hint":"h","option":{{"changes":{changes},"explanation":"e"}}}}"#
    )
}

fn hint_only(text: &str) -> bool {
    matches!(
        reading(text),
        Ok(Reading::Hint { option: None, hint, .. }) if hint == "h"
    )
}

/// An option that fails a check is dropped alone: the hint stays.
#[test]
fn an_option_failing_a_check_is_dropped_and_the_hint_stays() {
    // A literal with no provenance (37), one that the phrase gives (20), before (80), after
    // (60), the difference (20) and the target's own value (100) are the 2c rule's.
    let lit = |ref_id: &str, n: i64| {
        option_with(&format!(
            r#"[{{"ref":"{ref_id}","expr":{{"op":"add","l":{{"ref":"{ref_id}"}},"r":{{"lit":{n}}}}}}}]"#
        ))
    };
    assert!(hint_only(&lit("hp", 37)));
    for n in [20, 80, 60] {
        assert!(
            matches!(
                reading(&lit("hp", n)),
                Ok(Reading::Hint {
                    option: Some(_),
                    ..
                })
            ),
            "{n}"
        );
    }
    // The change target's own value is allowed, another field's value is not.
    assert!(matches!(
        reading(&lit("max_hp", 100)),
        Ok(Reading::Hint {
            option: Some(_),
            ..
        })
    ));
    assert!(hint_only(&lit("max_hp", 99)));
    // An enum option the pin does not declare, an enum change on an integer, an expression on
    // an enum, a reference to an enum field, an unknown value reference (gold is unknown).
    assert!(hint_only(&option_with(
        r#"[{"ref":"stance","enum":"angry"}]"#
    )));
    assert!(hint_only(&option_with(
        r#"[{"ref":"max_hp","enum":"calm"}]"#
    )));
    assert!(hint_only(&option_with(
        r#"[{"ref":"stance","expr":{"lit":20}}]"#
    )));
    // A reference outside the request's integer IDs is the schema's enum: nothing is shown.
    assert_eq!(
        reading(&option_with(r#"[{"ref":"hp","expr":{"ref":"stance"}}]"#)),
        Err(Invalid::Schema)
    );
    assert!(hint_only(&option_with(
        r#"[{"ref":"hp","expr":{"ref":"gold"}}]"#
    )));
    // Two changes with one target; a change equal to the value before cannot bind.
    let twice = r#"[{"ref":"hp","expr":{"lit":20}},{"ref":"hp","expr":{"lit":80}}]"#;
    let Ok(Reading::Hint {
        option: Some((changes, _)),
        ..
    }) = reading(&option_with(twice))
    else {
        panic!("the reader leaves binding to bind_with_reads")
    };
    assert_eq!(changes.len(), 2);
}

/// A fraction word in the decrease's phrase admits 2, 3, 4 and 100 as in step 2c.
#[test]
fn a_fraction_word_admits_the_fraction_literals() {
    let mut request = request();
    request.decrease.phrase = "lost half".into();
    let text = option_with(r#"[{"ref":"hp","expr":{"op":"mul","l":{"ref":"hp"},"r":{"lit":2}}}]"#);
    assert!(matches!(
        read(&text, &request),
        Ok(Reading::Hint {
            option: Some(_),
            ..
        })
    ));
    assert!(hint_only(&text), "without the word, 2 has no provenance");
}
