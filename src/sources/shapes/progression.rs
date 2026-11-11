//! Progression: one line per level (`1st`, `2nd`, `3rd`, `4th`, `1°`, `Level 1`, `Livello 1`),
//! levels increasing by one, the rest of the line split into cells.
use std::sync::LazyLock;

use regex::Regex;

use super::{Draft, PROBLEM_PENALTY, Problem, Shape, excerpt, is_value, ragged, split_gaps};

/// A progression needs at least this many level rows.
const MIN_LEVELS: usize = 3;

struct Level {
    number: u32,
    cells: Vec<String>,
}

/// The level number, the first cell, and the text after the level word.
fn leading(line: &str) -> Option<(u32, String, &str)> {
    static ORDINAL: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^(\d{1,2})(?:st|nd|rd|th|ST|ND|RD|TH|\u{b0}|\u{ba})$").expect("static pattern")
    });
    let end = line.find(char::is_whitespace).unwrap_or(line.len());
    let first = &line[..end];
    let rest = line[end..].trim_start();
    if let Some(caps) = ORDINAL.captures(first) {
        return Some((caps[1].parse().ok()?, first.to_owned(), rest));
    }
    if first.eq_ignore_ascii_case("level") || first.eq_ignore_ascii_case("livello") {
        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        let digits = &rest[..end];
        if (1..=2).contains(&digits.len()) && digits.bytes().all(|b| b.is_ascii_digit()) {
            return Some((
                digits.parse().ok()?,
                format!("{first} {digits}"),
                rest[end..].trim_start(),
            ));
        }
    }
    None
}

/// Whether a (normalised) line begins a level row.
pub(super) fn starts_row(line: &str) -> bool {
    leading(line).is_some()
}

/// Split on runs of two or more spaces or tabs; without any, on single spaces between numeric
/// tokens (`+2`, `1d8`, a dash), keeping a run of words together as one cell.
fn split_cells(text: &str) -> Vec<String> {
    if text.contains('\t') || text.contains("  ") {
        return split_gaps(text);
    }
    let mut cells = Vec::new();
    let mut words = String::new();
    for token in text.split_whitespace() {
        if is_value(token) {
            if !words.is_empty() {
                cells.push(std::mem::take(&mut words));
            }
            cells.push(token.to_owned());
        } else {
            if !words.is_empty() {
                words.push(' ');
            }
            words.push_str(token);
        }
    }
    if !words.is_empty() {
        cells.push(words);
    }
    cells
}

pub(super) fn read(lines: &[String]) -> Draft {
    let Some(first) = lines.iter().position(|l| leading(l).is_some()) else {
        return Draft::none(Shape::Progression, Vec::new());
    };
    let mut levels: Vec<Level> = Vec::new();
    let mut unplaced = Vec::new();
    let mut consumed = 0_usize;
    for line in &lines[first..] {
        if let Some((number, head, rest)) = leading(line) {
            let mut cells = vec![head];
            cells.extend(split_cells(rest));
            levels.push(Level { number, cells });
            consumed += 1;
        } else if let Some(last) = levels.last_mut().and_then(|l| l.cells.last_mut()) {
            last.push(' ');
            last.push_str(line);
            consumed += 1;
        }
    }
    if levels.len() < MIN_LEVELS {
        return Draft::none(Shape::Progression, Vec::new());
    }
    let mut problems = Vec::new();
    for (index, pair) in levels.windows(2).enumerate() {
        let n = index + 2;
        problems.push(match pair[1].number.cmp(&(pair[0].number + 1)) {
            std::cmp::Ordering::Equal => continue,
            std::cmp::Ordering::Greater => Problem::Gap(n),
            std::cmp::Ordering::Less if pair[1].number == pair[0].number => Problem::Duplicate(n),
            std::cmp::Ordering::Less => Problem::Interleaved(n),
        });
    }
    let mut rows: Vec<Vec<String>> = levels.into_iter().map(|l| l.cells).collect();
    let (modal, ragged_rows) = ragged(&rows);
    problems.extend(ragged_rows);

    // The line just before the first level is a header when it splits into as many cells and
    // has no numbers; every other earlier line stays as a single-cell row in front.
    let mut header = false;
    let mut lead = Vec::new();
    let before = &lines[..first];
    for (index, line) in before.iter().enumerate() {
        let cells = split_gaps(line);
        let cells = if cells.len() == modal {
            cells
        } else {
            line.split_whitespace().map(str::to_owned).collect()
        };
        if index + 1 == before.len()
            && cells.len() == modal
            && modal >= 2
            && !cells.iter().any(|c| c.split_whitespace().any(is_value))
        {
            rows.insert(0, cells);
            header = true;
        } else {
            unplaced.push(excerpt(line));
            lead.push(line.clone());
        }
        consumed += 1;
    }
    let penalty = u32::try_from(problems.len())
        .unwrap_or(u32::MAX)
        .saturating_mul(PROBLEM_PENALTY);
    let base = u32::try_from(100 * consumed / lines.len()).unwrap_or(0);
    problems.extend(unplaced.into_iter().map(Problem::Unplaced));
    Draft::build(
        Shape::Progression,
        lead,
        rows,
        header,
        base.saturating_sub(penalty),
        problems,
    )
}
