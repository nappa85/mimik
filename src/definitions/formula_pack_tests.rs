//! M3: the Lantern Reach pack through envelope → admission → pin → stored-value validation →
//! `ImmutablePin::evaluate`. The conformance fixture already covers every character at engine
//! level; this covers the definition and pin path the fixture cannot reach.
use super::{formula, private_pin};
use serde_json::{Map, Value, json};
use std::time::Instant;

pub(crate) const DEFINITION: &[u8] =
    include_bytes!("../../examples/tool-loop-demo/definition.json");
pub(crate) const CHARACTER: &[u8] =
    include_bytes!("../../examples/tool-loop-demo/characters/bram-holt.json");

fn unresolved() -> Value {
    json!({"state": "unresolved", "reason": "Synthetic pack"})
}

/// Pack `type` → Mimik `control`; enum values become options labelled by their ID.
fn control(item: &Value, out: &mut Map<String, Value>) {
    let ty = item["type"].as_str().unwrap();
    out.insert(
        "control".into(),
        Value::from(if ty == "enum" { "enum" } else { ty }),
    );
    if let Some(values) = item["values"].as_array() {
        out.insert(
            "options".into(),
            values
                .iter()
                .map(|value| json!({"id": value, "label": value}))
                .collect(),
        );
    }
}

/// Minimal test-only translation of the pack format into a Mimik definition envelope. The
/// pack's `encoding`, `rules_revision` and `formula_language` have no envelope member
/// (`deny_unknown_fields`) and are dropped; rule IDs are lowercased; every field sits in one
/// group. Expectations come from the stored characters, never from engine output.
pub(crate) fn envelope(pack: &Value) -> Value {
    let mut fields = Vec::new();
    for item in pack["fields"].as_array().unwrap() {
        let mut field = Map::new();
        field.insert("id".into(), item["name"].clone());
        field.insert("label".into(), item["name"].clone());
        control(item, &mut field);
        field.insert("nullable".into(), false.into());
        field.insert("mapping".into(), unresolved());
        if let Some(expr) = item.get("derived") {
            field.insert(
                "formula".into(),
                json!({"expr": expr, "rule_ids": [], "evidence": unresolved()}),
            );
        }
        fields.push(Value::Object(field));
    }
    for list in pack["collections"].as_array().unwrap() {
        let properties: Vec<Value> = list["fields"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| {
                let mut property = Map::new();
                property.insert("id".into(), item["name"].clone());
                property.insert("label".into(), item["name"].clone());
                control(item, &mut property);
                property.insert(
                    "optional".into(),
                    item.get("optional").cloned().unwrap_or(false.into()),
                );
                Value::Object(property)
            })
            .collect();
        fields.push(
            json!({"id": list["name"], "label": list["name"], "control": "list",
                           "nullable": false, "mapping": unresolved(),
                           "items": {"properties": properties}}),
        );
    }
    let ids: Vec<Value> = fields.iter().map(|field| field["id"].clone()).collect();
    let constraints: Vec<Value> = pack["constraints"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| {
            let rules: Vec<Value> = item["rule_ids"]
                .as_array()
                .unwrap()
                .iter()
                .map(|rule| rule.as_str().unwrap().to_lowercase().into())
                .collect();
            json!({"id": item["id"], "expr": item["expr"], "rule_ids": rules,
                   "message": item["message"], "evidence": unresolved()})
        })
        .collect();
    json!({
        "contract_version": 1, "game_id": pack["game_id"], "edition_id": pack["edition_id"],
        "definition_id": "lantern-reach", "title": "Lantern Reach", "sources": [],
        "fields": fields,
        "groups": [{"id": "sheet", "label": "Sheet", "fields": ids, "column_hint": "main"}],
        "formula_profile": pack["formula_profile"],
        "constraints": constraints
    })
}

struct Pack {
    pin: private_pin::ImmutablePin,
    inputs: Value,
    expected: Map<String, Value>,
}

/// The stored character, split into inputs and its stored derived values.
fn pack() -> Pack {
    let pack: Value = serde_json::from_slice(DEFINITION).unwrap();
    let started = Instant::now();
    let pin = private_pin::for_tests(&envelope(&pack));
    let compiled = started.elapsed();
    let derived: Vec<&str> = pin
        .definition()
        .fields
        .iter()
        .filter(|field| field.formula.is_some())
        .map(|field| field.id.as_str())
        .collect();
    let character: Value = serde_json::from_slice(CHARACTER).unwrap();
    let mut inputs = character["values"].as_object().unwrap().clone();
    let expected = derived
        .iter()
        .map(|id| ((*id).to_owned(), inputs.remove(*id).unwrap()))
        .collect();
    let formulas = pin.formulas().unwrap();
    println!(
        "pack timing (report-only): admission + pin + compile {compiled:?}; cost {}; \
         expression nodes {}",
        formulas.cost(),
        formulas.node_count()
    );
    Pack {
        pin,
        inputs: Value::Object(inputs),
        expected,
    }
}

fn evaluate(pack: &Pack, inputs: &Value) -> formula::Evaluation {
    pack.pin.validate_values(inputs).unwrap();
    let started = Instant::now();
    let evaluation = pack.pin.evaluate(inputs).unwrap().unwrap();
    println!(
        "pack timing (report-only): evaluate {:?}",
        started.elapsed()
    );
    evaluation
}

#[test]
fn lantern_reach_character_reproduces_stored_derived_values() {
    let pack = pack();
    assert_eq!(pack.expected.len(), 11);
    let evaluation = evaluate(&pack, &pack.inputs);
    assert_eq!(evaluation.derived.len(), 11);
    for (id, result) in &evaluation.derived {
        let value = serde_json::to_value(result.as_ref().unwrap()).unwrap();
        assert_eq!(&value, &pack.expected[id], "{id}");
    }
    assert_eq!(evaluation.constraints.len(), 21);
    for (id, outcome) in &evaluation.constraints {
        assert_eq!(*outcome, formula::Outcome::Pass, "{id}");
    }
}

#[test]
fn second_outer_armor_gives_one_outer_armor_violated() {
    let pack = pack();
    let mut inputs = pack.inputs.clone();
    let cuirass = inputs["equipment"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|item| item["id"] == "cuirass-1")
        .unwrap();
    assert_eq!(cuirass["equipped"], false);
    cuirass["equipped"] = true.into();
    let evaluation = evaluate(&pack, &inputs);
    let violated: Vec<&str> = evaluation
        .constraints
        .iter()
        .filter(|(_, outcome)| *outcome == formula::Outcome::Violated)
        .map(|(id, _)| id.as_str())
        .collect();
    assert_eq!(violated, vec!["one-outer-armor"]);
}
