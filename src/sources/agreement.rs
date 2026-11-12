//! The agreement rule (`agreement-v1`): whether a unit's text equals, word for word, the stored
//! words of the PDF text layer that lie inside its polygon. Pure and deterministic; an
//! agreement is never a decision and never evidence (SRC-6), it only tells the Owner which
//! units to look at first. See `docs/source-agreement-plan.md`.
use std::collections::HashMap;

use serde_json::Value;
use unicode_normalization::UnicodeNormalization;

use super::page_words::PageWord;
use super::table_words::Region;

/// The rule's version. A change to the word, token or verdict rules bumps it, and so does a
/// change to the worker's word splitter (stored rows carry no splitter version).
pub const RULE_VERSION: &str = "agreement-v1";

/// Two words with the same text whose four box edges are all within this many points are one
/// word drawn twice (a shadow duplicate); the later one is dropped.
/// A page over this many units, or this many bytes of unit text, is not computed.
pub const MAX_UNITS: usize = 2_000;
pub const MAX_TEXT_BYTES: usize = 64 * 1024;
const SHADOW_DISTANCE: f32 = 3.0;
/// The display diff is computed only when both sides have at most this many tokens.
const MAX_DIFF_TOKENS: usize = 2_000;
/// The display diff keeps this many changes.
const MAX_DIFF_CHANGES: usize = 20;
/// Kinds whose text is not prose and is never compared.
const EXCLUDED_KINDS: [&str; 8] = [
    "Table",
    "TableGroup",
    "Figure",
    "FigureGroup",
    "Picture",
    "PictureGroup",
    "Equation",
    "Form",
];
/// Characters that can end a line-broken word: hyphen-minus, hyphen, soft hyphen.
const LINE_HYPHENS: [char; 3] = ['-', '\u{2010}', '\u{ad}'];

// ---------------------------------------------------------------------------------------
// Provenance
// ---------------------------------------------------------------------------------------

/// What a source's second reading proves (the plan's "second reading").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadingKind {
    /// Units read from the page image; the text layer is a different reading.
    Independent,
    /// Units taken from the text layer itself.
    SameLayer,
    /// Units built from the stored words; agreement holds by construction.
    Derived,
    Unknown,
}

/// Extractor name the prototype tools record (`tools/extract_manual.py`, `"extractor"`).
pub const MARKER_NAME: &str = "marker-pdf";
/// Exact Marker versions known to record `force_ocr` / `disable_ocr` as the table expects.
/// Pinned by `tools/requirements-extraction.txt` (`marker-pdf==2.0.0`), which the prototype
/// writes as `extractor_version` through `importlib.metadata.version("marker-pdf")`.
pub const MARKER_VERSIONS: &[&str] = &["2.0.0"];
/// Route A (a source built from the PDF's own text, so from the stored words).
// TODO(SRC-21): no route A bundle writer exists yet, so the name and version it will declare
// are not known; this placeholder classifies nothing real until it does.
pub const ROUTE_A_NAME: &str = "mimik-route-a";
pub const ROUTE_A_VERSIONS: &[&str] = &["1"];

/// Classify a source by the extractor provenance its bundle declares (self-declared, L-SRC-4):
/// a pinned table keyed by name and exact version, anything else is `Unknown`.
#[must_use]
pub fn classify_provenance(name: &str, version: &str, settings: &Value) -> ReadingKind {
    if name == ROUTE_A_NAME && ROUTE_A_VERSIONS.contains(&version) {
        return ReadingKind::Derived;
    }
    if name == MARKER_NAME && MARKER_VERSIONS.contains(&version) {
        let flag = |key: &str| settings.get(key) == Some(&Value::Bool(true));
        let llm_off = settings
            .get("use_llm")
            .is_none_or(|v| *v == Value::Bool(false));
        return if flag("force_ocr") && !flag("disable_ocr") && llm_off {
            ReadingKind::Independent
        } else {
            ReadingKind::SameLayer
        };
    }
    ReadingKind::Unknown
}

// ---------------------------------------------------------------------------------------
// Input and output
// ---------------------------------------------------------------------------------------

/// Why a unit is not compared.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exclusion {
    /// An owner unit.
    Owner,
    /// Already reviewed (any state other than `unreviewed`).
    Reviewed,
    /// A kind that is not prose (tables, figures, pictures, equations, forms).
    Kind,
    /// Inside a table proposal (caller).
    TableProposal,
    /// Its text alone scores as a table shape (caller).
    TableShape,
    /// Past the proposals' text budget (caller).
    TextBudget,
    /// Targeted by an open suggestion (caller).
    OpenSuggestion,
    /// The page has an open page-level `missing_text` or `note` suggestion (caller).
    PageSuggestion,
    /// The unit has no text to compare.
    NoText,
    /// A word of the unit also lies inside another unit's polygon.
    SharedWord,
    /// An exclusion box drops words from, or reaches into, the unit's polygon.
    ExcludedArea,
}

/// One unit of the page. `text` is the text the table form draft uses (the raw text).
#[derive(Clone, Debug)]
pub struct UnitInput<'a> {
    pub id: i64,
    pub kind: &'a str,
    pub origin: &'a str,
    pub state: &'a str,
    /// JSON point list, in the words' space.
    pub polygon: &'a str,
    pub text: &'a str,
    /// A page-dependent exclusion decided by the caller.
    pub excluded: Option<Exclusion>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffReason {
    /// The token sequences differ.
    Tokens,
    /// The tokens are equal but a text-layer word in no unit lies next to the unit.
    OrphanWords,
}

/// One difference: the unit's tokens and the text layer's tokens at that place (either may be
/// empty), each joined with single spaces.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub unit: String,
    pub layer: String,
}

/// A bounded difference for display: the first changes only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diff {
    pub changes: Vec<Change>,
    pub truncated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    Agrees,
    /// `diff` is `None` when a side is over the display bound or the tokens are equal.
    Differs {
        reason: DiffReason,
        diff: Option<Diff>,
    },
    /// No stored word lies inside the unit (or its polygon is invalid).
    NoWords,
    Excluded(Exclusion),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnitVerdict {
    pub id: i64,
    pub verdict: Verdict,
}

/// Facts about the page as a whole.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PageFacts {
    /// Boxes (`[x0, y0, x1, y1]`) of the words inside no unit, for outlining on the page image.
    /// Boxes only: the word text of an orphan is not returned (a `Change` does carry the text
    /// layer's words of a differing unit).
    pub orphans: Vec<[f32; 4]>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Agreement {
    /// One verdict per input unit, in the input order.
    pub verdicts: Vec<UnitVerdict>,
    pub facts: PageFacts,
    /// False when the page is over the bounds and nothing was computed.
    pub computed: bool,
}

// ---------------------------------------------------------------------------------------
// Words
// ---------------------------------------------------------------------------------------

fn centre(word: &PageWord) -> (f64, f64) {
    let (x, y) = word.centre();
    (f64::from(x), f64::from(y))
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "stored boxes are bounded by the largest page edge"
)]
fn cell(value: f32) -> i32 {
    (value / SHADOW_DISTANCE).floor() as i32
}

/// The same text drawn again: every edge shifted by less than `min(3, 0.3 x` the smaller
/// height`)` and the boxes overlapping over at least half of the smaller one.
fn shadows(a: &PageWord, b: &PageWord) -> bool {
    let height = (a.bounds[3] - a.bounds[1]).min(b.bounds[3] - b.bounds[1]);
    let tolerance = SHADOW_DISTANCE.min(0.3 * height);
    if a.text != b.text
        || !a
            .bounds
            .iter()
            .zip(&b.bounds)
            .all(|(p, q)| (p - q).abs() < tolerance)
    {
        return false;
    }
    let overlap_x = (a.bounds[2].min(b.bounds[2]) - a.bounds[0].max(b.bounds[0])).max(0.0);
    let overlap_y = (a.bounds[3].min(b.bounds[3]) - a.bounds[1].max(b.bounds[1])).max(0.0);
    let area = |w: &PageWord| (w.bounds[2] - w.bounds[0]) * (w.bounds[3] - w.bounds[1]);
    overlap_x * overlap_y >= 0.5 * area(a).min(area(b))
}

/// Step 1: drop a word drawn again (see [`shadows`]) over an earlier one. Linear: words are
/// bucketed by text and a 3-point grid cell.
fn without_shadows<'a>(words: &[&'a PageWord]) -> Vec<&'a PageWord> {
    let mut grid: HashMap<(&str, i32, i32), Vec<&PageWord>> = HashMap::new();
    let mut kept = Vec::with_capacity(words.len());
    for &word in words {
        let (cx, cy) = (cell(word.bounds[0]), cell(word.bounds[1]));
        let duplicate = (-1..=1).any(|dx| {
            (-1..=1).any(|dy| {
                grid.get(&(word.text.as_str(), cx + dx, cy + dy))
                    .is_some_and(|bucket| bucket.iter().any(|other| shadows(other, word)))
            })
        });
        if !duplicate {
            grid.entry((word.text.as_str(), cx, cy))
                .or_default()
                .push(word);
            kept.push(word);
        }
    }
    kept
}

/// Step 3: group into lines by vertical overlap over half the smaller height, lines top to
/// bottom, words left to right. Stream order is not used.
fn lines_of(mut words: Vec<&PageWord>) -> Vec<Vec<&PageWord>> {
    words.sort_by(|a, b| {
        a.bounds[1]
            .total_cmp(&b.bounds[1])
            .then(a.bounds[0].total_cmp(&b.bounds[0]))
    });
    let mut lines: Vec<(f32, f32, Vec<&PageWord>)> = Vec::new();
    for word in words {
        let (y0, y1) = (word.bounds[1], word.bounds[3]);
        match lines.last_mut() {
            Some((top, bottom, line))
                if y1.min(*bottom) - y0.max(*top) > 0.5 * (y1 - y0).min(*bottom - *top) =>
            {
                *top = top.min(y0);
                *bottom = bottom.max(y1);
                line.push(word);
            }
            _ => lines.push((y0, y1, vec![word])),
        }
    }
    lines
        .into_iter()
        .map(|(_, _, mut line)| {
            line.sort_by(|a, b| {
                a.bounds[0]
                    .total_cmp(&b.bounds[0])
                    .then(a.bounds[1].total_cmp(&b.bounds[1]))
            });
            line
        })
        .collect()
}

// ---------------------------------------------------------------------------------------
// Tokens
// ---------------------------------------------------------------------------------------

/// The explicit fold table, and nothing else.
fn fold(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\u{fb00}' => out.push_str("ff"),
            '\u{fb01}' => out.push_str("fi"),
            '\u{fb02}' => out.push_str("fl"),
            '\u{fb03}' => out.push_str("ffi"),
            '\u{fb04}' => out.push_str("ffl"),
            '\u{fb05}' | '\u{fb06}' => out.push_str("st"),
            '\u{2018}' | '\u{2019}' | '\u{201a}' | '\u{201b}' | '\u{2032}' | '\u{2bc}' => {
                out.push('\'');
            }
            '\u{201c}' | '\u{201d}' | '\u{201e}' | '\u{201f}' | '\u{2033}' => out.push('"'),
            '\u{2010}' | '\u{2011}' | '\u{2013}' | '\u{2014}' => out.push('-'),
            '\u{2026}' => out.push_str("..."),
            c if c.is_whitespace() => out.push(' '),
            c => out.push(c),
        }
    }
    out
}

/// A list-bullet glyph the text layer may draw where html2text wrote a `* ` list marker.
const BULLET_GLYPHS: [&str; 3] = ["\u{2022}", "\u{25e6}", "\u{25aa}"];

/// A line's html2text prefix: `#`..`######` and a space (heading), or `* ` (list item).
/// Returns the rest and whether a list marker was removed. `- ` and a bare `-` are never
/// removed, nor is an ordered marker (`1. `), which the text layer draws too.
fn strip_prefix_markup(line: &str) -> (&str, bool) {
    let trimmed = line.trim_start();
    let hashes = trimmed.bytes().take_while(|b| *b == b'#').count();
    if (1..=6).contains(&hashes)
        && let Some(rest) = trimmed[hashes..].strip_prefix(' ')
        && !rest.trim().is_empty()
    {
        return (rest, false);
    }
    match trimmed.strip_prefix("* ") {
        Some(rest) if !rest.trim().is_empty() => (rest, true),
        _ => (line, false),
    }
}

/// Remove paired emphasis (`*x*`, `**x**`) from whitespace-separated tokens: the opener starts
/// a token and is followed by a non-space, non-`*` character; the closer ends a later token
/// (optionally before trailing punctuation) after a non-`*` character. An unpaired `*` is
/// literal text and stays, so the unit then differs from the text layer.
fn strip_emphasis(tokens: &mut [String]) {
    const TRAILING: [char; 7] = ['.', ',', ';', ':', '!', '?', ')'];
    let closes = |token: &str, marker: &str| {
        token
            .trim_end_matches(TRAILING)
            .strip_suffix(marker)
            .is_some_and(|rest| rest.chars().next_back().is_some_and(|c| c != '*'))
    };
    // next_close[m][j]: the first token at or after j that can close marker m (0: `**`, 1: `*`),
    // from one backward pass, so every opener finds its closer in constant time.
    let mut next_close = vec![[usize::MAX; 2]; tokens.len() + 1];
    for j in (0..tokens.len()).rev() {
        next_close[j] = next_close[j + 1];
        for (m, marker) in ["**", "*"].into_iter().enumerate() {
            if closes(&tokens[j], marker) {
                next_close[j][m] = j;
            }
        }
    }
    let mut i = 0;
    while i < tokens.len() {
        let (m, marker) = if tokens[i].starts_with("**") {
            (0, "**")
        } else if tokens[i].starts_with('*') {
            (1, "*")
        } else {
            i += 1;
            continue;
        };
        let after = &tokens[i][marker.len()..];
        if after.chars().next().is_none_or(|c| c == '*') {
            i += 1;
            continue;
        }
        let end = if closes(after, marker) {
            i
        } else {
            next_close[i + 1][m]
        };
        if end == usize::MAX {
            i += 1;
            continue;
        }
        tokens[i].drain(..marker.len());
        let token = &mut tokens[end];
        let body_len = token.trim_end_matches(TRAILING).len();
        token.replace_range(body_len - marker.len()..body_len, "");
        i = end + 1;
    }
}

/// The unit's tokens and how many list markers were removed: html2text's own markup removed
/// on the unit side only, NFC, folds, split.
pub(super) fn unit_tokens(text: &str) -> (Vec<String>, usize) {
    let mut markers = 0;
    let lines: Vec<&str> = text
        .lines()
        .map(|line| {
            let (rest, marker) = strip_prefix_markup(line);
            markers += usize::from(marker);
            rest
        })
        .collect();
    let nfc: String = lines.join(" ").nfc().collect();
    let mut tokens: Vec<String> = fold(&nfc).split_whitespace().map(str::to_owned).collect();
    strip_emphasis(&mut tokens);
    tokens.retain(|t| !t.is_empty());
    (tokens, markers)
}

/// A text-layer token. A line-end hyphenation takes two words and matches the unit's token
/// joined without the hyphen or with it.
enum LayerToken {
    Word(String),
    Hyphenated {
        joined: String,
        kept: String,
        first: String,
        second: String,
    },
}

struct Flat {
    text: String,
    line_end: bool,
}

/// Whether `first` (at a line end) and `second` (the next line's first word) are one word
/// broken at the line end: a letter then a hyphen, then a lowercase letter, no digit anywhere.
fn hyphenation(first: &str, second: &str) -> Option<(String, String)> {
    let stem = first.strip_suffix(LINE_HYPHENS)?;
    if !stem.chars().next_back().is_some_and(char::is_alphabetic)
        || !second.chars().next().is_some_and(char::is_lowercase)
        || first
            .chars()
            .chain(second.chars())
            .any(|c| c.is_numeric() || c.is_whitespace())
    {
        return None;
    }
    let (stem, second) = (fold(stem), fold(second));
    Some((format!("{stem}{second}"), format!("{stem}-{second}")))
}

/// `bullets` is how many list markers the unit side lost: that many line-start bullet glyphs
/// of the text layer are dropped, no more.
fn layer_tokens(lines: &[Vec<&PageWord>], mut bullets: usize) -> Vec<LayerToken> {
    let mut flat: Vec<Flat> = Vec::new();
    for line in lines {
        for (i, word) in line.iter().enumerate() {
            if i == 0 && bullets > 0 && BULLET_GLYPHS.contains(&word.text.as_str()) {
                bullets -= 1;
                continue;
            }
            flat.push(Flat {
                text: word.text.nfc().collect(),
                line_end: i + 1 == line.len(),
            });
        }
    }
    let mut tokens = Vec::with_capacity(flat.len());
    let mut i = 0;
    while i < flat.len() {
        let word = &flat[i];
        if word.line_end
            && let Some(next) = flat.get(i + 1)
            && let Some((joined, kept)) = hyphenation(&word.text, &next.text)
        {
            tokens.push(LayerToken::Hyphenated {
                joined,
                kept,
                first: fold(&word.text),
                second: fold(&next.text),
            });
            i += 2;
        } else {
            // A word is split on whitespace like the unit's text (a no-break space).
            tokens.extend(
                fold(&word.text)
                    .split_whitespace()
                    .map(|part| LayerToken::Word(part.to_owned())),
            );
            i += 1;
        }
    }
    tokens
}

/// Linear equality of the unit's tokens and the text layer's.
fn equal(unit: &[String], layer: &[LayerToken]) -> bool {
    let at = |i: usize| unit.get(i).map(String::as_str);
    let mut i = 0;
    for token in layer {
        match token {
            LayerToken::Word(word) => {
                if at(i) != Some(word) {
                    return false;
                }
                i += 1;
            }
            LayerToken::Hyphenated {
                joined,
                kept,
                first,
                second,
            } => {
                if at(i) == Some(joined) || at(i) == Some(kept) {
                    i += 1;
                } else if at(i) == Some(first) && at(i + 1) == Some(second) {
                    i += 2;
                } else {
                    return false;
                }
            }
        }
    }
    i == unit.len()
}

fn display_tokens(layer: &[LayerToken]) -> Vec<&str> {
    let mut out = Vec::with_capacity(layer.len());
    for token in layer {
        match token {
            LayerToken::Word(word) => out.push(word.as_str()),
            LayerToken::Hyphenated { first, second, .. } => {
                out.push(first);
                out.push(second);
            }
        }
    }
    out
}

/// For display only: bounded in both input size and output size.
fn display_diff(unit: &[String], layer: &[LayerToken]) -> Option<Diff> {
    let layer = display_tokens(layer);
    if unit.len() > MAX_DIFF_TOKENS || layer.len() > MAX_DIFF_TOKENS {
        return None;
    }
    let unit: Vec<&str> = unit.iter().map(String::as_str).collect();
    let mut changes = Vec::new();
    let mut truncated = false;
    for op in similar::capture_diff_slices(similar::Algorithm::Myers, &unit, &layer) {
        let (old, new) = match op {
            similar::DiffOp::Equal { .. } => continue,
            similar::DiffOp::Delete {
                old_index, old_len, ..
            } => (old_index..old_index + old_len, 0..0),
            similar::DiffOp::Insert {
                new_index, new_len, ..
            } => (0..0, new_index..new_index + new_len),
            similar::DiffOp::Replace {
                old_index,
                old_len,
                new_index,
                new_len,
            } => (
                old_index..old_index + old_len,
                new_index..new_index + new_len,
            ),
        };
        if changes.len() == MAX_DIFF_CHANGES {
            truncated = true;
            break;
        }
        changes.push(Change {
            unit: unit[old].join(" "),
            layer: layer[new].join(" "),
        });
    }
    Some(Diff { changes, truncated })
}

// ---------------------------------------------------------------------------------------
// The rule
// ---------------------------------------------------------------------------------------

/// A candidate's words, geometry and the box an orphan must stay out of.
struct Candidate<'a> {
    lines: Vec<Vec<&'a PageWord>>,
    near: [f64; 4],
}

fn candidate<'a>(words: &[&'a PageWord], region: &Region) -> Candidate<'a> {
    let lines = lines_of(words.to_vec());
    let line_height = lines
        .iter()
        .map(|line| {
            let top = line.iter().map(|w| w.bounds[1]).fold(f32::MAX, f32::min);
            let bottom = line.iter().map(|w| w.bounds[3]).fold(f32::MIN, f32::max);
            f64::from(bottom - top)
        })
        .fold(0.0, f64::max);
    let mut near = region.bounds();
    for word in words {
        near[0] = near[0].min(f64::from(word.bounds[0]));
        near[1] = near[1].min(f64::from(word.bounds[1]));
        near[2] = near[2].max(f64::from(word.bounds[2]));
        near[3] = near[3].max(f64::from(word.bounds[3]));
    }
    Candidate {
        lines,
        near: [
            near[0] - line_height,
            near[1] - line_height,
            near[2] + line_height,
            near[3] + line_height,
        ],
    }
}

fn prefilter(unit: &UnitInput<'_>) -> Option<Exclusion> {
    if unit.origin != "extracted" {
        Some(Exclusion::Owner)
    } else if unit.state != "unreviewed" {
        Some(Exclusion::Reviewed)
    } else if EXCLUDED_KINDS.contains(&unit.kind) {
        Some(Exclusion::Kind)
    } else {
        unit.excluded
    }
}

/// Run the rule over one page. `words` are the decoded stored words (an invalid or missing row
/// is the caller's case: nothing to compare); `exclusion_polygons` are confirmed exclusion
/// boxes, whose words are left out of every unit and count.
#[must_use]
pub fn evaluate(
    units: &[UnitInput<'_>],
    words: &[PageWord],
    exclusion_polygons: &[&str],
) -> Agreement {
    let text_bytes: usize = units.iter().map(|u| u.text.len()).sum();
    if units.len() > MAX_UNITS || text_bytes > MAX_TEXT_BYTES {
        return Agreement {
            verdicts: units
                .iter()
                .map(|u| UnitVerdict {
                    id: u.id,
                    verdict: Verdict::Excluded(Exclusion::TextBudget),
                })
                .collect(),
            facts: PageFacts::default(),
            computed: false,
        };
    }
    let boxes: Vec<Region> = exclusion_polygons
        .iter()
        .filter_map(|p| Region::parse(p))
        .collect();
    let (dropped, live): (Vec<&PageWord>, Vec<&PageWord>) = words.iter().partition(|word| {
        let (x, y) = centre(word);
        boxes.iter().any(|region| region.contains(x, y))
    });
    let words = without_shadows(&live);
    let regions: Vec<Option<Region>> = units.iter().map(|u| Region::parse(u.polygon)).collect();

    // Which units each word's centre lies in, against every unit of the page.
    let mut inside: Vec<Vec<usize>> = vec![Vec::new(); units.len()];
    let mut shared = vec![false; units.len()];
    let mut orphans: Vec<&PageWord> = Vec::new();
    for (index, &word) in words.iter().enumerate() {
        let (x, y) = centre(word);
        let owners: Vec<usize> = regions
            .iter()
            .enumerate()
            .filter(|(_, region)| region.as_ref().is_some_and(|r| r.contains(x, y)))
            .map(|(i, _)| i)
            .collect();
        if owners.is_empty() {
            orphans.push(word);
        }
        for &owner in &owners {
            inside[owner].push(index);
            shared[owner] |= owners.len() > 1;
        }
    }
    let facts = PageFacts {
        orphans: orphans.iter().map(|w| w.bounds).collect(),
    };

    let verdicts = units
        .iter()
        .enumerate()
        .map(|(i, unit)| {
            let verdict = if let Some(reason) = prefilter(unit) {
                Verdict::Excluded(reason)
            } else {
                let (tokens, bullets) = unit_tokens(unit.text);
                if tokens.is_empty() {
                    Verdict::Excluded(Exclusion::NoText)
                } else if regions[i]
                    .as_ref()
                    .is_some_and(|region| touches_exclusion(region, &boxes, &dropped))
                {
                    Verdict::Excluded(Exclusion::ExcludedArea)
                } else if shared[i] {
                    Verdict::Excluded(Exclusion::SharedWord)
                } else {
                    let unit_words: Vec<&PageWord> = inside[i].iter().map(|&w| words[w]).collect();
                    match &regions[i] {
                        Some(region) if !unit_words.is_empty() => compare(
                            &tokens,
                            bullets,
                            &candidate(&unit_words, region),
                            &facts.orphans,
                        ),
                        _ => Verdict::NoWords,
                    }
                }
            };
            UnitVerdict {
                id: unit.id,
                verdict,
            }
        })
        .collect();
    Agreement {
        verdicts,
        facts,
        computed: true,
    }
}

/// Whether an exclusion box dropped a word from inside the unit's polygon or reaches into the
/// polygon's bounding box: either way the unit's words are not all there.
fn touches_exclusion(region: &Region, boxes: &[Region], dropped: &[&PageWord]) -> bool {
    let b = region.bounds();
    boxes.iter().any(|exclusion| {
        let e = exclusion.bounds();
        e[0] <= b[2] && e[2] >= b[0] && e[1] <= b[3] && e[3] >= b[1]
    }) || dropped.iter().any(|word| {
        let (x, y) = centre(word);
        region.contains(x, y)
    })
}

fn compare(
    tokens: &[String],
    bullets: usize,
    candidate: &Candidate<'_>,
    orphans: &[[f32; 4]],
) -> Verdict {
    let layer = layer_tokens(&candidate.lines, bullets);
    if !equal(tokens, &layer) {
        return Verdict::Differs {
            reason: DiffReason::Tokens,
            diff: display_diff(tokens, &layer),
        };
    }
    let n = candidate.near;
    // An orphan whose box meets the union box (polygon and words) grown by one line height.
    if orphans.iter().any(|o| {
        f64::from(o[0]) <= n[2]
            && f64::from(o[2]) >= n[0]
            && f64::from(o[1]) <= n[3]
            && f64::from(o[3]) >= n[1]
    }) {
        return Verdict::Differs {
            reason: DiffReason::OrphanWords,
            diff: None,
        };
    }
    Verdict::Agrees
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn word(text: &str, b: [f32; 4]) -> PageWord {
        PageWord {
            text: text.into(),
            bounds: b,
        }
    }

    fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> String {
        format!("[[{x0},{y0}],[{x1},{y0}],[{x1},{y1}],[{x0},{y1}]]")
    }

    /// Words laid out on lines 30 points apart, each 10 high and 40 wide, 50 apart.
    #[allow(clippy::cast_precision_loss)]
    fn layout(lines: &[&[&str]]) -> Vec<PageWord> {
        let mut out = Vec::new();
        for (row, line) in lines.iter().enumerate() {
            for (col, text) in line.iter().enumerate() {
                let (x, y) = (10.0 + 50.0 * col as f32, 20.0 + 30.0 * row as f32);
                out.push(word(text, [x, y, x + 40.0, y + 10.0]));
            }
        }
        out
    }

    fn unit<'a>(id: i64, polygon: &'a str, text: &'a str) -> UnitInput<'a> {
        UnitInput {
            id,
            kind: "Text",
            origin: "extracted",
            state: "unreviewed",
            polygon,
            text,
            excluded: None,
        }
    }

    /// One unit covering the whole page against `lines`.
    fn check(text: &str, lines: &[&[&str]]) -> Verdict {
        let polygon = rect(0.0, 0.0, 300_000.0, 3000.0);
        let result = evaluate(&[unit(1, &polygon, text)], &layout(lines), &[]);
        result.verdicts[0].verdict.clone()
    }

    fn differs(verdict: &Verdict) -> bool {
        matches!(verdict, Verdict::Differs { .. })
    }

    #[test]
    fn equal_words_agree_and_a_single_difference_does_not() {
        assert_eq!(check("alpha beta", &[&["alpha", "beta"]]), Verdict::Agrees);
        assert_eq!(
            check("alpha\nbeta", &[&["alpha"], &["beta"]]),
            Verdict::Agrees
        );
        for (text, layer) in [
            ("Alpha beta", ["alpha", "beta"]),
            ("alpha beta", ["alpha", "bet\u{e0}"]),
            ("alpha beta", ["alpha", "beta."]),
            ("alpha beta", ["beta", "alpha"]),
        ] {
            assert!(differs(&check(text, &[&layer])), "{text}");
        }
        assert!(differs(&check("alpha", &[&["alpha", "beta"]])));
        assert!(differs(&check("alpha beta", &[&["alpha"]])));
    }

    #[test]
    fn digit_sign_and_script_cases_differ_in_both_directions() {
        for (unit_text, layer) in [
            ("16", vec!["1\u{2013}", "6"]),
            ("1\u{2013} 6", vec!["16"]),
            ("2", vec!["-", "2"]),
            ("102", vec!["10\u{b2}"]),
            ("10\u{b2}", vec!["102"]),
            ("armatura1", vec!["armatura\u{b9}"]),
            ("armatura\u{b9}", vec!["armatura1"]),
            ("x\u{2212}2", vec!["x-2"]),
        ] {
            assert!(differs(&check(unit_text, &[&layer])), "{unit_text}");
        }
    }

    #[test]
    fn folds_are_the_listed_ones_and_normalisation_is_nfc() {
        for (unit_text, layer) in [
            ("fine office", vec!["\u{fb01}ne", "o\u{fb03}ce"]),
            ("l'uomo", vec!["l\u{2019}uomo"]),
            ("\"si\"", vec!["\u{201c}si\u{201d}"]),
            ("ex-post", vec!["ex\u{2010}post"]),
            ("a-b", vec!["a\u{2014}b"]),
            ("dopo...", vec!["dopo\u{2026}"]),
            ("caf\u{e9}", vec!["cafe\u{301}"]),
            ("a b", vec!["a\u{a0}b"]),
        ] {
            let result = check(unit_text, &[&layer]);
            assert_eq!(result, Verdict::Agrees, "{unit_text}");
        }
        assert!(differs(&check("\u{bd}", &[&["1/2"]])));
        assert!(differs(&check("1/2", &[&["\u{bd}"]])));
    }

    fn rendered(html: &str) -> String {
        html2text::from_read(html.as_bytes(), 120).unwrap()
    }

    #[test]
    fn html2text_markup_is_removed_from_the_unit_side_only() {
        assert_eq!(
            check("**bold** text", &[&["bold", "text"]]),
            Verdict::Agrees
        );
        assert_eq!(
            check("**two words**, then", &[&["two", "words,", "then"]]),
            Verdict::Agrees
        );
        assert_eq!(check("*em* text", &[&["em", "text"]]), Verdict::Agrees);
        assert_eq!(check("## Heading", &[&["Heading"]]), Verdict::Agrees);
        // The real renderer's output.
        let list = rendered("<ul><li>one</li><li>two</li></ul>");
        assert!(list.contains("* one"), "{list}");
        assert_eq!(check(&list, &[&["one"], &["two"]]), Verdict::Agrees);
        let bold = rendered("<p><strong>Rats</strong> and <em>owls</em></p>");
        assert_eq!(check(&bold, &[&["Rats", "and", "owls"]]), Verdict::Agrees);
        let heading = rendered("<h2>Title</h2>");
        assert_eq!(check(&heading, &[&["Title"]]), Verdict::Agrees);
    }

    #[test]
    fn a_list_bullet_glyph_is_dropped_only_where_a_list_marker_was() {
        assert_eq!(check("* item", &[&["\u{2022}", "item"]]), Verdict::Agrees);
        assert_eq!(
            check("* a\n* b", &[&["\u{25aa}", "a"], &["\u{25e6}", "b"]]),
            Verdict::Agrees
        );
        // No marker on the unit side: the glyph is a difference. One marker: one glyph.
        assert!(differs(&check("item", &[&["\u{2022}", "item"]])));
        assert!(differs(&check(
            "* a\nb",
            &[&["\u{2022}", "a"], &["\u{2022}", "b"]]
        )));
        // A lost marker on the text layer side is not forgiven either.
        assert!(differs(&check("* item", &[&["item", "\u{2022}"]])));
    }

    #[test]
    fn markup_never_hides_a_sign_or_a_star_on_one_side() {
        for (unit_text, layer) in [
            ("- 2", "2"),
            ("-2", "2"),
            ("2", "-2"),
            ("armatura*", "armatura"),
            ("*armatura", "armatura"),
            ("a * b", "a b"),
            ("2*3*4", "234"),
            ("**bold", "bold"),
            ("-", ""),
        ] {
            let layer: Vec<&str> = layer.split_whitespace().collect();
            let verdict = check(unit_text, &[&layer]);
            assert!(
                differs(&verdict) || verdict == Verdict::NoWords,
                "{unit_text}: {verdict:?}"
            );
        }
    }

    #[test]
    fn an_orphan_in_another_column_at_the_same_height_does_not_matter() {
        let polygon = rect(0.0, 0.0, 120.0, 60.0);
        let u = [unit(1, &polygon, "alpha beta")];
        let mut words = layout(&[&["alpha", "beta"]]);
        words.push(word("gamma", [400.0, 20.0, 440.0, 30.0]));
        let result = evaluate(&u, &words, &[]);
        assert_eq!(result.verdicts[0].verdict, Verdict::Agrees);
        assert_eq!(result.facts.orphans.len(), 1);
    }

    #[test]
    fn line_end_hyphenation_joins_with_or_without_the_hyphen() {
        let lines: &[&[&str]] = &[&["una", "inter-"], &["nazionale", "cosa"]];
        assert_eq!(check("una internazionale cosa", lines), Verdict::Agrees);
        assert_eq!(check("una inter-nazionale cosa", lines), Verdict::Agrees);
        let lines: &[&[&str]] = &[&["una", "inter\u{2010}"], &["nazionale"]];
        assert_eq!(check("una internazionale", lines), Verdict::Agrees);
        let lines: &[&[&str]] = &[&["una", "inter\u{ad}"], &["nazionale"]];
        assert_eq!(check("una internazionale", lines), Verdict::Agrees);
        // The unit kept the two words as they are drawn.
        assert_eq!(
            check(
                "una inter- nazionale",
                &[&["una", "inter-"], &["nazionale"]]
            ),
            Verdict::Agrees
        );
    }

    #[test]
    fn hyphenation_never_touches_digits_signs_capitals_or_a_lone_hyphen() {
        for (unit_text, lines) in [
            ("1020", vec![vec!["10-"], vec!["20"]]),
            ("2x", vec![vec!["2-"], vec!["x"]]),
            ("a2", vec![vec!["a-"], vec!["2b"]]),
            ("interNazionale", vec![vec!["inter-"], vec!["Nazionale"]]),
            ("abc", vec![vec!["-"], vec!["abc"]]),
            (
                "internazionale",
                vec![vec!["inter\u{2013}"], vec!["nazionale"]],
            ),
            (
                "internazionale",
                vec![vec!["inter\u{2212}"], vec!["nazionale"]],
            ),
            // Not at a line end: no joining.
            ("internazionale", vec![vec!["inter-", "nazionale"]]),
        ] {
            let lines: Vec<&[&str]> = lines.iter().map(Vec::as_slice).collect();
            assert!(differs(&check(unit_text, &lines)), "{unit_text}");
        }
    }

    #[test]
    fn a_shadow_duplicate_within_three_points_is_dropped_and_a_real_repeat_is_not() {
        let polygon = rect(0.0, 0.0, 300.0, 100.0);
        let u = [unit(1, &polygon, "alpha beta")];
        let mut words = layout(&[&["alpha", "beta"]]);
        words.insert(1, word("alpha", [12.0, 21.0, 52.0, 31.0]));
        assert_eq!(
            evaluate(&u, &words, &[]).verdicts[0].verdict,
            Verdict::Agrees
        );
        // Four points away is a second word, not a shadow.
        words[1] = word("alpha", [14.5, 20.0, 54.5, 30.0]);
        assert!(differs(&evaluate(&u, &words, &[]).verdicts[0].verdict));
        // Another text at the same place is not a shadow either.
        words[1] = word("gamma", [10.0, 20.0, 50.0, 30.0]);
        assert!(differs(&evaluate(&u, &words, &[]).verdicts[0].verdict));
    }

    #[test]
    fn a_word_beside_a_unit_in_no_unit_makes_it_differ_but_a_far_one_does_not() {
        let polygon = rect(0.0, 0.0, 120.0, 60.0);
        let u = [unit(1, &polygon, "alpha beta")];
        let mut words = layout(&[&["alpha", "beta"]]);
        // Just outside its box, within one line height.
        words.push(word("gamma", [112.0, 20.0, 142.0, 30.0]));
        let result = evaluate(&u, &words, &[]);
        assert_eq!(
            result.verdicts[0].verdict,
            Verdict::Differs {
                reason: DiffReason::OrphanWords,
                diff: None
            }
        );
        assert_eq!(result.facts.orphans.len(), 1);
        // Below the box by more than a line height.
        words[2] = word("gamma", [10.0, 300.0, 50.0, 310.0]);
        let result = evaluate(&u, &words, &[]);
        assert_eq!(result.verdicts[0].verdict, Verdict::Agrees);
        assert_eq!(result.facts.orphans, [[10.0, 300.0, 50.0, 310.0]]);
    }

    #[test]
    fn words_of_other_units_are_not_orphans_and_exclusion_boxes_are_left_out() {
        let (a, b) = (rect(0.0, 0.0, 120.0, 60.0), rect(125.0, 0.0, 300.0, 60.0));
        let mut other = unit(2, &b, "gamma");
        other.origin = "owner";
        let mut words = layout(&[&["alpha", "beta"]]);
        words.push(word("gamma", [130.0, 20.0, 170.0, 30.0]));
        let result = evaluate(&[unit(1, &a, "alpha beta"), other], &words, &[]);
        assert_eq!(result.verdicts[0].verdict, Verdict::Agrees);
        assert_eq!(
            result.verdicts[1].verdict,
            Verdict::Excluded(Exclusion::Owner)
        );
        assert_eq!(result.facts.orphans.len(), 0);
        // A watermark word inside the exclusion box counts nowhere, not even as an orphan.
        let mark = rect(125.0, 0.0, 300.0, 60.0);
        let result = evaluate(&[unit(1, &a, "alpha beta")], &words, &[&mark]);
        assert_eq!(result.verdicts[0].verdict, Verdict::Agrees);
        assert_eq!(result.facts.orphans.len(), 0);
        // Inside the unit's own polygon it is dropped from the unit too.
    }

    #[test]
    fn an_exclusion_box_never_silently_removes_a_word_from_a_unit() {
        let mark = rect(125.0, 0.0, 300.0, 60.0);
        let wide = rect(0.0, 0.0, 300.0, 60.0);
        let mut words = layout(&[&["alpha", "beta"]]);
        words.push(word("gamma", [130.0, 20.0, 170.0, 30.0]));
        let result = evaluate(&[unit(1, &wide, "alpha beta")], &words, &[&mark]);
        assert_eq!(
            result.verdicts[0].verdict,
            Verdict::Excluded(Exclusion::ExcludedArea)
        );
        // A box that only overlaps the polygon's bounding box excludes it too.
        let corner = rect(250.0, 50.0, 400.0, 100.0);
        let result = evaluate(&[unit(1, &wide, "alpha beta")], &words[..2], &[&corner]);
        assert_eq!(
            result.verdicts[0].verdict,
            Verdict::Excluded(Exclusion::ExcludedArea)
        );
        // A box elsewhere changes nothing.
        let far = rect(500.0, 500.0, 600.0, 600.0);
        let result = evaluate(&[unit(1, &wide, "alpha beta")], &words[..2], &[&far]);
        assert_eq!(result.verdicts[0].verdict, Verdict::Agrees);
    }

    #[test]
    fn an_orphan_box_touching_the_grown_unit_box_makes_it_differ() {
        let polygon = rect(0.0, 0.0, 100.0, 40.0);
        let u = [unit(1, &polygon, "alpha beta")];
        let words = vec![
            word("alpha", [10.0, 20.0, 50.0, 30.0]),
            word("beta", [55.0, 20.0, 95.0, 30.0]),
            // Its centre (140) is far outside the box; its left edge is inside the margin.
            word("armatura", [100.0, 20.0, 180.0, 30.0]),
        ];
        assert_eq!(
            evaluate(&u, &words, &[]).verdicts[0].verdict,
            Verdict::Differs {
                reason: DiffReason::OrphanWords,
                diff: None
            }
        );
    }

    #[test]
    fn a_tiny_font_repeat_is_two_words_not_a_shadow() {
        let polygon = rect(0.0, 0.0, 300.0, 100.0);
        let u = [unit(1, &polygon, "1")];
        let words = vec![
            word("1", [10.0, 20.0, 12.0, 24.0]),
            word("1", [13.0, 20.0, 15.0, 24.0]),
        ];
        assert!(differs(&evaluate(&u, &words, &[]).verdicts[0].verdict));
        // Same place, barely moved: a shadow.
        let words = vec![
            word("1", [10.0, 20.0, 12.0, 24.0]),
            word("1", [10.5, 20.0, 12.5, 24.0]),
        ];
        assert_eq!(
            evaluate(&u, &words, &[]).verdicts[0].verdict,
            Verdict::Agrees
        );
        // Within 3 points but barely overlapping the smaller box: not a shadow.
        let words = vec![
            word("ab", [10.0, 20.0, 12.0, 80.0]),
            word("ab", [11.9, 20.0, 40.0, 80.0]),
        ];
        let u = [unit(1, &polygon, "ab")];
        assert!(differs(&evaluate(&u, &words, &[]).verdicts[0].verdict));
    }

    #[test]
    fn emphasis_stripping_is_linear_and_pairs_correctly() {
        let text = "*a ".repeat(200_000);
        let (tokens, _) = unit_tokens(&text);
        assert_eq!(tokens.len(), 200_000);
        assert!(tokens.iter().all(|t| t == "*a"));
        let (tokens, _) = unit_tokens("*a b* *c **d e** x");
        assert_eq!(tokens, ["a", "b", "*c", "d", "e", "x"]);
        let (tokens, _) = unit_tokens(&"*a b ".repeat(100_000));
        assert_eq!(tokens.len(), 200_000);
    }

    #[test]
    fn a_page_over_the_bounds_is_not_computed() {
        let polygon = rect(0.0, 0.0, 300.0, 100.0);
        let words = layout(&[&["alpha"]]);
        let many: Vec<UnitInput<'_>> = (0..=MAX_UNITS)
            .map(|i| unit(i64::try_from(i).unwrap(), &polygon, "alpha"))
            .collect();
        let result = evaluate(&many, &words, &[]);
        assert!(!result.computed);
        assert_eq!(result.verdicts.len(), many.len());
        assert!(
            result
                .verdicts
                .iter()
                .all(|v| v.verdict == Verdict::Excluded(Exclusion::TextBudget))
        );
        let big = "word ".repeat(MAX_TEXT_BYTES / 5 + 1);
        let result = evaluate(&[unit(1, &polygon, &big)], &words, &[]);
        assert!(!result.computed);
        assert_eq!(
            result.verdicts[0].verdict,
            Verdict::Excluded(Exclusion::TextBudget)
        );
        let result = evaluate(&[unit(1, &polygon, "alpha")], &words, &[]);
        assert!(result.computed);
        assert_eq!(result.verdicts[0].verdict, Verdict::Agrees);
    }

    #[test]
    fn line_break_versions_of_the_sign_and_digit_regressions_differ() {
        assert!(differs(&check("16", &[&["1\u{2013}"], &["6"]])));
        assert!(differs(&check("2", &[&["-"], &["2"]])));
        assert!(differs(&check("2", &[&["\u{2010}"], &["2"]])));
        assert!(differs(&check("x2", &[&["x-"], &["2"]])));
    }

    #[test]
    fn a_word_in_two_units_excludes_both_even_when_one_is_not_a_candidate() {
        let (a, b) = (rect(0.0, 0.0, 70.0, 60.0), rect(30.0, 0.0, 300.0, 60.0));
        let words = layout(&[&["alpha", "beta"]]);
        // alpha's centre (30) lies in both; beta's only in the second.
        let both = evaluate(
            &[unit(1, &a, "alpha"), unit(2, &b, "alpha beta")],
            &words,
            &[],
        );
        assert_eq!(
            both.verdicts[0].verdict,
            Verdict::Excluded(Exclusion::SharedWord)
        );
        assert_eq!(
            both.verdicts[1].verdict,
            Verdict::Excluded(Exclusion::SharedWord)
        );
        let mut reviewed = unit(1, &a, "alpha");
        reviewed.state = "accepted";
        let result = evaluate(&[reviewed, unit(2, &b, "alpha beta")], &words, &[]);
        assert_eq!(
            result.verdicts[0].verdict,
            Verdict::Excluded(Exclusion::Reviewed)
        );
        assert_eq!(
            result.verdicts[1].verdict,
            Verdict::Excluded(Exclusion::SharedWord)
        );
    }

    #[test]
    fn drop_caps_and_glyphs_chaining_two_lines_differ() {
        // A drop cap taller than the first lines merges them into one line.
        let mut words = layout(&[&["orem", "ipsum"], &["dolor", "sit"]]);
        words.push(word("L", [0.0, 20.0, 9.0, 60.0]));
        let polygon = rect(0.0, 0.0, 300.0, 100.0);
        let u = [unit(1, &polygon, "Lorem ipsum dolor sit")];
        assert!(differs(&evaluate(&u, &words, &[]).verdicts[0].verdict));
        // A tall glyph between two lines chains them: words are read out of order.
        let mut words = layout(&[&["a", "b"], &["c", "d"]]);
        words.push(word("|", [200.0, 20.0, 205.0, 60.0]));
        let u = [unit(1, &polygon, "a b c d |")];
        assert!(differs(&evaluate(&u, &words, &[]).verdicts[0].verdict));
    }

    #[test]
    fn exclusions_and_missing_words_are_reported_in_order() {
        let polygon = rect(500.0, 500.0, 600.0, 600.0);
        let words = layout(&[&["alpha"]]);
        let mut cases = Vec::new();
        let mut owner = unit(1, &polygon, "alpha");
        owner.origin = "owner";
        cases.push((owner, Verdict::Excluded(Exclusion::Owner)));
        let mut reviewed = unit(2, &polygon, "alpha");
        reviewed.state = "rejected";
        cases.push((reviewed, Verdict::Excluded(Exclusion::Reviewed)));
        let mut table = unit(3, &polygon, "alpha");
        table.kind = "Table";
        cases.push((table, Verdict::Excluded(Exclusion::Kind)));
        let mut asked = unit(4, &polygon, "alpha");
        asked.excluded = Some(Exclusion::OpenSuggestion);
        cases.push((asked, Verdict::Excluded(Exclusion::OpenSuggestion)));
        cases.push((
            unit(5, &polygon, " \n "),
            Verdict::Excluded(Exclusion::NoText),
        ));
        cases.push((unit(6, &polygon, "alpha"), Verdict::NoWords));
        cases.push((unit(7, "not json", "alpha"), Verdict::NoWords));
        let here = rect(0.0, 0.0, 300.0, 100.0);
        cases.push((unit(8, &here, "alpha"), Verdict::Agrees));
        let units: Vec<UnitInput<'_>> = cases.iter().map(|(u, _)| u.clone()).collect();
        let result = evaluate(&units, &words, &[]);
        for ((_, expected), got) in cases.iter().zip(&result.verdicts) {
            assert_eq!(&got.verdict, expected, "unit {}", got.id);
        }
    }

    #[test]
    fn a_difference_is_shown_bounded() {
        let Verdict::Differs {
            diff: Some(diff), ..
        } = check("a b c", &[&["a", "x", "c"]])
        else {
            panic!("expected a difference");
        };
        assert_eq!(
            diff.changes,
            [Change {
                unit: "b".into(),
                layer: "x".into()
            }]
        );
        assert!(!diff.truncated);
        // Thirty separate changes keep the first twenty.
        let unit_text = (0..30)
            .map(|i| format!("t{i} u{i}"))
            .collect::<Vec<_>>()
            .join(" ");
        let layer: Vec<Vec<String>> = (0..30)
            .map(|i| vec![format!("t{i}"), format!("v{i}")])
            .collect();
        let lines: Vec<Vec<&str>> = layer
            .iter()
            .map(|l| l.iter().map(String::as_str).collect())
            .collect();
        let lines: Vec<&[&str]> = lines.iter().map(Vec::as_slice).collect();
        let Verdict::Differs {
            diff: Some(diff), ..
        } = check(&unit_text, &lines)
        else {
            panic!("expected a difference");
        };
        assert_eq!(diff.changes.len(), MAX_DIFF_CHANGES);
        assert!(diff.truncated);
        // Over the token bound there is no diff, but still a difference.
        let long: Vec<String> = (0..=MAX_DIFF_TOKENS).map(|i| format!("w{i}")).collect();
        let lines: Vec<&str> = long.iter().map(String::as_str).collect();
        let text = format!("{} extra", long.join(" "));
        assert_eq!(
            check(&text, &[&lines]),
            Verdict::Differs {
                reason: DiffReason::Tokens,
                diff: None
            }
        );
    }

    #[test]
    fn a_large_agreeing_page_is_decided_without_a_diff() {
        let long: Vec<String> = (0..5_000).map(|i| format!("w{i}")).collect();
        let lines: Vec<&str> = long.iter().map(String::as_str).collect();
        assert_eq!(check(&long.join(" "), &[&lines]), Verdict::Agrees);
    }

    #[test]
    fn provenance_comes_from_a_pinned_table() {
        use serde_json::json;
        let kind = |name, version, settings| classify_provenance(name, version, &settings);
        let forced = json!({"force_ocr": true, "disable_ocr": false});
        for llm in [json!(true), json!("false"), json!(null)] {
            assert_eq!(
                kind(
                    MARKER_NAME,
                    "2.0.0",
                    json!({"force_ocr": true, "use_llm": llm})
                ),
                ReadingKind::SameLayer
            );
        }
        assert_eq!(
            kind(
                MARKER_NAME,
                "2.0.0",
                json!({"force_ocr": true, "use_llm": false})
            ),
            ReadingKind::Independent
        );
        let kind = |name, version, settings| classify_provenance(name, version, &settings);
        assert_eq!(
            kind(MARKER_NAME, "2.0.0", forced.clone()),
            ReadingKind::Independent
        );
        assert_eq!(
            kind(MARKER_NAME, "2.0.0", json!({"force_ocr": true})),
            ReadingKind::Independent
        );
        for settings in [
            json!({"force_ocr": false}),
            json!({"force_ocr": true, "disable_ocr": true}),
            json!({"force_ocr": "true"}),
            json!({}),
        ] {
            assert_eq!(kind(MARKER_NAME, "2.0.0", settings), ReadingKind::SameLayer);
        }
        assert_eq!(
            kind(MARKER_NAME, "2.0.1", forced.clone()),
            ReadingKind::Unknown
        );
        assert_eq!(kind("other", "2.0.0", forced.clone()), ReadingKind::Unknown);
        assert_eq!(
            kind(ROUTE_A_NAME, ROUTE_A_VERSIONS[0], json!({})),
            ReadingKind::Derived
        );
        assert_eq!(kind(ROUTE_A_NAME, "99", json!({})), ReadingKind::Unknown);
        assert_eq!(RULE_VERSION, "agreement-v1");
    }

    // ---- property tests ----

    const CHAR: &str = "[a-z0-9+\u{2212}\u{2013}\u{e9}\u{e0}\u{fc}\u{301}-]";
    const WORD: &str = "[a-z0-9+\u{2212}\u{2013}\u{e9}\u{e0}\u{fc}\u{301}-]{1,6}";

    fn tokens() -> impl Strategy<Value = Vec<String>> {
        proptest::collection::vec(WORD, 1..8)
    }

    /// The layer on one line, or broken into two lines after `split` tokens.
    fn laid_out(unit_tokens: &[String], layer: &[String], split: Option<usize>) -> Verdict {
        let layer: Vec<&str> = layer.iter().map(String::as_str).collect();
        let text = unit_tokens.join(" ");
        match split.filter(|k| (1..layer.len()).contains(k)) {
            Some(k) => check(&text, &[&layer[..k], &layer[k..]]),
            None => check(&text, &[&layer]),
        }
    }

    fn single_line(unit_tokens: &[String], layer: &[String]) -> Verdict {
        laid_out(unit_tokens, layer, None)
    }

    proptest! {
        #[test]
        fn identical_tokens_agree(t in tokens(), split in proptest::option::of(1_usize..8)) {
            prop_assert_eq!(laid_out(&t, &t, split), Verdict::Agrees);
        }

        #[test]
        fn one_changed_character_differs(
            t in tokens(), at in any::<prop::sample::Index>(), pos in any::<prop::sample::Index>(),
            fresh in CHAR, on_unit in any::<bool>(), split in proptest::option::of(1_usize..8),
        ) {
            let i = at.index(t.len());
            let mut chars: Vec<char> = t[i].chars().collect();
            let p = pos.index(chars.len());
            let fresh = fresh.chars().next().unwrap();
            // The folds make the dashes one character.
            prop_assume!(fold(&chars[p].to_string()) != fold(&fresh.to_string()));
            chars[p] = fresh;
            let mut changed = t.clone();
            changed[i] = chars.into_iter().collect();
            let verdict = if on_unit { laid_out(&changed, &t, split) } else { laid_out(&t, &changed, split) };
            prop_assert!(differs(&verdict));
        }

        #[test]
        fn a_sign_on_one_side_only_differs(
            t in tokens(), at in any::<prop::sample::Index>(),
            sign in prop::sample::select(vec!['+', '-', '\u{2212}']), on_unit in any::<bool>(),
            split in proptest::option::of(1_usize..8),
        ) {
            let mut signed = t.clone();
            let i = at.index(t.len());
            signed[i] = format!("{sign}{}", t[i]);
            let verdict = if on_unit { laid_out(&signed, &t, split) } else { laid_out(&t, &signed, split) };
            prop_assert!(differs(&verdict));
        }

        #[test]
        fn two_swapped_words_differ(t in tokens(), a in any::<prop::sample::Index>(), b in any::<prop::sample::Index>()) {
            let (i, j) = (a.index(t.len()), b.index(t.len()));
            prop_assume!(fold(&t[i]) != fold(&t[j]));
            let mut swapped = t.clone();
            swapped.swap(i, j);
            prop_assert!(differs(&single_line(&t, &swapped)));
        }

        #[test]
        fn an_added_word_differs_on_either_side(
            t in tokens(), at in any::<prop::sample::Index>(), extra in WORD,
            on_unit in any::<bool>(), split in proptest::option::of(1_usize..8),
        ) {
            let mut longer = t.clone();
            longer.insert(at.index(t.len() + 1), extra);
            let verdict = if on_unit { laid_out(&longer, &t, split) } else { laid_out(&t, &longer, split) };
            prop_assert!(differs(&verdict));
        }
    }
}
