//! Lexical retrieval over reviewed units: `SQLite` FTS5 (`unicode61 remove_diacritics 2`),
//! bm25 ranking, eligibility filters in the same statement as MATCH.
use super::{Evidence, EvidenceSearch, sanitized};
use crate::AppError;
use rusqlite::{Connection, ErrorCode, params};
use std::time::{Duration, Instant};

/// Schema version (migration number) that introduces the search tables.
pub const SEARCH_SCHEMA: i64 = 17;
const _: () = assert!(SEARCH_SCHEMA <= crate::store::SCHEMA_VERSION);
/// Wall-clock budget for one lookup; `SQLite` is interrupted past it (the request timeout
/// cannot interrupt a running statement that holds the connection mutex).
pub(crate) const QUERY_BUDGET: Duration = Duration::from_secs(1);
/// `SQLite` VM instructions between budget checks.
const BUDGET_CHECK_OPS: i32 = 10_000;
/// Units derived per backfill batch, bounding memory to batch x 64 KiB of raw HTML.
const BACKFILL_BATCH: i64 = 64;
/// Raw HTML byte bound shared with import and the materials byte guards.
const MAX_RAW_HTML_BYTES: usize = 64 * 1024;

/// Bump whenever [`search_text`] output can change (including html2text upgrades);
/// the golden test below fails until this is bumped deliberately.
pub(crate) const SEARCH_NORMALISER: i64 = 2;
/// Matches the `source_search_text.search_text` CHECK constraint.
const MAX_SEARCH_TEXT_BYTES: usize = 256 * 1024;
/// Wide enough that no unit (raw HTML is at most 64 KiB) is ever wrapped.
const UNWRAPPED_WIDTH: usize = 1 << 20;
pub(crate) const MATCH_LIMIT: usize = 50;
const MAX_QUERY_PIECES: usize = 16;

/// Separator between the cells of a table row in the search text (the same as the lookup
/// text of owner tables, `tables::plain`).
const CELL_SEPARATOR: &str = " | ";

/// Inline tags of the sanitiser allowlist that do not separate words inside a cell.
const INLINE_TAGS: [&str; 7] = ["b", "strong", "i", "em", "sub", "sup", "code"];

/// Where text inside a table goes.
#[derive(Clone, Copy, PartialEq)]
enum Open {
    /// Between rows or cells: text becomes its own line (a caption unwrapped by the
    /// sanitiser, for instance).
    Nothing,
    /// Inside a stray-text line.
    Stray,
    /// Inside the last cell of the last row.
    Cell,
}

/// Rewrite every table of sanitised HTML as one `<pre>` block: one line per row (`tr`),
/// cells in order joined by [`CELL_SEPARATOR`]. Cell text keeps its entities, loses its
/// markup (block tags and `br` become a space) and has whitespace collapsed; empty cells
/// stay as empty positions so columns stay aligned (`a |  | c`); `colspan`/`rowspan` are
/// not expanded. A table nested in a cell is flattened into that cell's text. Text in a
/// table but outside any cell gets its own line. HTML without a table is returned
/// unchanged.
///
/// Relies on the guarantees of [`sanitized`]: lowercase tags, `<` escaped in text, and no
/// attribute other than numeric `colspan`/`rowspan`, so a `<` always starts a tag and a
/// tag ends at the first `>`.
fn flatten_tables(clean: &str) -> std::borrow::Cow<'_, str> {
    if !clean.contains("<table") {
        return std::borrow::Cow::Borrowed(clean);
    }
    let mut out = String::with_capacity(clean.len());
    let mut rest = clean;
    let mut depth = 0_usize;
    let mut lines: Vec<Vec<String>> = Vec::new();
    let mut open = Open::Nothing;
    while !rest.is_empty() {
        let (text, tag_start) = rest.split_at(rest.find('<').unwrap_or(rest.len()));
        if depth == 0 {
            out.push_str(text);
        } else {
            append_text(&mut lines, &mut open, text);
        }
        let end = tag_start.find('>').map_or(tag_start.len(), |i| i + 1);
        let tag = &tag_start[..end];
        rest = &tag_start[end..];
        if tag.is_empty() {
            break;
        }
        let name = tag
            .trim_start_matches('<')
            .trim_start_matches('/')
            .split(|c: char| c.is_whitespace() || c == '>' || c == '/')
            .next()
            .unwrap_or("");
        let closing = tag.starts_with("</");
        match (name, closing) {
            ("table", false) => {
                depth += 1;
                if depth == 1 {
                    lines.clear();
                    open = Open::Nothing;
                } else {
                    append_text(&mut lines, &mut open, " ");
                }
            }
            ("table", true) if depth > 0 => {
                depth -= 1;
                if depth == 0 {
                    out.push_str("<pre>");
                    let rendered: Vec<String> = lines
                        .iter()
                        .map(|line| {
                            line.iter()
                                .map(|cell| cell.trim_end())
                                .collect::<Vec<_>>()
                                .join(CELL_SEPARATOR)
                        })
                        .collect();
                    out.push_str(&rendered.join("\n"));
                    out.push_str("</pre>");
                } else {
                    append_text(&mut lines, &mut open, " ");
                }
            }
            ("tr", false) if depth == 1 => {
                lines.push(Vec::new());
                open = Open::Nothing;
            }
            ("td" | "th", false) if depth == 1 => {
                if open == Open::Stray || lines.is_empty() {
                    lines.push(Vec::new());
                }
                if let Some(line) = lines.last_mut() {
                    line.push(String::new());
                }
                open = Open::Cell;
            }
            ("tr" | "td" | "th", true) if depth == 1 => open = Open::Nothing,
            _ if depth == 0 => out.push_str(tag),
            _ if INLINE_TAGS.contains(&name) => {}
            _ => append_text(&mut lines, &mut open, " "),
        }
    }
    std::borrow::Cow::Owned(out)
}

/// Append text to the open cell, collapsing whitespace runs to one space. Outside a cell,
/// non-blank text starts (or continues) a line of its own.
fn append_text(lines: &mut Vec<Vec<String>>, open: &mut Open, text: &str) {
    if text.trim().is_empty() && *open != Open::Cell && *open != Open::Stray {
        return;
    }
    if *open == Open::Nothing {
        lines.push(vec![String::new()]);
        *open = Open::Stray;
    }
    let Some(cell) = lines.last_mut().and_then(|line| line.last_mut()) else {
        return;
    };
    let mut gap = text.starts_with(char::is_whitespace);
    for word in text.split_whitespace() {
        if gap && !cell.is_empty() && !cell.ends_with(' ') {
            cell.push(' ');
        }
        cell.push_str(word);
        gap = true;
    }
    if text.ends_with(char::is_whitespace) && !cell.is_empty() && !cell.ends_with(' ') {
        cell.push(' ');
    }
}

/// Unwrapped, undecorated plain text for indexing. Each table row is one line, cells
/// joined by ` | ` (see [`flatten_tables`]), so a search hit keeps which cells share a row
/// and a quoted phrase can match across adjacent cells and rows (documented behaviour).
///
/// # Errors
/// Rejects unsanitizable HTML or output above the stored byte bound.
pub(crate) fn search_text(raw_html: &str) -> Result<String, AppError> {
    let sanitised = sanitized(raw_html)?;
    let clean = flatten_tables(&sanitised);
    let text = html2text::config::plain_no_decorate()
        .raw_mode(true)
        .no_table_borders()
        .allow_width_overflow()
        .string_from_read(clean.as_bytes(), UNWRAPPED_WIDTH)
        .map_err(|_| AppError::Invalid("Unit text could not be normalised for search."))?;
    if text.len() > MAX_SEARCH_TEXT_BYTES || text.contains('\0') {
        return Err(AppError::Invalid(
            "Normalised unit search text exceeds 256 KiB or contains NUL; no search data changed.",
        ));
    }
    Ok(text)
}

fn quoted(piece: &str) -> String {
    format!("\"{}\"", piece.replace('"', "\"\""))
}

/// Build an FTS5 MATCH expression that only contains quoted strings generated here, so
/// user text can never introduce operators, column filters or NEAR groups.
///
/// Balanced `"..."` groups are exact phrases; other whitespace-separated words; all are
/// joined with a generated `AND`.
/// A word with at least three letters/digits matches exactly or as a prefix
/// (`("w" OR "w"*)`), so exact occurrences score higher under bm25.
///
/// # Errors
/// Rejects control characters, more than 16 words/phrases, or no searchable words.
pub(crate) fn fts_query(input: &str) -> Result<String, AppError> {
    if input.chars().any(char::is_control) {
        return Err(AppError::Invalid(
            "Search text must not contain control characters.",
        ));
    }
    let mut pieces: Vec<(bool, String)> = Vec::new();
    let mut rest = input;
    while !rest.is_empty() {
        let (before, after) = match rest.find('"') {
            Some(start) => match rest[start + 1..].find('"') {
                Some(end) => {
                    let phrase = &rest[start + 1..start + 1 + end];
                    (&rest[..start], Some((phrase, &rest[start + 2 + end..])))
                }
                // An unbalanced quote is only a separator.
                None => (rest, None),
            },
            None => (rest, None),
        };
        pieces.extend(
            before
                .split(|c: char| c.is_whitespace() || c == '"')
                .map(|word| (false, word.to_owned())),
        );
        match after {
            Some((phrase, tail)) => {
                pieces.push((true, phrase.to_owned()));
                rest = tail;
            }
            None => break,
        }
    }
    // Pieces without letters/digits produce no unicode61 tokens and are dropped.
    pieces.retain(|(_, piece)| piece.chars().any(char::is_alphanumeric));
    if pieces.is_empty() {
        return Err(AppError::Invalid(
            "Search text needs at least one word with letters or digits.",
        ));
    }
    if pieces.len() > MAX_QUERY_PIECES {
        return Err(AppError::Invalid(
            "Search text supports at most 16 words or quoted phrases.",
        ));
    }
    Ok(pieces
        .iter()
        .map(|(phrase, piece)| {
            if !phrase && piece.chars().filter(|c| c.is_alphanumeric()).count() >= 3 {
                let exact = quoted(piece);
                format!("({exact} OR {exact}*)")
            } else {
                quoted(piece)
            }
        })
        .collect::<Vec<_>>()
        .join(" AND "))
}

/// Byte-guarded text read (as in materials): NULL means oversized or wrong type.
pub(crate) fn guarded(column: &str, limit: usize) -> String {
    format!(
        "CASE WHEN typeof({column})='text' AND length(CAST({column} AS BLOB))<={limit} THEN {column} END"
    )
}

pub(crate) fn read_guarded(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<String> {
    row.get::<_, Option<String>>(index)?.ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Null,
            "Evidence text exceeds the supported byte limit or has an invalid type.".into(),
        )
    })
}

/// Exact edition selection; unresolved legacy labels exclude associated sources.
fn predicate(registered: bool) -> &'static str {
    if registered {
        "r.game_id=?1 AND r.edition_id=?2"
    } else {
        "r.source_id IS NULL AND s.system_id=?1 AND s.edition=?2"
    }
}

/// The FTS table drives the join (CROSS JOIN fixes the order) so cost follows matches.
pub(crate) fn match_sql(registered: bool) -> String {
    let predicate = predicate(registered);
    format!(
        "SELECT u.id,{},{},{},u.revision,u.state,p.id,p.revision,s.id,{},p.pdf_page,{},CASE WHEN u.state='corrected' THEN {} ELSE {} END \
         FROM source_search_fts CROSS JOIN source_units u ON u.id=source_search_fts.rowid \
         CROSS JOIN source_pages p ON p.id=u.page_id CROSS JOIN sources s ON s.id=p.source_id \
         CROSS JOIN source_primary_pages primary_page ON primary_page.source_id=s.id AND primary_page.pdf_page=p.pdf_page \
         LEFT JOIN source_rulesets r ON r.source_id=s.id \
         LEFT JOIN source_search_text t ON t.unit_id=u.id \
         WHERE source_search_fts MATCH ?3 AND {predicate} AND p.complete=1 AND u.state IN ('accepted','corrected') \
         AND (?4 IS NULL OR s.id IN (SELECT value FROM json_each(?4))) \
         ORDER BY bm25(source_search_fts),s.id,p.pdf_page,u.id LIMIT {}",
        guarded("u.block_id", 512),
        guarded("u.kind", 100),
        guarded("u.polygon", 1024),
        guarded("s.title", 200),
        guarded("p.printed_label", 200),
        guarded("u.correction", 80_000),
        guarded("t.search_text", MAX_SEARCH_TEXT_BYTES),
        MATCH_LIMIT + 1
    )
}

/// How many VM operations pass between two calls of the connection's progress handler. A
/// production connection checks every `BUDGET_CHECK_OPS`; test builds call the handler on every
/// operation so `Budget::checking_every` can ask for any interval. That costs a call per VM
/// operation on every Store connection of every unit test; integration tests under `tests/` link the
/// library without `cfg(test)` and use the production interval.
#[cfg(not(test))]
const HANDLER_OPS: i32 = BUDGET_CHECK_OPS;
#[cfg(test)]
const HANDLER_OPS: i32 = 1;

/// The time budgets armed on this thread (a stack), read by the progress handler of the connection that
/// executes the guarded statements (SQLite runs it on the calling thread).
struct Active {
    deadline: Instant,
    every: i32,
    seen: i32,
    fired: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

thread_local! {
    static ACTIVE: std::cell::RefCell<Vec<Active>> = const { std::cell::RefCell::new(Vec::new()) };
}

fn budget_exceeded() -> bool {
    ACTIVE.with(|active| {
        let mut active = active.borrow_mut();
        let Some(budget) = active.last_mut() else {
            return false;
        };
        budget.seen = budget.seen.saturating_add(HANDLER_OPS);
        if budget.seen < budget.every {
            return false;
        }
        budget.seen = 0;
        let expired = Instant::now() >= budget.deadline;
        if expired {
            budget
                .fired
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
        expired
    })
}

/// Installs the progress handler that enforces `Budget` on `db`. rusqlite 0.43 needs exclusive
/// access to set a handler, so it is installed once, when the store opens its connection, and
/// stays: with no `Budget` running on the thread it never interrupts. A connection without it
/// ignores every `Budget`.
///
/// # Errors
/// SQLite refuses the handler.
pub(crate) fn install_budget_handler(db: &mut Connection) -> rusqlite::Result<()> {
    db.progress_handler(HANDLER_OPS, Some(budget_exceeded as fn() -> bool))
}

/// Arms the time budget of the statements run on this thread until it is dropped. Budgets form a
/// stack (the innermost one is checked); a drop removes its own entry whatever the order.
pub(crate) struct Budget<'a> {
    interrupted: std::sync::Arc<std::sync::atomic::AtomicBool>,
    db: std::marker::PhantomData<&'a Connection>,
}

impl<'a> Budget<'a> {
    pub(crate) fn start(db: &'a Connection, budget: Duration) -> Self {
        Self::checking_every(db, budget, BUDGET_CHECK_OPS)
    }

    /// `start` with another check interval (tests that must interrupt very short reads).
    pub(crate) fn checking_every(_db: &'a Connection, budget: Duration, ops: i32) -> Self {
        let interrupted = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let armed = Active {
            deadline: Instant::now() + budget,
            every: ops,
            seen: 0,
            fired: interrupted.clone(),
        };
        ACTIVE.with(|active| active.borrow_mut().push(armed));
        Self {
            interrupted,
            db: std::marker::PhantomData,
        }
    }

    pub(crate) fn fired(&self) -> bool {
        self.interrupted.load(std::sync::atomic::Ordering::Relaxed)
    }
}

impl Drop for Budget<'_> {
    fn drop(&mut self) {
        let ours = &self.interrupted;
        ACTIVE.with(|active| {
            active
                .borrow_mut()
                .retain(|armed| !std::sync::Arc::ptr_eq(&armed.fired, ours));
        });
    }
}

pub(crate) fn interrupted(error: rusqlite::Error) -> AppError {
    if error.sqlite_error_code() == Some(ErrorCode::OperationInterrupted) {
        AppError::Invalid("Query too broad for the lookup time budget; add more specific words.")
    } else {
        error.into()
    }
}

pub(crate) fn lookup(
    db: &Connection,
    system: &str,
    edition: &str,
    query: &str,
    registered: bool,
    sources: Option<&[i64]>,
) -> Result<EvidenceSearch, AppError> {
    lookup_within(
        db,
        system,
        edition,
        query,
        registered,
        sources,
        QUERY_BUDGET,
    )
}

fn lookup_within(
    db: &Connection,
    system: &str,
    edition: &str,
    query: &str,
    registered: bool,
    sources: Option<&[i64]>,
    budget: Duration,
) -> Result<EvidenceSearch, AppError> {
    let expression = fts_query(query.trim())?;
    let predicate = predicate(registered);
    let set = sources.map(serde_json::to_string).transpose()?;
    // The budget covers every statement of the lookup (counts and matches). It bounds
    // VM-level work: one long FTS5 xFilter call is only interrupted at its next opcode.
    let _budget = Budget::start(db, budget);
    let from = "sources s LEFT JOIN source_rulesets r ON r.source_id=s.id";
    let count = |sql: String| -> Result<i64, AppError> {
        db.query_row(&sql, params![system, edition], |row| row.get(0))
            .map_err(interrupted)
    };
    let source_count = count(format!("SELECT count(*) FROM {from} WHERE {predicate}"))?;
    let primary = "JOIN source_primary_pages primary_page ON primary_page.source_id=s.id AND primary_page.pdf_page=p.pdf_page";
    let incomplete_pages = count(format!(
        "SELECT count(*) FROM {from} JOIN source_pages p ON p.source_id=s.id {primary} WHERE {predicate} AND p.complete=0"
    ))?;
    let partial_runs = count(format!(
        "SELECT count(*) FROM (SELECT DISTINCT m.document_id FROM {from} JOIN source_document_members m ON m.source_id=s.id WHERE {predicate} AND EXISTS(SELECT 1 FROM source_primary_pages v WHERE v.source_id=s.id) AND (s.total_pages=0 OR s.total_pages<>(SELECT count(*) FROM source_primary_pages v JOIN sources s ON s.id=v.source_id LEFT JOIN source_rulesets r ON r.source_id=s.id WHERE v.document_id=m.document_id AND {predicate})))"
    ))?;
    let unindexed_units = count(format!(
        "SELECT count(*) FROM {from} JOIN source_pages p ON p.source_id=s.id {primary} JOIN source_units u ON u.page_id=p.id WHERE {predicate} AND p.complete=1 AND u.state='accepted' AND NOT EXISTS(SELECT 1 FROM source_search_text t WHERE t.unit_id=u.id)"
    ))?;
    let needs_primary_choice = count(format!(
        "SELECT count(*) FROM (SELECT DISTINCT m.document_id,p.pdf_page FROM {from} JOIN source_document_members m ON m.source_id=s.id JOIN source_pages p ON p.source_id=s.id WHERE {predicate} AND NOT EXISTS(SELECT 1 FROM source_primary_pages v WHERE v.document_id=m.document_id AND v.pdf_page=p.pdf_page))"
    ))?;
    let primary_choice_sources = {
        let mut statement = db.prepare(&format!("SELECT DISTINCT s.id FROM {from} JOIN source_document_members m ON m.source_id=s.id JOIN source_pages p ON p.source_id=s.id WHERE {predicate} AND NOT EXISTS(SELECT 1 FROM source_primary_pages v WHERE v.document_id=m.document_id AND v.pdf_page=p.pdf_page) ORDER BY s.id LIMIT 50"))?;
        statement
            .query_map(params![system, edition], |r| r.get(0))
            .map_err(interrupted)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(interrupted)?
    };
    let mut statement = db.prepare(&match_sql(registered))?;
    let mut matches = statement
        .query_map(params![system, edition, expression, set], |row| {
            Ok(Evidence {
                unit_id: row.get(0)?,
                block_id: read_guarded(row, 1)?,
                kind: read_guarded(row, 2)?,
                polygon: read_guarded(row, 3)?,
                unit_revision: row.get(4)?,
                state: row.get(5)?,
                page_id: row.get(6)?,
                page_revision: row.get(7)?,
                source_id: row.get(8)?,
                title: read_guarded(row, 9)?,
                pdf_page: row.get(10)?,
                printed_label: read_guarded(row, 11)?,
                text: read_guarded(row, 12)?,
            })
        })
        .map_err(interrupted)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(interrupted)?;
    drop(statement);
    let truncated = matches.len() > MATCH_LIMIT;
    matches.truncate(MATCH_LIMIT);
    Ok(EvidenceSearch {
        source_count,
        incomplete_pages,
        partial_runs,
        unindexed_units,
        needs_primary_choice,
        primary_choice_sources,
        truncated,
        matches,
    })
}

/// Derive missing/outdated search text in bounded batches. Units that cannot be
/// normalised are recorded in `source_search_failures` (retried when the normaliser
/// version changes) and stay out of the index; they never abort the open.
///
/// Returns the number of units currently recorded as failed for this normaliser.
fn derive_missing(db: &Connection) -> Result<i64, AppError> {
    derive_with(db, SEARCH_NORMALISER, search_text)
}

fn derive_with(
    db: &Connection,
    normaliser: i64,
    normalise: fn(&str) -> Result<String, AppError>,
) -> Result<i64, AppError> {
    let select = format!(
        "SELECT u.id,{} FROM source_units u LEFT JOIN source_search_text t ON t.unit_id=u.id LEFT JOIN source_search_failures f ON f.unit_id=u.id \
         WHERE u.id>?1 AND (t.unit_id IS NULL OR t.normaliser<>?2) AND (f.unit_id IS NULL OR f.normaliser<>?2) ORDER BY u.id LIMIT ?3",
        guarded("u.raw_html", MAX_RAW_HTML_BYTES)
    );
    let mut query = db.prepare(&select)?;
    let mut cursor = 0;
    loop {
        let batch = query
            .query_map(params![cursor, normaliser, BACKFILL_BATCH], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let Some(&(last, _)) = batch.last() else {
            break;
        };
        cursor = last;
        for (id, html) in batch {
            let derived = html
                .ok_or(AppError::Invalid(
                    "Raw unit HTML exceeds 64 KiB or is not text.",
                ))
                .and_then(|html| normalise(&html));
            match derived {
                Ok(text) => {
                    db.execute("INSERT INTO source_search_text(unit_id,normaliser,search_text) VALUES (?1,?2,?3) ON CONFLICT(unit_id) DO UPDATE SET normaliser=excluded.normaliser,search_text=excluded.search_text", params![id, normaliser, text])?;
                    db.execute("DELETE FROM source_search_failures WHERE unit_id=?1", [id])?;
                }
                Err(AppError::Invalid(reason)) => {
                    db.execute("INSERT INTO source_search_failures(unit_id,normaliser,reason) VALUES (?1,?2,?3) ON CONFLICT(unit_id) DO UPDATE SET normaliser=excluded.normaliser,reason=excluded.reason", params![id, normaliser, reason])?;
                    // A projection from an older normaliser is stale: drop it and its index row.
                    db.execute(
                        "DELETE FROM source_search_text WHERE unit_id=?1 AND normaliser<>?2",
                        params![id, normaliser],
                    )?;
                }
                Err(other) => return Err(other),
            }
        }
    }
    Ok(db.query_row(
        "SELECT count(*) FROM source_search_failures WHERE normaliser=?1",
        [normaliser],
        |row| row.get(0),
    )?)
}

const EXPECTED_INDEX: &str = "SELECT u.id, u.correction FROM source_units u WHERE u.state='corrected' UNION ALL SELECT u.id, t.search_text FROM source_units u JOIN source_search_text t ON t.unit_id=u.id WHERE u.state='accepted'";

/// Rebuild the FTS table from the eligible set and run FTS5's integrity check.
///
/// # Errors
/// Storage failures or a failed integrity check.
pub(crate) fn rebuild(db: &Connection) -> Result<i64, AppError> {
    let failed = derive_missing(db)?;
    db.execute_batch(&format!("DELETE FROM source_search_fts; INSERT INTO source_search_fts(rowid,body) {EXPECTED_INDEX}; INSERT INTO source_search_fts(source_search_fts) VALUES('integrity-check');"))?;
    Ok(failed)
}

/// Every open: batched derivation of missing rows plus a count comparison; full rebuild
/// (with FTS5 integrity check) only when the counts disagree. An upgrade creates empty
/// tables, so any eligible unit forces that rebuild. Equal counts with stale content are
/// not detected here; use [`crate::Store::rebuild_search_index`]. A changed normaliser
/// re-derives every unit inside the caller's single transaction (startup blocks until done;
/// one line is printed with the count) and then optimises the FTS index. Returns the
/// failed-unit count.
///
/// # Errors
/// Storage failures only; unnormalisable units are recorded, not fatal.
pub(crate) fn ensure(db: &Connection) -> Result<i64, AppError> {
    let stale: i64 = db.query_row(
        "SELECT count(*) FROM source_search_text WHERE normaliser<>?1",
        [SEARCH_NORMALISER],
        |row| row.get(0),
    )?;
    if stale > 0 {
        eprintln!(
            "Re-deriving the search text of {stale} unit(s) for a new search normaliser; startup waits for it."
        );
    }
    let failed = derive_missing(db)?;
    if stale > 0 {
        db.execute_batch("INSERT INTO source_search_fts(source_search_fts) VALUES('optimize');")?;
    }
    let (indexed, eligible): (i64, i64) = db.query_row(&format!("SELECT (SELECT count(*) FROM source_search_fts),(SELECT count(*) FROM ({EXPECTED_INDEX}))"), [], |row| Ok((row.get(0)?, row.get(1)?)))?;
    if indexed != eligible {
        rebuild(db)?;
    }
    Ok(failed)
}

/// Test support: drop every `source_search_*` object found in `sqlite_master`.
#[cfg(test)]
pub(crate) fn drop_schema(db: &Connection) -> Result<(), AppError> {
    super::documents::rewind_schema(db)?;
    let objects = |kind: &str| -> Result<Vec<String>, AppError> {
        let mut query = db.prepare("SELECT name FROM sqlite_master WHERE type=?1 AND name GLOB 'source_search_*' AND sql IS NOT NULL ORDER BY sql LIKE 'CREATE VIRTUAL TABLE%' DESC, name")?;
        Ok(query
            .query_map([kind], |row| row.get(0))?
            .collect::<Result<_, _>>()?)
    };
    for trigger in objects("trigger")? {
        db.execute_batch(&format!("DROP TRIGGER \"{trigger}\";"))?;
    }
    for table in objects("table")? {
        // Dropping the FTS table removes its shadow tables, which are listed after it.
        db.execute_batch(&format!("DROP TABLE IF EXISTS \"{table}\";"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn armed() -> usize {
        ACTIVE.with(|active| active.borrow().len())
    }

    #[test]
    fn nested_budgets_restore_the_outer_one_and_leave_nothing_armed() {
        let db = Connection::open_in_memory().unwrap();
        {
            let _outer = Budget::start(&db, Duration::from_secs(60));
            assert_eq!(armed(), 1);
            {
                let _inner = Budget::start(&db, Duration::ZERO);
                assert_eq!(armed(), 2);
            }
            assert_eq!(armed(), 1);
            assert!(ACTIVE.with(|active| active.borrow()[0].deadline > Instant::now()));
        }
        assert_eq!(armed(), 0);
    }

    #[test]
    fn out_of_order_drops_never_leave_an_expired_budget_armed() {
        let db = Connection::open_in_memory().unwrap();
        let outer = Budget::start(&db, Duration::ZERO);
        let inner = Budget::start(&db, Duration::from_secs(60));
        drop(outer);
        assert_eq!(armed(), 1);
        assert!(ACTIVE.with(|active| active.borrow()[0].deadline > Instant::now()));
        drop(inner);
        assert_eq!(armed(), 0);
    }

    #[test]
    fn normaliser_output_is_golden() {
        // If this fails after a dependency upgrade, bump SEARCH_NORMALISER and update.
        assert_eq!(SEARCH_NORMALISER, 2);
        let html = "<h2>Armi</h2><p>Una <b>spada</b> <i>lunga</i> che continua per molte parole senza mai andare a capo anche oltre centoventi colonne di testo continuo qui</p><table><tr><th>Arma</th><th>Danno</th></tr><tr><td>Spada lunga a due mani</td><td>d8 più forza</td></tr></table><ul><li>uno</li><li>due</li></ul><ol><li>tre</li></ol><blockquote>citazione</blockquote><p>a<br>b</p>";
        assert_eq!(
            search_text(html).unwrap(),
            // Structural markers (`##`, `*`, `1.`, `>`) remain; unicode61 drops the
            // punctuation, but ordered-list numbers are indexed as number tokens.
            "## Armi\n\nUna spada lunga che continua per molte parole senza mai andare a capo anche oltre centoventi colonne di testo continuo qui\n\nArma | Danno\nSpada lunga a due mani | d8 più forza\n* uno\n* due\n1. tre\n\n> citazione\n\na\nb\n"
        );
    }

    #[test]
    fn lookup_is_interrupted_past_its_time_budget() {
        let dir = tempfile::tempdir().unwrap();
        let store = crate::Store::open(&dir.path().join("budget.sqlite3")).unwrap();
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let result = rt.block_on(store.run(|db| {
            db.execute_batch("INSERT INTO sources (id,bundle_hash,source_id,title,system_id,edition,language,pdf_hash,raw_json,prepared_json,total_pages) VALUES (1,'b','s','t','mork-borg','e','it','00','{}','{}',1); INSERT INTO source_rulesets VALUES (1,'mork-borg','it-2025'); INSERT INTO source_pages (id,source_id,pdf_page,png,complete) VALUES (1,1,1,x'00',1);")?;
            // Seeds an unfinalized legacy-style reading; new selections otherwise need
            // finalization, so the guard is lifted for the registration only and restored.
            let guard: String = db.query_row("SELECT sql FROM sqlite_master WHERE type='trigger' AND name='source_primary_events_insert'", [], |r| r.get(0))?;
            db.execute_batch("DROP TRIGGER source_primary_events_insert")?;
            super::super::documents::register_import(db, 1)?;
            db.execute_batch(&guard)?;
            for id in 1..=500 {
                db.execute("INSERT INTO source_units (id,page_id,block_id,kind,polygon,raw_html,raw_text) VALUES (?1,1,?2,'Text','[]','<p>veleno comune</p>','')", params![id, format!("b{id}")])?;
            }
            ensure(db)?;
            db.execute_batch("UPDATE source_units SET state='accepted',revision=revision+1; UPDATE source_pages SET complete=1,revision=revision+1;")?;
            let within = lookup_within(db, "mork-borg", "it-2025", "veleno", true, None, QUERY_BUDGET)?;
            assert!(within.truncated);
            lookup_within(db, "mork-borg", "it-2025", "veleno", true, None, Duration::ZERO)
        }));
        assert!(
            matches!(result, Err(AppError::Invalid(message)) if message.starts_with("Query too broad")),
            "{:?}",
            result.err()
        );
        // No budget stays armed afterwards: a statement well past BUDGET_CHECK_OPS runs.
        let counted = rt
            .block_on(store.run(|db| {
                Ok(db.query_row("WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM c WHERE x<100000) SELECT count(*) FROM c", [], |row| row.get::<_, i64>(0))?)
            }))
            .unwrap();
        assert_eq!(counted, 100_000);
    }

    #[test]
    fn failed_rederivation_removes_the_stale_projection() {
        let dir = tempfile::tempdir().unwrap();
        let store = crate::Store::open(&dir.path().join("stale.sqlite3")).unwrap();
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let state = rt
            .block_on(store.run(|db| {
                db.execute_batch("INSERT INTO sources (id,bundle_hash,source_id,title,system_id,edition,language,pdf_hash,raw_json,prepared_json,total_pages) VALUES (1,'b','s','t','mork-borg','e','it','00','{}','{}',1); INSERT INTO source_pages (id,source_id,pdf_page,png,complete) VALUES (1,1,1,x'00',1); INSERT INTO source_units (id,page_id,block_id,kind,polygon,raw_html,raw_text) VALUES (1,1,'b','Text','[]','<p>vecchio</p>','');")?;
                ensure(db)?;
                db.execute_batch("UPDATE source_units SET state='accepted',revision=revision+1;")?;
                // A later normaliser version that cannot handle this unit.
                let failed = derive_with(db, SEARCH_NORMALISER + 1, |_| {
                    Err(AppError::Invalid("synthetic normaliser failure"))
                })?;
                let rows = |table: &str| -> rusqlite::Result<i64> {
                    db.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| row.get(0))
                };
                Ok((failed, rows("source_search_text")?, rows("source_search_fts")?))
            }))
            .unwrap();
        assert_eq!(state, (1, 0, 0));
    }

    fn flat(html: &str) -> String {
        search_text(html).unwrap().trim_end().to_owned()
    }

    #[test]
    fn text_in_a_table_outside_any_cell_gets_its_own_line() {
        assert_eq!(
            flat("<table><caption>Armi</caption><tr><td>a</td><td>b</td></tr></table>"),
            "Armi\na | b"
        );
    }

    #[test]
    fn block_tags_in_a_cell_separate_words_and_inline_tags_do_not() {
        assert_eq!(
            flat(
                "<table><tr><td><h4>Titolo</h4>testo</td><td>s<sub>1</sub>x<b>y</b></td></tr></table>"
            ),
            "Titolo testo | s1xy"
        );
    }

    #[test]
    fn several_tables_footers_empty_rows_and_nested_containers() {
        assert_eq!(
            flat(
                "<table><tr><td>a</td></tr></table><p>mezzo</p><table><tr><td>b</td><td>c</td></tr></table>"
            ),
            "a\n\nmezzo\n\nb | c"
        );
        assert_eq!(
            flat(
                "<table><tbody><tr><td>x</td></tr></tbody><tfoot><tr><td>Totale</td><td>3</td></tr></tfoot></table>"
            ),
            "x\nTotale | 3"
        );
        assert_eq!(
            flat(
                "<table><tr><td>a</td><td>b</td></tr><tr><td></td><td></td></tr><tr><td>c</td><td>d</td></tr></table>"
            ),
            "a | b\n | \nc | d"
        );
        assert_eq!(
            flat(
                "<ul><li>voce<table><tr><td>a</td><td>b</td></tr></table></li></ul><blockquote><table><tr><td>c</td><td>d</td></tr></table></blockquote>"
            ),
            "* voce\n  \n  a | b\n\n> c | d"
        );
    }

    #[test]
    fn the_sanitiser_leaves_no_attribute_but_numeric_spans() {
        let dirty = "<TABLE class=x onclick=\"a>b\" title=\"<td>\"><TR id=r><TD colspan=\"2\" rowspan=3 style=\"x\" data-x=1>a &lt;b&gt;</TD><th ROWSPAN=\"2\" title='>'>c</th></TR></TABLE><p class=z>t</p><a href=\"javascript:1\">l</a>";
        let clean = sanitized(dirty).unwrap();
        let tags = regex::Regex::new(r"<[^>]*>").unwrap();
        let span = regex::Regex::new(r#"^<(td|th)( (colspan|rowspan)="[0-9]+")*>$"#).unwrap();
        let plain = regex::Regex::new(r"^</?[a-z0-9]+>$").unwrap();
        for tag in tags.find_iter(&clean) {
            assert!(
                plain.is_match(tag.as_str()) || span.is_match(tag.as_str()),
                "{} in {clean}",
                tag.as_str()
            );
        }
        assert!(!clean.contains('<') || tags.replace_all(&clean, "").find('<').is_none());
    }

    #[test]
    fn html_without_a_table_is_not_rewritten() {
        let html = "<h2>Armi</h2><p>Una <b>spada</b> &amp; <i>lunga</i></p><ul><li>uno</li><li>due</li></ul><ol><li>tre</li></ol><blockquote>citazione</blockquote><p>a<br>b</p>";
        let clean = sanitized(html).unwrap();
        assert_eq!(flatten_tables(&clean), clean);
        assert_eq!(
            search_text(html).unwrap(),
            "## Armi\n\nUna spada & lunga\n* uno\n* due\n1. tre\n\n> citazione\n\na\nb\n"
        );
    }

    #[test]
    fn table_rows_become_lines_with_aligned_empty_cells() {
        let marker = "<table><thead><tr><th>Arma</th><th>Danno</th><th>Note</th></tr></thead><tbody><tr><td>Spada <b>lunga</b></td><td>d8 &amp; d6</td><td></td></tr><tr><td colspan=\"2\">Ascia</td><td>a una mano<br>o due<table><tr><td>x</td><td>y</td></tr></table></td></tr></tbody></table><p>Dopo</p>";
        assert_eq!(
            search_text(marker).unwrap(),
            "Arma | Danno | Note\nSpada lunga | d8 & d6 | \nAscia | a una mano o due x y\n\nDopo\n"
        );
        let owner = crate::sources::tables::html(
            &[
                vec!["Spada".to_owned(), "d8".to_owned(), String::new()],
                vec!["Ascia".to_owned(), "d6".to_owned(), "una mano".to_owned()],
            ],
            false,
        );
        let text = search_text(&owner).unwrap();
        assert_eq!(
            text.trim_end(),
            crate::sources::tables::plain(&[
                vec!["Spada".to_owned(), "d8".to_owned(), String::new()],
                vec!["Ascia".to_owned(), "d6".to_owned(), "una mano".to_owned()],
            ])
        );
    }

    #[test]
    fn oversized_search_text_is_an_explicit_error() {
        let html = format!("<p>{}</p>", "a ".repeat(140_000));
        assert!(
            matches!(search_text(&html), Err(AppError::Invalid(message)) if message.contains("256 KiB"))
        );
    }

    #[test]
    fn query_builder_quotes_everything_and_ranks_exact_first() {
        assert_eq!(fts_query("forza").unwrap(), "(\"forza\" OR \"forza\"*)");
        assert_eq!(fts_query("d8").unwrap(), "\"d8\"");
        assert_eq!(
            fts_query("NEAR(a b) \"due mani\" body:x").unwrap(),
            "(\"NEAR(a\" OR \"NEAR(a\"*) AND \"b)\" AND \"due mani\" AND (\"body:x\" OR \"body:x\"*)"
        );
        assert_eq!(
            fts_query("\"aperta forza").unwrap(),
            "(\"aperta\" OR \"aperta\"*) AND (\"forza\" OR \"forza\"*)"
        );
        for bad in ["", "---", "€ →", "a\0b", "a\u{7}b", "\u{301}"] {
            assert!(fts_query(bad).is_err(), "{bad:?}");
        }
        let seventeen = vec!["w"; 17].join(" ");
        assert!(fts_query(&seventeen).is_err());
        assert!(fts_query(&vec!["w"; 16].join(" ")).is_ok());
    }

    #[test]
    fn fts_drives_the_query_plan() {
        let dir = tempfile::tempdir().unwrap();
        let store = crate::Store::open(&dir.path().join("plan.sqlite3")).unwrap();
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let plan = rt
            .block_on(store.run(|db| {
                let mut query = db.prepare(&format!("EXPLAIN QUERY PLAN {}", match_sql(true)))?;
                let rows = query
                    .query_map(
                        params!["mork-borg", "it-2025", "\"x\"", Option::<String>::None],
                        |row| row.get::<_, String>(3),
                    )?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(rows)
            }))
            .unwrap();
        assert!(
            plan[0].starts_with("SCAN source_search_fts VIRTUAL TABLE"),
            "{plan:?}"
        );
        // The selected-reading join uses the (document, page) primary key or the source index.
        assert!(
            plan.iter()
                .any(|row| row.starts_with("SEARCH primary_page USING")
                    && (row.contains("source_primary_pages_source")
                        || row.contains("PRIMARY KEY"))),
            "{plan:?}"
        );
        for alias in ["u", "p", "s", "r", "t"] {
            assert!(
                plan.contains(&format!(
                    "SEARCH {alias} USING INTEGER PRIMARY KEY (rowid=?)"
                )) || plan.contains(&format!(
                    "SEARCH {alias} USING INTEGER PRIMARY KEY (rowid=?) LEFT-JOIN"
                )),
                "{plan:?}"
            );
        }
    }
}
