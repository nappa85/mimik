# Field-reference and Laya experiments: decision rule (written before any run)

Prototype tier (AGENTS.md). Question: can the default model (Granite 4.2 3B Q4_K_M, CPU)
interpret a note that refers to another field (ASSIST-L32), and is Laya a better route?
Nothing here is tuned on these cases; the rules below are fixed before the first run.

## Experiment 1: Granite arms

- **Cases:** the 30 cases `fr-01` to `fr-30` of `tools/assist-eval/fieldref-cases.json`
  (written fresh; sheet, phrases and values unrelated to the screen cases). Every case
  expects one accepted value, reached only through a reference to a related field (the
  related field is in the context). The request is the real Rust builder's at B = 1,024
  bytes (related fields only, byte bound only, tokens unbounded: the longest request is
  under 1 KiB).
- **Arms** (`tools/run_fieldref.sh`, `granite42-3b`, greedy, `max_tokens` 352, primed,
  5 threads on cores 0-4, idle host, pass 1 only, one run per arm):
  *base* = the adopted v2 prefix (`--contract v2`); *compact* = base plus only the sentence
  "Reply with compact single-line JSON: no spaces or line breaks between tokens."
  (`--contract v2-compact`).
- **Metrics per arm, denominator 30:** *correct_ref* = a shown option has the accepted value
  and its expression uses a context alias besides `t`; *false_confident* = an option is
  shown and none has the accepted value; *abstained* = no option shown (empty, ambiguous,
  dropped, invalid or truncated reply); *correct_noref* = accepted value without such an
  alias (coincidence, reported, not counted as correct). The four sum to 30. Also output
  tokens p50 and p95, total latency p50 (from `summarize`). A run with any failed or
  not-run request is void and is repeated whole.
- **Reliable enough for the default model** (judged on the arm chosen below): correct_ref
  at least 24 of 30 AND false_confident at most 3 of 30. (Abstaining is the safe failure;
  a shown wrong option is the harmful one.)
- **If not reliable:** the default model does not reference other fields. Options whose
  expression uses an alias other than `t` are not offered (treated as abstained), the
  assistant tells the player to enter such a value by hand, and ASSIST-L32 is rewritten as
  "field references unsupported by the default model; revisit with a larger model or
  Laya". The 2d-1 default-model choice is then made without field references.
- **Choosing the arm:** compact replaces base only if all hold: correct_ref(compact) >=
  correct_ref(base) - 2, false_confident(compact) <= false_confident(base) + 2, and
  output-token p50 is at least 20% lower. If compact beats base by 3 or more correct_ref
  or by 3 or more fewer false_confident, compact wins regardless of tokens; if base beats
  compact by 3 or more in either measure, base wins. Otherwise base. Differences smaller than 3
  of 30 are noise and are not claimed.
- **Limits stated in advance:** synthetic author-written cases, one sampler, one model, one
  run per arm, 30 cases (a 3-case difference is the smallest acted on). The pass-1 screen
  baseline for comparison is v2-short: 0 of 6 field references correct, 3 false-confident
  (`docs/results.md`, a different sheet and request set: context only, not a control).

## Experiment 2: Laya (zero-shot, CPU, 2 threads, `tools/laya_eval.py`)

Convai Innovations' Laya 0.4.15, the English (421M) and multilingual (322M) checkpoints
from `convaiinnovations/laya` revision `7b928d82`, hash-verified. Run only after the owner
confirms this file has been reviewed. Both checkpoints run both measures; the better
checkpoint per measure is reported (no tuning, so no selection bias beyond that choice).

- **Measure A, field choice, denominator 30:** state = the note and the target label;
  one `choice` question whose options are the request's integer aliases, each described by
  its label and value. Correct = the top choice is a referenced field of the case
  (`ref_fields`). Reported with the expected chance level and the count of a trivial
  baseline (a unique context label appearing as a word of the note).
- **Measure B, act versus ask, 50 dev cases (`cases.json`):** state = note and target;
  one `choice` question, `act` or `ask`. Truth: `ask` for the 10 cases that expect no value
  (`none`), `act` for the 40 others. Reported: false_act (act where truth is ask, of 10) and
  wrong_ask (ask where truth is act, of 40). Granite v2-short on the same 50 (pass 1):
  false-confident 6 of 50 (all wrong values on valued cases: dh-10, fv-06, rf-03, rf-04,
  rf-06, ip-01), shown on 0 of the 10 ask cases, abstained on 7 of the 40 act cases. Laya
  cannot produce a value, so only the ask-case false_act and the act-case wrong_ask compare
  directly; the 6 wrong values have no Laya counterpart.
- **Laya is worth a follow-up (a runtime decision, ASSIST-29/33, or fine-tuning)** if one
  checkpoint has A correct at least 24 of 30 and above the trivial baseline by at least 6,
  AND B false_act at most 1 of 10 AND wrong_ask at most 8 of 40. Otherwise zero-shot Laya is
  dropped for this use (its own card reports 0.36 zero-shot accuracy, so a fail is
  expected). A pass still does not ship Laya: it needs its own review and runtime decision.
