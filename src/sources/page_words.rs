//! Stored word boxes of a source page (`source_page_words`): the owner-visible text of a page
//! split into words, each with its box in PDF points (the unit polygons' space). A row is an
//! import artefact for the table shapes: never evidence, never shown as text of its own, not
//! indexed for search. The bytes are untrusted when read; an invalid row counts as absent.
//!
//! Encoding, version 1: one version byte (`1`), then for each word four little-endian `f32`
//! (`x0 y0 x1 y1`) and a little-endian `u16` text length followed by 1-256 bytes of UTF-8.
//! A page with no words is the version byte alone.

/// The encoding's version byte.
pub(crate) const VERSION: u8 = 1;
/// Bounds shared with the worker's `Words` records.
pub(crate) const MAX_WORDS: usize = 20_000;
pub(crate) const MAX_WORD_BYTES: usize = 256;
const WORD_OVERHEAD: usize = 18;
/// The most bytes a stored row can hold (also the migration's `CHECK`).
pub(crate) const MAX_ROW_BYTES: usize = MAX_WORDS * (MAX_WORD_BYTES + WORD_OVERHEAD) + 1;
/// How far past the page edge the `f32` narrowing of a box may put it, in points.
const ROUNDING: f32 = 0.01;
/// Largest page edge the worker accepts, in points; the bound a stored box is read against
/// (the page's own size is not stored; it was checked when the row was written).
const MAX_EDGE: f32 = 4_000.0;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PageWord {
    pub text: String,
    /// `[x0, y0, x1, y1]`, y down from the top, in PDF points.
    pub bounds: [f32; 4],
}

impl PageWord {
    pub(crate) fn centre(&self) -> (f32, f32) {
        (
            f32::midpoint(self.bounds[0], self.bounds[2]),
            f32::midpoint(self.bounds[1], self.bounds[3]),
        )
    }
}

fn valid(word: &PageWord, edge: (f32, f32)) -> bool {
    let b = word.bounds;
    !word.text.is_empty()
        && word.text.len() <= MAX_WORD_BYTES
        && !word.text.chars().any(char::is_control)
        && b.iter().all(|v| v.is_finite())
        && b[0] >= 0.0
        && b[1] >= 0.0
        && b[2] <= edge.0
        && b[3] <= edge.1
        && b[0] < b[2]
        && b[1] < b[3]
}

/// Encode words of a page of `width` x `height` points. The far corner is clamped to the
/// page when the narrowing to `f32` rounded it past the edge by a hair.
///
/// # Errors
/// Refuses a word that is empty, too long or outside the page, or too many words.
pub(crate) fn encode(words: &[PageWord], width: f64, height: f64) -> Result<Vec<u8>, &'static str> {
    if words.len() > MAX_WORDS {
        return Err("too many words");
    }
    #[allow(clippy::cast_possible_truncation)]
    let edge = (width as f32, height as f32);
    let mut out = Vec::with_capacity(1 + words.len() * 32);
    out.push(VERSION);
    for word in words {
        let mut bounds = word.bounds;
        // Only the narrowing's rounding is absorbed; a box really outside the page is refused.
        if bounds[2] > edge.0 && bounds[2] - edge.0 <= ROUNDING {
            bounds[2] = edge.0;
        }
        if bounds[3] > edge.1 && bounds[3] - edge.1 <= ROUNDING {
            bounds[3] = edge.1;
        }
        let checked = PageWord {
            text: word.text.clone(),
            bounds,
        };
        if !valid(&checked, edge) {
            return Err("invalid word");
        }
        for value in bounds {
            out.extend_from_slice(&value.to_le_bytes());
        }
        let length = u16::try_from(word.text.len()).map_err(|_| "invalid word")?;
        out.extend_from_slice(&length.to_le_bytes());
        out.extend_from_slice(word.text.as_bytes());
    }
    if out.len() > MAX_ROW_BYTES {
        return Err("row too large");
    }
    Ok(out)
}

/// Decode a stored row.
///
/// # Errors
/// Refuses any wrong version, truncation, trailing bytes, bad length, non-UTF-8 text, empty
/// text, non-finite or inverted box, box outside the largest page, or too many words.
pub(crate) fn decode(bytes: &[u8]) -> Result<Vec<PageWord>, &'static str> {
    if bytes.len() > MAX_ROW_BYTES {
        return Err("row too large");
    }
    let (&version, mut rest) = bytes.split_first().ok_or("empty row")?;
    if version != VERSION {
        return Err("unknown version");
    }
    let mut words = Vec::new();
    while !rest.is_empty() {
        if words.len() == MAX_WORDS {
            return Err("too many words");
        }
        let header = rest.get(..WORD_OVERHEAD).ok_or("truncated word")?;
        let mut bounds = [0.0_f32; 4];
        let (chunks, _) = header[..16].as_chunks::<4>();
        for (slot, chunk) in bounds.iter_mut().zip(chunks) {
            *slot = f32::from_le_bytes(*chunk);
        }
        let length = usize::from(u16::from_le_bytes([header[16], header[17]]));
        if length == 0 || length > MAX_WORD_BYTES {
            return Err("invalid word length");
        }
        let text = rest
            .get(WORD_OVERHEAD..WORD_OVERHEAD + length)
            .ok_or("truncated word")?;
        let word = PageWord {
            text: std::str::from_utf8(text)
                .map_err(|_| "invalid text")?
                .to_owned(),
            bounds,
        };
        if !valid(&word, (MAX_EDGE, MAX_EDGE)) {
            return Err("invalid word");
        }
        words.push(word);
        rest = &rest[WORD_OVERHEAD + length..];
    }
    Ok(words)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(text: &str, b: [f32; 4]) -> PageWord {
        PageWord {
            text: text.into(),
            bounds: b,
        }
    }

    #[test]
    fn rows_round_trip_and_an_empty_page_is_the_version_byte() {
        let words = vec![
            word("Sword", [10.0, 20.5, 40.25, 30.0]),
            word("d\u{e9}j\u{e0}", [50.0, 20.5, 80.0, 30.0]),
        ];
        let bytes = encode(&words, 595.0, 842.0).unwrap();
        assert_eq!(bytes[0], 1);
        assert_eq!(decode(&bytes).unwrap(), words);
        assert_eq!(encode(&[], 595.0, 842.0).unwrap(), [1]);
        assert_eq!(decode(&[1]).unwrap().len(), 0);
    }

    #[test]
    fn encode_refuses_what_decode_would_refuse() {
        let ok = [1.0, 1.0, 2.0, 2.0];
        for bad in [
            word("", ok),
            word(&"x".repeat(257), ok),
            word("a", [f32::NAN, 1.0, 2.0, 2.0]),
            word("a", [2.0, 1.0, 1.0, 2.0]),
            word("a", [-1.0, 1.0, 2.0, 2.0]),
            word("a", [1.0, 1.0, 2.0, 900.0]),
            word("a\u{0}b", ok),
            word("a\u{85}", ok),
            word("tab\t", ok),
        ] {
            assert!(encode(&[bad], 595.0, 842.0).is_err());
        }
        assert!(encode(&vec![word("a", ok); MAX_WORDS + 1], 595.0, 842.0).is_err());
        assert!(encode(&vec![word("a", ok); MAX_WORDS], 595.0, 842.0).is_ok());
    }

    #[test]
    fn a_box_a_hair_past_the_edge_is_clamped_and_one_further_is_refused() {
        let hair = word("a", [590.0, 1.0, 595.28, 2.0]);
        let bytes = encode(&[hair], 595.2756, 842.0).unwrap();
        assert!(decode(&bytes).unwrap()[0].bounds[2] <= 595.2756_f32);
        assert!(encode(&[word("a", [590.0, 1.0, 595.5, 2.0])], 595.2756, 842.0).is_err());
    }

    #[test]
    fn malformed_rows_are_refused() {
        let good = encode(&[word("ab", [1.0, 1.0, 5.0, 5.0])], 595.0, 842.0).unwrap();
        assert!(decode(&[]).is_err());
        let mut v2 = good.clone();
        v2[0] = 2;
        assert!(decode(&v2).is_err());
        for cut in 2..good.len() {
            assert!(decode(&good[..cut]).is_err(), "cut {cut}");
        }
        let mut trailing = good.clone();
        trailing.push(0);
        assert!(decode(&trailing).is_err());
        let mut zero_length = good.clone();
        zero_length[17] = 0;
        zero_length[18] = 0;
        assert!(decode(&zero_length).is_err());
        let mut long = good.clone();
        long[17..19].copy_from_slice(&257_u16.to_le_bytes());
        long.resize(long.len() + 300, b'a');
        assert!(decode(&long).is_err());
        let mut utf8 = good.clone();
        let end = utf8.len();
        utf8[end - 2..].copy_from_slice(&[0xff, 0xfe]);
        assert!(decode(&utf8).is_err());
        let mut nan = good.clone();
        nan[1..5].copy_from_slice(&f32::NAN.to_le_bytes());
        assert!(decode(&nan).is_err());
        let mut huge = good;
        huge[9..13].copy_from_slice(&1.0e9_f32.to_le_bytes());
        assert!(decode(&huge).is_err());
        assert!(decode(&vec![1; MAX_ROW_BYTES + 1]).is_err());
    }
}
