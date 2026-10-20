# Mimik: work tracker

Current work, next steps, open blockers and pending user questions only. Remove items once
done. Decisions in force: `docs/decisions.md`. Known limits and follow-ups:
`docs/limitations.md`. System reference: `docs/architecture.md` (docs map).

## Current work

- **Assisted-sheet MVP** (`docs/assisted-sheet-plan.md`). Step 1 (slices 1.1–1.7) and step
  2a (sheet module, strict CSP, D-AA) are merged into master. Next: 2b.

## Next steps

1. Before 2b starts: a short independent confirm of the round-3 plan fixes (`5b1bf23`).
   Started 2026-10-20 (Sol, Sonnet, third chain); if no verdict is recorded here, rerun it
   (prompt: "confirm each round-3 finding in the 5b1bf23 commit message is resolved in
   §1.4"). Then 2b crate and runtime selection (STROT via pyo3 or a Rust port among the
   candidates, judged on results).
2. 2c interpretation, 3 bundles and reminders, 4 suggestions, 5 docs and gate (includes the
   `docs/release-scope.md` update for schema 20 and the assisted scope).

## Blockers

- **GPU out of service** (cooling fault, 2026-10-16): re-paste it, then rerun the 20 s
  diagnostic (aim for a junction 15–25 °C above the edge). Until then model runs are CPU-only
  (`device: cpu`, no `/dev/dri`).

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
- Independent review of the CPU S40 screen results before they drive further decisions.
- Harness v3: re-review the pre-registered scoring variants S1–S4/lenient, the selection
  script and the manifest gate, then build them (`docs/evaluation-tooling.md`). H3 live smoke
  (needs inference).
- After the GPU repair: G0 load test of the 35B, S40 screens of the 8B dense candidates, then
  the v3 dev arms and the held-out verdict run. Before any thinking arm, verify thinking
  support of Gemma 4 E4B, Ministral 3 8B and Llama 3.1 8B against their templates.
