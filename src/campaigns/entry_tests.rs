//! Slice 1.1 grammar table and property tests for `entry::number` (assisted sheet plan
//! §3a, §5 slice 1.1).
use super::{MAX_CHARS, MAX_SCALARS, Unreadable, number};
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
            "-5+3",
            "-9223372036854775808-1",
            "+5-1",
            "--5",
            "5\u{a0}-1",
            "\u{a0}5",
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
