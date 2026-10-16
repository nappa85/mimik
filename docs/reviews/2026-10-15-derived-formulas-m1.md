# Derived formulas (CEL): plan revisions and milestone M1

Scope: `docs/derived-formulas-plan.md` revisions 1–2 and milestone M1 in the
`derived-formulas` worktree (committed as `97ad442`). M1 covers:

- `src/definitions/formula/`;
- `schemas/formula-conformance-v1.json`;
- `examples/formula_eval.rs`, `tools/test_formula_cli.py`;
- the Cargo.toml and ci.yml changes.

M2–M4 (definition envelope, admission, migration, sheet view/save, list editing) are not
implemented and are not covered by this record.

## Reviews

| Review | Reviewer (fresh context, read-only) | Verdict |
|---|---|---|
| Plan rev 1 (custom language, winnow) | Independent subagent; model not recorded in the coordinator handoff | GO-with-amendments, then superseded by the user's CEL decision |
| Plan rev 2 (CEL via `cel` 0.15.0) | Sonnet 5.5 subagent; the external Muse Spark reviewer was out of quota | GO-with-amendments, 20 items |
| M1 implementation | Sonnet 5.5 subagent; Muse Spark out of quota | APPROVE-with-fixes |

Both reviews were independent of the implementer, but neither used a different model
family, because the external reviewer was unavailable. Self-checks below are supplemental.

## Plan rev 2 findings

All 20 items have a disposition in plan §14.1.

- **Blocking (resolved in M1):**
  - 1: i64-only conversion, with item-level integer checks moving to M2's
    `validate_values`;
  - 2: overflow coverage, plus a debug and release differential test;
  - 3: composed bounds (canonical/pin caps, one list constant, cost and deadline
    guards, string cost, calibration).
- **Medium:**
  - 4–7 and 10–13 are resolved in M1;
  - 8 (hash member order) is an M2 gate;
  - 9 (underscores) is partly M1 (reserved names), the rest M2.
- **Low:**
  - 14, 17, 19 are resolved;
  - 15, 16, 20 are M2 gates with named regressions;
  - 18 is plan-only.

Finding of our own during M1: in debug builds the ANTLR parser overflowed a 2 MiB stack
(at 12 nested parentheses) and cel's evaluator overflowed it on a 61-term chain, both
inside the profile limits. Compile and evaluate now run on a 16 MiB scoped thread, and
regressions call in from 2 MiB threads.

## M1 implementation findings

| # | Severity | Finding | Disposition |
|---|---|---|---|
| 1 | Medium | No test covered the `catch_unwind` in `run` or the `on_formula_stack` join-error mapping | Fixed: `guarded` helper plus test-only fault injection; three tests; mutation confirmed |
| 2 | Medium | No aggregate input-size bound; per-value caps allowed hundreds of MB, doubled by cloning | Fixed in M1: 1 MiB `MAX_INPUT_BYTES`, charged before copying, `input_too_large`. M2 gate recorded for the sheet `evaluate` boundary |
| 3 | Low | `sum`, `size`, `max_of`, `min_of` charged a flat 1 unit | Fixed: `MAX_LIST_ITEMS` units each; exact-cost test |
| 4 | Low | Two filter-shape checks were unprotected by tests | Fixed: hand-built near-miss comprehension test; mutation confirmed |
| 5 | Low | `pub mod formula` is public only for the example | Comment added; revisit at M2 (plan §14.1a) |
| 6 | Low | Wall-clock assertion would flake on a loaded CI runner | Now asserts cost; time is report-only |

## Mutation results

- **Reviewer, before the fixes:** 10 of 13 mutants killed. The survivors were the
  `catch_unwind` in `run`, the `on_formula_stack` join mapping, and the filter-shape checks
  in `profile::shape`.
- **Implementer, after the fixes:** the same tests were re-run against six targeted
  mutants, each applied and then restored by a script. All six were killed:
  - `guarded` without `catch_unwind`;
  - `on_formula_stack` without the thread;
  - the filter else-branch accumulator check;
  - the appended-element check;
  - the `size` list charge;
  - the input budget.

  The new tests for items 2 and 3 were also red before their fixes. The item 4 test
  guards existing code, so it was confirmed by mutation instead.

## Gates (2026-10-15, `mimik-ci` container, volume `mimik-formula-target`)

- `cargo fmt --check` and `cargo clippy --locked --all-targets --all-features -- -D warnings`:
  pass. No `#[allow]` was added.
- `cargo test --locked --all-targets --all-features`: all pass, 27 of them formula tests.
- `cargo test --release --locked --lib formula`: 27 pass. The calibration report shows
  18.9 ms for the largest admitted worst-case definition.
- `cargo build --locked --example formula_eval`: pass.
- `ruff check` and `ruff format --check`: pass. `python -m unittest discover -s tools`:
  128 tests pass, including the CLI conformance tests, which run the golden fixture
  through the CLI.
- `actionlint`: pass.
- `cargo audit --deny warnings` and `cargo about --fail` passed on the M1 dependency set
  before these fixes. The fixes add no dependency.

Not run: release binary size, package/vendor/unpacked artifact and browser gates. These are
M3/M4 scope. M1 is unit and CLI verification only: there is no application integration,
and no live model or benchmark result.

## Handoff

- **Open M2 gates (plan §14.1, §14.1a):**
  - i64 walk over item values in `validate_values`;
  - an input cap at the sheet `evaluate` boundary;
  - hash member order with a byte-compare test;
  - scoping the underscore relaxation;
  - a migration ladder contiguity test after rebasing onto admin's `MIGRATIONS`;
  - encoding-2 snapshot rejection;
  - `formula-profile-required` through `private_pin::validate`;
  - narrowing `pub mod formula`.
- **Not changed:** `tools/tool_loop_benchmark.py` and `examples/tool-loop-demo/`. A
  benchmark is running from the main checkout, and the switch to the CLI comes after it.

## M2 (definition envelope, admission, values, publication, migration 018)

Status: **implemented and independently reviewed, with review fixes applied
(committed as `1166a1d`).** The scope, deviations and verification are in plan §15.

Self-check findings, supplemental only and not independent:

| # | Severity | Finding | Disposition |
|---|---|---|---|
| a | Medium | The planned snapshot encoding 2 conflicts with the immutable `CHECK(encoding=1)` and the trigger's key set | Kept encoding 1; separate unresolved lists derived from content; tamper test. Reviewer to confirm |
| b | Medium | Formula definitions could have been published before view/save shows derived values | Approval gated (`formula-evaluation-unsupported`) until M3 |
| c | Low | The contiguity test's last-statement parse could not handle a comment-only preamble | The test now skips `--` lines |
| d | Low | First version of the encoding tamper test survived its mutant: the hash mismatch rejected the tamper before the encoding gate did | Test now rehashes consistently; mutant killed |

Mutation results, implementer-run, all killed:
- publish gate removed;
- evidence reference check skipped;
- `verify_approval` encoding gate opened;
- list-item i64 check disabled.

Gates: see plan §15 (all passed in `mimik-ci`; `cargo clean --release` afterwards).

### M2 independent review

Reviewer: Sonnet 5.5 subagent (fresh context; Muse unavailable). Verdict:
**APPROVE-with-fixes**, nothing blocking. All three deviations were judged sound: encoding 1
kept, formula publication gated until M3, and the contiguity test skipping comment lines.

| # | Severity | Finding | Disposition | Test evidence |
|---|---|---|---|---|
| 1 | Medium | Evidence capture in `publication::snapshot` was untested: deleting the loop left the tests green | Test `formula_evidence_units_are_captured_in_the_wiring_snapshot`. The fixture gets a second unit cited only by constraint evidence. Via `compare_definition` (ungated), the test asserts the unit is in `snapshot.units` and that correcting it bumps its captured revision and changes `comparison_hash` | Mutant (loop `take(0)`) killed |
| 2 | Medium | `Mapped { regions: [] }` was accepted as evidence | The schema already requires one region. `formula_admission::evidence` now also rejects empty lists (`evidence-regions-required`) as a domain check | Red before the fix; mutant killed |
| 3 | Medium | The 1 MiB total cap applied retroactively to formula-free pins | The cap now applies only when `formula_profile` is set. A test shows a formula-free pin over 1 MiB still validates | Red before the fix; both mutants (cap always applied, cap removed) killed |
| 4 | Low | Is evidence region validation equivalent to field mappings? | Confirmed equivalent: same `mapping` schema `$def` (1–4 unique positive-ID regions), same unit/page/source SQL match through the shared walk, and the same 1,024 dependency limit. The **only difference is intended**: evidence requires a `core`/`supplement` source role, wiring requires `sheet`. **Accepted** | Existing tests |
| 5 | Low | Leftover nested blocks in `private_evidence_available`, and duplicated region SQL in `store::references` | Flattened. `formula_admission::dependencies()` is the single walk, used by `regions()`, `evidence_regions()` and `references()` | Covered by the tests above |
| 6 | Low | `references()` reported `/evidence` for evidence mismatches | Now reports the exact `/fields/N/formula/evidence` or `/constraints/N/evidence` path. Field wiring keeps its legacy `/fields` path | Red before the fix; mutant killed |
| 7 | — | The reviewer's mutants for the list cap and item-ID uniqueness did not compile | Re-run by the implementer. Uniqueness mutant: killed. List-cap mutant (`maxItems` 1000): **initially survived**, because the 257-item case reused one ID and uniqueness rejected it first. The case now uses unique IDs, and the mutant is killed | Both killed |

Gates after the fixes (`mimik-ci`; disk 40G free):
- fmt and strict clippy: pass.
- `cargo test --all-targets --all-features`: 332 passed, 0 failed.
- Release formula tests: 46 pass; calibration 17.9 ms.
- `formula_eval` build, ruff, unittest (93), actionlint, `cargo audit` and `cargo about`:
  all pass.
- `cargo clean --release` afterwards.
