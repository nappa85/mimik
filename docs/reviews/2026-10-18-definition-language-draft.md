# Review: game definition language draft (2026-10-18)

**Scope.**
- `docs/definition-language-draft.md` as of `08d5c1b`: the building blocks, the worked
  multiclass example, and the paper-check gaps G1–G18.
- Checked against the runtime definition contract, the M3/M4 formula plans,
  `model-assisted-games.md`, `architecture.md`, `campaign-workspace.md`, `AGENTS.md`, and
  `src/definitions/formula/*` and `src/campaigns/*`.
- Prompt: the session scratchpad `defdraft-prompt.md`.

**Reviewers.**
- Sol (`--chain main`, `openai/gpt-6.1-sol`)
- Fresh Sonnet subagent
- Space Bunny (`--chain third`)

All three were static and read-only. All three returned **SOUND WITH CHANGES**: the direction
holds (structure in Rust, game concepts as data, the LLM as author and the owner as reviewer),
but the draft cannot drive code or a plan yet.

**Adjudication.** The Blocking findings were checked against the code and are confirmed:
- `nested_aggregate` and the rejection of indexing: `derived-formulas-plan.md:124,151`.
- `int` is a reserved name: `formula/profile.rs:35`.
- The 1,024-byte source limit: `formula/mod.rs:26`.
- Results are advisory: `runtime-definition-contract.md:41-47` and `campaigns/formulas.rs:4`.
- The 256 KiB definition cap and the 1,024 evidence references: contract `:143,147`.
- The catalogue is deferred: `derived-formulas-m4-plan.md:616`.

## Findings and disposition

| # | Source | Severity | Finding | Disposition |
|---|---|---|---|---|
| 1 | all three (B1) | Blocking | The worked example is not admissible under `cel-mimik-1`: nested macros, reference dereference, indexing, list-valued properties, `sum` signature, `contains`, reserved `int`. References, tables and lookups need a new profile with compositional cost accounting; profile-1 pins stay unchanged | Accepted. Draft marked: the example is an authoring sketch, and step 3 is replaced (see the order below) |
| 2 | all three (B2/B3) | Blocking | `error` "blocks save" contradicts the advisory-only contract and `AGENTS.md`. A wrong extracted rule could freeze sheets | Accepted. Default `warning`. Any blocking needs a separate rule-review state and an escape hatch. **User decision pending** |
| 3 | Sol B3 | Blocking | Final-state constraints cannot enforce transitions (unpaid purchases, choices made without the advancement that allows them). Direct edits bypass actions | Accepted. Design must separate state invariants, before/after checks, rules operations and corrections, for every write path. **User decision pending** (warn on diffs versus guided operations) |
| 4 | Sonnet B3, Space Bunny M4 | Blocking | Catalogues exceed the 256 KiB envelope and the 1,024 evidence cap. Page-scoped invalidation makes big catalogues perpetually stale | Accepted. A separate content-addressed catalogue bundle (the deferred M4 §8 milestone) |
| 5 | Space Bunny B2, Sol M5, Sonnet M2 | Blocking / Major | References have no stored form. M4 row IDs are slot labels, not identities. Errata and pin versions orphan references | Accepted. Choose projection or bounded host lookup; separate catalogue-entry, instance and position identity; define a migration policy |
| 6 | Space Bunny B4, Sol M2 | Blocking / Major | Campaign and scenario scope reads break history reproducibility and concurrency | Accepted. Forbid cross-scope reads in derived values for now (copy values onto the character with "as of"), or plan world-state revisions separately |
| 7 | all three | Major | Actions, flows and events have no execution semantics: order of assignments, collection mutation, idempotency, stale flows, history provenance, event loops, deadline fail-closed | Accepted for the spec; no events triggering events |
| 8 | Sol M6, Sonnet M4 | Major | Missing mechanic families: attributed grants with retraction (several sources for one effect), conditional bonuses, derived lists and text, sub-characters, containers, level-down, order of operations | Accepted; added to the gap list |
| 9 | Sol M7, Sonnet m1 | Major | G1's sum/max/min stacking is underspecified (mixed signs, grouping); `max_of` already exists | Accepted |
| 10 | Sol M9, Sonnet M4 | Major | The example's `(int - 10) / 2` truncates toward zero (9 → 0, not −1). No floor division | Accepted; the profile needs explicit rounding |
| 11 | Sol M8 | Major | The multiclass constraint does not enforce what the choice offers (a spell rank higher than the class level allows passes) | Accepted |
| 12 | all three | Major | The LLM-as-author premise is unmeasured. CPU speed makes catalogue-scale authoring take hours. Step 4 is not a measurement | Accepted. Measure deterministic extraction plus review first; owner review time is the primary cost; the model is never on the correctness path; pre-registered protocol and held-out pages |
| 13 | all three | Major / Minor | "No game needs Rust code specific to it" is an overclaim, from memory and not from manuals | Fixed in the draft |
| 14 | Sonnet M5, Sol m1, Space Bunny §4 | Minor | Block collapse: box track, tags, choice, flow and event lower to records, collections, constraints and transitions. Keep the catalogue-versus-instance distinction | Accepted for the revision |
| 15 | Sonnet m4–m6, Space Bunny §5 | Minor | Reserved-name policy, the 1,024-byte formula limit, prompt-injected text in descriptions (escape it, show it beside its source) | Accepted |
| 16 | all three | Note | Prior art to evaluate first: CEL extensions, Foundry data models and Active Effects (PF2e rule elements), GCS, PCGen, Datalog crates, 5e JSON schemas as shape references, statecharts | To record in `dependency-decisions.md` before design |

**Agreed order of work** (merged from the three reviews):
0. Decisions: legality authority (warn or block); transition checks; catalogue storage;
   references (projection or lookup); cross-scope reads.
1. A read-only catalogue with click-to-show descriptions and source links. This is the
   user's use case 4, needs no new formulas, and forces the storage and identity decisions.
2. An expressiveness harness: admitted CEL per archetype run through `formula_eval`, with
   the rejections recorded. Then profile 2 with constants and tables.
3. References, then warnings over them.
4. Actions (with their semantics), choices, flows.
5. Grants, modifiers, events, campaign scope (each with its own plan).
6. The LLM-authoring measurement last.

Each step gets its own plan and review.

**Verification.** Static review only. No code, builds or inference.
