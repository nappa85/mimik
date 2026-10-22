use super::*;

#[test]
fn plan_examples() {
    let cases: &[(&str, &[u64])] = &[
        ("heal 15", &[15]),
        ("take \u{2212}15 damage", &[15]),
        ("take -15 damage", &[15]),
        ("15%", &[15]),
        ("1.5", &[]),
        ("x.5", &[]),
        ("5.", &[5]),
        ("I lost 5.", &[5]),
        ("1,000", &[]),
        ("1e2", &[]),
        ("3d6", &[]),
        ("15hp", &[]),
        ("15\u{b2}", &[]),
        ("\u{2460}", &[]),
        ("\u{2460}5", &[]),
        ("007", &[]),
        ("0", &[0]),
        ("\u{ff11}\u{ff15}", &[15]),
        ("\u{0661}\u{0665}", &[15]),
        ("1234567890123456789", &[]),
        ("123456789012345678", &[123_456_789_012_345_678]),
        ("1\u{066b}5", &[]),
        ("1\u{ff0e}5", &[]),
        ("1\u{ff0c}000", &[]),
        ("1\u{a0}500", &[]),
        ("1\u{202f}500", &[]),
        ("1'500", &[]),
        ("1\u{2019}500", &[]),
        ("15, then", &[15]),
        ("(15)", &[15]),
        ("15/20", &[15, 20]),
        ("heal,15", &[]),
        ("1,2,3", &[]),
        ("took 4 and 4", &[4, 4]),
        ("\u{2167}5", &[]),
    ];
    for (phrase, expected) in cases {
        assert_eq!(&phrase_numbers(phrase), expected, "{phrase:?}");
    }
}

#[test]
fn signs_and_spaces_are_separators() {
    assert_eq!(phrase_numbers("\u{2212}15"), [15]);
    assert_eq!(phrase_numbers("84 \u{a0}15"), [84, 15]);
    assert_eq!(phrase_numbers("heal\u{a0}15"), [15]);
    assert_eq!(phrase_numbers("lost\u{202f}5"), [5]);
}

#[test]
fn every_decimal_digit_folds_to_its_value() {
    // Mathematical digits sit in five adjacent runs of ten.
    assert_eq!(decimal('\u{1d7ce}'), Some(0));
    assert_eq!(decimal('\u{1d7d9}'), Some(1));
    assert_eq!(decimal('\u{1d7ff}'), Some(9));
    assert_eq!(decimal('\u{0669}'), Some(9));
    assert_eq!(decimal('\u{b2}'), None);
    assert_eq!(decimal('a'), None);
}

#[test]
fn fraction_words() {
    for phrase in [
        "half of 84",
        "HALF",
        "a third",
        "a quarter",
        "10 percent",
        "10%",
        "metà",
        "meta",
        "METÀ",
        "mezzo",
        "mezza",
        "terzo",
        "terza",
        "quarto",
        "quarta",
        "percento",
        "dieci per cento",
        "Per  Cento",
    ] {
        assert!(has_fraction_word(phrase), "{phrase:?}");
    }
    for phrase in [
        "halfway",
        "quarterly",
        "lost 4",
        "percentage",
        "metadata",
        "per",
        "cento",
        "",
    ] {
        assert!(!has_fraction_word(phrase), "{phrase:?}");
    }
}
