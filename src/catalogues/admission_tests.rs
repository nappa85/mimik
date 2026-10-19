//! Envelope, limits, canonical identity and redaction of catalogue admission (plan §3 step 1).
//! Every fixture is synthetic original text.
use super::{
    CatalogueError,
    admission::{self, MAX_DISTINCT_UNITS, MAX_OWNER_TEXT_TOTAL, validate},
    decode::{self, MAX_BYTES, MAX_NODES, parse_catalogue},
};
use serde_json::{Value, json};

/// Distinctive synthetic strings that must never reach a diagnostic (plan D6).
const SENTINELS: &[&str] = &[
    "zqxname",
    "zqxkey",
    "zqxowner",
    "zqxprop",
    "zqxvalue",
    "zqxlabel",
    "zqxtitle",
    "zqxmember",
];

fn example() -> Value {
    serde_json::from_slice(include_bytes!("../../schemas/catalogue-example.json")).unwrap()
}

/// The example with a sentinel in every author-chosen string member.
fn sentinel_example() -> Value {
    let mut value = example();
    value["title"] = "Title zqxtitle".into();
    value["entries"][0]["name"] = "Name zqxname".into();
    value["entries"][0]["key"] = "zqxkey-flare".into();
    value["entries"][0]["properties"]["range"] = "zqxvalue".into();
    value["entries"][1]["description"]["owner_text"] = "zqxowner text".into();
    value["entry_types"][0]["label"] = "zqxlabel".into();
    value["entry_types"][1]["properties"] =
        json!([{"id":"zqxprop","label":"Prop","control":"text"}]);
    value["entries"][1]["properties"] = json!({"zqxprop": "zqxvalue"});
    value
}

fn bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).unwrap()
}

fn assert_redacted(error: &CatalogueError) {
    let text = format!("{error} {error:?}");
    for sentinel in SENTINELS {
        assert!(
            !text.contains(sentinel),
            "diagnostic leaked {sentinel}: {text}"
        );
    }
}

/// Assert a rejection with a redacted diagnostic; returns its first code.
fn rejected(value: &Value) -> String {
    rejected_bytes(&bytes(value))
}

fn rejected_bytes(input: &[u8]) -> String {
    let Err(error) = validate(input) else {
        panic!("accepted an invalid catalogue");
    };
    assert_redacted(&error);
    match error {
        CatalogueError::Invalid(report) => report.diagnostics[0].code.clone(),
        other => panic!("unexpected error kind {other:?}"),
    }
}

fn accepted(value: &Value) -> admission::Admitted {
    match validate(&bytes(value)) {
        Ok(admitted) => admitted,
        Err(error) => panic!("rejected a valid catalogue: {error}"),
    }
}

#[test]
fn definition_decoder_constants_are_unchanged_by_the_catalogue_profile() {
    assert_eq!(
        crate::definitions::decode::MAX_NODES,
        crate::campaigns::values::FORM_LIMITS.nodes
    );
    assert_eq!(crate::definitions::decode::MAX_NODES, 32_768);
    assert_eq!(crate::definitions::MAX_BYTES, 256 * 1024);
    assert_eq!(MAX_NODES, 524_288);
    assert_eq!(MAX_BYTES, 4 * 1024 * 1024);
    // A sheet snapshot still stops at the definition node cap: 2 + k * 257 nodes.
    let sheet = |keys: usize| {
        let data: serde_json::Map<String, Value> = (0..keys)
            .map(|n| (format!("k{n}"), json!(vec![0; 256])))
            .collect();
        serde_json::to_vec(&json!({ "data": data })).unwrap()
    };
    assert!(crate::definitions::decode::parse_sheet(&sheet(127)).is_ok());
    assert!(crate::definitions::decode::parse_sheet(&sheet(128)).is_err());
}

#[test]
fn canonical_encoding_has_a_frozen_independent_hash_vector() {
    let admitted = accepted(&example());
    let canonical = concat!(
        r#"{"contract_version":1,"game_id":"synthetic-game","edition_id":"first","catalogue_id":"spells","title":"Synthetic spells","#,
        r#""sources":[{"source_id":1,"pdf_sha256":"1111111111111111111111111111111111111111111111111111111111111111"},"#,
        r#"{"source_id":3,"pdf_sha256":"3333333333333333333333333333333333333333333333333333333333333333"}],"#,
        r#""entry_types":[{"id":"spell","label":"Spell","properties":[{"id":"level","label":"Level","control":"integer"},"#,
        r#"{"id":"school","label":"School","control":"enum","options":[{"id":"light","label":"Light"},{"id":"dusk","label":"Dusk"}]},"#,
        r#"{"id":"ritual","label":"Ritual","control":"boolean"},{"id":"range","label":"Range","control":"text"}]},"#,
        r#"{"id":"charm","label":"Charm","properties":[]}],"#,
        r#""entries":[{"key":"lantern-flare","type":"spell","name":"Lantern Flare","properties":{"level":1,"school":"light","ritual":false,"range":"Near"},"#,
        r#""description":{"units":[{"source_id":3,"page_id":41,"unit_id":977},{"source_id":1,"page_id":2,"unit_id":5}]}},"#,
        r#"{"key":"house-charm","type":"charm","name":"House Charm","properties":{},"description":{"owner_text":"A small synthetic charm written for this fixture."}},"#,
        r#"{"key":"old-glow","type":"spell","name":"Old Glow","properties":{"level":0,"school":"dusk","ritual":true,"range":""},"retired":true,"#,
        r#""description":{"units":[{"source_id":1,"page_id":2,"unit_id":5}]}}]}"#
    );
    assert_eq!(admitted.content, canonical);
    // Independent of `admission::hash`: the domain prefix, a NUL, then the canonical bytes.
    let mut prefixed = b"mimik-catalogue-v1\0".to_vec();
    prefixed.extend_from_slice(canonical.as_bytes());
    assert_eq!(
        admitted.hash,
        hex::encode(<sha2::Sha256 as sha2::Digest>::digest(&prefixed))
    );
    assert_eq!(
        admitted.hash,
        "f9a3fc788bb8aa12a39aa2f1f1a6b9e760ce043cc9eda2b71c3e5c079fb17647"
    );
    // The canonical form is a fixed point.
    assert_eq!(
        accepted(&serde_json::from_str(canonical).unwrap()).content,
        canonical
    );
    assert_eq!(
        admitted.counts,
        super::CatalogueCounts {
            sources: 2,
            entry_types: 2,
            entries: 3,
            retired: 1,
            owner_written: 1,
            unit_references: 3,
            distinct_units: 2,
        }
    );
}

#[test]
fn source_order_and_property_spelling_are_normalised_but_author_order_is_identity() {
    let hash = accepted(&example()).hash;
    let mut same = example();
    same["sources"].as_array_mut().unwrap().reverse();
    same["entries"][1]
        .as_object_mut()
        .unwrap()
        .remove("retired");
    assert_eq!(accepted(&same).hash, hash);
    for change in [
        "/entries/0/description/units",
        "/entries",
        "/entry_types",
        "/entry_types/0/properties/1/options",
    ] {
        let mut value = example();
        value
            .pointer_mut(change)
            .unwrap()
            .as_array_mut()
            .unwrap()
            .reverse();
        assert_ne!(accepted(&value).hash, hash, "{change}");
    }
    // Declared property order, not author or alphabetical order, decides the encoding.
    let mut value = example();
    value["entry_types"][0]["properties"]
        .as_array_mut()
        .unwrap()
        .reverse();
    let admitted = accepted(&value);
    assert!(
        admitted
            .content
            .contains(r#""properties":{"range":"Near","ritual":false,"school":"light","level":1}"#)
    );
}

/// `key` with its first scalar spelled as a JSON `\\uXXXX` escape (an escaped-equivalent key).
fn escaped(key: &str) -> String {
    let mut chars = key.chars();
    let first = chars.next().unwrap();
    let spelled = format!(
        "{}u{:04x}{}",
        char::from(92),
        u32::from(first),
        chars.as_str()
    );
    assert!(
        spelled.starts_with(char::from(92)),
        "escape lost: {spelled}"
    );
    assert_eq!(
        serde_json::from_str::<String>(&format!("\"{spelled}\"")).unwrap(),
        key
    );
    spelled
}

#[test]
fn decoder_rejects_duplicate_and_escaped_equivalent_keys_at_every_level() {
    let compact = serde_json::to_string(&example()).unwrap();
    // (level, an existing member, its value spelling)
    for (level, key, value) in [
        ("root", "contract_version", "1"),
        ("entry type", "id", "\"charm\""),
        ("entry", "name", "\"House Charm\""),
        ("entry properties", "level", "1"),
        (
            "description",
            "owner_text",
            "\"A small synthetic charm written for this fixture.\"",
        ),
        ("unit", "unit_id", "977"),
    ] {
        let from = format!("\"{key}\":{value}");
        assert!(compact.contains(&from), "{level}: {from}");
        for spelling in [key.to_owned(), escaped(key)] {
            let to = format!("{from},\"{spelling}\":{value}");
            let code = rejected_bytes(compact.replacen(&from, &to, 1).as_bytes());
            assert_eq!(
                code, "malformed-json-or-duplicate-key-or-input-limit",
                "{level}: {spelling}"
            );
        }
    }
    for raw in [
        compact.replace(r#""level":1"#, r#""level":1.0"#),
        compact.replace(r#""level":1"#, r#""level":1e0"#),
        compact.replace(r#""level":1"#, r#""level":9223372036854775808"#),
        compact.clone() + "{}",
        String::from("[]"),
    ] {
        rejected_bytes(raw.as_bytes());
    }
}

#[test]
fn unknown_members_are_rejected_at_every_level() {
    for pointer in [
        "",
        "/sources/0",
        "/entry_types/0",
        "/entry_types/0/properties/0",
        "/entry_types/0/properties/1/options/0",
        "/entries/0",
        "/entries/0/description",
        "/entries/0/description/units/0",
    ] {
        let mut value = example();
        value
            .pointer_mut(pointer)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("zqxmember".into(), json!(true));
        let code = rejected(&value);
        assert!(
            ["additionalProperties", "oneOf"].contains(&code.as_str()),
            "{pointer}: {code}"
        );
    }
    for (member, value) in [
        ("approved", json!(true)),
        ("$ref", json!("https://outside.invalid/schema")),
        ("contract_version", json!(2)),
    ] {
        let mut changed = example();
        changed[member] = value;
        rejected(&changed);
    }
}

#[test]
fn identifiers_follow_the_registry_grammar_with_a_leading_letter() {
    let pointers = [
        "/catalogue_id",
        "/game_id",
        "/entries/0/key",
        "/entry_types/0/id",
        "/entry_types/0/properties/0/id",
        "/entry_types/0/properties/1/options/0/id",
    ];
    for bad in ["Upper", "1lead", "-lead", "under_score", "spa ce", ""] {
        for pointer in pointers {
            let mut value = example();
            *value.pointer_mut(pointer).unwrap() = bad.into();
            rejected(&value);
        }
    }
    for pointer in pointers {
        let mut value = example();
        *value.pointer_mut(pointer).unwrap() = "a".repeat(65).into();
        rejected(&value);
    }
}

#[test]
fn duplicate_ids_keys_sources_and_units_are_rejected() {
    let mut value = example();
    value["entries"][1]["key"] = "lantern-flare".into();
    assert_eq!(rejected(&value), "invalid-or-duplicate-entry-key");
    value = example();
    value["entry_types"][1]["id"] = "spell".into();
    assert_eq!(rejected(&value), "invalid-or-duplicate-type-id-or-label");
    value = example();
    value["entry_types"][0]["properties"][1]["id"] = "level".into();
    assert_eq!(
        rejected(&value),
        "invalid-or-duplicate-property-id-or-label"
    );
    value = example();
    value["entry_types"][0]["properties"][1]["options"][1]["id"] = "light".into();
    assert_eq!(rejected(&value), "invalid-or-duplicate-option-id-or-label");
    value = example();
    value["sources"][1]["source_id"] = 3.into();
    assert_eq!(
        rejected(&value),
        "duplicate-source-or-invalid-lowercase-pdf-hash"
    );
    value = example();
    value["sources"][0]["pdf_sha256"] = "A".repeat(64).into();
    assert_eq!(
        rejected(&value),
        "duplicate-source-or-invalid-lowercase-pdf-hash"
    );
    value = example();
    let unit = value["entries"][0]["description"]["units"][0].clone();
    value["entries"][0]["description"]["units"]
        .as_array_mut()
        .unwrap()
        .push(unit);
    assert_eq!(rejected(&value), "duplicate-unit-in-entry");
}

#[test]
fn entry_types_and_property_key_sets_and_controls_must_match() {
    let mut value = example();
    value["entries"][0]["type"] = "potion".into();
    assert_eq!(rejected(&value), "entry-type-not-declared");
    value = example();
    value["entries"][0]["properties"]
        .as_object_mut()
        .unwrap()
        .remove("level");
    assert_eq!(rejected(&value), "property-key-set-mismatch");
    value = example();
    value["entries"][0]["properties"]["zqxprop"] = 1.into();
    assert_eq!(rejected(&value), "property-key-set-mismatch");
    value = example();
    value["entries"][1]["type"] = "spell".into();
    assert_eq!(rejected(&value), "property-key-set-mismatch");
    for (property, wrong) in [
        ("level", json!("1")),
        ("level", json!(true)),
        ("ritual", json!(0)),
        ("ritual", json!("false")),
        ("range", json!(3)),
        ("school", json!("zqxvalue")),
        ("school", json!(1)),
    ] {
        let mut value = example();
        value["entries"][0]["properties"][property] = wrong;
        assert_eq!(
            rejected(&value),
            "property-value-control-mismatch",
            "{property}"
        );
    }
    for wrong in [json!(null), json!([1]), json!({"a": 1}), json!(1.5)] {
        let mut value = example();
        value["entries"][0]["properties"]["range"] = wrong;
        rejected(&value);
    }
    value = example();
    value["entry_types"][0]["properties"][1]
        .as_object_mut()
        .unwrap()
        .remove("options");
    assert_eq!(rejected(&value), "enum-options-required");
    value = example();
    value["entry_types"][0]["properties"][0]["options"] = json!([{"id":"a","label":"A"}]);
    assert_eq!(rejected(&value), "options-only-supported-for-enum");
    for control in ["multiline", "list", "catalogue_ref", "script"] {
        let mut value = example();
        value["entry_types"][0]["properties"][3]["control"] = control.into();
        rejected(&value);
    }
}

#[test]
fn descriptions_are_units_xor_owner_text() {
    let mut value = example();
    value["entries"][0]["description"]["owner_text"] = "zqxowner".into();
    rejected(&value);
    value = example();
    value["entries"][0]["description"] = json!({});
    rejected(&value);
    value = example();
    value["entries"][0]["description"]["units"] = json!([]);
    rejected(&value);
    value = example();
    value["entries"][1]["description"]["owner_text"] = " \n ".into();
    assert_eq!(rejected(&value), "description-needs-units-xor-owner-text");
    value = example();
    value["entries"][0]["description"]["units"][0]["source_id"] = 2.into();
    assert_eq!(rejected(&value), "unit-source-not-listed");
    for blank in ["", "   "] {
        for pointer in ["/entries/0/name", "/title", "/entry_types/0/label"] {
            let mut value = example();
            *value.pointer_mut(pointer).unwrap() = blank.into();
            rejected(&value);
        }
    }
}

fn spell(key: &str, units: &[Value]) -> Value {
    json!({"key": key, "type": "charm", "name": "Synthetic", "properties": {},
           "description": {"units": units}})
}

fn owner(key: &str, text: &str) -> Value {
    json!({"key": key, "type": "charm", "name": "Synthetic", "properties": {},
           "description": {"owner_text": text}})
}

#[test]
fn string_limits_count_scalars_and_reject_one_more() {
    // (pointer, limit); multibyte scalars prove counting is by scalar, not byte.
    for (pointer, limit) in [
        ("/entries/0/name", 100),
        ("/title", 100),
        ("/entry_types/0/label", 100),
        ("/entry_types/0/properties/0/label", 100),
        ("/entry_types/0/properties/1/options/0/label", 100),
        ("/entries/0/properties/range", 1_000),
        ("/entries/1/description/owner_text", 20_000),
    ] {
        let mut value = example();
        *value.pointer_mut(pointer).unwrap() = "é".repeat(limit).into();
        accepted(&value);
        *value.pointer_mut(pointer).unwrap() = "é".repeat(limit + 1).into();
        rejected(&value);
    }
}

fn with_array(pointer: &str, items: Vec<Value>) -> Value {
    let mut value = example();
    *value.pointer_mut(pointer).unwrap() = Value::Array(items);
    value
}

#[test]
fn array_limits_accept_the_limit_and_reject_one_more() {
    let types = |count: usize| {
        (0..count)
            .map(|n| json!({"id": format!("t{n}"), "label": "Type", "properties": []}))
            .collect::<Vec<_>>()
    };
    let mut value = with_array("/entry_types", types(32));
    value["entry_types"][0]["id"] = "charm".into();
    value["entries"] = json!([example()["entries"][1].clone()]);
    accepted(&value);
    value["entry_types"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":"extra","label":"Type","properties":[]}));
    rejected(&value);

    let properties = |count: usize| {
        (0..count)
            .map(|n| json!({"id": format!("p{n}"), "label": "Property", "control": "integer"}))
            .collect::<Vec<_>>()
    };
    let values = |count: usize| {
        (0..count)
            .map(|n| (format!("p{n}"), json!(n)))
            .collect::<serde_json::Map<_, _>>()
    };
    let mut value = with_array("/entry_types/1/properties", properties(16));
    value["entries"][1]["properties"] = Value::Object(values(16));
    accepted(&value);
    let mut value = with_array("/entry_types/1/properties", properties(17));
    value["entries"][1]["properties"] = Value::Object(values(17));
    rejected(&value);

    let options = |count: usize| {
        (0..count)
            .map(|n| json!({"id": if n == 0 { "light".into() } else { format!("o{n}") }, "label": "Option"}))
            .collect::<Vec<_>>()
    };
    let mut value = with_array("/entry_types/0/properties/1/options", options(128));
    value["entries"][2]["properties"]["school"] = "light".into();
    accepted(&value);
    let mut value = with_array("/entry_types/0/properties/1/options", options(129));
    value["entries"][2]["properties"]["school"] = "light".into();
    rejected(&value);

    let units = |count: i64| {
        (1..=count)
            .map(|n| json!({"source_id": 1, "page_id": 2, "unit_id": n}))
            .collect::<Vec<_>>()
    };
    accepted(&with_array("/entries/0/description/units", units(16)));
    rejected(&with_array("/entries/0/description/units", units(17)));

    let sources = |count: i64| {
        (1..=count)
            .map(|n| json!({"source_id": n, "pdf_sha256": "a".repeat(64)}))
            .collect::<Vec<_>>()
    };
    accepted(&with_array("/sources", sources(32)));
    rejected(&with_array("/sources", sources(33)));
}

#[test]
fn entry_count_accepts_6144_and_rejects_6145() {
    let entries = |count: usize| {
        (0..count)
            .map(|n| owner(&format!("e{n}"), "x"))
            .collect::<Vec<_>>()
    };
    let admitted = accepted(&with_array("/entries", entries(decode::MAX_ENTRIES)));
    assert_eq!(admitted.counts.entries, 6_144);
    rejected(&with_array("/entries", entries(decode::MAX_ENTRIES + 1)));
}

#[test]
fn owner_text_total_is_capped_at_one_mebibyte_of_utf8() {
    let total = |last: usize| {
        let mut entries = (0..52)
            .map(|n| owner(&format!("e{n}"), &"x".repeat(20_000)))
            .collect::<Vec<_>>();
        entries.push(owner("last", &"x".repeat(last)));
        with_array("/entries", entries)
    };
    let remainder = MAX_OWNER_TEXT_TOTAL - 52 * 20_000;
    accepted(&total(remainder));
    assert_eq!(rejected(&total(remainder + 1)), "owner-text-total-limit");
    // Bytes, not scalars: 27 entries of 20,000 two-byte scalars exceed 1 MiB.
    let entries = (0..27)
        .map(|n| owner(&format!("e{n}"), &"é".repeat(20_000)))
        .collect::<Vec<_>>();
    assert_eq!(
        rejected(&with_array("/entries", entries)),
        "owner-text-total-limit"
    );
}

#[test]
fn all_distinct_units_are_bounded_by_the_reference_cap() {
    // Every distinct unit is at least one reference, so the reference cap (6,144, plan §14)
    // bounds distinct units below the D5 cap of 16,384, which is unreachable (kept as defence
    // in depth, `distinct-unit-limit` untested by construction). All-distinct catalogues are
    // accepted up to the reference cap; past it, even at the distinct cap, the reference cap
    // refuses first.
    let entries = |total: i64| {
        (0..(total + 15) / 16)
            .map(|entry| {
                let first = entry * 16 + 1;
                let last = (first + 15).min(total);
                spell(
                    &format!("e{entry}"),
                    &(first..=last)
                        .map(|unit| json!({"source_id": 1, "page_id": 2, "unit_id": unit}))
                        .collect::<Vec<_>>(),
                )
            })
            .collect::<Vec<_>>()
    };
    let references = i64::try_from(admission::MAX_UNIT_REFERENCES).unwrap();
    assert_eq!(
        accepted(&with_array("/entries", entries(references)))
            .counts
            .distinct_units,
        admission::MAX_UNIT_REFERENCES
    );
    let limit = i64::try_from(MAX_DISTINCT_UNITS).unwrap();
    for total in [references + 1, limit, limit + 1] {
        assert_eq!(
            rejected(&with_array("/entries", entries(total))),
            "unit-reference-limit"
        );
    }
}

/// A decodable document of exactly `nodes` decoder nodes: the root, the `entries` array,
/// 100-node entries, then 1-node empty entries.
fn nodes_document(nodes: usize) -> Vec<u8> {
    // Short member names keep 524,288 nodes under the 4 MiB byte cap.
    let units: Vec<Value> = (0..16).map(|_| json!({"a": 0, "b": 0, "c": 0})).collect();
    let properties: serde_json::Map<String, Value> =
        (0..32).map(|n| (n.to_string(), json!(0))).collect();
    let full = json!({"description": {"units": units}, "properties": properties});
    let rest = nodes - 2;
    let mut entries = vec![full; rest / 100];
    entries.extend(std::iter::repeat_n(json!({}), rest % 100));
    let bytes = serde_json::to_vec(&json!({ "entries": entries })).unwrap();
    assert!(bytes.len() <= MAX_BYTES);
    bytes
}

#[test]
fn decoder_node_depth_and_byte_caps_reject_one_more() {
    assert!(parse_catalogue(&nodes_document(MAX_NODES)).is_ok());
    assert!(parse_catalogue(&nodes_document(MAX_NODES + 1)).is_err());
    // Containers at depths 0..=7 decode; one more level does not.
    let nested = |levels: usize| {
        let mut value = json!(0);
        for _ in 0..levels {
            value = json!({ "a": value });
        }
        serde_json::to_vec(&value).unwrap()
    };
    assert!(parse_catalogue(&nested(decode::MAX_DEPTH)).is_ok());
    assert!(parse_catalogue(&nested(decode::MAX_DEPTH + 1)).is_err());
    let mut padded = bytes(&example());
    padded.resize(MAX_BYTES, b' ');
    assert!(validate(&padded).is_ok());
    padded.push(b' ');
    assert_eq!(
        rejected_bytes(&padded),
        "malformed-json-or-duplicate-key-or-input-limit"
    );
}

#[test]
fn every_rejection_in_this_suite_is_redacted_including_schema_paths() {
    let base = sentinel_example();
    accepted(&base);
    let mut value = base.clone();
    value["entries"][1]["properties"]["zqxprop"] = 7.into();
    let code = rejected(&value);
    assert_eq!(code, "property-value-control-mismatch");
    let mut value = base.clone();
    value["entries"][1]["properties"]["zqxprop"] = json!([1]);
    rejected(&value);
    let mut value = base.clone();
    value["entries"][0]["zqxmember"] = json!("zqxvalue");
    rejected(&value);
    let mut value = base;
    value["entries"][1]["properties"]["zqxprop"] = "zqxvalue".repeat(200).into();
    rejected(&value);
}

/// The first diagnostic path of a rejection.
fn rejected_path(value: &Value) -> String {
    let Err(CatalogueError::Invalid(report)) = validate(&bytes(value)) else {
        panic!("expected an invalid-report rejection");
    };
    report.diagnostics[0].path.clone()
}

#[test]
fn schema_paths_redact_author_keys_by_position_not_spelling() {
    // A numeric property key and property keys spelled like envelope members, each carrying
    // a schema-invalid value: their spelling must never reach the path.
    for key in [
        "314159265358979",
        "name",
        "properties",
        "0",
        "units",
        "label",
    ] {
        let mut value = example();
        value["entries"][1]["properties"] = json!({ key: null });
        let path = rejected_path(&value);
        assert_eq!(path, "/entries/1/properties/*", "{key}");
    }
    // Numeric segments survive only at array positions, members only where they exist.
    let mut value = example();
    value["sources"][1]["source_id"] = json!(0);
    assert_eq!(rejected_path(&value), "/sources/1/source_id");
    // Units sit under the description `oneOf`, reported at the description.
    let mut value = example();
    value["entries"][0]["description"]["units"][1]["unit_id"] = json!(0);
    assert_eq!(rejected_path(&value), "/entries/0/description");
    let mut value = example();
    value["entry_types"][0]["properties"][1]["options"][1]["label"] = json!("");
    assert_eq!(
        rejected_path(&value),
        "/entry_types/0/properties/1/options/1/label"
    );
}

#[test]
fn control_characters_are_rejected_in_names_labels_and_text_values() {
    for pointer in [
        "/title",
        "/entry_types/0/label",
        "/entry_types/0/properties/0/label",
        "/entry_types/0/properties/1/options/0/label",
        "/entries/0/name",
        "/entries/0/properties/range",
    ] {
        for control in ['\u{0}', '\u{7}', '\n', '\t', '\u{1b}', '\u{85}'] {
            let mut value = example();
            *value.pointer_mut(pointer).unwrap() = format!("Text{control}more").into();
            assert_eq!(
                rejected(&value),
                "control-character-in-text",
                "{pointer} {control:?}"
            );
        }
    }
    let pointer = "/entries/1/description/owner_text";
    let mut value = example();
    *value.pointer_mut(pointer).unwrap() = "Line one\n\tLine two".into();
    accepted(&value);
    for control in ['\u{0}', '\u{7}', '\r', '\u{1b}', '\u{85}'] {
        *value.pointer_mut(pointer).unwrap() = format!("Text{control}more").into();
        assert_eq!(rejected(&value), "control-character-in-text", "{control:?}");
    }
}

/// Step 1 review item 7: invisible format characters (Cf: bidi overrides, zero-width, BOM, soft
/// hyphen) and line/paragraph separators (Zl, Zp) are refused in every displayed short text,
/// including a name made only of them. Owner text refuses Zl and Zp (normalise to LF) and
/// keeps Cf, which right-to-left prose needs.
#[test]
fn format_and_separator_characters_are_rejected_in_names_labels_and_text_values() {
    let cf = [
        '\u{200b}',
        '\u{200d}',
        '\u{202e}',
        '\u{2066}',
        '\u{feff}',
        '\u{ad}',
        '\u{e0001}',
    ];
    let separators = ['\u{2028}', '\u{2029}'];
    for pointer in [
        "/title",
        "/entry_types/0/label",
        "/entry_types/0/properties/0/label",
        "/entry_types/0/properties/1/options/0/label",
        "/entries/0/name",
        "/entries/0/properties/range",
    ] {
        for scalar in cf.iter().chain(&separators) {
            let mut value = example();
            *value.pointer_mut(pointer).unwrap() = format!("Text{scalar}more").into();
            assert_eq!(
                rejected(&value),
                "format-or-separator-character-in-text",
                "{pointer} {scalar:?}"
            );
            *value.pointer_mut(pointer).unwrap() = scalar.to_string().repeat(3).into();
            assert_eq!(
                rejected(&value),
                "format-or-separator-character-in-text",
                "{pointer} only {scalar:?}"
            );
        }
    }
    let pointer = "/entries/1/description/owner_text";
    let mut value = example();
    for scalar in cf {
        *value.pointer_mut(pointer).unwrap() = format!("Text{scalar}more").into();
        accepted(&value);
    }
    for scalar in separators {
        *value.pointer_mut(pointer).unwrap() = format!("Text{scalar}more").into();
        assert_eq!(
            rejected(&value),
            "format-or-separator-character-in-text",
            "{scalar:?}"
        );
    }
}

#[test]
fn containers_where_scalars_belong_are_rejected() {
    for pointer in [
        "/title",
        "/catalogue_id",
        "/sources/0/source_id",
        "/sources/0/pdf_sha256",
        "/entry_types/0/id",
        "/entry_types/0/properties/0/control",
        "/entry_types/0/properties/1/options/0/label",
        "/entries/0/key",
        "/entries/0/name",
        "/entries/2/retired",
        "/entries/0/properties/level",
        "/entries/1/description/owner_text",
        "/entries/0/description/units/0/unit_id",
    ] {
        for container in [json!({}), json!([]), json!({"a": 1}), json!([1])] {
            let mut value = example();
            *value.pointer_mut(pointer).unwrap() = container.clone();
            rejected(&value);
        }
    }
}

#[test]
fn redaction_walks_the_envelope_structure() {
    for (path, kept) in [
        ("", ""),
        ("/entries/3/name", "/entries/3/name"),
        ("/entries/3/properties", "/entries/3/properties"),
        ("/entries/3/properties/name", "/entries/3/properties/*"),
        ("/entries/3/properties/12345/0", "/entries/3/properties/*"),
        ("/entries/name", "/entries/*"),
        ("/entries/03", "/entries/*"),
        ("/12345", "/*"),
        ("/name", "/*"),
        ("/title/0", "/title/*"),
        ("/sources/2/units", "/sources/2/*"),
        (
            "/entry_types/1/properties/4/options/7/label",
            "/entry_types/1/properties/4/options/7/label",
        ),
        (
            "/entries/0/description/units/15/page_id",
            "/entries/0/description/units/15/page_id",
        ),
        (
            "/entries/0/description/owner_text/x",
            "/entries/0/description/owner_text/*",
        ),
    ] {
        assert_eq!(admission::redacted_path(path), kept, "{path}");
    }
}

#[test]
fn total_unit_references_are_capped_per_catalogue() {
    // 6,144 references (384 entries x 16) over 3,072 distinct units, each cited twice:
    // within the distinct cap, so only the total (counted with repetition) trips.
    let entries = |extra: usize| {
        let mut entries: Vec<Value> = (0..384_i64)
            .map(|entry| {
                spell(
                    &format!("e{entry}"),
                    &(0..16_i64)
                        .map(|k| json!({"source_id": 1, "page_id": 2, "unit_id": (entry * 16 + k) % 3_072 + 1}))
                        .collect::<Vec<_>>(),
                )
            })
            .collect();
        if extra > 0 {
            entries.push(spell(
                "extra",
                &(0..extra)
                    .map(|k| json!({"source_id": 1, "page_id": 2, "unit_id": k + 1}))
                    .collect::<Vec<_>>(),
            ));
        }
        with_array("/entries", entries)
    };
    let at_cap = accepted(&entries(0));
    assert_eq!(
        (at_cap.counts.unit_references, at_cap.counts.distinct_units),
        (admission::MAX_UNIT_REFERENCES, 3_072)
    );
    assert_eq!(admission::MAX_UNIT_REFERENCES, 6_144);
    assert_eq!(rejected(&entries(1)), "unit-reference-limit");
}

#[test]
fn blank_looking_names_labels_and_text_are_rejected() {
    // Braille pattern blank and Hangul fillers render as nothing; alone (or with spaces) they
    // make a name or label look empty. Mixed with visible text they stay allowed.
    for pointer in [
        "/title",
        "/entry_types/0/label",
        "/entry_types/0/properties/0/label",
        "/entry_types/0/properties/1/options/0/label",
        "/entries/0/name",
        "/entries/1/description/owner_text",
    ] {
        for scalar in ['\u{2800}', '\u{3164}', '\u{115f}', '\u{1160}', '\u{ffa0}'] {
            let mut value = example();
            *value.pointer_mut(pointer).unwrap() = format!(" {scalar}{scalar} ").into();
            rejected(&value);
            *value.pointer_mut(pointer).unwrap() = format!("Text{scalar}more").into();
            accepted(&value);
        }
    }
}

#[test]
fn owner_text_of_only_invisible_characters_is_rejected() {
    // Owner text keeps format characters (Cf) for bidi prose, but text made only of them,
    // whitespace and the blank-looking scalars renders as nothing.
    let pointer = "/entries/1/description/owner_text";
    for blank in [
        "\u{200b}",
        "\u{200e}\u{200f}",
        " \u{feff}\n\t\u{2800} ",
        "\u{2066}\u{2069}",
        "\u{00ad}\u{3164}",
        "\u{061c}\u{2060}",
    ] {
        let mut value = example();
        *value.pointer_mut(pointer).unwrap() = blank.into();
        assert_eq!(
            rejected(&value),
            "description-needs-units-xor-owner-text",
            "{blank:?}"
        );
    }
    for visible in [
        "Text\u{200e}more",
        "\u{2067}\u{646}\u{635}\u{2069} text",
        "\u{200b}x",
    ] {
        let mut value = example();
        *value.pointer_mut(pointer).unwrap() = visible.into();
        accepted(&value);
    }
}

#[test]
fn blank_text_property_values_are_rejected() {
    // A text property value is displayed like a name: non-empty text made only of
    // whitespace, blank-looking scalars or lone marks renders as nothing (plan §14). The
    // empty string stays the explicit "no value" (the example's `old-glow` uses it).
    let pointer = "/entries/0/properties/range";
    for blank in [
        "\u{2800}",
        "   ",
        " \u{3164}\u{ffa0} ",
        "\u{034f}",
        "\u{fe0f}",
    ] {
        let mut value = example();
        *value.pointer_mut(pointer).unwrap() = blank.into();
        assert_eq!(rejected(&value), "blank-text-property-value", "{blank:?}");
    }
    for shown in ["", "Near\u{2800}", "e\u{0301}", "\u{2764}\u{fe0f}"] {
        let mut value = example();
        *value.pointer_mut(pointer).unwrap() = shown.into();
        accepted(&value);
    }
}

#[test]
fn text_of_only_nonspacing_marks_is_rejected() {
    // Combining marks and variation selectors (Mn) alone render as nothing or as a dotted
    // circle: required text needs a scalar beyond them (plan §14). With a base they stay.
    for pointer in [
        "/title",
        "/entry_types/0/label",
        "/entry_types/0/properties/0/label",
        "/entry_types/0/properties/1/options/0/label",
        "/entries/0/name",
        "/entries/1/description/owner_text",
    ] {
        for blank in [
            "\u{034f}",
            "\u{fe0f}",
            "\u{17b4}",
            "\u{17b5}",
            " \u{0301}\u{fe0e} ",
        ] {
            let mut value = example();
            *value.pointer_mut(pointer).unwrap() = blank.into();
            rejected(&value);
        }
        for shown in ["e\u{0301}", "Text\u{034f}more", "\u{2764}\u{fe0f}"] {
            let mut value = example();
            *value.pointer_mut(pointer).unwrap() = shown.into();
            accepted(&value);
        }
    }
    let mut value = example();
    *value
        .pointer_mut("/entries/1/description/owner_text")
        .unwrap() = "\u{17b4}\u{200b}".into();
    assert_eq!(rejected(&value), "description-needs-units-xor-owner-text");
}

#[test]
fn admission_profile_one_is_pinned() {
    // `catalogue-admission-1` is stored with every published version and re-validates it
    // (plan §12, §13). Changing any limit, the blank-looking set or the schema below changes
    // what profile 1 admits: that needs a new profile id with its own validator (and a CHECK
    // widened by migration), never an edit of these values.
    assert_eq!(admission::ADMISSION_PROFILE, "catalogue-admission-1");
    assert_eq!(
        [
            decode::MAX_BYTES,
            decode::MAX_NODES,
            decode::MAX_DEPTH,
            decode::MAX_KEYS,
            decode::MAX_KEY,
            decode::MAX_ENTRIES,
            decode::MAX_TYPES,
            decode::MAX_PROPERTIES,
            decode::MAX_OPTIONS,
            decode::MAX_UNITS,
            decode::MAX_SOURCES,
            decode::MAX_NAME,
            decode::MAX_ID,
            decode::MAX_TEXT_PROPERTY,
            decode::MAX_OWNER_TEXT,
            MAX_DISTINCT_UNITS,
            admission::MAX_UNIT_REFERENCES,
            MAX_OWNER_TEXT_TOTAL,
        ],
        [
            4 * 1024 * 1024,
            524_288,
            8,
            32,
            64,
            6_144,
            32,
            16,
            128,
            16,
            32,
            100,
            64,
            1_000,
            20_000,
            16_384,
            6_144,
            1024 * 1024,
        ]
    );
    assert_eq!(
        admission::BLANK_LOOKING,
        ['\u{2800}', '\u{3164}', '\u{115f}', '\u{1160}', '\u{ffa0}']
    );
    // The required-text predicate is frozen too (Space Bunny round 3 N4, Sol round 4): a
    // scalar that is not whitespace, blank-looking, Cf or Mn must be present.
    for (text, admitted) in [
        ("a", true),
        ("e\u{0301}", true),
        ("\u{2764}\u{fe0f}", true),
        ("\u{2067}\u{646}\u{2069}", true),
        ("\u{20e3}", true),
        ("", false),
        (" \t\n", false),
        ("\u{2800}\u{3164}", false),
        ("\u{200b}\u{2067}", false),
        ("\u{034f}\u{fe0f}\u{17b4}\u{17b5}", false),
    ] {
        assert_eq!(admission::label(text), admitted, "{text:?}");
    }
    assert_eq!(
        hex::encode(<sha2::Sha256 as sha2::Digest>::digest(include_bytes!(
            "../../schemas/catalogue-v1.json"
        ))),
        "5c2b30ea1a7f7fe0c26bda4cbce889f6dd5c99a7e16abe0add73bd1e873d5cb7"
    );
}

#[test]
fn migration_ordinal_bounds_follow_the_caps() {
    // Space Bunny round 5 F7: the storage CHECKs repeat the entry, type and unit caps; a cap
    // lowered in Rust must not leave a looser bound in migration 019. Round 6 (review row 22):
    // each bound is matched inside its own table, so bounds swapped between tables fail.
    let migration = include_str!("../../migrations/019_catalogues.sql");
    assert_eq!(ordinal_bounds_by_table(migration), Ok(()));
    let swapped = migration
        .replace("BETWEEN 0 AND 31)", "BETWEEN 0 AND @)")
        .replace("BETWEEN 0 AND 6143)", "BETWEEN 0 AND 31)")
        .replace("BETWEEN 0 AND @)", "BETWEEN 0 AND 6143)");
    assert_ne!(swapped, migration);
    assert!(ordinal_bounds_by_table(&swapped).is_err());
}

/// Each projection table's own `ordinal` CHECK equals its Rust cap minus one.
fn ordinal_bounds_by_table(migration: &str) -> Result<(), String> {
    for (table, cap) in [
        ("catalogue_entry_types", decode::MAX_TYPES),
        ("catalogue_entries", decode::MAX_ENTRIES),
        ("catalogue_entry_units", decode::MAX_UNITS),
    ] {
        let start = migration
            .find(&format!("CREATE TABLE {table} ("))
            .ok_or(format!("{table} missing"))?;
        let body = &migration[start..];
        let body = &body[..body
            .find(") STRICT")
            .ok_or(format!("{table} unterminated"))?];
        let bound = format!(
            "ordinal INTEGER NOT NULL CHECK (ordinal BETWEEN 0 AND {})",
            cap - 1
        );
        if body.matches(&bound).count() != 1
            || body.matches("ordinal INTEGER NOT NULL CHECK").count() != 1
        {
            return Err(format!("{table}: {bound}"));
        }
    }
    if migration.matches("ordinal INTEGER NOT NULL CHECK").count() != 3 {
        return Err("unexpected ordinal CHECK".into());
    }
    Ok(())
}
