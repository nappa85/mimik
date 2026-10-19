# Review: catalogue plan step 2 implementation (2026-10-19)

**Scope.**
- Branch `catalogue`: `aa23ace` (step 2), review fixes `cb9ddb8` (round 1), `339b23c`
  (round 2), `4a7c399` (round 3) and the round-4 fix commit recorded in plan §15.
- Contents:
  - migration 019 extended (catalogues, versions, entry types, entries, entry units, seals,
    triggers, the latest-only FTS5 name index);
  - `src/catalogues/publication.rs` (compare, ticketed publish, verify, rebuild);
  - the CLI commands `compare-catalogue`, `publish-catalogue`, `show-catalogue-version`,
    `verify-catalogues` and `rebuild-catalogue-index`;
  - Cf/Zl/Zp rejection;
  - `tools/catalogue_profile.py`.
- Specification: `docs/catalogue-plan.md` step 2, the amendments R1–R28, and §9. The
  implementation notes and red→green tables are in plan §11–§15.
- The review included the concrete seal-trigger SQL, as the plan review had asked.

**Reviewers.**

| Round | Commit | Sol (`--chain main`) | Sonnet (fresh subagent) | Third chain |
|---|---|---|---|---|
| 1 | `aa23ace` | FIX THEN MERGE | FIX THEN MERGE | unavailable (all three failed preflight) |
| 2 | `cb9ddb8` | FIX THEN MERGE | FIX THEN MERGE | unavailable (startup error, then preflight failures on retry) |
| 3 | `339b23c` | FIX THEN MERGE | MERGE-READY | Space Bunny: MERGE-READY |
| 4 | `4a7c399` | FIX THEN MERGE (profiler gate only) | MERGE-READY | Space Bunny: MERGE-READY |

- No reviewer reported a Blocking finding in any round.
- **Seal SQL:** every reviewer found it sound. Sonnet ran tamper sequences against a scratch
  database built from migrations 001–019, and every one was refused:
  - same-count substitutions;
  - NULL and type tricks;
  - after the seal, INSERT, REPLACE, UPSERT, UPDATE and DELETE.
- Sol and Space Bunny found it sound by reading the code.

## Findings and disposition

| # | Round | Source | Severity | Finding | Disposition |
|---|---|---|---|---|---|
| 1 | 1 | Sol F2, Sonnet F4 | Major | `read_projections` joined unit rows to entries, so an orphan unit row was invisible to verify | Confirmed against the code and fixed in `cb9ddb8`: units are read by `version_id`. Tamper regression red, then green |
| 2 | 1 | Sol F1, Sonnet F1 | Major | The R8 lock-hold evidence was incomplete: the probe saw only the remaining hold, v2 was not probed, and total references were unbounded | Fixed over rounds 1–3: a lock probe that opens before the publish starts; conservative spans identified by the version row (§14); several worst-case shapes; caps lowered (see #7) |
| 3 | 1 | Sonnet F2 | Major | verify re-admitted stored versions under the current rules | Fixed: `admission_profile` stored per version (CHECK, immutable); verify and show dispatch on it; profile 1 pinned by a test |
| 4 | 1 | Sol F4 | Minor | verify aborted on a missing or reshaped FTS table | Fixed: the definition is checked first. Static codes `name-index-missing` and `name-index-definition-mismatch` |
| 5 | 1 | Sonnet F4–F7 | Minor | verify had no foreign-key or sequence checks; draft content could be rewritten without a revision bump; an unsealed manual insert blocks its catalogue; `DROP_SCHEMA` duplicated | Fixed or documented: `foreign_key_check`, `version-sequence-gap`, `catalogue-without-versions`; the `catalogue_draft_revision_advances` trigger; an operator note; a sync test for `DROP_SCHEMA` |
| 6 | 1 | Sol F3 | Minor | clap parse errors echo argv | The guarantee is narrowed in the docs: stored content is never printed, while argv is the operator's own input |
| 7 | 2–3 | Sol 2, Sonnet A, Sonnet r3, Space Bunny | Major / Minor | The fan-out shape was not the worst admitted shape. Long names, 3-character tokens, 64-scalar keys and declaration-heavy shapes were all unprofiled | Profiled. Their holds forced the caps down step by step, ending at **6,144 entries and 6,144 unit references** (`MAX_ENTRIES`, schema `maxItems`, `MAX_UNIT_REFERENCES`). The worst measured hold is 1,804.0 ms (the fts3 update), and the docs say "worst measured on this host", not a proven bound |
| 8 | 2 | Sol 1 | Major | Probe timestamps were taken before `BEGIN`, so the bounds were not bounds | Fixed (§13): full probe windows |
| 9 | 3 | Sol 1 | Major | Transaction attribution: the startup lock, then the contender, then the publish could cap the release with the contender's earlier acquisition | Fixed (§14): `publish_span` runs from the last free probe that reads the old version count to the first that reads the new count, and is conservative by construction. The contender cap was removed. Space Bunny and Sonnet (in a 3,000-trial simulation) had found the previous bounds sound; Sol's case needed two missed gaps. It was fixed anyway because the fix was cheap and makes the evidence airtight |
| 10 | 2–3 | Sonnet B, Sonnet r3, Space Bunny M3 | Minor | Invisible-only text was admitted: Cf-only owner text, lone Mn, and blank-looking text property values | Fixed: `label()` requires a scalar that is not whitespace, blank-looking, Cf or Mn. A non-empty text property value must pass it, and `""` remains the explicit "no value" |
| 11 | 3 | Space Bunny M5 | Minor | The profiler's caps were copied by hand from Rust | Fixed: `RUST_LIMITS` is parsed from the Rust sources by a test. Round 4 added `MAX_DISTINCT_UNITS` and a duplicate-name check |
| 12 | 4 | Sol 1 | Major (tool) | The R8 gate passed with zero gated publications, or with a NaN or infinite budget | Fixed in the round-4 commit (§15): `r8_gate` fails when no publication was gated, or when the contender was requested but not observed; `hold_budget` rejects budgets that are non-finite or ≤ 0. Regressions added |
| 13 | 4 | Sol 2 | Minor | The acquisition and release endpoint intervals could belong to the store-open lock or the contender | Fixed: they are renamed `indicative_*`, and the outer `span_after_ms` is the one reported as the bound |
| 14 | 4 | Sol 3, Sonnet, Space Bunny M1 | Minor | Stale "8,192 cap" docstrings; comment and docs overstated that the keys shape failed at 4,096 references | Fixed |
| 15 | 4 | Space Bunny | Minor | The profile pin did not cover the Mn rule | Fixed: the pin test freezes `label()` on an accept/reject table |
| 16 | 4 | Sol 4 | Minor | The real-lock profiler test records "released" before COMMIT | Open as a follow-up: the gate's bound does not depend on it (the closing free probe must take the write lock). A controlled commit-phase regression is in the step-3 follow-ups |

**Accepted author decisions:**
- identical content that cites a now-rejected unit is refused with `rejected-units`
  (regression added);
- the crash test assumes the rollback journal, to be re-checked if WAL is enabled;
- owner text keeps Cf for bidi prose, and step 3 renders it in an isolated bidi context with
  a browser test (in the step-3 checklist);
- tightening `catalogue-admission-1` in place is acceptable only while nothing is published
  outside tests and the branch is unmerged. **After the merge, any admission change needs a
  new profile id.**

**Follow-ups:**
- Next lever for the caps: about 0.5 s of the worst hold is the name index re-tokenizing the
  previous names. A contentless, `detail=column` or no-prefix index experiment may restore
  higher caps.
- Unprofiled shapes: 52 owner texts at 20,000 scalars, and multilingual token alphabets.
- Re-measure on the target hardware with the R8 gate before raising any cap.
- Search (step 4) must never pass raw user text to MATCH: quote tokens as phrases and bound
  their count.
- The pre-existing tools error `test_run_queue.ProcessLauncherTests.test_fake_runner_script`
  keeps the helper suite from being fully green.

**Verification.**
- Each round's author gate is recorded in plan §11–§15.
- Profiling ran at 8,192 and 6,144 entries in a container with `--cpus 6` before Monday
  09:00.
- The reviews were static, plus Sonnet's scratch-DB, FTS and simulation probes. No reviewer
  reran the Rust gate.
