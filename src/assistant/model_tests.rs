use super::*;

fn rule(text: &str) -> Rule {
    parse_spec(text)
        .expect_err(&format!("{text:?} must be refused"))
        .rule
}

fn max_tokens(spec: &str, sha256: Option<&str>) -> Result<u32, String> {
    resolve(spec, sha256).map(|(_, tokens)| tokens)
}

#[test]
fn each_compiled_in_file_resolves_to_its_own_hash_and_budget() {
    assert_eq!(
        resolve(DEFAULT_MODEL, None),
        Ok((DEFAULT_MODEL_SHA256.to_owned(), 352))
    );
    assert_eq!(
        resolve(GPU_DEFAULT_MODEL, None),
        Ok((GPU_DEFAULT_MODEL_SHA256.to_owned(), 352))
    );
    // An explicit hash is kept, and a compiled-in one gives its budget under any name.
    assert_eq!(
        resolve("local:renamed.gguf", Some(GPU_DEFAULT_MODEL_SHA256)),
        Ok((GPU_DEFAULT_MODEL_SHA256.to_owned(), 352))
    );
    assert_eq!(
        resolve(GPU_DEFAULT_MODEL, Some(&"a".repeat(64))),
        Ok(("a".repeat(64), 256))
    );
    assert!(resolve("local:other.gguf", None).is_err());
}

#[test]
fn the_gpu_default_has_its_hash_and_budget_compiled_in() {
    assert!(valid_sha256(GPU_DEFAULT_MODEL_SHA256));
    assert_eq!(max_tokens(GPU_DEFAULT_MODEL, None), Ok(352));
    assert_eq!(trial_max_tokens(GPU_DEFAULT_MODEL_SHA256, 100), 352);
    assert_eq!(
        default_alias(GPU_DEFAULT_MODEL).unwrap(),
        "qwen3.5-9b-q4_k_m"
    );
    // Another file under a similar name needs its own hash.
    assert!(max_tokens("local:Qwen3.5-9B-Q4_K_M2.gguf", None).is_err());
}

#[test]
fn local_specs_parse_to_their_file_name() {
    assert_eq!(
        parse_spec(DEFAULT_MODEL),
        Ok(Spec::Local("granite-4.2-3b-Q4_K_M.gguf".into()))
    );
    for spec in [
        "local:a.gguf",
        "local:A_b-c.1.GGUF.gguf",
        "local:consol.gguf",
    ] {
        assert!(parse_spec(spec).is_ok(), "{spec}");
    }
    // The longest name, 128 characters.
    let longest = format!("local:{}.gguf", "a".repeat(123));
    assert!(parse_spec(&longest).is_ok());
    assert_eq!(
        rule(&format!("local:{}.gguf", "a".repeat(124))),
        Rule::NameLength
    );
}

#[test]
fn refused_specs_name_the_rule_they_break() {
    for (spec, expected) in [
        ("local:../x.gguf", Rule::PathSeparator),
        ("local:/etc/x.gguf", Rule::PathSeparator),
        ("local:a/b.gguf", Rule::PathSeparator),
        ("local:a\\b.gguf", Rule::PathSeparator),
        ("local:.x.gguf", Rule::LeadingDot),
        ("local:.gguf", Rule::LeadingDot),
        ("local:CON.gguf", Rule::DeviceName),
        ("local:nul.gguf", Rule::DeviceName),
        ("local:Com1.gguf", Rule::DeviceName),
        ("local:lpt9.x.gguf", Rule::DeviceName),
        ("local:aux.gguf", Rule::DeviceName),
        ("local:prn.gguf", Rule::DeviceName),
        ("local:x.bin", Rule::Extension),
        ("local:x", Rule::Extension),
        ("local:x.gguf.part", Rule::Extension),
        ("local:a:b.gguf", Rule::Charset),
        ("local:a%2fb.gguf", Rule::Charset),
        ("local:", Rule::NameLength),
        ("local: x.gguf", Rule::Whitespace),
        ("local:x.gguf ", Rule::Whitespace),
        (" local:x.gguf", Rule::Whitespace),
        ("local:a b.gguf", Rule::Whitespace),
        ("local:a\tb.gguf", Rule::Whitespace),
        ("local:a\nb.gguf", Rule::Whitespace),
        ("local:a\0b.gguf", Rule::Whitespace),
        ("local:\u{e9}.gguf", Rule::NotAscii),
        ("local:x.gguf\u{a0}", Rule::NotAscii),
        ("x.gguf", Rule::Form),
        ("LOCAL:x.gguf", Rule::Form),
        ("file:x.gguf", Rule::Form),
        ("", Rule::Form),
    ] {
        assert_eq!(rule(spec), expected, "{spec:?}");
    }
    // 512 bytes pass the length rule (and then fail the name rule); 513 do not.
    let at_limit = format!("local:{}", "a".repeat(512 - 6));
    assert_eq!(rule(&at_limit), Rule::NameLength);
    assert_eq!(rule(&format!("{at_limit}a")), Rule::TooLong);
    assert_eq!(rule(&"a".repeat(513)), Rule::TooLong);
}

const GRANITE_HF: &str = "hf:ibm-granite/granite-4.2-3b-GGUF/granite-4.2-3b-Q4_K_M.gguf@c40945d71cd90f249a56985e8155551a9188dc30";
const QWEN_HF: &str =
    "hf:unsloth/Qwen3.5-9B-GGUF/Qwen3.5-9B-Q4_K_M.gguf@3885219b6810b007914f3a7950a8d1b469d598a5";

#[test]
fn the_two_pinned_hf_specs_are_accepted_and_keep_their_parts() {
    for text in [GRANITE_HF, QWEN_HF] {
        let Ok(Spec::Hf(hf)) = parse_spec(text) else {
            panic!("{text} must parse as an hf spec");
        };
        assert_eq!(hf.canonical(), text);
    }
    let Ok(Spec::Hf(hf)) = parse_spec(QWEN_HF) else {
        panic!()
    };
    assert_eq!(
        (hf.org.as_str(), hf.repo.as_str()),
        ("unsloth", "Qwen3.5-9B-GGUF")
    );
    assert_eq!(hf.path, "Qwen3.5-9B-Q4_K_M.gguf");
    // A nested path is allowed.
    let nested = QWEN_HF.replace("/Qwen3.5-9B-Q4", "/dir/sub/Qwen3.5-9B-Q4");
    assert!(parse_spec(&nested).is_ok(), "{nested}");
    // The other rules come first, as for any spec.
    assert_eq!(rule(&format!("{GRANITE_HF} ")), Rule::Whitespace);
}

#[test]
fn an_hf_spec_that_could_reach_another_url_is_refused_with_its_own_rule() {
    let rev = "c40945d71cd90f249a56985e8155551a9188dc30";
    let ok = |path: &str| format!("hf:org/repo/{path}@{rev}");
    for (text, expected) in [
        // A branch, tag, short or upper-case revision, or none.
        ("hf:org/repo/m.gguf@main".to_owned(), Rule::HfRevision),
        ("hf:org/repo/m.gguf@c40945d".to_owned(), Rule::HfRevision),
        (
            format!("hf:org/repo/m.gguf@{}", rev.to_uppercase()),
            Rule::HfRevision,
        ),
        (format!("hf:org/repo/m.gguf@{rev}0"), Rule::HfRevision),
        ("hf:org/repo/m.gguf".to_owned(), Rule::HfForm),
        ("hf:org/m.gguf@".to_owned() + rev, Rule::HfForm),
        ("hf:".to_owned(), Rule::HfForm),
        // Traversal and encoding in the path.
        (ok("../m.gguf"), Rule::HfPath),
        (ok("./m.gguf"), Rule::HfPath),
        (ok("a/../m.gguf"), Rule::HfPath),
        (ok("%2e%2e/m.gguf"), Rule::HfPath),
        (ok("a%2fb.gguf"), Rule::HfPath),
        (ok("a\\b.gguf"), Rule::HfPath),
        (ok("a//b.gguf"), Rule::HfPath),
        (ok("/m.gguf"), Rule::HfPath),
        (ok("m?x.gguf"), Rule::HfPath),
        (ok("m#x.gguf"), Rule::HfPath),
        // Not a single model file.
        (ok("m.bin"), Rule::HfExtension),
        (ok("m"), Rule::HfExtension),
        (ok("m-00001-of-00003.gguf"), Rule::HfSplit),
        // Owner and repository.
        (format!("hf:../repo/m.gguf@{rev}"), Rule::HfOwner),
        (format!("hf:-org/repo/m.gguf@{rev}"), Rule::HfOwner),
        (format!("hf:or--g/repo/m.gguf@{rev}"), Rule::HfOwner),
        (format!("hf:org/re..po/m.gguf@{rev}"), Rule::HfRepo),
        (format!("hf:org/repo./m.gguf@{rev}"), Rule::HfRepo),
        (
            format!("hf:{}/repo/m.gguf@{rev}", "o".repeat(97)),
            Rule::HfOwner,
        ),
    ] {
        assert_eq!(rule(&text), expected, "{text}");
    }
    // Segment length and an overlong path.
    assert_eq!(
        rule(&ok(&format!("{}.gguf", "a".repeat(129)))),
        Rule::HfPath
    );
}

#[test]
fn every_rule_has_its_own_message() {
    let rules = [
        Rule::TooLong,
        Rule::NotAscii,
        Rule::Whitespace,
        Rule::HfForm,
        Rule::HfOwner,
        Rule::HfRepo,
        Rule::HfPath,
        Rule::HfExtension,
        Rule::HfSplit,
        Rule::HfRevision,
        Rule::Form,
        Rule::NameLength,
        Rule::PathSeparator,
        Rule::Charset,
        Rule::LeadingDot,
        Rule::Extension,
        Rule::DeviceName,
    ];
    let messages: std::collections::BTreeSet<String> = rules
        .iter()
        .map(|&rule| SpecError { rule }.to_string())
        .collect();
    assert_eq!(messages.len(), rules.len());
}

#[test]
fn flags_derive_the_alias_and_the_unmeasured_max_tokens() {
    assert_eq!(
        default_alias(DEFAULT_MODEL).unwrap(),
        "granite-4.2-3b-q4_k_m"
    );
    assert_eq!(max_tokens(DEFAULT_MODEL, None), Ok(352));
    assert_eq!(
        max_tokens(DEFAULT_MODEL, Some(DEFAULT_MODEL_SHA256)),
        Ok(352)
    );
    // Whatever its name, another file is 256.
    assert_eq!(max_tokens(DEFAULT_MODEL, Some(&"a".repeat(64))), Ok(256));
    assert_eq!(
        max_tokens("local:other.gguf", Some(DEFAULT_MODEL_SHA256)),
        Ok(352)
    );
    assert!(max_tokens("local:other.gguf", None).is_err());
    assert!(max_tokens("local:other.gguf", Some("ABC")).is_err());
    for refused in [
        "hf:x/y/z.gguf@0",
        "local:.hidden.gguf",
        "local:a/b.gguf",
        "local:x.bin",
        "local:",
    ] {
        assert!(default_alias(refused).is_err(), "{refused}");
    }
}

#[test]
fn the_trial_rule_is_the_phrases_largest_predicted_n_with_the_floor_and_the_cap() {
    // 1.25 x n rounded up to a multiple of 32, never below 256, at most 384.
    for (largest, expected) in [
        (0, 256),
        (1, 256),
        (200, 256),
        (204, 256),
        (205, 288),
        (230, 288),
        (231, 320),
        (256, 320),
        (257, 352),
        (263, 352),
        (281, 352),
        (282, 384),
        (307, 384),
        (384, 384),
        (u32::MAX, 384),
    ] {
        assert_eq!(measured_max_tokens(largest), expected, "{largest}");
    }
}

#[test]
fn the_default_file_keeps_352_by_hash_and_a_file_with_its_name_does_not() {
    // By hash: the default file keeps 352 whatever the phrases measured.
    assert_eq!(trial_max_tokens(DEFAULT_MODEL_SHA256, 100), 352);
    assert_eq!(trial_max_tokens(DEFAULT_MODEL_SHA256, 384), 352);
    // Any other hash gets the measured rule, whatever name its file has.
    assert_eq!(trial_max_tokens(&"b".repeat(64), 100), 256);
    assert_eq!(trial_max_tokens(&"b".repeat(64), 263), 352);
    assert_eq!(trial_max_tokens(&"b".repeat(64), 300), 384);
}

#[test]
fn sha256_text_is_lowercase_hex_of_64_digits() {
    assert!(valid_sha256(DEFAULT_MODEL_SHA256));
    assert!(!valid_sha256(&DEFAULT_MODEL_SHA256.to_uppercase()));
    assert!(!valid_sha256(&DEFAULT_MODEL_SHA256[1..]));
    assert!(!valid_sha256(&format!("{DEFAULT_MODEL_SHA256}0")));
}
