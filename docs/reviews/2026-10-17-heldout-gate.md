# Review record: held-out hash gate (plan §11 item 8)

2026-10-17. Scope: `0b9742d` on `harness-v3`: `tools/heldout-v3-pin.json`, the gate in
`tools/tool_loop_benchmark.py`, `tools/run_tool_loop_benchmark.sh` and `tools/run_queue.py`, the
tests and `docs/tool-loop-heldout-v3-freeze.md`. No reviewer read the frozen file.

## Reviewers

- **Sonnet:** CLEARED for the queue and runner path only (F1–F6).
- **Sol:** NOT CLEARED (1–2 high, 3–4 medium).
- **Space Bunny** (third chain): NOT CLEARED (1–2 blocking, 3–7 low/informational).

## Findings and dispositions

| ID | Sev. | Finding | Fix | Regression test (red at `0b9742d` → green) |
|---|---|---|---|---|
| Sol 1, SB 1, Son F1/F2 | High | `--heldout-pin` let a caller replace the pin; no code anchor; the pin's identity was not recorded | CLI flag removed; `HELDOUT_FROZEN_SHA256` code constant checked by `load_heldout_pin` (harness, runner via the harness loader, queue); headers record `heldout_pin_sha256` | `test_no_cli_pin_override`, `test_editing_the_pin_json_alone_cannot_move_the_anchor`, `test_matching_file_runs_and_the_header_records_case_and_pin_hashes`, runner `test_an_edited_pin_json_alone_is_refused` |
| Sol 2 | High | `load_ledgers` and the bar did not enforce the frozen hash; merges ignored it | Held-out headers and rows must carry the frozen hash (`load_ledgers`); `heldout_cases_sha256` and `heldout_pin_sha256` in `GROUP_IDENTITY`; `bar_verdict` gives `REFUSED` for an unpinned held-out summary | `test_unpinned_heldout_ledger_is_refused`, `test_merging_heldout_ledgers_with_different_case_hashes_is_refused`, `test_heldout_bar_refuses_an_unpinned_summary` |
| Sol 3, Son F4, SB 3/5 | Medium | Hash and parse in separate reads (harness, queue ids, subsets) | `read_verified_heldout`: one read, compare, parse those bytes; `Pack(heldout_bytes=…)`; queue ids and subsets from the same bytes | `test_one_read_hashed_then_parsed`, `test_hash_mismatch_is_refused_before_parsing`, `test_heldout_subset_files_come_from_one_verified_read`, queue `test_case_ids_come_from_one_verified_read` |
| Sol 4, Son F3, SB 2 | Medium | Held-out queue arms ignored `cases`/`cases_file` | `heldout_cases()` honours them; a `cases_file` must be a held-out subset whose source hash is the pin | queue `test_a_100_case_subset_arm_runs_end_to_end` (plan → completion), `test_a_subset_of_another_source_is_refused` |
| SB 4 | Low | Provenance injection through a tab/newline path | Refused in `HELDOUT_CASES` and the resolved path; provenance written with `printf` | runner `test_heldout_path_with_a_newline_is_refused` |
| Son F5, SB 6 | Low | `--validate-pack --split heldout` printed held-out gold details | Refused without `--allow-heldout-gold-replay`; with it, counts and pass/fail only | `test_validate_heldout_needs_the_opt_in_and_prints_counts_only` |
| Son F6 | Low | Space-unsafe pin parsing; empty arrays under `set -u` on bash < 4.4 | One value per loader call; `${arr[@]+"${arr[@]}"}` | runner `test_optional_arrays_expand_safely_under_set_u` |
| SB 7 | Info | Docs drift (runner header, usage, benchmark doc) | Runner header and usage name `HELDOUT_CASES`; `docs/tool-loop-benchmark.md` gate section; freeze record hardening list | — |
| SB 3 (cross-container) | Low | The runner's check and the harness read at different times | Accepted with the harness's one-read verification as the binding check (documented); the runner's provenance records its own pre-run hash | — |

Red runs: `test_heldout_gate.py` 14 of 16 failed against `0b9742d`; the five new queue tests
and the three new runner tests failed against the `0b9742d` queue and runner.

## Verification

Gate in `mimik-toolloop-py:20261015` (`--cpus 2`): ruff clean, **693 tests, 11 skipped, 1 error** (the environmental `test_package`); ShellCheck clean on `run_tool_loop_benchmark.sh`. The
frozen file was checked only with `stat` and `sha256sum -c --status` (matches the pin).

## Blockers and handoff

- Re-review of the fixes before any A0 verdict run.

## Round 2 (re-review of `f4b5e15`)

- **Sonnet:** CLEARED on condition M1.
- **Space Bunny:** CLEARED with two Medium findings (B1, B2) and B3–B7.
- **Sol:** NOT CLEARED (1 high, 2–3 medium).

| ID | Sev. | Finding | Fix | Regression test (red at `f4b5e15` → green) |
|---|---|---|---|---|
| Sonnet M1 | Medium | Held-out runs printed per-case `id outcome= correct=` lines unless QUIET=1; held-out validation errors named ids and fields | The harness forces quiet progress for held-out; the runner forces `--quiet-progress` (records `quiet_forced`); held-out validation and unknown `--cases` ids are counted, never echoed. Other output paths audited: run_aborted/stderr carry reasons only, provenance holds hashes and paths, the queue console is `console_safe`, summaries are aggregates | `test_heldout_progress_is_always_quiet`, `test_heldout_validation_errors_do_not_echo_case_content`, runner `test_heldout_progress_is_forced_quiet` |
| Sol 1 | High | `--write-case-subset --split heldout` wrote `outcome_counts`; successive prefixes revealed every label | Held-out subsets are registered frozen-order prefixes only, ids only (no `outcome_counts` or other gold) | `test_heldout_subset_artifact_carries_no_gold_and_only_registered_sizes` |
| Sol 2 | Medium | Default/dev `--validate-pack` loaded, replayed and printed a non-empty `cases-heldout.json` | CLI packs never load the pack's own held-out file (`include_heldout=False`); held-out cases only come from the verified bytes | `test_dev_validation_never_touches_a_heldout_file`; the existing `test_validate_pack_reports_prompt_audit_and_replay` now asserts no held-out replay |
| Space Bunny B1 | Medium | The verdict was not pinned to a pre-registered case set | `verdict_case_sets` in the pin (`full`, `h1-prefix-100`, by rule from the frozen order); runs on other id lists refused (harness before any ledger, queue arms); headers and rows record `heldout_case_set`; `load_ledgers` checks it against `case_order`; `bar_verdict` refuses unregistered or mismatching sets; in `GROUP_IDENTITY` | `test_registered_case_sets_only_and_recorded`, `test_bar_refuses_unregistered_or_mismatching_case_sets`, queue `test_an_unregistered_case_list_is_refused` |
| Space Bunny B2 | Medium | A dry-run / gold-replay held-out ledger summarised like a live run | Held-out rows carry `dry_run`, `dry_run_expected`, `allow_heldout_gold_replay`; summaries carry them; `bar_verdict` refuses (unknown counts as set) | `test_bar_refuses_dry_run_and_gold_replay_summaries` |
| Sol 3 | Medium | A space in the held-out path injected provenance fields | All whitespace refused in `HELDOUT_CASES` and the resolved path | runner `test_heldout_path_with_a_space_is_refused_and_injects_nothing` (queue classification reads `failure=config`) |
| Space Bunny B3 | Low | Other optional arrays still expanded unguarded under `set -u` | Every optional array uses `${a[@]+"${a[@]}"}` | runner `test_optional_arrays_expand_safely_under_set_u` (widened to all optional arrays) |
| Space Bunny B4 | Low | `fail()` interpolated unvalidated values into `failure_detail=` | `fail()` replaces tab/CR/LF and writes with `printf` | runner `test_failure_details_never_add_provenance_lines` |
| Space Bunny B5 | Info | Benchmark doc still listed the gate as deferred | Fixed | — |
| Space Bunny B6 | Info | `heldout_source["path"]` was dead | Removed | — |
| Space Bunny B7 | Info | The hash chain is not tamper-evident | Recorded as a limitation in the freeze record | — |

Red runs (round 2): of the 12 new tests, 11 failed against the `f4b5e15` harness, queue and
runner. The twelfth, `test_heldout_path_with_a_space_is_refused_and_injects_nothing`, passed
there only because the old pin loader rejected the new pin format first (exit 2, `config`); its
red-ness for the space itself is shown by the reviewer's scenario, not by that run. Legacy bar
fixtures now carry the fields of a live, gated, registered full-set run.

Verification (round 2): gate in `mimik-toolloop-py:20261015` (`--cpus 2`): ruff clean,
**704 tests, 11 skipped, 1 error** (the environmental `test_package`); ShellCheck clean on the
runner. The frozen file was checked only with `sha256sum -c --status` (matches). The pin's
per-set `sha256` fields are left null: filling them needs the frozen ids, which this session may
not read; the run-time rule check from the verified bytes holds without them.

Handoff: re-review of round 2 before any A0 verdict run.

## Round 3 (re-review of `7ed4887`)

- **Sonnet:** CLEARED conditionally (F1–F4, plus a FormulaInfraError note).
- **Sol:** NOT CLEARED (1–2 high, 3 medium).
- **Space Bunny:** NOT CLEARED (R3-1 high, R3-2 medium, R3-3–R3-7).
- **Main-session decisions:** fill the per-set digests (one authorised ids-only computation),
  anchor them in code, and accept gold in held-out ledgers by policy.

**Ids-only computation (authorised).** A scratch script read the frozen file only through
`read_verified_heldout` (hash-checked bytes), took the case ids, and printed only:
`full 165 4f5f0b031f6f2595ff3bae8f73fa6ce94b7787d30559bef63b4e04fab6bbfce1` and
`h1-prefix-100 100 9d5768426c503c4581627af5172a690907fdcbdfaf8c03c7dcaa97cd3b24f4a5`, using
the harness's own `case_set_sha256`. No id, case content or label was printed or stored.

| ID | Sev. | Finding | Fix | Regression test |
|---|---|---|---|---|
| Sol 1, Sonnet F2 | High | Ledger and verdict checks accepted a self-consistent unregistered set (any ids as `full`) | Digests in the pin and as code constants (`HELDOUT_VERDICT_CASE_SETS`, with sizes); `load_ledgers` requires name, digest, size and `case_order` re-hash to match the constant, and every row's set equals the header's; `bar_verdict` requires the same and `unique_cases == size`; run time (harness, queue) requires the computed digest to equal the pinned one | `test_ledger_with_a_self_consistent_unregistered_set_is_refused`, `test_committed_case_set_digests_are_code_anchored` |
| Space Bunny R3-1 | High | `verdict_case_sets` were shape-checked only | `load_heldout_pin` refuses sets other than the code constant and pin bytes other than `HELDOUT_PIN_SHA256` | `test_editing_only_the_verdict_case_sets_is_refused` |
| Space Bunny R3-2 | Medium | `bar_verdict` re-read the live pin | Rows and summaries carry `heldout_pin_sha256`; the verdict checks it and the set digests against code constants only, with no pin file needed | `test_bar_uses_the_ledger_recorded_pin_not_the_live_file` |
| Sol 2 | High | Held-out ledgers carry per-case gold; a plain held-out `--dry-run` wrote such rows | Accepted by policy (scoring needs gold): held-out ledgers are mode 0600 and read only through `--summarize`'s aggregate output (freeze record, benchmark doc, TODO). Held-out `--dry-run` requires `--allow-heldout-gold-replay`. The interleaved `case_order` in the header is the same gold-bearing artefact and falls under the same policy | `test_heldout_dry_run_needs_the_opt_in`, `test_heldout_ledgers_are_private`, `test_summarize_output_is_aggregate_only` (guard) |
| Sol 3, Sonnet F1, Space Bunny R3-5 | Medium | `--summarize --episodes-csv` exported per-case gold | Refused for held-out rows without `--allow-heldout-gold-replay` | `test_episodes_csv_refuses_heldout_rows_without_the_opt_in` |
| Space Bunny R3-3 | Low | No test for the counts-only unknown-id branch | Test added (the branch already counted; the test is a guard) | `test_unknown_heldout_case_ids_are_counted_not_echoed` |
| Space Bunny R3-4 | Low | Doc said validation prints failing ids | Sentence fixed | — |
| Space Bunny R3-6 | Info | `console_safe` missed `h3-001`-style ids | Pattern widened to letters-with-digits prefixes and 3+ digit numbers | `test_every_case_id_shape_is_masked` |
| Space Bunny R3-7 | Info | A `PackError` from episode setup could name a case | Held-out runs re-raise it generically | — (unreachable today; covered by load-time validation) |
| Sonnet F3 | Low | `run_aborted` rows carry `case_id` | Ledger only, covered by the ledger policy. **Correction:** round 2 of this record said "run_aborted/stderr carry reasons only"; the stderr line carries the reason only, the `run_aborted` *ledger row* carries the case id | — |
| Sonnet F4 | Low | `QUIET` echoed unsanitised into provenance | `QUIET` must be 0 or 1 (else exit 2); the echo is a fixed string | covered by the runner tests |
| Sonnet (note) | Low | `FormulaInfraError` `stderr_tail` could echo held-out formulas | Held-out runs print a generic message without the error text or tail | `test_formula_faults_on_heldout_print_no_stderr_tail` |

Red runs (round 3): all 11 new tests failed against `7ed4887`. Most of the failures were
attribute errors on the new constants; the behavioural regressions are
`test_editing_only_the_verdict_case_sets_is_refused` and `test_every_case_id_shape_is_masked`.
The aggregate-summary and unknown-id tests are guards for behaviour that was already correct.

Verification (round 3): gate in `mimik-toolloop-py:20261015` (`--cpus 2`): ruff clean,
**715 tests, 11 skipped, 1 error** (the environmental `test_package`). ShellCheck clean on the
runner. The committed pin loads under its anchors. The frozen file was touched only by the
authorised ids-only script and by `sha256sum -c --status` (which matches).

Handoff: re-review of round 3 before any A0 verdict run.

## Round 4 (re-review of `66f56c1`)

- **Sol:** **CLEARED** for A0 held-out runs (two Lows).
- **Sonnet:** **CLEARED** (one Low, plus optional Info items).
- **Third reviewer unavailable** (`--chain third` preflight); proceeded with the two main
  reviews per AGENTS.md.

| ID | Sev. | Finding | Fix | Regression test (red at `66f56c1` → green) |
|---|---|---|---|---|
| Sol 1, Sonnet 1 | Low | The outer `FormulaInfraError` handler matched `"heldout" in argv`, so `--split=heldout` (or an abbreviation) printed the error and its stderr tail | `_split_of` parses the arguments with the harness's own parser; anything other than a clean `dev` parse gets the generic message | `test_outer_fault_handler_parses_the_split` (red) |
| Sol 2 | Low | The membership regression test changed the set size, so a size check alone could keep it green | New same-size forgery: one id replaced by an unregistered one, case order and rows updated, with (a) a recomputed digest and (b) the registered digest falsely attached; both refused by `load_ledgers` and `bar_verdict` | `test_same_size_replacement_sets_are_refused_by_digest` (coverage: green before and after, the digest checks already existed) |
| Sonnet 2 | Low | `load_ledgers` did not cross-check each row's `heldout_pin_sha256` against the header | Refused on any mismatch | `test_row_pin_digests_must_match_the_header` (red) |
| Sonnet (Info) | Info | `QUIET` other than 0/1 exits on dev too; double opt-in flags in test helpers | Kept: strict validation is intended, and the duplicate flag is harmless | — |

Verification (round 4): gate on `harness-v3` (`--cpus 2`): ruff clean, **718 tests, 11 skipped,
1 error** (the environmental `test_package`); the queue fixture's rows now carry the pin digest
like real rows. Then merged into master (see `TODO.md`).
