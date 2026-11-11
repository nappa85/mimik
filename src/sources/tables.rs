//! Owner-built tables: the cell grid the Owner edits, its text form, and the generated HTML.
//!
//! The grid is entered as plain text, one row per line, cells separated by `|`. A literal `|`
//! or `\` inside a cell is written `\|` or `\\`; any other backslash stays as typed. Cells are
//! trimmed; all-empty lines are dropped; short rows are padded with empty cells. The first row
//! can be marked as the header (`<thead><th>`).
use crate::AppError;

/// Columns of the table (and of the `Plain columns` draft, which makes one column per unit).
pub(crate) const MAX_COLUMNS: usize = 16;
/// Units that can be selected for one table: shapes read units as rows (a d20 table is 20 units).
pub(crate) const MAX_TABLE_SOURCES: usize = 200;
/// A page of rules rarely has more than a few dozen rows in one table; 200 keeps the editor
/// form, the generated HTML (64 KiB) and the lookup text (20,000 characters) comfortably bounded.
pub(crate) const MAX_ROWS: usize = 200;
/// A cell is a short fact (a name, a number, a rule line), not a paragraph.
pub(crate) const MAX_CELL_CHARS: usize = 1_000;
/// The same limit as any corrected unit text.
pub(crate) const MAX_PLAIN_CHARS: usize = 20_000;
/// The import/display contract for raw HTML.
pub(crate) const MAX_HTML_BYTES: usize = 65_536;

const INVALID: &str = "Table: 1-200 rows by 1-16 columns, cells of at most 1,000 characters, at most 20,000 characters of text and 64 KiB of HTML; no NUL.";

fn escape(cell: &str) -> String {
    cell.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// Split the editable text into a rectangular grid (bounds measured with `header` applied).
///
/// # Errors
/// Too many rows, columns or characters, NUL, or no nonempty cell.
pub(crate) fn parse_grid(input: &str, header: bool) -> Result<Vec<Vec<String>>, AppError> {
    if input.contains('\0') || input.chars().count() > 4 * MAX_PLAIN_CHARS {
        return Err(AppError::Invalid(INVALID));
    }
    let mut rows = Vec::new();
    for line in input.lines() {
        let line = strip_edges(line);
        let mut cells = vec![String::new()];
        let mut chars = line.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '\\' if matches!(chars.peek(), Some('|' | '\\')) => {
                    cells.last_mut().expect("one cell").extend(chars.next());
                }
                '|' => cells.push(String::new()),
                _ => cells.last_mut().expect("one cell").push(c),
            }
        }
        let cells: Vec<String> = cells.into_iter().map(|c| c.trim().to_owned()).collect();
        if cells.iter().all(String::is_empty) || is_separator(&cells) {
            continue;
        }
        if cells.len() > MAX_COLUMNS || rows.len() >= MAX_ROWS {
            return Err(AppError::Invalid(INVALID));
        }
        rows.push(cells);
    }
    let width = rows.iter().map(Vec::len).max().unwrap_or(0);
    for row in &mut rows {
        row.resize(width, String::new());
    }
    check(&rows, header)?;
    Ok(rows)
}

/// A line written as `| a | b |` (a pasted Markdown table) has its edge bars removed.
fn strip_edges(line: &str) -> &str {
    let t = line.trim();
    if t.len() >= 2 && t.starts_with('|') && t.ends_with('|') {
        let inner = &t[1..t.len() - 1];
        let slashes = inner.len() - inner.trim_end_matches('\\').len();
        if slashes.is_multiple_of(2) {
            return inner;
        }
    }
    line
}

/// A Markdown header separator (`---`, `:---:`), not data.
fn is_separator(cells: &[String]) -> bool {
    cells.iter().all(|c| {
        let c = c.trim_matches(':');
        c.len() >= 3 && c.bytes().all(|b| b == b'-')
    })
}

/// The editable text of a grid (the inverse of [`parse_grid`]).
pub(crate) fn format_grid(rows: &[Vec<String>]) -> String {
    rows.iter()
        .map(|row| {
            let cells = row
                .iter()
                .map(|cell| cell.replace('\\', "\\\\").replace('|', "\\|"))
                .collect::<Vec<_>>()
                .join(" | ");
            format!("| {cells} |")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Structural bounds of a rectangular grid, checked again at the store boundary.
///
/// # Errors
/// Any bound of [`MAX_ROWS`], [`MAX_COLUMNS`], [`MAX_CELL_CHARS`], [`MAX_PLAIN_CHARS`]; ragged rows; NUL.
pub(crate) fn check(rows: &[Vec<String>], header: bool) -> Result<(), AppError> {
    let width = rows.first().map_or(0, Vec::len);
    if rows.is_empty()
        || rows.len() > MAX_ROWS
        || width == 0
        || width > MAX_COLUMNS
        || rows.iter().any(|row| row.len() != width)
        || rows.iter().flatten().all(String::is_empty)
        || rows
            .iter()
            .flatten()
            .any(|cell| cell.chars().count() > MAX_CELL_CHARS || cell.contains('\0'))
        || plain(rows).chars().count() > MAX_PLAIN_CHARS
        || html(rows, header).len() > MAX_HTML_BYTES
    {
        return Err(AppError::Invalid(INVALID));
    }
    Ok(())
}

/// Fixed `table > thead/tbody > tr > td/th` structure with escaped text. With `header` the first
/// row is the `<thead>` row of `<th>` cells; otherwise there is no header row (a Marker table is
/// read the same way, and the grid does not guess one).
pub(crate) fn html(rows: &[Vec<String>], header: bool) -> String {
    let mut out = String::from("<table>");
    for (index, row) in rows.iter().enumerate() {
        let head = header && index == 0;
        if head {
            out.push_str("<thead>");
        } else if index == usize::from(header) {
            out.push_str("<tbody>");
        }
        out.push_str("<tr>");
        let tag = if head { "th" } else { "td" };
        for cell in row {
            out.push('<');
            out.push_str(tag);
            out.push('>');
            out.push_str(&escape(cell));
            out.push_str("</");
            out.push_str(tag);
            out.push('>');
        }
        out.push_str("</tr>");
        if head {
            out.push_str("</thead>");
        }
    }
    if rows.len() <= usize::from(header) {
        out.push_str("<tbody>");
    }
    out.push_str("</tbody></table>");
    out
}

/// The lookup text of a table: one line per row, cells joined by ` | `.
pub(crate) fn plain(rows: &[Vec<String>]) -> String {
    rows.iter()
        .map(|row| row.join(" | "))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_text_round_trips_pipes_and_backslashes() {
        let rows = vec![
            vec!["a | b".to_owned(), "c\\d".to_owned(), String::new()],
            vec!["x".to_owned(), "y".to_owned(), "z".to_owned()],
        ];
        assert_eq!(parse_grid(&format_grid(&rows), false).unwrap(), rows);
    }

    #[test]
    fn short_rows_pad_and_blank_lines_drop() {
        let grid = parse_grid("a | b | c\n\n  \nd", false).unwrap();
        assert_eq!(grid, vec![vec!["a", "b", "c"], vec!["d", "", ""]]);
        assert_eq!(
            parse_grid("a\\x | \\q", false).unwrap(),
            vec![vec!["a\\x", "\\q"]]
        );
    }

    #[test]
    fn pasted_markdown_loses_edge_bars_and_separator_row() {
        let grid = parse_grid(
            "| Name | Dmg |\n| --- | :---: |\n| Sword | d6 |\n| a \\| |",
            false,
        )
        .unwrap();
        assert_eq!(
            grid,
            vec![vec!["Name", "Dmg"], vec!["Sword", "d6"], vec!["a |", ""]]
        );
        let rows = vec![
            vec![String::new(), "x".to_owned()],
            vec!["y".to_owned(), String::new()],
        ];
        assert_eq!(parse_grid(&format_grid(&rows), false).unwrap(), rows);
    }

    #[test]
    fn bounds_are_refused() {
        assert!(parse_grid("", false).is_err());
        assert!(parse_grid(" | ", false).is_err());
        assert!(parse_grid(&"a\n".repeat(201), false).is_err());
        assert!(parse_grid(&vec!["a"; 17].join("|"), false).is_err());
        assert!(parse_grid(&"a".repeat(1001), false).is_err());
        assert!(parse_grid("a\0b", false).is_err());
        assert!(parse_grid(&vec!["a"; 16].join("|"), false).is_ok());
    }

    #[test]
    fn html_is_escaped_in_the_fixed_structure() {
        let out = html(&[vec!["<script>x</script>".into(), "\"&'".into()]], false);
        assert_eq!(
            out,
            "<table><tbody><tr><td>&lt;script&gt;x&lt;/script&gt;</td><td>&quot;&amp;&#39;</td></tr></tbody></table>"
        );
    }

    #[test]
    fn header_row_is_a_thead_of_th_cells_and_counts_toward_the_html_bound() {
        let rows = vec![
            vec!["d6".to_owned(), "Result".to_owned()],
            vec!["1".to_owned(), "Rats".to_owned()],
        ];
        assert_eq!(
            html(&rows, true),
            "<table><thead><tr><th>d6</th><th>Result</th></tr></thead><tbody><tr><td>1</td><td>Rats</td></tr></tbody></table>"
        );
        assert_eq!(
            html(&rows[..1], true),
            "<table><thead><tr><th>d6</th><th>Result</th></tr></thead><tbody></tbody></table>"
        );
        // A grid whose plain HTML is exactly at the bound is refused once the header's extra
        // `<thead>` bytes are counted.
        let mut grid = vec![vec!["&".repeat(100)]];
        while html(&[grid.clone(), vec![vec!["&".repeat(100)]]].concat(), false).len()
            <= MAX_HTML_BYTES
        {
            grid.push(vec!["&".repeat(100)]);
        }
        let need = MAX_HTML_BYTES - html(&grid, false).len();
        grid[0][0].push_str(&"x".repeat(need));
        assert_eq!(html(&grid, false).len(), MAX_HTML_BYTES);
        assert!(check(&grid, false).is_ok());
        assert!(check(&grid, true).is_err());
    }
}
