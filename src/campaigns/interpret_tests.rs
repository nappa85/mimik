use super::*;
use crate::assistant::interpreter::stub::StubInterpreter;
use crate::assistant::request::{Bounds, render};
use serde_json::json;

fn definition_json(formulas: bool) -> Value {
    let unresolved = json!({"state": "unresolved", "reason": "Synthetic fixture"});
    let field = |id: &str, control: &str, nullable: bool| {
        json!({"id": id, "label": format!("Label {id}"), "control": control, "nullable": nullable,
               "mapping": unresolved})
    };
    let property = |id: &str, control: &str, optional: bool| json!({"id": id, "label": format!("Label {id}"), "control": control, "optional": optional});
    let mut class = field("class", "enum", false);
    class["options"] =
        json!([{"id": "fighter", "label": "Fighter"}, {"id": "mage", "label": "Mage"}]);
    let mut kind = property("kind", "enum", false);
    kind["options"] = json!([{"id": "tool", "label": "Tool"}, {"id": "arms", "label": "Arms"}]);
    let mut fields = vec![
        field("hp", "integer", false),
        field("max_hp", "integer", false),
        field("armour", "integer", false),
        field("luck", "integer", true),
        class,
        field("story", "multiline", false),
        field("title", "text", false),
        field("dead", "boolean", false),
    ];
    if formulas {
        fields.push(
            json!({"id": "gear", "label": "Gear", "control": "list", "nullable": false,
            "mapping": unresolved, "items": {"properties": [
                property("name", "text", false), property("qty", "integer", false),
                property("weight", "integer", false), kind, property("note", "text", true),
                property("charges", "integer", true)]}}),
        );
        fields.push(
            json!({"id": "load", "label": "Load", "control": "integer", "nullable": false,
            "mapping": unresolved, "formula": {"expr": "sum(gear.map(i, i.qty * i.weight))",
            "rule_ids": ["s-1"], "evidence": unresolved}}),
        );
        fields.push(
            json!({"id": "spare", "label": "Spare", "control": "integer", "nullable": false,
            "mapping": unresolved, "formula": {"expr": "max_hp - load",
            "rule_ids": ["s-1"], "evidence": unresolved}}),
        );
    }
    let ids: Vec<Value> = fields.iter().map(|field| field["id"].clone()).collect();
    let mut definition = json!({
        "contract_version": 1, "game_id": "synthetic-game", "edition_id": "first",
        "definition_id": "interpret-sheet", "title": "Synthetic", "sources": [],
        "fields": fields,
        "groups": [{"id": "main", "label": "Main", "fields": ids, "column_hint": "main"}],
    });
    if formulas {
        definition["formula_profile"] = json!(1);
        definition["constraints"] = json!([
            {"id": "hp-cap", "expr": "hp <= max_hp", "rule_ids": ["s-2"], "message": "m", "evidence": unresolved},
            {"id": "arms", "expr": "gear.all(g, g.kind == \"tool\" || has(g.charges))", "rule_ids": ["s-2"], "message": "m", "evidence": unresolved},
        ]);
    }
    definition
}

fn definition(formulas: bool) -> Definition {
    serde_json::from_value(definition_json(formulas)).unwrap()
}

fn data() -> Value {
    json!({"hp": 84, "max_hp": 90, "armour": -1, "luck": null, "class": "mage", "story": "s",
    "title": "t", "dead": false,
    "gear": [
        {"id": "r1-1", "name": "Rope", "qty": 2, "weight": 3, "kind": "tool", "note": "n", "charges": 1},
        {"id": "r1-2", "name": "Sword", "qty": 1, "weight": 5, "kind": "arms"},
    ]})
}

fn field<'a>(definition: &'a Definition, id: &str) -> &'a Field {
    definition
        .fields
        .iter()
        .find(|field| field.id == id)
        .unwrap()
}

fn ids(draft: &Draft) -> Vec<(&str, &str)> {
    draft
        .items
        .iter()
        .map(|(item, _)| (item.alias.as_str(), item.id.as_str()))
        .collect()
}

fn scalar_draft(definition: &Definition, id: &str, bytes: usize) -> Draft {
    let compiled = crate::definitions::compiled_for_tests(definition);
    super::draft(
        definition,
        compiled.as_ref(),
        &data(),
        (
            Target::Field(field(definition, id)),
            &format!("field:{id}"),
            "Label",
        ),
        "took 15 damage, luck too",
        bytes,
    )
}

fn cell_draft(host: &str, property: &str, bytes: usize) -> Draft {
    let definition = definition(true);
    let compiled = crate::definitions::compiled_for_tests(&definition);
    let list = field(&definition, "gear");
    let property = super::super::values::properties(list)
        .iter()
        .find(|item| item.id == property)
        .unwrap();
    super::draft(
        &definition,
        compiled.as_ref(),
        &data(),
        (
            Target::Cell {
                list,
                host,
                property,
            },
            &format!("cell:gear/{host}/{}", property.id),
            "Label, row 1 of Gear",
        ),
        "two more",
        bytes,
    )
}

#[test]
fn with_a_large_bound_every_stored_integer_and_enum_field_is_sent_in_order() {
    let draft = scalar_draft(&definition(true), "hp", 2048);
    assert_eq!(
        ids(&draft),
        [
            ("f1", "max_hp"),
            ("f2", "armour"),
            ("f3", "luck"),
            ("f4", "class")
        ]
    );
    assert_eq!(draft.target.value, Some(84));
    assert_eq!(draft.target.id, "hp");
    assert_eq!(draft.target_address, "field:hp");
    // Enum items are context, not references; an unknown integer is null.
    assert!(!draft.items[3].1.integer);
    assert_eq!(draft.items[2].0.value, Value::Null);
    assert_eq!(draft.items[2].1.value, None);
}

#[test]
fn with_b_at_most_1024_only_fields_read_with_the_target_are_sent() {
    // hp is read with max_hp by a check; armour is only in the same group and luck is
    // named in the phrase: neither is sent.
    assert_eq!(
        ids(&scalar_draft(&definition(true), "hp", 1024)),
        [("f1", "max_hp")]
    );
    // max_hp: with hp (the check) and through spare (a derived value reading it).
    assert_eq!(
        ids(&scalar_draft(&definition(true), "max_hp", 768)),
        [("f1", "hp")]
    );
    assert_eq!(
        ids(&scalar_draft(&definition(true), "armour", 512)).len(),
        0
    );
}

#[test]
fn a_pin_without_a_formula_layer_sends_no_related_items() {
    assert_eq!(ids(&scalar_draft(&definition(false), "hp", 1024)).len(), 0);
    assert_eq!(ids(&scalar_draft(&definition(false), "hp", 4096)).len(), 4);
}

#[test]
fn a_row_cell_sends_its_co_read_cells_and_related_scalars() {
    for bytes in [4096, 1024] {
        let draft = cell_draft("r1-1", "qty", bytes);
        let cells: Vec<_> = ids(&draft)
            .into_iter()
            .filter(|(alias, _)| alias.starts_with('c'))
            .collect();
        // weight is read with qty in one iteration; kind and charges in another iteration
        // are not; text cells never are; cells of other rows never are.
        assert_eq!(cells, [("c1", "weight")], "{bytes}");
        let (_, bound) = draft
            .items
            .iter()
            .find(|(item, _)| item.alias == "c1")
            .unwrap();
        assert_eq!(bound.address, "cell:gear/r1-1/weight");
        assert_eq!(bound.value, Some(3));
        assert_eq!(draft.target.value, Some(2));
        assert_eq!(draft.target.id, "qty");
    }
    // At 1 KiB the scalars are those read with qty: spare reads load reads qty, and
    // spare reads max_hp.
    let related = cell_draft("r1-1", "qty", 1024);
    let scalars: Vec<_> = ids(&related)
        .into_iter()
        .filter(|(alias, _)| alias.starts_with('f'))
        .collect();
    assert_eq!(scalars, [("f1", "max_hp")]);
    // The second row's own values, never the first row's.
    let other = cell_draft("r1-2", "qty", 4096);
    let (_, bound) = other
        .items
        .iter()
        .find(|(item, _)| item.alias == "c1")
        .unwrap();
    assert_eq!(bound.address, "cell:gear/r1-2/weight");
    assert_eq!(bound.value, Some(5));
}

#[test]
fn a_presence_test_co_reads_and_an_absent_cell_is_not_sent() {
    // `arms` reads kind and charges (has()) in one iteration.
    let first = cell_draft("r1-1", "charges", 4096);
    let cells: Vec<_> = ids(&first)
        .into_iter()
        .filter(|(alias, _)| alias.starts_with('c'))
        .collect();
    assert_eq!(cells, [("c1", "kind")]);
    // Row 2 has no charges set: its kind target gets no cell (charges is co-read but
    // absent, and absent cells have no value to send).
    let second = cell_draft("r1-2", "kind", 4096);
    assert!(
        ids(&second)
            .into_iter()
            .all(|(alias, _)| !alias.starts_with('c'))
    );
}

#[test]
fn excluded_types_never_appear_in_the_request() {
    let text = serde_json::to_string(
        &scalar_draft(&definition(true), "hp", 4096)
            .items
            .iter()
            .map(|(item, _)| item.clone())
            .collect::<Vec<_>>(),
    )
    .unwrap();
    for excluded in ["story", "title", "dead", "load", "spare", "gear"] {
        assert!(!text.contains(&format!("\"{excluded}\"")), "{excluded}");
    }
}

#[tokio::test]
async fn related_items_count_towards_b_and_drop_from_the_end() {
    let stub = StubInterpreter::default();
    let full = scalar_draft(&definition(true), "hp", 4096);
    let all = render(
        &full,
        Bounds {
            bytes: 4096,
            tokens: usize::MAX,
        },
        &stub,
    )
    .await
    .unwrap();
    assert_eq!(all.checks.supplied.len(), 5);
    let tight = all.dynamic.len() - 1;
    let fewer = render(
        &full,
        Bounds {
            bytes: tight,
            tokens: usize::MAX,
        },
        &stub,
    )
    .await
    .unwrap();
    assert_eq!(fewer.checks.supplied.len(), 4);
    assert!(fewer.checks.supplied.contains_key("f1"));
    assert!(!fewer.checks.supplied.contains_key("f4"));
}

/// Prototype tooling, not a check: renders the field-reference experiment's requests with
/// the real builder at B = 1,024 bytes (related fields only) and writes them next to their
/// cases. Run with `cargo test --lib render_fieldref_cases -- --ignored`; set
/// `FIELDREF_DIR` to the directory holding `fieldref-source.json` (default
/// `tools/assist-eval`). Writes `fieldref-cases.json` there.
#[tokio::test]
#[ignore = "regenerates tools/assist-eval/fieldref-cases.json"]
async fn render_fieldref_cases() {
    let dir = std::env::var("FIELDREF_DIR")
        .unwrap_or_else(|_| format!("{}/tools/assist-eval", env!("CARGO_MANIFEST_DIR")));
    let source: Value =
        serde_json::from_slice(&std::fs::read(format!("{dir}/fieldref-source.json")).unwrap())
            .unwrap();
    let definition: Definition = serde_json::from_value(source["definition"].clone()).unwrap();
    let compiled = crate::definitions::compiled_for_tests(&definition);
    let stub = StubInterpreter::default();
    let mut out = source.clone();
    out.as_object_mut().unwrap().remove("definition");
    out.as_object_mut().unwrap().remove("data");
    for case in out["cases"].as_array_mut().unwrap() {
        let mut data = source["data"].clone();
        for (id, value) in case["values"].as_object().into_iter().flatten() {
            data[id] = value.clone();
        }
        let id = case["target"].as_str().unwrap().to_owned();
        let draft = super::draft(
            &definition,
            compiled.as_ref(),
            &data,
            (
                Target::Field(field(&definition, &id)),
                &format!("field:{id}"),
                &field(&definition, &id).label,
            ),
            case["phrase"].as_str().unwrap(),
            RELATED_ONLY_BYTES,
        );
        let rendered = render(
            &draft,
            Bounds {
                bytes: RELATED_ONLY_BYTES,
                tokens: usize::MAX,
            },
            &stub,
        )
        .await
        .unwrap();
        let supplied: serde_json::Map<String, Value> = rendered
            .checks
            .supplied
            .iter()
            .filter(|(_, bound)| bound.integer)
            .map(|(alias, bound)| (alias.clone(), json!(bound.value)))
            .collect();
        let excluded: Vec<&String> = rendered
            .checks
            .supplied
            .iter()
            .filter(|(_, bound)| !bound.integer)
            .map(|(alias, _)| alias)
            .collect();
        case["rendered"] = json!({"user": rendered.dynamic, "supplied": supplied,
            "excluded": excluded, "target_value": rendered.checks.target_value});
    }
    std::fs::write(
        format!("{dir}/fieldref-cases.json"),
        serde_json::to_string_pretty(&out).unwrap() + "\n",
    )
    .unwrap();
}
