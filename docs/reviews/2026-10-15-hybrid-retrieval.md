# Hybrid retrieval (phase 1: lexical FTS5) — review record

## Scope

Harness-assessment findings C1, C3, C4 and B6: FTS5 lexical lookup replacing the
`LIKE '%q%'` scan; unwrapped, diacritic-insensitive search text; edition/source-set/
reviewed-only filtering in the ranking statement; bm25 ranking; truncation flag; unit,
block, polygon and revision identity in `Evidence`. Vector search and RRF are planned and
deferred. Plan: `docs/hybrid-retrieval-plan.md` (§12 plan-review amendments, §13 fixes
from this implementation review). Code: `migrations/017_source_search.sql`,
`src/sources/search.rs`, `src/sources/{import,mod,store}.rs`, `src/store.rs`,
`src/main.rs`, `templates/source_search.html`, fixtures and tests listed below. Rebased
onto master `1c848f1` (admin migration 016). Uncommitted at the time of writing.

## Reviews

| Stage | Reviewer | Verdict |
|---|---|---|
| Plan | Independent subagent (fresh context) | GO with amendments (11 binding, none blocking) |
| Implementation | Independent Sonnet 5.5 subagent (fresh context), read-only, with mutation testing | APPROVE with fixes |

The implementer (Opus 5.5) fixed every finding. The reviewers made no changes.

## Implementation findings and disposition

| # | Finding | Severity | Disposition |
|---|---|---|---|
| 1 | Backfill loaded every pending `(id, raw_html)` into memory (about 6 GB at 100k units). One unnormalisable legacy unit aborted every `Store::open` with no repair path, and the guard message was circular. | High | Fixed. 64-unit id-cursor batches with a 64 KiB SQL read guard. Failures go to `source_search_failures`, the unit stays unindexed, and the open continues. Accepting it is blocked with an instruction to record a correction or rejection. The lookup page shows an `unindexed_units` badge and the server prints a startup warning. Repair is documented in `docs/source-import-contract.md`. |
| 2 | No query time budget. A lookup holds the single connection mutex, and the 5 s request timeout cannot interrupt SQLite. | Medium | Fixed. A progress handler (rusqlite `hooks` feature) with a 1 s deadline returns `Invalid("Query too broad …")`. A drop guard clears the handler on every path. |
| 3 | `source_search_text_delete_guard` was untested (the `WHEN 0` mutant survived). | Medium | Fixed. A test asserts that deleting an accepted unit's search text aborts, and that deleting a non-finalized source with an accepted unit cascades the derived rows cleanly. |
| 4 | `IF NOT EXISTS` / `DROP IF EXISTS` in the migration masked drift. Fixture drop lists were maintained by hand. | Medium | Fixed. Plain `CREATE`. All objects are named `source_search_*` and fixtures drop them from `sqlite_master`. |
| 5 | `raw_html` drift was invisible to the open-time count check. | Medium | Fixed. A `source_search_unit_raw_immutable` trigger covers all sources. Material byte-guard tests now replace the fixture unit instead of updating it. |
| 6 | A large upgrade rebuild can exceed the 5 s `busy_timeout`. | Low | Documented (plan §13). A concurrent opener fails with BUSY (fails closed) and can retry. |
| 7 | `cli_owner_reset_can_complete_in_browser…` returned 408 under full-suite load. | Low (pre-existing) | The 5 s request timeout fires under load with real argon2. Not caused by this change. Not fixed here; tracked by the coordinator. |
| 8 | Confirm the materials-path index test runs in CI. | Info | Confirmed. `campaigns::tests::material_tests::material_review_path_maintains_the_search_index` runs under `cargo test --all-targets` (it passed in the final run). |
| — | Merge-order change: admin takes 016. | — | Renumbered to 017. Added to `MIGRATIONS`; `SCHEMA_VERSION=17`; `SEARCH_SCHEMA=17` is compile-time checked to be `<= SCHEMA_VERSION`. |

### Mutation results

- Reviewer: 11 of 12 mutants killed; the survivor was `WHEN 0` on the delete guard (finding 3). No eligibility leakage found.
- Implementer re-check after the fixes: each mutant below was reverted after its run.
  - Failures fatal again: `unnormalisable_…` and `unsanitizable_…` failed.
  - Budget handler removed: `lookup_is_interrupted_past_its_time_budget` failed.
  - Delete guard `WHEN 0`: `accepted_search_text_is_retained_…` failed.
  - Raw-immutability `WHEN 0`: `raw_html_is_immutable_for_unfinalized_sources_too` failed.

### Tests written after the code

These tests were not seen failing before their fix. Failure was shown only by later mutation, as noted:
- `rejection_and_correction_maintain_the_index`: it passed on the old LIKE code too.
- `material_review_path_maintains_the_search_index`.
- `exact_words_rank_before_prefix_only_matches`.
- `oversized_search_text_is_an_explicit_error`.
- `normaliser_output_is_golden`.
- `fts_drives_the_query_plan`.
- `backfill_streams_more_units_than_one_batch`: it checks correctness across batches, not memory.
- Shown failing only by the mutations above: the five tests for review findings 1, 2, 3 and 5.

Seen failing first against the old code:
- Runtime failures: wrapped phrase, narrow table cell, non-ASCII case folding, Italian diacritics, relevance order, FTS syntax literal, backfill.
- Compile failures: evidence identity, truncation, edition/source-set isolation, unreviewed exclusion.

## Verification (CI container `mimik-ci:latest`, combined tree on master `1c848f1`)

- `cargo fmt --check`: pass.
- `cargo clippy --locked --all-targets --all-features -- -D warnings`: pass, after two doc-comment backtick fixes. The full test run below happened before those comment-only edits; the search unit tests were rerun after them.
- `cargo test --locked --all-targets --all-features`: exit 0.
- `ruff check tools` / `ruff format --check tools`: pass.
- `python -m unittest discover -s tools -p "test_*.py"`: OK.
- Browser: release binary with `mimik-browser`, `--network=none --auth`, worktree `tools` mounted. Both Chromium stages passed, including source review and lookup. The new badges and region links are not specifically asserted.
- Not run here: `cargo audit` and `cargo about`. CI runs them.

## Performance

Ignored measurement test `measure_lookup_latency_on_100k_units` (release build, `--cpus 4`). The corpus is deliberately adverse: a 24-word vocabulary, so nearly every unit matches.

| Units | Worst query p95 | Max |
|---|---:|---:|
| 15,000 (about the legacy 12,956-chunk scale) | 41 ms | 42 ms |
| 100,000 | 275 ms | 280 ms |

These numbers are after the progress-handler change. Before it they were 29 ms and 194 ms; the extra handler calls and other container load on the host are both possible causes. The 50 ms target holds at the expected corpus size, not at 100k adverse units. The 1 s budget bounds the worst case. C6 (one connection mutex) remains open.

## Remaining open items

- Vector index and RRF (blocked on choosing an embedding runtime).
- No CLI command for `Store::rebuild_search_index`.
- bm25 statistics are global: top-50 membership can shift when other editions change. Citations must pin unit id + revision.
- Phrases can match across adjacent table cells, and ordered-list numbers are indexed as tokens.
- The page view itself cannot render unsanitizable legacy HTML. Repair for such units is therefore through re-import (or the store API), not the review page; oversized units can still be repaired in the UI.
- Flaky argon2/request-timeout test (finding 7).

## Re-review (fixes and rebase onto `1c848f1`)

Reviewer: independent Sonnet 5.5 subagent (fresh context, read-only). Muse Spark was unavailable.
Verdict: APPROVE with fixes, nothing blocking. Mutation testing: 4 of 6 mutants caught. The
two survivors were the handler-reset removal (finding 1) and the unbudgeted count queries
(finding 2).

| # | Finding | Severity | Disposition |
|---|---|---|---|
| 1 | The "handler removed afterwards" check in `lookup_is_interrupted_past_its_time_budget` was vacuous: the follow-up statement ran fewer than 1,000 VM ops. | Medium | Fixed. After the interrupted lookup, a `WITH RECURSIVE` count to 100,000 must succeed. Mutation (reset removed from `Budget::drop`): the test now FAILS; restored. |
| 2 | Count queries and `unindexed_units` ran outside the budget. | Low | Fixed. One `Budget` guard covers every statement of a lookup; count errors are mapped through `interrupted`. |
| 3 | The progress handler ticks only on VM opcodes, so a heavy FTS5 `xFilter` can't be interrupted mid-call. | Low | Documented (plan §13 and the code comment): the budget bounds VM-level work only. |
| 4 | Re-measure with `BUDGET_CHECK_OPS` 1,000 vs 10,000. | Low | Measured (release, `--cpus 4`, adverse corpus). 1,000: 15k p95 33 ms, 100k p95 216 ms. 10,000: 15k p95 32 ms, 100k p95 207 ms. The difference is within noise, and the zero-budget interruption test passes with 10,000, so **10,000 is kept**. The earlier 275 ms run was most likely host load. **The 100k adverse case misses the plan's 50 ms target**; the 15k case meets it. |
| 5 | The `upgraded` flag of `ensure` couldn't be tested independently. | Low | Argument removed. An upgrade creates empty tables, so the count check forces the rebuild. Equal counts with stale content are not detected at open; `Store::rebuild_search_index` covers that (documented). |
| 6 | A unit that fails re-derivation after a normaliser bump kept its stale indexed text. | Low | Fixed. On failure, a projection from another normaliser version is deleted. The delete guard now allows that case only (a failure recorded under a different normaliser), and a new `source_search_text_delete_fts` trigger removes the index row. Failing-first test `failed_rederivation_removes_the_stale_projection` (via `derive_with` with a synthetic failing normaliser): failed with `(1, 1, 1)`, then passed with `(1, 0, 0)`. |

Re-verification on the combined tree, CI container: `cargo fmt --check`, clippy
`-D warnings`, `cargo test --locked --all-targets --all-features` (exit 0), ruff
check/format, Python unittest (OK) all pass. The release-binary Chromium smoke
(`--network=none --auth`) passed. Uncommitted.
