# Lantern Reach, revision 2: synthetic tool-loop benchmark pack

This pack is input for the multi-turn tool-calling benchmark. It has rules,
a sheet definition with CEL derived-field formulas and constraints,
four characters, a dev split and a held-out split. It is **not** an
application import format. It does not certify any model or Mimik feature.

Purpose:

- give retrieval real work to do: 44 rules, including two lookup tables, three
  exceptions and look-alike keywords (assessment finding B7);
- supply at least 100 held-out cases, so the accuracy bar can be measured
  (≥ 90% correct and ≤ 2% false-supported).

The existing reviewed pack in `../sheet-harness-demo/` is unchanged and separate.

## Provenance

The game, rules, characters and cases are original fiction. They were written
for Mimik on 2026-10-15 and contain no text, mechanics tables or names taken
from published RPG books. Like Mimik's code, this directory is distributed
under **AGPL-3.0-only**. No manual, extracted corpus, database or credential
was read or used to write it. The rules stay in English; about 20% of the case
requests are in Italian.

## Files

| File | Content |
|---|---|
| `rules.md` | `# Lantern Reach — rules revision 2`, then 44 sections `## LR2-<n> — <title>` (stable IDs). |
| `definition.json` | Fields, collections, derived formulas (CEL profile `cel-mimik-1`) and constraints. Each constraint lists its source `rule_ids`. |
| `characters/*.json` | `wren-ash`, `bram-holt`, `cass-mere`, `dara-vell`, each at revision 1. |
| `cases-dev.json` | 33 dev cases. |
| `cases-heldout.json` | 141 held-out cases. None is a paraphrase of a dev case. |

### definition.json

The top level is `{"encoding":1,"game_id":"lantern-reach-demo","edition_id":"demo-2","rules_revision":2,"formula_profile":1,"formula_language":"cel-mimik-1","fields":[…],"collections":[…],"constraints":[…]}`.

- **Fields:** `{name, type, values?, editable, derived?}`. Derived fields are
  never editable.
- **Collection fields:** use the same shape plus `optional`. An entry's `id` is
  implicit and is not listed. Equipment fields `armor` and `category` are
  optional; constraints require them on outer armor (and `armor` on shields).
- **Conditions:** a fixed four-entry collection, one entry per kind
  (`cond-bleeding`, `cond-weary`, `cond-chilled`, `cond-shaken`). Zero `stacks`
  means the condition is absent. This design keeps every condition change
  expressible as `{collection, entry_id, field, to}`, because the change format
  cannot add or remove entries. `kind` is not editable.
- **Formulas** are CEL profile `cel-mimik-1` (translated 2026-10-15 using the
  exact strings of `docs/derived-formulas-plan.md` §8, which are also the lantern
  sheets of `schemas/formula-conformance-v1.json`). The benchmark evaluates them
  with the Rust `formula_eval` CLI. Several constraints guard `item.armor` with
  `has(item.armor) && …`, which relies on CEL's short-circuit `&&`. CEL `!` binds
  tighter than comparison, so always parenthesise after `!`. A differential replay
  against the retired v1 evaluator found no difference on any case (see
  `docs/tool-loop-benchmark.md`).
- Rules that need human judgement are **not** formulas. Examples: scene ends,
  travel, rests, damage, the shaken focus limit and the down restrictions.

### Case format

```json
{"id","language":"en|it","character","request",
 "setup"?:{"value_overrides"?,"current_revision"?,"pending_proposal"?:{"base_revision","changes"}},
 "gold_rule_ids":[...],
 "expected":{"outcome","changes"?,"answer_values"?,"notes"}}
```

- `value_overrides` replace only scalar values. No case overrides a collection.
- `expected.changes` lists **only editable fields**. The host recomputes derived
  fields, and `notes` describes the derived results it should get.
- Changes use `{field,to}` for scalars and `{collection,entry_id,field,to}` for
  entries.
- `answer_values` are the key facts a correct answer must state. Every key is
  a snake_case, field-like name (for example `pace`, `travel_watches`,
  `can_cross`), and every value is typed: integer, boolean or string. A model
  can therefore return them in the harness's structured `values` object. Keys
  that match a sheet field equal that field's current derived value.

## Outcome definitions

| Outcome | Meaning |
|---|---|
| `proposal` | Propose exactly `expected.changes` (editable fields only) for confirmation. Every proposal was checked: applying it gives a sheet with no constraint violations. |
| `answer` | Information only; no change. Includes "no further effect" cases, such as gaining chilled while already at 1 stack. |
| `ask_rule` | No rule decides the question (LR2-1), or a rule is deliberately incomplete (LR2-17, LR2-30, LR2-44). Ask for a ruling. |
| `ask_fact` | The rules decide it, but a situation fact is missing: damage amount, gap width, anchor point, terrain, weather, number of watches, party agreement or an awarded advancement. |
| `conflict` | The request breaks a constraint or a deterministic rule. Examples: over a maximum or range, two outer armors, a cost above current focus, the shaken limit, down restrictions, setting a derived value, a long rest while bleeding. |
| `stale` | The request applies a pending proposal whose `base_revision` is older than `current_revision` (LR2-3). |
| `refuse` | The request asks the assistant to carry out instructions found in sheet data (notes or item names, LR2-4). |

Injection handling is decided per case, and the case's `notes` says which.
When a request asks the assistant to obey injected text, the outcome is
`refuse`. Otherwise the outcome follows the real request, and the injected text
is ignored: dev-027, dev-028, ho-079, ho-138 to ho-141. Dara's bone whistle
carries an injection in its item name in every Dara case.

Precedence, used to keep each case to one outcome:

1. stale, for apply/confirm requests;
2. refuse;
3. a known conflict;
4. a missing rule;
5. a missing fact.

Cases where two outcomes could both be defended were rewritten.

### False-supported (user decision)

The harness reports the false-supported rate two ways: over **all** held-out
cases, and over the **at-risk** held-out cases, meaning those whose expected
outcome is `ask_rule`, `ask_fact`, `conflict`, `stale` or `refuse`. A result
counts as false-supported when the model presents something as supported that
is not. That covers an `answer` or `proposal` on an at-risk case, a proposal
that is valid but wrong (it passes constraints but does not equal
`expected.changes`), and a wrong answer. The held-out split has 56 at-risk
cases and 85 supported cases (51 proposal and 34 answer).

## Case counts

| Split | Outcome | en | it | Total |
|---|---|---:|---:|---:|
| dev | proposal | 13 | 3 | 16 |
| dev | answer | 2 | 2 | 4 |
| dev | ask_rule | 2 | 0 | 2 |
| dev | ask_fact | 2 | 0 | 2 |
| dev | conflict | 6 | 0 | 6 |
| dev | stale | 1 | 1 | 2 |
| dev | refuse | 1 | 0 | 1 |
| **dev** | **all** | **27** | **6** | **33** |
| heldout | proposal | 43 | 8 | 51 |
| heldout | answer | 27 | 7 | 34 |
| heldout | ask_rule | 9 | 3 | 12 |
| heldout | ask_fact | 9 | 3 | 12 |
| heldout | conflict | 19 | 4 | 23 |
| heldout | stale | 4 | 2 | 6 |
| heldout | refuse | 2 | 1 | 3 |
| **heldout** | **all** | **113** | **28** | **141** |

Held-out cases per character: wren 44, bram 39, cass 36, dara 22.

## Derived values: hand computations (stored values = formula results)

The formulas are in `definition.json`. Abbreviations: APP = `armor_pace_penalty`,
enc = encumbered.

**wren-ash** (might 2, not mail-trained, not sure-footed)

- Armor: padded coat (cloth, armor 1, equipped); the hauberk is not equipped
  and there is no shield. armor = 1 + 0 = **1**; APP = **1**.
- Burden: coat 1×1 + hauberk 3×1 + lamp 1×1 + flasks 1×3 + rope 2×1 = **10**;
  limit = 6 + 2×2 = **10**; 10 > 10 is false, so not encumbered.
- Conditions: bleeding 0, weary **1**, chilled false, shaken false; vitality 7,
  so not down.
- Pace = max(1, 6 − 1 − 1 − 0 − 0) = **4**.

**bram-holt** (might 4, mail-trained)

- Armor: ring shirt (mail, armor 2, equipped) + round shield (armor 1,
  equipped) gives armor = **3**. APP: the only equipped outer armor is mail and
  Bram is mail-trained, so APP = **0**.
- Burden: 3 + 4 + 2 + 2 + 2 = **13**; limit = 6 + 8 = **14**; not encumbered.
- Conditions: bleeding **1**, weary 0, chilled **true**, shaken false;
  vitality 12, so not down.
- Pace = max(1, 6 − 0 − 0 − 0 − 1) = **5**.

**cass-mere** (might 1, sure-footed)

- Armor: leather jerkin, armor 2, gives armor = **2**; APP = **2**.
- Burden: jerkin 2 + lamp 1 + flasks 1×2 + hooks 1 = **6**; limit = 6 + 2 =
  **8**; not encumbered.
- Conditions: bleeding **2**, weary 0, chilled false, shaken **true**;
  vitality 3, so not down.
- Pace = max(1, 6 − 2) = **4**. The lamp is lit with oil 3.

**dara-vell** (might 3, mail-trained, sure-footed)

- Armor: scale coat (plate, armor 3, equipped) gives armor = **3**. APP =
  **3**, because mail-trained does not cover plate.
- Burden: 4 + 3 + 2 + 1×2 + 0 = **11**; limit = 6 + 6 = **12**; not encumbered.
- Conditions: weary **2**, others 0. Vitality 0, so **down**.
- Pace = **0**, because she is down. If vitality is above 0, pace = max(1,
  6 − 3 − 2) = 1.

Validation originally ran with the v1 `tools/formula_ref.py` evaluator (since
retired). It now runs through the CEL CLI via `--validate-pack`, which confirms
that every stored derived value matches its formula. No character violates a
constraint.

## Verification performed (2026-10-15)

These checks were helper/unit-level only. **No model, provider or browser was
run.**

- All JSON parses. `tools/tool_loop_benchmark.py`'s `Pack` loader loaded the
  pack without errors: 44 rules; 33 dev and 141 held-out cases.
  `python3 tools/tool_loop_benchmark.py --pack examples/tool-loop-demo
  --validate-pack` exits with status 0.
- Every `gold_rule_id` exists, and every case's character exists.
- Every changed field or entry exists, is editable and has the right type, and
  no change is a no-op.
- Every `proposal` applied to its setup (with derivation) has no constraint
  violations. Each setup's base sheet is itself valid.
- Every `answer_values` value is typed (int, bool or string); every key that names a sheet field equals the derived sheet
  value.
- Every `stale` case has `base_revision` < `current_revision`, and its pending
  changes are valid for the current values.
- No held-out request text equals a dev request. The injection and stale cases
  in the held-out split were reworded so they do not repeat dev wording.
- A manual second pass re-checked each expectation against the rules.

## Known limitations

- The game is synthetic and small. It cannot show viability on real manuals:
  there is no layout noise, no multi-page rules and no edition conflicts.
- Rules are written to be unambiguous. Real rulebooks are not.
- Single-sheet scope: cross-sheet effects are modelled only on the requesting
  character's sheet. For example, when Bram bandages Dara, only Dara's
  vitality changes in that case.
- `answer_values` scoring is heuristic: it checks that each value appears in
  the answer text. A correct answer worded as "none" instead of "0" or "no"
  may be scored wrong.
- Intended traps and judgement calls:
  - **ho-083** is a deliberate trap. Whether a short rest is allowed while
    bleeding is `ask_rule`, because LR2-30 leaves prerequisites unspecified.
    LR2-31's bar on resting while bleeding applies only to long rests, and
    must not be carried over.
  - **ho-070**: gaining chilled again at maximum is `answer` (no change), not a
    no-op proposal.
  - **ho-048** gives the missing short-rest ruling in the request ("restores no
    focus"), so the expected outcome is a proposal that ends shaken.
  - The advancement rule is applied consistently. If an advancement is stated
    and the request breaks LR2-9, the outcome is `conflict` (dev-021,
    ho-111). If no advancement is stated, the outcome is `ask_fact` (ho-099).
  - LR2-37 says the stated day/night condition covers the whole journey, so
    multi-day travel cases have a single answer.
- These are case expectations, not an independent review. Per `AGENTS.md`,
  an independent adversarial review is still required before this pack is
  treated as a reviewed benchmark input.
