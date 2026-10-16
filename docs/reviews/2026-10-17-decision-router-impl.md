# Review record: decision router SYS-R implementation

2026-10-17. Scope: the dev-only router implementation on `harness-v3` at `95cc971`
(revision 3.1 of [decision-router-plan.md](../decision-router-plan.md), notes in
[decision-router-implementation.md](../decision-router-implementation.md)): `decision_router.py`,
`router_state_builder.py`, `router_driver.py`, `router_tokens.py`, `decision_models.py` and
`.json`, `run_decision_router.sh`, the router arms in `run_queue.py`, the worker changes in
`local_decision_benchmark.py`, live SYS-R in `tool_loop_benchmark.py`, and their tests.
Prototype tier: the review is required before the code drives any decision. No inference ran.

## Reviewers

- **OpenAI Sol** (`openai/gpt-6.1-sol`, `tools/external_review.py --chain main`), static
  read of a sanitized snapshot: **NOT CLEARED** for architecture-driving results (1 blocking,
  4 major, 2 minor).
- **Sonnet** (fresh subagent, read-only): **conditional GO** (2 major, 7 minor, nits).
- **Third reviewer: unavailable.** Space Bunny, DeepSeek v4.1 Flash and Muse Spark all timed
  out in preflight (3 attempts each, ≈00:20 on 2026-10-17); proceeded with the two main reviews
  per AGENTS.md. The main session will retry the third chain on the fixed commit.
  - Third-reviewer findings: **pending** (main session).

Adjudication by the implementer against the code at `95cc971`; every finding below reproduced
as a failing test before its fix (red run recorded), except the payload pin, which is a guard
that passed before and after.

## Findings and dispositions

| ID | Sev. | Finding | Verdict | Fix | Regression test (red at 95cc971 → green) |
|---|---|---|---|---|---|
| Sol B1, Son M2, m3, m9, nit | Block / Major | A report was not bound to its screen, states, plan, mapping, option hash or runtime; the decision file trusted the report's gate and rebuilt runtime provenance from today's registry; the harness checked only header keys; the probe was not tied to the registry hashes; choose mixed reports | Valid | Driver summary carries `states_sha256`, `input_plan_sha256`, option hash, worker runtime, the runner's run identity (image, artefacts); rows carry `state_hash`. `check_attempt` verifies every binding and the live pack hashes; the report binds the input hashes and runtime. `build_decision_file` recomputes the selection from the attempts and refuses a stale report or a runtime that differs from the registry. `verify_decision_file` compares pack hashes, fold hash and builder hash with the live pack. `probe_result` checks image and artefacts. `choose` refuses reports from different option hash, folds or pack | `test_router_review_fixes`: `test_report_binds_every_input_hash_and_the_run_runtime`, `test_stale_report_is_refused`, `test_wrong_screen_states_are_refused`, `test_wrong_runtime_is_refused`, `test_row_option_hash_mismatch_is_refused`, `test_verify_compares_pack_fold_and_builder_hashes`, `test_probe_summary_bound_to_image_and_artefacts`, `test_choose_refuses_reports_from_different_inputs`; `test_sys_router.test_decision_file_refusals` (pack hash, falsifier through the harness) |
| Sol 2, Son M1 | Major | `falsifier.null` was ignored by choose and the decision file; ablations had no consolidated effect | Valid | choose and `build_decision_file` exclude falsifier-null routers; `verify_decision_file` refuses them; a `claim` block requires the gate, a non-null refit and falsifier, and a complete, valid ablation that the full state beats | `test_gate_passing_falsifier_null_router_is_never_chosen_or_filed` (supported-perfect, at-risk always conflict), `test_claim_needs_a_complete_ablation_that_the_full_state_beats` |
| Sol 5 | Major | Per-stratum Wilson used the confusion diagonal, counting failure fallbacks on ask_rule as correct | Valid (confirmed) | Stratum `correct` by `route_correct`, `failures` shown separately; the confusion keeps fallback destinations | `test_per_stratum_correct_excludes_failure_fallbacks` |
| Sol 4 | Major | "Complete" meant complete against the unit's own plan; the queue checked only counts | Valid | `expected_jobs` rebuilds the exact (key, variant, order) set from the mapping and S40 subset; select and the queue refuse missing reversed jobs and same-count wrong-variant jobs, and a plan differing from the prepared one | `test_screen_without_reversed_jobs_is_incomplete`, `test_same_count_wrong_variant_jobs_are_incomplete`, `test_run_queue_router.test_same_count_wrong_variant_screen_is_never_complete` |
| Sol 3 | Major | Setup (hashes, states) used fixed timeouts, one gate before all hashes; CHECK_ONLY skipped the absolute deadline | Valid | `setup_step` before every artefact and step (queue liveness, absolute cutoff, window gate); `bounded` caps each check container by the time left; CHECK_ONLY keeps the absolute-deadline check | `test_run_decision_router`: `test_a_cutoff_crossed_mid_hashing_stops_before_the_next_artefact`, `test_setup_steps_are_bounded_by_the_time_left`, `test_check_only_keeps_the_absolute_deadline` (fake clock) |
| Son m5 | Minor | plan.json written only after the loop; narrow exception handling | Valid | plan.json written before the first decision; any other exception is a `driver_error` failure and the worker is discarded; a unit without a summary is reported incomplete, not a traceback | `test_plan_written_before_the_first_decision`, `test_unexpected_exception_is_a_failed_decision_and_the_unit_continues` |
| Son m6 | Minor | Eager restart after the last failed retry burned the cap | Valid | Restart lazily before the next decision | `test_no_restart_after_the_final_attempt_when_nothing_remains` |
| Sol 7 | Minor | Worker RSS discarded | Valid | `worker_peak_rss_kib` per attempt and decision; summary max with its scope (worker process, not container) | `test_worker_rss_is_kept_per_attempt_and_in_the_summary` |
| Son m4 | Minor | Dev case count returned 0 silently | Valid | `unit_decisions(pack)` raises on a missing or empty pack; sizes from `--pack` | `test_unit_sizes_refuse_a_missing_pack` |
| Son m7 | Minor | The C0 identity test skipped silently without the CLI; no HttpModel payload pin | Valid | In CI (`CI` set) a missing `MIMIK_FORMULA_EVAL` fails the test; locally it skips. The gate mounts the CLI via the `mimik-formula-target` volume. Payload with no tools argument pinned to master `f9d34e0`'s digest | `test_router_off_ledger_is_byte_identical_to_v3_c0` (CI branch checked with `CI=1`: fails), `test_http_payload_without_router_equals_the_pre_router_harness` (guard, green before and after) |
| Son m8 | Minor | Whole tools/ mounted; dirty tree not refused | Valid (deadline note accepted) | Allowlisted module mounts for the router and states containers; a queued run (`QUEUE_PID`) refuses a dirty tree | `test_router_container_mounts_an_allowlist_of_tools`, `test_a_queued_run_refuses_a_dirty_tree` |
| Sol 6 | Minor | No containment after symlink resolution | Valid | Runner and `decision_models.check_files` require resolved artefacts, the GGUF and the Kev cache under the resolved model root (HF snapshot→blob links inside the root still pass) | `test_an_artefact_resolving_outside_the_model_root_is_refused`, `test_resolved_artefacts_must_stay_under_the_model_root` |
| Son nit | Nit | Duplicate `text_hash` and probability checks | Partly | `text_hash` now has one definition (`router_state_builder`). The probability checks stay separate: the legacy benchmark keeps its 1e-5 tolerance and alphabetical ties, the router its shared 1e-6 and canonical ties (rev. 3.1 A3); merging would change the legacy behaviour | — |

No finding was rejected. Sol's note that the C0 golden test is evidence for the normalised
scripted ledger only, not a general byte-identity proof for live execution, is accepted and
recorded; the payload pin adds the live request body for the router-off path.

## Verification

- Red runs: `test_router_review_fixes.py` 19 of 20 failed at `95cc971` (the payload pin passed
  as a guard); the 7 new runner tests failed against the `95cc971` runner; the queue job-set
  test failed against the `95cc971` queue.
- Gate in `mimik-toolloop-py:20261015` (`--cpus 2`, `--network none`, `MIMIK_FORMULA_EVAL`
  mounted): ruff check and format clean; **649 tests, 11 skipped, 1 error** (the
  environmental `test_package` `cargo package` without network, as on the unchanged branch).
  ShellCheck clean on `run_decision_router.sh`.
- Not run: inference, the real-docker smoke (the running CPU queue owns the container names).

## Blockers and handoff

- Third reviewer pending (main session).
- The fixed commit needs the re-review's confirmation before router results drive decisions.
