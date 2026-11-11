//! Roll table: a die's results, one row per roll value or range.
//!
//! Rows start with a marker (`3`, `3.`, `3)`, `3:`, `3-4`, `96-00`, `5+`) that together form a
//! complete cover of a die. The scan is line by line; a second scan reads the token stream of the
//! joined lines for tables Marker merged into one paragraph (`1 Rats 2 Goblins 3 ...`), looking
//! for the next expected marker only, with no backtracking.
use std::sync::LazyLock;

use regex::Regex;

use super::{Draft, PROBLEM_PENALTY, Problem, Shape, caption_candidate, excerpt, split_gaps};

const DICE: [u32; 11] = [2, 3, 4, 5, 6, 8, 10, 12, 20, 30, 100];
const SUM_SIDES: [u32; 6] = [4, 6, 8, 10, 12, 20];
/// A table read from a merged paragraph is less certain than one read line by line.
const INLINE_CAP: u32 = 90;
/// Values above this are not roll values (6d20 reaches 120).
const MAX_VALUE: u32 = 120;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Die {
    count: u32,
    /// 66 means d66 (tens and units, each 1..=6).
    sides: u32,
}

impl Die {
    fn range(self) -> (u32, u32) {
        if self.sides == 66 {
            (11, 66)
        } else {
            (self.count, self.count * self.sides)
        }
    }
}

#[derive(Clone, Debug)]
struct Marker {
    lo: u32,
    hi: u32,
    plus: bool,
    range: bool,
    /// Followed by `.`, `)` or `:`.
    punct: Option<char>,
    text: String,
}

impl Marker {
    /// An `<ol>` style `N.` marker: never proves a die alone.
    fn weak(&self) -> bool {
        self.punct == Some('.') && !self.range && !self.plus
    }
}

struct RollRow {
    marker: Marker,
    outcome: String,
}

#[derive(Default)]
struct Scan {
    rows: Vec<RollRow>,
    caption: Option<String>,
    unplaced: Vec<String>,
}

fn value(digits: &str) -> Option<u32> {
    if digits.is_empty() || digits.len() > 3 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let n: u32 = digits.parse().ok()?;
    match n {
        0 if digits == "00" => Some(100),
        0 => None,
        n if n <= MAX_VALUE => Some(n),
        _ => None,
    }
}

fn parse_token(token: &str) -> Option<Marker> {
    let (core, punct) = match token.chars().last()? {
        c @ ('.' | ')' | ':') => (&token[..token.len() - 1], Some(c)),
        _ => (token, None),
    };
    let digits = core.bytes().take_while(u8::is_ascii_digit).count();
    let lo = value(&core[..digits])?;
    let after = &core[digits..];
    let (hi, plus, range) = if after.is_empty() {
        (lo, false, false)
    } else if after == "+" {
        (lo, true, false)
    } else {
        let sep = after.chars().next()?;
        if !matches!(sep, '-' | '\u{2013}' | '\u{2014}') {
            return None;
        }
        (value(&after[sep.len_utf8()..])?, false, true)
    };
    if lo > hi {
        return None;
    }
    Some(Marker {
        lo,
        hi,
        plus,
        range,
        punct,
        text: core.to_owned(),
    })
}

/// A marker at the start of a line and the text after it.
fn parse_marker(line: &str) -> Option<(Marker, &str)> {
    let end = line.find(char::is_whitespace).unwrap_or(line.len());
    let marker = parse_token(&line[..end])?;
    Some((marker, line[end..].trim_start()))
}

fn clean_outcome(rest: &str) -> &str {
    for sep in ['\u{2013}', '\u{2014}', '-', ':'] {
        if let Some(after) = rest.strip_prefix(sep)
            && (after.is_empty() || after.starts_with(char::is_whitespace))
        {
            return after.trim_start();
        }
    }
    rest
}

fn parse_die(token: &str) -> Option<Die> {
    static DIE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^(\d{0,2})[dD](\d{1,3})$").expect("static pattern"));
    let caps = DIE.captures(token)?;
    let count = if caps[1].is_empty() {
        1
    } else {
        caps[1].parse().ok()?
    };
    let sides: u32 = caps[2].parse().ok()?;
    let ok = match count {
        1 => DICE.contains(&sides) || sides == 66,
        2..=6 => SUM_SIDES.contains(&sides),
        _ => false,
    };
    ok.then_some(Die { count, sides })
}

/// `d6 Result`, `1d20`, `d66`: the die and the header cells.
fn parse_header(line: &str) -> Option<(Die, Vec<String>)> {
    if line.chars().count() > 60 {
        return None;
    }
    let end = line.find(char::is_whitespace).unwrap_or(line.len());
    let die = parse_die(&line[..end])?;
    let rest = line[end..].trim();
    if parse_marker(rest).is_some() {
        return None;
    }
    let mut cells = vec![line[..end].to_owned()];
    cells.extend(split_gaps(rest));
    Some((die, cells))
}

/// The next expected value after `hi`.
fn next_after(hi: u32, d66: bool) -> u32 {
    if d66 && hi % 10 == 6 {
        (hi / 10 + 1) * 10 + 1
    } else {
        hi + 1
    }
}

/// The die a marker sequence covers, when no header names it.
fn infer(rows: &[RollRow]) -> Option<Die> {
    let first = rows.first()?.marker.lo;
    let last = &rows.last()?.marker;
    let ranged = rows.iter().any(|r| r.marker.range || r.marker.plus);
    let top = if last.plus {
        DICE.iter().copied().find(|d| *d >= last.lo)?
    } else {
        last.hi
    };
    if first == 1 && DICE.contains(&top) && rows.len() >= if ranged { 3 } else { 4 } {
        return Some(Die {
            count: 1,
            sides: top,
        });
    }
    let digit_ok = |n: u32| (1..=6).contains(&(n % 10)) && (1..=6).contains(&(n / 10));
    if first == 11
        && top == 66
        && rows.len() >= 6
        && rows
            .iter()
            .all(|r| !r.marker.range && !r.marker.plus && digit_ok(r.marker.lo))
    {
        return Some(Die {
            count: 1,
            sides: 66,
        });
    }
    if ranged && (2..=6).contains(&first) {
        for sides in SUM_SIDES {
            if top == first * sides {
                return Some(Die {
                    count: first,
                    sides,
                });
            }
        }
    }
    None
}

fn scan_lines(pre: &[String], body: &[String], die: Option<Die>) -> Scan {
    let mut scan = Scan::default();
    if let Some(first) = pre.first() {
        if caption_candidate(first) && parse_marker(first).is_none() {
            scan.caption = Some(first.clone());
        } else {
            scan.unplaced.push(first.clone());
        }
    }
    let mut first_line = pre.is_empty();
    for line in body {
        if let Some((marker, rest)) = parse_marker(line) {
            scan.rows.push(RollRow {
                marker,
                outcome: clean_outcome(rest).to_owned(),
            });
        } else if let Some(last) = scan.rows.last_mut() {
            if !last.outcome.is_empty() {
                last.outcome.push(' ');
            }
            last.outcome.push_str(line);
        } else if first_line && caption_candidate(line) {
            scan.caption = Some(line.clone());
        } else {
            scan.unplaced.push(line.clone());
        }
        first_line = false;
    }
    // Marker rows before the row that starts the cover are not part of the table.
    let start = die.map_or_else(
        || {
            scan.rows.iter().position(|r| r.marker.lo == 1).or_else(|| {
                scan.rows
                    .iter()
                    .position(|r| r.marker.lo == 11 && !r.marker.range)
            })
        },
        |d| {
            let (min, _) = d.range();
            scan.rows.iter().position(|r| r.marker.lo == min)
        },
    );
    if let Some(start) = start.filter(|s| *s > 0) {
        for row in scan.rows.drain(..start) {
            scan.unplaced
                .push(format!("{} {}", row.marker.text, row.outcome));
        }
    }
    scan
}

/// One linear pass over the tokens for the markers `start`, then the next value each time.
fn scan_tokens(pre: &[String], body: &[String], start: u32, d66: bool) -> Scan {
    let mut scan = Scan::default();
    if let Some(first) = pre.first() {
        if caption_candidate(first) && parse_marker(first).is_none() {
            scan.caption = Some(first.clone());
        } else {
            scan.unplaced.push(first.clone());
        }
    }
    let mut next = start;
    let mut before: Vec<&str> = Vec::new();
    for token in body.iter().flat_map(|line| line.split_whitespace()) {
        let marker = parse_token(token).filter(|m| m.lo == next);
        if let Some(marker) = marker {
            next = if marker.plus {
                u32::MAX
            } else {
                next_after(marker.hi, d66)
            };
            scan.rows.push(RollRow {
                marker,
                outcome: String::new(),
            });
        } else if let Some(last) = scan.rows.last_mut() {
            if !last.outcome.is_empty() {
                last.outcome.push(' ');
            }
            last.outcome.push_str(token);
        } else {
            before.push(token);
        }
    }
    if !before.is_empty() {
        let text = before.join(" ");
        if scan.caption.is_none() && pre.is_empty() && caption_candidate(&text) {
            scan.caption = Some(text);
        } else {
            scan.unplaced.push(text);
        }
    }
    scan
}

struct Context<'a> {
    header: Option<&'a (Die, Vec<String>)>,
    total_lines: usize,
    inline: bool,
}

fn assemble(mut scan: Scan, ctx: &Context<'_>) -> Draft {
    let none = |problems: Vec<Problem>| Draft::none(Shape::Roll, problems);
    let unplaced: Vec<Problem> = scan
        .unplaced
        .iter()
        .map(|line| Problem::Unplaced(excerpt(line)))
        .collect();
    let rows = &mut scan.rows;
    if rows.len() < 2 {
        return none(unplaced);
    }
    let last_index = rows.len() - 1;
    if rows[..last_index].iter().any(|r| r.marker.plus) {
        return none(unplaced);
    }
    let Some(die) = ctx.header.map(|h| h.0).or_else(|| infer(rows)) else {
        return none(unplaced);
    };
    let ranged = rows.iter().any(|r| r.marker.range || r.marker.plus);
    if ctx.header.is_none() && !ranged && rows.iter().all(|r| r.marker.weak()) {
        return none(unplaced);
    }
    // A merged paragraph of bare numbers is also what prose with numbers looks like ("On day 1
    // ... On day 2 ..."): without a header or a strong marker, it takes at least six rows.
    if ctx.inline
        && ctx.header.is_none()
        && rows.len() < 6
        && !rows.iter().any(|r| r.marker.range || r.marker.plus)
    {
        return none(unplaced);
    }
    let (min, top) = die.range();
    if rows[last_index].marker.plus {
        rows[last_index].marker.hi = top.max(rows[last_index].marker.lo);
    }
    let d66 = die.sides == 66;
    let mut problems = Vec::new();
    let mut expected = min;
    let mut previous: Option<u32> = None;
    for (index, row) in rows.iter().enumerate() {
        let n = index + 1;
        let lo = row.marker.lo;
        if lo > expected {
            problems.push(Problem::Gap(n));
        } else if lo < expected {
            problems.push(match previous {
                Some(p) if lo == p => Problem::Duplicate(n),
                Some(p) if lo < p => Problem::Interleaved(n),
                _ => Problem::Overlap(n),
            });
        }
        if row.marker.hi > top {
            problems.push(Problem::Overlap(n));
        }
        expected = expected.max(next_after(row.marker.hi, d66));
        previous = Some(lo);
    }
    let last_hi = rows[last_index].marker.hi;
    if last_hi < top {
        problems.push(Problem::Incomplete(last_hi, top));
    }
    let penalty = u32::try_from(problems.len())
        .unwrap_or(u32::MAX)
        .saturating_mul(PROBLEM_PENALTY);

    let parts: Vec<Vec<String>> = rows.iter().map(|r| split_gaps(&r.outcome)).collect();
    let width = parts[0].len();
    let split = width >= 2 && parts.iter().all(|p| p.len() == width);
    let mut cells: Vec<Vec<String>> = rows
        .iter()
        .zip(parts)
        .map(|(row, parts)| {
            let mut cells = vec![row.marker.text.clone()];
            if split {
                cells.extend(parts);
            } else {
                cells.push(row.outcome.clone());
            }
            cells
        })
        .collect();
    let header = ctx.header.is_some();
    if let Some((_, header_cells)) = ctx.header {
        cells.insert(0, header_cells.clone());
    }
    let unplaced_lines = scan.unplaced.len().min(ctx.total_lines);
    let base = u32::try_from(100 * (ctx.total_lines - unplaced_lines) / ctx.total_lines.max(1))
        .unwrap_or(0);
    let mut score = base.saturating_sub(penalty);
    if ctx.inline {
        score = score.min(INLINE_CAP);
    }
    let mut all = problems;
    all.extend(unplaced);
    let lead = scan.caption.into_iter().chain(scan.unplaced).collect();
    Draft::build(Shape::Roll, lead, cells, header, score, all)
}

pub(super) fn read(lines: &[String]) -> Draft {
    if lines.is_empty() {
        return Draft::none(Shape::Roll, Vec::new());
    }
    let header_at = lines
        .iter()
        .take(2)
        .enumerate()
        .find_map(|(i, line)| parse_header(line).map(|h| (i, h)));
    let (pre, body, header) = match &header_at {
        Some((i, h)) => (&lines[..*i], &lines[i + 1..], Some(h)),
        None => (&lines[..0], lines, None),
    };
    let ctx = |inline| Context {
        header,
        total_lines: lines.len(),
        inline,
    };
    let die = header.map(|h| h.0);
    let line_draft = assemble(scan_lines(pre, body, die), &ctx(false));
    if line_draft.score >= 100 {
        return line_draft;
    }
    let starts: Vec<(u32, bool)> = match die {
        Some(d) => vec![(d.range().0, d.sides == 66)],
        None => vec![
            (1, false),
            (11, true),
            (2, false),
            (3, false),
            (4, false),
            (5, false),
            (6, false),
        ],
    };
    let mut best = line_draft;
    for (start, d66) in starts {
        let draft = assemble(scan_tokens(pre, body, start, d66), &ctx(true));
        // The token reading must have found more rows than the line reading to replace it: a
        // repeated marker the lines show (a duplicate) must not be skipped as outcome text.
        if draft.rows.len() > best.rows.len() && draft.score > best.score {
            best = draft;
        }
    }
    best
}
