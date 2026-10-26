# Field-reference and Laya experiments: decision rule (written before any run)

Prototype tier (AGENTS.md). Question: can the default model (Granite 4.2 3B Q4_K_M, CPU)
interpret a note that refers to another field (ASSIST-L32), and is Laya a better route?
Nothing here is tuned on these cases; the rules below are fixed before the first run.

## Experiment 1: Granite arms

- **Cases:** the 30 cases `fr-01` to `fr-30` of `tools/assist-eval/fieldref-cases.json`
  (written fresh: a different sheet, field names, values and phrasings; the set does reuse the
  screen's arithmetic patterns: add or subtract a field, a multiple of a field, set equal to a
  field, a field plus a literal). Every case
  expects one accepted value, reached only through a reference to a related field (the
  related field is in the context). The request is the real Rust builder's at B = 1,024
  bytes (related fields only, byte bound only, tokens unbounded: the longest request is
  under 1 KiB).
- **Arms** (`tools/run_fieldref.sh`, `granite42-3b`, greedy, `max_tokens` 352, primed,
  5 threads on cores 0-4, idle host, pass 1 only, one run per arm):
  *base* = the adopted v2 prefix (`--contract v2`); *compact* = base plus only the sentence
  "Reply with compact single-line JSON: no spaces or line breaks between tokens."
  (`--contract v2-compact`).
- **Metrics per arm, denominator 30:** *correct_ref* = a shown option has an accepted value
  AND its expression uses an alias of one of the case's `ref_fields`, resolved through the
  rendered context (fr-09 accepts crew or daily wage); *false_confident* = an option is
  shown and no option is correct_ref, including a wrong-field or literal coincidence that
  reaches the accepted value (reported separately as `coincidence`); *abstained* = no option
  shown (empty, ambiguous, dropped, invalid or truncated reply). The three sum to 30. Cases
  are built so that no single wrong alias reaches an accepted value (t+f, t-f, f; a test
  checks this). Also output tokens p50 and p95, total latency p50 (from `summarize`).
  **A run is void unless all 30 distinct cases completed:** the summary prints INVALID,
  `run_fieldref.sh` exits 1, and the whole run is repeated.
- **No should-abstain cases:** every case has an answer, so abstaining never counts as
  right and the screen cannot show that the model abstains when it should. It measures
  referencing only.
- **Screen test** (applied to the arm chosen below): correct_ref at least 24 of 30 AND
  false_confident at most 3 of 30. Passing means only "not rejected on this screen", not
  "reliable": with n = 30 the 95% Wilson intervals are 24/30 = 80% (63-90%) and 3/30 = 10%
  (3-26%). Failing rejects field references for the default model.
- **If rejected** (superseded by the owner on 2026-10-26: field references stay and the failure is a bug, ASSIST-L32; kept as written for provenance): the default model does not reference other fields. Options whose
  expression uses an alias other than `t` are not offered (treated as abstained), the
  assistant tells the player to enter such a value by hand, and ASSIST-L32 is rewritten as
  "field references unsupported by the default model; revisit with a larger model or
  Laya". The 2d-1 default-model choice is then made without field references.
- **Choosing the arm** (material margin: 3 cases of 30; smaller differences are noise),
  in this order: (1) if base is better than compact by 3 or more in correct_ref, or by 3 or
  more fewer false_confident, base wins; this holds even if compact is better in the other
  measure. (2) Else if compact is better by 3 or more in either measure, compact wins. (3)
  Else compact wins only if its output-token p50 is at least 20% lower; otherwise base. The
  screen test is then applied to the chosen arm only.
- **Limits stated in advance:** synthetic author-written cases, one sampler, one model, one
  run per arm, 30 cases (a 3-case difference is the smallest acted on). The pass-1 screen
  baseline for comparison is v2-short: 0 of 6 field references correct, 3 false-confident
  (`docs/results.md`, a different sheet and request set: context only, not a control).

## Experiment 2: Laya (zero-shot, CPU, 2 threads, `tools/laya_eval.py`)

Convai Innovations' Laya 0.4.15, the English (421M) and multilingual (322M) checkpoints
from `convaiinnovations/laya` revision `7b928d82`, hash-verified against `tools/laya-models.sha256`, image
`mimik-laya` by id. Run after this review, a one-case smoke test per checkpoint first
(including loading the multilingual folder). Both checkpoints run both measures; the better
checkpoint per measure is reported (no tuning, so no selection bias beyond that choice).

- **Measure A, field choice, denominator 30:** state = the note and the target label;
  one `choice` question whose options are the request's integer aliases, each described by
  its label and value. Correct = the top choice is a referenced field of the case
  (`ref_fields`). Reported with the expected chance level (sum of len(ref_fields) / options
  over the cases) and two trivial baselines: *strict* (exactly one context label is a whole
  word of the note) and *overlap* (the field whose label tokens overlap the note's tokens
  most, unique maximum, after lowercasing, stripping a trailing "s" and ignoring the words
  the, a, of, on, my, in, for, to). The rule uses the stronger baseline.
- **Measure B, act versus ask, 50 dev cases (`cases.json`):** state = note and target;
  one `choice` question, `act` or `ask`. `act`: the note states one numeric change to the
  target, possibly through another field, or has several acceptable readings of it (for
  example a loss of half rounded either way); instruction attempts inside it are ignored when
  such a change is present. `ask`: the note is nonsense or unrelated, or the direction or
  amount cannot be determined at all. Truth: `ask` for the 10 cases that expect no value
  (`none`), `act` for the 40 others. Reported: false_act (act where truth is ask, of 10) and
  wrong_ask (ask where truth is act, of 40). Granite v2-short on the same 50 (pass 1):
  false-confident 6 of 50 (all wrong values on valued cases: dh-10, fv-06, rf-03, rf-04,
  rf-06, ip-01), shown on 0 of the 10 ask cases, abstained on 7 of the 40 act cases. Laya
  cannot produce a value, so only the ask-case false_act and the act-case wrong_ask compare
  directly; the 6 wrong values have no Laya counterpart.
- **Laya is worth a follow-up (a runtime decision, ASSIST-29/33, or fine-tuning)** if one
  checkpoint has A correct at least 24 of 30 and above the stronger trivial baseline by at least 6,
  AND B false_act at most 1 of 10 AND wrong_ask at most 8 of 40. Otherwise zero-shot Laya is
  dropped for this use (its own card reports 0.36 zero-shot accuracy, so a fail is
  expected). A pass still does not ship Laya: it needs its own review and runtime decision.
