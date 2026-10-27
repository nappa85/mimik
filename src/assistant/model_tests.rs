use super::*;

fn rule(text: &str) -> Rule {
    parse_spec(text)
        .expect_err(&format!("{text:?} must be refused"))
        .rule
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

#[test]
fn an_hf_spec_says_downloads_arrive_later() {
    let hf = "hf:ibm-granite/granite-4.2-3b-GGUF/granite-4.2-3b-Q4_K_M.gguf@c40945d71cd90f249a56985e8155551a9188dc30";
    let error = parse_spec(hf).unwrap_err();
    assert_eq!(error.rule, Rule::Download);
    assert!(
        error.to_string().contains("downloads arrive later"),
        "{error}"
    );
    // The other rules come first, as for any spec.
    assert_eq!(rule(&format!("{hf} ")), Rule::Whitespace);
}

#[test]
fn every_rule_has_its_own_message() {
    let rules = [
        Rule::TooLong,
        Rule::NotAscii,
        Rule::Whitespace,
        Rule::Download,
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
