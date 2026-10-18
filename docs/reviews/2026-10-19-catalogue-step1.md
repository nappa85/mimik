# Review: catalogue plan step 1 implementation (2026-10-19)

**Scope.**
- Branch `catalogue`: `f4938e2` (step 1) and `b822bd6` (review fixes).
- Contents: the catalogue decode profile, `schemas/catalogue-v1.json`, domain checks, the
  canonical encoding and frozen hash vector, source consistency checks at draft save, migration
  019 (`catalogue_drafts` only), and the redacted CLI commands `import-catalogue-draft` and
  `show-catalogue-draft`.
- Specification: `docs/catalogue-plan.md` step 1, D2/D5/D6, and the amendments in §7–§9.

**Reviewers.**
- Round 1:
  - Sol (`--chain main`): FIX THEN MERGE.
  - Fresh Sonnet subagent: FIX THEN MERGE.
  - Third chain: unavailable (Space Bunny, DeepSeek and Muse all failed preflight).
- Round 2 (the fixes): Sol and Sonnet, both **MERGE-READY FOR STEP 1**. The third chain was not
  retried, since it had been unavailable in every round today.

## Findings and disposition

| # | Source | Severity | Finding | Disposition |
|---|---|---|---|---|
| 1 | Sol F1 | Major | `redacted_path` kept all-digit and member-name segments regardless of position, so an author property key such as `314159265358979` reached errors and CLI stderr | Fixed in `b822bd6`. The walker is position-aware and every property-map key is redacted. Library and CLI regressions were red on the old code |
| 2 | Sol F2, Sonnet 2 | Major | Deferred transaction at draft save: lock upgrade gave `Storage(DatabaseBusy)` instead of `Conflict`, and distinct catalogues could fail | Fixed: `BEGIN IMMEDIATE` before every read. Deterministic rendezvous race tests were red on the old code; the CLI race now requires `Conflict`. The definitions mirror has the same pattern; it is recorded as a pre-existing issue and left unchanged |
| 3 | Sol F3, Sonnet 1 | Major | Escaped-duplicate tests had no escapes (the backslashes were lost when written); the source/page/unit join had no test reaching it | Fixed: escaped keys are built in code at six levels; two real finalized sources with four cross-wired relationships. Red was shown by mutation (duplicate check removed, `p.source_id` predicate dropped, unit→page join neutralised) |
| 4 | Sonnet 3 | Minor | Control characters accepted in names, labels and text | Fixed: Cc is rejected; owner text allows only LF and TAB |
| 5 | Sonnet 4 | Minor | Untested: identical-content save, `Other` contexts, an adapter-bearing nonnative pair | Tests added (the adapter test uses a seam, because no product path can create that state) |
| 6 | Sol F4 | Minor | Admission worker lifecycle duplicated from definitions | Accepted for now; noted in plan §10 |
| 7 | Sonnet round 2 low 1 | Low | `char::is_control` misses category Cf (bidi overrides, zero-width) and Zl/Zp, so display spoofing is possible, and a name of only zero-width characters is accepted | **Carried into step 2:** reject Cf, Zl and Zp in names and labels before any catalogue text is displayed |
| 8 | Sonnet round 2 low 2 | Low | CRLF owner text is rejected | Deliberate; document "normalise to LF" in `catalogue-publication.md` (step 2 docs) |
| 9 | Sonnet round 2 low 4 | Note | The race tests assume the rollback journal; re-check them if WAL is enabled | Noted |
| 10 | Sonnet note | Note | Migration 019 grows on this branch, so **step 1 must not be merged alone** | The branch merges after step 2 |

**Accepted author decisions:**
- the text property cap of 1,000 scalars;
- at least one entry type, while zero entries are allowed;
- nonnative, adapter-free pairs only;
- the property map closed by the exact key set;
- the owner-text total counted in bytes;
- the canonical cap kept as an unreachable defensive check.

**Verification.**
- **Author gate** (container, `--cpus 6`, Sunday 23:36–23:43): fmt, pedantic Clippy clean,
  498 Rust tests passed with 1 pre-existing ignored, ruff, and the tools tests OK.
- **Reviews** were static. Neither reviewer reran the tests.
