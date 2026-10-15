# Target hardware and inference direction

Inspected locally on 2026-10-11 using `lscpu`, `free -h`, `lspci -nn`, `vulkaninfo --summary`, and the GPU's sysfs VRAM counter.

## Current machine

- Linux x86_64.
- AMD Ryzen 5 1600: 6 cores / 12 threads, AVX2 available.
- Approximately 32 GB RAM (`free` reports 31 GiB).
- AMD Radeon RX 6650 XT (confirmed by Vulkan), approximately 8 GB VRAM.
- Mesa RADV driver; Vulkan device API 1.4.354, instance 1.4.357.
- Vulkan inspection tool, `glslc`, and CMake are installed.

Vulkan device discovery succeeded. No inference workload, model memory fit, or performance has been verified.

## Confirmed user constraints

- This machine is the initial target; aim for broad hardware compatibility.
- Prefer a vendor-neutral Vulkan backend over requiring a particular GPU vendor.
- Python is acceptable for prototyping; prefer Rust for a polished, lightweight final product.

## Proposed runtime path

Keep inference optional so sheets and implemented rules remain usable without GPU hardware or a model. Require a CPU fallback and consider Vulkan acceleration on supported devices. Avoid requiring CUDA/ROCm as the default distribution path.

Evaluate a Rust application calling a native inference library through bindings. [llama.cpp](https://github.com/ggml-org/llama.cpp) documents a C API, CPU and Vulkan builds, and multiple other backends in its [build guide](https://github.com/ggml-org/llama.cpp/blob/master/docs/build.md). This would mean Rust application code with a C/C++ inference dependency, not an entirely Rust implementation. It is a candidate, not a chosen or integrated runtime.

For AnyJev-style training-free decisions, first check whether the selected native runtime exposes the logits/readout needed to reproduce the intended decision semantics. For Kev, verify specialized pointer-head, tensor, attention/cache, and checkpoint-conversion support rather than assuming ordinary Qwen GGUF support suffices. Compare any port against the reference Python implementation on identical inputs before trusting it.

Native-runtime compatibility is a selection criterion alongside RPG accuracy. If a Python prototype is the strongest candidate, use it to establish a baseline, then measure the cost and feasibility of a Rust/native implementation before committing to the shipping model.

An 8 GB GPU makes small/quantized models reasonable candidates to investigate, but final memory needs also depend on context, caches, precision, runtime, and concurrent requests. No fit or latency guarantee is inferred from parameter count alone. Apple platforms may benefit from a native Metal path; broad compatibility should not require Vulkan on every platform.
