//! The stored words that belong to a set of selected units: a word belongs to a unit when its
//! centre lies inside the unit's polygon. Pure functions; the row is untrusted and an invalid
//! row counts as absent.
use super::page_words::{self, PageWord};

/// What the form says about the Column grid's input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WordsState {
    /// The stored words of the selected units were used.
    Used,
    /// The page has no stored words (imported without them).
    Missing,
    /// The stored row failed validation and was ignored.
    Invalid,
    /// The page has words, but none lies inside the selected units.
    Outside,
}

impl WordsState {
    /// The sentence shown on the form when the Column grid could not use word boxes.
    pub(crate) fn note(self) -> &'static str {
        match self {
            WordsState::Used => "",
            WordsState::Missing => {
                "This page has no stored word boxes (it was imported without them), so Column grid reads each line's runs of two or more spaces. Running import-source again where the worker is available adds them."
            }
            WordsState::Invalid => {
                "The stored word boxes of this page could not be read and were ignored, so Column grid reads each line's runs of two or more spaces."
            }
            WordsState::Outside => {
                "No stored word box of this page lies inside the selected units, so Column grid reads each line's runs of two or more spaces."
            }
        }
    }
}

/// Polygons with more points than this are tested by their bounding box (a bound on the work).
const MAX_EXACT_POINTS: usize = 64;

struct Region {
    points: Vec<[f64; 2]>,
    bounds: [f64; 4],
}

impl Region {
    fn parse(polygon: &str) -> Option<Region> {
        let points: Vec<[f64; 2]> = serde_json::from_str(polygon).ok()?;
        if points.is_empty()
            || points.len() > 1024
            || points.iter().flatten().any(|v| !v.is_finite())
        {
            return None;
        }
        let bounds = points
            .iter()
            .fold([f64::MAX, f64::MAX, f64::MIN, f64::MIN], |b, [x, y]| {
                [b[0].min(*x), b[1].min(*y), b[2].max(*x), b[3].max(*y)]
            });
        Some(Region { points, bounds })
    }

    fn contains(&self, x: f64, y: f64) -> bool {
        let b = self.bounds;
        if x < b[0] || x > b[2] || y < b[1] || y > b[3] {
            return false;
        }
        if self.points.len() > MAX_EXACT_POINTS || self.points.len() < 3 {
            return true;
        }
        // Even-odd ray casting.
        let mut inside = false;
        let mut previous = self.points[self.points.len() - 1];
        for &point in &self.points {
            if (point[1] > y) != (previous[1] > y)
                && x < (previous[0] - point[0]) * (y - point[1]) / (previous[1] - point[1])
                    + point[0]
            {
                inside = !inside;
            }
            previous = point;
        }
        inside
    }
}

/// The words of a stored row whose centres lie inside any of `polygons` (each a JSON point
/// list), in the row's order, and the state for the form. `stored` is `None` when the page has
/// no row.
pub(crate) fn words_in(
    stored: Option<&[u8]>,
    polygons: &[&str],
) -> (Option<Vec<PageWord>>, WordsState) {
    let Some(bytes) = stored else {
        return (None, WordsState::Missing);
    };
    let Ok(words) = page_words::decode(bytes) else {
        return (None, WordsState::Invalid);
    };
    let regions: Vec<Region> = polygons.iter().filter_map(|p| Region::parse(p)).collect();
    let inside: Vec<PageWord> = words
        .into_iter()
        .filter(|word| {
            let (x, y) = word.centre();
            regions
                .iter()
                .any(|region| region.contains(f64::from(x), f64::from(y)))
        })
        .collect();
    if inside.is_empty() {
        (None, WordsState::Outside)
    } else {
        (Some(inside), WordsState::Used)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(words: &[(&str, [f32; 4])]) -> Vec<u8> {
        let words: Vec<PageWord> = words
            .iter()
            .map(|(t, b)| PageWord {
                text: (*t).into(),
                bounds: *b,
            })
            .collect();
        page_words::encode(&words, 600.0, 800.0).unwrap()
    }

    const BOX: &str = "[[10,10],[110,10],[110,60],[10,60]]";

    #[test]
    fn words_belong_to_a_unit_by_their_centre() {
        let bytes = row(&[
            ("in", [20.0, 20.0, 40.0, 30.0]),
            ("straddles", [100.0, 20.0, 140.0, 30.0]),
            ("out", [200.0, 20.0, 220.0, 30.0]),
        ]);
        let (words, state) = words_in(Some(&bytes), &[BOX]);
        let text: Vec<_> = words.unwrap().into_iter().map(|w| w.text).collect();
        // The straddling word's centre (120) is outside the box.
        assert_eq!(text, ["in"]);
        assert_eq!(state, WordsState::Used);
    }

    #[test]
    fn a_non_rectangular_polygon_is_tested_exactly() {
        // A triangle: its bounding box would also take the second word.
        let triangle = "[[0,0],[100,0],[0,100]]";
        let bytes = row(&[
            ("a", [5.0, 5.0, 15.0, 15.0]),
            ("b", [80.0, 80.0, 90.0, 90.0]),
        ]);
        let (words, _) = words_in(Some(&bytes), &[triangle]);
        assert_eq!(words.unwrap().len(), 1);
    }

    #[test]
    fn missing_invalid_and_outside_rows_say_so() {
        assert_eq!(words_in(None, &[BOX]), (None, WordsState::Missing));
        assert_eq!(words_in(Some(&[9, 9]), &[BOX]), (None, WordsState::Invalid));
        let bytes = row(&[("far", [300.0, 300.0, 320.0, 310.0])]);
        assert_eq!(words_in(Some(&bytes), &[BOX]), (None, WordsState::Outside));
        assert_eq!(words_in(Some(&[1]), &[BOX]), (None, WordsState::Outside));
        assert_ne!(WordsState::Missing.note(), "");
        assert_eq!(WordsState::Used.note(), "");
    }
}
