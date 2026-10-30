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

- **Real ruleset material for the prompt tests (owner, 2026-10-29):** MÖRK BORG IT PDF pages
  22–39 extracted with Marker and forced OCR (`docs/results.md`, extractor screen), bundle
  `data/extractions/review-ready-dbb400754fb94e87af95cce347903ea8` with two suggestion reports
  (Claude's visual review, 64 suggestions; the text-layer cross-check) and a preview of the
  corrected text (`proposed-reading.md` there). Next: the owner imports it into the live
  database (`mimik import-source`, needs SRC-10) and reviews each unit in
  `/admin/materials`, then the prompt tests use it. The page 29 footnote on armour grades is in
  no unit (L-SRC-1).

- **Resume after a reboot:** the CI helper is `target/agent-tools/ci.sh` (`CPUS=2 sh
  target/agent-tools/ci.sh <cmd>`, image `mimik-ci` = 1.99.1, volumes mimik-cargo-cache and
  mimik-catalogue-target). Free-model reviews: never in parallel (`tools/external_review.py`
  locks and skips limited models, `~/.cache/mimik-review/limits.json`).

- Hakoniwa audit exception: check new Hakoniwa releases; when one drops postcard's default
  features, remove the RUSTSEC-2023-0089 ignore from `.cargo/audit.toml` (DEP-24).

## Next steps

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
   helper; the Qwen3.5-9B rev3 S40 re-run.
5. 2c-2b-1 review leftovers (Minor): `tools/assist_runtime_measure.py` stop-reason race
   between the watchdog and `wait_healthy` (set the reason before killing), and `poll()`
   reaping the leader before `killpg`.

- `browser_smoke.py`'s "A 409 with an unreadable body did not hold the field" check fails
  on the dev host for master's release binary as well as `assist-2c2`'s (2026-10-23, Chromium
  container at 2 CPUs on a loaded host); find whether it is timing (its 500 ms waits) before CI
  relies on it.

## Blockers

- None.

## Pending user questions

- Does the correct-password login rejection seen on 2026-10-14 still happen? Diagnose or close.
- Remaining LAN and multi-device scope to support.
- M4 default Q5 (drafts over 1 MiB are not echoed back), applied without an answer: confirm or
  change.
- Precedence or errata policy between the MÖRK BORG IT 2025 and EN 2019 sources (SRC-9).
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
  (a factor added or dropped, the wrong sign, replace instead of add). Open (owner decision):
  accept the 9B default with the owner's confirmation as the safeguard, change the bar, or try
  further prompt work (for example examples of "per each" and of buying versus spending).
  - Qwen4 (announced, not released on 2026-10-27): screen it when it is out.
  - Engines (ASSIST-54): re-screen when mistral.rs gains Vulkan or vllm.cpp has stable releases.

- A launch refused for owner setup leaves a new, already migrated empty database (and its
  `data/` folder) behind: `create_data_dir` and `Store::open` run before the owner check.
- Start-up order: no regression test that a failing search-index check exits without starting
  the assistant (the assistant now starts last).
- Sources (SRC-10 review, Minor): no test runs `prepare_review.py`'s output for a childless
  `ListGroup` through `Store::import_source`; a shared fixture for both suites would catch the
  two rules drifting apart.
- Review helper: `external_review.py`'s short-output unavailable heuristic can reject a real
  successful verdict that discusses "unavailable" status. Add a regression and narrow the
  classification; direct sanitized Astra follow-up was needed to read the actual verdict.

- macOS and Windows model runtime (ASSIST-34): not verifiable from this host.
- **GPU back in service (2026-10-20, after the re-paste):** at 143 W for 5 min with the fan
  on manual 255 and a room fan, the junction plateaus at 77–79 °C (max 83), edge 57 °C, gap
  about 21 °C. On automatic fan it reaches 85 °C in 22 s, so long GPU runs need the manual fan
  after every reboot (the queue refuses otherwise); the short live GPU tests (`assist_gpu_live.py
  --short`, ASSIST-50) accept the automatic fan with a 75 °C stop. Switch the model tooling back from the
  CPU-only default (`tools/model_registry.py`, `tools/run_tool_loop_benchmark.sh`
  `DEVICE=cpu`). Diagnostic: `LIMIT=90 DUR=300 target/agent-tools/gpu_diag.sh`.
- 2a module test gaps from the last review (Minor): no browser case for the `requestSubmit`
  constraint failure, the `dropped()` wording, or the queue drop after an unreadable 409 /
  kept after a 422; `sheet_json::integer` indexes fields directly. No JS linter in CI
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
- GPU S40 screen reviewed (`docs/results.md`). Before any 35B-versus-9B claim, re-run the
  Qwen3.5-9B on rev3 S40 (GPU, about 10–30 min; evening/weekend, fan manual). Then the v3 dev arms and the held-out verdict run. Before any thinking arm, verify thinking
  support of Gemma 4 E4B, Ministral 3 8B and Llama 3.1 8B against their templates.
