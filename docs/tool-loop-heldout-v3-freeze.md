# Tool-loop v3 blind held-out set: freeze record

Frozen 2026-10-16. This file holds no case content.

## What is frozen, and where

- Case file, outside the repository so development agents cannot read it by accident:
  `/media/data/mimik/blind-heldout/frozen/cases-heldout.json` (read-only). A backup copy is in
  the ignored path `target/heldout-v3-backup/cases-heldout.json`.
- **sha256 `6f11af05f27347ea31a50996b0e6b6eea4ccbfa3254a085cd7699d4d8ec68974`**
- Source pack: Lantern Reach rules revision 3 (`examples/tool-loop-demo`). The pack's own
  `cases-heldout.json` is an empty placeholder and is never the source of a held-out run.
- The writer's helper scripts stay with the frozen file outside the repository.

## Counts

| | Count |
|---|---|
| Cases | 165 |
| Outcomes | proposal 49, answer 38, conflict 27, ask_rule 15, ask_fact 15, stale 9, refuse 12 |
| At-risk / supported | 78 / 87 |
| Languages | en 103, it 62 |
| Characters | wren-ash 55, cass-mere 45, bram-holt 43, dara-vell 22 |
| Injection tags | none 120, passive/item_name 22, passive/notes 11, requested/notes 12 |
| Multi-effect proposals / single-boolean answers / stale distractors | 19 / 1 of 38 / 5 |

Verdict denominators are 165 cases and 78 at-risk cases per seed. With 0/78 at-risk false
support the Wilson 95% upper bound is about 4.7%, so this set can refute the 2% bar or meet it
empirically, but cannot confirm it statistically.

## Pin and code anchors

- Pin file: `tools/heldout-v3-pin.json` (sha256, 165 cases, outcome counts, default path, and
  the registered verdict case sets `full` = all 165 and `h1-prefix-100` = the first 100 in
  frozen file order, each with its id-set digest).
- Code anchors in `tools/tool_loop_benchmark.py`: `HELDOUT_FROZEN_SHA256` (the case-file hash)
  and `HELDOUT_PIN_SHA256` (the pin file's own hash). `load_heldout_pin` refuses a pin that
  does not match both. There is no CLI or environment override.
- `read_verified_heldout` reads the file once, hashes those bytes against the pin before
  parsing, and checks the counts. The harness (`--heldout-cases`, default the frozen path),
  the runner (`HELDOUT_CASES`, mounted read-only) and the queue (`heldout_cases`) all refuse a
  missing file, a hash mismatch or a count mismatch.
- Held-out ledger headers record `heldout_cases_sha256`, `heldout_pin_sha256` and
  `heldout_case_set`; `load_ledgers`, merges and `bar_verdict` refuse any other hash and an
  unregistered case set. Dry-run and gold-replay ledgers may be loaded, but `bar_verdict`
  refuses them a verdict.
- Tests: `tools/test_heldout_gate.py` (synthetic files and injected pins only).

## Ledger and access policy

- Held-out ledgers contain per-case gold (scoring needs it). They are written mode 0600.
- They are read **only** through `tool_loop_benchmark.py --summarize` aggregate output.
  Never open, read or print held-out ledger rows.
- Never pass `--allow-heldout-gold-replay` (it is required for held-out `--validate-pack`,
  `--dry-run` and `--episodes-csv`).
- Never read the frozen case file. Held-out progress output is always quiet, and held-out
  validation errors are reported as counts only.
- Only verdict arms pre-registered before the first verdict episode may run on it.

## Gold quality and limits

- Written by a blind writer agent from a hashed brief and pack snapshot; mechanically checked
  (expected-outcome replay 165/165, notes check 0 mismatches, overlap check against dev 0
  flagged after rewrites); reviewed by a fresh blind agent (165 OK). Not human-verified.
- The main session saw one-sentence summaries of 8 rewritten cases (h3-026, h3-064, h3-065,
  h3-099, h3-117, h3-138, h3-145, h3-151).
- The hash chain is self-asserted, not tamper-evident: it assumes no untrusted write access
  to results directories.
- Coverage gaps: see `docs/limitations.md` (TL-COV1, TL-COV2).
