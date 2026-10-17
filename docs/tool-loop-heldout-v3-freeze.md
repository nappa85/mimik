# Tool-loop v3 blind held-out set: freeze record (2026-10-16)

**Status: frozen.** This file holds no case content. The case file lives **outside the
repository** so that development agents cannot read it by accident:

| Location | Notes |
|---|---|
| `/media/data/mimik/blind-heldout/frozen/cases-heldout.json` | read-only |
| `target/heldout-v3-backup/cases-heldout.json` | backup copy (ignored path) |

**sha256 `6f11af05f27347ea31a50996b0e6b6eea4ccbfa3254a085cd7699d4d8ec68974`**

Only the v3 verdict arms (plan §11: A0, and any arm pre-registered before A0's first episode)
may read it. **Hash gate (plan §11 item 8), implemented 2026-10-17:** the pin — this sha256,
165 cases and the outcome counts below — lives in one committed file,
`tools/heldout-v3-pin.json`. The harness (`--heldout-cases`, default the frozen path above),
the runner (`HELDOUT_CASES`, mounted read-only) and the queue refuse a held-out run unless the
case file's sha256 equals the pin; the file is hashed before it is parsed, and the loaded
counts must equal the pinned ones. A held-out ledger header records `heldout_cases_sha256`.

Hardening after review (2026-10-17, `docs/reviews/2026-10-17-heldout-gate.md`):
- **Code anchor:** the pin JSON must name exactly `HELDOUT_FROZEN_SHA256` in
  `tools/tool_loop_benchmark.py`; editing the JSON alone refuses every held-out run. There is
  no CLI or environment override of the pin.
- **One read:** harness, queue and `--write-case-subset` read the file once, compare those
  bytes with the pin and parse the same bytes; nothing parses unverified bytes.
- **Ledgers:** held-out headers record `heldout_cases_sha256` and `heldout_pin_sha256`, rows
  carry `heldout_cases_sha256`; `load_ledgers`, merges and `bar_verdict` refuse anything that
  is not the frozen hash. `pack_hashes["cases-heldout.json"]` in a header is the pack's empty
  placeholder, never the run's source; the source is `heldout_cases_sha256`.
- **Subsets:** held-out arms may use `cases` or `cases_file` (the 100-case prefix arm); a
  subset's `source_cases_sha256` must be the pin.
- **Gold:** `--validate-pack --split heldout` refuses without `--allow-heldout-gold-replay`, and
  with it prints counts and pass/fail only. Under the blind protocol the main session never
  passes that flag.

## Counts

| | Count |
|---|---|
| Cases | **165**: none dropped, so no trim to 150 |
| Outcomes | proposal 49, answer 38, conflict 27, ask_rule 15, ask_fact 15, stale 9, refuse 12 |
| At-risk / supported | **78** / 87 |
| Languages | en 103, it 62 |
| Characters | wren-ash 55, cass-mere 45, bram-holt 43, dara-vell 22 (cap 22) |
| Injection tags | none 120, passive/item_name 22, passive/notes 11, requested/notes 12 |
| Multi-effect proposals | 19 |
| Single-boolean answers | 1 of 38 |
| Stale distractors | 5 |

The verdict denominators are therefore 165 cases, and 78 at-risk cases, per seed. Plan §11
item 1 three-valued verdict: with 0/78 at-risk false support, the Wilson 95% upper bound is
≈4.7%, so this set alone cannot *statistically confirm* the 2% bar; it can only refute it or
meet it empirically.

## Process

1. **Pack:** Lantern Reach revision 3, merged `34bbd46`, reviewed
   (`docs/reviews/2026-10-16-tool-loop-pack-rev3.md`). Snapshot taken at master `cef5565`.
2. **Writer brief** (extractor agent): brief sha256 `df481d7c…f98`, with a hash manifest of the
   snapshot. A self-check found 0 dev ids, 0 shared 6- or 8-token sequences and 0 dev requests
   in the brief.
3. **Blind writer** (fresh agent, saw only the brief and snapshot): wrote 165 cases. Its own
   validation was clean.
4. **Mechanical checks** (main session, counts and ids only):
   - `--validate-pack` with the candidate as held-out: replay 165/165; notes check 220 claims,
     0 mismatches, 0 unresolved, 0 violations; oil check 5 cases, 0 mismatches;
   - longest prompt about 6.3k tokens;
   - overlap check against dev (token Jaccard ≥ 0.5, difflib ≥ 0.8, identical triple): 8
     flagged at first. The writer rewrote them by id without being told the match. Second
     pass: **0 flagged**.
   - Italian was raised to 62, so the 45 minimum survives any 15 drops.
5. **Blind gold review** (fresh Sonnet 5.5 agent, same isolation): **165 OK, 0 FIX, 0
   AMBIGUOUS, 0 drops**. It ruled on the writer's 10 judgement calls (h3-141, -036, -129, -014,
   -102, -015, -058, -018, -068, -119) and noted low residual risk on h3-129, h3-160 and
   h3-145.

## Limitations and exposure

- **Gold quality:** agent-reviewed and mechanically checked, **not human-verified**. The user
  may sample cases (plan §11 item 13). One reviewer finding zero issues in 165 cases is
  plausible given the mechanical checks, but it is unconfirmed.
- **Main-session exposure:** the writer's rewrite report described the 8 rewritten cases in one
  sentence each. The main session therefore saw short summaries of h3-026, h3-064, h3-065,
  h3-099, h3-117, h3-138, h3-145 and h3-151, which goes beyond the counts-and-ids rule. The
  main session is not a model under test and writes no prompts aimed at these cases.
- **Coverage gaps:** there are no "apply a current, non-stale pending proposal" cases in either
  split, because the behaviour is undefined in the pack. The open ambiguity classes, and cases
  that need new or removed entries (including the last unit), are excluded by the writer scope.
- **Writer helper scripts** (`work/`) stay with the frozen file outside the repo. They contain
  case-generation code.
