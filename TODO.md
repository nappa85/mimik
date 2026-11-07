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
  ran (`docs/results.md`). Next: 3. Level 3's return is its
  own later step (plan, "Level 3 behind an owner opt-in").

- **llama.cpp v0.9.0 runtime pin (b12110)**: branch `llama-v090` (`mimik-wt/llama090`, Opus
  approved). Since the 2026-11-03 OpenSSL 4 upgrade the host needs `dev-libs/openssl-compat:3`
  (installed 2026-11-06) for the official llama-server's `libssl.so.3`. With it, all 31 CPU launch
  live tests pass, and the full container gate passed on the branch (2026-11-06). Left: the
  weekend GPU live tests (`tools/assist_gpu_live.py`), the grammar-reply check, then merge. Follow-up: a runtime built
  without OpenSSL (`LLAMA_OPENSSL=OFF`), so other OpenSSL 4 hosts can run it too.

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
  `/media/data/mimik/route-a/score_it_reference.py`. Next in 4a0: API-fit fixture, clipping
  cases, serialized-size measurement, the Bare Bones reference draft, then the freeze. Follow-ups:
  fuzzing of the record parser and the worker's MuPDF entry points (cargo-fuzz in a container);
  a MuPDF security-advisory watch at each release (DEP-29). Intermittent under 16-thread
  oversubscription (2026-11-07, pass alone): `assistant::download::task::tests::cancel_keeps_the_partial_and_resume_asks_for_the_rest_with_range`
  (`Err(Finishing)`, task_tests.rs:347) and
  `assistant::runtime::launch::tests::a_port_that_cannot_be_bound_is_tried_three_times_then_fails`
  (launch_tests.rs:463): find their timing dependence. Meanwhile the prompt tests can use MÖRK BORG Bare Bones EN
  (`~/temp/`, tagged, clean text layer) through the prototype tools.

- **Resume after a reboot:** the CI helper is `/home/marco/Progetti/mimik/target/agent-tools/ci.sh` (`CPUS=2 sh
  /home/marco/Progetti/mimik/target/agent-tools/ci.sh <cmd>`, always by this absolute path, image `mimik-ci` = 1.99.1, volumes mimik-cargo-cache and
  mimik-catalogue-target). Free-model reviews: never in parallel (the `external-agents` skill locks
  and skips limited models, `~/.local/state/external-agents/limits.json`).

- Hakoniwa audit exception: check new Hakoniwa releases; when one drops postcard's default
  features, remove the RUSTSEC-2023-0089 ignore from `.cargo/audit.toml` (DEP-24).

## Next steps

- **Source size limits review (SRC-15, owner 2026-11-07):** step 3's upload and import limits
  in master (`docs/sources-and-retrieval.md`: 128 MiB PDF, 64 selected pages, 128 MiB of PNGs,
  20,000 units, 32 MiB `source.json`, 8 MiB prepared bundle, per-upload text 512 MiB) are
  whole-upload totals: classify each as security/technical (keep, with its reason, per unit of
  work) or arbitrary (remove, storing and processing per page or batch). Plan, review, then
  implement before route A's 4c builds on them.

- **Master view (CAMP-14, ROAD-8):** plan `docs/master-view-plan.md` on branch `master-view`
  (4d465f1; two review rounds, final verdicts approve with changes, applied). Owner decisions
  2026-11-05: GET + `/seen` (auto with JS, one click without); existing sheets are player sheets;
  one shared NPC section; try the htmx SSE extension first under the unchanged strict CSP
  (native EventSource only if that is impossible); 1 live stream per Master (newest tab wins);
  own save on the latest revision counts as seen; departure keeps freezing NPCs in v1; NPCs
  share the 32/2048 sheet limits. Next: record these in the plan, then slice (a).
0. llama.cpp versions (ASSIST-46): (a) a scheduled CI job that tries each new llama.cpp release
   against the test suite (flags, output parsing, grammar conversion, the confinement suite),
   before the first release; (b) the system-llama.cpp mode with distribution packaging
   (compatibility checks instead of a version pin; its own design).
1. 3 bundles and reminders, 4 suggestions, 5 docs and gate (includes the
   `docs/release-scope.md` update for schema 23, the assisted scope and the system CA store
   requirement of the downloads).
2. Owed live measurement for the shipped prefix (ASSIST-56; ASSIST-L47; CPU, 5 threads, outside
   weekday 09:00-18:00): replay the 104-reply screen (`tools/run_assist_screen.sh`,
   `CONTRACT=v2-ids-53`, Granite 4.2 3B, `MAX_TOKENS=384`) and take the largest valid, untruncated
   `predicted_n`; the default file's `max_tokens` is `min(384, round up to 32 of 1.25 ×
   that)` (`DEFAULT_MAX_TOKENS` in `src/assistant/model.rs`, 352 until then); then re-run the idle
   CPU calibration (`tools/assist_runtime_measure.py`) for the priming and the budgets. Also a
   live trial of a second model through `/admin/assistant` (`local:`), CPU first, GPU at a
   weekend. The general 104-reply screen with D ran (`docs/results.md`): Granite 37/8/5, 9B on the GPU
   42/4/4. Open: `max_tokens` of both default files (largest replies 173 and 178: 224 by the
   2b rule, 256 by the trial rule, 352 now; kept until long ids are measured, L52).
   Live acceptance check before release: prime, a structured request, then a request with a
   different `ref` enum (another context): the fixed prefix's tokens are reused
   (`cached_tokens` covers the prefix), plus the grammar-compilation time per request; and
   the token length of the largest schema-valid reply (3 options, 8 leaves, 64-character ids,
   120-character explanations) with the model's tokenizer against `max_tokens` (ASSIST-L52).
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
  - TwIL-8B (webAI, Granite 4.2 8B dense reasoning model, webAI Non-Commercial 1.0: local
    evaluation only): owner 2026-11-05 asked to try it. Official Q4_K_M balanced at revision
    58d49f0d, SHA-256 17bda82d…faa0e, in `/media/data/mimik/models/twil-8b`; GPU
    screen (104 replies, field refs) with thinking off and on, after 18:00 once the owner
    confirms the fans.
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
- Remove `--dev-owner-play` (AUTH-L10), with its own security review.
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
