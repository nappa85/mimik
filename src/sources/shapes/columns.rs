//! Column grid: a table read from the positions of its words. Rows are words grouped by
//! baseline, columns are the edges that recur in at least half the rows. Without word boxes
//! (a page imported without them) each line is split on runs of two or more spaces instead, a
//! weaker reading that scores lower.
use super::{Draft, Shape, ShapeInput, caption_candidate, is_value, ragged, split_gaps};
use crate::sources::page_words::PageWord;

/// The best a word-box reading scores; roll tables and the like (up to 100) still win.
const COLUMN_CAP: usize = 80;
/// The best a two-spaces reading scores: the gaps are only a hint of the columns.
const TEXT_CAP: usize = 60;
/// A word joins a row when it overlaps the row's baseline band by more than this share of the
/// smaller height.
const SAME_ROW: f32 = 0.5;
/// Column edges within this many median character widths are one column.
const TOLERANCE: f32 = 0.6;
/// Never tighter than this, in points (a page of boxes rounded to tenths).
const MIN_TOLERANCE: f32 = 0.5;

pub(super) fn read(input: &ShapeInput) -> Draft {
    match input.words.as_deref() {
        Some(words) if !words.is_empty() => from_words(words),
        _ => from_lines(&input.lines()),
    }
}

/// A small count as a float (rows and characters are far below `u16::MAX`).
fn count(n: usize) -> f32 {
    f32::from(u16::try_from(n).unwrap_or(u16::MAX))
}

fn numeric_cell(cell: &str) -> bool {
    !cell.is_empty() && cell.split_whitespace().all(is_value)
}

fn median(mut values: Vec<f32>) -> f32 {
    values.sort_unstable_by(f32::total_cmp);
    values.get(values.len() / 2).copied().unwrap_or(0.0)
}

/// Words grouped by baseline, each row left to right.
fn rows_of(words: &[PageWord]) -> Vec<Vec<&PageWord>> {
    let mut order: Vec<&PageWord> = words.iter().collect();
    order.sort_by(|a, b| a.centre().1.total_cmp(&b.centre().1));
    let mut rows: Vec<Vec<&PageWord>> = Vec::new();
    for word in order {
        let joined = rows.last().is_some_and(|row| {
            let n = count(row.len());
            let top = row.iter().map(|w| w.bounds[1]).sum::<f32>() / n;
            let bottom = row.iter().map(|w| w.bounds[3]).sum::<f32>() / n;
            let overlap = bottom.min(word.bounds[3]) - top.max(word.bounds[1]);
            overlap > SAME_ROW * (bottom - top).min(word.bounds[3] - word.bounds[1])
        });
        if joined {
            if let Some(row) = rows.last_mut() {
                row.push(word);
            }
        } else {
            rows.push(vec![word]);
        }
    }
    for row in &mut rows {
        row.sort_by(|a, b| a.bounds[0].total_cmp(&b.bounds[0]));
    }
    rows
}

/// One edge of a word: where it sits, in which row, and where its word starts.
struct Edge {
    at: f32,
    row: usize,
    start: f32,
}

/// The starts of the columns that recur in at least half the rows: edges closer than
/// `tolerance` are one cluster, and a cluster is a column when it has a word in half the rows.
fn recurring(mut edges: Vec<Edge>, rows: usize, tolerance: f32) -> Vec<f32> {
    edges.sort_by(|a, b| a.at.total_cmp(&b.at));
    let mut starts = Vec::new();
    let mut cluster: Vec<&Edge> = Vec::new();
    let mut close = |cluster: &mut Vec<&Edge>| {
        let mut seen: Vec<usize> = cluster.iter().map(|e| e.row).collect();
        seen.sort_unstable();
        seen.dedup();
        if !seen.is_empty() && seen.len() * 2 >= rows {
            starts.push(cluster.iter().map(|e| e.start).fold(f32::MAX, f32::min));
        }
        cluster.clear();
    };
    for edge in &edges {
        if cluster
            .last()
            .is_some_and(|last| edge.at - last.at > tolerance)
        {
            close(&mut cluster);
        }
        cluster.push(edge);
    }
    close(&mut cluster);
    starts
}

fn merged(mut starts: Vec<f32>, tolerance: f32) -> Vec<f32> {
    starts.sort_unstable_by(f32::total_cmp);
    let mut out: Vec<f32> = Vec::new();
    for start in starts {
        if out.last().is_none_or(|last| start - last > tolerance) {
            out.push(start);
        }
    }
    out
}

fn edges(rows: &[Vec<&PageWord>], pick: impl Fn(&PageWord) -> Option<f32>) -> Vec<Edge> {
    let mut out = Vec::new();
    for (row, words) in rows.iter().enumerate() {
        for word in words {
            if let Some(at) = pick(word) {
                out.push(Edge {
                    at,
                    row,
                    start: word.bounds[0],
                });
            }
        }
    }
    out
}

fn header_row(rows: &[Vec<String>]) -> bool {
    let Some((first, rest)) = rows.split_first() else {
        return false;
    };
    if rest.is_empty() || first.iter().any(|cell| numeric_cell(cell)) {
        return false;
    }
    let numeric_rows = rest
        .iter()
        .filter(|row| {
            let filled = row.iter().filter(|c| !c.is_empty()).count();
            let numeric = row.iter().filter(|c| numeric_cell(c)).count();
            numeric > 0 && numeric * 2 >= filled
        })
        .count();
    numeric_rows * 2 >= rest.len()
}

fn from_words(words: &[PageWord]) -> Draft {
    let width = median(
        words
            .iter()
            .map(|w| (w.bounds[2] - w.bounds[0]) / count(w.text.chars().count().max(1)))
            .collect(),
    );
    let tolerance = (TOLERANCE * width).max(MIN_TOLERANCE);
    let rows = rows_of(words);
    if rows.len() < 2 {
        return Draft::none(Shape::Column, Vec::new());
    }
    let numeric = |w: &PageWord| numeric_cell(&w.text);
    // Left edges for text, right edges for numbers; centres only when those find no grid.
    let mut starts = recurring(edges(&rows, |w| Some(w.bounds[0])), rows.len(), tolerance);
    starts.extend(recurring(
        edges(&rows, |w| numeric(w).then_some(w.bounds[2])),
        rows.len(),
        tolerance,
    ));
    let mut starts = merged(starts, tolerance);
    if starts.len() < 2 {
        starts = merged(
            recurring(edges(&rows, |w| Some(w.centre().0)), rows.len(), tolerance),
            tolerance,
        );
    }
    if starts.len() < 2 {
        return Draft::none(Shape::Column, Vec::new());
    }
    let grid: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            let mut cells = vec![String::new(); starts.len()];
            for word in row {
                let column = starts
                    .partition_point(|s| *s <= word.bounds[0] + tolerance)
                    .saturating_sub(1);
                if !cells[column].is_empty() {
                    cells[column].push(' ');
                }
                cells[column].push_str(&word.text);
            }
            cells
        })
        .collect();
    let filled: Vec<Vec<String>> = grid
        .iter()
        .map(|row| row.iter().filter(|c| !c.is_empty()).cloned().collect())
        .collect();
    let (_, problems) = ragged(&filled);
    let score = COLUMN_CAP * (grid.len() - problems.len()) / grid.len();
    let header = header_row(&grid);
    Draft::build(
        Shape::Column,
        Vec::new(),
        grid,
        header,
        u32::try_from(score).unwrap_or(0),
        problems,
    )
}

fn from_lines(lines: &[String]) -> Draft {
    let split: Vec<Vec<String>> = lines.iter().map(|line| split_gaps(line)).collect();
    let caption = split.first().is_some_and(|cells| cells.len() < 2)
        && lines.first().is_some_and(|line| caption_candidate(line));
    let body = lines.len() - usize::from(caption);
    let grid_rows = split.iter().filter(|cells| cells.len() >= 2).count();
    if grid_rows < 2 || grid_rows * 2 < body {
        return Draft::none(Shape::Column, Vec::new());
    }
    let lead: Vec<String> = lines.iter().take(usize::from(caption)).cloned().collect();
    let rows: Vec<Vec<String>> = split.into_iter().skip(usize::from(caption)).collect();
    let (modal, problems) = ragged(&rows);
    if modal < 2 {
        return Draft::none(Shape::Column, Vec::new());
    }
    let consumed = rows.len() - problems.len() + usize::from(caption);
    let score = u32::try_from(TEXT_CAP * consumed / lines.len()).unwrap_or(0);
    let header = header_row(&rows);
    Draft::build(Shape::Column, lead, rows, header, score, problems)
}
