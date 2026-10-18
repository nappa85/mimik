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

#[test]
fn decoder_rejects_duplicate_and_escaped_equivalent_keys_at_every_level() {
    let compact = serde_json::to_string(&example()).unwrap();
    for (from, to) in [
        (
            r#""contract_version":1"#,
            r#""contract_version":1,"contract_version":1"#,
        ),
        (
            r#""contract_version":1"#,
            r#""contract_version":1,"contract_version":1"#,
        ),
        (
            r#""name":"House Charm""#,
            r#""name":"House Charm","name":"Other""#,
        ),
        (r#""level":1"#, r#""level":1,"level":2"#),
        (r#""level":1"#, r#""level":1,"level":2"#),
        (r#""unit_id":977"#, r#""unit_id":977,"unit_id":978"#),
        (r#""id":"charm""#, r#""id":"charm","id":"charm""#),
    ] {
        assert!(compact.contains(from), "{from}");
        let code = rejected_bytes(compact.replacen(from, to, 1).as_bytes());
        assert_eq!(code, "malformed-json-or-duplicate-key-or-input-limit");
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
fn entry_count_accepts_8192_and_rejects_8193() {
    let entries = |count: usize| {
        (0..count)
            .map(|n| owner(&format!("e{n}"), "x"))
            .collect::<Vec<_>>()
    };
    let admitted = accepted(&with_array("/entries", entries(decode::MAX_ENTRIES)));
    assert_eq!(admitted.counts.entries, 8_192);
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
fn distinct_units_are_capped_per_catalogue() {
    let limit = i64::try_from(MAX_DISTINCT_UNITS).unwrap();
    let entries = |extra: i64| {
        (0..(limit + extra + 15) / 16)
            .map(|entry| {
                let first = entry * 16 + 1;
                let last = (first + 15).min(limit + extra);
                spell(
                    &format!("e{entry}"),
                    &(first..=last)
                        .map(|unit| json!({"source_id": 1, "page_id": 2, "unit_id": unit}))
                        .collect::<Vec<_>>(),
                )
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        accepted(&with_array("/entries", entries(0)))
            .counts
            .distinct_units,
        MAX_DISTINCT_UNITS
    );
    assert_eq!(
        rejected(&with_array("/entries", entries(1))),
        "distinct-unit-limit"
    );
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
