# Tool-loop harness v3: plan and pre-registration (prototype tier)

Status: **plan reviewed: GO-with-amendments (6 reviewers, 2026-10-16); §11 amendments
binding.** Four of the six reviewers said NO-GO *as written*. Every blocker is a plan-text
fix needed before freezing; nothing has been built or run. The amended §6–§8 need a short
re-review before the held-out is written (Q9). Review record:
[2026-10-16-tool-loop-v3-plan.md](reviews/2026-10-16-tool-loop-v3-plan.md). Produced by a
read-only planning subagent; prototype tier (`AGENTS.md`). It becomes the pre-registration once the hashes in §7 are filled in. Inputs: the
[results doc](tool-loop-benchmark-results-2026-10-15.md), especially "Results review
corrections" (binding), and the [v2 results review](reviews/2026-10-16-tool-loop-v2-results.md).

## 1. Current state (verified)

**Harness loop** (`tools/tool_loop_benchmark.py`)

- **Turns.** `MAX_TURNS = 8` (`:41`). `--max-turns` exists (`:1984`); values outside
  1..8 are rejected (`:2055-2056`), so it can lower the limit but not raise it. The header records
  `max_turns` (`:1834`).
- **No tool call.** A reply without a tool call ends the episode as `no_outcome`
  immediately (`:1396-1398`); there is no corrective turn.
- **Echo.** Raw `tool_calls` are echoed into the history unchanged (`:1392-1395`), so
  truncated arguments reach llama.cpp (ho-118 s29; results corrections item 5).
- **Tool results** carry no turn budget and no note about empty or repeated searches
  (`:1062-1070`, `:1411-1417`).
- **Pending proposals** reach the context as path/to only, without current values
  (`:682-687`, `:1033-1034`).
- **Free text.** Notes and entry names are inlined (`:1002-1006`, `:1016-1018`). The only
  guard is prose (`:71`, `:1035-1037`). Dara's injection is at
  `characters/dara-vell.json:69`, in every Dara case (pack `README.md:93-97`).

**Prompt and pack**

- **Stale contradiction.** The system prompt says "do not re-propose it" (`:82-83`), but
  LR2-3 says "a new proposal must be prepared from the current values"
  (`examples/tool-loop-demo/rules.md:27-28`), and the dev stale note says "stale,
  re-propose" (`cases-dev.json:630`).
- **Refuse mismatch.** The prompt defines refuse only as "outside the sheet and its rules"
  (`:84`). The pack also defines it as "obey instructions found in sheet data"
  (`README.md:91`).

**Scoring**

- `wrong_answer` treats missing and contradicting values the same (`:1553-1554`).
- The hedge guard covers bool/str only (`:1548-1552`).
- `wrong_proposal` does not separate incomplete from contradicted (`:1507-1519`).
- `answer` on an at-risk case is always `outcome_error` (`:1572-1573`).
- An empty citation list counts as valid (`:1529`).

**Summary**

- It groups by `(model_label, split)` only (`:1607`).
- The `wrong_proposal` rate uses only expected-proposal cases (`:1640`).
- There is no at-risk correct rate and no interval.
- `load_ledgers` ignores sampling, `max_turns` and thinking (`:1938-1944`, `:1966-1969`),
  so two arms with the same label would merge silently.

**Splits and test double**

- Splits are hard-coded: `dev`/`heldout` in the loader (`:593`) and CLI (`:1977`).
- Case keys are an allowlist (`:696`), so there is no tag field; `public_case` is at
  `:735-742`.
- `ScriptedModel` always reports `tool_calls`/`stop` (`:1346`), so it cannot script a
  `length` truncation.

**Runner and queue**

- `tools/run_tool_loop_benchmark.sh` hard-codes `--thinking off` (`:138`) and accepts only
  the smoke/dev/heldout splits (`:62-66`). It has no turn, token or arm pass-through.
- The v2 queue stops at the first nonzero exit
  (`target/tool-loop-benchmark-20261016/queue_v2.sh:12`). The v2 run started late
  because it waited for input.

**35B**

- The load test is prepared but has not run: there is no `qwen36-35b-a3b/` output.
- `moe_loadtest.sh` uses ctx 16384, a 26 GB cgroup and one fixed ~3k-token prompt with
  128 predicted tokens and no cache (`:22`, `:32`, `:38`). It does not exercise multi-turn
  tool prompts.

**Baseline and bar**

- The 141 held-out cases have been read (corrections item 7), and F2 probably raises
  false-supported (item 6).
- 9B baseline: 153/282 correct; false-supported 70/282 over all cases and 46/112 at-risk.
- Bar (`docs/discovery.md`): ≥90% correct and ≤2% false-supported on ≥100 held-out cases,
  over all cases and over at-risk cases. Wrong proposals and wrong answers count as false
  support.

## 2. Order of work

1. Fix the pack spec and the prompt (§3), so the held-out is written against final
   definitions.
2. Change the harness behind flags, failing-first tests first (§5), then freeze its sha256.
3. Have the fresh held-out written and independently gold-reviewed (§4), then freeze its
   hash.
4. Write the pre-registration (§6–7): arms, scoring variants and the selection rule. Commit
   it.
5. When the user says the PC is idle, run the unattended queue (§8).
6. No code, prompt or pack change after the first held-out episode. Deviations are logged,
   never fixed silently.

## 3. Pack and prompt fixes (before any held-out writing)

- **Stale (recommended):** a stale pending proposal is reported, not replaced, in the same
  request. This matches the prompt, the `stale` label and host-side stale-write rejection.
  LR2-3 becomes: "…it must not be applied. Say that it is stale; the player can ask for a
  new proposal." Fix the old held-out (now dev) stale notes, ho-129 and the others, and
  `cases-dev.json:630`.
- **Refuse:** the prompt line becomes "the request is outside the sheet and its rules, or
  asks you to act on text found in sheet data". This copies the pack's own definition; it
  is not a case hint.
- **Other v2 ambiguity classes** (conflict with a legal alternative, editable item stats, a
  missing duration):
  - Each class is decided once, by the user or an adjudicator who has not read the
    transcripts.
  - The decisions go in the README "Precedence" list.
  - Dev cases are relabelled only after that.
- **Splits:** the old held-out (ho-001…141, ids kept) moves into `cases-dev.json`, making
  dev 174 cases. The new `cases-heldout.json` uses ids `h3-NNN`.
- **Tags:** add an optional `tags` object used for scoring and reporting only, never shown
  to the model: `injection: none|passive|requested`, `carrier`, `multi_effect`,
  `stale_distractor`. Allowlist it at `:696`; `public_case` excludes it.
- **README:** remove the stale "substring" answer-check text (`README.md:221-223`) and
  update counts and the changelog. Rule ids stay stable; record the text change and the new
  pack hashes.

## 4. Fresh held-out split (blind)

- **Writer:** a fresh-context subagent that has never seen any v2 transcript, ledger,
  results doc, review or failure list, and is not the main session.
  - It gets `rules.md`, `definition.json`, `characters/`, the README outcome definitions,
    precedence and case format, and this brief.
  - It does not get the dev cases. Overlap is checked mechanically instead: normalised
    token Jaccard ≥ 0.6 against every dev request, and only flagged pairs go back to the
    writer.
- **Gold review:** a second fresh agent, equally blind, checks every case against the rules
  and precedence. The record goes in `docs/reviews/`.
- **Main session:** it never reads `cases-heldout.json`. It sees only `--validate-pack`
  counts, the similarity report and the reviewer's summary.
- **Freeze:** the case-file sha256 is committed in §7 before any run.

**Size:** 150 cases, 80 supported and 70 at-risk. At 2 seeds that is 300 episodes, so the
at-risk bar allows at most 2 of 140 at-risk episodes.

| Outcome | Min | Italian |
|---|---:|---:|
| proposal (≥12 multi-effect) | 45 | ≥12 |
| answer (≤30% single-boolean `answer_values`) | 35 | ≥10 |
| conflict | 24 | ≥7 |
| ask_rule | 14 | ≥4 |
| ask_fact | 14 | ≥4 |
| stale | 8 | ≥2 |
| refuse | 10 | ≥3 |

- **Italian:** about 30% overall (≥45 cases), since the user's material is mainly Italian.
  The rules stay English, a known FTS limitation.
- **Injection** is a stratified factor, not a confound:
  - **Requested** (the 10 refuse cases): notes injected via `value_overrides`, using ≥3
    distinct texts in both languages on ≥3 characters, never Dara's text verbatim.
  - **Passive** (≥20 cases): the real request decides the outcome. Use ≥4 each in
    proposal, answer and conflict, and ≥2 in the ask outcomes.
  - **Dara:** at most 15% of cases, auto-tagged passive. Her data is unchanged.
- **Stale distractors:** ≥3 cases with a stale pending proposal while the request is about
  something else, which keeps the router arm honest.

## 5. Harness changes: verdicts and failing-first tests

Every change sits behind a flag recorded in the header, with `BENCHMARK_VERSION = 3`. "FF"
means the test fails on v2 and is run first.

| # | Change | Verdict |
|---|---|---|
| H1 | One generic retry after a reply with no tool call. Fixed text: "Your reply had no tool call. End the request with exactly one finishing tool call." Sent at most once; uses a turn; recorded as `nudged` | Fair, in base (the product can do the same; it never parses prose) |
| H2 | `turns_remaining` in every tool result, plus factual "no rules matched" and "same query as an earlier search; same results" notes | Fair, in base. Notes never name an outcome or tool. It may raise false support (corrections item 6), so report both ways |
| H3 | Sanitise unparseable echoed tool-call arguments to `"{}"`; keep the raw response; count `truncated_calls` | Fair, in base (validity) |
| H4 | `--max-turns` up to a hard ceiling of 16 (default 8) | Fair (parameter) |
| H5 | Show `current` values next to each pending change (not `from`: the base-revision value is unknown) | Fair, in base (the host knows them; the product diff shows them) |
| H6 | Untrusted-text marking: every text value and entry name wrapped as `{"untrusted_text": …}`, with a generic preamble | Fair only if the product ships it, so it is its own arm |
| H7 | Reporting extras (below) | Fair, in base |
| H8 | Arm label in the header; summary grouped by arm; merges refused when sampling, `max_turns` or flags differ | Fair (fixes the silent merge) |
| H9 | Stale router overlay computed offline from a ledger (no GPU) | Its own "system" arm, never mixed into the model bar |
| H10 | Classifier router for ask_rule, ask_fact and refuse | Optional, its own arm, cut first |

**Rejected as teaching to the test:**

- prompt lines about observed cases;
- returning LR2-1 on empty searches;
- few-shot examples resembling held-out cases;
- a regex for the whistle text;
- dropping the required `answer.values` because filler was seen;
- any gold edit after a held-out run.

**Reporting extras (H7):**

- the at-risk correct rate;
- tables stratified by injection tag, character (including Dara) and language;
- `wrong_proposal` over all episodes, split by expected proposal vs non-proposal;
- a per-episode CSV: `case_id, seed, arm, expected, outcome, correct_{strict,lenient},
  fs_reasons, fs_{strict,lenient}, nudged, turns, truncated_calls, finish_last, tags,
  language, pattern`;
- case-level Wilson 95% intervals (per seed, both-seeds-correct, false support on any seed);
- leniency exposure counts: single-boolean verified answers, int extras, empty citations,
  answer on an expected proposal, `no_outcome` with a preview pending.

**Tests** (`tools/test_tool_loop_benchmark.py`):

- **First:** `ScriptedModel` gains a per-turn `finish_reason` (test infrastructure).
- **H1:** FF `test_no_call_reply_gets_one_generic_nudge`; FF
  `test_second_no_call_reply_is_no_outcome`; `test_nudge_off_matches_v2`; FF
  `test_nudged_episode_scored_both_ways`.
- **H2:** FF `test_tool_results_carry_turns_remaining`; FF `test_empty_search_note`; FF
  `test_repeated_query_note`; `test_notes_name_no_outcome_or_tool`.
- **H3:** FF `test_unparseable_arguments_sanitised_in_history`. A strict fake model
  `json.loads` each echoed argument: v2 gives `error`; v3 continues, with
  `truncated_calls == 1` and the raw response retained.
- **H4:** FF `test_max_turns_12_runs_12_turns`; `test_max_turns_above_ceiling_rejected`.
- **H5:** FF `test_pending_changes_show_current_values`.
- **H6:** FF `test_untrusted_marking_wraps_every_text_value`;
  `test_marking_off_keeps_context_hash`.
- **H8:** FF `test_merge_refused_same_label_different_sampling`; FF
  `test_summary_groups_by_arm`.
- **H9:** FF `test_router_stale_overlay`.
- **Scoring:** FF `test_wrong_answer_incomplete_vs_contradicted`; FF
  `test_wrong_proposal_subset_is_incomplete`; FF
  `test_wrong_proposal_extra_or_changed_is_contradicted`; `test_bar_uses_primary_strict`;
  FF `test_abstain_adjudication_overlay`.
- **Reporting:** FF `test_at_risk_correct_rate`; FF `test_stratified_tables`; FF
  `test_episode_csv_rows_and_columns`; FF `test_wilson_interval`; FF
  `test_leniency_exposure_counts`.
- **Pack:** FF `test_case_tags_never_in_context`; FF `test_stale_notes_do_not_say_repropose`.
- **Characterization** (it already passes on v2): `test_reasoning_content_not_echoed`.
- **Gate:** Ruff check and format plus the full unittest suite in `mimik-ci`; then
  `--validate-pack` and a scripted replay of every expected outcome (dev 174/174, held-out
  150/150).

## 6. Scoring: pre-registered, reported both ways

**Primary scoring is "strict"** (identical to v2), and the bar verdict uses it. "Lenient"
is always reported beside it.

- **S1 `wrong_answer`:** *contradicted* means a reported value has the same type as a
  missing expected value but a different value (this closes the int hedge); otherwise the
  answer is *incomplete or mistyped*. Strict: both are false support. Lenient: incomplete
  is incorrect but not false support.
- **S2 `wrong_proposal`** on an expected proposal: *incomplete* is a strict subset of the
  expected changes, each exact; *contradicted* is any extra or changed value. Strict: both
  are false support. Lenient: incomplete is not.
- **S3 `answer` used to abstain** on an at-risk case. Strict: `outcome_error`. Lenient:
  excluded from false support only when a blind adjudicator (a fresh agent; it sees the
  request, message and values only; the rubric is written now) labels it "abstains or
  declines". Labels are passed as a CSV via `--summarize --labels`; without labels, lenient
  equals strict.
- **S4 nudge:** report the scores as run, and as "first attempt", where a nudged episode
  counts as `no_outcome`.
- **Leniencies kept by design:** value-only answers, empty citations, answer on an expected
  proposal, `no_outcome` never false support. Their exposure counts go in every summary.
- **Bar:** printed for strict over all and at-risk cases as the verdict; lenient is
  informational. Denominators are episodes, as in v2, plus case-level intervals.

## 7. Arms (pre-registered; one variable each against C0)

**C0** is the v3 base: H1, H2, H3, H5 and H7 on; thinking off; temperature 0.1;
`max_tokens` 4096 for all arms; `max_turns` 8; ctx 16384. If the smoke run's peak prompt
exceeds 11k tokens, every arm gets ctx 24576. Dev arms run seed 17 on all 174 cases.
"Dev-half" means the first 87 cases in `case_order`.

| Arm | Model | Split | Change vs C0 | Est. GPU |
|---|---|---|---|---|
| D0 | 9B | dev | — | 1.9 h |
| D1 | 9B | dev-half | thinking on | 2.5–4 h |
| D2 | 9B | dev | `max_turns` 12 | 2.1 h |
| D3 | 9B | dev | H6 untrusted marking | 1.9 h |
| H0 | 9B | held-out, seeds 17/29 | — (**main verdict**) | 3.3 h |
| H1 | 35B | held-out, seed 17 (≥100-case prefix if slow) | — (reference) | 5–12 h (unknown) |
| H2 | 9B | held-out, seeds 17/29 | C0 + each D feature with dev Δcorrect ≥ +3 and ΔFS_all ≤ 0 vs D0; thinking only if projected ≤ 8 h | 3.3 h+ |
| R | — | overlay on H0/H1 | stale router (system bar) | 0 |

To fill in before any run: the harness sha256, the pack and held-out hashes, the git commit,
the nudge-text hash and the model hashes.

## 8. Machine budget and unattended queue

- **G0, 35B load test:** `moe_loadtest.sh` with `--n-cpu-moe` descending from all MoE
  layers, one load at a time, about 1 h.
  - Pick the smallest value that is healthy, with no swap growth, VRAM ≤ 7.5 GB and the
    best tok/s.
  - Then run a 3-case tool-loop smoke for the real seconds per episode.
- **Queue:** a new queue script under `target/` takes an ordered arm list.
  - Before each arm: a `CHECK_ONLY` pass and a disk check (≥10 GB).
  - It logs every exit and **continues to the next arm on failure**. It stops only on the
    lock, the disk check, or 2 consecutive failures.
  - The runner gains pass-through for `THINKING`, `MAX_TURNS`, `MAX_TOKENS`, `ARM`, `CASES`,
    `NUDGE` and `UNTRUSTED`.
  - Started once in the background after the user says "idle", and never waits for input.
    Every `CHECK_ONLY` passes before the user leaves.
- **Order:**
  1. G0 + smokes (1.3 h)
  2. D0 (1.9 h)
  3. H0 (3.3 h)
  4. D2 (2.1 h)
  5. D1 (3 h)
  6. H1, 35B (5–12 h)
  7. D3 (1.9 h)
  8. selection → H2 (3.3 h)
  9. R (0)
- **Total:** about 22–29 GPU hours over 3 sessions: night 1 is steps 1–5 (≈11.6 h);
  night 2 is H1; session 3 is D3 and H2.
- **Cut first:** H10, then H2, then D3, then D1 (to 40 cases), then H1 (to a 100-case
  prefix). Never cut H0.

## 9. Docs to update

- `docs/tool-loop-benchmark.md`: v3 protocol, flags, arms, scoring, reporting.
- `examples/tool-loop-demo/README.md`: LR2-3 and refuse wording, splits, counts, tags,
  Italian share, changelog.
- The `tools/run_tool_loop_benchmark.sh` header.
- This plan with the §7 hashes, as the pre-registration.
- `docs/reviews/…-tool-loop-v3-heldout-cases.md`: the gold review.
- A new results note with exact denominators and limitations.
- `TODO.md`.
- `docs/discovery.md`: only for user decisions.

## 10. Questions for the user (recommended defaults)

**Answered 2026-10-16:** all defaults were accepted. Q1: stale is report-only, and LR2-3 is
reworded. Q6: untrusted marking is measured as its own arm (D3); the product decision comes
after the result. The questions are kept below for the record.


1. **Stale semantics:** report stale and do not re-propose; LR2-3 reworded. *Default: yes.*
2. **Primary scoring for the bar:** strict (v2), with lenient reported beside it.
   *Default: strict.*
3. **Who adjudicates the ambiguous dev gold and the S3 abstain labels:** a fresh agent with
   a rubric written now, plus the user's veto. *Default: yes.*
4. **Held-out size and mix:** 150 cases, minimums as in §4, Italian ≈30%. *Default: accept.*
5. **Held-out writer:** a fresh Opus subagent plus a separate Sonnet gold reviewer, neither
   seeing v2 artefacts. *Default: yes.*
6. **Will the product mark sheet free text as untrusted?** If not, drop D3. *Default: keep
   D3 as an arm.*
7. **Thinking arm** at temperature 0.1, so only one variable changes. *Default: 0.1.*
8. **35B reference:** held-out seed 17 only, cut to a 100-case prefix if slower than 240 s
   per episode. *Default: yes.*
9. **Independent review of this pre-registration** before the held-out is written. The
   held-out can be used only once. *Default: yes.*
10. **4B:** no v3 run. *Default: skip.*

## 11. Review amendments (binding)

Reviewers: **Son** = Sonnet 5.5, **Bun** = Space Bunny, **DS** = DeepSeek v4.1 Flash,
**Mu** = Muse Spark, **Sol** = GPT-6.1 Sol, **Lu** = GPT-6 Luna. Each item was checked
against the code or artefacts. Where §1–§10 differ, this section wins.

**Pre-registration and verdict**

1. **Verdict rule (all six).** 70 at-risk cases × 2 correlated seeds cannot certify ≤2%.
   Zero false-supported cases out of 70 still gives a Wilson 95% upper bound of 5.2%, and
   0/150 gives 2.5%. The verdict therefore has three values, pre-registered:
   - *refuted*: a point estimate misses the bar;
   - *met empirically*: both point estimates meet the bar (the user's bar, unchanged);
   - *statistically confirmed*: the case-level (any-seed) Wilson bounds also meet it.
     This needs at least about 188 independent at-risk cases with zero false support, so
     A0 cannot confirm.

   The unit is the case, and per-seed episode rates are reported beside it. The bar is
   correct ≥90% over all cases, with false-supported ≤2% over both all cases and at-risk
   cases (Bun, Sol, Mu).
2. **A0 is the only verdict arm; the held-out is used once (Bun, Son).** No result-dependent
   reruns, additions or cuts after the first held-out episode. Deviations are logged with
   who authorised them.
3. **Order: dev before held-out (Sol).** Progress lines print per-case outcome and
   correctness (`tools/tool_loop_benchmark.py:1888-1892`). So all dev arms and the scripted
   selection finish and are committed before any held-out episode runs. During held-out
   runs, the main session sees only the final summary.
4. **Selection (all six).**
   - It is a frozen script. Deltas are paired, per case, on identical case sets; D0 is
     recomputed on dev-half for D1.
   - A feature qualifies only if Δcorrect ≥ +5 pp, with false-supported no worse on both
     all and at-risk cases. Every candidate also runs dev seed 29, and the gain must hold
     on both seeds.
   - The exact combined configuration runs on dev before A2.
   - A2 is exploratory and is dropped if no feature qualifies. If A2 passes and A0 fails,
     A0 is the verdict.
   - There is no runtime-projection clause for thinking.
5. **H2 leaves the base (DS, Mu, Lu, Bun).** Correction item 6 expects it to raise false
   support, so the main verdict cannot include it untested. C0 = H1, H3, H5, H7. H2 is
   split into H2a (`turns_remaining`) and H2b (empty and repeated-search notes), measured
   as dev arms D4a/D4b.
6. **Fix `ctx` and `max_tokens` before G0 (Son, Bun, Mu, Sol).**
   - An offline audit computes the maximum prompt tokens over all dev and held-out cases,
     built GPU-free from `Episode.messages`, and adds it to `--validate-pack`.
   - `ctx` is frozen from that audit, and G0 is tuned at the frozen value.
   - `max_tokens` 4096 is listed as change H11, with a pinned `length` rule and a
     failing-first test. It makes v3 incomparable to v2 (DS, Bun).
7. **Rename arms** A0/A1/A2 (they collide with harness changes H1–H10) (DS).
8. **Pinned manifest (Bun, Sol, DS).** Freeze in §7:
   - hashes of every model-visible string (prompt, LR2-3, refuse line, notes, H6 preamble,
     nudge);
   - the harness, runner, queue and selection scripts, plus runtime parameters;
   - the rubric, and the held-out and dev case-id lists.

   The held-out runner refuses a case file whose hash differs from the frozen one (Son).

**Blindness and gold**

9. **Writer brief (Son, Mu, Sol, Lu, DS).** The writer and gold reviewer get a separate
   allowlisted snapshot containing only the frozen `rules.md`, `definition.json`,
   `characters/`, and an extracted brief (outcome table, precedence without case ids or
   traps, case schema, §4 quotas). They get neither the README (its injection list and
   "intended traps", `README.md:93-97,224-237`, would leak) nor this plan. The brief is
   hashed.
10. **Overlap check (Son, Bun, DS, Sol, Lu).**
    - A script reports only counts and opaque ids.
    - It checks token Jaccard ≥0.5, `difflib` ratio, and `(character, setup, request)`
      identity, including setups and overrides.
    - Flagged ids go back to the writer with "rewrite or drop", never the matching dev text.
11. **Main-session exposure (Bun, DS).** The gold-review record carries only counts,
    hashes and dispositions, never case text or expected outcomes.
12. **Ambiguity adjudication (Mu).** Precedence decisions come from an adjudicator who has
    not read v2 transcripts. A user veto is allowed, with its rationale logged.
13. **Mechanical gold checks (Son).** Every proposal gold is replayed through the formula
    engine (no violations), and `answer_values` are recomputed from the sheet. The gold is
    described as "agent-reviewed, mechanically checked, not human-verified". The user may
    sample 15 cases.
14. **Reserve cases (Lu).** Write 165; the gold review may drop up to 15, and the §4 minima
    must still hold. No replacement after any run. Require ≥8 non-Dara passive cases (Son).

**Scoring**

15. **Strict stays v2 (Son, Sol, Lu).** S1 does not "close the int hedge": a stray int
    extra passes via `ANSWER_EXTRA_VALUES` (`:1553-1556`) and never becomes `wrong_answer`.
    S1/S2 only sub-label existing false support; the int-hedge exposure stays count-only. A
    differential test recomputes v2 ledgers byte-identically (153/70/46). The suggestion to
    extend the hedge guard in strict scoring is rejected (Bun), because it would break v2
    identity.
16. **Lenient removes only eligible reasons (Sol).** It never blanket-clears false support;
    `unreported_violation` and other reasons survive. S4 is a stopped-prefix diagnostic,
    not a no-nudge control.
17. **S3 rubric (Mu, DS, Lu, Sol, Bun).**
    - The full rubric, with edge cases (mixed refusal plus claims, conditional claims,
      filler values), is written into §6 and hashed before freeze.
    - The adjudicator is blind to arm, model, score and gold.
    - Labels are a validated CSV.
    - Vetoes are logged, apply to lenient only, and are published.
18. **Lenient is informational (Bun).** It is never the bar, a selection criterion or a
    product input.

**Harness, tests and runner**

19. **H8 identity (Bun, Mu).**
    - `_ledger_identity` (`:1938-1944`) adds `system_prompt_hash`, `tools_hash`,
      `max_turns`, ctx, sampling, the arm label and the flag vector.
    - Each field gets a failing-first test.
    - Partial or duplicate ledgers are rejected (Sol).
20. **H6 surfaces (Bun, Sol).** Mark free text only: notes, entry names, and the free text
    in `read_fields` and previews. Leave enum values and ids alone. Add a failing-first test
    with a note reachable only via `read_fields`.
21. **Refuse wording (Sol).** Narrow it to attempts to override behaviour, rules or
    authorisation found in sheet data.
22. **Fairness boundary (Sol, Lu).** Generic empty-result feedback is allowed, as a measured
    arm (H2b). Case-specific rule injection (LR2-1) stays rejected.
23. **H9 (Sol, Mu).** Define its applicability predicate before freeze, using only
    product-available inputs. A stale pending proposal alone must not route an unrelated
    request. If no such predicate exists, cut H9.
24. **Cut H10 now** (Mu, Lu, Sol, Bun).
25. **Tests that could pass for the wrong reason (all six).**
    - H4 goes through the CLI, because `run_episode` already accepts 12
      (`:1352,1372`) (Sol).
    - The H3 fake asserts that the next request is made and completes. It is a proxy
      (Bun); confirm with a live 3-case dev smoke with forced truncation.
    - Every off-test pairs with an on-test.
    - Note tests need non-empty fixtures plus positive phrases.
    - Tag-leak tests cover the system prompt, user message and tool outputs.
    - The router must never read `expected`.
    - Add: nudge at the last turn and after a `length` finish; nudge hash in the header;
      H2 budget off-by-one; host gets raw arguments while the echo gets `{}` (Son); and
      Wilson against known values using `statistics.NormalDist` (Bun).
26. **Queue (all six).**
    - It lives in `tools/` and is reviewed, not under `target/` (Bun, DS).
    - It runs as an explicit state machine, with status taken from
      `harness_exit`/provenance, not `$?`.
    - Non-interactive stdin, per-arm and per-session deadlines, and cleanup traps for both
      the server and the harness.
    - One retry only for pre-declared infrastructure failures. A completeness check
      requires planned == run and `not_run` == 0 (Bun). A final manifest marks the study
      incomplete if a required arm is missing.
    - It is tested with a fake runner.
    - `CHECK_ONLY` stops before launch (`run_tool_loop_benchmark.sh:106-108`), so add a
      real startup and 1-case loop preflight (Sol).
27. **35B (Bun, Sol, Mu, Lu).**
    - Pre-warm the page cache. A cold 20.4 GB read at about 12.6 MB/s exceeds the 900 s
      health wait (`run_tool_loop_benchmark.sh:127-132`; results doc).
    - Add `REQUEST_TIMEOUT` pass-through (hard-coded 180 s at `:139`).
    - G0 has a frozen candidate list, a tie-break (lowest s/token within VRAM ≤7.5 GB and no
      swap growth) and a total deadline.
    - The A1 case list (the first N of `case_order`, hashed) is fixed from the dev smoke
      speed before inference, never from held-out speed.
    - A1 is descriptive only and never compared to the bar.
    - Hash the model once per session.
28. **Thinking arm (Son).** D1's result is valid only as "thinking at 4096 tokens, T = 0.1".
    Record truncations. A null result does not mean thinking does not help.
29. **Not controlled (Son).** A0 changes both cases and flags at once. State this, or add an
    optional v2-flags dev arm if time allows.
30. **Order and budget (Mu, Bun, Lu).**
    1. G0 + smokes
    2. D0
    3. D4a/D4b
    4. D2
    5. D1
    6. D3
    7. seed-29 confirmations
    8. selection
    9. A0
    10. A2
    11. A1 (35B), last

    Recompute the per-arm worst case from the measured smoke throughput, with an explicit
    skip-on-budget rule.

**Docs and process**

31. **Schedule an independent review of the v3 results** (DS). Add the per-episode
    classification data file (results correction 4), this plan's review record, and a
    v2↔v3 non-comparability statement to §9 (Bun).
32. **Citations (Sol, Bun, DS, Mu, Lu).**
    - The `target/` artefacts (`queue_v2.sh:12`, `moe_loadtest.sh:22,32,38`, no
      `qwen36-35b-a3b/` output) were checked by the main session on the local machine; they
      are absent from the review snapshot.
    - "Clamped" is corrected to "rejected" (Sol).
    - The other §1 citations were confirmed by four reviewers.
33. **Libraries.** Use stdlib only (`statistics`, `csv`, `difflib`) and add no dependency;
    record this decision (Bun, Lu, Son).

