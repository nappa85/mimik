# Game definition language: building-block draft (2026-10-18)

Status: **draft for discussion with the user. Not reviewed, not a plan.** It needs an
independent adversarial review before any of it drives code.

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
