# Runtime games and model-assisted sheet wiring

## User clarification

The goal is support for previously unknown games using owner-uploaded manuals and sheets, not a new Rust implementation for every game. Current MÖRK BORG forms/calculations are an exploration fixture. They must not become the mandatory extension mechanism.

2026-10-15: user explicitly prioritizes validating this end-to-end experience before
more administration and reiterates a "pure" game-agnostic core. The Owner supplies
the template file; explicit web field bindings/canonical values supply the sheet
context. Models dialogue with the sheet through bounded operations, optionally
propose textual mappings and semantically review edits. The user explicitly rejects
vision models for this trial; JavaScript may handle browser interaction without
introducing another scripting language/DSL. First trial workflow and unresolved
execution choices: [experience validation](experience-validation-plan.md). Existing
provider-free runtime definitions do not prove this AI workflow has been delivered.

## Intended data-driven architecture

1. **Owner source set:** manual editions, original sheet assets, layout-aware extraction, page/region evidence and review state. Source language/edition is explicit; a simpler English manual is not silently substituted for Italian rules.
2. **Runtime game definition:** stable owner-defined game/edition identifiers, versioned canonical field schema, sheet section/grouping presentation, source references, supported validation/calculation capabilities and model task configuration. Definitions are local data, not newly compiled vendor/game code.
3. **Textual sheet wiring/context:** owner web template supplies explicit bindings and labels/groups, joined with canonical field IDs/types/current values. Model mapping proposals are optional structured drafts, not image inference or executable code. Compare coverage/structure against the original template. Current flat controls/repeating-entry gaps must be tested, not assumed supported.
4. **Generic web rendering/Rust storage:** trusted generic JavaScript handles interactions/bindings and adaptive HTML; Rust remains authoritative for structural validation/access/revisions/persistence/history. Character values stay independent of layout. Uploaded markup is presentation/data, not permission to execute active code with session privileges. Template format remains to be selected; HTML/CSS bindings are a proposal.
5. **Explicit prompt tasks:** sheet wiring, sheet validation, rules inquiry and contextual options. Each receives canonical character data, exact game/source identity, declared situation, retrieved evidence and a task-specific response schema. No hidden mutation or inferred world facts.
6. **Validation tiers:** models dialogue with the sheet and report source-linked advisory findings/proposed typed patches through bounded domain operations. Structural validation stays deterministic; missing/ambiguous rules remain explicit. No new rules DSL/expression facility is required for the experience trial; JSON definitions are data contracts, not a scripting language.
7. **Changes:** proposed sheet edits are separate from answers/findings; explicit owner/player action and deterministic supported checks govern application. Rules queries do not spend resources or alter sheets.

## Accuracy boundary

Making the sheet context and using specialized prompts is appropriate, but does not make a model 100% accurate. The software must distinguish draft wiring, advisory findings, reviewed evidence and enforced checks. The model cannot by itself establish legality, approve extraction or become the authority for automatic state changes. Owner uploads remain essential: they define the actual game, edition, layout and evidence.

## Migration from the prototype

- Keep existing MÖRK BORG characters/read-only checklist working; use them as compatibility and evaluation fixtures.
- The loaded-and-ready selector should eventually query approved runtime definitions/capabilities. Its temporary compiled MÖRK BORG catalog is not the final architecture.
- Replace the fixed family import restriction with a runtime identifier/definition registry; imported materials may exist before a usable sheet definition.
- Do not add more native per-game form modules as the normal expansion path.
- Evaluate maintained schema/rendering/provider libraries before implementing generic facilities. Existing genai/Rig research supplies provider-abstraction candidates; specialized local decision runtimes and any expression facility are future options, not requirements for this text-only trial.
- Review the context/tool/definition contract independently before code and retain unknown-game regressions. Compare bindings with the uploaded web template and text-model findings with manual evidence; human verification against original source images is separate from model input.

## Bare Bones candidate

`~/temp/MÖRK BORG BARE BONES EDITION.pdf` is a76-page English/plain-text edition with documented transcription/remediation. It is a promising extraction/evaluation candidate. Freely downloadable does not relicense it under Mimik's AGPL; keep content local. Confirm rule/version equivalence and retain separate provenance before using it to support the Italian adapter or a new runtime definition.
