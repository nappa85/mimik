# Roadmap

Status: proposed, not a commitment to particular editions or delivery dates.

Confirmed product focus: standalone open-source sheets and contextual rule assistance at the physical table. Current progress and completed checkboxes live in `../TODO.md`; the character-record portion of phase 1 is implemented.

**Current next priority:** an [experience-first viability experiment](experience-validation-plan.md):
owner-uploaded web sheet + manual text → model–sheet dialogue through a generic
harness → explicitly reviewed edits and real table use. The first model is text-only;
vision is explicitly excluded. Generic JavaScript handles browser interaction;
Rust governs access, structure and persistence. No extra scripting language/DSL.
**Local inference with RAG over extracted material is required for ordinary use.**
Hosted API credentials are solely for optional OCR/extraction review, not a
fallback usage model. Actual local probe results:
[2026-10-15 benchmark](local-rag-benchmark-2026-10-15.md).
The user explicitly
prioritizes proving usefulness and acceptable preparation effort over more admin
panels. Further registry/definition administration is paused. Existing native
MÖRK BORG code is an evaluation fixture; new games must not require hardcoded game
logic, fields or sheet templates. The [runtime-definition contract](runtime-definition-contract.md)
supplies existing structural boundaries, not proof that AI wiring or uploaded
sheet fidelity already works. Numbered phases group goals rather than execution order.

**Accuracy-first follow-up (2026-10-15):** the user authorizes disposable harness
experiments; sheet-as-context is a high-level goal, not a frozen prompt design.
See [accuracy experiments](accuracy-experiments.md) for the inspected legacy
Pathfinder artifacts, structured evidence versus derived Markdown/indexes, optional
frontier-model page comparison, context assembly/arithmetic tools, and controlled
extraction/retrieval/reasoning tests. Keep MÖRK BORG as the first playable trial and
use a bounded, edition-pinned Pathfinder slice as the rule-heavy stress test. The
old experiment already used FAISS+BM25; a vector database alone is not a fidelity fix.

User follow-up: preserve book-wide hierarchy across arbitrarily long chapters and
batch boundaries; evaluate automated frontier review to reduce human work, with
targeted checks and separately labelled automated/human coverage. Fast broad model
screening (6–10 candidates, then 2–3 finalists) supersedes the one-challenger limit;
include Clef/Flash, DJev DiffusionGemma and smaller trained alternatives subject to
faithful local runtime fit. See [model tournament](decision-models.md#fast-broad-screening-2026-10-15).

Executed follow-up: [multi-book hierarchy prototype and partial screen](local-hierarchy-screen-2026-10-15.md).
Supplied-structure validation is tested; actual PDF hierarchy discovery and app
integration remain pending. Three existing references reached inference; new-model
preparation and the broad tournament remain unfinished.

## 0. Discovery and build-versus-integrate decision

- Review existing companions and decide standalone application versus integration.
- Confirm exact games, editions, manual languages/formats, official sheet assets, and first playable system.
- Confirm player/GM scope, deployment, hardware, privacy preferences, and desired sheet fidelity.

## 1. Rust/htmx foundation

- HTTP application, HTML templates, local configuration, SQLite migrations.
- System registry and internal rules-adapter contract.
- Character create/load/edit/save with server validation and clear coverage indicators.
- Start with a clearly labelled development schema until a real edition is selected; do not claim a generic form is an official sheet.

## 2. First playable uploaded game package

- Use the Owner's selected manual/edition and uploaded sheet as original references.
- Explicit template bindings, canonical field/value context, model tool dialogue and
  source-linked semantic review use generic rendering/versioned definitions; no
  image-field discovery requirement or new per-game Rust path.
- Test resources, inventory, repeated entries, relevant actions and spells/powers
  as actual uploaded-sheet requirements, not bespoke compiled controls.
- Official-layout rendering/export as specified in discovery.
- Recreate the official sheet structure in adaptive HTML/CSS, with expandable text and repeated entries; treat print/export separately.
- Generic structural/revision/permission checks stay deterministic. Models produce
  advisory findings and proposed changes; user confirmation applies displayed values.
- Compare original field/layout coverage and actual table tasks before expanding.

## 3. Situational companion

- Capture explicit situation facts and player intent.
- List available options, costs, prerequisites, uncertainties, and cited explanations.
- Include combat, social, and exploration contexts as prioritized by the user.
- Explicit house-rule overrides and GM adjudication.

## 4. Manual ingestion and model evaluation

- Import PDFs/text with durable document/page provenance.
- Edition-isolated lexical retrieval, then benchmark hybrid retrieval.
- Page/section review states, visual comparison with source PDFs, semantic action-symbol recovery, and versioned corrections before verified retrieval.
- Optional instance-owner-key commercial-model comparison of original page images against extracted blocks; model findings are suggestions and do not auto-approve evidence.
- Provider-neutral typed decision interface; benchmark Kev and AnyJev/Tacit on held-out RPG cases before choosing a local model.
- Compare prototype outputs with Rust/native-runtime readouts; evaluate Vulkan acceleration and CPU fallback on the initial AMD GPU target.
- Optional generative explanations through a separate provider adapter.
- Final broad vendor support through an existing Rust abstraction (evaluate genai/Rig), with explicit per-model vision/JSON/embedding capabilities and owner credentials.
- Evaluate answer grounding, abstention, latency, and hardware fit on real manuals.
- Execute staged [accuracy experiments](accuracy-experiments.md#experiments-and-promotion-gates):
  source-checked evidence, lexical/hybrid retrieval, oracle-evidence controls,
  focused expandable sheet context, generic arithmetic and complete typed patches.
  Retain improvements demonstrated on held-out cases, with false-supported
  outcomes and assistance coverage reported together. These are planned, not run.

- 2026-10-15 decisions (user) from the assessment: tool-calling loop with host-filled metadata (benchmarked first), declarative derived-field formulas evaluated in Rust, then Rust FTS5 with a diacritic-insensitive tokenizer plus vector/RRF retrieval. Layered ≥100-case evaluation with a ≥90% correct / ≤2% false-supported bar for model-dependent product work. See [harness-assessment-2026-10-15.md](harness-assessment-2026-10-15.md).

## 5. Expand the initial game set

- Prioritize runtime game definitions/model-assisted wiring and a generic field renderer so an unknown game does not require new native Rust code. Treat the current MÖRK BORG implementation as the evaluation/compatibility fixture.

- Add the remaining selected D&D, Pathfinder, Vampire, and MÖRK BORG editions.
- Validate that the shared adapter contract handles genuinely different mechanics.
- Publish an honest field/rule/action coverage matrix for every adapter.

## 6. Plugin system

- Versioned manifests: system/edition ID, plugin version, host API version, schema version, sources, and capabilities.
- Declarative packs for sheet mappings, reference data, and simple supported calculations.
- Versioned WASM or subprocess execution for richer rules adapters; choose after prototyping the multi-system contract.
- Host-controlled storage, rendering, capability access, resource limits, and validated plugin outputs.
- Plugin discovery/loading, compatibility checks, data migrations, and actionable diagnostics.
- Author documentation, example plugin, and contract conformance tests.

## 7. Campaign and interoperability features

- Shared-instance user management and campaign membership are user-confirmed future work. Distinguish technical deployment **Owner** from campaign **Master**; players and their campaign Master can edit the relevant characters. Do not grant gameplay editing merely from deployment ownership.
- Full transactional sheet-edit history with reconstructible versions and rollback as a new revision, covering every native/generic write path.
- Character Delete button performs logical deletion; restoration is a separate permission. Physical purge is Owner-only, with retention/history/backups and rollback exceptions agreed before implementation.
- Settle visibility, role grants/revocation, custody transfer, multiple Masters and delete/restore/rollback policy; deliver reviewed authentication/authorization before network sharing. See [collaboration/history plan](collaboration-and-history.md).
- Proposed campaign-private/Master-only knowledge; visibility policy requires confirmation.
- Sheet import/export and optional Foundry interoperability.
- Packaging, backups, localization, and deployment appropriate to the chosen audience.
