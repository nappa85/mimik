//! Assisted sheet slice 1.6: a list's optional `items.quick_add` member (`docs/assisted-entry.md`
//! "Quick add", ASSIST-3). It is omitted when absent, so the canonical bytes and hashes of every
//! list without it are unchanged; admission refuses it for lists a quick-add line could not
//! fill (a required enum, yes/no, text or multiline property other than the mapped name) and
//! when it names a property of the wrong type.
use super::{DefinitionError, admission};
use serde_json::{Value, json};

const EXAMPLE: &[u8] = include_bytes!("../../schemas/definition-formula-example.json");
/// The formula example (a list without `quick_add`) and `gear()`, captured from the pre-1.6
/// implementation (master 77a4c1e). New members must not change them.
const EXAMPLE_HASH: &str = "a8749172ce6d6b60728e0ada6850f37636b64bd21eacdc49e6d4f4830fdc29d5";
pub(super) const GEAR_HASH: &str =
    "b6650d7c99ee2b4e46c0b7eb472a5244085ab65f176477fd79e8a3d9cf8a4c2e";

pub(super) fn code(result: Result<admission::Admitted, DefinitionError>) -> String {
    match result {
        Err(DefinitionError::Invalid(report)) => {
            format!(
                "{} {}",
                report.diagnostics[0].path, report.diagnostics[0].code
            )
        }
        Err(other) => format!("other {other}"),
        Ok(_) => "admitted".into(),
    }
}

pub(super) fn admit(value: &Value) -> Result<admission::Admitted, DefinitionError> {
    admission::validate(&serde_json::to_vec(value).unwrap())
}

pub(super) fn property(id: &str, control: &str, optional: bool) -> Value {
    json!({"id": id, "label": id.to_uppercase(), "control": control, "optional": optional})
}

/// A list (field 1) of a required text `name`, a required integer `count`, a required integer
/// `weight` and optional enum, yes/no, text and multiline properties.
pub(super) fn gear() -> Value {
    let unresolved = json!({"state": "unresolved", "reason": "r"});
    let mut kind = property("kind", "enum", true);
    kind["options"] = json!([{"id": "tool", "label": "Tool"}]);
    json!({
        "contract_version": 1, "game_id": "synthetic-game", "edition_id": "first",
        "definition_id": "gear", "title": "Gear", "sources": [],
        "fields": [
            {"id": "title", "label": "Title", "control": "text", "nullable": false,
             "mapping": unresolved},
            {"id": "gear", "label": "Gear", "control": "list", "nullable": false,
             "mapping": unresolved, "items": {"properties": [
                property("name", "text", false),
                property("count", "integer", false),
                property("weight", "integer", false),
                kind,
                property("worn", "boolean", true),
                property("maker", "text", true),
                property("story", "multiline", true),
             ]}},
        ],
        "formula_profile": 1,
        "groups": [{"id": "main", "label": "Main", "fields": ["title", "gear"],
                    "column_hint": "main"}],
    })
}

fn quick(mut definition: Value, member: Value) -> Value {
    definition["fields"][1]["items"]["quick_add"] = member;
    definition
}

#[test]
fn lists_without_quick_add_keep_their_canonical_bytes_and_hash() {
    let example = admission::validate(EXAMPLE).unwrap();
    assert_eq!(example.hash, EXAMPLE_HASH);
    let gear = admit(&gear()).unwrap();
    assert_eq!(gear.hash, GEAR_HASH);
    for admitted in [example, gear] {
        assert!(!admitted.content.contains("quick_add"));
        let again = admission::validate(admitted.content.as_bytes()).unwrap();
        assert_eq!(again.hash, admitted.hash);
    }
}

#[test]
fn quick_add_is_admitted_canonical_and_part_of_the_identity() {
    let full = admit(&quick(gear(), json!({"quantity": "count", "name": "name"}))).unwrap();
    assert!(
        full.content
            .contains(r#"}],"quick_add":{"name":"name","quantity":"count"}}}"#),
        "after the properties, in member order: {}",
        full.content
    );
    assert_ne!(full.hash, GEAR_HASH);
    let again = admission::validate(full.content.as_bytes()).unwrap();
    assert_eq!(
        (again.content, again.hash),
        (full.content, full.hash.clone())
    );
    // The quantity is optional and omitted when absent.
    let named = admit(&quick(gear(), json!({"name": "name"}))).unwrap();
    assert!(named.content.contains(r#""quick_add":{"name":"name"}}"#));
    assert_ne!(named.hash, full.hash);
    // An optional mapped name or quantity is admitted (both are always filled).
    let mut optional = gear();
    optional["fields"][1]["items"]["properties"][0]["optional"] = true.into();
    optional["fields"][1]["items"]["properties"][1]["optional"] = true.into();
    assert_eq!(
        code(admit(&quick(
            optional,
            json!({"name": "name", "quantity": "count"})
        ))),
        "admitted"
    );
}

#[test]
fn quick_add_refusals() {
    let path = "/fields/1/items/quick_add";
    let set = |p: usize, key: &str, value: Value| {
        let mut definition = gear();
        definition["fields"][1]["items"]["properties"][p][key] = value;
        definition
    };
    let member = json!({"name": "name", "quantity": "count"});
    let cases: Vec<(&str, Value, Value, String)> = vec![
        (
            "a name that is no property",
            gear(),
            json!({"name": "nothing"}),
            format!("{path} quick-add-name-not-text-property"),
        ),
        (
            "an integer name",
            gear(),
            json!({"name": "count"}),
            format!("{path} quick-add-name-not-text-property"),
        ),
        (
            "a multiline name",
            gear(),
            json!({"name": "story"}),
            format!("{path} quick-add-name-not-text-property"),
        ),
        (
            "a quantity that is no property",
            gear(),
            json!({"name": "name", "quantity": "nothing"}),
            format!("{path} quick-add-quantity-not-integer-property"),
        ),
        (
            "a text quantity",
            gear(),
            json!({"name": "name", "quantity": "maker"}),
            format!("{path} quick-add-quantity-not-integer-property"),
        ),
        (
            "a required enum",
            set(3, "optional", false.into()),
            member.clone(),
            format!("{path} quick-add-required-choice-property"),
        ),
        (
            "a required yes/no",
            set(4, "optional", false.into()),
            member.clone(),
            format!("{path} quick-add-required-choice-property"),
        ),
        (
            "a required text other than the name",
            set(5, "optional", false.into()),
            member.clone(),
            format!("{path} quick-add-required-text-property"),
        ),
        (
            "a required multiline",
            set(6, "optional", false.into()),
            member.clone(),
            format!("{path} quick-add-required-text-property"),
        ),
        (
            "a null quantity",
            gear(),
            json!({"name": "name", "quantity": null}),
            format!("{path}/quantity type"),
        ),
        (
            "no name",
            gear(),
            json!({"quantity": "count"}),
            format!("{path} required"),
        ),
        (
            "an unknown member",
            gear(),
            json!({"name": "name", "count": "count"}),
            format!("{path} additionalProperties"),
        ),
    ];
    for (case, definition, member, expected) in cases {
        assert_eq!(code(admit(&quick(definition, member))), expected, "{case}");
    }
    // Without the member the same lists are admitted: only the quick-add line is refused.
    for p in 3..=6 {
        assert_eq!(code(admit(&set(p, "optional", false.into()))), "admitted");
    }
}
