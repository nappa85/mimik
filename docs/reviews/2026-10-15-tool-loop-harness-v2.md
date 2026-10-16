# Independent review: tool-loop harness v2

**Reviewer:** a fresh-context, read-only reviewer on Sonnet 5.5 (Muse was unavailable),
commissioned by the coordinating session.

**Implementer:** the harness subagent (Opus 5.5). It owns every fix below. The reviewer did
not edit code.

**Verdict:** READY-with-fixes. All findings were applied the same day, with regression tests.
Review findings are not proof of correctness, and this record is not a certification. Under
AGENTS.md this is prototype-tier evaluation code, and the review is required before the
overnight results drive a model or product decision.

**Scope reviewed:**

- `tools/tool_loop_benchmark.py` (harness v2) and its tests;
- the CEL seam (`CelCliEngine` over `examples/formula_eval.rs`);
- the translated pack `examples/tool-loop-demo/definition.json`;
- the runner, then `harness-v2/harness-run-v2.sh` and now `tools/run_tool_loop_benchmark.sh` (moved into the repository after the review, with a 35B model key and `N_CPU_MOE`);
- `docs/tool-loop-benchmark.md`.

Excluded: the Rust formula module (it has its own review, `2026-10-15-derived-formulas-plan.md`) and model results.

## Findings and dispositions

| # | Severity | Finding | Disposition | Regression tests (failing first unless noted) |
|---|---|---|---|---|
| 1 | Blocking | A submitted proposal where a non-proposal outcome was expected (e.g. answer) was not false support | Fixed: `wrong_proposal` is set for any submitted proposal when the expected outcome is not a proposal (at-risk cases also keep `outcome_error`). **Decision:** an `answer` where a proposal was expected is incorrect but not false support (documented) | `ReviewScoringTests.test_proposal_when_answer_expected_is_wrong_proposal` (failed first). `…test_answer_when_proposal_expected_is_incorrect_not_false_supported` passed before the fix and locks the decision in. `OutcomeTests.test_submitted_violation_is_false_supported` updated to include `wrong_proposal` |
| 2 | Blocking | Value scoring passed hedges and allowed len+2 values | Fixed: a reported bool or string that contradicts the expected values of its type makes the answer `wrong` (false support). The cap is now len(expected)+1 (`over_cap`, false support). `key_exact` is still reported beside `correct`, and the docs state that the bar uses these rules | `test_hedged_booleans_and_enums_fail`: `{x:true, y:false}` vs `{can_cross:false}` fails; the single correct value passes; an enum hedge fails. `test_cap_is_one_extra_value`: the int dump at the old len+2 cap now fails. `AnswerByValueTests.test_over_cap_dump_fails` updated to the new cap |
| 3 | Blocking | CLI infrastructure faults (exit, closed output, timeout, broken pipe, bad JSON) were raised as coded `FormulaEvalError` and could reach the model as rejections; `violations()` was unguarded; CLI stderr went to `DEVNULL` | Fixed: a distinct `FormulaInfraError` (not a `FormulaEvalError` subclass) covers process exit, closed output, timeout, `BrokenPipeError`/`OSError`, `JSONDecodeError`, protocol `error` responses and malformed results in both `derive` and `violations`. `run_episode` re-raises it. `run()` writes a `run_aborted` row with a ≤ 8 KiB stderr tail and no summary, and `main` exits 3. Coded `violations()` errors become proposal rejections. Stderr goes to a temp file, read back bounded | `FormulaInfraTests` with a fake CLI (exit, garbage, hang/timeout): all errored before the fix. Also `test_infra_fault_aborts_run_with_flagged_ledger` and `test_main_exits_nonzero_on_infra_fault`. `test_cel_engine.test_killed_cli_mid_run_is_infra_error` kills the real CLI mid-run; it was **written after the fix**, and the fake-CLI tests are the failing-first cover |
| 4 | Medium | The runner did not hash the model before launch and lacked harness and source provenance | Fixed: the model and CLI are hashed before launch, with abort on mismatch (exit 5 and 3). `provenance.txt` and the ledger header record the actual hashes, the `tools/tool_loop_benchmark.py` sha256, git HEAD and dirty count, the harness file's git status and the CLI source commit (`formula-cli-build.txt`: 97ad442, formula sources clean) | Runner self-test (`CHECK_ONLY=1` with a 12-byte stand-in model): wrong hash → exit 5; right hash → exit 0 with full provenance. This is a shell test outside unittest (the runner is a local, ignored file) |
| 5 | Medium | Tool-description examples used pack terms (focus, rope-1, pace 4, encumbered) | Fixed: neutral `score`, `items/item-1/active`, `level` and `ready`. The answer description also says one value per answer, no alternatives, and at most one extra | `NeutralToolTextTests.test_tool_descriptions_are_game_agnostic` (word-boundary check; failed first) |
| 6 | Low | The hash-check container was not hardened; runs were not documented as sequential; no fail-fast | Fixed: the check container uses the pinned `mimik-ci` image id, `--network none`, uid 1000, `--cap-drop ALL`, no-new-privileges, `--read-only` and memory/pid limits. A `flock` lock file enforces strictly sequential runs (exit 9). The harness aborts after 5 consecutive `error` episodes, or after a failed `GET /health` following an error (exit 4) | `FailFastTests.test_five_consecutive_errors_abort` and `…test_failed_health_check_aborts` (failed first) |
| 7 | Low | File-order jobs let a budget cut drop whole outcome groups; `--summarize` merged incompatible ledgers | Fixed: a deterministic, seed-independent proportional interleave by expected outcome (`case_order` in the header). `--summarize` refuses mismatched harness version, pack hashes, CLI sha256, or per-label `model_sha256`, plus aborted or headerless ledgers | `OrderAndMergeTests` (failed first). `AbortedLedgerTests` was added after the fix for the extra aborted/headerless refusal |
| 8 | Low | Strict typing was undocumented | Fixed in `docs/tool-loop-benchmark.md`: `"4"` ≠ `4`, `true` ≠ `1`, exact case-sensitive strings | Documentation only |

Correction during the work: my first draft of the interleave test expected exactly one answer
case in the first half, but proportional interleaving legitimately gives one or two. I
loosened the test to that range, which does not weaken the property. The game-agnostic test
first matched "rope" inside "properties", and now uses word boundaries.

## Verification (2026-10-15, after all fixes)

- **Unit tests:** ruff check, ruff format and unittest in `mimik-ci` with the CLI mounted
  (`MIMIK_FORMULA_EVAL`): **181 tests OK**, including other agents' suites. Without the
  binary, `test_formula_cli` (master) and `test_cel_engine` fail closed by design.
- **Pack:** `--validate-pack` on `examples/tool-loop-demo` passed.
- **Dry runs** in the hardened harness container (uid 1000, cap-drop, read-only,
  `--network none`):
  - trivial script: 66 dev and 282 held-out episodes, 0 errors;
  - expected-outcome replay: 33/33 and 141/141 correct, 0 false support, 0 errors (a
    harness check, not a model result);
  - `--summarize` merge of compatible ledgers OK.
- No live inference after the review. The single 9B smoke happened before it, under v2.0
  scoring, and is not rescored.

## Open items and handoff

- The overnight rerun has not started. It waits for the user to confirm an idle machine.
  Command: `tools/run_tool_loop_benchmark.sh <model> <dev|heldout> [seeds]`, one run at a time.
- `tools/tool_loop_benchmark.py` and its tests are untracked, and the runner records that
  (`harness_file_git_status='??'`). Committing them before the rerun would make the
  provenance commit-addressable.
- Independent review of the overnight results is still required before they drive any
  decision.

## Commit preparation (after the review)

The runner moved to `tools/run_tool_loop_benchmark.sh`. `ROOT` comes from the script
location, output goes to `target/tool-loop-benchmark-<date>/`, and there is a third model key,
`qwen36-35b-a3b`, which requires `N_CPU_MOE`. Smoke now uses a new harness `--cases` filter
(failing-first `CaseFilterTests`). Runner self-tests (`CHECK_ONLY=1`):

- a missing `N_CPU_MOE` exits 2;
- a bad model hash exits 5;
- with good hashes, `--n-cpu-moe` appears in `server_args`.

Packaging: Cargo `include` gains `/tools/*.sh`, `/tools/fixtures/**` and
`/examples/tool-loop-demo/**`. `verify_source_package.REQUIRED` lists the harness, its tests,
the runner, the fixture pack and the demo pack. The `test_package` required-member regression
failed first for all 8 new names.

The full `ci.yml` checks sequence ran in mimik-ci as root, with a dedicated target volume, on
2026-10-15. Every step passed (exit 0):

- fmt, clippy, ruff and cargo test;
- the formula_eval build and source_fixture;
- unittest: 182 OK;
- actionlint, cargo audit and cargo about;
- the release build;
- cargo package, the source-member allowlist and vendor;
- package_release;
- unpacked-crate helper ruff and unittest: 173 OK, with 2 skipped by design (the CLI example
  is not in the source package);
- SHA256SUMS;
- `smoke_release.py --auth` on the unpacked binary.

Not run: the offline dependency-free source rebuild, the browser smoke and the LAN smoke of
the other CI jobs.
