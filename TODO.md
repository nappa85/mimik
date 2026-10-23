# Mimik: work tracker

Current work, next steps, open blockers and pending user questions only. Remove items once
done. Decisions in force: `docs/decisions.md`. Known limits and follow-ups:
`docs/limitations.md`. System reference: `docs/architecture.md` (docs map).

## Current work

- **Assisted-sheet MVP** (`docs/assisted-sheet-plan.md`). Steps 1, 2a, 2b, 2c-1, 2c-2a (the CPU
  runtime at level 1 only, ASSIST-43, `docs/model-runtime.md`) and the Rust 1.99.1 / rig-core
  0.45.0 upgrade are merged into master. 2c-2b is split (user, 2026-10-23): 2c-2b-1, the GPU
  path, is built and live-verified on `assist-2c2b` (the reviews of the last fixes and the merge
  are owed, plan "2c-2b-1"); then
  2c-2b-2, Landlock behind `--assistant-allow-landlock`, on `assist-2c2b2` from master. Level
  3's return is its own later step (plan, "Level 3 behind an owner opt-in").

- **Resume after a reboot:** the CI helper is `target/agent-tools/ci.sh` (`CPUS=2 sh
  target/agent-tools/ci.sh <cmd>`, image `mimik-ci` = 1.99.1, volumes mimik-cargo-cache and
  mimik-catalogue-target). Free-model reviews: never in parallel (`tools/external_review.py`
  locks and skips limited models, `~/.cache/mimik-review/limits.json`).

## Next steps

0. llama.cpp versions (ASSIST-46): (a) a scheduled CI job that tries each new llama.cpp release
   against the test suite (flags, output parsing, grammar conversion, the confinement suite),
   before the first release; (b) the system-llama.cpp mode with distribution packaging
   (compatibility checks instead of a version pin; its own design).
1. 2c-2b-2 Landlock, 2d-1 owner model setting, 2d-2 downloads, 3 bundles and reminders, 4
   suggestions, 5 docs and gate (includes the `docs/release-scope.md` update for schema 22 and the
   assisted scope).
2. Before 2d-1 chooses the default model (ASSIST-L32), a field-reference experiment, decision
   rule written down before running: base the adopted v2 prefix (v2-short), plus one arm that
   adds only the compact-output sentence; about 30 fresh field-reference cases written without
   reusing screen cases; real 1 KiB related-context requests rendered by the Rust builder; an
   idle host at the ASSIST-41 thread count; pass 1 only.
   Alongside it (user 2026-10-23), a prototype-tier Laya experiment (Convai Innovations'
   Apache-2.0 non-generative decision models, 322M/421M): choose the field from the supplied
   aliases on the same fresh cases, and act-versus-ask on the 50 dev cases against Granite's
   false-confident count. Run it in a container with the `laya` package, since stock
   llama-server does not run it, and write the decision rule down before running. Shipping it
   would need its own runtime decision (ASSIST-29/33).
3. Owed measurements: the minimum-host and AppArmor VM checks (ASSIST-L36); GPU-path checks
   at each llama.cpp pin change (`tools/assist_gpu_live.py`); a trace of the GPU path on another
   driver (AMDVLK, Intel, NVIDIA) for ASSIST-L44/L45; the Qwen3.5-9B rev3 S40 re-run.

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

- macOS and Windows model runtime (ASSIST-34): not verifiable from this host.
- **GPU back in service (2026-10-20, after the re-paste):** at 143 W for 5 min with the fan
  on manual 255 and a room fan, the junction plateaus at 77–79 °C (max 83), edge 57 °C, gap
  about 21 °C. On automatic fan it reaches 85 °C in 22 s, so GPU runs need the manual fan
  after every reboot (the queue refuses otherwise). Switch the model tooling back from the
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
