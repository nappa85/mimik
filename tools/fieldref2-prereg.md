# Field-reference experiment 2: prompt arms (decision rule, written before any run)

Prototype tier (AGENTS.md). Follow-up to the field-reference screen of 2026-10-26
(`docs/results.md`, `tools/fieldref-prereg.md`), where Granite 4.2 3B got 1 of 30 right
(ASSIST-L32). Question: is the prompt the cause?

Owner's hypothesis: the model does not need field values, because Mimik computes the
expression. Showing the values leads the model to copy them as literals, mapping a label to an
opaque alias is an extra step where it can go wrong, and the instructions have no example of
`{"ref": "fN"}`.

## Setup, identical for every arm

- **Cases:** the same 30 cases `fr-01` to `fr-30` of `tools/assist-eval/fieldref-cases.json`,
  with the same Rust-rendered requests, expected values and `ref_fields`.
- **Model and sampler:** Granite 4.2 3B Q4_K_M, greedy, `max_tokens` 352, primed, pass 1 only,
  5 threads on cores 0-4, CPU, no GPU, one run per arm. The four arms run in one unattended
  run of `tools/run_fieldref.sh` with `ARMS="control refex novals ids"`.
- **Machine:** weekday evening, after 18:00; the runner refuses 09:00-18:00 and starts no
  request at or after the next weekday 09:00. The runner's idle check uses `LOAD_MAX`. The
  host's load was 1.41 / 1.32 (1 and 5 minutes) just before this was written, from the
  owner's light activity, so `LOAD_MAX=1.5` instead of 1.0 (recorded in `run.log`). Load
  affects latency only, not the replies.

## Arms

- **A, control** (`--contract v2`): the v2-compact prefix as shipped (ASSIST-52). Same as the
  previous screen's compact arm; it is re-run, not reused, so all four arms share one run.
- **B, `v2-refex`:** A plus two field-reference examples in the instructions' example block,
  after "lost half". The user message is unchanged (byte-identical to A's):
  - `note "the cistern was emptied into the lantern"`, with f1 the field Cistern at 6:
    `t + f1` (adds a field);
  - `note "level with the tide mark"`, with f2 the field Tide mark at 12: `f2` (sets the
    target equal to a field).
  The examples use a lighthouse sheet (Lantern fuel, Cistern, Tide mark). The cases use a road
  ledger and the screen cases use a character sheet. The words cistern, lantern, tide, mark,
  level and emptied occur in no case file (a test checks this with whole-word matches), the
  phrasings ("emptied into", "level with") are not those of any fr-* case ("for each", "per",
  "as many as", "equal to", "matches", "set equal", "plus") and the field labels and values
  occur in no case. Each example has one reference only, so the model is not shown how to
  combine references with literals or multiples (fr-01, fr-04, fr-12 and others need that).
- **C, `v2-novals`:** B, plus the context items sent without a `value` key, so the model sees
  alias, id, label and type. **The target keeps its value** (the model needs it for
  `{"ref": "t"}` explanations and for the fraction rules, and it is the one number the note
  can be about). This applies to enum items too. Instruction sentences that talk about values
  change accordingly: "a value of null is unknown" and "never copy their values / with a known
  value" are replaced by "Only the target's value is given: the sheet works out the values of
  the other items itself" and "Only integer items can be referenced; enum items cannot". The
  examples drop their " at 6" / " at 12". Explanations can no longer quote field values; that
  is accepted.
- **D, `v2-ids`:** C, but context items are named by their `id` instead of an alias. A `ref`
  takes the item's id (`{"ref": "crew"}`), `t` stays for the target, and the context items'
  `alias` key is dropped (the target keeps `"alias":"t"`). The `response_format` schema of this
  arm allows exactly `t` and the request's context ids as the `ref` enum. The harness maps ids
  back to aliases before the existing scorer, so the scoring code and the metrics are
  identical; the ledger keeps the raw reply in `content`. The examples use the ids `cistern`
  and `tide`.
- **Transforms are prototype-only.** B changes the instructions text only. C and D rewrite the
  request that the Rust builder rendered, in Python (`transform_fieldref2` in
  `tools/assist_eval.py`); they are **not the Rust builder's output**, and D's ref grammar and
  mapping are not implemented in Rust. A product change would need Rust support, a schema
  change for D and its own review.

## Metrics (as in `tools/fieldref-prereg.md`, denominator 30 per arm)

- *correct_ref*: a shown option has an accepted value AND uses an alias of one of the case's
  `ref_fields`; *false_confident*: an option is shown and no option is correct_ref (a
  coincidence is included and reported separately); *abstained*: no option shown. They sum to
  30. Also output tokens p50 and total latency p50.
- **Diagnostic only, not part of any rule: dropped but right.** Among the abstained cases, the
  ones where a dropped option would have evaluated to an accepted value had its literal
  provenance rule been lifted (for example fr-08's spelled-out "one"); a second count requires
  the option to use a `ref_fields` alias. It shows how much of an arm's abstention is the
  literal rule rather than the model's choice. It changes no verdict.
- **A run is void unless all 30 cases completed in every arm** (the runner exits 1 and the
  whole run is repeated). Schema validity and truncation are reported.
- **Screen test** (applied to the candidate arm, or to A if none): correct_ref at least 24 and
  false_confident at most 3. Passing means only "not rejected on this screen".

## Decision rule

A material difference is 3 cases of 30; smaller ones are noise.

1. **Primary question:** does any of B, C or D beat A by at least 3 on correct_ref without
   worsening false_confident by 3 or more (false_confident at least 3 above A's)? If not, the
   answer is no.
2. **Candidate for a product change:**
   1. Take the arm with the highest correct_ref among the arms that do not worsen
      false_confident by 3 or more relative to A (A is among them).
   2. If several arms are within 2 of the highest correct_ref, prefer the one with the fewest
      false_confident.
   3. If still tied, prefer the simpler change, in the order B, then C, then D (A before all).
   The candidate counts only if it is B, C or D and passes the primary question; if the pick is
   A, or the pick fails the primary question, there is no candidate.
3. **If no arm improves:** the prompt is not the cause, and the next lead is a larger model.
4. A candidate is only a candidate. It needs a Rust implementation (C and D), held-out cases
   and an independent adversarial review before any product decision.

## Limits, stated in advance

- The 30 cases were already seen by us in the previous screen (and two of its failure modes
  motivated this experiment), so this is a **dev comparison, not held out**; the examples were
  written to avoid them, but the arms are not tuned blind.
- One run per arm, one model, one sampler, 30 synthetic cases, no should-abstain cases. The
  smallest difference acted on is 3 cases; with n = 30 an arm at 12/30 has a 95% Wilson
  interval of about 25-59%.
- Literal handling is unchanged: options with spelled-out numbers stay dropped. The
  dropped-but-right count says how much that matters, but is not scored.
- C and D remove information from the prompt; a gain there is not a gain for Rust unless the
  Rust builder and the schema change with it.
