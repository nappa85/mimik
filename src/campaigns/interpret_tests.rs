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

/// The IDs of the items, in the order they are sent.
fn ids(draft: &Draft) -> Vec<&str> {
    draft
        .items
        .iter()
        .map(|(item, _)| item.id.as_str())
        .collect()
}

/// The IDs of the same-row cells among the items.
fn cells(draft: &Draft) -> Vec<&str> {
    draft
        .items
        .iter()
        .filter(|(_, bound)| bound.address.starts_with("cell:"))
        .map(|(item, _)| item.id.as_str())
        .collect()
}

/// The IDs of the fields among the items.
fn scalars(draft: &Draft) -> Vec<&str> {
    draft
        .items
        .iter()
        .filter(|(_, bound)| bound.address.starts_with("field:"))
        .map(|(item, _)| item.id.as_str())
        .collect()
}

/// The supplied binding of the item with `id`.
fn bound<'a>(draft: &'a Draft, id: &str) -> &'a Supplied {
    &draft
        .items
        .iter()
        .find(|(item, _)| item.id == id)
        .unwrap()
        .1
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
    assert_eq!(ids(&draft), ["max_hp", "armour", "luck", "class"]);
    assert_eq!(draft.target.value, Some(84));
    assert_eq!(draft.target.id, "hp");
    assert_eq!(draft.target_address, "field:hp");
    // Enum items are context, not references; an unknown integer is null.
    assert!(!draft.items[3].1.integer);
    assert_eq!(draft.items[2].1.value, None);
    // The values are bound for Mimik's own use, and never part of the envelope.
    assert_eq!(draft.items[0].1.value, Some(90));
    assert!(
        !serde_json::to_string(&draft.items[0].0)
            .unwrap()
            .contains("90")
    );
}

#[test]
fn with_b_at_most_1024_only_fields_read_with_the_target_are_sent() {
    // hp is read with max_hp by a check; armour is only in the same group and luck is
    // named in the phrase: neither is sent.
    assert_eq!(
        ids(&scalar_draft(&definition(true), "hp", 1024)),
        ["max_hp"]
    );
    // max_hp: with hp (the check) and through spare (a derived value reading it).
    assert_eq!(ids(&scalar_draft(&definition(true), "max_hp", 768)), ["hp"]);
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
        // weight is read with qty in one iteration; kind and charges in another iteration
        // are not; text cells never are; cells of other rows never are.
        assert_eq!(cells(&draft), ["weight"], "{bytes}");
        let weight = bound(&draft, "weight");
        assert_eq!(weight.address, "cell:gear/r1-1/weight");
        assert_eq!(weight.value, Some(3));
        assert_eq!(draft.target.value, Some(2));
        assert_eq!(draft.target.id, "qty");
    }
    // At 1 KiB the scalars are those read with qty: spare reads load reads qty, and
    // spare reads max_hp.
    let related = cell_draft("r1-1", "qty", 1024);
    assert_eq!(scalars(&related), ["max_hp"]);
    // The second row's own values, never the first row's.
    let other = cell_draft("r1-2", "qty", 4096);
    let weight = bound(&other, "weight");
    assert_eq!(weight.address, "cell:gear/r1-2/weight");
    assert_eq!(weight.value, Some(5));
}

#[test]
fn a_presence_test_co_reads_and_an_absent_cell_is_not_sent() {
    // `arms` reads kind and charges (has()) in one iteration.
    let first = cell_draft("r1-1", "charges", 4096);
    assert_eq!(cells(&first), ["kind"]);
    // Row 2 has no charges set: its kind target gets no cell (charges is co-read but
    // absent, and absent cells have no value to send).
    let second = cell_draft("r1-2", "kind", 4096);
    assert_eq!(cells(&second), Vec::<&str>::new());
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
    assert!(fewer.checks.supplied.contains_key("max_hp"));
    assert!(!fewer.checks.supplied.contains_key("class"));
    assert_eq!(fewer.ref_ids, ["max_hp", "armour", "luck"]);
}

/// The definition of the tests with extra integer fields appended.
fn definition_with(extra: &[&str]) -> Definition {
    let mut value = definition_json(true);
    let unresolved = json!({"state": "unresolved", "reason": "Synthetic fixture"});
    for id in extra {
        value["fields"].as_array_mut().unwrap().push(
            json!({"id": id, "label": format!("Extra {id}"), "control": "integer",
                   "nullable": false, "mapping": unresolved}),
        );
        value["groups"][0]["fields"]
            .as_array_mut()
            .unwrap()
            .push(json!(id));
    }
    serde_json::from_value(value).unwrap()
}

fn cell_draft_in(definition: &Definition, property: &str, bytes: usize) -> Draft {
    let compiled = crate::definitions::compiled_for_tests(definition);
    let list = field(definition, "gear");
    let property = super::super::values::properties(list)
        .iter()
        .find(|item| item.id == property)
        .unwrap();
    super::draft(
        definition,
        compiled.as_ref(),
        &data_with(&["weight", "qty", "t", "t1", "f1", "c1", "ta"]),
        (
            Target::Cell {
                list,
                host: "r1-1",
                property,
            },
            &format!("cell:gear/r1-1/{}", property.id),
            "Label, row 1 of Gear",
        ),
        "two more",
        bytes,
    )
}

/// The test data plus values for the extra fields.
fn data_with(extra: &[&str]) -> Value {
    let mut value = data();
    for (n, id) in extra.iter().enumerate() {
        if value.get(id).is_none() {
            value[id] = json!(100 + n);
        }
    }
    value
}

#[test]
fn a_field_and_a_cell_of_the_row_sharing_an_id_are_both_left_out() {
    // A field `weight` beside the list property `weight` of the target's row: neither is
    // picked as the winner, so a reference to `weight` is an unsupplied reference.
    let definition = definition_with(&["weight"]);
    let draft = cell_draft_in(&definition, "qty", 4096);
    let sent = ids(&draft);
    assert!(!sent.contains(&"weight"), "{sent:?}");
    assert_eq!(cells(&draft), Vec::<&str>::new());
    assert!(sent.contains(&"max_hp"), "other items stay: {sent:?}");
    // Without the colliding field the cell is sent as before.
    let alone = cell_draft_in(&definition_with(&[]), "qty", 4096);
    assert_eq!(cells(&alone), ["weight"]);
}

#[tokio::test]
async fn a_colliding_id_stays_out_after_the_budget_dropped_everything_else() {
    let stub = StubInterpreter::default();
    let definition = definition_with(&["weight"]);
    let draft = cell_draft_in(&definition, "qty", 4096);
    for bytes in [4096, 1024, 400] {
        let rendered = render(
            &draft,
            Bounds {
                bytes,
                tokens: usize::MAX,
            },
            &stub,
        )
        .await
        .unwrap();
        // Whatever the budget keeps, `weight` is never one of the supplied IDs: neither the
        // field (left out as ambiguous) nor the cell (dropped by the budget, then or not).
        assert!(!rendered.ref_ids.contains(&"weight".to_owned()), "{bytes}");
        assert!(!rendered.checks.supplied.contains_key("weight"), "{bytes}");
        assert!(!rendered.dynamic.contains(r#""id":"weight""#), "{bytes}");
        // A reply that refers to it loses that option only.
        let reply = r#"{"kind":"options","options":[{"expr":{"op":"add","l":{"ref":"t"},"r":{"ref":"weight"}},"explanation":"x"},{"expr":{"op":"add","l":{"ref":"t"},"r":{"lit":2}},"explanation":"x"}]}"#;
        let crate::assistant::output::Reading::Options { shown, dropped } =
            crate::assistant::output::read(reply, &rendered.checks).unwrap()
        else {
            panic!("options")
        };
        assert_eq!(shown.len(), 1, "{bytes}");
        assert_eq!(dropped, [crate::assistant::output::Drop::UnsuppliedRef]);
    }
}

#[test]
fn an_id_equal_to_the_targets_id_or_to_t_is_left_out_and_look_alikes_stay() {
    let definition = definition_with(&["qty", "t", "t1", "f1", "c1", "ta"]);
    let draft = cell_draft_in(&definition, "qty", 4096);
    let sent = ids(&draft);
    // The target is the cell `qty`: a field of that ID, and the field `t`, cannot be named.
    assert!(!sent.contains(&"qty") && !sent.contains(&"t"), "{sent:?}");
    // `f1`, `c1`, `t1` and `ta` are ordinary IDs.
    for id in ["t1", "f1", "c1", "ta"] {
        assert!(sent.contains(&id), "{id}: {sent:?}");
    }
    // A field target: the cell `qty` is not in scope and the field `t` is still refused.
    let draft = scalar_draft(&definition, "hp", 4096);
    assert!(!ids(&draft).contains(&"t"));
    assert!(ids(&draft).contains(&"t1"));
}

#[test]
fn an_id_the_field_grammar_refuses_is_left_out() {
    let item = |id: &str| {
        (
            EnvelopeItem {
                id: id.into(),
                label: "L".into(),
                kind: ItemKind::Integer,
            },
            Supplied {
                address: format!("field:{id}"),
                integer: true,
                value: Some(1),
            },
        )
    };
    let long = "a".repeat(65);
    let kept = unambiguous(
        vec![
            item("hp-max"),
            item("Hp"),
            item("a b"),
            item(&long),
            item(""),
            item("\u{e9}"),
            item(&"a".repeat(64)),
            item("t"),
            item("target"),
        ],
        "target",
    );
    let kept: Vec<&str> = kept.iter().map(|(item, _)| item.id.as_str()).collect();
    assert_eq!(kept, ["hp-max", "a".repeat(64).as_str()]);
}

/// The experiment inputs of the field-reference screens in the format they were measured in
/// (tools/assist-eval/fieldref-cases.json; arms A to C, and D after `transform_fieldref2`):
/// context items named `f1`, `f2`... with an alias and a value, the checks keyed by alias. The
/// shipped builder no longer sends that shape (ASSIST-56); the experiments' Python arms still
/// need it, so it is rebuilt here from the builder's selection (the same items in the same
/// order) and the sheet's data.
fn legacy_rendering(rendered: &crate::assistant::request::Rendered, data: &Value) -> Value {
    let inner = &rendered.dynamic["<data>\n".len()..rendered.dynamic.len() - "\n</data>".len()];
    let mut envelope: Value = serde_json::from_str(inner).unwrap();
    let mut by_alias = std::collections::BTreeMap::new();
    by_alias.insert("t".to_owned(), rendered.checks.supplied["t"].clone());
    for (n, item) in envelope["context"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .enumerate()
    {
        let id = item["id"].as_str().unwrap().to_owned();
        let alias = format!("f{}", n + 1);
        by_alias.insert(alias.clone(), rendered.checks.supplied[&id].clone());
        *item = json!({"alias": alias, "id": id, "label": item["label"], "type": item["type"],
            "value": data.get(&id).cloned().unwrap_or(Value::Null)});
    }
    // The envelope's escaping: `<`, `>` and `&` as \u escapes.
    let json = serde_json::to_string(&envelope)
        .unwrap()
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026");
    let supplied: serde_json::Map<String, Value> = by_alias
        .iter()
        .filter(|(_, bound)| bound.integer)
        .map(|(alias, bound)| (alias.clone(), json!(bound.value)))
        .collect();
    let excluded: Vec<&String> = by_alias
        .iter()
        .filter(|(_, bound)| !bound.integer)
        .map(|(alias, _)| alias)
        .collect();
    json!({"user": format!("<data>\n{json}\n</data>"), "supplied": supplied,
        "excluded": excluded, "target_value": rendered.checks.target_value})
}

/// The field-reference cases with their rendered requests in the measured format.
async fn legacy_fieldref_cases(dir: &str) -> Value {
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
        case["rendered"] = legacy_rendering(&rendered, &data);
    }
    out
}

fn fieldref_dir() -> String {
    std::env::var("FIELDREF_DIR")
        .unwrap_or_else(|_| format!("{}/tools/assist-eval", env!("CARGO_MANIFEST_DIR")))
}

/// Prototype tooling, not a check: regenerates the field-reference experiment's requests
/// (see [`legacy_rendering`]) at B = 1,024 bytes (related fields only) next to their cases.
/// Run with `cargo test --lib render_fieldref_cases -- --ignored`; set `FIELDREF_DIR` to the
/// directory holding `fieldref-source.json` (default `tools/assist-eval`). Writes
/// `fieldref-cases.json` there.
#[tokio::test]
#[ignore = "regenerates tools/assist-eval/fieldref-cases.json"]
async fn render_fieldref_cases() {
    let dir = fieldref_dir();
    let out = legacy_fieldref_cases(&dir).await;
    std::fs::write(
        format!("{dir}/fieldref-cases.json"),
        serde_json::to_string_pretty(&out).unwrap() + "\n",
    )
    .unwrap();
}

/// The committed experiment input is what the builder still regenerates: the same items in
/// the same order with the same values, so a later change to the builder cannot silently
/// change what the Python arms were measured on.
#[tokio::test]
async fn the_measured_fieldref_input_is_still_regenerable() {
    let dir = fieldref_dir();
    let committed: Value =
        serde_json::from_slice(&std::fs::read(format!("{dir}/fieldref-cases.json")).unwrap())
            .unwrap();
    let regenerated = legacy_fieldref_cases(&dir).await;
    assert_eq!(regenerated["cases"], committed["cases"]);
}
