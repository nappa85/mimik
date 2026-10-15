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
pub(crate) const SEARCH_NORMALISER: i64 = 1;
/// Matches the `source_search_text.search_text` CHECK constraint.
const MAX_SEARCH_TEXT_BYTES: usize = 256 * 1024;
/// Wide enough that no unit (raw HTML is at most 64 KiB) is ever wrapped.
const UNWRAPPED_WIDTH: usize = 1 << 20;
pub(crate) const MATCH_LIMIT: usize = 50;
const MAX_QUERY_PIECES: usize = 16;

/// Unwrapped, undecorated plain text for indexing. Tables are flattened to one cell per
/// line, so a quoted phrase can match across adjacent cells (documented behaviour).
///
/// # Errors
/// Rejects unsanitizable HTML or output above the stored byte bound.
pub(crate) fn search_text(raw_html: &str) -> Result<String, AppError> {
    let clean = sanitized(raw_html)?;
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
fn guarded(column: &str, limit: usize) -> String {
    format!(
        "CASE WHEN typeof({column})='text' AND length(CAST({column} AS BLOB))<={limit} THEN {column} END"
    )
}

fn read_guarded(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<String> {
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

/// Clears the progress handler however the guarded statement ends.
struct Budget<'a>(&'a Connection);

impl<'a> Budget<'a> {
    fn start(db: &'a Connection, budget: Duration) -> Self {
        let deadline = Instant::now() + budget;
        db.progress_handler(BUDGET_CHECK_OPS, Some(move || Instant::now() >= deadline));
        Self(db)
    }
}

impl Drop for Budget<'_> {
    fn drop(&mut self) {
        self.0.progress_handler(0, None::<fn() -> bool>);
    }
}

fn interrupted(error: rusqlite::Error) -> AppError {
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
    let incomplete_pages = count(format!(
        "SELECT count(*) FROM {from} JOIN source_pages p ON p.source_id=s.id WHERE {predicate} AND p.complete=0"
    ))?;
    let partial_runs = count(format!(
        "SELECT count(*) FROM {from} WHERE {predicate} AND (s.total_pages=0 OR s.total_pages<>(SELECT count(*) FROM source_pages p WHERE p.source_id=s.id))"
    ))?;
    let unindexed_units = count(format!(
        "SELECT count(*) FROM {from} JOIN source_pages p ON p.source_id=s.id JOIN source_units u ON u.page_id=p.id WHERE {predicate} AND p.complete=1 AND u.state='accepted' AND NOT EXISTS(SELECT 1 FROM source_search_text t WHERE t.unit_id=u.id)"
    ))?;
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
/// not detected here; use [`crate::Store::rebuild_search_index`]. Returns the failed-unit count.
///
/// # Errors
/// Storage failures only; unnormalisable units are recorded, not fatal.
pub(crate) fn ensure(db: &Connection) -> Result<i64, AppError> {
    let failed = derive_missing(db)?;
    let (indexed, eligible): (i64, i64) = db.query_row(&format!("SELECT (SELECT count(*) FROM source_search_fts),(SELECT count(*) FROM ({EXPECTED_INDEX}))"), [], |row| Ok((row.get(0)?, row.get(1)?)))?;
    if indexed != eligible {
        rebuild(db)?;
    }
    Ok(failed)
}

/// Test support: drop every `source_search_*` object found in `sqlite_master`.
#[cfg(test)]
pub(crate) fn drop_schema(db: &Connection) -> Result<(), AppError> {
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

    #[test]
    fn normaliser_output_is_golden() {
        // If this fails after a dependency upgrade, bump SEARCH_NORMALISER and update.
        assert_eq!(SEARCH_NORMALISER, 1);
        let html = "<h2>Armi</h2><p>Una <b>spada</b> <i>lunga</i> che continua per molte parole senza mai andare a capo anche oltre centoventi colonne di testo continuo qui</p><table><tr><th>Arma</th><th>Danno</th></tr><tr><td>Spada lunga a due mani</td><td>d8 più forza</td></tr></table><ul><li>uno</li><li>due</li></ul><ol><li>tre</li></ol><blockquote>citazione</blockquote><p>a<br>b</p>";
        assert_eq!(
            search_text(html).unwrap(),
            // Structural markers (`##`, `*`, `1.`, `>`) remain; unicode61 drops the
            // punctuation, but ordered-list numbers are indexed as number tokens.
            "## Armi\n\nUna spada lunga che continua per molte parole senza mai andare a capo anche oltre centoventi colonne di testo continuo qui\n\nArma\nDanno\nSpada lunga a due mani\nd8 più forza\n* uno\n* due\n1. tre\n\n> citazione\n\na\nb\n"
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
            db.execute_batch("INSERT INTO sources (id,bundle_hash,source_id,title,system_id,edition,language,pdf_hash,pdf,raw_json,prepared_json,total_pages) VALUES (1,'b','s','t','mork-borg','e','it','00',x'00','{}','{}',1); INSERT INTO source_rulesets VALUES (1,'mork-borg','it-2025'); INSERT INTO source_pages (id,source_id,pdf_page,png,complete) VALUES (1,1,1,x'00',1);")?;
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
        // The handler is removed afterwards: a statement well past BUDGET_CHECK_OPS runs.
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
                db.execute_batch("INSERT INTO sources (id,bundle_hash,source_id,title,system_id,edition,language,pdf_hash,pdf,raw_json,prepared_json,total_pages) VALUES (1,'b','s','t','mork-borg','e','it','00',x'00','{}','{}',1); INSERT INTO source_pages (id,source_id,pdf_page,png,complete) VALUES (1,1,1,x'00',1); INSERT INTO source_units (id,page_id,block_id,kind,polygon,raw_html,raw_text) VALUES (1,1,'b','Text','[]','<p>vecchio</p>','');")?;
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
