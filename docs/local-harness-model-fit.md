# Local text model: initial fit assessment

Assessed2026-10-15. Documentation-based candidate assessment only: no runtime/model
installed, model weights downloaded, inference performed or speed/quality measured.
This historical assessment preceded the now-executed
[local RAG benchmark](local-rag-benchmark-2026-10-15.md); use that report for actual
measurements and remaining quality limitations.
The user reports likely no personal OpenAI API credits and an Anthropic work account
unsuitable for this personal project; neither account nor credentials were inspected.

> **2026-10-15 update:** newer local candidates (Qwen3.6-35B-A3B with MoE CPU
> offload, Qwen3.5-9B, Gemma 4) and a proposed tool-loop wiring are recorded in
> [harness-assessment-2026-10-15.md](harness-assessment-2026-10-15.md#local-model-candidates-newer-than-the-benchmarked-qwen3-not-measured-here).
> They are not measured here.

## Purpose

The selected model would dialogue with the sheet, read authorized canonical state
and relevant rule text, ask questions and semantically review/propose edits. This
is not primarily extraction double-checking and requires no vision capability.
Keep provider protocols behind a maintained Rust abstraction when integration starts.

## Hardware and credible starting point

Recorded host: Ryzen5 1600, about32GiB RAM, RX6650XT/about8GiB VRAM, previously
working Vulkan discovery. Discovery alone does not establish runtime compatibility.

Start evaluation with a quantized **Qwen3-4B** using a containerized **llama.cpp
Vulkan** runtime; compare **Qwen3-8B** if memory and measured usability permit.
These are candidate baselines, not claims to be the best current models. Weight
memory is only part of fit: context/KV cache, buffers, driver and desktop VRAM
also matter. Begin with bounded short context, concurrency one and output caps.
Model non-thinking/thinking mode and the exact chat template must be selected
explicitly; do not let unbounded reasoning consume the interaction budget.

Expected feasibility: short explicit source-grounded edits and structured proposals
are plausible. Multi-step interpretation, long manuals, incomplete rules, unreliable
tool use and confident unsupported answers are risks to measure, not solved by
JSON-constrained syntax. No tokens/sec, success rate or latency estimate is claimed.

## Maintained runtime/library fit

- llama.cpp documents Vulkan and a Vulkan Docker build path. This fits the existing
  AMD/Mesa environment without requiring host ROCm installation. Pin/review an exact
  maintained revision/image/license and its actual runtime dependencies before the
  technical experiment; upstream master documentation is not an artifact pin.
- Ollama is a reasonable alternative: RX6650XT is absent from its listed ROCm
  support table, **but current upstream also documents Vulkan**. Do not conclude it
  cannot accelerate this GPU. Prefer the smaller explicit llama.cpp experiment to
  separate device/backend/model behavior from scheduler/service behavior initially.
- Official Qwen3-4B and8B cards declare Apache-2.0, thinking/non-thinking modes and
  llama.cpp/Ollama support. Model-card tool claims are not task-quality evidence.
  Pin the exact quantization publisher/revision/hash/license before downloading;
  do not assume a third-party quantization's provenance from the base model name.
- Keep models/runtime separate from the lightweight shipping Rust application and
  outside public source/runtime archives. Install tooling only inside containers.
  No custom vendor protocol, new application scripting language or hosted fallback.

## Minimal next experiment

Use `examples/sheet-harness-demo`'s rules and canonical values, not expected answers,
as model input. Test exact edits, coupled armor/pace proposals, conflict, incomplete
rule, missing fact, stale proposal and note injection. Repeated runs and held-out
paraphrases must count all failures/retries. Evaluate factual/citation support,
unintended changes, questions/abstention and malformed responses separately from
syntactic output validity. App authorization/stale checks stay deterministic.

Measure cold/warm total response latency, output/context size, RAM/VRAM and failure
recovery; CPU fallback is a separately labelled measurement, never hidden as GPU
success. A successful synthetic probe is not a usable Mimik integration or a
real-game viability certificate. Independently review the pinned benchmark plan
before installing/downloading/running; then select the model/harness path from results.

## Primary documentation

- [Qwen3-4B official card](https://huggingface.co/Qwen/Qwen3-4B)
- [Qwen3-8B official card](https://huggingface.co/Qwen/Qwen3-8B)
- [llama.cpp build guide, Vulkan section](https://github.com/ggml-org/llama.cpp/blob/master/docs/build.md#vulkan)
- [Ollama GPU support, including Vulkan](https://docs.ollama.com/gpu)

This is a library/runtime selection decision for a candidate probe, not a new
shipping dependency choice. Recheck selected version/license/API/portability and
dependency/runtime costs in the bounded technical plan.
