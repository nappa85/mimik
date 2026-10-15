# Hybrid retrieval plan (phase 1: lexical FTS5)

Status: PLAN ONLY, awaiting independent adversarial review. Nothing implemented.
Scope: harness-assessment findings C1, C3, C4, B6 (and the C6 interaction).
Vector index + reciprocal-rank fusion (RRF) is designed for, but deferred (§9).

## 1. Current behaviour (verified by reading code)

- `src/sources/store.rs` `lookup()` (line ~332): `CASE corrected THEN correction ELSE raw_text END LIKE '%q%'`,
  full scan, `ORDER BY s.id,p.pdf_page,u.id LIMIT 50`, no truncation signal (C1).
- `src/sources/import.rs` `normalized()` (line ~329): `raw_text = html2text::from_read(sanitized(html), 120)`.
  Paragraphs are hard-wrapped and tables are laid out in columns, so a cell longer than its
  column wraps and its words are **interleaved with neighbouring cells** (probe below shows
  `Spada lunga │d8 più` / `a due mani  │forza`). `LIKE` folds ASCII only (C3, B6).
- `Evidence` (`src/sources/mod.rs:91`) has page id/title/pdf page/label/text only (C4).
- `raw_text` is immutable after finalization (`units_finalized_update`, migration 007) **and**
  re-derived and compared byte-for-byte on re-import (`verify_existing`). Changing how
  `raw_text` is produced would break idempotent re-import of every existing source, so
  `raw_text` stays exactly as is. Search text is a separate derived artifact.
- Callers: only `src/sources/web.rs` `search()` (template `templates/source_search.html`) and
  tests (`tests/source_review.rs`, `tests/registry.rs`). `mork_advice.rs`, `campaigns/*` and
  `definitions/*` do not call lookup (grep for `search_evidence`, `search_ruleset_evidence`,
  `EvidenceSearch`). No campaign-context caller to update.

## 2. Verification of bundled SQLite capabilities (done, in container)

Temporary probe `tests/zz_fts5_probe.rs` (deleted after running) in the CI image:

```
docker run --rm --cpus 4 \
  --mount type=bind,src=<copy>,dst=/workspace \
  --mount type=volume,src=mimik-cargo-cache,dst=/opt/cargo/registry \
  --mount type=volume,src=mimik-retrieval-target,dst=/target \
  -e CARGO_TARGET_DIR=/target -w /workspace mimik-ci:latest \
  cargo test --locked --test zz_fts5_probe -- --nocapture
```

Results (rusqlite 0.37 / libsqlite3-sys 0.35 `bundled`):
- `sqlite_version=3.50.2`; `PRAGMA compile_options` contains `ENABLE_FTS5` (also FTS3, RTREE,
  `ENABLE_LOAD_EXTENSION`). No Cargo feature change needed.
- `unicode61 remove_diacritics 2`: `"mork"`, `"MÖRK"`, `"Mörk"` all match `MÖRK BORG`;
  `"CITTA"` matches `città`; `"citta" "e"` matches `città è`; phrase `"capacity reference"`
  matches across `\n`; `"dell'armatura"` and `"armatura"` match `dell'armatura`; `"+8"` matches.
- Quoting neutralises syntax: `"OR" "NEAR" "body:x"` → 0 rows, no error; `"a""b"` → no error.
- `"---"` (no tokens) → matches nothing alone and is ignored inside an AND; empty MATCH
  string → `fts5: syntax error`. So the builder must reject token-less queries.
- `"capac"*` prefix works; `bm25()` ordering works; `trigram remove_diacritics 1` and
  `contentless_delete=1` are available (3.45/3.43 features).
- html2text 0.17.1: `config::plain().raw_mode(true).no_table_borders().allow_width_overflow()
  .string_from_read(html, usize::MAX/4)` yields unwrapped paragraphs and **one table cell per
  line** (`Arma\nDanno\nSpada lunga a due mani\nd8 più forza`), no box drawing. Plain wide
  mode without `raw_mode` still emits box-drawing table layout.

## 3. Schema: `migrations/016_source_search.sql` (user_version 16)

```sql
-- Derived, reproducible search text for each unit. Raw evidence rows are untouched.
CREATE TABLE source_unit_search (
  unit_id INTEGER PRIMARY KEY REFERENCES source_units(id) ON DELETE CASCADE,
  normaliser INTEGER NOT NULL CHECK (normaliser > 0),
  search_text TEXT NOT NULL CHECK (instr(search_text, char(0)) = 0
                                   AND length(CAST(search_text AS BLOB)) <= 262144)
) STRICT;
-- Lexical index over *eligible effective text only* (accepted -> search_text, corrected ->
-- correction). rowid = source_units.id. Regular (content-storing) table so integrity-check
-- and reconciliation can compare stored text; duplication is small (manual-sized corpus).
CREATE VIRTUAL TABLE source_unit_fts USING fts5(
  body, tokenize = 'unicode61 remove_diacritics 2', prefix = '3'
);
```

Triggers (pure SQL, no app function needed):
- `source_unit_search_ai` AFTER INSERT ON `source_unit_search`: delete fts rowid, then insert
  `(NEW.unit_id, CASE u.state WHEN 'corrected' THEN u.correction ELSE NEW.search_text END)`
  from `source_units u WHERE u.id=NEW.unit_id AND u.state IN ('accepted','corrected')`.
- `source_unit_search_au` AFTER UPDATE ON `source_unit_search`: same delete+reinsert.
- `source_units_review_fts` AFTER UPDATE OF state, correction ON `source_units`: delete fts
  `rowid=OLD.id`; insert `NEW.correction` when `NEW.state='corrected'`; insert the
  `source_unit_search.search_text` when `NEW.state='accepted'` (only if the derived row
  exists). `rejected`/`unreviewed` → removed.
- `source_units_delete_fts` AFTER DELETE ON `source_units`: delete fts row (side row cascades;
  units of finalized sources can't be deleted anyway, migration 007).
- Index on `source_units(page_id)` already implied by `UNIQUE(page_id,block_id)`; no new index.

Not used: an `ALTER TABLE source_units ADD COLUMN search_text` — it would sit outside the
007 immutability column list (mutable after finalization) and mix derived data into the raw
evidence row. Not used: per-row `mimik_*` SQL function in triggers — external/raw
connections (tests, sqlite3 CLI) would fail on any unit insert.

Why triggers for review-state maintenance (not application code): every state change,
including web review, CLI review and the `unit_review_page_cascade` path, goes through
`UPDATE source_units`; a trigger inside the same transaction cannot be bypassed or forgotten
and rolls back with the review. Why application code for `search_text`: deriving it needs the
Rust sanitizer + html2text; SQL cannot compute it.

Page completeness is **not** mirrored into the index: it is checked by join at query time
(page completion flips on every unit review via the cascade, and joining is cheap by rowid).

## 4. Application-level maintenance and upgrade backfill

- One normaliser function `search_text(raw_html) -> Result<String>` in `src/sources/import.rs`:
  `sanitized()` then html2text raw mode (above), `const SEARCH_NORMALISER: i64 = 1`.
- Import (`import_source` new-source path): insert `source_unit_search` for each unit in the
  same transaction right after the unit insert (units are unreviewed, so no fts row yet).
- `Store::migrate`: accept `version <= 16`; for `version < 16` run 016 SQL. Then, in the same
  IMMEDIATE transaction, on **every** open run `ensure_search_index(&tx)`:
  1. derive and INSERT rows for units with no `source_unit_search` row (backfill from schema
     15 and any raw-SQL test fixtures); UPDATE rows whose `normaliser <> SEARCH_NORMALISER`
     (triggers reindex). Already-reviewed legacy units become searchable via the insert trigger.
  2. reconcile: if the set `{(rowid, body)}` in fts differs from the expected eligible set
     (`state IN (accepted, corrected)`, effective text), `DELETE FROM source_unit_fts` and
     re-insert from the expected set; then `INSERT INTO source_unit_fts(source_unit_fts)
     VALUES('integrity-check')`. Cost O(units) once per start; bounded by import limits.
  A unit whose raw HTML fails sanitisation aborts the migration with an explicit error
  (fail closed; such HTML was already rejected at import, so this only fires on tampering).
- No raw/immutable row is rewritten; 007 triggers are unaffected (we only write new tables).

## 5. Query construction (no operator injection)

`fn fts_query(input: &str) -> Result<String, AppError>`:
- Existing bounds stay (trimmed 1–200 chars, no NUL/control chars). Split into balanced
  `"..."` phrase groups and bare whitespace-separated terms; an unbalanced `"` is treated
  as a separator. Max 16 terms/phrases.
- Drop pieces with no `char::is_alphanumeric`; if none remain → `Invalid("no searchable words")`.
- Emit only FTS5 *strings*: each piece wrapped in `"`, internal `"` doubled. Bare terms with
  ≥3 alphanumeric chars get a trailing `*` (prefix, served by `prefix='3'`); phrases are exact.
  Pieces are joined by a space (implicit AND). No `AND/OR/NOT/NEAR`, `:`, `^`, `(`, `{`, `-`
  is ever emitted outside quotes, so user text cannot select columns or operators.
- The string is bound as a parameter (`MATCH ?`), never formatted into SQL.
- Semantic change recorded in `docs/source-import-contract.md` / template copy: from
  "literal substring" to "all words (prefix) / quoted phrases, diacritic- and case-insensitive".
  Mid-word substrings (`BORG` inside `MÖRKBORG`) no longer match; a trigram side index is
  the deferred remedy if evaluation shows need (§9).

## 6. Filtering, ranking, truncation

```sql
SELECT u.id,u.block_id,u.kind,u.polygon,u.revision,u.state,p.id,p.revision,s.id,s.title,
       p.pdf_page,p.printed_label,
       CASE WHEN u.state='corrected' THEN u.correction ELSE u.raw_text END
FROM source_unit_fts f
JOIN source_units u ON u.id=f.rowid
JOIN source_pages p ON p.id=u.page_id
JOIN sources s ON s.id=p.source_id
LEFT JOIN source_rulesets r ON r.source_id=s.id
WHERE source_unit_fts MATCH ?3 AND {predicate}           -- unchanged exact edition predicate
  AND p.complete=1 AND u.state IN ('accepted','corrected') -- defence in depth
  AND (?4 IS NULL OR s.id IN (SELECT value FROM json_each(?4)))  -- optional source set
ORDER BY bm25(source_unit_fts), s.id, p.pdf_page, u.id
LIMIT 51
```
- All eligibility filters are in the same WHERE as MATCH, so ORDER/LIMIT apply only to the
  filtered set: other-edition/unreviewed/incomplete text can neither be returned nor take a
  top-k slot. Unreviewed/rejected text is not in the index at all.
- Known, accepted limitation for reviewer judgement: bm25 IDF/length statistics are
  table-global, so other editions' *reviewed* text can influence ordering (never membership).
  Scores are not exposed. Per-edition tables would remove this at significant complexity.
  Amended (§12.1): global statistics can also change top-50 *membership* within an edition
  when other editions change; this is a reproducibility issue, so citations pin unit id + revision.
- `truncated = rows.len() > 50`; return 50. Ties are deterministic.
- Source-set: `search_*` keep their signatures; an internal `LookupScope { source_ids:
  Option<Vec<i64>> (1..=32) }` is used by `lookup()` for the future tool loop. Only exercised
  by a store-level test now (not exposed in the web UI).
- Counts (`source_count`, `incomplete_pages`, `partial_runs`) unchanged, same transaction.

## 7. Result type and caller changes

`Evidence` gains: `unit_id`, `block_id`, `kind`, `polygon` (stored JSON string, as `UnitView`),
`unit_revision`, `page_revision`, `source_id`, `state` (accepted/corrected). `EvidenceSearch`
gains `truncated: bool`. Template: show "corrected" badge, link the region crop
`/sources/pages/{page}/units/{unit}/image` and the page anchor, and a "more than 50 matches;
refine the query" badge when truncated; update the lookup description text. Hierarchy
(stable node id/ancestor path, accuracy-experiments §"Book-wide hierarchy") does not exist in
the schema yet; the unit id is the join key a future node table will use.

## 8. Failing-first regressions (each run red before the fix, then kept in CI)

Store-level tests in `tests/source_review.rs` (synthetic fixtures only) plus unit tests for
`fts_query`:
1. Wrapped phrase: paragraph >120 cols, query spanning the wrap → match (fails today: `\n`).
2. Table cell phrase `spada lunga a due mani` in a narrow column → match (fails today: interleave).
3. Accented case folding: `città` text, query `CITTÀ` and `MÖRK`/`mörk` → match.
4. Italian diacritics removal: query `citta`/`perche` matches `città`/`perché`.
5. Edition isolation: identical reviewed text in two editions; each search returns only its
   own edition's unit ids; registered vs legacy-label paths both checked.
6. Unreviewed exclusion: an unreviewed unit, and an accepted unit on an incomplete page,
   → no match; the fts table has no rowid for unreviewed units.
7. Correction indexed: corrected unit found by correction words, not by raw words.
8. Rejected removed: accepted → rejected removes the fts row and the match.
9. Truncation flag: 51 matching units → 50 returned, `truncated=true`; 50 → false.
10. FTS syntax characters: `OR`, `NEAR(a b)`, `body:x`, `"`, `*`, `^`, `(`, `-x`, `{body}`
    return evidence or empty results, never an SQLite error; token-less query → `Invalid`.
11. Backfill from schema 15: build a v15 DB via the existing migration fixture path with an
    already accepted unit on a complete page, open with new code → searchable; second open is
    a no-op; reconciliation repairs a manually deleted fts row.
12. Ranking: unit with more query-term occurrences ranks before a page-earlier unit (fails today).
13. Evidence ids: returned `unit_id/block_id/polygon` equal the reviewed unit's.
14. Immutability unchanged: existing `tests/source_finalization.rs` still passes; a review on a
    finalized source updates fts without tripping 007 triggers.
Existing lookup tests (`registry.rs`, `source_review.rs`) keep passing (their queries are words).

## 9. Performance bound

Corpus bound: ≤64 pages × ≤20,000 units per import; legacy Pathfinder scale ≈13k chunks.
MATCH cost is posting-list driven; ≤16 terms, prefix only for ≥3 chars (prefix index), LIMIT
51, eligibility joins by primary key. Target p95 < 50 ms for 100k synthetic units on the CI
container, measured with an `#[ignore]` bench test and recorded (not a timed CI assertion).
If exceeded, add `rusqlite` `hooks` feature and a progress-handler step budget returning
`Invalid("query too broad")`. The global connection mutex (C6) still serialises lookups; WAL
and connection pooling are a separate change. Startup reconciliation is O(units).

## 10. Vector + RRF follow-up (deferred, plan only)

- Shape: `source_unit_embeddings(unit_id, model_id, dim, input_hash, vector BLOB)`, keyed by
  unit id and effective-text hash so corrections/rejections invalidate (same triggers delete
  rows; a Rust job re-embeds). Candidate generation: lexical top-N (e.g. 100) and vector
  top-N **both computed over the same filtered eligible set**, fused with RRF
  (`score = Σ 1/(60 + rank)`), deterministic tie-break by unit id. `lookup()` already returns
  ranked unit ids, so fusion is an additive layer.
- Storage/search options: (a) brute-force cosine in Rust over BLOBs after SQL eligibility
  filtering — at ≤100k × 768 f32 this is tens of ms and needs no new dependency; (b)
  `sqlite-vec` (Alex Garcia; MIT/Apache-2.0; Rust crate bundles the C extension, registered
  via `sqlite3_auto_extension`) — pre-1.0 and release cadence slowed, metadata filtering
  less flexible than SQL joins; maintenance status to be re-checked at implementation. Start
  with (a); adopt (b) only on demonstrated need.
- Blocked on: an embedding runtime/model choice (multilingual, Italian; container-only per
  AGENTS.md), and the retrieval recall@k evaluation set. No model is added in this phase.

## 11. Docs/records to update on implementation

`docs/source-import-contract.md` (lookup semantics), `docs/source-quality.md`,
`docs/dependency-decisions.md` (FTS5 built-in; html2text raw mode; sqlite-vec deferred),
`TODO.md` C1/C3/B6/C4 item, `docs/reviews/` record of this plan's review and the implementation review.

## 12. Plan review amendments (independent review: GO with amendments)

Binding amendments and how the implementation applies them. These supersede the
sections above where they differ.

1. **Global bm25 statistics (§6).** IDF and length statistics cover the whole FTS table, so
   importing or reviewing another edition can change *which* units make the top 50 of this
   edition, not only their order. This is a reproducibility issue, not a leak: membership is
   always restricted to the filtered eligible set and scores are not exposed. Tool-loop
   citations must pin `unit_id` + `unit_revision` (both returned in `Evidence`), never rank
   position. Tests do not assert absolute order across multi-edition fixtures (they compare
   sorted id sets).
2. **No silent index drop.** Triggers `source_units_insert_requires_search` (a unit can't be
   inserted already accepted/corrected) and `source_units_review_requires_search` (a review to
   accepted/corrected aborts if the unit has no `source_unit_search` row) make a missing
   search row a hard error. `source_unit_search_delete_guard` keeps search rows of reviewed
   units. Upgrades backfill before any review can run.
3. **Fixtures.** `tests/support/schema_fixture.rs` now expects schema 16, always drops the
   016 triggers, drops the derived tables on a true rewind to 6 and, when remigrating,
   keeps them and re-applies 016 (its DDL is idempotent: `IF NOT EXISTS` /
   `DROP TRIGGER IF EXISTS`, so test rewinds to 4–14 re-apply it safely). The schema snapshot
   orders FTS5 `WITHOUT ROWID` shadow tables by key and skips the virtual table itself.
   Version assertions were moved from 15 to 16; unknown-version rejection now uses 17.
4. **Open-time cost.** Every open runs only the anti-join (units without a search row, or with
   another `SEARCH_NORMALISER`) and a count comparison (FTS rows vs eligible units). Full
   rebuild + FTS5 `integrity-check` runs only on upgrade from <16, on a count mismatch, or
   via `Store::rebuild_search_index()` (explicit command; no CLI subcommand yet).
   `search_text()` rejects output over 256 KiB (or containing NUL) with an explicit error
   before any insert; failures abort the open and leave the schema at its old version.
5. **Displayed text.** `Evidence.text` is the effective indexed text: the correction, or the
   unwrapped `search_text`; `raw_text` stays stored and immutable. The page links each hit to
   its region crop (`/sources/pages/{page}/units/{unit}/image`). Crops are linked, not inlined:
   the region renderer admits one job at a time (`region::admit`, semaphore of 1), so 50 inline
   images would mostly receive 503.
6. **Join order.** `match_sql()` uses `CROSS JOIN` from `source_unit_fts`; a unit test asserts
   `EXPLAIN QUERY PLAN` starts with `SCAN source_unit_fts VIRTUAL TABLE` and reaches
   units/pages/sources/rulesets/search rows by primary key.
7. **Normaliser.** `html2text::config::plain_no_decorate().raw_mode(true).no_table_borders()
   .allow_width_overflow()` at width 2^20. It still emits structural markers (`## `, `* `,
   `1. `, `> `); unicode61 drops the punctuation, but ordered-list numbers become number
   tokens (documented, golden-tested). Table cells are one per line, so phrases can span
   adjacent cells (test `table_cells_are_flattened_so_phrases_can_span_adjacent_cells`). The
   golden test `normaliser_output_is_golden` fails on any output change until
   `SEARCH_NORMALISER` is bumped, which then re-derives every row on next open.
8. **Query builder.** Control characters (including NUL) are rejected; more than 16
   words/phrases is rejected (`Invalid`), not truncated; pieces without a Rust alphanumeric
   character are dropped and a query with none is rejected. Pieces that Rust counts as
   alphanumeric but unicode61 does not tokenise (e.g. lone combining marks) become empty
   FTS5 strings, which SQLite ignores inside an AND or matches nothing (probe-verified; a lone
   U+0301 is rejected by the builder). Former LIKE wildcards (`%`, `_`, `\`) are now `Invalid`
   instead of an empty result; the two existing tests were updated to assert that.
9. **Prefix and ranking.** `prefix='3'` serves 3-character prefixes; longer prefixes use FTS5's
   term range scan over the main index (correct, slower on huge vocabularies). Each bare word
   of 3+ letters/digits is emitted as `("w" OR "w"*)` so exact occurrences match both arms and
   rank above prefix-only matches (test `exact_words_rank_before_prefix_only_matches`). Pieces
   are joined with a generated `AND` (FTS5 rejects implicit AND between parenthesised groups).
10. **Additional tests.** correction → accepted switches the indexed text back, corrected →
    rejected removes it (`rejection_and_correction_maintain_the_index`); the owner materials
    review path updates the index (`material_review_path_maintains_the_search_index`);
    unsanitisable legacy HTML aborts the upgrade (`unsanitizable_legacy_unit_aborts_upgrade`);
    the legacy raw fixture in `tests/source_review.rs` stays green; page completion changes
    do not write FTS rows (`completeness_changes_do_not_rewrite_the_index`).
11. **Byte guards.** New `Evidence` text fields use SQL-side `CASE typeof/length` guards
    (block id 512, kind 100, polygon 1024, title/label 200, correction 80,000, search text
    256 KiB bytes); a NULL guard result is a conversion error, never truncated text.

### Measured performance (amends §9)

`measure_lookup_latency_on_100k_units` (ignored test, release build, CI container,
`--cpus 4`; synthetic 24-word vocabulary, 40 words per unit, so nearly every unit matches
every common word — a deliberately adverse corpus):
- 100,000 units: p95 194 ms, max 202 ms (4-word AND query); 0.35 ms for a missing word;
  37 ms for a phrase; 110 ms for `di`.
- 15,000 units (about the legacy 12,956-chunk scale): p95 29 ms, max 31 ms.
The 50 ms p95 target holds at the expected corpus size but **not** at 100k adverse units.
A progress-handler step budget (rusqlite `hooks` feature) was not added; it remains an open
follow-up, together with C6 (one connection mutex serialises lookups).

Command: `MIMIK_BENCH_UNITS=15000 cargo test --locked --release --test source_search measure -- --ignored --nocapture` (omit the variable for 100k), in the container described in §2.

## 13. Implementation review fixes (second independent review: APPROVE with fixes)

- **Renumbered to migration 017** after the admin branch took 016 (`016_registry_administration.sql`).
  The file is in `store::MIGRATIONS`; `sources::SEARCH_SCHEMA` (17) names it and is
  compile-time checked to be <= `SCHEMA_VERSION`. `ensure` no longer takes an `upgraded`
  flag: an upgrade creates empty tables, so the count check forces the rebuild; equal counts
  with stale content are left to `Store::rebuild_search_index`.
- **Object names** are now all `source_search_*` (`source_search_text`,
  `source_search_failures`, `source_search_fts`, `source_search_unit_*`/`source_search_text_*`
  triggers). Test fixtures find and drop them from `sqlite_master`
  (`tests/support/schema_fixture.rs::drop_search_schema`, `sources::search::drop_schema` for
  unit tests) instead of hand-kept lists. The migration uses plain `CREATE` (no
  `IF NOT EXISTS`), so drift fails loudly. A remigrating fixture keeps the search objects of
  the live session and sets the version back to current.
- **Bounded, non-fatal backfill.** `derive_missing` streams in batches of 64 units by id
  cursor, reading raw HTML through a 64 KiB SQL guard (memory <= batch x 64 KiB). Units that
  cannot be normalised go into `source_search_failures` (reason, normaliser version) and stay
  unindexed. The open does not fail. Repair: review the unit as `corrected` (indexed from the
  correction) or `rejected`, or import a corrected bundle. Accepting it is blocked with that
  instruction (`source_search_unit_accept_guard`). `EvidenceSearch.unindexed_units` puts a
  per-edition gap badge on the lookup page; the server prints a startup warning with the count.
  Corrected units no longer need a search row (the correction is indexed directly).
- **Time budget.** `rusqlite` `hooks` feature (no new crate). A progress handler checks a 1 s
  deadline every 10,000 VM instructions across all statements of one lookup (counts and the
  MATCH query) and interrupts; the result is `Invalid("Query too broad …")`. A drop guard
  clears the handler on every path. It bounds VM-level work only: a single long FTS5
  `xFilter` call (posting-list traversal inside the extension) is interrupted only at its
  next opcode.
- **Raw HTML immutability** for all sources (`source_search_unit_raw_immutable`), so the
  open-time count check can't miss content drift. Owner-material byte-guard tests now
  replace the unreviewed fixture unit instead of updating its HTML.
- **Delete guard** (`source_search_text_delete_guard`, accepted units only, unless a newer
  normaliser version recorded a failure; the stale projection and its index row are then
  removed, test `failed_rederivation_removes_the_stale_projection`) is tested. The
  test also deletes a non-finalized source with an accepted unit (units → pages → source) and
  checks that the derived rows go with it. Reviewed units that have review events can't be
  deleted at all (append-only events reference them).
- **Busy timeout.** A large upgrade rebuild runs inside the IMMEDIATE migration transaction
  and can take longer than the 5 s `busy_timeout`. A concurrent opener then fails with
  SQLITE_BUSY (fails closed, no partial state) and can simply be retried.

Post-fix measurements (same command, with the progress handler): first run 15k p95 41 ms, 100k p95 275 ms; re-run with BUDGET_CHECK_OPS=1,000: 15k p95 33 ms, 100k p95 216 ms; with 10,000 (kept): 15k p95 32 ms, 100k p95 207 ms. The 100k adverse case **misses the 50 ms target**; the 1 s budget bounds VM-level work only. See docs/reviews/2026-10-15-hybrid-retrieval.md.
