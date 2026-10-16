//! M2: formula profile in the definition envelope, admission, canonical hash and pin values.
use super::{DefinitionError, admission, private_pin};
use serde_json::{Value, json};

/// A labelled edit of a JSON document under test.
type Mutation = Box<dyn Fn(&mut Value)>;

const EXAMPLE: &[u8] = include_bytes!("../../schemas/definition-formula-example.json");
/// Canonical bytes of a formula-free definition exercising every pre-M2 member, captured
/// from the pre-M2 implementation. New members must not change them.
const FORMULA_FREE_HASH: &str = "b8f29f75a700c85c57cfc5ba7566a095b35ec0e25e1151651ad55457c4c58737";
const FORMULA_FREE_CONTENT: &str = r#"{"contract_version":1,"game_id":"synthetic-game","edition_id":"first","definition_id":"plain","title":"Plain","sources":[{"source_id":2,"pdf_sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","roles":["core","sheet"]}],"fields":[{"id":"name","label":"Name","control":"text","nullable":true,"help":"h","options":null,"mapping":{"state":"mapped","regions":[{"source_id":2,"page_id":3,"unit_id":1},{"source_id":2,"page_id":9,"unit_id":4}]}},{"id":"size","label":"Size","control":"enum","nullable":false,"help":null,"options":[{"id":"small","label":"Small"},{"id":"big","label":"Big"}],"mapping":{"state":"unresolved","reason":"r"}},{"id":"hit-points","label":"HP","control":"integer","nullable":false,"help":null,"options":null,"mapping":{"state":"unresolved","reason":"r"}}],"groups":[{"id":"main","label":"Main","fields":["name","size","hit-points"],"column_hint":"sidebar"}]}"#;

fn example() -> Value {
    serde_json::from_slice(EXAMPLE).unwrap()
}

fn code(result: Result<admission::Admitted, DefinitionError>) -> String {
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

fn admit(value: &Value) -> Result<admission::Admitted, DefinitionError> {
    admission::validate(&serde_json::to_vec(value).unwrap())
}

fn formula_free() -> Value {
    json!({
        "contract_version": 1, "game_id": "synthetic-game", "edition_id": "first",
        "definition_id": "plain", "title": "Plain",
        "sources": [{"source_id": 2, "pdf_sha256": "a".repeat(64), "roles": ["sheet", "core"]}],
        "fields": [
            {"id": "name", "label": "Name", "control": "text", "nullable": true, "help": "h",
             "mapping": {"state": "mapped", "regions": [
                 {"source_id": 2, "page_id": 9, "unit_id": 4},
                 {"source_id": 2, "page_id": 3, "unit_id": 1}]}},
            {"id": "size", "label": "Size", "control": "enum", "nullable": false,
             "options": [{"id": "small", "label": "Small"}, {"id": "big", "label": "Big"}],
             "mapping": {"state": "unresolved", "reason": "r"}},
            {"id": "hit-points", "label": "HP", "control": "integer", "nullable": false,
             "mapping": {"state": "unresolved", "reason": "r"}}
        ],
        "groups": [{"id": "main", "label": "Main", "fields": ["name", "size", "hit-points"],
                    "column_hint": "sidebar"}]
    })
}

#[test]
fn formula_free_canonical_bytes_are_unchanged() {
    let admitted = admit(&formula_free()).unwrap();
    assert_eq!(admitted.content, FORMULA_FREE_CONTENT);
    assert_eq!(admitted.hash, FORMULA_FREE_HASH);
    for member in ["formula_profile", "constraints", "formula", "items"] {
        assert!(!admitted.content.contains(member), "{member}");
    }
    let again = admission::validate(admitted.content.as_bytes()).unwrap();
    assert_eq!(
        (again.content, again.hash),
        (admitted.content, admitted.hash)
    );
}

#[test]
fn formula_definition_admits_and_readmits_byte_identically() {
    let admitted = admission::validate(EXAMPLE).unwrap();
    let again = admission::validate(admitted.content.as_bytes()).unwrap();
    assert_eq!(again.content, admitted.content);
    assert_eq!(again.hash, admitted.hash);
    // New members follow the existing ones in canonical order.
    let groups = admitted.content.find("\"groups\"").unwrap();
    let profile = admitted.content.find("\"formula_profile\"").unwrap();
    assert!(groups < profile);
    // rule_ids are sorted and deduplicated; expression text is kept verbatim.
    assert!(admitted.content.contains(r#""rule_ids":["lr-3","lr-4"]"#));
    assert!(admitted.content.contains(r#""expr":"6 - armor""#));
    let mut spaced = example();
    spaced["fields"][4]["formula"]["expr"] = "6 -  armor".into();
    assert_ne!(admit(&spaced).unwrap().hash, admitted.hash);
}

#[test]
fn formula_profile_marker_is_required_both_ways() {
    let mut missing = example();
    missing.as_object_mut().unwrap().remove("formula_profile");
    assert_eq!(
        code(admit(&missing)),
        "/formula_profile formula-profile-required"
    );
    let mut unused = formula_free();
    unused["formula_profile"] = 1.into();
    assert_eq!(
        code(admit(&unused)),
        "/formula_profile formula-profile-without-formulas"
    );
    let mut wrong = example();
    wrong["formula_profile"] = 2.into();
    assert_eq!(code(admit(&wrong)), "/formula_profile const");
}

#[test]
fn field_ids_accept_underscores_but_other_ids_do_not() {
    let mut value = formula_free();
    value["fields"][2]["id"] = "hit_points".into();
    value["groups"][0]["fields"][2] = "hit_points".into();
    assert!(admit(&value).is_ok());
    value["groups"][0]["id"] = "main_group".into();
    assert_ne!(code(admit(&value)), "admitted");
    let mut choice = formula_free();
    choice["fields"][1]["options"][0]["id"] = "very_small".into();
    assert_ne!(code(admit(&choice)), "admitted");
}

#[test]
fn reserved_names_are_rejected_only_in_formula_definitions() {
    assert!(
        admit(&formula_free()).is_ok(),
        "a field named `size` stays valid"
    );
    let mut value = example();
    value["fields"][1]["id"] = "size".into();
    value["groups"][0]["fields"][1] = "size".into();
    assert_eq!(code(admit(&value)), "/fields/1 formula-reserved-identifier");
}

#[test]
fn formula_rejections_point_at_the_formula() {
    let mut unknown = example();
    unknown["fields"][4]["formula"]["expr"] = "6 - missing".into();
    assert_eq!(
        code(admit(&unknown)),
        "/fields/4/formula formula-unknown-field"
    );
    let mut constraint = example();
    constraint["constraints"][0]["expr"] = "vitality + 1".into();
    assert_eq!(
        code(admit(&constraint)),
        "/constraints/0 formula-type-mismatch"
    );
    let mut cycle = example();
    cycle["fields"][3]["formula"]["expr"] = "pace".into();
    assert_eq!(
        code(admit(&cycle)),
        "/fields/3/formula formula-formula-cycle"
    );
}

#[test]
fn derived_and_list_field_shapes_are_checked() {
    let cases: Vec<(Mutation, &str)> = vec![
        (
            Box::new(|v| v["fields"][4]["nullable"] = true.into()),
            "/fields/4 formula-field-must-not-be-nullable",
        ),
        (
            Box::new(|v| v["fields"][4]["control"] = "list".into()),
            "/fields/4 list-items-required",
        ),
        (
            Box::new(|v| v["fields"][2]["nullable"] = true.into()),
            "/fields/2 list-must-not-be-nullable",
        ),
        (
            Box::new(|v| {
                v["fields"][2].as_object_mut().unwrap().remove("items");
            }),
            "/fields/2 list-items-required",
        ),
        (
            Box::new(
                |v| v["fields"][1]["items"] = json!({"properties": [{"id": "a", "label": "A", "control": "text", "optional": false}]}),
            ),
            "/fields/1 items-only-supported-for-list",
        ),
        (
            Box::new(|v| v["fields"][2]["items"]["properties"][2]["id"] = "armor-value".into()),
            "/fields/2/items invalid-or-duplicate-item-property",
        ),
        (
            Box::new(|v| v["fields"][2]["items"]["properties"][2]["id"] = "kind".into()),
            "/fields/2/items invalid-or-duplicate-item-property",
        ),
        (
            Box::new(|v| v["fields"][2]["items"]["properties"][2]["id"] = "id".into()),
            "/fields/2/items invalid-or-duplicate-item-property",
        ),
        (
            Box::new(|v| {
                v["fields"][2]["items"]["properties"][0]
                    .as_object_mut()
                    .unwrap()
                    .remove("options");
            }),
            "/fields/2/items enum-options-required",
        ),
        (
            Box::new(|v| {
                v["fields"][2]["items"]["properties"][2]["options"] =
                    json!([{"id": "a", "label": "A"}]);
            }),
            "/fields/2/items options-only-supported-for-enum",
        ),
        (
            Box::new(|v| v["fields"][2]["items"]["properties"][2]["control"] = "list".into()),
            "/fields/2/items/properties/2/control enum",
        ),
        (
            Box::new(|v| v["fields"][2]["id"] = "gear-list".into()),
            "/fields/2 list-id-must-be-identifier",
        ),
    ];
    for (mutate, expected) in cases {
        let mut value = example();
        mutate(&mut value);
        if let Some(id) = value["fields"][2]["id"].as_str().map(str::to_owned) {
            value["groups"][0]["fields"][2] = id.into();
        }
        assert_eq!(code(admit(&value)), expected);
    }
}

#[test]
fn constraint_and_rule_id_rules() {
    let cases: Vec<(Mutation, &str)> = vec![
        (
            Box::new(|v| v["constraints"][0]["message"] = "  ".into()),
            "/constraints/0 constraint-message-required",
        ),
        (
            Box::new(|v| v["constraints"][0]["rule_ids"] = json!(["LR-3"])),
            "/constraints/0 invalid-rule-id",
        ),
        (
            Box::new(|v| v["fields"][4]["formula"]["rule_ids"] = json!(["lr 4"])),
            "/fields/4/formula invalid-rule-id",
        ),
        (
            Box::new(|v| v["constraints"][0]["id"] = "One".into()),
            "/constraints/0 formula-invalid-identifier",
        ),
        (
            Box::new(|v| {
                v["constraints"][0]["evidence"] = json!({"state": "unresolved", "reason": " "});
            }),
            "/constraints/0/evidence unresolved-reason-required",
        ),
    ];
    for (mutate, expected) in cases {
        let mut value = example();
        mutate(&mut value);
        assert_eq!(code(admit(&value)), expected);
    }
}

#[test]
fn formula_evidence_needs_a_core_or_supplement_source() {
    let mut value = example();
    value["sources"] = json!([{"source_id": 7, "pdf_sha256": "b".repeat(64), "roles": ["sheet"]}]);
    value["fields"][4]["formula"]["evidence"] =
        json!({"state": "mapped", "regions": [{"source_id": 7, "page_id": 1, "unit_id": 1}]});
    assert_eq!(
        code(admit(&value)),
        "/fields/4/formula/evidence evidence-needs-selected-core-or-supplement-source"
    );
    value["sources"][0]["roles"] = json!(["core"]);
    let admitted = admit(&value).unwrap();
    assert!(admitted.content.contains(r#""evidence":{"state":"mapped""#));
}

fn pin_for(value: &Value) -> private_pin::ImmutablePin {
    let admitted = admit(value).unwrap();
    private_pin::validate(private_pin::RawPin {
        id: 1,
        version: 1,
        game: "synthetic-game".into(),
        edition: "first".into(),
        content: admitted.content,
        hash: admitted.hash,
    })
    .unwrap()
}

fn sheet_values() -> Value {
    json!({
        "name": "Mira", "vitality": 3,
        "equipment": [
            {"id": "coat-1", "kind": "outer-armor", "equipped": true, "armor": 1},
            {"id": "lamp-1", "kind": "tool", "equipped": false}
        ]
    })
}

#[test]
fn pinned_list_values_are_exact_and_derived_values_are_not_inputs() {
    let pin = pin_for(&example());
    assert!(pin.validate_values(&sheet_values()).is_ok());
    let invalid: Vec<(Mutation, &str)> = vec![
        (
            Box::new(|v| v["equipment"][0]["armor"] = json!(9_223_372_036_854_775_808_u64)),
            "item integer above i64",
        ),
        (
            Box::new(|v| v["equipment"][0]["armor"] = Value::Null),
            "null inside an item",
        ),
        (
            Box::new(|v| v["equipment"][1]["id"] = "coat-1".into()),
            "duplicate item id",
        ),
        (
            Box::new(|v| v["equipment"][1]["id"] = "Lamp_1".into()),
            "item id outside the slug grammar",
        ),
        (
            Box::new(|v| {
                v["equipment"][1].as_object_mut().unwrap().remove("id");
            }),
            "missing item id",
        ),
        (
            Box::new(|v| {
                v["equipment"][1]
                    .as_object_mut()
                    .unwrap()
                    .remove("equipped");
            }),
            "missing required property",
        ),
        (
            Box::new(|v| v["equipment"][1]["colour"] = "red".into()),
            "unknown property",
        ),
        (
            Box::new(|v| v["equipment"][1]["kind"] = "lamp".into()),
            "enum outside options",
        ),
        (
            Box::new(|v| v["pace"] = 5.into()),
            "derived value submitted as input",
        ),
        (
            Box::new(|v| {
                // Unique IDs, so only the 256-item cap can reject this.
                v["equipment"] = (0..257)
                    .map(|index| json!({"id": format!("x-{index}"), "kind": "tool", "equipped": false}))
                    .collect();
            }),
            "list over 256 items",
        ),
    ];
    for (mutate, why) in invalid {
        let mut values = sheet_values();
        mutate(&mut values);
        assert!(pin.validate_values(&values).is_err(), "{why}");
    }
}

#[test]
fn pinned_values_have_an_aggregate_size_cap() {
    let mut value = example();
    let mut fields = Vec::new();
    for index in 0..60 {
        fields.push(
            json!({"id": format!("story{index}"), "label": "Story", "control": "multiline",
                           "nullable": false, "mapping": {"state": "unresolved", "reason": "r"}}),
        );
    }
    let names: Vec<Value> = fields.iter().map(|field| field["id"].clone()).collect();
    value["fields"].as_array_mut().unwrap().extend(fields);
    value["groups"][0]["fields"]
        .as_array_mut()
        .unwrap()
        .extend(names);
    let pin = pin_for(&value);
    let mut values = sheet_values();
    for index in 0..60 {
        // Each value is within its own 20,000-character cap; together they exceed 1 MiB.
        values[format!("story{index}")] = "x".repeat(20_000).into();
    }
    assert!(pin.validate_values(&values).is_err());
    for index in 0..60 {
        values[format!("story{index}")] = "x".repeat(10_000).into();
    }
    assert!(pin.validate_values(&values).is_ok());
}

#[test]
fn private_pin_rejects_formulas_without_the_profile_marker() {
    let admitted = admission::validate(EXAMPLE).unwrap();
    let stripped = admitted.content.replace(r#","formula_profile":1"#, "");
    assert_ne!(stripped, admitted.content);
    assert_eq!(
        code(admission::validate(stripped.as_bytes())),
        "/formula_profile formula-profile-required"
    );
    let raw = private_pin::RawPin {
        id: 1,
        version: 1,
        game: "synthetic-game".into(),
        edition: "first".into(),
        hash: admission::hash(stripped.as_bytes()),
        content: stripped,
    };
    assert!(private_pin::validate(raw).is_err());
}

#[test]
fn formula_definitions_project_to_formula_declarations() {
    let admitted = admission::validate(EXAMPLE).unwrap();
    let spec = super::formula_admission::formula_spec(&admitted.definition);
    let compiled = super::formula::Compiled::new(&spec).unwrap();
    assert_eq!(compiled.derived_order(), vec!["armor", "pace"]);
    let evaluation = compiled.evaluate(&sheet_values()).unwrap();
    assert_eq!(evaluation.to_json()["derived"]["pace"], json!({"value": 5}));
    assert_eq!(
        evaluation.to_json()["constraints"]["one-outer-armor"],
        json!("pass")
    );
}

#[test]
fn empty_evidence_region_lists_are_not_evidence() {
    // The envelope schema already requires one region; the domain check is the second line.
    let mut value = example();
    value["fields"][4]["formula"]["evidence"] = json!({"state": "mapped", "regions": []});
    assert_ne!(code(admit(&value)), "admitted");
    let mut definition = admission::validate(EXAMPLE).unwrap().definition;
    definition.fields[4].formula.as_mut().unwrap().evidence = super::Mapping::Mapped {
        regions: Vec::new(),
    };
    let Err(error) = super::formula_admission::check(&definition) else {
        panic!("empty evidence regions were admitted");
    };
    assert_eq!(
        code(Err(error)),
        "/fields/4/formula/evidence evidence-regions-required"
    );
    let mut definition = admission::validate(EXAMPLE).unwrap().definition;
    definition.constraints[0].evidence = super::Mapping::Mapped {
        regions: Vec::new(),
    };
    let Err(error) = super::formula_admission::check(&definition) else {
        panic!("empty evidence regions were admitted");
    };
    assert_eq!(
        code(Err(error)),
        "/constraints/0/evidence evidence-regions-required"
    );
}

#[test]
fn the_aggregate_cap_does_not_apply_retroactively_to_formula_free_pins() {
    let mut value = formula_free();
    let mut names = Vec::new();
    for index in 0..60 {
        let id = format!("story{index}");
        value["fields"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id": id, "label": "Story",
            "control": "multiline", "nullable": false,
            "mapping": {"state": "unresolved", "reason": "r"}}));
        names.push(Value::String(id));
    }
    value["groups"][0]["fields"]
        .as_array_mut()
        .unwrap()
        .extend(names);
    let pin = pin_for(&value);
    let mut values = json!({"name": null, "size": "small", "hit-points": 1});
    for index in 0..60 {
        values[format!("story{index}")] = "x".repeat(20_000).into();
    }
    assert!(serde_json::to_vec(&values).unwrap().len() > 1024 * 1024);
    assert!(pin.validate_values(&values).is_ok());
}

#[test]
fn pin_evaluates_formula_definitions_and_not_formula_free_ones() {
    let pin = pin_for(&example());
    let direct =
        super::formula::Compiled::new(&super::formula_admission::formula_spec(pin.definition()))
            .unwrap();
    let values = sheet_values();
    let pinned = pin
        .evaluate(&values)
        .expect("formula pin evaluates")
        .unwrap();
    assert_eq!(pinned, direct.evaluate(&values).unwrap());
    assert_eq!(pinned.to_json()["derived"]["pace"], json!({"value": 5}));
    let mut two = sheet_values();
    two["equipment"][1] =
        json!({"id": "coat-2", "kind": "outer-armor", "equipped": true, "armor": 2});
    assert_eq!(
        pin.evaluate(&two).unwrap().unwrap(),
        direct.evaluate(&two).unwrap()
    );
    assert_eq!(
        pin.evaluate(&json!({"name": "Mira"})).unwrap(),
        direct.evaluate(&json!({"name": "Mira"})),
        "input errors are returned, not hidden"
    );
    let plain = pin_for(&formula_free());
    assert!(plain.formulas().is_none());
    assert!(
        plain
            .evaluate(&json!({"name": null, "size": "small", "hit-points": 1}))
            .is_none()
    );
}

/// Pins now carry compiled ASTs, so they can be dropped on async or database worker threads
/// (2 MiB stacks): a maximum-depth definition must drop there without recursion overflow.
#[test]
fn pin_with_maximum_depth_formulas_drops_on_a_small_stack() {
    let mut value = example();
    let depth = super::formula::MAX_AST_DEPTH;
    value["fields"][4]["formula"]["expr"] = format!("vitality{}", " + 1".repeat(depth - 1)).into();
    let pin = pin_for(&value);
    assert!(pin.formulas().is_some());
    std::thread::Builder::new()
        .stack_size(2 * 1024 * 1024)
        .spawn(move || drop(pin))
        .unwrap()
        .join()
        .expect("no stack overflow while dropping");
}

/// Hyphenated field IDs are valid but not declared to the formula layer, which rejects every
/// undeclared key: stored values must be projected onto the declared inputs first.
#[test]
fn hyphenated_unreferenced_fields_do_not_block_evaluation() {
    let mut value = example();
    value["fields"].as_array_mut().unwrap().push(
        json!({"id": "hit-points", "label": "HP", "control": "integer",
                     "nullable": true, "mapping": {"state": "unresolved", "reason": "r"}}),
    );
    value["groups"][0]["fields"]
        .as_array_mut()
        .unwrap()
        .push("hit-points".into());
    let pin = pin_for(&value);
    let mut values = sheet_values();
    values["hit-points"] = 7.into();
    pin.validate_values(&values).unwrap();
    let evaluation = pin.evaluate(&values).unwrap().unwrap();
    assert_eq!(evaluation.to_json()["derived"]["pace"], json!({"value": 5}));
    // Keys that are not definition fields still reach the engine and are refused.
    let mut unknown = values.clone();
    unknown["bogus"] = 1.into();
    assert_eq!(pin.evaluate(&unknown).unwrap().unwrap_err().path, "/bogus");
    // A derived value submitted as input is not projected away either.
    let mut derived = values;
    derived["pace"] = 5.into();
    assert_eq!(pin.evaluate(&derived).unwrap().unwrap_err().path, "/pace");
}

/// Freezing validates up to 32 pins at once and never evaluates: write-only pins drop the
/// compiled formulas, and evaluating one fails closed instead of reading as formula-free.
#[test]
fn write_only_pins_carry_no_compiled_formulas_and_fail_closed() {
    let admitted = admit(&example()).unwrap();
    let pin = private_pin::validate_for_write(private_pin::RawPin {
        id: 1,
        version: 1,
        game: "synthetic-game".into(),
        edition: "first".into(),
        content: admitted.content,
        hash: admitted.hash,
    })
    .unwrap_or_else(|_| panic!("write-only pin rejected"));
    assert!(pin.formulas().is_none());
    pin.validate_values(&sheet_values()).unwrap();
    let refused = pin.evaluate(&sheet_values()).unwrap().unwrap_err();
    assert_eq!(refused.code, super::formula::Code::InternalError);
}
