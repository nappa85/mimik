//! Catalogue plan §3 step 3 (D8): the optional `catalogues` definition member.
use super::{DefinitionError, admission};
use serde_json::{Value, json};

fn example() -> Value {
    serde_json::from_slice(include_bytes!("../../schemas/definition-example.json")).unwrap()
}

fn admit(value: &Value) -> Result<admission::Admitted, DefinitionError> {
    admission::validate(&serde_json::to_vec(value).unwrap())
}

fn rejection(value: &Value) -> String {
    match admit(value) {
        Err(DefinitionError::Invalid(report)) => format!(
            "{} {}",
            report.diagnostics[0].path, report.diagnostics[0].code
        ),
        Err(other) => format!("other {other}"),
        Ok(_) => "admitted".into(),
    }
}

/// Frozen vectors of definitions without the member: the example envelope and the
/// formula-free envelope that exercises every pre-M2 member.
#[test]
fn frozen_definition_vectors_are_unchanged_without_catalogues() {
    let admitted = admit(&example()).unwrap();
    assert_eq!(
        admitted.hash,
        "89861020e920b01707b467c46b5b6e17bfcccf4bd0687e1f80b863ff91e1ad20"
    );
    assert!(!admitted.content.contains("catalogues"));
    let plain = admit(&super::formula_admission_tests::formula_free()).unwrap();
    assert_eq!(
        plain.content,
        super::formula_admission_tests::FORMULA_FREE_CONTENT
    );
    assert_eq!(
        plain.hash,
        super::formula_admission_tests::FORMULA_FREE_HASH
    );
}

#[test]
fn catalogue_links_are_the_last_member_and_part_of_the_identity() {
    let plain = admit(&example()).unwrap();
    let mut linked = example();
    linked["catalogues"] = json!([{"id": "spells"}, {"id": "items"}]);
    let admitted = admit(&linked).unwrap();
    assert!(
        admitted
            .content
            .ends_with(r#","catalogues":[{"id":"spells"},{"id":"items"}]}"#),
        "{}",
        admitted.content
    );
    assert_ne!(admitted.hash, plain.hash);
    // Canonical bytes re-admit to themselves.
    let again = admission::validate(admitted.content.as_bytes()).unwrap();
    assert_eq!(again.content, admitted.content);
    assert_eq!(again.hash, admitted.hash);
    // Author order is kept, as for fields: links are display order on the sheet.
    linked["catalogues"] = json!([{"id": "items"}, {"id": "spells"}]);
    assert_ne!(admit(&linked).unwrap().hash, admitted.hash);
    assert_eq!(
        admitted
            .definition
            .catalogues
            .iter()
            .map(|link| link.id.as_str())
            .collect::<Vec<_>>(),
        ["spells", "items"]
    );
}

#[test]
fn catalogue_links_are_closed_bounded_and_unique() {
    let eight: Vec<Value> = (0..8).map(|n| json!({"id": format!("c{n}")})).collect();
    let mut value = example();
    value["catalogues"] = Value::Array(eight.clone());
    assert!(admit(&value).is_ok(), "eight links are the limit");
    let mut nine = eight;
    nine.push(json!({"id": "c8"}));
    for (links, expected) in [
        (json!([]), "/catalogues minItems"),
        (
            Value::Array(nine),
            "/ malformed-json-or-duplicate-key-or-input-limit",
        ),
        (
            json!([{"id": "spells"}, {"id": "spells"}]),
            "/catalogues invalid-or-duplicate-catalogue-link",
        ),
        (
            json!([{"id": "Spells"}]),
            "/catalogues invalid-or-duplicate-catalogue-link",
        ),
        (
            json!([{"id": "1spells"}]),
            "/catalogues invalid-or-duplicate-catalogue-link",
        ),
        (
            json!([{"id": "spell_list"}]),
            "/catalogues invalid-or-duplicate-catalogue-link",
        ),
        (json!([{"id": ""}]), "/catalogues/0/id minLength"),
        (
            json!([{"id": "a".repeat(65)}]),
            "/ malformed-json-or-duplicate-key-or-input-limit",
        ),
        (
            json!([{"id": "spells", "version": 1}]),
            "/catalogues/0 additionalProperties",
        ),
        (json!([{}]), "/catalogues/0 required"),
        (json!(["spells"]), "/catalogues/0 type"),
        (json!({"id": "spells"}), "/catalogues type"),
    ] {
        value["catalogues"] = links.clone();
        assert_eq!(rejection(&value), expected, "{links}");
    }
    // A 64-character id is the limit.
    value["catalogues"] = json!([{"id": format!("a{}", "b".repeat(63))}]);
    assert!(admit(&value).is_ok());
}
