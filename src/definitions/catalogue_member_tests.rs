//! Assisted sheet step 3a: the optional `catalogue` member of a text item property
//! (`docs/runtime-definition-contract.md`). Omitted, it changes no canonical bytes or hash;
//! admission checks its link, its key property and the quick-add mapping with one static code
//! at an exact path.
use super::quick_add_tests::{GEAR_HASH, admit, code, gear, property};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// `gear()` linked to a catalogue, with the member on `name` (property 0) keyed by a new
/// optional text property `gear_key`.
fn member() -> Value {
    let mut definition = gear();
    definition["catalogues"] = json!([{"id": "assist-gear"}]);
    let properties = &mut definition["fields"][1]["items"]["properties"];
    properties[0]["catalogue"] =
        json!({"id": "assist-gear", "entry_type": "gear", "key": "gear_key"});
    properties
        .as_array_mut()
        .unwrap()
        .push(property("gear_key", "text", true));
    definition
}

const PATH: &str = "/fields/1/items/properties/0/catalogue";

#[test]
fn definitions_without_the_member_keep_their_hash() {
    let plain = admit(&gear()).unwrap();
    assert_eq!(plain.hash, GEAR_HASH);
    assert!(!plain.content.contains("catalogue\""));
}

/// The canonical form of a definition with the member, and its hash computed here from the
/// documented encoding (domain prefix, NUL, canonical bytes) rather than by `admission::hash`.
#[test]
fn the_member_is_canonical_last_and_has_a_frozen_hash_vector() {
    let admitted = admit(&member()).unwrap();
    assert!(
        admitted.content.contains(
            r#"{"id":"name","label":"NAME","control":"text","optional":false,"options":null,"catalogue":{"id":"assist-gear","entry_type":"gear","key":"gear_key"}}"#
        ),
        "{}",
        admitted.content
    );
    let mut sha = Sha256::new();
    sha.update(b"mimik-definition-v1\0");
    sha.update(admitted.content.as_bytes());
    assert_eq!(admitted.hash, hex::encode(sha.finalize()));
    assert_eq!(
        admitted.hash, "6fed2be15fed2f6e789dabdd12d4ce35cbfc3103331fb7867942b15568718222",
        "frozen: a change here changes every published definition that uses the member"
    );
    let again = super::admission::validate(admitted.content.as_bytes()).unwrap();
    assert_eq!(
        (again.content, again.hash),
        (admitted.content, admitted.hash)
    );
    // The entry type is optional and omitted when absent.
    let mut untyped = member();
    untyped["fields"][1]["items"]["properties"][0]["catalogue"]
        .as_object_mut()
        .unwrap()
        .remove("entry_type");
    let untyped = admit(&untyped).unwrap();
    assert!(
        untyped
            .content
            .contains(r#""catalogue":{"id":"assist-gear","key":"gear_key"}"#)
    );
}

#[test]
fn the_member_is_closed_and_needs_its_id_and_key() {
    for (change, expected) in [
        (json!({"id": "assist-gear"}), "required"),
        (json!({"key": "gear_key"}), "required"),
        (
            json!({"id": "assist-gear", "key": "gear_key", "extra": 1}),
            "additionalProperties",
        ),
        (json!({"id": "", "key": "gear_key"}), "minLength"),
        (json!("assist-gear"), "type"),
    ] {
        let mut definition = member();
        definition["fields"][1]["items"]["properties"][0]["catalogue"] = change.clone();
        assert_eq!(
            code(admit(&definition)),
            format!(
                "{PATH}{} {expected}",
                if expected == "minLength" { "/id" } else { "" }
            ),
            "{change}"
        );
    }
}

#[test]
fn every_refusal_has_one_code_at_its_exact_path() {
    let at = |mut definition: Value, p: usize, member: Value| {
        definition["fields"][1]["items"]["properties"][p]["catalogue"] = member;
        definition
    };
    // Not a text property.
    let mut not_text = member();
    not_text["fields"][1]["items"]["properties"][0]["control"] = "multiline".into();
    assert_eq!(
        code(admit(&not_text)),
        format!("{PATH} catalogue-member-not-text")
    );
    // The id is not one of the definition's links.
    let mut unlinked = member();
    unlinked["fields"][1]["items"]["properties"][0]["catalogue"]["id"] = "other".into();
    assert_eq!(
        code(admit(&unlinked)),
        format!("{PATH} catalogue-member-unlinked")
    );
    let mut none = member();
    none.as_object_mut().unwrap().remove("catalogues");
    assert_eq!(
        code(admit(&none)),
        format!("{PATH} catalogue-member-unlinked")
    );
    // A second member in the list.
    let repeated = at(member(), 5, json!({"id": "assist-gear", "key": "worn"}));
    assert_eq!(
        code(admit(&repeated)),
        "/fields/1/items/properties/5/catalogue catalogue-member-repeated"
    );
    // The key names no other property (nor the property itself).
    for key in ["nothing", "name"] {
        let mut unknown = member();
        unknown["fields"][1]["items"]["properties"][0]["catalogue"]["key"] = key.into();
        assert_eq!(
            code(admit(&unknown)),
            format!("{PATH} catalogue-key-unknown"),
            "{key}"
        );
    }
    // The key is not optional text, or has a member itself.
    for (key, change) in [
        ("count", None),
        ("worn", None),
        ("story", None),
        ("gear_key", Some(("optional", json!(false)))),
    ] {
        let mut invalid = member();
        invalid["fields"][1]["items"]["properties"][0]["catalogue"]["key"] = key.into();
        if let Some((member, value)) = change {
            invalid["fields"][1]["items"]["properties"][7][member] = value;
        }
        assert_eq!(
            code(admit(&invalid)),
            format!("{PATH} catalogue-key-invalid"),
            "{key}"
        );
    }
    let mut backed = member();
    backed["fields"][1]["items"]["properties"][7]["catalogue"] =
        json!({"id": "assist-gear", "key": "maker"});
    // A key property that carries a member itself is an invalid key.
    assert_eq!(
        code(admit(&backed)),
        format!("{PATH} catalogue-key-invalid")
    );
    // Quick add must not name the key property.
    let mut quick = member();
    quick["fields"][1]["items"]["quick_add"] = json!({"name": "gear_key"});
    assert_eq!(
        code(admit(&quick)),
        "/fields/1/items/quick_add quick-add-name-is-catalogue-key"
    );
    // Naming the member property itself stays allowed.
    let mut named = member();
    named["fields"][1]["items"]["quick_add"] = json!({"name": "name", "quantity": "count"});
    assert_eq!(code(admit(&named)), "admitted");
}

#[test]
fn the_key_property_is_an_ordinary_optional_text_property() {
    let admitted = admit(&member()).unwrap();
    let list = &admitted.definition.fields[1];
    let key = list
        .items
        .as_ref()
        .unwrap()
        .properties
        .iter()
        .find(|property| property.id == "gear_key")
        .unwrap();
    assert!(key.optional && key.catalogue.is_none());
}
