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

#[test]
fn number_words_count_as_their_number() {
    let cases: &[(&str, &[u64])] = &[
        ("one lost for each day", &[1]),
        ("ten", &[10]),
        ("dieci", &[10]),
        ("TEN", &[10]),
        ("Venti danni", &[20]),
        ("zero", &[0]),
        ("twenty", &[20]),
        ("ninety", &[90]),
        ("a hundred", &[100]),
        ("cento", &[100]),
        ("uno", &[1]),
        ("una", &[1]),
        ("un", &[1]),
        ("un'ora", &[1]),
        ("un\u{2019}ora", &[1]),
        ("due e tre", &[2, 3]),
        ("lost one, then one", &[1, 1]),
        ("twenty-one", &[20, 1]),
        ("one hundred", &[1, 100]),
        ("lost 5 and two", &[2]),
        ("somewhat", &[]),
        ("someone", &[]),
        ("alone", &[]),
        ("anyone", &[]),
        ("tenant", &[]),
        ("ventuno", &[]),
        ("one5", &[]),
        ("a point", &[]),
        ("an arrow", &[]),
        ("tr\u{e9}", &[3]),
        ("Tr\u{e8} danni", &[3]),
        ("di\u{e8}ci", &[10]),
        ("\u{17f}ei", &[]),
        ("", &[]),
    ];
    for (phrase, expected) in cases {
        assert_eq!(&number_words(phrase), expected, "{phrase:?}");
    }
}

#[test]
fn every_number_word_has_its_value() {
    let words: &[(&str, u64)] = &[
        ("zero", 0),
        ("one", 1),
        ("two", 2),
        ("three", 3),
        ("four", 4),
        ("five", 5),
        ("six", 6),
        ("seven", 7),
        ("eight", 8),
        ("nine", 9),
        ("ten", 10),
        ("eleven", 11),
        ("twelve", 12),
        ("thirteen", 13),
        ("fourteen", 14),
        ("fifteen", 15),
        ("sixteen", 16),
        ("seventeen", 17),
        ("eighteen", 18),
        ("nineteen", 19),
        ("twenty", 20),
        ("thirty", 30),
        ("forty", 40),
        ("fifty", 50),
        ("sixty", 60),
        ("seventy", 70),
        ("eighty", 80),
        ("ninety", 90),
        ("hundred", 100),
        ("zero", 0),
        ("uno", 1),
        ("una", 1),
        ("un", 1),
        ("due", 2),
        ("tre", 3),
        ("quattro", 4),
        ("cinque", 5),
        ("sei", 6),
        ("sette", 7),
        ("otto", 8),
        ("nove", 9),
        ("dieci", 10),
        ("undici", 11),
        ("dodici", 12),
        ("tredici", 13),
        ("quattordici", 14),
        ("quindici", 15),
        ("sedici", 16),
        ("diciassette", 17),
        ("diciotto", 18),
        ("diciannove", 19),
        ("venti", 20),
        ("trenta", 30),
        ("quaranta", 40),
        ("cinquanta", 50),
        ("sessanta", 60),
        ("settanta", 70),
        ("ottanta", 80),
        ("novanta", 90),
        ("cento", 100),
    ];
    for (word, value) in words {
        assert_eq!(number_words(word), [*value], "{word}");
    }
}
