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
