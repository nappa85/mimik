# Tool-loop pack: blind ambiguity adjudication (2026-10-16)

Status: **user decisions recorded 2026-10-16 (see "User decisions" below); applied in pack
revision 3** (see "Pack revision 3 relabels" at the end). No dev outcome changed.

- **Adjudicator:** a fresh subagent given only a copy of the pack: README, rules.md,
  definition.json and characters/. It had no access to the repo, v2 transcripts, results or
  reviews (v3 plan §11 item 12). No case files were in its copy.
- **Source:** the adjudicator's own file was lost when the host rebooted (tmpfs `/tmp`).
  This record reproduces its final report; the decision lines are verbatim.
- **Policy choice:** one sub-case of decision 7 is the user's.

These lines continue the README "Precedence" list as items 6–8.

## 6. Conflict with a legal alternative

> A request that breaks a constraint or a deterministic rule as stated is a `conflict`, even
> when a different legal action exists. The assistant may name the legal alternative in its
> message, but it proposes no changes; the player can ask for that alternative. This is not a
> conflict when a rule itself says how the requested action resolves, for example by capping
> a value, discarding an excess or unequipping a previous item. Then the request is legal, and
> the outcome is a `proposal` that includes the change the rule requires.

- **Grounds:**
  - LR2-3, LR2-2 ("cannot be carried out as asked") and LR2-10 ("cannot be paid in part");
  - the `conflict` and `proposal` definitions, and the `stale` precedent ("reported, not
    replaced");
  - for the rule-resolves-it exception: LR2-5, LR2-13, LR2-27, LR2-31, LR2-33 and LR2-34.
- **Confidence:** high; medium-high on where the exception's line falls.
- **Strongest counter-argument:** proposing the obvious legal fix is more helpful, and the
  player confirms it anyway. Against that, it offers a change the player never asked for, and
  scoring would then depend on which alternative was picked.

## 7. Item stats

> Stats recorded as equipment entry fields (such as `armor`, `category`, `weight`, `quantity`
> or `kind`) are editable sheet data. A request to change one gives a `proposal` when the
> result satisfies every constraint, and a `conflict` when it breaks a constraint or a rule's
> range. Sheet-level values with the same names stay derived (point 3 and LR2-2). A request
> for a stat or mechanic that has no field in the definition and no rule is `ask_rule` (LR2-1).

- **Grounds:**
  - every equipment field in `definition.json` is `"editable": true`;
  - the constraints `outer-armor-range`, `shield-value`, `weight-range`, `quantity-positive`
    and `outer-armor-fields` bound them;
  - LR2-20: an item's mechanics "come from its kind and recorded fields";
  - no rule gates item stats behind an event, the way LR2-9 gates maxima and LR2-17 gates
    talents.
- **Confidence:** high for fields that exist.
- **User policy choice:** the last sentence covers a stat with no field, such as weapon
  damage. The adjudicator proposes `ask_rule`, with low-to-medium confidence. The
  alternatives are `conflict` (the change format cannot express it) and `refuse` (outside the
  sheet).
- **Strongest counter-argument:** in a real game an item's protection is fixed. That would
  need these fields marked `editable: false`, or a gating rule.

## 8. Missing duration

> When an effect depends on a duration or an elapsed count of watches, scenes, rounds or days:
> if a rule fixes the duration or the event that ends it, apply that rule (`answer` or
> `proposal`) without asking. If the rule decides the effect but the elapsed amount is not
> stated, the outcome is `ask_fact`. If the rules deliberately leave the duration unspecified,
> or no rule covers it, the outcome is `ask_rule`, which takes precedence over a missing fact
> by point 4. No default duration may be assumed unless a rule states it.

- **Grounds:**
  - the `ask_fact` definition names "number of watches";
  - LR2-30 and LR2-44 leave the duration out on purpose;
  - LR2-31 ("takes a night"), LR2-22, LR2-24, LR2-25 and LR2-26 fix durations or the events
    that end them;
  - LR2-1 forbids inventing a rule, and no rule states a default duration.
- **Confidence:** high.
- **Strongest counter-argument:** some requests imply one unit, so guessing would be more
  helpful. Against that, guessing the count a change depends on produces a valid but wrong
  proposal, which is exactly what false support counts.

## Other contradictions and gaps (reported, not fixed)

1. The README `conflict` examples include "two outer armors", but LR2-13 says equipping a
   different outer armor unequips the previous one. Only "keep both equipped" is a conflict.
2. LR2-14 gives shields no swap rule, unlike LR2-13 for outer armor.
3. `refuse` ("outside the sheet and its rules") overlaps LR2-1 (not covered, so ask for a
   ruling). Nothing says where the line between them falls.
4. `might` is editable, but no rule says how it changes. Talents are editable, but LR2-17 says
   gaining or losing one is not covered.
5. Sheet-level `armor` (derived) and `equipment[].armor` (editable) share a name, so "set my
   armor to N" is ambiguous.
6. LR2-8's "cannot use equipment" for a down character does not say whether equipping,
   unequipping or lighting a lamp counts as use.
7. When oil runs out, LR2-33 turns `lamp_lit` false, but `lamp_lit` is editable, not derived.
   A proposal that drains the oil must set it itself; the `lit-needs-oil` constraint enforces
   that.
8. LR2-40 (a ration per travel day) and LR2-31 (a ration at the start of a long rest) do not
   say whether both apply on the same day.

## Next steps

1. User decisions: veto or accept 6–8, and choose the decision 7 sub-case (default
   `ask_rule`).
2. Add the accepted lines to the README precedence list.
3. Fix gaps 1, 2, 5 and 7 in the pack text or README. Each needs a decision on wording, not
   on outcomes.
4. Relabel the affected dev cases. Only then can the writer brief for the blind held-out set
   be extracted (§11 item 9).

## User decisions (2026-10-16)

The user's answers supersede the adjudicator's lines where they differ.

- **Rule 6: accepted.** A request that breaks a rule is `conflict`. Naming a legal alternative
  is friendly but optional, and never scored.
- **Rule 7: reversed.** Item stats are normally **not** editable: players find better items
  instead. The pack definition is to mark equipment stat fields (`armor`, `category`, `kind`,
  `weight`) as non-editable, while `quantity`, `equipped` and `name` stay editable.
  - A request to change an item's stat is `conflict`.
  - For a stat with no field (for example weapon damage), the assistant looks up the rule
    that derives it and answers with what determines it. It proposes only rule-backed changes
    the player asks for, and asks for a ruling only when no rule covers it. In this pack no
    rule defines weapon damage, so the outcome is `ask_rule`.
  - Acquiring a better item means adding an entry. The pack has no item catalogue, so this
    needs a rule; otherwise it is `ask_rule`.
- **Rule 8: confirmed.** If time is part of the calculation, the player must state it, and no
  default is assumed (`ask_fact`). If the rules leave the duration undefined, the outcome is
  `ask_rule`.

Applying these is a pack revision: definition editable flags, the README precedence, an
LR2-20 wording change and dev relabels. It is done with the other v3 pack fixes before the
blind writer brief is extracted.

## Pack revision 3 relabels (2026-10-16)

Scope: all 174 dev cases (the held-out split is an empty placeholder). Every case was checked
against precedence points 6–8 and against the new non-editable equipment fields (`kind`,
`weight`, `armor`, `category`). Inputs were the pack itself, this record and the harness code;
no results, ledgers or failure analyses were read.

**Outcome changes: none (0 of 174).** Counts by old → new outcome: 0 for every pair. The
expected outcome counts stay proposal 67, answer 38, ask_rule 14, ask_fact 14, conflict 29,
stale 8 and refuse 4.

Notes-only changes (outcome, gold rule ids and changes unchanged):

| Case | Old outcome | New outcome | Reason |
|---|---|---|---|
| ho-115 | conflict | conflict | Point 7: an item's `weight` is a fixed stat, so the request is a conflict for any value; the old note gave only the 0–5 range. |
| ho-086 | ask_rule | ask_rule | Point 7 sub-case: weapon damage has no field and no rule, so `ask_rule`; the note now cites point 7. |

Why nothing else changed:

- **Item stats (7).** No expected or pending change names `kind`, `weight`, `armor` or
  `category` of an equipment entry; every equipment change is `equipped` or `quantity`. The
  only dev request to set an item stat (ho-115) was already a conflict. Requests to set
  "armor" (ho-114, ho-125) mean the derived sheet value and were already conflicts.
- **Conflict with a legal alternative (6).** The conflicts that have a legal alternative
  (for example dev-021, ho-107, ho-111, ho-113, ho-120, ho-122, ho-123) were already
  `conflict` with no changes. The proposals that go past a limit are those where a rule
  resolves the excess (ho-011, ho-020, ho-021, ho-030, ho-037), so they stay proposals.
- **Time (8).** Every proposal that depends on time states it (watches, leagues, a scene
  end, a night's rest); ho-101 (unstated watches) was already `ask_fact`; dev-006, dev-022,
  ho-082, ho-083 and ho-091 (durations the rules leave open) were already `ask_rule`.

Mechanical checks (helper level, `mimik-ci`, formula CLI mounted): the pack loads (the
loader rejects a gold change to a non-editable field), the expected-outcome replay gives
174/174 correct (a gold proposal with a constraint violation would fail it), and
`tools/test_tool_loop_v3.py` `PackRevision3Tests` checks the editable flags, that no gold
or pending change touches a non-editable field, the rules wording, and the full replay.

Not decided here (returned to the user):

- dev-019 stays a `proposal` that raises an existing entry's
  `quantity`. Point 7 says acquiring a new or better item needs a rule; it does not say
  whether one more unit of an item already carried is "acquiring". If it is, dev-019
  becomes `ask_rule`.
