# Game definition language: building-block draft (2026-10-18)

Status: **reviewed 2026-10-18 (Sol, Sonnet, Space Bunny): SOUND WITH CHANGES.** It is not
yet a basis for code. Record and agreed order of work:
[reviews/2026-10-18-definition-language-draft.md](reviews/2026-10-18-definition-language-draft.md).
The YAML example below is an authoring sketch. It is **not admissible** under `cel-mimik-1`
(nested macros, dereference, indexing, reserved `int`, truncating division), and §"How to
validate" step 3 is superseded by the review's order of work. User decisions (2026-10-18):
1. **Rule violations warn and never block.** Every change is logged and can be reverted.
   Where possible, the app suggests a correction.
2. **Advancement is context-driven.** An edit that only makes sense inside an advancement
   (for example raising max HP directly) is warned as unsupported, because the context is
   missing. Starting from the trigger (raising the level) opens that context. The app then
   highlights a "happy path": which fields can or should change, with hints on each field. It
   checks the changes against the state before the advancement.

## Product principle (user, 2026-10-18)

**Assist players; don't police them.** At a real table the GM trusts the players, and anyone
can cheat with a pencil. Mimik adds visibility and help for the player, not surveillance.
- Warnings are help, addressed to the person editing the sheet, with a suggested correction.
- No GM alert feeds, no "violations" dashboards, no audit views built to monitor players.
  The existing history stays an ordinary record of edits.
- The target user may not know the rules, even in a very complex game. Guidance (hints on
  fields, guided advancement, descriptions on click, plain explanations) should let them play
  almost without reading the manual.

## Why

User direction (2026-10-18):
- Players edit the sheet themselves. The app checks that edits are legal and helps with
  arithmetic, such as applying damage.
- It guides advancement ("level up": what to change, and whether the result is correct).
- It tells which options a character may pick, such as spells per class level in a
  multiclass character.
- It shows the full description of a sheet item on click.
- Free chat comes last.

The engine must stay **game-agnostic**: an owner can load a game that did not exist when Mimik
was written. Games differ a lot, and some have no hit points, levels or classes. So Rust knows
only **structure**, and every game concept is data in the game definition. The LLM's main job
moves to *authoring* that definition from the manual, with owner review. It is no longer
judging every edit at play time. See the router results
(`decision-router-results-2026-10-17.md`) for why play-time judging failed on this hardware.

This changes `model-assisted-games.md` §6 ("no new rules DSL is required for the trial"):
derived formulas (CEL) already exist, and this draft extends the declarative data around
them.

## Archetypes used to test the blocks

These are anonymous mechanic families drawn from general knowledge of published games. No game
is named and no text is reproduced. The test: every archetype must be expressible with the
blocks below.

| # | Archetype | Mechanics the blocks must express |
|---|---|---|
| A | Class and level fantasy, multiclass | per-class levels; a total level that sums them; progression tables by class level; spell slots from a table on *one* class's level, or a combined caster level with weights; prepared spells = an attribute bonus + a class level; spells restricted to the class lists; feats with prerequisites; rests that refill resources |
| B | Skill-based, no classes or levels | percentile skills; improvement by ticking skills used and rolling later; a derived resource (sanity-like) with a starting value from an attribute and a maximum lowered by a skill; points spent at creation from pools computed from attributes |
| C | Narrative / playbook | moves granted by the playbook; stress boxes; harm with severity levels and a limited number of slots per level that spill upward; progress clocks; XP triggers and advance options picked from a list, each pickable once or a limited number of times |
| D | Attribute + skill dice pools, experience purchase | pool = attribute + skill; buying a dot costs new rating × multiplier, with a different multiplier per trait kind; caps that depend on another trait (generation, rank) |
| E | Vehicle / ship / base | a container with capacity (slots, mass, power); modules that consume capacity; modules installable only in matching mounts; damage to a module disables what depends on it |
| F | Rules-light, slot inventory | inventory slots = an attribute + constant; items occupy 1–2 slots; random tables at creation; a per-day resource from a die roll; "the GM decides" rules |

## The blocks

Rust implements only these. Each has a source link (document, page, region) and a review
state, as sources do today.

| Block | What it is | Archetype uses |
|---|---|---|
| **Value types** | integer, boolean, text, enum, dice notation (as text, no rolling), box track (n boxes, each in one of k states) | all; box tracks for C |
| **Entity type** | a named record type with fields. The character is one entity type; a spell, class, module or harm is another | all |
| **Catalogue** | the reviewed instances of an entity type that come from the manual (the spell list, class list, module list). Read-only at play time | A spells and classes, C advances, E modules |
| **Reference** | a typed link from one entity to another, with a cardinality (one, many, or many with a count). Example: a character has many *class levels*, each referencing a class; a spell references the classes whose list it is on | A, C, E |
| **Table** | lookup by key or range: `slots[class_level][spell_level]`, `cost[kind]`, `modifier[score range]` | A, D, F |
| **Formula** | CEL expression, no side effects, bounded; may aggregate over references (`count`, `sum`, `filter`, `exists`) | all |
| **Constraint** | a formula that must hold, with severity `error` (blocks save) or `warning` (shown, saves), message and source | all |
| **Choice** | "pick n from a filtered catalogue": n and the filter are formulas. Example: pick spells where `spell.classes contains c` and `spell.level <= max_spell_level(c)` | A spells, C advances, A feats |
| **Action** | a named, parameterised state change: precondition formulas, then assignments computed by formulas, applied atomically as one revision. Example: `take_damage(n)`, `spend(resource, n)`, `rest(kind)` | all |
| **Flow** | an ordered list of steps (choices, actions, manual edits with constraints), each step's options computed from the state so far. Committed only at the end, as one revision with a diff. Example: level-up | A, B, C, D |
| **Text rule** | a rule the blocks cannot express ("the GM decides", odd exceptions), stored with its source. Used only by the optional play-time LLM, which may raise **warnings**, never errors | F, edge cases everywhere |

Explicitly **not** in Rust: hit points, level, class, spell, race, skill, rest, or any other
game concept. Dice rolling is outside the sheet-first scope (notation only).

## Worked example: the multiclass spell question (archetype A)

```yaml
entity_types:
  class:      { fields: { name: text, caster: enum[none, full, half], list_id: text } }
  spell:      { fields: { name: text, level: int, lists: [text], description: text } }
  class_level:{ fields: { class: ref(class), levels: int } }
  character:
    fields:
      classes:  many(class_level)
      known:    many(spell)
      int_mod:  { derived: "(int - 10) / 2" }
    formulas:
      total_level: "sum(classes, c, c.levels)"
      mage_level:  "sum(classes.filter(c, c.class.list_id == 'mage'), c, c.levels)"
      prepared_max: "max(1, int_mod + mage_level)"
    constraints:
      - error: "known.all(s, s.lists.exists(l, classes.exists(c, c.class.list_id == l)))"
        message: "A spell is not on any of your classes' lists"
tables:
  mage_slots: { key: [class_level, spell_level], source: "p. …" }
choices:
  learn_spell:
    pick: 1
    from: spell
    where: "s.lists.exists(l, l == 'mage') && mage_slots[mage_level][s.level] > 0"
```

A warrior 4 / mage 1 gets `total_level` 5 and `mage_level` 1. The choice offers only level-1
mage spells. The constraint rejects a spell from another list. Clicking a spell shows
`spell.description` with its source link. Combined caster-level rules ("full casters count
fully, half casters count half") are one more formula. They need no Rust change.

## Other archetypes, briefly

- **C, harm spill-over:** harm is a box track per severity (e.g. 2, 2, 1 slots). Action
  `take_harm(sev)` places it at `sev`, or at the first higher severity with a free slot.
  This needs a bounded "first index where" formula: **gap to check in CEL**.
- **D, experience purchase:** action `buy(trait)` has precondition `xp >= cost` and the
  assignment `xp -= table cost[kind] * (rating + 1)`; constraint `rating <= cap(rank)`.
- **E, capacity:** constraint `sum(modules, m, m.power) <= reactor.output`; a module's
  `mount` reference must match a mount type; a formula derives `disabled` from the damage
  state of the module it depends on.
- **B, improvement:** a boolean `checked` per skill; flow `session_end` lists the checked
  skills. The roll happens at the table: the player enters the result and the action applies
  it.
- **F:** constraint `sum(items, i, i.slots) <= str + 10`; "the GM decides" goes in as a text
  rule.

## Known gaps and risks

1. **Mapped from current code:** fields, collections (lists, M4), CEL derived formulas and
   constraints exist. References between entities, catalogues, tables, choices, actions and
   flows do not.
2. **CEL coverage:** aggregates over lists exist as macros; "first index where", bounded
   recursion and table lookups need checking in the CEL profile (`cel-mimik-1`). A gap here
   should be closed with a host function, not a new language.
3. **Modifier stacking** ("bonuses of the same type don't stack", "take the highest") is
   common and awkward in plain formulas. It may need a block of its own.
4. **Timing and state:** "until your next rest", "once per day" and conditions with durations
   are state plus actions. They are expressible, but verbose.
5. **Authoring burden:** a full spell catalogue is hundreds of entries. Extraction by the LLM
   plus owner review is the main cost, and it is the part to measure next.
6. **Over-generality:** every block added must be justified by an archetype above.

## Paper check against the owner's games (2026-10-18)

- **Method:** the main session's general knowledge of the mechanics, not the PDFs. The
  owner's library covers archetypes A (three games, one of them Italian), D (one) and F (one),
  plus one d6-pool game (D) with campaign tiers and keywords, and one narrative investigation game (C) built around cards and spends.
- **Coverage:** E has no game in the library and is covered from general knowledge only.
- **What is recorded:** gaps, and only at a structural level. No game text or tables.

| # | Gap | Seen in | Proposed fix (to review) |
|---|---|---|---|
| G1 | **Typed modifier stacking.** Bonuses of the same type don't add up, only the highest counts; penalties work the same way; some values are "the best of several formulas" | A (strongly in one game) | a `modifier` block: `{target, type, value, source}`, with a stacking policy per type (`sum`, `max`, `min`); plus a `max_of` over alternative formulas |
| G2 | **Multi-state box tracks with conversion.** Boxes are empty, light or heavy; when the track is full, light damage converts to heavy; incoming damage may be halved first | D | box track type with per-state counts, plus host functions for "count in state" and "first box in state"; the conversion is an action |
| G3 | **Ordered action steps with intermediate values.** Damage goes to a temporary pool first, then the remainder to the main pool | A, D | `let` bindings inside an action: each assignment can use earlier results |
| G4 | **Choices keyed to a schedule and to which class gained the level.** E.g. a feat at class levels 4/8/…, or per-category picks at fixed character levels | A | the flow carries context (`gained_class`, `new_level`); a choice's `when` is a formula over that context |
| G5 | **Prerequisite graphs inside catalogues.** An item needs other items or ranks; a power needs a rank in another discipline-like trait; a spell can be taken at higher ranks | A, D | references between catalogue entries, plus prerequisite formulas evaluated against the character |
| G6 | **Choice with distinct picks and conditional effects.** Pick 4 different attributes; each gets +2, or +1 above a threshold | A | `distinct: true` on choices, with the effect as a formula of the current value |
| G7 | **Costs that depend on how the character relates to the item.** A trait costs more if it is out of the character's clan or class; cost depends on how many matching aptitudes the character has | D, percentile game | table keyed on a computed key (already formulas + tables; confirm the lookup can take a formula key) |
| G8 | **State outside the character.** Campaign-wide doom trackers, party resources | F | campaign-scoped entity (ties to the campaign workspace plan) |
| G9 | **Values rolled at creation or level-up.** HP = attribute + one die; random tables | A, F | dice notation stays text; the player enters the result; a constraint checks the result is in the die's range; random tables are catalogues with a roll range |
| G10 | **Fractional units.** Light items count as a tenth of a unit | A | integer scaled units (store tenths), declared in the field |
| G11 | **Durations and events.** Conditions that drop by 1 at a time step; resources that refill on rest or at dawn | A, D, F | named events (`rest_short`, `dawn`) that run a list of actions; no clock in the engine |
| G12 | **Localised definitions.** An Italian game, and Italian and English editions of the same rules | A | labels and catalogue text in the source language; edition identity already exists |
| G13 | **Tags (keyword sets).** Faction and species keywords gate options and interact with rules; requirements like "has keyword X" or "any of these keywords" | D (d6-pool game) | a `set of enum/text` value type with `contains`/`intersects`; catalogue filters use it |
| G14 | **Campaign-level limits on the character.** The campaign's tier caps ratings and sets starting XP and which archetypes are allowed; a party pool is shared | D (d6-pool game), F | same campaign-scoped entity as G8; character formulas may read it (read-only) |
| G15 | **Cumulative cost tables.** Raising a rating from a to b costs the sum of the per-step costs | D | a host function `table_sum(table, from, to)`, or a precomputed cumulative column |
| G16 | **Cards with a lifecycle.** Persistent advantages and problems are cards held by the character: gained, played, discarded, cancelled by a matching card, some carried over to the next adventure. Effects are mostly free text | C (investigation game) | catalogue entries instanced with a `state` enum and a `pairs_with` reference; the effect text is a text rule shown on click; the app tracks the hand, it doesn't adjudicate the effect |
| G17 | **Scenario scope.** Spendable pools are separate from ratings, and they refill at the start of each adventure; some state lasts one adventure only | C, D | a third state scope between character and campaign (`scenario`), plus the G11 `scenario_start` event; pool ≤ rating is an ordinary constraint |
| G18 | **Abilities without numbers.** Having an investigative ability gives its clues automatically; only extra benefits cost spends | C | boolean fields plus a pool; no new block, recorded to confirm the blocks don't assume numbers |

**Result.** For the mechanic slices examined from memory, no per-game Rust code was found
necessary; executable coverage is unverified. G1, G2, G4 and G17 are the significant
additions; the rest are small extensions of blocks already in the draft. G8 depends on the
campaign workspace.

## How to validate this draft (proposed)

1. **On paper:** describe one real game from the owner's PDFs per archetype with these blocks.
   The descriptions stay local, because manuals and derived data never enter the repo.
   Record the gaps only.
2. Review this draft (Sol + Sonnet + third chain) before any code.
3. **Smallest code step:** references and tables, then the multiclass example as a fixture
   test of the existing CEL evaluator.
4. Then the new LLM task: extract one catalogue (e.g. a spell list) from a manual into this
   format, scored against an owner-reviewed sample. This replaces the play-time tool-loop
   benchmark as the model question.
