//! Shared text-folding primitives (DEP-31 and DEP-19 crates, no new dependency), and the
//! catalogue name rule built on them.
//!
//! The primitives are single steps: an NFKC step, a full case-fold step (`unicase`), a
//! mark-stripping step (NFD, then drop `Mn`) and a whitespace collapse. Each caller composes
//! its own rule from them: `sources::upload_exclusions::normalize` (repeated-text matching),
//! `assistant::lexer::fold` (the fraction words) and [`name_norm`] (catalogue entry names,
//! linking on view). The first two keep their exact behaviour; only the name rule is new.
use std::fmt::Write as _;
use unicode_general_category::{GeneralCategory, get_general_category};
use unicode_normalization::UnicodeNormalization;

/// The name rule's own number. Bump it when [`name_norm`] changes for any input; a change of
/// the Unicode tables moves [`name_fold_version`] on its own (the marker of
/// `catalogue_entry_names`, rebuilt at store open when it differs).
pub(crate) const NAME_FOLD_RULE: u32 = 1;

/// The marker stored beside the derived name rows: the rule number and the Unicode version of
/// the normalisation tables (`unicode-normalization`, whose `unicase` sibling is pinned with
/// it, see the test that ties both pins to this marker).
pub(crate) fn name_fold_version() -> String {
    let (major, minor, update) = unicode_normalization::UNICODE_VERSION;
    let mut marker = format!("name-fold-{NAME_FOLD_RULE}-unicode-{major}.{minor}");
    if update != 0 {
        let _ = write!(marker, ".{update}");
    }
    marker
}

/// NFKC.
pub(crate) fn nfkc(text: &str) -> String {
    text.nfkc().collect()
}

/// Full Unicode case folding (`ß` becomes `ss`).
pub(crate) fn full_fold(text: &str) -> String {
    unicase::UniCase::unicode(text).to_folded_case()
}

/// Decomposed (NFD) with the nonspacing marks (`Mn`) removed.
pub(crate) fn strip_marks(text: &str) -> String {
    text.nfd()
        .filter(|&c| get_general_category(c) != GeneralCategory::NonspacingMark)
        .collect()
}

/// Whitespace runs become one space, the ends are trimmed.
pub(crate) fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Unicode `Default_Ignorable_Code_Point` ranges (`DerivedCoreProperties`, as of Unicode 17;
/// the set grows only by newly assigned characters, such as U+061C in 6.3 and U+180F in 14.0).
const DEFAULT_IGNORABLE: [(u32, u32); 17] = [
    (0x00AD, 0x00AD),
    (0x034F, 0x034F),
    (0x061C, 0x061C),
    (0x115F, 0x1160),
    (0x17B4, 0x17B5),
    (0x180B, 0x180F),
    (0x200B, 0x200F),
    (0x202A, 0x202E),
    (0x2060, 0x206F),
    (0x3164, 0x3164),
    (0xFE00, 0xFE0F),
    (0xFEFF, 0xFEFF),
    (0xFFA0, 0xFFA0),
    (0xFFF0, 0xFFF8),
    (0x1BCA0, 0x1BCA3),
    (0x1D173, 0x1D17A),
    (0xE0000, 0xE0FFF),
];

/// A default-ignorable code point or a format character (`Cf`).
fn invisible(c: char) -> bool {
    let code = u32::from(c);
    DEFAULT_IGNORABLE
        .iter()
        .any(|&(low, high)| (low..=high).contains(&code))
        || get_general_category(c) == GeneralCategory::Format
}

/// The catalogue name rule: remove default-ignorable code points (`Cf` included), NFKC, full
/// fold, NFD and drop `Mn`, NFC, collapse whitespace, trim. Two names that read alike to a
/// player have one norm; the cell's name and the entry's name are compared only through it.
pub(crate) fn name_norm(name: &str) -> String {
    let visible: String = name.chars().filter(|&c| !invisible(c)).collect();
    let folded = full_fold(&nfkc(&visible));
    collapse_whitespace(&strip_marks(&folded).nfc().collect::<String>())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_fold_case_width_diacritics_and_whitespace() {
        assert_eq!(name_norm("Straße"), name_norm("STRASSE"));
        assert_eq!(name_norm("Straße"), "strasse");
        assert_eq!(name_norm("Épée  de\tFeu"), "epee de feu");
        assert_eq!(name_norm("e\u{301}pe\u{301}e de feu"), "epee de feu");
        assert_eq!(name_norm("ＳＷＯＲＤ"), "sword");
        assert_eq!(name_norm("ﬁre bolt"), "fire bolt");
        assert_eq!(name_norm("  Rope \n "), "rope");
        assert_eq!(name_norm("\u{a0}Rope\u{2003}ladder"), "rope ladder");
        assert_eq!(name_norm("İstanbul"), "istanbul");
    }

    #[test]
    fn invisible_characters_never_make_a_name_differ() {
        for hidden in [
            '\u{ad}',
            '\u{34f}',
            '\u{200b}',
            '\u{200d}',
            '\u{200e}',
            '\u{2060}',
            '\u{fe0f}',
            '\u{feff}',
            '\u{e0020}',
            '\u{600}',
            '\u{180e}',
        ] {
            assert_eq!(
                name_norm(&format!("Sw{hidden}ord")),
                "sword",
                "{:x}",
                u32::from(hidden)
            );
        }
        assert_eq!(name_norm("\u{200b}\u{feff}"), "");
    }

    #[test]
    fn scripts_whose_marks_carry_meaning_lose_them() {
        // Recorded limit (docs/limitations.md): dropping Mn merges names in such scripts.
        assert_eq!(name_norm("\u{905}\u{901}"), name_norm("\u{905}"));
        assert_eq!(name_norm("\u{5e9}\u{5c1}"), name_norm("\u{5e9}"));
    }

    #[test]
    fn primitives_compose_the_two_older_rules_unchanged() {
        assert_eq!(strip_marks("Éa\u{301}"), "Ea");
        assert_eq!(full_fold("Σς"), "σσ");
        assert_eq!(collapse_whitespace("  a \t b\n"), "a b");
        assert_eq!(nfkc("ﬃ"), "ffi");
    }

    #[test]
    fn the_marker_follows_the_rule_number_and_the_pinned_unicode_tables() {
        // A crate update that changes the tables must change the marker: the pins below are
        // the versions the marker was reviewed against. Bump the rule number when `name_norm`
        // changes; update these pins (and re-check the fold tests) with any crate update.
        let lock = include_str!("../Cargo.lock");
        let version_of = |package: &str| {
            let at = lock
                .find(&format!("name = \"{package}\"\nversion = \""))
                .expect("a locked package");
            let rest = &lock[at..];
            let start = rest.find("version = \"").unwrap() + "version = \"".len();
            rest[start..start + rest[start..].find('"').unwrap()].to_owned()
        };
        assert_eq!(version_of("unicase"), "2.10.0");
        assert_eq!(version_of("unicode-normalization"), "0.1.25");
        // Its `Mn` and `Cf` tables decide what `name_norm` removes.
        assert_eq!(version_of("unicode-general-category"), "1.1.0");
        assert_eq!(name_fold_version(), "name-fold-1-unicode-17.0");
        assert!(
            include_str!("../Cargo.toml").contains("unicase = \"=2.10.0\""),
            "unicase must stay exactly pinned"
        );
    }
}
