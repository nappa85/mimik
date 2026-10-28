use super::*;
use crate::assistant::model::{Spec, parse_spec};
use serde_json::{Value, json};

const REV: &str = "3885219b6810b007914f3a7950a8d1b469d598a5";
const SHA: &str = "03b74727a860a56338e042c4420bb3f04b2fec5734175f4cb9fa853daf52b7e8";

fn spec() -> HfSpec {
    let Ok(Spec::Hf(spec)) = parse_spec(&format!(
        "hf:unsloth/Qwen3.5-9B-GGUF/Qwen3.5-9B-Q4_K_M.gguf@{REV}"
    )) else {
        panic!("fixture spec")
    };
    spec
}

/// A well-formed answer the tests damage one field at a time.
fn good() -> Value {
    json!({
        "sha": REV,
        "private": false,
        "gated": false,
        "cardData": {"license": "apache-2.0"},
        "siblings": [
            {"rfilename": "README.md", "size": 100},
            {"rfilename": "LICENSE", "size": 11_357},
            {"rfilename": "Qwen3.5-9B-Q4_K_M.gguf", "size": 5_680_522_464_u64,
             "lfs": {"sha256": SHA, "size": 5_680_522_464_u64, "pointerSize": 135}}
        ]
    })
}

fn admit_json(value: &Value) -> Result<Facts, Refusal> {
    admit(&serde_json::to_vec(value).unwrap(), &spec())
}

fn refused(value: &Value) -> String {
    admit_json(value).expect_err("must be refused").0
}

#[test]
fn a_well_formed_answer_gives_the_facts() {
    let facts = admit_json(&good()).unwrap();
    assert_eq!(facts.commit, REV);
    assert_eq!(facts.sha256, SHA);
    assert_eq!(facts.size, 5_680_522_464);
    assert_eq!(facts.licence_id.as_deref(), Some("apache-2.0"));
    // No card link: the root LICENSE is the licence file.
    assert_eq!(facts.licence_file.as_deref(), Some("LICENSE"));
    assert_eq!(facts.licence_size, Some(11_357));
}

#[test]
fn a_commit_other_than_the_revision_is_refused() {
    // A moved tag or a rewritten branch must not become a download.
    let mut value = good();
    value["sha"] = json!("0".repeat(40));
    assert!(refused(&value).contains("commit"));
    let mut value = good();
    value.as_object_mut().unwrap().remove("sha");
    assert!(refused(&value).contains("commit"));
}

#[test]
fn a_missing_lfs_entry_or_a_malformed_hash_or_size_is_refused() {
    for damage in [
        |v: &mut Value| {
            v["siblings"][2].as_object_mut().unwrap().remove("lfs");
        },
        |v: &mut Value| v["siblings"][2]["lfs"]["sha256"] = json!(SHA.to_uppercase()),
        |v: &mut Value| v["siblings"][2]["lfs"]["sha256"] = json!(&SHA[..63]),
        |v: &mut Value| v["siblings"][2]["lfs"]["sha256"] = json!(5),
        |v: &mut Value| v["siblings"][2]["lfs"]["size"] = json!("5680522464"),
        |v: &mut Value| v["siblings"][2]["lfs"]["size"] = json!(5.5),
        |v: &mut Value| v["siblings"][2]["lfs"]["size"] = json!(-1),
        |v: &mut Value| v["siblings"][2]["lfs"]["size"] = json!(0),
        |v: &mut Value| {
            v["siblings"][2]["lfs"]["size"] = json!(8_589_934_593_u64);
            v["siblings"][2]["size"] = json!(8_589_934_593_u64);
        },
        |v: &mut Value| v["siblings"][2]["size"] = json!(1),
        |v: &mut Value| v["siblings"] = json!([]),
        |v: &mut Value| {
            v.as_object_mut().unwrap().remove("siblings");
        },
    ] {
        let mut value = good();
        damage(&mut value);
        assert!(admit_json(&value).is_err(), "{value}");
    }
    // The 8 GiB bound itself is allowed.
    let mut value = good();
    value["siblings"][2]["lfs"]["size"] = json!(8_589_934_592_u64);
    value["siblings"][2]["size"] = json!(8_589_934_592_u64);
    assert!(admit_json(&value).is_ok());
}

#[test]
fn metadata_over_one_mebibyte_or_not_json_is_refused_unread() {
    let mut value = good();
    value["padding"] = json!("x".repeat(MAX_METADATA_BYTES));
    assert!(refused(&value).contains("1 MiB"));
    assert!(admit(b"not json", &spec()).is_err());
    assert!(admit(b"[]", &spec()).is_err());
}

#[test]
fn a_gated_or_private_repository_is_refused_with_the_by_hand_advice() {
    for (key, value) in [
        ("gated", json!("auto")),
        ("gated", json!("manual")),
        ("gated", json!(true)),
        ("private", json!(true)),
    ] {
        let mut answer = good();
        answer[key] = value;
        let why = refused(&answer);
        assert!(why.contains("by hand"), "{why}");
    }
    // Absent flags are not assumed false.
    for key in ["gated", "private"] {
        let mut answer = good();
        answer.as_object_mut().unwrap().remove(key);
        assert!(admit_json(&answer).is_err(), "{key}");
    }
}

#[test]
fn the_licence_id_must_be_a_short_plain_string() {
    for bad in [
        json!(5),
        json!(["mit"]),
        json!("x".repeat(65)),
        json!("a b"),
        json!("<b>"),
        json!(""),
    ] {
        let mut answer = good();
        answer["cardData"]["license"] = bad.clone();
        assert!(admit_json(&answer).is_err(), "{bad}");
    }
    let mut answer = good();
    answer["cardData"] = json!({});
    assert_eq!(admit_json(&answer).unwrap().licence_id, None);
    answer.as_object_mut().unwrap().remove("cardData");
    assert_eq!(admit_json(&answer).unwrap().licence_id, None);
}

#[test]
fn the_licence_file_is_a_listed_repository_path_never_an_external_link() {
    // The card names a repository file: that one, not the root LICENSE.
    let mut answer = good();
    answer["cardData"]["license_link"] = json!("docs/NOTICE.txt");
    answer["siblings"]
        .as_array_mut()
        .unwrap()
        .push(json!({"rfilename": "docs/NOTICE.txt", "size": 40}));
    let facts = admit_json(&answer).unwrap();
    assert_eq!(facts.licence_file.as_deref(), Some("docs/NOTICE.txt"));

    // An external link is text only; the root file is still the licence file.
    let mut answer = good();
    answer["cardData"]["license_link"] = json!("https://evil.example/LICENSE");
    let facts = admit_json(&answer).unwrap();
    assert_eq!(facts.licence_file.as_deref(), Some("LICENSE"));
    assert_eq!(
        facts.licence_link.as_deref(),
        Some("https://evil.example/LICENSE")
    );

    // A protocol-relative link and traversal are never a path.
    for link in ["//evil.example/x", "../LICENSE", "a/../LICENSE"] {
        let mut answer = good();
        answer["cardData"]["license_link"] = json!(link);
        let facts = admit_json(&answer).unwrap();
        assert_eq!(facts.licence_file.as_deref(), Some("LICENSE"), "{link}");
        assert!(facts.licence_link.is_some(), "{link}");
    }

    // Named but missing: refused, not "none".
    let mut answer = good();
    answer["cardData"]["license_link"] = json!("NOPE.txt");
    assert!(refused(&answer).contains("does not list"));
}

#[test]
fn no_licence_file_is_none_and_an_oversize_one_is_refused() {
    let mut answer = good();
    answer["siblings"].as_array_mut().unwrap().remove(1);
    let facts = admit_json(&answer).unwrap();
    assert_eq!((facts.licence_file, facts.licence_link), (None, None));

    let mut answer = good();
    answer["siblings"][1]["size"] = json!(MAX_LICENCE_BYTES + 1);
    assert!(refused(&answer).contains("licence file is larger"));
    // Fallback order among root names.
    let mut answer = good();
    answer["siblings"][1]["rfilename"] = json!("LICENSE.txt");
    answer["siblings"]
        .as_array_mut()
        .unwrap()
        .push(json!({"rfilename": "LICENSE.md", "size": 5}));
    assert_eq!(
        admit_json(&answer).unwrap().licence_file.as_deref(),
        Some("LICENSE.md")
    );
}

#[test]
fn the_repository_path_grammar_matches_the_spec_path() {
    for ok in ["LICENSE", "a/b/LICENSE.md", "a_b-c.d/e"] {
        assert!(valid_repo_path(ok), "{ok}");
    }
    for bad in [
        "", ".", "..", "a/../b", "/a", "a/", "a//b", "a%2fb", "a\\b", "a b", "a?b",
    ] {
        assert!(!valid_repo_path(bad), "{bad}");
    }
    assert!(!valid_repo_path(&"a".repeat(129)));
}
