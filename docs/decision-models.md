# Jev-style decision models

Initial documentation review:2026-10-11. Actual local AnyJev/Kev measurements now
exist in [2026-10-15 decision benchmark](local-decision-benchmark-2026-10-15.md).
Those18-case synthetic results are not general RPG accuracy/calibration claims.

> **2026-10-15 assessment:** the generative-patch failures in the local RAG benchmark
> are hypothesised to stem mainly from the one-shot contract rather than model
> choice (unmeasured). Newer
> generative candidates and a tool-loop proposal are in [harness-assessment-2026-10-15.md](harness-assessment-2026-10-15.md). Typed-decision
> screening below is a separate track and remains valid as recorded.

## Recommendation

Evaluate a small local decision model for bounded interpretation and ranking tasks. Keep the architecture provider-neutral. A decision model complements deterministic rules and source retrieval; it does not replace either, and it is not a free-form explanation/chat model.

Qwen versus Jev-style is not necessarily a choice of base model: Kev and several AnyJev/Tacit models are built on Qwen. The important distinction is the trained task and output contract.

## Candidates

| Candidate | Documented behavior | Fit and caveats |
| --- | --- | --- |
| [Jev / TypeSafe AI](https://typesafe.ai/) | Hosted System One API with typed decisions and confidence/probabilities | Useful reference contract. The reviewed source does not establish downloadable Jev weights or local inference. Typed output and vendor “zero hallucinations” language do not prove decision correctness. |
| [Kev](https://github.com/jaredpalmer/kev) | Apache-2.0 project with downloadable decision models, yes/no, choice, score, and local `POST /v1/systemone`; documented sizes 0.8B, 4B, 9B, 27B | Strong first candidate for a local typed-decision endpoint. Built on Qwen; current documented server is Python. Start evaluation with 0.8B and 4B if hardware permits. Published benchmarks are not RPG accuracy measurements. |
| [AnyJev / Tacit](https://github.com/nokia-applied-research/AnyJev) | Apache-2.0 project; training-free readout from open causal models and self-distilled Tacit checkpoints; typed decisions and probabilities; optional capped reasoning fallback; HTTP gateway | Alternative for testing existing models or adaptive reasoning. Training-free L0 averages option rotations, which can require multiple evaluations. The project explicitly distinguishes bias correction from labelled calibration. |

Primary README sources: [Kev](https://raw.githubusercontent.com/jaredpalmer/kev/main/README.md), [AnyJev](https://raw.githubusercontent.com/nokia-applied-research/AnyJev/main/README.md).

## Proposed responsibilities

1. **Game-agnostic harness:** own permissions, field types, version checks, source availability and explicit application of displayed changes. Do not require compiled game-specific rules or templates; model decisions cannot establish legality or automatically write values.
2. **Retriever:** find exact-edition evidence with document and page provenance.
3. **Decision model:** classify source support/conflict/missing context, help select relevant passages, and rank supplied candidate IDs against an explicit goal. Candidates may be user/model proposals or reviewed runtime data, not implicitly legal actions from a hardcoded per-game engine.
4. **Explanation layer:** render deterministic descriptions and source excerpts initially. Add an optional generative model for conversational explanations if needed.

Example: given “I want to escape without harming the guard,” user/model proposals
are kept distinct from supported actions; retrieved evidence, explicit facts and
reviewed runtime data describe their known costs/prerequisites. A decision model
may rank proposal IDs or return missing-context/unsupported findings. It cannot
make the proposals legal, waive a prerequisite or spend resources. Tactical
preference depends on goals and incomplete facts, not objective legality.

## Rust integration

The application and UI remain Rust/htmx. The user accepts Python for prototypes and prefers Rust for a lightweight final product. A Rust HTTP client may call a local reference model service during evaluation; the shipping plan should investigate Rust/native inference with CPU fallback and Vulkan support. Do not assume specialized checkpoints run in ordinary Ollama/llama.cpp simply because their base is Qwen. See [hardware and runtime direction](hardware-and-runtime.md).

Use separate `DecisionProvider` and `ExplanationProvider` boundaries. A decision request carries exact-edition evidence, known/unknown facts, a typed question, and allowed option IDs. Validate returned IDs, probability ranges, completeness, and normalization according to the provider contract. Persist model/checkpoint identity and evidence references for reproducibility. Provider failure or insufficient evidence should leave deterministic sheet functionality available.

## Evaluation before selection

Latest executed subset: actual AnyJev raw/L0 on two Vulkan GGUF parents and trained
Kev0.8CPU reference, focused state+retrieved rules, option permutations, wrong-
confident decisions and native cold/warm cache invalidation. L0 improved8B but
reduced4B accuracy in the tested configuration. NativeKev caching reduced median
6.52→2.12s with unchanged probabilities; the current AnyJev GGUF backend clears
KV per prompt. Tacit, neural phase decomposition, real manuals and app integration
remain untested. Keep future comparisons local; API usage is optional OCR review only.

- Build source-reviewed, edition-specific labelled situations with expected intent, relevant evidence, legal candidates, and missing facts.
- Include ambiguous requests, unsupported options, house rules, contradictory facts, different option orders, and English/Italian if selected.
- Compare deterministic heuristics, a small general model, Kev, and AnyJev/Tacit on the same held-out cases.
- Measure task accuracy, false-confident errors, abstention behavior, calibration, option-order sensitivity, latency, and memory on the actual hardware.
- Evaluate ranking against explicit player goals separately from rules legality.
- Pin model revisions and set thresholds using held-out RPG data. A confidence value is not a correctness guarantee; Kev explicitly states its API `confidence` field is not a measured accuracy rate.
- Choose a provider only after this comparison; no fine-tuning or model downloads are required during discovery.

## Fast broad screening (2026-10-15)

Executed follow-up: [hierarchy prototype/partial screen](local-hierarchy-screen-2026-10-15.md).
Three existing GGUF references reached new inference; two completed48observations,
one47/48withdeadlinefailure. Mapika2B transfer incomplete, Tacitqueued/notstarted,
no new trained-candidate result. Broad candidate selection remains pending.

User explicitly wants many models tested quickly, including Clef and a Jev variant
of DiffusionGemma. This supersedes the earlier one-reasoning-challenger proposal.
Public source/card inspection below is **not inference or a completed model
selection**. Ordinary usage/screening remains local and text-only; multimodal
backbones may be tested with text inputs without adding vision to sheet interaction.

### Newly inspected candidates

Primary sources accessed 2026-10-15, mutable `main` cards; immutable artifact/code
revisions and complete licenses must be pinned before execution:

| Candidate | Source-backed capability | Admission/fit finding, not measured accuracy |
|---|---|---|
| [Clef-Flash](https://huggingface.co/Cloudflare/clef-flash) | Qwen3.5-9B backbone plus joint schema head; choice/noul/score probabilities, no free-form generation; Apache-2.0 declared | High-priority compatibility probe. Card tested Torch 2.11/Transformers 5.10.2 on H200; native-head-preserving quantization and AMD/CPU path unverified. Plain backbone GGUF is not Clef. |
| [Clef](https://huggingface.co/Cloudflare/clef) | Qwen3.8-27B with the same specialized decision design; Apache-2.0 declared | Larger quality comparator if a faithful local runtime fits. Do not assume 27B native weights fit our 32GiB RAM/8GiB VRAM. |
| [DiffusionGemma DJev v10 step160](https://huggingface.co/snowicarus/diffusiongemma-26b-djev-v10-step160) | Full fine-tuned export; evaluated native Turbo-dLLM structured path with `think=0`, four samples, 64-token canvas | Preserve native sampler/settings. 26B-A4B does not mean 4B resident weights. AMD/offload feasibility unverified; no ordinary generic-generate substitute. Apache-2.0 declaration and linked/inherited base terms require artifact/base/runtime license verification. |
| [DJev third-party NVFP4](https://huggingface.co/SVPG-Labs/diffusiongemma-26b-djev-v10-step160-NVFP4) | Card reports 18GB artifact, ~22.6GB VRAM on RTX5090, specialized patched vLLM/djev path and one-read evaluation | Documented tested configuration exceeds our GPU; NVFP4 recipe is not evidence of AMD compatibility. Different samples/runtime from source card: separate configurations, not interchangeable benchmark numbers. |
| [Mapika decider-2b](https://huggingface.co/Mapika/decider-2b) | Native typed slot readout, current card v11, per-type temperatures, Torch/Transformers; Apache-2.0 declared | Promising smaller candidate; card reports 3.8GB BF16 weights, English-only limitations, hard-case overconfidence and regressions. Native runtime/CPU/AMD cost must be tested, not inferred from CUDA speed. |
| [Mapika decider-4b](https://huggingface.co/Mapika/decider-4b) | Current card v2.1, 8.4GB BF16 weights, native typed readout and per-type temperatures | Preserve selected revision and decider-ai >=1.4.0 temperature behavior. Hard-choice calibration and form-filling regressions explicitly documented; newer version is not uniformly better. |
| [Tacit-4B](https://huggingface.co/morriszjm/Tacit-4B) | Qwen3-4B self-distilled typed decisions, optional adaptive reasoning; Apache-2.0 declared | Reuse native trained prompt/readout. Review pinned helper code rather than execute mutable remote-code examples. Adaptive mode is a separately budgeted configuration. |
| [Kev-4B](https://huggingface.co/jaredpalmer/kev-4b) | LoRA + pointer head on Qwen3.5-4B-Base, shared-state cache; Apache-2.0 declared | Published serving footprint 14.3GB GPU, validated context 8,192 despite serving longer inputs. English-focused; faithful CPU/quantized path needs a bounded feasibility test. |
| [Phi-4-mini-reasoning](https://huggingface.co/microsoft/Phi-4-mini-reasoning) | 3.8B text generation, math-reasoning focus; MIT declared | Different model-role challenger, not a native decision head. Card explicitly tests math rather than general downstream work and supports English; Italian RPG/tool behavior remains unverified. |

Clef [announcement](https://blog.cloudflare.com/clef-decision-models) advertises
64k context; release helper cards default to 16,384 max-length. Distinguish model
capacity, helper default and actually tested budget. Its benchmark tables are
publisher-reported comparisons, not Mimik results; the exact DJev configuration
in those tables was not independently established here. API compatibility says
nothing about weight/runtime compatibility, calibration transfer or rule fidelity.

For **both Mapika 2B and 4B**, preserve the current per-type temperature maps through
decider-ai >=1.4.0 (or the equivalently pinned supplied inference subset); older
helpers ignore those maps. Pin the selected model/version and helper together.

### Tournament protocol to implement next

Initial pool: existing Qwen3-4B-Instruct and trained Kev0.8 references, Tacit4B,
Mapika2B/4B, Kev4B, Clef-Flash, Clef, DJev and Phi-mini-reasoning: ten candidates.
Historical Qwen3-8B results remain a reference, not a new screen result. The NVFP4
DJev build is an alternative configuration of the DJev candidate, not another
independent model. Do not promise ten successful local executions: admit what can
run faithfully and report the rest as blocked with a concrete reason.

1. **Cheap admission screen first:** artifact/base/head licenses, immutable pins,
   maintenance and native-runtime support; estimate weights + heads + conversion
   scratch + load/context RAM/VRAM and disk. Existing runtimes first. No downloads
   before the numerical budget and independent execution-plan review. Timebox a
   candidate rather than spend days adapting it. No host installs/hosted fallback.
2. **Shared development screen:** freeze a small common decision set from existing
   inspected fixtures plus new hierarchy/scope cases before results. Same logical
   state/evidence/questions/options for common supported tasks; native encodings
   may differ and must be logged. Record capability-specific denominators: native
   decision scoring, hierarchy-evidence judgment, and generative tool/patch loop
   are separate tracks. Decision-only models are not penalized for no chat output.
3. **Bounded cost:** the execution plan must set numerical per-candidate and total
   setup/download/disk/runtime/call/context limits before execution. Count rotations,
   adaptive reasoning and DJev samples as real compute; preserve failures/timeouts,
   not just successful timings. CPU/offload results are labelled and compared on
   their actual end-to-end cost, not copied H200/5090 latency.
4. **Select top 2–3 per relevant role:** predeclare unsupported-support errors,
   assistance coverage and resource-cost tie-breakers; do not select on one accuracy
   percentage. A small screen is a triage signal, not proof of superiority. Known
   fixtures are development evidence, not a general accuracy estimate.
5. **Fresh finalist test:** source-checked MÖRK BORG/Pathfinder cases, Italian as
   well as English synthetic controls, source hierarchy/exception traps, evidence
   ablations and option permutations. Freeze held-out cases/selection rules; no
   winner announced before these runs. Keep an alternate if a finalist regresses.

Statuses must distinguish documented, admitted, executed, failed-to-run/blocked,
incorrect answer and capability not applicable. The initial card-research pass
downloaded no weights; the execution follow-up left Mapika preparation partial
and ran only existing reference weights. Reuse the existing benchmark ledger,
deadlines/distribution validation/cache checks and maintained native implementations;
custom adapters only for demonstrated interface gaps, independently reviewed.
