# Discovery and decisions

## Confirmed requirements

- Multi-book clarification (2026-10-15): a game package can enable player/GM,
  bestiary and expansion manuals together. Preserve independent book identities,
  editions/revisions and trees within a source set; cross-book references and
  explicit conflict/precedence policy must not be replaced by title-based merging.
  Optional supplied-structure prototype and partial screen are recorded in
  `local-hierarchy-screen-2026-10-15.md`; no extraction/app integration is implied.

- Further accuracy clarification (2026-10-15): routine preparation must scale beyond
  full human rereading; test frontier-model extraction review for reduced effort.
  Preserve global chapter/section/class/subclass hierarchy even across hundreds
  of pages. User wants many model candidates screened quickly, explicitly Clef and
  Jev-like DiffusionGemma; prior one-challenger recommendation superseded. Automated
  review is not automatic human/source approval. Protocol in `accuracy-experiments.md`
  and `decision-models.md`; newly researched candidates have not been run.

- Accuracy-first clarification (2026-10-15): sheet-as-context/harness is a high-level
  direction; implement and measure generic context/tools/retrieval improvements,
  discard failed experiments, and consider other local model types. The user
  offers the old `~/temp/pathfinder` experiment as rule-heavy test material and
  reiterates original-page frontier-model extraction review. A bounded Pathfinder
  stress test alongside the lighter MÖRK BORG trial is the current recommendation,
  not a claim that either corpus is loaded/reviewed. See `accuracy-experiments.md`.

- Latest policy clarification: normal deployment Owner account is administrative
  only, not a player or campaign Master; use a separate enrolled player account.
  Explicit development-only Owner gameplay override may retain test workflows.
  Password policy confirmed: Owner minimum15, players minimum12 Unicode scalars,
  no mandatory character categories; existing maxima/PHC costs remain unchanged.

- Prototype records are disposable: wipe them if needed for strong constraints
  rather than weakening the intended schema/permissions to preserve test data.
  No implicit compatibility exception is permitted in new generic definitions.

- Shared-instance user management is planned. Players control their characters;
  campaign Masters may edit campaign characters. Technical deployment Owner and
  campaign Master are distinct roles. Full sheet edit history/rollback, logical
  character deletion and Owner-only physical purge are required future work; see
  `collaboration-and-history.md` for permission/retention questions still open.
- Master roles and player membership are campaign-scoped. Every new sheet belongs
  to a campaign bound to an exact game/edition ruleset; cross-game and cross-edition
  sheet creation is rejected. Existing prototype sheets require explicit future
   campaign assignment.

- Campaigns normally have one Master, their creator; additional temporary or
  long-term Masters are possible. The original creator promotes/demotes campaign
  players; automatic expiry is not specified. Characters remain personal and
  transfers are chosen by the owning player. After departure/removal the player
  retains read-only sheet access, the sheet remains linked to its exact campaign
  and ruleset, and neither player nor Masters may edit/progress it. Current
  Masters of that linked campaign retain read-only sheet/history access; demoted
  or revoked Masters gain no historical grant. No automatic detachment,
  reassignment or reactivation is implied.
- Owner may disable accounts and issue password-reset links invalidating existing
  sessions, without implicit sheet/history access. Enrollment is Owner-invited;
  sheets/history are private to the player and campaign Masters. Shared deployment
  targets a private LAN with optional Owner-configured TLS; the current service
  remains loopback-only until the required sharing gates pass.

- Updated architectural clarification: use the model to wire owner-uploaded sheets/manuals into runtime field/layout definitions and source-grounded validation/inquiry prompts. Avoid mandatory native per-game verticalization; current MÖRK BORG code is an exploration fixture.

- A GDR/TTRPG companion extensible to many existing games.
- Initial families: D&D, Pathfinder, Vampire (title ambiguous), MÖRK BORG.
- Rust application; web UI using htmx is acceptable.
- Editable, checked sheets with each game's official layout.
- Situational advice with contextual rules and action/spell explanations.
- Investigate a small Qwen instance and RAG over available manuals.
- Include a plugin system in the roadmap.
- Research existing tools before implementation, ask questions, and document the project.
- Mimik will be open source; current license is AGPL-3.0-only (see `licensing.md` for obligations).
- Standalone, sheet-first player companion for the physical table: reduce manual lookups and overloaded sheet notes, rather than reproduce a full virtual tabletop.
- This machine is the initial hardware target; prioritize broad compatibility and investigate Vulkan acceleration with CPU fallback.
- Python is acceptable for prototypes; Rust is preferred for the polished, lightweight final product.
- Manuals are local in `~/temp`; Pathfinder sources and an older low-quality RAG experiment are in `~/temp/pathfinder`. The experiment is not an implementation template.
- Extracted Markdown must be checked against the original PDFs before becoming verified evidence. See `sources.md` and `source-quality.md`.
- First playable system: MÖRK BORG. Initial implementation targets the supplied Italian 2025 printing; other source versions remain distinct.
- Deployment is local, not publicly exposed. Instance owners supply lawfully obtained manuals and are responsible for their content/use; no blanket transfer of every possible legal claim is promised. See `local-setup.md`.
- Extraction should use a layout-aware system similar to Marker, preserving tables without forcing OCR on every textual PDF. Graphical MÖRK BORG/V5 sources need visual verification.
- Optional commercial-LLM API review with an owner-supplied key is acceptable. Compare original page images and extraction, not assumed training knowledge. No provider/model/key configured yet.
- Accuracy goal: full reviewed coverage for supported rules and no unresolved known discrepancies; do not claim universal 100% automated correctness.
- Global rule: use maintained crates/libraries rather than reinvent infrastructure where a suitable dependency exists; record compatibility/fit decisions.
- Final provider support should span vendors through an established Rust abstraction; expansion is deferred to the final-product roadmap. The owner reports an OpenAI API key in `.env`; its contents are not printed/read back.
- Official sheets will be recreated as adaptive HTML/CSS with recognizable original grouping and expandable long-text fields, not constrained to PDF boxes.

## Research outcome

Existing tools cover substantial parts of the idea. The user clarified the reasons for a standalone Mimik: open-source ownership and a sheet-first physical-table workflow. Proceed with Mimik development. No game edition has been silently selected.

## Questions requiring answers

1. **Direction — answered:** Build standalone, open-source, sheet-first Mimik.
2. **Editions and priority:** Which D&D edition/year, Pathfinder edition/remaster status, and Vampire title/edition? Which one should be playable first? Which MÖRK BORG books/supplements apply?
3. **Source material — answered in part:** PDFs found locally, mainly Italian, with an English MÖRK BORG source too. Several have text layers; D&D shows OCR defects. Official sheet candidates exist but need attribution/layout review.
4. **Layout fidelity — answered:** Recreate official sheet structure in adaptive HTML/CSS with long-text expansion and mobile reflow. Print/export priorities remain to be decided.
5. **Audience/deployment — answered in part:** Private-LAN sharing with optional TLS, Owner-invited accounts, player/Master-private sheets/history, creator-controlled additional Masters, player-controlled transfers, frozen campaign-linked sheets after departure/removal with retained read-only access, and Owner disable/reset powers are confirmed. Current service remains loopback-only; delete/restore/rollback permissions and retention/purge details still need decisions before their respective implementation slices.
6. **Inference hardware — partly answered:** This Linux machine: Ryzen 5 1600, about 32 GB RAM, RX 6650 XT/about 8 GB VRAM; Vulkan discovery works. Prefer broadly compatible native/Rust shipping inference. Acceptable latency and local-only policy remain open.
7. **Situation input:** Manual structured facts/free text initially, or integration with a VTT/session state? Is advice mainly combat, or equally social/exploration?
8. **Validation policy:** Strict character-building legality, live resource checks, warnings for inconsistencies, and explicit house-rule overrides—which are required initially?
9. **First release:** Prioritize character creation, importing existing characters, live sheet tracking, action/spell advice, or manual Q&A? Are dice rolling, inventory, leveling, and campaign notes needed?
10. **Distribution/localization — partly answered:** Open-source, AGPL-3.0-only. English, Italian, or both for UI and answers?

## Proposed decisions awaiting confirmation

### Follow-up: decision models and session continuity

The user requested evaluation of Jev-style decision models as an alternative to Qwen and a persistent checkbox TODO file. `TODO.md` is the session entry point. Primary-source research is in `decision-models.md`; recommendation is a typed decision service alongside deterministic rules and retrieval, with generative explanations optional. No model provider has been selected. Python prototypes are acceptable; investigate native inference for shipping (see `hardware-and-runtime.md`).

- Deterministic rules own calculations and state; typed decision models can interpret/rank bounded options, and a generative model is an optional explanation layer.
- Axum + Tokio + Askama + SQLite; server-rendered HTML enhanced by htmx.
- Version-pinned per-edition adapters with a visible coverage matrix.
- Start with one edition and prove the abstraction across different games before publishing an executable plugin API.

## Experience-first direction (2026-10-15)

- User pauses further admin investment until the core usage experience proves viable.
- First experiment: owner web template + manual text → canonical sheet context →
  text-model dialogue, bounded read/rule lookup/questions/proposed patches and
  explicitly confirmed edits. Mimik is a harness for model–sheet interaction.
- User rejects vision-model wiring; JavaScript may provide browser interaction,
  with no new scripting language or rules DSL. HTML/CSS bindings are proposed,
  not yet a confirmed upload format; current values come from canonical state.
- User clarifies local inference plus RAG over extracted text is a core requirement.
  The API key is solely for optional OCR/extraction double-checking, never ordinary
  Mimik use. Do not use the work Anthropic account or assume paid hosted inference.
- Core must remain game-agnostic: uploaded templates/versioned definitions and
  canonical sheet context, not new compiled game logic or built-in sheet layouts.
- Structural checks/permissions/revisions stay deterministic; source-linked model
  findings are advisory and values change only through explicit user action.
- Execution mode/provider, actual matching assets, review-before-save behavior and
  small usability/effort/latency/cost scorecard remain to be settled before coding.
- See `experience-validation-plan.md`; this is design, not a tested AI integration.

## Harness assessment decisions (user, 2026-10-15)

User decisions on the proposals in [harness-assessment-2026-10-15.md](harness-assessment-2026-10-15.md):

- **Tool-calling loop:** adopt it. The model calls small tools (`search_rules`,
  `read_fields`, `propose_change`, `ask_user`). Rust fills old values and revisions,
  renders the diff and requires explicit user confirmation. This replaces the
  one-shot JSON patch contract.
- **Derived values:** declarative formulas in the definition, evaluated server-side
  in Rust. Template JavaScript was rejected: JS runs only in the browser while the
  model runs server-side, so results would have to round-trip, which is convoluted
  and fail-prone. This is a deliberate, scoped exception to "no rules DSL": a
  minimal expression language for derived fields only, not a general scripting
  language. Per `AGENTS.md`, evaluate maintained expression crates before writing
  one; the user suggests a fast `winnow` parser if a custom one is needed.
- **Process:** lighter tier for disposable experiments, recorded in `AGENTS.md`.
- **Ingestion vision:** acceptable during offline PDF ingestion (layout parsers such
  as Marker/Surya, Docling, MinerU). The runtime chat model stays text-only.
- **Next action:** rebuild the Python benchmark as a low-temperature tool loop on
  the existing demo first, to test the contract hypothesis before app retrieval work.
- **Models for the first tool-loop run:** Qwen3.6-35B-A3B and Qwen3.5-9B side by side,
  plus the existing Qwen3-4B-Instruct-2507 baseline.
- **Admin work:** continues in parallel. It is not paused.
- **Accuracy bar:** on at least 100 held-out cases, at least 90% correct outcomes and
  at most 2% false-supported (unsupported claims presented as supported). This gates
  model-dependent product work, not admin work.

## Environment

- Workspace initially empty: `/home/marco/Progetti/mimik`.
- Rust/Cargo 1.95.0 available.
- Manuals inventoried locally; source quality and exact printing/errata remain to be audited. Sheet candidates are not yet mapped.
- Hardware inspected; see `hardware-and-runtime.md`.
