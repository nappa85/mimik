//! Label and value: a label, a separator (dot leader, tab, ` - `, ` - ` dashes, `: `), a value.
use super::{Draft, Problem, Shape, caption_candidate, excerpt};

/// Labels are short names, not sentences.
const MAX_LABEL_CHARS: usize = 60;
const MAX_LABEL_WORDS: usize = 6;
/// Less certain than a numbered or structured shape.
const LABEL_CAP: usize = 85;

/// The earliest separator in the line: a leader of three dots or an ellipsis, a tab, a spaced
/// dash, or a colon followed by a space.
fn split_label(line: &str) -> Option<(&str, &str)> {
    let candidates = [
        (line.find("..."), 3),
        (line.find('\u{2026}'), '\u{2026}'.len_utf8()),
        (line.find('\t'), 1),
        (line.find(" \u{2013} "), " \u{2013} ".len()),
        (line.find(" \u{2014} "), " \u{2014} ".len()),
        (line.find(" - "), 3),
        (line.find(": "), 2),
    ];
    let (at, len) = candidates
        .into_iter()
        .filter_map(|(at, len)| at.map(|at| (at, len)))
        .min_by_key(|(at, _)| *at)?;
    let label = line[..at].trim();
    let value = line[at + len..]
        .trim_start_matches(['.', '\u{2026}'])
        .trim();
    (!label.is_empty()
        && !value.is_empty()
        && label.chars().count() <= MAX_LABEL_CHARS
        && label.split_whitespace().count() <= MAX_LABEL_WORDS)
        .then_some((label, value))
}

pub(super) fn read(lines: &[String]) -> Draft {
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut caption = None;
    let mut unplaced: Vec<String> = Vec::new();
    let mut continuations = 0_usize;
    for (index, line) in lines.iter().enumerate() {
        if let Some((label, value)) = split_label(line) {
            rows.push(vec![label.to_owned(), value.to_owned()]);
        } else if let Some(last) = rows.last_mut() {
            last[1].push(' ');
            last[1].push_str(line);
            continuations += 1;
        } else if index == 0 && caption_candidate(line) {
            caption = Some(line.clone());
        } else {
            unplaced.push(line.clone());
        }
    }
    let body = lines.len() - usize::from(caption.is_some());
    if rows.len() < 2 || rows.len() * 2 < body {
        return Draft::none(Shape::Label, Vec::new());
    }
    let consumed = rows.len() + continuations + usize::from(caption.is_some());
    let score = u32::try_from(LABEL_CAP * consumed / lines.len()).unwrap_or(0);
    let problems = unplaced
        .iter()
        .map(|l| Problem::Unplaced(excerpt(l)))
        .collect();
    let lead = caption.into_iter().chain(unplaced).collect();
    Draft::build(Shape::Label, lead, rows, false, score, problems)
}
