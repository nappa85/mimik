//! The area of each part of a split unit (pure). Two ways, and the result says which:
//!
//! 1. **Words**: only when the whole unit's tokens equal the text layer's under agreement-v1
//!    (the `Agrees` comparison without the orphan check). A part's tokens map back to the words
//!    they were read from, and its area is the box of those words; a line shared by two parts
//!    gives both the line's full height.
//! 2. **Bands** otherwise (no words, or any token mismatch, which interleaved columns usually
//!    cause): the unit's box cut into stacked horizontal bands in proportion to each part's
//!    token count, each at least one line high.
//!
//! The areas are a starting point the Owner sees and edits; nothing here decides text.
use super::agreement::{
    LayerToken, consumed, layered_tokens, lines_of, unit_tokens, without_shadows,
};
use super::page_words::PageWord;
use super::table_words::Region;

/// How the areas were made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AreaPath {
    /// From the words of the page's text layer.
    Words,
    /// The unit's box cut in stacked bands in proportion to the parts' token counts.
    Bands,
}

impl AreaPath {
    /// The sentence the form shows.
    pub(crate) fn note(self) -> &'static str {
        match self {
            AreaPath::Words => {
                "The areas follow the words of the page's text layer, which match this text exactly."
            }
            AreaPath::Bands => {
                "The areas are approximate: the unit's box is cut into stacked bands in proportion to each part's length, because the page's words do not match this text exactly (or are missing). Check and adjust every box."
            }
        }
    }
}

impl AreaPath {
    /// The sentence when the Owner's edited boxes were replaced by new areas.
    pub(crate) fn replaced_note(self) -> String {
        format!(
            "The boxes you submitted were replaced: the number of parts changed or a box was not four numbers. {}",
            self.note()
        )
    }
}

/// One `[x0, y0, x1, y1]` box per part, in PDF points, and how they were made.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Areas {
    pub boxes: Vec<[f64; 4]>,
    pub path: AreaPath,
}

/// Fewest/most lines a band is measured against: a band is at least the unit's height over its
/// line count, and never less than a twentieth of it.
const MAX_LINES: usize = 20;

/// The areas of `parts` (text of each, in order) split from the unit drawn at `polygon` on a
/// page of `page` points, or `None` when the unit has no usable box (or there are no parts).
/// `words` are the page's decoded stored words.
pub(crate) fn areas(
    parts: &[&str],
    polygon: &str,
    words: Option<&[PageWord]>,
    page: (f64, f64),
) -> Option<Areas> {
    if parts.is_empty() {
        return None;
    }
    let region = Region::parse(polygon)?;
    let unit = clamp(region.bounds(), page);
    if unit[0] >= unit[2] || unit[1] >= unit[3] {
        return None;
    }
    if let Some(boxes) = words.and_then(|w| by_words(parts, &region, w, page)) {
        return Some(Areas {
            boxes,
            path: AreaPath::Words,
        });
    }
    Some(Areas {
        boxes: bands(parts, unit),
        path: AreaPath::Bands,
    })
}

fn clamp(b: [f64; 4], (width, height): (f64, f64)) -> [f64; 4] {
    [
        b[0].clamp(0.0, width),
        b[1].clamp(0.0, height),
        b[2].clamp(0.0, width),
        b[3].clamp(0.0, height),
    ]
}

fn by_words(
    parts: &[&str],
    region: &Region,
    words: &[PageWord],
    page: (f64, f64),
) -> Option<Vec<[f64; 4]>> {
    let all: Vec<&PageWord> = words.iter().collect();
    let inside: Vec<&PageWord> = without_shadows(&all)
        .into_iter()
        .filter(|word| {
            let (x, y) = word.centre();
            region.contains(f64::from(x), f64::from(y))
        })
        .collect();
    if inside.is_empty() {
        return None;
    }
    let lines = lines_of(inside);
    let (tokens, bullets) = unit_tokens(&parts.join("\n"));
    // Each part's own tokens must add up to the whole's (emphasis cannot span a boundary).
    let counts: Vec<usize> = parts.iter().map(|p| unit_tokens(p).0.len()).collect();
    if counts.contains(&0) || counts.iter().sum::<usize>() != tokens.len() {
        return None;
    }
    let layered = layered_tokens(&lines, bullets);
    let layer: Vec<LayerToken> = layered.iter().map(|e| e.token.clone()).collect();
    let used_counts = consumed(&tokens, &layer)?;
    let mut part_of_token = Vec::with_capacity(tokens.len());
    for (part, count) in counts.iter().enumerate() {
        part_of_token.extend(std::iter::repeat_n(part, *count));
    }
    let flat: Vec<&PageWord> = lines.iter().flatten().copied().collect();
    let mut owner: Vec<Option<usize>> = vec![None; flat.len()];
    let mut at = 0;
    for (entry, used) in layered.iter().zip(&used_counts) {
        // A hyphenation the unit wrote as two tokens across a boundary goes to the earlier part.
        let part = part_of_token[at];
        for &index in &entry.words {
            owner[index] = Some(part);
        }
        at += used;
    }
    // A list-marker glyph the unit side lost belongs to the part of the next word.
    let mut next = None;
    for slot in owner.iter_mut().rev() {
        match slot {
            Some(part) => next = Some(*part),
            None => *slot = next,
        }
    }
    let mut boxes: Vec<Option<[f64; 4]>> = vec![None; parts.len()];
    let mut line_start = 0;
    for line in &lines {
        let range = line_start..line_start + line.len();
        line_start = range.end;
        let (top, bottom) = line.iter().fold((f64::MAX, f64::MIN), |(t, b), w| {
            (t.min(f64::from(w.bounds[1])), b.max(f64::from(w.bounds[3])))
        });
        let mut line_parts: Vec<usize> = owner[range.clone()].iter().flatten().copied().collect();
        line_parts.dedup();
        let shared = line_parts.len() > 1;
        for index in range {
            let (Some(part), word) = (owner[index], flat[index]) else {
                continue;
            };
            let mut b = [
                f64::from(word.bounds[0]),
                f64::from(word.bounds[1]),
                f64::from(word.bounds[2]),
                f64::from(word.bounds[3]),
            ];
            if shared {
                b[1] = top;
                b[3] = bottom;
            }
            boxes[part] = Some(match boxes[part] {
                Some(c) => [
                    c[0].min(b[0]),
                    c[1].min(b[1]),
                    c[2].max(b[2]),
                    c[3].max(b[3]),
                ],
                None => b,
            });
        }
    }
    boxes
        .into_iter()
        .map(|b| {
            let b = clamp(b?, page);
            (b[0] < b[2] && b[1] < b[3]).then_some(b)
        })
        .collect()
}

#[expect(
    clippy::cast_precision_loss,
    reason = "token and line counts are small"
)]
fn bands(parts: &[&str], unit: [f64; 4]) -> Vec<[f64; 4]> {
    let n = parts.len();
    let height = unit[3] - unit[1];
    let lines: usize = parts
        .iter()
        .map(|p| p.lines().filter(|l| !l.trim().is_empty()).count())
        .sum::<usize>()
        .max(1);
    let line = height / lines.min(MAX_LINES) as f64;
    let minimum = line.min(height / n as f64);
    let extra = height - minimum * n as f64;
    let weights: Vec<f64> = parts
        .iter()
        .map(|p| unit_tokens(p).0.len() as f64)
        .collect();
    let total: f64 = weights.iter().sum();
    let mut top = unit[1];
    let mut out = Vec::with_capacity(n);
    for (i, weight) in weights.iter().enumerate() {
        let share = if total > 0.0 {
            weight / total
        } else {
            1.0 / n as f64
        };
        let bottom = if i + 1 == n {
            unit[3]
        } else {
            (top + minimum + extra * share).min(unit[3])
        };
        out.push([unit[0], top, unit[2], bottom]);
        top = bottom;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const PAGE: (f64, f64) = (300.0, 400.0);
    const WHOLE: &str = "[[0,0],[300,0],[300,400],[0,400]]";

    fn word(text: &str, b: [f32; 4]) -> PageWord {
        PageWord {
            text: text.into(),
            bounds: b,
        }
    }

    /// Words on lines 30 points apart (y 20, 50, ...), each 10 high and 40 wide, 50 apart from x 10.
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

    fn split(parts: &[&str], words: &[PageWord]) -> Areas {
        areas(parts, WHOLE, Some(words), PAGE).unwrap()
    }

    #[test]
    fn an_agreeing_unit_gets_the_boxes_of_its_words() {
        let words = layout(&[&["alpha", "beta"], &["gamma", "delta"], &["eps"]]);
        let result = split(&["alpha beta", "gamma delta\neps"], &words);
        assert_eq!(result.path, AreaPath::Words);
        assert_eq!(
            result.boxes,
            [[10.0, 20.0, 100.0, 30.0], [10.0, 50.0, 100.0, 90.0]]
        );
        // A part may also start mid-line: the line's height is shared.
        let mid = split(&["alpha", "beta gamma delta\neps"], &words);
        assert_eq!(mid.path, AreaPath::Words);
        assert_eq!(mid.boxes[0], [10.0, 20.0, 50.0, 30.0]);
        assert_eq!(mid.boxes[1], [10.0, 20.0, 100.0, 90.0]);
    }

    #[test]
    fn a_mid_line_boundary_shares_the_height_of_the_taller_word() {
        let mut words = layout(&[&["alpha", "beta"]]);
        words[1].bounds[3] = 34.0;
        let result = split(&["alpha", "beta"], &words);
        assert_eq!(result.path, AreaPath::Words);
        assert_eq!(result.boxes[0], [10.0, 20.0, 50.0, 34.0]);
        assert_eq!(result.boxes[1], [60.0, 20.0, 100.0, 34.0]);
    }

    #[test]
    fn a_hyphenated_word_goes_to_the_part_of_its_token_and_a_straddling_pair_to_the_earlier() {
        let words = layout(&[&["alpha", "exam-"], &["ple", "beta"]]);
        // One token: both of its words (on two lines) belong to part 1; line 2 is shared.
        let whole = split(&["alpha exam-ple", "beta"], &words);
        assert_eq!(whole.path, AreaPath::Words);
        assert_eq!(whole.boxes[0], [10.0, 20.0, 100.0, 60.0]);
        assert_eq!(whole.boxes[1], [60.0, 50.0, 100.0, 60.0]);
        // Two tokens split across the boundary: the pair goes to the earlier part.
        let straddle = split(&["alpha exam-", "ple beta"], &words);
        assert_eq!(straddle.path, AreaPath::Words);
        assert_eq!(straddle.boxes[0], [10.0, 20.0, 100.0, 60.0]);
        assert_eq!(straddle.boxes[1], [60.0, 50.0, 100.0, 60.0]);
    }

    #[test]
    fn a_bullet_unit_keeps_each_glyph_with_its_item() {
        let words = layout(&[&["\u{2022}", "alpha"], &["\u{2022}", "beta"]]);
        let result = split(&["* alpha", "* beta"], &words);
        assert_eq!(result.path, AreaPath::Words);
        assert_eq!(
            result.boxes,
            [[10.0, 20.0, 100.0, 30.0], [10.0, 50.0, 100.0, 60.0]]
        );
    }

    #[test]
    fn two_columns_a_corrected_unit_and_equal_counts_with_other_text_fall_back_to_bands() {
        // Two columns whose lines interleave: read column by column the tokens differ.
        let columns = vec![
            word("a1", [10.0, 20.0, 50.0, 30.0]),
            word("b1", [200.0, 20.0, 240.0, 30.0]),
            word("a2", [10.0, 50.0, 50.0, 60.0]),
            word("b2", [200.0, 50.0, 240.0, 60.0]),
        ];
        let result = split(&["a1 a2", "b1 b2"], &columns);
        assert_eq!(result.path, AreaPath::Bands);
        // A corrected text that is not the page's.
        let words = layout(&[&["alpha", "beta"], &["gamma", "delta"]]);
        let corrected = split(&["alpha betta", "gamma delta"], &words);
        assert_eq!(corrected.path, AreaPath::Bands);
        // The same number of tokens, one of them different.
        let other = split(&["alpha beta", "gamma delt"], &words);
        assert_eq!(other.path, AreaPath::Bands);
        // No words at all.
        let none = areas(&["alpha beta", "gamma delta"], WHOLE, None, PAGE).unwrap();
        assert_eq!(none.path, AreaPath::Bands);
    }

    #[test]
    fn bands_are_proportional_with_a_line_minimum_and_cover_the_unit() {
        let unit = "[[10,100],[110,100],[110,300],[10,300]]";
        // Five lines: a band is at least 40 high; the other 120 is shared 1:4.
        let result = areas(&["a", "b\nc\nd\ne"], unit, None, PAGE).unwrap();
        assert_eq!(result.path, AreaPath::Bands);
        assert_eq!(
            result.boxes,
            [[10.0, 100.0, 110.0, 164.0], [10.0, 164.0, 110.0, 300.0]]
        );
        // A short part still gets a line.
        let tiny = areas(
            &["a", "b c d e f g h i j k l m n o p q r s t u v w x y z"],
            unit,
            None,
            PAGE,
        )
        .unwrap();
        assert!(tiny.boxes[0][3] - tiny.boxes[0][1] >= 100.0 - 1e-9);
    }

    #[test]
    fn an_unusable_unit_box_has_no_areas() {
        assert!(areas(&["a", "b"], "not a polygon", None, PAGE).is_none());
        assert!(areas(&["a", "b"], "[[5,5],[5,5],[5,5]]", None, PAGE).is_none());
        assert!(areas(&[], WHOLE, None, PAGE).is_none());
    }

    proptest! {
        #[test]
        fn one_area_per_part_inside_the_page_and_not_empty(
            parts in proptest::collection::vec("[a-z]{1,6}( [a-z]{1,6}){0,5}(\n[a-z]{1,6}){0,3}", 2..=20),
            x0 in 0.0_f64..250.0, y0 in 0.0_f64..350.0, w in 1.0_f64..250.0, h in 1.0_f64..350.0,
            with_words in any::<bool>(),
        ) {
            let polygon = format!("[[{x0},{y0}],[{},{y0}],[{},{}],[{x0},{}]]", x0 + w, x0 + w, y0 + h, y0 + h);
            let refs: Vec<&str> = parts.iter().map(String::as_str).collect();
            // Words laid out as the text reads, so the words path is taken when it can be.
            let lines: Vec<Vec<&str>> = refs.iter().flat_map(|p| p.lines()).map(|l| l.split(' ').collect()).collect();
            let slices: Vec<&[&str]> = lines.iter().map(Vec::as_slice).collect();
            let words = layout(&slices);
            let words = with_words.then_some(words.as_slice());
            let result = areas(&refs, &polygon, words, PAGE);
            let result = result.expect("a usable unit box");
            prop_assert_eq!(result.boxes.len(), parts.len());
            for b in result.boxes {
                prop_assert!(b[0] >= 0.0 && b[1] >= 0.0 && b[2] <= PAGE.0 && b[3] <= PAGE.1, "{b:?}");
                prop_assert!(b[0] < b[2] && b[1] < b[3], "{b:?}");
            }
        }
    }
}
