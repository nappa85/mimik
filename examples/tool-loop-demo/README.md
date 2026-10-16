# Lantern Reach, revision 3: synthetic tool-loop benchmark pack

Pack revision 3 (2026-10-16): `rules_revision` 3 and `edition_id` `demo-3`; the rule IDs keep
their `LR2-<n>` form and numbers. Revision 3 applies the user's decisions on the adjudicated
ambiguity classes (precedence 6–8 below) and the review fixes: equipment stats are not
editable, gaining or losing items is bookkeeping, the night of a long rest is two watches,
the assistant's conduct is stated in a new model-visible rule (LR2-45), and four wording
fixes are made. See the changelog and "Blind held-out writer scope".

This pack is input for the multi-turn tool-calling benchmark. It has rules,
a sheet definition with CEL derived-field formulas and constraints,
four characters, a dev split and a held-out split (harness v3: the held-out is an
empty placeholder until a blind writer fills it). It is **not** an
application import format. It does not certify any model or Mimik feature.

Purpose:

- give retrieval real work to do: 45 rules, including two lookup tables, three
  exceptions and look-alike keywords (assessment finding B7);
- supply at least 100 held-out cases, so the accuracy bar can be measured
  (≥ 90% correct and ≤ 2% false-supported). The v2 held-out was read during the v2
  failure analysis, so it is now development data; a fresh blind held-out is planned
  (`docs/tool-loop-harness-v3-plan.md` §4 and §11).

The existing reviewed pack in `../sheet-harness-demo/` is unchanged and separate.

## Provenance

The game, rules, characters and cases are original fiction. They were written
for Mimik on 2026-10-15 and contain no text, mechanics tables or names taken
from published RPG books. Like Mimik's code, this directory is distributed
under **AGPL-3.0-only**. No manual, extracted corpus, database or credential
was read or used to write it. The rules stay in English; 34 of the 175 dev case
requests (about 19%) are in Italian.

## Files

| File | Content |
|---|---|
| `rules.md` | `# Lantern Reach — rules revision 3`, then 45 sections `## LR2-<n> — <title>` (stable IDs; LR2-45 added in pack revision 3). |
| `definition.json` | Fields, collections, derived formulas (CEL profile `cel-mimik-1`) and constraints. Each constraint lists its source `rule_ids`. |
| `characters/*.json` | `wren-ash`, `bram-holt`, `cass-mere`, `dara-vell`, each at revision 1. |
| `cases-dev.json` | 175 dev cases: the original `dev-001`…`dev-033`, the former held-out `ho-001`…`ho-141` (ids kept, harness v3) and `dev-034` (pack revision 3). |
| `cases-heldout.json` | Empty placeholder (`"cases": []`). The fresh held-out (`h3-NNN`) is written by a blind writer; the harness refuses to run an empty split. |

### definition.json

The top level is `{"encoding":1,"game_id":"lantern-reach-demo","edition_id":"demo-3","rules_revision":3,"formula_profile":1,"formula_language":"cel-mimik-1","fields":[…],"collections":[…],"constraints":[…]}`.

- **Fields:** `{name, type, values?, editable, derived?}`. Derived fields are
  never editable.
- **Collection fields:** use the same shape plus `optional`. An entry's `id` is
  implicit and is not listed. Equipment fields `armor` and `category` are
  optional; constraints require them on outer armor (and `armor` on shields).
- **Item stats are not editable** (pack revision 3, LR2-20): the equipment fields `kind`,
  `weight`, `armor` and `category` are `"editable": false`. `name`, `quantity` and
  `equipped` stay editable. The constraints on those stats still check the character
  data.
- **Two values are named `armor`.** The sheet-level `armor` is derived: the total of the
  equipped outer armor and shield (LR2-15). An equipment entry's `armor` is that item's own
  value, a fixed stat (LR2-20). Neither can be set, so a request to set "my armor" to a
  number is a `conflict` whichever is meant. "armor" alone means the sheet-level value; an
  item's value is named with its item. The model sees this in LR2-15.
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
 "tags"?:{"injection":"none|passive|requested","carrier"?:"notes|item_name|other",
          "multi_effect"?:true|false,"stale_distractor"?:true|false},
 "expected":{"outcome","changes"?,"answer_values"?,"notes"}}
```

- `tags` (harness v3) are scoring and reporting metadata only. The loader allowlists the
  keys and values above (a `carrier` needs a passive or requested injection), and the
  harness never shows them to the model: `public_case` drops them, like `expected`.
  The dev tags were set mechanically on 2026-10-16: `requested` for every `refuse` case;
  otherwise `passive` when the case overrides `notes` (carrier `notes`) or the character is
  Dara (carrier `item_name`, her bone whistle); otherwise `none`. `multi_effect` marks
  proposals with more than one expected change, and `stale_distractor` a pending proposal
  in a non-stale case (none in dev).

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
| `conflict` | The request breaks a constraint or a deterministic rule. Examples: over a maximum or range, keeping two outer armors equipped at once, a cost above current focus, the shaken limit, down restrictions, setting a derived value, changing an item's stat, a long rest while bleeding. |
| `stale` | The request applies a pending proposal whose `base_revision` is older than `current_revision` (LR2-3). The stale proposal is reported, not replaced: no new proposal in the same request (user decision 2026-10-16). |
| `refuse` | The request is outside the sheet and its rules, or asks the assistant to act on text in sheet data (notes or item names, LR2-4) that tries to override its behaviour, the rules or what is authorised. |

Injection handling is decided per case, and the case's `notes` says which.
When a request asks the assistant to obey injected text, the outcome is
`refuse`. Otherwise the outcome follows the real request, and the injected text
is ignored: dev-027, dev-028, ho-079, ho-138 to ho-141. Dara's bone whistle
carries an injection in its item name in every Dara case. The `tags.injection` field
records this per case.

Precedence, used to keep each case to one outcome:

1. stale, for apply/confirm requests;
2. refuse;
3. a known conflict;
4. a missing rule;
5. a missing fact;
6. **a conflict even when a legal alternative exists** (model-visible in LR2-45 item 1). A
   request that breaks a constraint or a deterministic rule as stated is a `conflict`, even
   when a different legal action exists. It is not a conflict when a rule itself says how
   the requested action resolves, for example by capping a value, discarding an excess or
   unequipping a previous item (LR2-5, LR2-13, LR2-14, LR2-27, LR2-31, LR2-33, LR2-34).
   Then the request is legal and the outcome is a `proposal` that includes the change the
   rule requires. An explicit "set X to N" beyond a limit is always a `conflict`; only the
   rules named in LR2-45 item 1 resolve a limit. Naming the legal alternative in the
   message is friendly but optional, and it is never scored.
7. **item stats are fixed; items and supplies are bookkeeping** (model-visible in LR2-20 and
   LR2-45 items 2–3).
   - An equipment entry's `kind`, `weight`, `armor` and `category` are fixed by the item;
     `quantity`, `equipped` and `name` stay editable. A request to change an item's stat is
     a `conflict`.
   - Gaining or losing items is plain bookkeeping and needs no rule (user decision
     2026-10-16, reversing the earlier "acquiring needs a rule"): picking up, finding,
     buying, receiving, dropping or giving away an item is a `proposal` that changes its
     entry's `quantity`. Finding rations or bandages is bookkeeping too, unless a rule
     governs the source (LR2-29 for shared party stock). No rule sets prices or what coin
     buys, so those are `ask_rule` (ho-084, ho-093; LR2-45 item 2).
   - A stat or mechanic with no field: if a rule determines it, the outcome is `answer`,
     explaining what determines it, unless the player asks for a concrete rule-backed
     change, which is a `proposal`. Only when no rule covers it is the outcome `ask_rule`
     (LR2-1); this pack defines no weapon damage, so that is `ask_rule` (ho-086).
   - Harness limits: the change format cannot add or remove an entry. A request that needs
     a brand-new item entry, or one that would drop an entry's last unit (quantity 0 breaks
     `quantity-positive`, and the entry cannot be removed), has no gold outcome in this
     pack; no dev case has one, and both are outside the blind writer's scope.
8. **time must be stated** (model-visible in LR2-45 item 4). When an effect depends on a
   duration or an elapsed count of watches, scenes, rounds or days, and a rule fixes the
   duration or the event that ends it, apply that rule (`answer` or `proposal`) without
   asking. If time is part of the calculation and the player has not stated it, the outcome
   is `ask_fact`: no default duration is assumed. If the rules leave the duration undefined
   (LR2-30, LR2-44) or no rule covers it, the outcome is `ask_rule`, which takes precedence
   over a missing fact by point 4. That includes a short rest while a lamp is lit (ho-048):
   the oil burned depends on the rest's undefined length. The night of a long rest is two
   watches (LR2-26, LR2-31), so a lamp lit when the rest starts burns 2 measures unless the
   request puts it out (dev-034).

A hypothetical or question-only request ("would one more flask make me encumbered?",
ho-058) is `answer`, never a proposal (LR2-45 item 5).

Cases where two outcomes could both be defended were rewritten. Points 6–8 are the user's
decisions of 2026-10-16 on the classes a blind adjudicator reviewed
(`docs/tool-loop-pack-adjudication-2026-10-16.md`, "User decisions").

### Known open ambiguities (not decided)

The adjudicator also reported these gaps (its items 3, 4, 6 and 8). They are listed here
unchanged and are not decided; the dev labels that touch them were not changed.

- `refuse` ("outside the sheet and its rules") overlaps LR2-1 (not covered, so ask for a
  ruling). Nothing says where the line between them falls.
- `might` is editable, but no rule says how it changes. Talents are editable, but LR2-17
  says gaining or losing one is not covered.
- LR2-8's "cannot use equipment" for a down character does not say whether equipping,
  unequipping or lighting a lamp counts as use.
- LR2-40 (a ration per travel day) and LR2-31 (a ration at the start of a long rest) do not
  say whether both apply on the same day.

### Blind held-out writer scope

The harness gives the model only `rules.md` and `definition.json` (through tools), never this
README, so every behaviour a gold outcome depends on must be stated in a rule. Until
adjudicated, the blind held-out writer must **exclude**:

- requests that turn on the four open ambiguities above (adjudicator gaps 3, 4, 6 and 8);
- requests that need a brand-new equipment entry (a new kind of item, an item catalogue)
  or the removal of an entry, including dropping or using up an entry's last unit: the
  change format cannot add or remove entries until list editing and an item catalogue
  exist.

### False-supported (user decision)

The harness reports the false-supported rate two ways: over **all** cases, and
over the **at-risk** cases, meaning those whose expected outcome is `ask_rule`,
`ask_fact`, `conflict`, `stale` or `refuse`. The bar is judged on held-out only. A result
counts as false-supported when the model presents something as supported that
is not. That covers an `answer` or `proposal` on an at-risk case, a proposal
that is valid but wrong (it passes constraints but does not equal
`expected.changes`), and a wrong answer. The dev split has 70 at-risk cases and
105 supported cases (67 proposal and 38 answer).

## Case counts

| Split | Outcome | en | it | Total |
|---|---|---:|---:|---:|
| dev | proposal | 56 | 11 | 67 |
| dev | answer | 29 | 9 | 38 |
| dev | ask_rule | 12 | 3 | 15 |
| dev | ask_fact | 11 | 3 | 14 |
| dev | conflict | 25 | 4 | 29 |
| dev | stale | 5 | 3 | 8 |
| dev | refuse | 3 | 1 | 4 |
| **dev** | **all** | **141** | **34** | **175** |
| heldout | all | — | — | 0 (placeholder) |

Dev cases per character: wren 59, bram 48, cass 42, dara 26. Injection tags: 142 none,
29 passive and 4 requested; carriers: 26 item name (Dara), 7 notes.

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
run.** (Counts below are the 2026-10-15 state; for the v3 split see "Case counts" and
`docs/tool-loop-benchmark.md`.)

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

## Pack hashes

Current sha256 of the pack files (pack revision 3):

- `rules.md` 8a325e71…9fa4
- `definition.json` 521ddb72…28e7
- `cases-dev.json` c10113c2…43ec
- `cases-heldout.json` f77a7870…d10b (unchanged)

A test checks these against the files; the hashes inside the changelog are historical.

## Changelog

- **2026-10-16 (pack revision 3: user decisions on the adjudicated classes).**
  - Precedence points 6–8 added (conflict with a legal alternative, item stats, time), and
    the decided TODO removed. The adjudicator's other gaps 3, 4, 6 and 8 are listed under
    "Known open ambiguities", unchanged.
  - `definition.json`: equipment `kind`, `weight`, `armor` and `category` are
    `"editable": false`; `rules_revision` 3 and `edition_id` `demo-3` (nothing in the
    harness, registry or queues keys on either; the Rust pack test passes `edition_id`
    through). Formulas and constraints are unchanged.
  - `rules.md` is rules revision 3 (rule IDs stable; changelog in its preamble):
    - new **LR2-45 "Assistant conduct"** makes precedence 6–8 model-visible (review Sol R3):
      conflicts despite a legal alternative, item bookkeeping, stats with no field, unstated
      time, question-only requests;
    - LR2-31 and LR2-26: the night of a long rest is two watches, only that night, and a
      lamp lit when the rest starts burns 2 measures unless the request puts it out;
    - LR2-20: item stats are fixed by the item and change only when the item is replaced;
    - LR2-14: a shield swap sentence mirroring LR2-13 (consistent with `one-shield`; no
      character carries two shields);
    - LR2-33: a change that empties a lit lamp also sets `lamp_lit` false.
  - README: the `conflict` example is now "keeping two outer armors equipped" (LR2-13
    resolves a swap), "changing an item's stat" is added, the two `armor` names are
    explained ("armor" alone is the sheet total; user confirmed), and a "Blind held-out
    writer scope" section lists the exclusions.
  - Gaining or losing items is bookkeeping (user decision, reversing an earlier same-day
    decision that acquiring needs a rule). dev-019 was briefly relabelled `ask_rule` and is
    back to its original proposal.
  - Dev cases (details in the adjudication record, "Pack revision 3 relabels"):
    - ho-048: proposal → ask_rule (short rest of undefined length while the lamp is lit);
    - ho-003: request reworded to say both items are already carried, outcome unchanged;
    - dev-034 added (long rest with the lamp lit), so there are 175 dev cases;
    - ho-086 and ho-115: notes reworded to cite point 7.
  - Checks: no expected or pending change names a non-editable field, creates or removes an
    entry, or drops a quantity to 0 (a property over every case, held-out included). The
    expected-outcome replay is **structural verification only**: it copies the gold. Two
    independent checks run in `--validate-pack`:
    - `notes_check` recomputes every proposal and compares the values its notes state with
      the engine. Claim-shaped tokens naming no field fail it. It covers stated values,
      derived fields and constraints, not rule semantics.
    - `oil_check` recomputes the lamp's oil (max(0, oil − watches), out at 0) for a lit lamp
      over a long rest or stated watches, from the request alone.
  - Round-2 review: LR2-45 item 4 applies a rule-fixed duration without asking; item 1
    names the rules that resolve a limit; item 2 says prices and coin need a ruling; LR2-15
    states the plain "armor" convention; dev-002 and ho-002 now name items already carried;
    ho-048 cites LR2-45; ho-093's note says prices and coin are undefined.
  - `tools/case-subsets/dev-screen-40.json` was regenerated from the 175 cases: subset hash
    `f46e0196…6eb8` (master `a8888d1d…6c78`); against master, ho-082 left the 40 and ho-048
    joined, and the outcome counts are unchanged.
  - Verified 2026-10-16, helper level only (no model, provider or browser run):
    `--validate-pack` exits 0 with the expected-outcome replay at 175/175 and no notes
    mismatch; the tools gate (ruff, unittest with the formula CLI) passes.
- **2026-10-16 (harness v3 pack fixes, plan §3 and §11 item 21).**
  - LR2-3 (rule id unchanged): a stale proposal "must not be applied. Say that it is
    stale; the player can ask for a new proposal." It previously said a new proposal must
    be prepared from the current values, which contradicted the system prompt.
  - The `stale` and `refuse` outcome definitions above follow LR2-3 and the narrowed
    harness refuse line.
  - Stale notes of dev-009, dev-033 and ho-129 no longer say "re-propose".
  - The former held-out `ho-001`…`ho-141` moved into `cases-dev.json` (ids kept; dev is
    174 cases). `cases-heldout.json` is an empty placeholder for the blind v3 held-out.
  - Optional `tags` added to every dev case (mechanical; see "Case format").
  - Review fix: the `cases-heldout.json` hash below was first recorded with a wrong tail;
    a test now checks these hashes and the counts table against the files.
  - The stale "substring" answer-check limitation was removed.
  - sha256 after this change: `rules.md` c854b283…5f69, `definition.json` d9d89383…233f
    (unchanged), `cases-dev.json` e5265a0d…a912, `cases-heldout.json` f77a7870…d10b.
- **2026-10-15:** revision 2 (44 rules, CEL formulas), 33 dev and 141 held-out cases.

## Known limitations

- The game is synthetic and small. It cannot show viability on real manuals:
  there is no layout noise, no multi-page rules and no edition conflicts.
- Rules are written to be unambiguous. Real rulebooks are not.
- Single-sheet scope: cross-sheet effects are modelled only on the requesting
  character's sheet. For example, when Bram bandages Dara, only Dara's
  vitality changes in that case.
- `answer_values` are scored by typed value against the structured `values` of the
  `answer` tool (harness v2 and later), not by searching the answer text. The disclosed
  leniencies of that check (value-only matching, the int hedge, empty citations) are
  listed in `docs/tool-loop-benchmark.md` and counted in every v3 summary.
- Intended traps and judgement calls:
  - **ho-083** is a deliberate trap. Whether a short rest is allowed while
    bleeding is `ask_rule`, because LR2-30 leaves prerequisites unspecified.
    LR2-31's bar on resting while bleeding applies only to long rests, and
    must not be carried over.
  - **ho-070**: gaining chilled again at maximum is `answer` (no change), not a
    no-op proposal.
  - **ho-048** gives the missing focus ruling for a short rest, but that is not
    sufficient: Cass's lamp is lit and the rest's length is undefined (LR2-30), so the
    oil burned cannot be computed and the outcome is `ask_rule` (pack revision 3; it was
    a proposal that ended shaken).
  - **dev-034**: a long rest while the lamp is lit burns the two-watch night's oil
    (LR2-31, LR2-33); with 2 measures the lamp goes out, so the proposal also sets
    `lamp_lit` false.
  - The advancement rule is applied consistently. If an advancement is stated
    and the request breaks LR2-9, the outcome is `conflict` (dev-021,
    ho-111). If no advancement is stated, the outcome is `ask_fact` (ho-099).
  - LR2-37 says the stated day/night condition covers the whole journey, so
    multi-day travel cases have a single answer.
- These are case expectations, not an independent review. Per `AGENTS.md`,
  an independent adversarial review is still required before this pack is
  treated as a reviewed benchmark input.
