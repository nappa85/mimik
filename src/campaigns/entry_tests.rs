//! Slice 1.1 grammar table and property tests for `entry::number` (assisted sheet plan
//! §3a, §5 slice 1.1).
use super::{MAX_CHARS, MAX_SCALARS, Reading, Unreadable, number, offer_text, read, sum_text};
use proptest::prelude::*;

fn ok(cases: &[(&str, i64)]) {
    for (raw, want) in cases {
        assert_eq!(number(raw), Ok(*want), "{raw:?}");
    }
}

fn refused(cases: &[&str], want: Unreadable) {
    for raw in cases {
        assert_eq!(number(raw), Err(want), "{raw:?}");
    }
}

#[test]
fn literals_are_their_value() {
    ok(&[
        ("69", 69),
        ("+5", 5),
        ("-5", -5),
        ("\u{2212}5", -5),
        ("-0", 0),
        ("+0", 0),
        ("0", 0),
        ("0007", 7),
        ("-0007", -7),
        (" 42 ", 42),
        ("\t-1\t", -1),
    ]);
}

#[test]
fn sums_are_computed_left_to_right() {
    ok(&[
        ("84 -15", 69),
        ("84-15", 69),
        ("84 - 15", 69),
        ("84-15+3", 72),
        ("\t84 -15\t", 69),
        ("84 \t-\t 15", 69),
        ("84 +15", 99),
        ("84\u{2212}15", 69),
        ("0 - 5", -5),
        ("1-2-3", -4),
        ("007+0003", 10),
    ]);
}

#[test]
fn i64_bounds() {
    ok(&[
        ("-9223372036854775808", i64::MIN),
        ("9223372036854775807", i64::MAX),
        ("+9223372036854775807", i64::MAX),
        ("0-9223372036854775808", i64::MIN),
        // The running sum may leave i64 as long as the result fits.
        ("9223372036854775807+1-1", i64::MAX),
        (
            "9223372036854775807+9223372036854775807-9223372036854775807",
            i64::MAX,
        ),
    ]);
    refused(
        &[
            "9223372036854775807+1",
            "0-9223372036854775808-1",
            "9223372036854775808",
            "-9223372036854775809",
            &"9".repeat(40),
            &format!("-{}", "9".repeat(40)),
            &format!("1+{}", "9".repeat(40)),
        ],
        Unreadable::Range,
    );
}

#[test]
fn every_term_is_checked() {
    // 32 terms (the 200-character cap allows 32 five-digit terms): exact.
    let many = vec!["99999"; 32].join("+");
    assert!(many.len() <= MAX_CHARS);
    assert_eq!(number(&many), Ok(32 * 99_999));
    let mixed = vec!["99999"; 32].join("-");
    assert_eq!(number(&mixed), Ok(99_999 - 31 * 99_999));
    // Nine terms of i64::MAX: the running sum leaves i64 but stays in i128; refused.
    let max = i64::MAX.to_string();
    let up = [max.as_str(); 9].join("+");
    assert!(up.len() <= MAX_CHARS);
    assert_eq!(number(&up), Err(Unreadable::Range));
    // Five additions and four subtractions of i64::MAX come back into range.
    let mut back = vec![max.clone(); 5].join("+");
    for _ in 0..4 {
        back.push('-');
        back.push_str(&max);
    }
    assert_eq!(number(&back), Ok(i64::MAX));
    // Terms near i128::MAX: the second addition would overflow i128, and is refused.
    let huge = "9".repeat(38);
    let near = [huge.as_str(); 3].join("+");
    assert!(near.len() <= MAX_CHARS);
    assert_eq!(number(&near), Err(Unreadable::Range));
    // A 39-digit term is above i128::MAX on its own.
    assert_eq!(number(&"9".repeat(39)), Err(Unreadable::Range));
}

/// Sol review Minor 1: the accumulation is checked, not wrapping. With A = 38 nines,
/// `A+A-A-A` is 0 in exact arithmetic, but A + A already exceeds `i128::MAX`, so the second
/// term's addition is refused; a wrapping sum would come back to 0 and accept it.
#[test]
fn accumulation_overflow_is_not_cancelled_by_later_terms() {
    let a = "9".repeat(38);
    let cancelling = format!("{a}+{a}-{a}-{a}");
    assert!(cancelling.len() <= MAX_CHARS);
    assert_eq!(number(&cancelling), Err(Unreadable::Range));
    // The same terms in an order that never leaves i128 cancel exactly.
    assert_eq!(number(&format!("{a}-{a}+{a}-{a}")), Ok(0));
}

#[test]
fn unreadable_shapes() {
    refused(
        &[
            "84--15",
            "84+-2",
            "84 - -2",
            "- 5",
            "+ 5",
            "5 -",
            "5-",
            "-",
            "+",
            "\u{2212}",
            "1 2",
            "5.",
            "5.0",
            "1,5",
            "\u{ff11}\u{ff12}",
            "\u{0661}",
            "1e2",
            "x",
            "lots",
            "84 -15 hp",
            "2*3",
            "6/2",
            "(5)",
            "-5+-3",
            "- 5+3",
            "-5 3",
            "1\u{a0}2",
            "5\u{2009}-1",
            "--5",
        ],
        Unreadable::Syntax,
    );
}

#[test]
fn blank_is_unreadable() {
    refused(&["", " ", "   ", "\t", "\t \t"], Unreadable::Blank);
}

#[test]
fn limits_and_control_characters() {
    // 201 characters: a valid sum padded with spaces.
    let padded = format!("84 -15{}", " ".repeat(MAX_CHARS + 1 - 6));
    assert_eq!(padded.chars().count(), MAX_CHARS + 1);
    assert_eq!(number(&padded), Err(Unreadable::TooLong));
    // 200 characters is still read.
    let fits = format!("84 -15{}", " ".repeat(MAX_CHARS - 6));
    assert_eq!(number(&fits), Ok(69));
    // The cap counts characters, not bytes: 64 numbers joined by U+2212 are 253 bytes.
    let wide = format!("1{}", "\u{2212}1".repeat(63));
    assert!(wide.chars().count() <= MAX_CHARS && wide.len() > MAX_CHARS);
    assert_eq!(number(&wide), Ok(-62));
    // 64 numbers are read, 65 are refused.
    let sixty_four = vec!["1"; MAX_SCALARS].join("+");
    assert_eq!(number(&sixty_four), Ok(64));
    let sixty_five = vec!["1"; MAX_SCALARS + 1].join("+");
    assert_eq!(number(&sixty_five), Err(Unreadable::TooManyScalars));
    refused(
        &[
            "8\u{0}4", "\u{0}", "84\u{7}", "\u{1b}5", "5\u{7f}", "5\u{85}", "5\r", "5 \n",
        ],
        Unreadable::Control,
    );
}

#[test]
fn messages_name_the_problem() {
    for kind in [
        Unreadable::Blank,
        Unreadable::TooLong,
        Unreadable::Control,
        Unreadable::Syntax,
        Unreadable::TooManyScalars,
        Unreadable::Range,
    ] {
        assert!(kind.message().ends_with('.'), "{kind:?}");
    }
    assert!(Unreadable::Syntax.message().contains("84 -15"));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2_000))]

    /// Arbitrary strings up to 300 characters never panic, and an accepted input re-parses
    /// to the same value from its canonical decimal output.
    #[test]
    fn arbitrary_input_never_panics(raw in "(?s).{0,300}") {
        if let Ok(value) = number(&raw) {
            prop_assert_eq!(number(&value.to_string()), Ok(value));
        }
    }

    /// The same over the grammar's own alphabet, where most inputs are near-misses.
    #[test]
    fn grammar_alphabet_never_panics(raw in "[0-9+\\-\u{2212} \t]{0,300}") {
        if let Ok(value) = number(&raw) {
            prop_assert_eq!(number(&value.to_string()), Ok(value));
        }
    }

    /// A generated sum of i64 terms equals its exact i128 evaluation when that fits i64.
    #[test]
    fn generated_sums_match_i128(
        first in 0..=i64::MAX,
        rest in proptest::collection::vec((any::<bool>(), 0..=i64::MAX), 0..8),
        spaced in any::<bool>(),
    ) {
        let gap = if spaced { " " } else { "" };
        let mut raw = first.to_string();
        let mut exact = i128::from(first);
        for (plus, term) in &rest {
            raw.push_str(gap);
            raw.push(if *plus { '+' } else { '-' });
            raw.push_str(gap);
            raw.push_str(&term.to_string());
            exact += if *plus { i128::from(*term) } else { -i128::from(*term) };
        }
        let want = i64::try_from(exact).map_err(|_| Unreadable::Range);
        prop_assert_eq!(number(&raw), want);
    }
}

/// Slice 1.2: a note records whether the entry was a sum, and a sum note shows it
/// normalised.
#[test]
fn readings_tell_sums_from_literals() {
    for (raw, value, sum, signed) in [
        ("84", 84, false, false),
        ("-15", -15, false, true),
        ("+3", 3, false, true),
        ("0007", 7, false, false),
        ("84 -15", 69, true, false),
        ("84-15+3", 72, true, false),
        ("\t1 + 1\t", 2, true, false),
        ("-5+3", -2, true, true),
    ] {
        assert_eq!(read(raw), Ok(Reading { value, sum, signed }), "{raw:?}");
    }
    assert_eq!(read("lots"), Err(Unreadable::Syntax));
    assert_eq!(sum_text("84 -15").as_deref(), Some("84 \u{2212} 15"));
    assert_eq!(sum_text("084-15+03").as_deref(), Some("84 \u{2212} 15 + 3"));
    assert_eq!(sum_text("84 \u{2212}15").as_deref(), Some("84 \u{2212} 15"));
    for raw in ["84", "-15", "lots", "", "84--15"] {
        assert_eq!(sum_text(raw), None, "{raw:?}");
    }
}

#[test]
fn only_unreadable_text_keeps_a_known_value() {
    for kind in [
        Unreadable::Syntax,
        Unreadable::TooManyScalars,
        Unreadable::Range,
    ] {
        assert!(kind.keeps_value(), "{kind:?}");
    }
    for kind in [Unreadable::Blank, Unreadable::TooLong, Unreadable::Control] {
        assert!(!kind.keeps_value(), "{kind:?}");
    }
}

/// Slice 1.3, decision on slice-1.1 note V3: a sign-led entry with more terms ("-5+3") is a
/// sign-led sum. D-Q applies it literally (its own value, −2), and the switch offers it
/// relative to the saved value ("84 − 5 + 3 = 82"). The sign must still touch its digits.
#[test]
fn sign_led_sums_are_applied_literally() {
    ok(&[
        ("-5+3", -2),
        ("+5-1", 4),
        ("\u{2212}5 \u{2212} 3", -8),
        ("-5 + 3 - 1", -3),
        ("+0-0", 0),
        ("-9223372036854775807-1", i64::MIN),
    ]);
    refused(
        &["-9223372036854775808-1", "+9223372036854775807+1"],
        Unreadable::Range,
    );
    refused(
        &["-5+-3", "- 5+3", "-5 3", "--5+1", "-5+"],
        Unreadable::Syntax,
    );
    // The 64-number cap counts the first signed term.
    let sixty_five = format!("-1{}", "+1".repeat(MAX_SCALARS));
    assert_eq!(number(&sixty_five), Err(Unreadable::TooManyScalars));
    let sixty_four = format!("-1{}", "+1".repeat(MAX_SCALARS - 1));
    assert_eq!(number(&sixty_four), Ok(62));
}

/// Slice 1.3, decision on the slice-1.1 review (row 5): a no-break space (U+00A0, and the
/// narrow U+202F that some keyboards and pasted text use) counts as a space: it is trimmed
/// and may separate terms. Other Unicode spaces stay unreadable.
#[test]
fn no_break_spaces_count_as_spaces() {
    ok(&[
        ("\u{a0}5", 5),
        ("5\u{a0}", 5),
        ("5\u{a0}-1", 4),
        ("84\u{a0}-\u{a0}15", 69),
        ("84\u{202f}-15", 69),
        ("\u{a0}\t-15\u{a0}", -15),
    ]);
    refused(&["\u{a0}", "\u{202f}\u{a0}\t"], Unreadable::Blank);
    refused(&["1\u{a0}2", "-\u{a0}5", "5\u{2003}-1"], Unreadable::Syntax);
}

/// Slice 1.3: the switch text of a sign-led entry over a saved value, and sums shown with a
/// sign-led first term.
#[test]
fn offers_spell_the_relative_reading() {
    assert_eq!(offer_text(84, "-15").as_deref(), Some("84 \u{2212} 15"));
    assert_eq!(offer_text(84, "+3").as_deref(), Some("84 + 3"));
    assert_eq!(
        offer_text(84, " -5+03 ").as_deref(),
        Some("84 \u{2212} 5 + 3")
    );
    assert_eq!(
        offer_text(-1, "-1").as_deref(),
        Some("\u{2212}1 \u{2212} 1")
    );
    assert_eq!(offer_text(0, "\u{2212}7").as_deref(), Some("0 \u{2212} 7"));
    for raw in ["84", "84 -15", "lots", "", "- 5"] {
        assert_eq!(offer_text(84, raw), None, "{raw:?}");
    }
    assert_eq!(sum_text("-5+3").as_deref(), Some("\u{2212}5 + 3"));
    assert_eq!(sum_text("+5-1").as_deref(), Some("5 \u{2212} 1"));
    assert_eq!(sum_text("-15"), None);
}
