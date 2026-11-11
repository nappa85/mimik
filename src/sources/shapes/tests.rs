//! Synthetic fixtures only. Markup fixtures go through the same html2text renderer as the import
//! (`raw_text`), so the decoration the shapes strip is the real one.
use std::fmt::Write as _;

use proptest::prelude::*;

use super::*;

fn input<S: AsRef<str>>(texts: &[S]) -> ShapeInput {
    ShapeInput::new(texts)
}

fn rendered(html: &str) -> String {
    html2text::from_read(html.as_bytes(), 120).unwrap()
}

fn table(texts: &[&str], shape: Shape) -> Vec<Vec<String>> {
    read(&input(texts), shape).rows
}

/// Whether the shape read any rows (it does not fit otherwise).
fn fits<S: AsRef<str>>(texts: &[S], shape: Shape) -> bool {
    !read(&input(texts), shape).rows.is_empty()
}

fn cells(rows: &[&[&str]]) -> Vec<Vec<String>> {
    rows.iter()
        .map(|r| r.iter().map(|c| (*c).to_owned()).collect())
        .collect()
}

const D6: &str = "1 Rats\n2 Goblins\n3 Ogres\n4 Wolves\n5 Bandits\n6 A dragon";

#[test]
fn roll_clean_cover_reads_every_line_as_a_row() {
    let draft = read(&input(&[D6]), Shape::Roll);
    assert_eq!(draft.score, 100);
    assert!(!draft.header && draft.problems.is_empty());
    assert_eq!(draft.rows.len(), 6);
    assert_eq!(draft.rows[2], ["3", "Ogres"]);
    assert_eq!(best(&input(&[D6]))[0].shape, Shape::Roll);
}

#[test]
fn roll_markers_accept_punctuation_ranges_and_plus() {
    let rows = table(
        &["1. Rats\n2) Goblins\n3: Ogres\n4-5 Wolves\n6+ A dragon"],
        Shape::Roll,
    );
    // Not a weak list: ranges and a plus prove the die.
    assert_eq!(rows.len(), 5);
    assert_eq!(rows[3], ["4-5", "Wolves"]);
    assert_eq!(rows[4], ["6+", "A dragon"]);
    let dashes = table(&["1\u{2013}2 a\n3\u{2014}4 b\n5-6 c"], Shape::Roll);
    assert_eq!(dashes[1][0], "3\u{2014}4");
}

#[test]
fn roll_header_line_fixes_the_die_and_becomes_the_header_row() {
    let draft = read(
        &input(&["d6 Result\n1 Rats\n2 Goblins\n3 Ogres"]),
        Shape::Roll,
    );
    assert!(draft.header);
    assert_eq!(draft.rows[0], ["d6", "Result"]);
    // The header's die expects 6: the table stops early.
    assert_eq!(draft.problems, [Problem::Incomplete(3, 6)]);
    assert_eq!(draft.score, 80);
    let bare = read(
        &input(&["1d20\n1-5 a\n6-10 b\n11-15 c\n16-20 d"]),
        Shape::Roll,
    );
    assert!(bare.header);
    assert_eq!(bare.rows[0], ["1d20", ""]);
    assert_eq!(bare.score, 100);
}

#[test]
fn roll_continuation_lines_join_the_last_cell() {
    let rows = table(
        &["1 Rats\nand more rats\n2 Goblins\n3 Ogres\n4 Wolves"],
        Shape::Roll,
    );
    assert_eq!(rows[0], ["1", "Rats and more rats"]);
    assert_eq!(rows.len(), 4);
}

#[test]
fn roll_caption_is_a_single_cell_row_before_a_header_row() {
    let plain = read(&input(&[&format!("Random encounters\n{D6}")]), Shape::Roll);
    assert!(!plain.header);
    assert_eq!(plain.rows[0], ["Random encounters", ""]);
    assert_eq!(plain.rows.len(), 7);
    let headed = read(
        &input(&[&format!("Random encounters\nd6 Result\n{D6}")]),
        Shape::Roll,
    );
    // The caption keeps its place in the cells, so the header is no longer the first row.
    assert!(!headed.header);
    assert_eq!(headed.rows[0], ["Random encounters", ""]);
    assert_eq!(headed.rows[1], ["d6", "Result"]);
    assert_eq!(headed.rows.len(), 8);
}

#[test]
fn roll_gap_duplicate_overlap_and_interleave_are_problems_not_fits() {
    let gap = read(&input(&["1 a\n2 b\n3 c\n5 d\n6 e"]), Shape::Roll);
    assert_eq!(gap.problems, [Problem::Gap(4)]);
    assert_eq!(gap.score, 80);
    let dup = read(&input(&["1 a\n2 b\n2 c\n3 d\n4 e\n5 f\n6 g"]), Shape::Roll);
    assert_eq!(dup.problems, [Problem::Duplicate(3)]);
    let overlap = read(&input(&["1-2 a\n2-3 b\n4-6 c"]), Shape::Roll);
    assert_eq!(overlap.problems, [Problem::Overlap(2)]);
    let two = "1 a\n1 A\n2 b\n2 B\n3 c\n3 C\n4 d\n4 D\n5 e\n5 E\n6 f\n6 F";
    let side = read(&input(&[two]), Shape::Roll);
    assert!(
        side.score <= PLAIN_SCORE,
        "interleaved tables never fit: {}",
        side.score
    );
    assert_eq!(best(&input(&[two]))[0].shape, Shape::Plain);
    let back = read(&input(&["1 a\n2 b\n3 c\n4 d\n2 e\n5 f\n6 g"]), Shape::Roll);
    assert!(
        back.problems
            .iter()
            .any(|p| matches!(p, Problem::Interleaved(_) | Problem::Overlap(_)))
    );
}

#[test]
fn roll_covers_d100_with_00_d66_and_sums() {
    let d100 = table(&["01-25 a\n26-50 b\n51-75 c\n76-00 d"], Shape::Roll);
    assert_eq!(d100.len(), 4);
    assert_eq!(d100[3], ["76-00", "d"]);
    assert_eq!(
        read(&input(&["01-25 a\n26-50 b\n51-75 c\n76-00 d"]), Shape::Roll).score,
        100
    );
    let mut d66 = String::new();
    for tens in 1..=6 {
        for units in 1..=6 {
            let _ = writeln!(d66, "{tens}{units} row");
        }
    }
    let draft = read(&input(&[&d66]), Shape::Roll);
    assert_eq!((draft.rows.len(), draft.score), (36, 100));
    let sum = read(
        &input(&["2-3 a\n4-5 b\n6-8 c\n9-10 d\n11-12 e"]),
        Shape::Roll,
    );
    assert_eq!((sum.rows.len(), sum.score), (5, 100));
    let headed = read(
        &input(&["2d6 Result\n2-3 a\n4-5 b\n6-8 c\n9-10 d\n11-12 e"]),
        Shape::Roll,
    );
    assert!(headed.header);
    assert_eq!(headed.score, 100);
    let three = read(&input(&["3-5 a\n6-10 b\n11-14 c\n15-18 d"]), Shape::Roll);
    assert_eq!(three.score, 100);
}

#[test]
fn roll_outcome_splits_on_tabs_or_wide_gaps_when_every_row_agrees() {
    let split = table(
        &["1\tRats\td4\n2  Goblins  d6\n3\tOgres\td8\n4\tWolves\td10"],
        Shape::Roll,
    );
    assert_eq!(split[1], ["2", "Goblins", "d6"]);
    let mixed = table(&["1\tRats\td4\n2 Goblins\n3 Ogres\n4 Wolves"], Shape::Roll);
    assert_eq!(mixed[0], ["1", "Rats\td4"]);
}

#[test]
fn html2text_decoration_is_stripped_and_an_ordered_list_is_a_weak_marker() {
    let bold = rendered(
        "<p><strong>1</strong> Rats</p><p><strong>2</strong> Goblins</p><p><strong>3</strong> Ogres</p><p><strong>4</strong> Wolves</p>",
    );
    assert!(
        bold.contains("**1**"),
        "fixture is the real renderer output: {bold}"
    );
    assert_eq!(table(&[&bold], Shape::Roll)[0], ["1", "Rats"]);
    let bullets =
        rendered("<ul><li>1 Rats</li><li>2 Goblins</li><li>3 Ogres</li><li>4 Wolves</li></ul>");
    assert!(bullets.contains("* 1 Rats"), "{bullets}");
    assert_eq!(table(&[&bullets], Shape::Roll).len(), 4);
    let emphasis = rendered(
        "<p><em>1</em> Rats</p><p><em>2</em> Goblins</p><p><em>3</em> Ogres</p><p><em>4</em> Wolves</p>",
    );
    assert_eq!(table(&[&emphasis], Shape::Roll)[1], ["2", "Goblins"]);
    let heading =
        rendered("<h2>Encounters</h2><p>1 Rats</p><p>2 Goblins</p><p>3 Ogres</p><p>4 Wolves</p>");
    assert!(heading.starts_with("## "), "{heading}");
    assert_eq!(table(&[&heading], Shape::Roll)[0], ["Encounters", ""]);

    let ordered = rendered(
        "<ol><li>Rats</li><li>Goblins</li><li>Ogres</li><li>Wolves</li><li>Bandits</li><li>A dragon</li></ol>",
    );
    assert!(ordered.contains("1. Rats"), "{ordered}");
    // Alone, a six-item list is not a d6 ...
    assert!(!fits(&[&ordered], Shape::Roll));
    assert_eq!(best(&input(&[&ordered]))[0].shape, Shape::Plain);
    // ... with a dN header line it is.
    let draft = read(&input(&["d6 Result", &ordered]), Shape::Roll);
    assert!(draft.header);
    assert_eq!(draft.rows.len(), 7);
    assert_eq!(draft.score, 100);
}

#[test]
fn roll_reads_a_wrapped_merged_paragraph_by_token_stream() {
    let items = [
        "Rats swarm out of the drain pipes and gnaw at boots",
        "Goblins sneak past with a stolen lantern and a sack",
        "Ogres argue loudly over a half cooked lamb and a pot",
        "Wolves circle the camp at dusk and watch the fire",
        "Bandits demand a toll for the bridge and a song",
        "A dragon sleeps on a hill of coins and old bones",
    ];
    let paragraph: String = items
        .iter()
        .enumerate()
        .map(|(i, text)| format!("{} {text}", i + 1))
        .collect::<Vec<_>>()
        .join(" ");
    let text = rendered(&format!("<p>{paragraph}</p>"));
    assert!(
        text.lines().count() >= 3,
        "html2text wraps at 120 columns: {text}"
    );
    let draft = read(&input(&[&text]), Shape::Roll);
    assert_eq!(draft.rows.len(), 6);
    assert_eq!(draft.rows[0], ["1", items[0]]);
    assert_eq!(draft.rows[5], ["6", items[5]]);
    assert_eq!(draft.score, 90, "inline reads are capped below a line read");
    assert_eq!(draft.problems, []);
    assert_eq!(best(&input(&[&text]))[0].shape, Shape::Roll);
    // Several units of one merged table read as one stream.
    let split = read(
        &input(&[
            "Random encounters 1 Rats 2 Goblins",
            "3 Ogres 4 Wolves 5 Bandits 6 A dragon",
        ]),
        Shape::Roll,
    );
    assert_eq!(split.rows.len(), 7);
    assert_eq!(split.rows[0][0], "Random encounters");
}

#[test]
fn roll_near_misses_lose_to_plain_columns() {
    let misses: [&str; 6] = [
        "2 hours later the goblins arrive\nThey are hungry",
        "12",
        "Chapter 3\n14\nThe road goes on",
        "1 See page 3\n2 Ask the referee",
        "1 hour passes\n2 hours pass\n3 days pass",
        "On day 1 the party rests. On day 2 it travels. On day 3 it arrives. On day 4 it fights.",
    ];
    for text in misses {
        let drafts = best(&input(&[text]));
        assert_ne!(drafts[0].shape, Shape::Roll, "{text}");
        assert!(!fits(&[text], Shape::Roll), "{text}");
    }
}

#[test]
fn label_value_reads_leaders_dashes_colons_and_tabs() {
    let text = "Items\nSword ........ 10 gp\nShield \u{2026}\u{2026} 5 gp\nRope - 1 gp\nTorch: 1 sp\nLantern\t5 sp\nand oil";
    let draft = read(&input(&[text]), Shape::Label);
    assert_eq!(draft.rows[0], ["Items", ""]);
    assert_eq!(draft.rows[1], ["Sword", "10 gp"]);
    assert_eq!(draft.rows[2], ["Shield", "5 gp"]);
    assert_eq!(draft.rows[3], ["Rope", "1 gp"]);
    assert_eq!(draft.rows[4], ["Torch", "1 sp"]);
    assert_eq!(draft.rows[5], ["Lantern", "5 sp and oil"]);
    assert!(draft.score > PLAIN_SCORE && draft.score <= 85);
    assert_eq!(best(&input(&[text]))[0].shape, Shape::Label);
}

#[test]
fn label_value_near_misses_lose_to_plain_columns() {
    let one = "Note: do this";
    assert!(!fits(&[one], Shape::Label));
    let prose = "The goblin chief, who had waited long and patiently by the old well, said: it is time\nHe went home.\nNothing happened.\nThe end.";
    assert!(!fits(&[prose], Shape::Label));
    let sentences = "Name: Grunt\nA short sentence.\nAnother one follows.\nAnd a third.\nAnd more.";
    assert_eq!(best(&input(&[sentences]))[0].shape, Shape::Plain);
}

#[test]
fn stat_line_reads_pairs_dice_and_colon_sequences() {
    let single = read(&input(&["Str 12 Dex +1 HP 2d6 AC -2"]), Shape::Stat);
    assert!(!single.header);
    assert_eq!(
        single.rows,
        cells(&[
            &["Str", "12"],
            &["Dex", "+1"],
            &["HP", "2d6"],
            &["AC", "-2"]
        ])
    );
    let many = read(
        &input(&["Goblin\nStr 12 Dex 14\nStr 8 Dex 10\nStr 9 Dex 11"]),
        Shape::Stat,
    );
    assert!(!many.header, "the caption row comes first");
    assert_eq!(many.rows[0], ["Goblin", ""]);
    assert_eq!(many.rows[1], ["Str", "Dex"]);
    assert_eq!(many.rows[3], ["8", "10"]);
    let headed = read(&input(&["Str 12 Dex 14\nStr 8 Dex 10"]), Shape::Stat);
    assert!(headed.header);
    assert_eq!(headed.rows[0], ["Str", "Dex"]);
    let colon = read(&input(&["Morale: 7, HP: 4; Attack: 1d6"]), Shape::Stat);
    assert_eq!(
        colon.rows,
        cells(&[&["Morale", "7"], &["HP", "4"], &["Attack", "1d6"]])
    );
    let differing = read(&input(&["Str 12 Dex 14\nHP 3 AC 5"]), Shape::Stat);
    assert!(!differing.header);
    assert_eq!(differing.rows.len(), 4);
}

#[test]
fn stat_line_near_misses_are_not_stat_lines() {
    for text in [
        "Page 12 of 40",
        "Lord of Mist 3",
        "Str 12",
        "Str 12 Str 13",
        "Name: Grunt",
        "The goblin has 2 axes and 4 daggers today",
    ] {
        assert!(!fits(&[text], Shape::Stat), "{text}");
    }
    let mostly_prose = "Str 12 Dex 14\nthe goblin sneaks\naround the corner\nand waits";
    assert!(!fits(&[mostly_prose], Shape::Stat));
}

#[test]
fn progression_reads_ordinals_levels_and_a_header() {
    let text = "Level BAB Fort\n1st +0 +2\n2nd +1 +3\n3rd +2 +3\n4th +3 +4";
    let draft = read(&input(&[text]), Shape::Progression);
    assert!(draft.header);
    assert_eq!(draft.rows[0], ["Level", "BAB", "Fort"]);
    assert_eq!(draft.rows[2], ["2nd", "+1", "+3"]);
    assert_eq!(draft.score, 100);
    let words = read(
        &input(&["1st  +0  Sneak attack 1d6\n2nd  +1  Evasion\n3rd  +2  Uncanny dodge"]),
        Shape::Progression,
    );
    assert_eq!(words.rows[0], ["1st", "+0", "Sneak attack 1d6"]);
    let single_spaced = table(
        &["1st +0 Sneak attack 1d6\n2nd +1 Evasion \u{2014}\n3rd +2 Dodge 2"],
        Shape::Progression,
    );
    assert_eq!(single_spaced[0], ["1st", "+0", "Sneak attack", "1d6"]);
    assert_eq!(single_spaced[1], ["2nd", "+1", "Evasion", "\u{2014}"]);
    let italian = table(
        &["1\u{b0} +0 +2\n2\u{b0} +1 +3\n3\u{b0} +2 +3"],
        Shape::Progression,
    );
    assert_eq!(italian[1], ["2\u{b0}", "+1", "+3"]);
    let level = table(
        &["Livello 1 d8 +1\nLivello 2 d8 +2\nLevel 3 d10 +2"],
        Shape::Progression,
    );
    assert_eq!(level[0], ["Livello 1", "d8", "+1"]);
    let continued = table(
        &["1st +0 Sneak\nattack\n2nd +1 Evasion\n3rd +2 Dodge"],
        Shape::Progression,
    );
    assert_eq!(continued[0].last().unwrap(), "Sneak attack");
}

#[test]
fn progression_gaps_duplicates_ragged_rows_and_near_misses() {
    let gap = read(
        &input(&["1st +0\n2nd +1\n4th +3\n5th +4"]),
        Shape::Progression,
    );
    assert_eq!(gap.problems, [Problem::Gap(3)]);
    let dup = read(
        &input(&["1st +0\n2nd +1\n2nd +1\n3rd +2"]),
        Shape::Progression,
    );
    assert_eq!(dup.problems, [Problem::Duplicate(3)]);
    let ragged = read(
        &input(&["1st +0 +2\n2nd +1\n3rd +2 +3\n4th +3 +4"]),
        Shape::Progression,
    );
    assert_eq!(ragged.problems, [Problem::Ragged(2)]);
    for text in [
        "1st edition of the game",
        "1st +0\n2nd +1",
        "Read the 1st chapter\nthen the 2nd\nthen the 3rd",
    ] {
        assert!(
            read(&input(&[text]), Shape::Progression).rows.is_empty(),
            "{text}"
        );
    }
}

#[test]
fn plain_columns_is_one_column_per_unit_and_not_offered_above_sixteen_units() {
    let rows = table(&["Name\nSword\n\nBow", "Damage\nd6"], Shape::Plain);
    assert_eq!(
        rows,
        cells(&[&["Name", "Damage"], &["Sword", "d6"], &["Bow", ""]])
    );
    assert_eq!(read(&input(&["a", "b"]), Shape::Plain).score, PLAIN_SCORE);
    let many: Vec<&str> = vec!["x"; 17];
    // Above 16 units: one column, one row per line, never an empty editor.
    let draft = read(&input(&many), Shape::Plain);
    assert_eq!(draft.rows.len(), 17);
    assert_eq!(draft.rows[0], ["x"]);
    assert_eq!(draft.problems, [Problem::OneColumn]);
    assert_eq!(best(&input(&many))[0].rows.len(), 17);
    // 200 units of one line each: Plain is refused, Roll still reads them.
    let units: Vec<String> = (1..=20).map(|n| format!("{n} Result {n}")).collect();
    let draft = read(&input(&units), Shape::Roll);
    assert_eq!(draft.rows.len(), 20);
    assert_eq!(draft.score, 100);
    assert_eq!(best(&input(&units))[0].shape, Shape::Roll);
}

#[test]
fn drafts_that_cannot_be_saved_are_refused_with_a_problem() {
    let long = format!("{}\n{}", "a".repeat(1_001), "b");
    let draft = read(&input(&[&long]), Shape::Plain);
    assert_eq!(draft.problems, [Problem::CellTooLong]);
    let many = "x\n".repeat(201);
    assert_eq!(
        read(&input(&[&many]), Shape::Plain).problems,
        [Problem::TooManyRows]
    );
    let big: Vec<String> = (0..21).map(|_| "y".repeat(1_000)).collect();
    assert_eq!(
        read(&input(&[big.join("\n")]), Shape::Plain).problems,
        [Problem::TooLarge]
    );
    assert!(Problem::Gap(3).to_string().contains('3'));
}

#[test]
fn shape_keys_round_trip_and_ties_keep_the_fixed_order() {
    for shape in Shape::ALL {
        assert_eq!(Shape::from_key(shape.key()), Some(shape));
    }
    assert_eq!(Shape::from_key("other"), None);
    // Nothing fits but Plain.
    assert_eq!(best(&input(&["hello\nworld"]))[0].shape, Shape::Plain);
}

#[test]
fn normalise_strips_only_decoration() {
    assert_eq!(normalise("**1** Rats"), "1 Rats");
    assert_eq!(normalise("* 1 Rats"), "1 Rats");
    assert_eq!(normalise("- 1 Rats"), "1 Rats");
    assert_eq!(normalise("### Title"), "Title");
    assert_eq!(normalise("*1* Rats"), "1 Rats");
    assert_eq!(normalise("1. Rats"), "1. Rats");
    assert_eq!(normalise("#hashtag"), "#hashtag");
    assert_eq!(normalise("2 * 3"), "2 * 3");
}

fn within_bounds(draft: &Draft) {
    assert!(draft.score <= 100);
    assert!(draft.rows.len() <= MAX_ROWS);
    let width = draft.rows.first().map_or(0, Vec::len);
    assert!(width <= MAX_COLUMNS);
    assert!(draft.rows.iter().all(|row| row.len() == width));
    assert!(
        draft
            .rows
            .iter()
            .flatten()
            .all(|cell| cell.chars().count() <= MAX_CELL_CHARS)
    );
    if draft.rows.is_empty() {
        assert_eq!(draft.score, 0);
    } else {
        assert!(tables::check(&draft.rows, draft.header).is_ok());
    }
}

#[test]
fn adversarial_inputs_terminate_within_bounds() {
    let mut inputs: Vec<String> = vec![
        "1 ".repeat(32_000),
        (1..=12_000).fold(String::new(), |mut out, n| {
            let _ = write!(out, "{n} ");
            out
        }),
        "1\n".repeat(30_000),
        "d6\n".repeat(20_000),
        "Str 1 ".repeat(10_000),
        "1st ".repeat(15_000),
        ": ".repeat(30_000),
        "1-".repeat(30_000),
        "\t".repeat(60_000),
        "a  ".repeat(20_000),
        "...".repeat(20_000),
    ];
    inputs.push((1..=1000).fold(String::new(), |mut out, n| {
        let _ = writeln!(out, "{n}-{n} x");
        out
    }));
    for text in &inputs {
        let text = &text[..text.len().min(65_536)];
        for draft in best(&input(&[text])) {
            within_bounds(&draft);
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig {
        failure_persistence: None,
        ..ProptestConfig::with_cases(500)
    })]

    #[test]
    fn every_recogniser_terminates_within_bounds(
        units in proptest::collection::vec("[0-9dD+\u{2013}\u{2014}.:)(*#a-z\\- \t\n,;\u{b0}\u{2026}]{0,300}", 0..6),
    ) {
        for draft in best(&ShapeInput::new(&units)) {
            within_bounds(&draft);
        }
    }

    #[test]
    fn marker_heavy_text_never_panics(
        units in proptest::collection::vec("([0-9]{1,3}[-.:)+]? [A-Za-z ]{0,12}\n){0,60}", 0..4),
    ) {
        for draft in best(&ShapeInput::new(&units)) {
            within_bounds(&draft);
        }
    }

    #[test]
    fn no_word_of_the_units_leaves_the_cells(
        units in proptest::collection::vec("[0-9dD+\u{2013}.:)(*#a-zA-Z\\- \t\n,;\u{b0}\u{2026}]{0,200}", 0..5),
    ) {
        let shape_input = ShapeInput::new(&units);
        let words: Vec<String> = shape_input
            .units
            .iter()
            .flatten()
            .flat_map(|line| {
                line.split(|c: char| !c.is_alphanumeric())
                    .filter(|w| !w.is_empty())
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .collect();
        for draft in best(&shape_input).into_iter().filter(|d| !d.rows.is_empty()) {
            let text = draft.rows.iter().flatten().cloned().collect::<Vec<_>>().join(" ");
            for word in &words {
                prop_assert!(text.contains(word.as_str()), "{:?} lost {word:?}: {:?}", draft.shape, draft.rows);
            }
        }
    }

    #[test]
    fn arbitrary_text_stays_within_bounds(units in proptest::collection::vec(".{0,200}", 0..4)) {
        for draft in best(&ShapeInput::new(&units)) {
            within_bounds(&draft);
        }
    }
}

#[test]
fn lines_that_fit_no_row_stay_in_the_cells() {
    // Unplaced lines and roll rows before the cover starts are kept in full.
    let roll = read(
        &input(&["Page 12 of the book, a long unrelated sentence\n12 stray\n1 a\n2 b\n3 c\n4 d"]),
        Shape::Roll,
    );
    let text: String = roll
        .rows
        .iter()
        .flatten()
        .cloned()
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        text.contains("a long unrelated sentence") && text.contains("12 stray"),
        "{text}"
    );
    assert!(!roll.header);
    assert_eq!(roll.problems.len(), 1);
    let stat = read(
        &input(&["Str 12 Dex 14\nsome prose in the middle\nStr 8 Dex 10"]),
        Shape::Stat,
    );
    assert_eq!(stat.rows[2], ["some prose in the middle", ""]);
    let label = read(&input(&["A: 1\nB: 2\nC: 3"]), Shape::Label);
    assert_eq!(label.rows.len(), 3);
}

#[test]
fn prose_steps_and_title_lines_are_near_misses() {
    let steps = "Step 1) Open the door 2) Walk in 3) Close it 4) Leave";
    assert!(!fits(&[steps], Shape::Roll));
    assert_ne!(best(&input(&[steps]))[0].shape, Shape::Roll);
    let title = read(
        &input(&["Table 3 Fighter\n1st +0 +2\n2nd +1 +3\n3rd +2 +3"]),
        Shape::Progression,
    );
    assert!(!title.header, "a line with a number is not a header");
    assert_eq!(title.rows[0], ["Table 3 Fighter", "", ""]);
    assert_eq!(title.rows.len(), 4);
}
