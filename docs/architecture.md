# Proposed architecture

Status: proposal pending discovery answers. Rust and htmx are user requirements; the specific libraries and deployment defaults below are recommendations.

## Product boundary

Confirmed: AGPL-3.0-only, standalone, sheet-first companion for players at a physical table. Optimize for readable/editable sheets and quick contextual rule access. Virtual-tabletop maps, immersive scene rendering, and full-table replacement are outside the initial scope.

## Accuracy boundary

Qwen cannot provide a 100% accuracy guarantee. RAG improves grounding but does not eliminate missing retrieval, edition confusion, mistaken interpretation, or unsupported generation. Make an explicit, versioned rules engine authoritative for implemented calculations, validation, eligibility, and resource changes. Its own correctness depends on source review and meaningful rule tests.

The model may interpret a question, propose structured context, summarize retrieved passages, and explain engine results. Model output must never directly update a sheet or establish action legality. Distinguish verified implemented rules, source excerpts, model explanations, unknown context, and GM adjudication in the UI.

## Suggested stack

- Rust: Axum + Tokio for HTTP, Askama for escaped HTML templates.
- UI: semantic HTML/CSS, htmx for partial updates; business logic on the server. htmx itself is JavaScript, so this means no application JavaScript framework, not literally a JavaScript-free browser.
- Storage: SQLite initially, with schema migrations and transactional updates.
- Inference: an established Rust multi-provider abstraction for commercial/local API models, with thin Mimik-specific typed-decision/review/explanation boundaries. Evaluate genai/Rig before provider integration; do not hand-write vendor protocols. Evaluate Kev and AnyJev/Tacit separately for specialized local decisions. See [provider research](provider-abstraction.md) and [decision-model research](decision-models.md).
- Retrieval: begin with edition-filtered lexical search; add local embeddings and hybrid ranking after evaluating real manual questions. Vector search alone is not an accuracy guarantee.

Shipping inference should favor a lightweight Rust/native integration, with CPU fallback and Vulkan evaluated for vendor-neutral GPU acceleration. Python reference servers are acceptable for prototypes. Specialized decision-model support must be proven in the runtime; see [hardware and runtime direction](hardware-and-runtime.md).

## Domain boundaries

1. **System registry**: stable system/edition identifiers, adapter version, source set, supported capabilities.
2. **Character storage**: per-system structured data, schema version, revision, inventory, resources, notes, and explicit overrides. Do not force every game into a D&D-shaped schema.
3. **Rules adapter**: validation issues, derived fields, eligible options, prerequisites, resource costs, effects, and source references.
4. **Sheet presentation**: mappings between canonical data and edition-specific official sheet fields; responsive editor plus official-layout view/export.
5. **Situation context**: structured known facts, unknowns, targets, distances where applicable, conditions, equipment, remaining resources, and player intent. Free-text descriptions are proposals until confirmed.
6. **Knowledge service**: document ingestion, provenance, edition/source filtering, retrieval, and answer evidence.
7. **Decision service**: optional intent classification and ranking of engine-provided candidate IDs, with validated typed outputs and domain-evaluated uncertainty.
8. **Explanation service**: deterministic explanations first; optional generative elaboration with source links and explicit uncertainty.
9. **Campaign access and history (planned)**: separate technical Owner, campaign Master and character custody; server-side authorization, complete transactional edit history, version-aware rollback, soft deletion and Owner-only purge. See `collaboration-and-history.md`; these are not current runtime capabilities.

## Official sheet fidelity

Confirmed: recreate the original sheet's recognizable structure in adaptive HTML/CSS. Preserve section grouping and visual hierarchy while allowing long text, repeated entries, and mobile reflow rather than fixed PDF fields. Each edition needs its actual template and canonical field mapping. Print/export is a separate future feature. See [sheet presentation](sheet-presentation.md). Review asset terms before bundling; ownership of a manual alone does not establish redistribution rights.

## Dependency reuse policy

Before custom infrastructure, assess existing crates/libraries for maintained APIs, required capabilities, AGPL-compatible licensing, portability, and footprint. Record material selections or demonstrated gaps. Custom code should implement Mimik's domain behavior, not duplicate maintained provider clients, configuration parsers, storage engines, or extraction infrastructure.

## Validation and context

Separate invalid values, rule inconsistencies, missing facts, and intentional house-rule overrides. Derived values carry an explanation and source. Use transactional saves with revision checks to avoid silently overwriting concurrent edits. Advice can list an option as available, unavailable, or needing context; never equate “not yet implemented” with “illegal.”

## RAG flow

Current implementation (2026-10-15): the Rust app has only reviewed-only literal
`LIKE` lookup (`src/sources/store.rs`). It has no ranking, FTS5, embeddings or
model integration. Proposed hybrid retrieval and tool loop:
[harness-assessment-2026-10-15.md](harness-assessment-2026-10-15.md#proposed-architecture-awaiting-user-decision).


1. Register a source with title, edition, language, revision, document hash, and page metadata.
2. Extract text, using OCR when needed; preserve printed page labels separately from PDF page indexes.
3. Chunk by rule sections while retaining tables and cross-references where possible.
   Before treating a section as verified evidence, compare its extraction against rendered PDF pages, preserve action glyph semantics, and record review state/corrections. See [source quality](source-quality.md). Unreviewed legacy Markdown/indexes are not authoritative.
4. Filter by the character's exact system, edition, enabled books, and house-rule precedence before retrieval.
5. Retrieve evidence for the question and relevant engine facts; ask for missing context or return insufficient evidence when necessary.
6. Generate an explanation with source identifiers; validate that cited identifiers were actually retrieved. Citation existence does not prove that the claim is entailed, so evaluate that separately.
7. Keep manual evidence separate from campaign notes and GM-only information.

Decided model interaction (user, 2026-10-15; not implemented): small tools (`search_rules`, `read_fields`, `propose_change`, `ask_user`). The host fills old values and revisions, evaluates declarative derived-field formulas in Rust and shows a diff for explicit confirmation, instead of the model emitting a complete typed patch in one response. Formula evaluation exists (M3): sheets evaluate their pinned formulas on view, save and history, and the library function `formula::preview` reports what a proposed change would change (derived values, new/resolved/unknown constraint outcomes). No tool loop, route or model integration calls it yet.

Acceptance requires a curated set of questions and expected sources, edition-isolation checks, retrieval measurements, unsupported-answer behavior, and rule edge-case tests. Imported documents are data, never executable model instructions.

## Extensibility

The user's updated direction is runtime, model-assisted game definitions for unknown games. See `model-assisted-games.md`. The native MÖRK BORG fixture is exploratory, not the required extension path. Uploaded sheet/manual evidence should produce reviewed declarative schemas and task prompts that the generic Rust runtime can consume.

Define an internal Rust adapter contract before implementing the first game. Capabilities should include sheet schema/layout, validation, calculations, situational options, action/spell explanations, source metadata, and import/export. Expose coverage so incomplete adapters are visibly incomplete.

Use runtime declarative definitions for fields, layouts, source sets and advisory task configuration. The compiled MÖRK BORG module is only an exploration/compatibility fixture, not mandatory per-game code. Stabilize this data contract against unknown-game fixtures before considering executable WASM/subprocess plugins for demonstrated gaps. Host-controlled generic rendering and validated proposals keep storage and UI consistent.
