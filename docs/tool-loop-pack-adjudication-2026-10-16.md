# Tool-loop pack: blind ambiguity adjudication (2026-10-16)

Status: **user decisions recorded 2026-10-16 (see "User decisions" below); applied in pack
revision 3** (see "Pack revision 3 relabels" at the end). One existing dev outcome changed
(ho-048), and one dev case was added (dev-034).

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
  - ~~Acquiring a better item means adding an entry. The pack has no item catalogue, so this
    needs a rule; otherwise it is `ask_rule`.~~ **Reversed 2026-10-16** (see "Second
    follow-up"): gaining and losing items is bookkeeping.
- **Rule 8: confirmed.** If time is part of the calculation, the player must state it, and no
  default is assumed (`ask_fact`). If the rules leave the duration undefined, the outcome is
  `ask_rule`.

Applying these is a pack revision: definition editable flags, the README precedence, an
LR2-20 wording change and dev relabels. It is done with the other v3 pack fixes before the
blind writer brief is extracted.

## Follow-up user decisions (2026-10-16)

Answers to the points the first pack revision 3 pass returned:

1. **Acquiring is acquiring** (*reversed later on 2026-10-16; see "Second follow-up"*). Any increase in an item's quantity that comes from acquiring it
   (finding, buying or receiving), even of a kind already carried, needs a rule. The pack has
   none, so such requests are `ask_rule`. Decreases through consumption follow the consuming
   rule (for example LR2-34) and stay valid proposals.
2. **A night is two watches.** Added to LR2-31 and LR2-26 as "the night of a long rest is two
   watches", so a lamp left lit through a long rest burns 2 measures (LR2-33). It is scoped
   to the long rest because a night journey's length comes from LR2-36 and LR2-37 (a night
   journey can take more than two watches).
3. **"armor" alone means the sheet total** (README convention, confirmed).
4. **Visible revision bumped:** `rules_revision` 3. `edition_id` became `demo-3`: the harness
   only copies it into the model context, the Rust pack test only passes it through, and no
   registry, queue, ledger identity or case subset keys on it.

## Second follow-up user decisions (2026-10-16, after the reviews)

Review of `c81c669`: Sol REJECT, Sonnet APPROVE-with-fixes, third reviewer unavailable. The
user then decided:

- **Items are bookkeeping (reverses follow-up decision 1).** Gaining and losing items
  (picking up, finding, buying, receiving, dropping or giving away) is plain bookkeeping.
  The assistant proposes the change (quantity up or down, or removing an entry) and needs no
  rule. Item stats stay fixed. Finding rations or bandages is bookkeeping too, unless a rule
  governs the source (LR2-29 for shared stock).
- **Harness limitation.** `propose_change` cannot create or remove an entry. A request that
  needs a brand-new entry is out of the blind writer's scope until list editing and an item
  catalogue exist. A decrease to 0 breaks `quantity-positive`, and removal is impossible in
  the harness, so a request to drop or use up an entry's last unit is out of scope too.
  Implementer's decision: no gold outcome is defined for either, and no dev case has one.
- **Review fixes adopted:**
  - the conduct rules are now model-visible in a new LR2-45 (Sol R3);
  - ho-048 is relabelled (Sol R1);
  - ho-003 is reworded (Sol R4);
  - the long rest with a lit lamp is stated, with a new dev-034 (Sonnet R1);
  - a no-field stat governed by a rule gives `answer`, or `proposal` for a concrete
    rule-backed change (Sonnet R2);
  - supplies count as bookkeeping (Sonnet R4);
  - question-only requests give `answer` (Sonnet R5);
  - the writer scope exclusions are written down (Sol R5);
  - the notes check is added (Sol R2), and the tests fail closed instead of skipping (T1,
    T2);
  - the two-watch figure is limited to a long rest's night (D1).

## Pack revision 3 relabels (2026-10-16)

Scope: all dev cases (174 before, 175 after dev-034; the held-out split is an empty
placeholder). Every case was checked against:

- precedence points 6–8;
- the non-editable equipment fields;
- item bookkeeping;
- the two-watch night;
- the review findings.

Inputs were the pack itself, this record, the review findings as relayed and the harness
code; no results, ledgers or failure analyses were read.

**Outcome changes against master (rules revision 2): 1 of 174, plus 1 added case.** By
old → new outcome: proposal → ask_rule 1 (ho-048); every other pair 0. New case dev-034
(proposal). dev-019 went proposal → ask_rule in `c81c669` and back to proposal here, so it
is unchanged against master. The expected outcome counts become proposal 67, answer 38,
ask_rule 15, ask_fact 14, conflict 29, stale 8 and refuse 4 (at-risk 70, supported 105).

| Case | Old outcome | New outcome | Reason |
|---|---|---|---|
| ho-048 | proposal | ask_rule | Sol R1, rule 8: LR2-30 leaves a short rest's duration undefined, and Cass's lamp is lit (LR2-33), so the oil burned cannot be computed; the Master's focus ruling is not sufficient. Changes removed; gold LR2-30, LR2-25 → LR2-30, LR2-33, LR2-45 (LR2-45 added in round 2, Sonnet N6). |
| dev-019 | proposal | proposal | Reverted to the master gold (changes and gold LR2-19, LR2-20) after the bookkeeping decision; the note cites LR2-45. |
| dev-034 | — (new) | proposal | Sonnet R1: a long rest with the lamp lit (override) burns the two-watch night's 2 measures; the lamp empties, so `lamp_lit` becomes false (LR2-31, LR2-33). |
| ho-003 | proposal | proposal | Sol R4: the request now says both items are already carried; changes unchanged. |
| dev-002, ho-002 | proposal | proposal | Round-2 Sonnet N5: same rewording as ho-003; changes unchanged. |
| ho-093 | ask_rule | ask_rule | Round-2 Sol RR3: the note says prices and coin-to-rations conversion are undefined (buying itself is bookkeeping). |
| ho-115 | conflict | conflict | Notes only. Point 7: an item's `weight` is a fixed stat, so the request is a conflict for any value; the old note gave only the 0–5 range. |
| ho-086 | ask_rule | ask_rule | Notes only. Point 7 sub-case: weapon damage has no field and no rule, so `ask_rule`; the note now cites point 7. |

Why nothing else changed:

- **Item stats (7).** No expected or pending change names `kind`, `weight`, `armor` or
  `category` of an equipment entry; every equipment change is `equipped` or `quantity`. The
  only dev request to set an item stat (ho-115) was already a conflict. Requests to set
  "armor" (ho-114, ho-125) mean the derived sheet total and were already conflicts.
- **Bookkeeping (7).** dev-019 is the only gold that raises a quantity. Five golds lower an
  oil flask's quantity by pouring it into a lamp (LR2-34: dev-018, ho-021, ho-022, ho-023,
  ho-037), and none reaches 0. Shared rations or bandages (dev-024, ho-033, ho-034) stay
  under LR2-29. The price questions (ho-084, ho-093) stay `ask_rule`: pricing has no rule.
- **Conflict with a legal alternative (6).** The conflicts that have a legal alternative
  (for example dev-021, ho-107, ho-111, ho-113, ho-120, ho-122, ho-123) were already
  `conflict` with no changes. The proposals that go past a limit are those where a rule
  resolves the excess (ho-011, ho-020, ho-021, ho-030, ho-037), so they stay proposals.
- **Time (8).**
  - Undefined durations: the short-rest cases dev-006, ho-082 and ho-083 and the foraging
    cases dev-022 and ho-091 were already `ask_rule`.
  - ho-056 asks only whether a short rest ends shaken, so it stays an `answer`.
  - ho-048 was the only proposal that depended on an undefined duration.
  - Scene ends burn no oil (LR2-33), so dev-023 (Cass, lamp lit) is unaffected.
  - ho-101 (watches not stated) was already `ask_fact`.
- **Two-watch night.** The existing long-rest cases are dev-026 (a bleeding conflict),
  ho-017 and ho-018. In each, the lamp is unlit or there is no lamp. Night travel (ho-031,
  ho-041, ho-067) takes its length from LR2-36 and LR2-37.

Mechanical checks (helper level, `mimik-ci`, formula CLI mounted):

- The pack loads; the loader rejects a gold change to a non-editable field.
- The expected-outcome replay is complete; a gold proposal with a constraint violation
  would fail it.
- `--validate-pack` `notes_check` recomputes every proposal with the engine and compares
  the values its notes state with the engine's results. It reads overrides, "a -> b"
  transitions and "Host derives". It covers stated values, derived fields and constraints,
  not whether the right rule was applied.
- `tools/test_tool_loop_v3.py` `PackRevision3Tests` checks the editable flags, the
  revision, the rules wording, the model-visible counterparts of points 6–8 in LR2-45,
  bookkeeping (no quantity reaches 0; dev-019 raises one), the review relabels, the full
  replay and the notes check. The CLI-dependent tests fail instead of skipping when the CLI
  is missing, and skip only in the source package.

### Round-2 review fixes (2026-10-16, review of `2b22ee5`)

Sol REJECT and Sonnet APPROVE-with-fixes; the third reviewer was unavailable. All the items
were narrow, and all were applied. No outcome changed.

- **RR1:** the quantity test is now a property over dev and held-out, with no pinned ids. A
  quantity may rise or fall only in a proposal or a pending proposal, only on an existing
  entry, and never to 0. No change creates or removes an entry.
- **RR2:** LR2-45 item 4 applies a rule-fixed duration or ending event without asking. It
  asks for the elapsed time only when no rule fixes it, and asks for a ruling when the
  rules leave a duration undefined.
- **N2:** LR2-45 item 1 makes an explicit set beyond a limit a conflict and names the rules
  that resolve a limit (LR2-5, LR2-13, LR2-14, LR2-33, LR2-34).
- **N3:** LR2-45 item 2 says no rule sets prices or what coin buys, so those need a ruling.
- **"armor" convention:** LR2-15 now says plain "armor" in a request is the sheet value.
- **N1:** a new `oil_check` in `--validate-pack` independently computes max(0, oil −
  watches) for a lit lamp over a long rest or stated watches. It checks dev-017, ho-020 and
  dev-034, and a mutation of dev-034 without its oil change is rejected. The replay is
  documented as structural only.
- **N4:** `notes_check` fails on unresolved claim tokens. The demo counts are pinned at 55
  proposal cases with 144 claims.
- **N5, N6, RR3:** applied as listed in the relabel table.

