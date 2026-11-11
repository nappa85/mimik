//! Table shapes: deterministic suggestions for the cells of an Owner-built table.
//!
//! Pure functions over the text of the selected units (no I/O, no model). A [`Draft`] is only
//! a suggestion for the editor: the table unit stays `unreviewed`, nothing is accepted from a
//! score, and a draft is never evidence. Every recogniser is a linear scan (character scanning
//! and linear regexes, no backtracking).
use std::fmt;
use std::sync::LazyLock;

use regex::Regex;

use super::tables::{self, MAX_CELL_CHARS, MAX_COLUMNS, MAX_ROWS};

mod labels;
mod progression;
mod roll;
mod stats;
#[cfg(test)]
mod tests;

/// `Plain columns` has a fixed low score so that any shape that fits wins.
pub(crate) const PLAIN_SCORE: u32 = 10;
/// A caption (a first line that fits no row) is at most this many characters.
const CAPTION_CHARS: usize = 80;
/// Score lost for each sequence problem (gap, duplicate, ragged row, ...).
const PROBLEM_PENALTY: u32 = 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Shape {
    Roll,
    Progression,
    Stat,
    Label,
    Plain,
}

impl Shape {
    /// The fixed order that breaks score ties.
    pub(crate) const ALL: [Shape; 5] = [
        Shape::Roll,
        Shape::Progression,
        Shape::Stat,
        Shape::Label,
        Shape::Plain,
    ];

    pub(crate) fn key(self) -> &'static str {
        match self {
            Shape::Roll => "roll",
            Shape::Progression => "progression",
            Shape::Stat => "stat",
            Shape::Label => "label",
            Shape::Plain => "plain",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Shape::Roll => "Roll table",
            Shape::Progression => "Progression",
            Shape::Stat => "Stat line",
            Shape::Label => "Label and value",
            Shape::Plain => "Plain columns",
        }
    }

    pub(crate) fn from_key(key: &str) -> Option<Shape> {
        Shape::ALL.into_iter().find(|shape| shape.key() == key)
    }
}

/// What a shape could not place or had to flag. Shown as text on the form; never auto-fixed.
/// Row numbers are 1-based among the recognised rows (header and lead lines not counted).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Problem {
    /// A value is missing before this row.
    Gap(usize),
    /// This row repeats the previous one's value.
    Duplicate(usize),
    /// This row's values overlap the previous row's, or run past the die.
    Overlap(usize),
    /// The numbers go back (two tables side by side read as 1, 1, 2, 2...).
    Interleaved(usize),
    /// The table stops before the top of its die (last value, top).
    Incomplete(u32, u32),
    /// A line that fits no row (an excerpt; the line itself is kept in the cells).
    Unplaced(String),
    /// More than 16 units: one column, one row per line.
    OneColumn,
    /// A row with a different number of cells from most rows.
    Ragged(usize),
    TooManyRows,
    TooManyColumns,
    CellTooLong,
    TooLarge,
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Problem::Gap(row) => write!(f, "A value is missing before row {row}."),
            Problem::Duplicate(row) => write!(f, "Row {row} repeats the previous row's value."),
            Problem::Overlap(row) => write!(
                f,
                "Row {row} overlaps the previous row or runs past the die."
            ),
            Problem::Interleaved(row) => write!(
                f,
                "The numbers go back at row {row}: two tables side by side, read as one?"
            ),
            Problem::Incomplete(last, top) => {
                write!(f, "The table stops at {last}; its die reaches {top}.")
            }
            Problem::Unplaced(line) => write!(f, "This line fits no row: \u{201c}{line}\u{201d}."),
            Problem::Ragged(row) => write!(f, "Row {row} has a different number of cells."),
            Problem::OneColumn => write!(
                f,
                "More than {MAX_COLUMNS} units: read as one column, one row per line."
            ),
            Problem::TooManyRows => write!(f, "More than {MAX_ROWS} rows."),
            Problem::TooManyColumns => write!(f, "More than {MAX_COLUMNS} columns."),
            Problem::CellTooLong => write!(f, "A cell has more than {MAX_CELL_CHARS} characters."),
            Problem::TooLarge => write!(f, "The draft is larger than a table can be."),
        }
    }
}

/// A suggested grid. `rows` is rectangular; `rows` is empty when the shape does not fit (score 0).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Draft {
    pub shape: Shape,
    pub rows: Vec<Vec<String>>,
    /// The first row of `rows` is a recognised header (`d6 Result`, a column-name line).
    pub header: bool,
    /// 0..=100: the share of lines consumed minus the problem penalties; never an acceptance.
    pub score: u32,
    pub problems: Vec<Problem>,
}

impl Draft {
    pub(crate) fn none(shape: Shape, problems: Vec<Problem>) -> Draft {
        Draft {
            shape,
            rows: Vec::new(),
            header: false,
            score: 0,
            problems,
        }
    }

    /// Pads rows, puts the `lead` lines (a caption, lines that fit no row) in front as
    /// single-cell rows so no text of the units is lost, and enforces the table bounds (a draft
    /// that cannot be saved is no draft). The header flag only holds when no lead row precedes
    /// the header row.
    fn build(
        shape: Shape,
        lead: Vec<String>,
        mut rows: Vec<Vec<String>>,
        header: bool,
        score: u32,
        mut problems: Vec<Problem>,
    ) -> Draft {
        let header = header && lead.is_empty();
        rows.splice(0..0, lead.into_iter().map(|text| vec![text]));
        let width = rows.iter().map(Vec::len).max().unwrap_or(0);
        for row in &mut rows {
            row.resize(width, String::new());
        }
        if rows.is_empty() {
            return Draft::none(shape, problems);
        }
        let invalid = if rows.len() > MAX_ROWS {
            Some(Problem::TooManyRows)
        } else if width > MAX_COLUMNS {
            Some(Problem::TooManyColumns)
        } else if rows
            .iter()
            .flatten()
            .any(|cell| cell.chars().count() > MAX_CELL_CHARS)
        {
            Some(Problem::CellTooLong)
        } else if tables::check(&rows, header).is_err() {
            Some(Problem::TooLarge)
        } else {
            None
        };
        if let Some(problem) = invalid {
            problems.push(problem);
            return Draft::none(shape, problems);
        }
        Draft {
            shape,
            rows,
            header,
            score: score.min(100),
            problems,
        }
    }
}

/// The selected units in reading order, each as its trimmed nonempty lines (corrected text where
/// corrected, extraction text otherwise: the caller chooses).
pub(crate) struct ShapeInput {
    units: Vec<Vec<String>>,
}

impl ShapeInput {
    pub(crate) fn new<S: AsRef<str>>(texts: &[S]) -> ShapeInput {
        ShapeInput {
            units: texts
                .iter()
                .map(|text| {
                    text.as_ref()
                        .lines()
                        .map(str::trim)
                        .filter(|line| !line.is_empty())
                        .map(str::to_owned)
                        .collect()
                })
                .collect(),
        }
    }

    /// The lines of all units in reading order with html2text's decoration removed.
    fn lines(&self) -> Vec<String> {
        self.units
            .iter()
            .flatten()
            .map(|line| normalise(line))
            .filter(|line| !line.is_empty())
            .collect()
    }
}

/// `raw_text` comes from html2text's decorating renderer: `**` around bold text, `*text*`
/// emphasis, `* ` and `- ` bullets and `#` headings. They are removed for matching; an `<ol>`
/// marker (`1. `) is left alone and is treated as a weak marker by the roll shape.
fn normalise(line: &str) -> String {
    let mut text = line.trim();
    let hashes = text.bytes().take_while(|b| *b == b'#').count();
    if (1..=6).contains(&hashes) && text[hashes..].starts_with(' ') {
        text = text[hashes..].trim_start();
    }
    if let Some(rest) = text.strip_prefix("* ").or_else(|| text.strip_prefix("- ")) {
        text = rest.trim_start();
    }
    let mut out = text.replace("**", "");
    if out.starts_with('*')
        && out[1..]
            .chars()
            .next()
            .is_some_and(|c| !c.is_whitespace() && c != '*')
        && let Some(end) = out[1..].find('*')
    {
        out.remove(end + 1);
        out.remove(0);
    }
    out.trim().to_owned()
}

/// Cells separated by a tab or by two or more spaces.
fn split_gaps(text: &str) -> Vec<String> {
    let mut cells = Vec::new();
    let mut cell = String::new();
    let mut spaces = 0_usize;
    let mut flush = |cell: &mut String| {
        let trimmed = cell.trim();
        if !trimmed.is_empty() {
            cells.push(trimmed.to_owned());
        }
        cell.clear();
    };
    for c in text.chars() {
        match c {
            '\t' => {
                flush(&mut cell);
                spaces = 0;
            }
            ' ' => spaces += 1,
            _ => {
                if spaces >= 2 {
                    flush(&mut cell);
                } else if spaces == 1 && !cell.is_empty() {
                    cell.push(' ');
                }
                spaces = 0;
                cell.push(c);
            }
        }
    }
    flush(&mut cell);
    cells
}

/// A short excerpt of a line for a problem message.
fn excerpt(line: &str) -> String {
    let mut out: String = line.chars().take(40).collect();
    if line.chars().count() > 40 {
        out.push('\u{2026}');
    }
    out
}

/// A short first line with no row marker can be a caption.
fn caption_candidate(line: &str) -> bool {
    line.chars().count() <= CAPTION_CHARS
}

/// A numeric table value: a signed number, a fraction, a percentage, a dice expression or a dash.
fn is_value(token: &str) -> bool {
    static VALUE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^(?:[+\-\u{2212}\u{2013}]?\d+(?:/\d+)?%?|\d*[dD]\d+(?:[+\-\u{2212}]\d+)?|[\u{2014}\u{2013}\-])$")
            .expect("static pattern")
    });
    VALUE.is_match(token)
}

/// Rows that differ in width from the most common width.
fn ragged(rows: &[Vec<String>]) -> (usize, Vec<Problem>) {
    let mut counts = std::collections::BTreeMap::new();
    for row in rows {
        *counts.entry(row.len()).or_insert(0_usize) += 1;
    }
    // Most common width; the first one reached wins a tie.
    let mut modal = rows.first().map_or(0, Vec::len);
    let mut best = 0;
    for row in rows {
        let n = counts[&row.len()];
        if n > best {
            best = n;
            modal = row.len();
        }
    }
    let problems = rows
        .iter()
        .enumerate()
        .filter(|(_, row)| row.len() != modal)
        .map(|(i, _)| Problem::Ragged(i + 1))
        .collect();
    (modal, problems)
}

/// One column per selected unit, one row per nonempty line, short columns padded with empty cells.
fn plain(input: &ShapeInput) -> Draft {
    if input.units.len() > MAX_COLUMNS {
        let rows = input
            .units
            .iter()
            .flatten()
            .map(|line| vec![line.clone()])
            .collect();
        return Draft::build(
            Shape::Plain,
            Vec::new(),
            rows,
            false,
            PLAIN_SCORE,
            vec![Problem::OneColumn],
        );
    }
    let height = input.units.iter().map(Vec::len).max().unwrap_or(0);
    let rows = (0..height)
        .map(|r| {
            input
                .units
                .iter()
                .map(|column| column.get(r).cloned().unwrap_or_default())
                .collect()
        })
        .collect();
    Draft::build(
        Shape::Plain,
        Vec::new(),
        rows,
        false,
        PLAIN_SCORE,
        Vec::new(),
    )
}

/// The draft of one shape (a shape that does not fit has no rows and score 0).
pub(crate) fn read(input: &ShapeInput, shape: Shape) -> Draft {
    match shape {
        Shape::Plain => plain(input),
        Shape::Roll => roll::read(&input.lines()),
        Shape::Progression => progression::read(&input.lines()),
        Shape::Stat => stats::read(&input.lines()),
        Shape::Label => labels::read(&input.lines()),
    }
}

/// The draft of every shape, best score first; ties keep the fixed shape order.
pub(crate) fn best(input: &ShapeInput) -> Vec<Draft> {
    let mut drafts: Vec<Draft> = Shape::ALL
        .into_iter()
        .map(|shape| read(input, shape))
        .collect();
    drafts.sort_by_key(|draft| std::cmp::Reverse(draft.score));
    drafts
}
