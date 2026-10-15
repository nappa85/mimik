# Rust provider abstraction research

Reviewed upstream documentation: 2026-10-11. Provider integration is for the final product; the current Python reviewer remains an OpenAI-compatible prototype. No crate has been added or live provider capability verified in this step.

## Candidates

| Library | Documented capabilities | Fit |
| --- | --- | --- |
| [genai](https://github.com/jeremychone/rust-genai) | Unified native-protocol chat/streaming, vision, structured output, major commercial vendors, local Ollama and custom endpoint/auth support | First candidate for a focused review/explanation client without choosing a broader orchestration framework. Upstream package declares MIT OR Apache-2.0. |
| [Rig](https://github.com/0xPlaygrounds/rig) | Provider-neutral completion/embedding contracts, multiple vendors, multimodal messages, vector-store integrations, separable core/agent runtimes | Candidate if shared retrieval/embedding/workflow infrastructure is useful. Its current repository includes experimental [`rig-typesafeai`](https://github.com/0xPlaygrounds/rig/tree/main/crates/rig-typesafeai) Jev judgments. Check release availability, maturity, and selected component licenses before depending on it. |
| [llm](https://github.com/graniet/llm) | Unified chat/completion traits, feature-gated providers, vision, selected structured-output support, retry/evaluation functionality | Additional candidate if its API or supported providers fit better. Check selected release license and multimodal behavior. This is graniet's provider library, not the archived rustformers local-inference project sharing the name. |

These are documentation findings, not an independent benchmark. Current upstream branches may include unreleased capabilities. Pin a tested stable version rather than copying main-branch APIs/provider claims into a release.

## Recommended selection process

Start by evaluating `genai` for image-based extraction review and source-backed explanations. Compare Rig's portable core if embedding/retrieval or typed Jev integration makes it a better overall fit. Use one general-provider abstraction unless a concrete unsupported capability warrants another dependency.

Do not build custom vendor protocols merely to provide Mimik's own interface. Keep only a thin domain layer for evidence, review questions/findings, decision IDs, source provenance, and application policy. API chat/vision abstraction does not replace the local specialized-model runtime or guarantee Kev/AnyJev readout compatibility.

## Final-product requirements

2026-10-15 clarification: ordinary Mimik dialogue/semantic review must work with
local inference and RAG over extracted material. Hosted API credentials were only
for optional OCR/extraction double-checking, not the usage model. Provider-neutral
Rust support must include the selected local runtime endpoint; hosted vendor breadth
is not a prerequisite for the experience trial. Keep OCR-review and usage roles
separate, with no silent hosted fallback or shared outbound character context.

- Owner-configured provider/model/endpoint and credentials; support multiple vendors as a roadmap item.
- Distinguish model roles: image-based extraction reviewer, optional explanation model, embeddings, typed decisions.
- Explicit capability checks for images, structured JSON/schema output, token/context limits, usage reporting, and local endpoints. A provider name alone is not proof that its selected model supports a feature.
- Preserve evidence/review semantics across adapters. Unsupported features should be explicit failures, not silent changes to prompts, evidence, or approval rules.
- Preserve sheet visibility/drafts if the local model is unavailable; do not label
  missing semantic review as passed. Unreviewed-save policy remains to be settled
  for the experience trial. Ordinary model dialogue/RAG must not require hosted API configuration.
- Minimize enabled dependency features; compare footprint and cross-platform TLS/runtime requirements.
- Record actual crate/version/license choice and test representative adapters before release.

## Credential handling now

The owner reports an existing `.env` with an OpenAI API key. The prototype reviewer loads `.env` through `python-dotenv` (no custom parser), accepting `OPENAI_API_KEY` or `MIMIK_REVIEW_API_KEY`. Nonempty process environment values take precedence over file values; within each source the reviewer-specific key takes precedence. Dotenv loading performs no API calls and does not execute shell expressions or mutate process environment.

`.env` and `.env.*` are ignored; only `.env.example` is distributable. No real credentials were displayed or copied into project files. The final Rust implementation should likewise use an existing configuration/secret-loading library rather than a bespoke dotenv parser.

## 2026-10-15 note

docs.rs lists a `rig` crate at 0.43 (web claim, unverified). Rig has historically
been published as `rig-core`, so confirm the crate's identity and release cadence before relying on it. The llama.cpp server
exposes an OpenAI-compatible chat/tool API, so one client path can serve local
and hosted backends. Version and licence fit are still to be checked at selection
time. Context: [harness-assessment-2026-10-15.md](harness-assessment-2026-10-15.md).
