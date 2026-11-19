# Mimik: work tracker

Current work, next steps, open blockers and pending user questions only. Remove items once
done. Decisions in force: `docs/decisions.md`. Known limits and follow-ups:
`docs/limitations.md`. System reference: `docs/architecture.md` (docs map).

## Current work

- **Assisted-sheet MVP** (`docs/assisted-sheet-plan.md`). Steps 1, 2a, 2b, 2c-1, 2c-2a (the CPU
  runtime at level 1 only, ASSIST-43, `docs/model-runtime.md`) and the Rust 1.99.1 / rig-core
  0.45.0 upgrade are merged into master, and so is 2c-2b-1 (the GPU path, live-verified on the
  RX 6650 XT). 2d-1 (the owner model setting with local models), ASSIST-53 and ASSIST-55
  (Qwen3.5-9B default on the GPU) are merged; ASSIST-56 (prompt format D: context by ID, no
  values) is merged too, and so is 2d-2 (model downloads, `docs/model-downloads.md`). There is
  no level 2 (ASSIST-48). Owed for 2d-2: live checks 4 (browser, Downloads panel) and 5 (certificate
  outside the system store, clean machine) (`docs/assisted-sheet-plan.md`, "2d-2"). Checks 1-3
  ran (`docs/results.md`). Steps 3a (catalogue member, linking on view), 3b (bundles), 3c-0's CPU part
  (prompt cache), 3c-1 (reminders with item names), 3c-1b (the cache on the GPU path) and 3c-1c
  (reminders that fit) are merged; next: their live checks (plan), then 3c-2 (catalogue entry
  text). Level 3's return is its
  own later step (plan, "Level 3 behind an owner opt-in").

- **Real ruleset material for the prompt tests (owner, 2026-10-29):** MÖRK BORG IT PDF pages
  22–39 extracted with Marker and forced OCR (`docs/results.md`, extractor screen), bundle
  `data/extractions/review-ready-dbb400754fb94e87af95cce347903ea8` with two suggestion reports
  (Claude's visual review, 64 suggestions; the text-layer cross-check) and a preview of the
  corrected text (`proposed-reading.md` there). Next: the owner imports it into the live
  database (`mimik import-source`, then `mimik import-suggestions --source ID --report FILE` for
  both reports) and reviews each page in `/admin/materials` (the review screen shows the
  suggestions), then the prompt tests use it. The page 29 footnote on armour grades is in
  no unit (L-SRC-1).

- **Source import plan** (`docs/source-import-plan.md`, reviewed, SRC-11 to SRC-14): steps 1
  (schema) and 2 (review screen, `mimik import-suggestions`, browser checks) merged. Step 3's plan
  (`docs/source-upload-plan.md`, reviewed) is merged; part 3a (the sandboxed MuPDF worker, its
  record protocol, the hostile corpus, availability, notices, DEP-29) is merged. Steps 3b (staging, ledger, jobs, upload route and screens) and 3c (repeated-text exclusion)
  are merged (2026-11-05, host live tests and real-PDF checks passed). Step 4 (route A) has its plan
  (`docs/source-route-a-plan.md` on branch `source-route-a`, 809be8f, stacked on 3c; two review
  rounds, final verdicts approve with changes, applied). Owner decisions 2026-11-05: change the
  renderer pin rather than wrap MuPDF: no crate exposes them safely (mupdf 0.8.0 and main,
  pdfium-render 0.9.5 checked 2026-11-06), so the owner chose a local fork of mupdf-rs
  (`/home/marco/Progetti/mupdf-rs`, branch from v0.8.0) adding safe `TextLine::dir`,
  `TextChar::bidi`/`is_rtl` and struct-block traversal (`TextBlock::structure`, `TextStruct`,
  `StructureType`), vendored into Mimik via `[patch.crates-io]` until the owner's upstream PR
  is merged and released. Fork branch `mimik/text-direction-structure` (1adf801, 5 commits on
  v0.8.0): implemented by a Sonnet subagent test-first, 17 text_page tests pass in the
  container; Opus review approve with changes, confirmation approve with changes (doc only,
  applied). Not covered: RTL text (no RTL font in base14); the fork's clippy -D warnings
  fails only on the pre-existing src/error.rs:20 lint. For the PR the owner rebases onto
  upstream main. Vendoring into Mimik, the boundary-lint allowance and the DEP-29 update
  belong to 4a0; tags decided after the 4a0 preflight; the proposed extraction
  flags; `CLIP` off; Claude drafts the Bare Bones reference from page images only, the owner
  checks every row; the proposed probe-v1 as advice; a failed bar keeps step 4 blocked, with a
  reviewed new version allowed (recorded in the plan). 4a0 in progress: the fork is vendored
  and wired (merged 2026-11-07, DEP-29). Preflight findings so far (local notes
  `/media/data/mimik/route-a/preflight-notes.md`): IT 2025 untagged, Bare Bones EN tagged (88
  tables), so the tag slice stays and its cost estimate is owed; the IT scorer recovered to
  `/media/data/mimik/route-a/score_it_reference.py`. API fit, clipping, encoding and sizes
  settled (merged 2026-11-07, plan 4a0 "Settled"). Next in 4a0: the Bare Bones reference draft
  (owner checks every row), then the freeze of `derive_units_v1`, thresholds, bar hashes and scorer. Follow-ups:
  fuzzing of the record parser and the worker's MuPDF entry points (cargo-fuzz in a container);
  a MuPDF security-advisory watch at each release (DEP-29). Intermittent under 16-thread
  oversubscription (2026-11-07, pass alone): `assistant::download::task::tests::cancel_keeps_the_partial_and_resume_asks_for_the_rest_with_range`
  (`Err(Finishing)`, task_tests.rs:347) and
  `assistant::runtime::launch::tests::a_port_that_cannot_be_bound_is_tried_three_times_then_fails`
  (launch_tests.rs:463), and `assistant::runtime::launch::tests::a_leftover_still_running_blocks_the_start_and_keeps_its_entry`
  (failed once in a full gate beside another test container, 2026-11-10; 3 of 3 alone), and seven
  `assistant::download::task` tests on a slow post-merge run (2026-11-10, 1,360 s suite; the
  5 s `until` poll in task_tests.rs:214 ran out; 52 of 52 pass alone, 3 of 3; `a_blocked_disk_write_never_blocks_cancel_or_the_runtime` failed once in the
  table-shapes gate, 2026-11-12, 3 of 3 alone): find their timing dependence. Meanwhile the prompt tests can use MÖRK BORG Bare Bones EN
  (`~/temp/`, tagged, clean text layer) through the prototype tools.

- **Worker live tests for the Words operation (table shapes, merged 2026-11-12):** the ignored
  `sources::worker::live_tests` (words in unit polygons, rotated page, offset CropBox, UserUnit 2,
  the corpus words case) have not run: containers cannot create the sandbox. Owner to choose: a
  relaxed container (seccomp/apparmor unconfined, SYS_ADMIN, no network) or a host run from the
  main checkout. The real worker did run once on the host (2026-11-12): the owner's re-import of
  the IT source stored word boxes for all 18 pages. Still owed: the browser check of the table
  form and page proposals.

- **Resume after a reboot:** build/test agent worktrees with
  `WT=<worktree> [CPUS=n] sh /home/marco/Progetti/mimik/target/agent-tools/ci.sh <cmd>` (image
  `mimik-ci` = 1.99.1; per-worktree registry copy and target volume, removed after the task);
  full gate: `/media/data/mimik/gate/fullgate.sh` in a container with its own volumes. Check
  `df` before gates. Free-model reviews: never in parallel (the `external-agents` skill locks
  and skips limited models, `~/.local/state/external-agents/limits.json`).

- Hakoniwa audit exception: check new Hakoniwa releases; when one drops postcard's default
  features, remove the RUSTSEC-2023-0089 ignore from `.cargo/audit.toml` (DEP-24).

## Next steps

- **Review screen points (owner):** collect the owner's points on the materials review screen
  (cards, table form, merge, split) and plan them.

- **mupdf-rs release:** PR messense/mupdf-rs#280 merged upstream 2026-11-08 (dd46f0e); its text
  API is identical to the vendored fork. No release yet (latest v0.8.0, checked 2026-11-12). When
  a release ships: pin it from crates.io, drop `vendor/mupdf`, the `[patch.crates-io]` entry and
  the `tools/vendored_fork.py` packaging steps, re-check `mupdf-sys`/MuPDF notices, update DEP-29.

- **Source size limits (SRC-15, owner decisions 2026-11-07):** worker opens the PDF from its file
  (no in-memory copy; PDF bounded by disk only); remove the per-upload totals (64 pages, 128 MiB
  PNGs, 512 MiB text, 20,000 units) and the fixed 2 GiB staging total (use free space minus the
  512 MiB margin); keep per-page/per-request bounds and one upload at a time; CLI import keeps
  its JSON bounds. Plan reviewed and merged (`docs/source-limits-plan.md`, also several exclusion
  boxes per page and a content-addressed PDF store). Next: implement slice by slice, before route
  A's 4c builds on staging.

- **Master view (CAMP-14, ROAD-8):** plan `docs/master-view-plan.md` on branch `master-view`
  (4d465f1; two review rounds, final verdicts approve with changes, applied). Owner decisions
  2026-11-05: GET + `/seen` (auto with JS, one click without); existing sheets are player sheets;
  one shared NPC section; try the htmx SSE extension first under the unchanged strict CSP
  (native EventSource only if that is impossible); 1 live stream per Master (newest tab wins);
  own save on the latest revision counts as seen; departure keeps freezing NPCs in v1; NPCs
  share the 32/2048 sheet limits. Next: record these in the plan, then slice (a).
0. llama.cpp built without OpenSSL (`LLAMA_OPENSSL=OFF`) so the runtime starts on hosts with only
   OpenSSL 4; the official b12110 build needs `libssl.so.3` (this host: `openssl-compat:3`).
   llama.cpp versions (ASSIST-46): (a) a scheduled CI job that tries each new llama.cpp release
   against the test suite (flags, output parsing, grammar conversion, the confinement suite),
   before the first release; (b) the system-llama.cpp mode with distribution packaging
   (compatibility checks instead of a version pin; its own design).
1. 3 bundles and reminders, 4 suggestions, 5 docs and gate (includes the
   `docs/release-scope.md` update for schema 23, the assisted scope and the system CA store
   requirement of the downloads).
2. Owed: a live trial of a second model through `/admin/assistant` (`local:`), CPU first, GPU
   at a weekend (ASSIST-L47). The CPU and GPU start-up with the shipped prefix ran 2026-11-16
   (`docs/results.md`). The general 104-reply screen with D ran
   (`docs/results.md`): Granite 37/8/5, 9B on the GPU 42/4/4. Measured 2026-11-16
   (`docs/results.md`): the prefix is reused across reply schemas and the grammar costs under 30
   ms.
3. Older-ABI Landlock evidence (below ABI 6; the Ubuntu VM has ABI 4 and fails the two
   ABI-6 live checks, as documented).
4. Owed measurements: the minimum-host and AppArmor VM checks (ASSIST-L36); GPU-path checks
   at each llama.cpp pin change (`tools/assist_gpu_live.py`); a trace of the GPU path on another
   driver (AMDVLK, Intel, NVIDIA) for ASSIST-L44/L45; the superseding-job (cancel) timings on CPU and GPU with the fixed
   helper.
5. 2c-2b-1 review leftovers (Minor): `tools/assist_runtime_measure.py` stop-reason race
   between the watchdog and `wait_healthy` (set the reason before killing), and `poll()`
   reaping the leader before `killpg`.

## Blockers

- None.

## Pending user questions

- Remaining LAN and multi-device scope to support.
- Exact editions for D&D, Pathfinder (remaster or not), Vampire and MÖRK BORG.
- When that work starts: print/export priorities, UI and answer languages, acceptable latency,
  situation input, first-release features, games after the POC.

## Other next actions

- Browser smoke CSP coverage (review 2026-11-17, Minor): the signed-in scenario checks for CSP
  violations only on the page each Owner stage ends on (the probe resets per page), and
  `tools/lan_client.py` installs no probe; record violations across navigations (a
  context-level list) and add the probe to the LAN gate.
- Gates must use `--all-features` (the auth-feature suites hid a stale `<nav>` assertion until
  2026-11-17).
- Prompt cache on the GPU path (3c-1b review, Minor): the comment of the ctx-0 assertion in
  `the_memory_check_counts_the_cache_and_the_slots_checkpoints` is wrong; the live test's second
  readiness loop does not notice a server that died. Pending live: RAM headroom on the minimum
  host.
- **Reminders that fit (3c-1c):** live checks and the fake-model browser reminder
  scenario passed 2026-11-19 (`docs/results.md`). Qwen3.5 on the GPU reuses only 1,318 of
  1,805 prefix tokens for an interpretation right after another (about 2.7 s). Review Minor: the progress skip of the reminder prime is never cleared
  by a later update (a CPU-to-GPU retry or reminders turned on during a CPU start show a total
  one too low until the prime step); add `Progress::unskip` in `skip_idle_prime`.
- Bundles (3b review, Minor): time the bundled worst-page fixture (`list_tests.rs`): each bundled
  Undo offer runs `changed_members` (about 32,000 `locate` calls on the worst page); compute once
  per bundle if slow. The module's multi-unit redraw (`settleApplied`: drafted, held and in-flight
  units skipped; the asking unit not among the changes) has no automated test; cover it in 3c's
  browser check, when reminders produce multi-change proposals.
- Catalogues (3a review, Minor): no failure-injection test that a failed `names::replace_rows`
  rolls the whole seal back.

- **Interactive walkthrough of the model tests (owner, 2026-10-26):** show the owner how the
  interpretation screens work, using a few cases (for example from `tools/assist-eval/fieldref-cases.json`):
  1. show the exact request (instructions plus the `<data>` JSON);
  2. let the owner answer as if they were the model and score their answer the way the harness does;
  3. show the real model's reply for the same case side by side, and why it scored the way it did.
- **Field references (owner 2026-10-26: they stay; wrong results are a bug).** Experiments 1-4
  in `docs/results.md`. Live with the ASSIST-53 instructions (experiment 4) no arm passes:
  correct_ref reaches 24-25 but false_confident is 4-8 (cap 3); the errors are misreadings
  (a factor added or dropped, the wrong sign, replace instead of add). Owner 2026-11-05: more
  prompt work (examples of "per each" and of buying versus spending), then re-screen (GPU).
  - Qwen4 (announced, not released on 2026-10-27): screen it when it is out.
  - Engines (ASSIST-54): re-screen when mistral.rs gains Vulkan or vllm.cpp has stable releases.

- A launch refused for owner setup leaves a new, already migrated empty database (and its
  `data/` folder) behind: `create_data_dir` and `Store::open` run before the owner check.
- Start-up order: no regression test that a failing search-index check exits without starting
  the assistant (the assistant now starts last).
- Sources (SRC-10 review, Minor): no test runs `prepare_review.py`'s output for a childless
  `ListGroup` through `Store::import_source`; a shared fixture for both suites would catch the
  two rules drifting apart.
- CSRF defence in depth (Opus, review screen confirmation, Minor): `src/registry_admin.rs`
  (`owner_write`) and `src/accounts/mod.rs` recheck the CSRF admission without binding it to the
  Owner's session and principal, as the material writes now do (`recheck_owner_write`). Not
  exploitable today (both come from one request session).

- macOS and Windows model runtime (ASSIST-34): not verifiable from this host.
- **GPU back in service (2026-10-20, after the re-paste):** at 143 W for 5 min with the fan
  on manual 255 and a room fan, the junction plateaus at 77–79 °C (max 83), edge 57 °C, gap
  about 21 °C. On automatic fan it reaches 85 °C in 22 s, so long GPU runs need the manual fan
  after every reboot (the queue refuses otherwise); the short live GPU tests (`assist_gpu_live.py
  --short`, ASSIST-50) accept the automatic fan with a 75 °C stop. Switch the model tooling back from the
  CPU-only default (`tools/model_registry.py`, `tools/run_tool_loop_benchmark.sh`
  `DEVICE=cpu`). Diagnostic: `LIMIT=90 DUR=300 target/agent-tools/gpu_diag.sh`.
- 2a module test gaps from the last review (Minor): no browser case for the `requestSubmit`
  constraint failure, the `dropped()` wording, or the queue drop itself after an unreadable 409
  (the hold on leaving the field is covered by `browser_smoke.py`) / kept after a 422; `sheet_json::integer` indexes fields directly. No JS linter in CI
  (ASSIST-L17).
- Kept entries lose control characters in single-line inputs (ASSIST-L13): cell inputs, and
  a lone CR in a refused quick-add entry (switch to a textarea on any control character but
  tab).
- Replace capacity caps with safety-only limits (RR-L2).
- Catalogues step 5 for the merged steps 1–3: local-data inventory entries, the D12 CI check
  (no non-synthetic catalogue rows in fixtures or packages), the R15 note in
  `docs/campaigns.md`, release-scope update, and a LAN smoke with the synthetic catalogue.
- Catalogues: router-level test of the 404 paths with a grant or session revoked between the
  two transactions.
- `schemas/formula-conformance-v1.json` `note` still names the deleted
  `docs/derived-formulas-plan.md`. The fixture is hash-locked: fix it only in a reviewed
  content revision (new `FIXTURE_SHA256` in `src/definitions/formula/tests.rs` and
  `tools/test_formula_cli.py`).
- Re-import the MÖRK BORG IT sample into the live database (0 sources at the 2026-10-15
  check; deferred by the user). Verify the local manuals' editions and errata, and identify
  the official blank-sheet assets.
- Optional OCR reviewer: choose the owner's API endpoint and model and verify it with real
  responses.
- Harness v3: re-review the pre-registered scoring variants S1–S4/lenient, the selection
  script and the manifest gate, then build them (`docs/evaluation-tooling.md`). H3 live smoke
  (needs inference).
- GPU S40 screen with the Qwen3.5-9B re-run (`docs/results.md`, 2026-10-30: 9B equals the 35B on
  correct, 22/40 each). Then the v3 dev arms and the held-out verdict run. Before any thinking arm, verify thinking
  support of Gemma 4 E4B, Ministral 3 8B and Llama 3.1 8B against their templates.
