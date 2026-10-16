# Pinned GGUF fetch and runtime support — 2026-10-15

Prototype tier (AGENTS.md): self-check plus this note; no independent review yet.
These results must not drive a product/architecture decision before review.

## Controller (TODO prerequisite)

`tools/fetch_models.py` is the outer elapsed-deadline controller. Per candidate it
records the start once; every phase — HF metadata (`curl -I`, pinned
`x-repo-commit`, `x-linked-size`, `x-linked-etag` = pinned SHA256), provenance
(README model card + HF API revision JSON), each transport attempt
(`curl -C -` sequential resume, then `aria2c -c` fallback, alternating up to four
attempts), and SHA256 verification — takes its timeout from
`screen_limits.setup_remaining(global_cutoff, original_start, now, cap_seconds)`.
A transport switch never restarts the clock; an exhausted phase is not launched;
on host timeout the container is `docker stop`ped/removed, and every container
also runs under in-container `timeout -s KILL ceil(remaining)` so the deadline
holds if the supervisor dies. A `<file>.verified.json` marker is written only
after exact size and SHA256 match; it is removed at candidate start.

Containers: `mimik-model-screen:20261015`
(`sha256:550abfbd87bbb52cb34160d30ad990c4026012828026b3c97704e040b3c5f295`,
aria2 1.37.0, curl 8.14.1), `--user 1000:1000 --cap-drop ALL
--security-opt no-new-privileges --read-only --tmpfs /tmp --memory 2g
--pids-limit 256`, only the output directory mounted; hashing runs with
`--network none`. No host installs (supervisor is host stdlib Python).

`setup_remaining` gained `cap_seconds` (default 900, so existing callers and the
927.2 s regression keep behaviour; non-positive/non-finite/bool caps rejected).

### Checks run

- Failing first: `tools/test_fetch_models.py` errored (module missing) before
  implementation. Mutation self-check: restarting the clock on transport switch,
  launching exhausted phases, and skipping the hash comparison each made exactly
  the corresponding regression fail (a/b/c).
- `docker run --rm --mount type=bind,src=$PWD,dst=/workspace mimik-ci bash -c
  'ruff check tools && ... python -m unittest discover -s tools -p "test_*.py"'`:
  `ruff check tools` passed; `ruff format --check` passes for the files changed
  here (it flags `tools/test_formula_ref.py`, another concurrent task's file);
  unittest `Ran 132 tests ... OK`. (A read-only mount makes
  `test_package` fail on `cargo package`; the writable documented mount passed.)
- Not covered by unit tests: real docker stop at deadline, live network failures.

## Pins

| | B (first) | A |
|---|---|---|
| Repo | `unsloth/Qwen3.5-9B-GGUF` | `ggml-org/Qwen3.6-35B-A3B-GGUF` |
| Revision | `3885219b6810b007914f3a7950a8d1b469d598a5` | `baec3ebee244827cda0f4557eafa8b28f7545fa6` |
| File | `Qwen3.5-9B-Q4_K_M.gguf` | `Qwen3.6-35B-A3B-Q4_K_M.gguf` |
| Bytes (x-linked-size) | 5,680,522,464 (5.29 GiB) | 20,419,565,568 (19.02 GiB) |
| SHA256 | `03b74727…52b7e8` | `671e47e0…5140c7` |
| GGUF arch (HF API) | `qwen35` | `qwen35moe` |
| License (card) | apache-2.0 | apache-2.0 |

Neither repo has a `LICENSE` file at the pinned revision (HTTP 404); provenance is
the README model card (license front matter) and API revision JSON, saved as
`<ident>.README.md` / `<ident>.api.json`.

## Budget

- Probe (15:50:49–15:51:51 UTC): 60 s bounded byte-range `curl` of A's URL in
  the container: 187,106,479 bytes, 3,118,478 B/s (2.97 MiB/s; curl exit 28 =
  the intended `--max-time`).
- Caps = size / (0.6 × 3,118,478 B/s), covering ALL phases of a candidate:
  B 3,035.9 s (50.6 min), A 10,913.2 s (3.03 h); projected total 13,949 s
  (3.87 h). Expected pure transfer at measured speed: B ≈1,822 s, A ≈6,548 s.
- Absolute global cutoff `1792097458` = 2026-10-15T20:50:58Z (now + 5 h − 60 s);
  plan fits (`fits_global: true`). Order B then A.

## Ledger and progress

`target/model-fetch-20261015/` (ignored): `ledger.jsonl` (probe, plan,
candidate_start/end, per-phase start/end/timeout/exit/bytes, hash),
`logs/<container>.log`, `supervisor.log`, `supervisor.pid`. Supervisor launched
with `setsid nohup python3 tools/fetch_models.py fetch --throughput-bytes-per-s
3118478 --global-cutoff-unix 1792097458` (PID in `supervisor.pid`).
Check: `tail -3 target/model-fetch-20261015/ledger.jsonl`,
`ls -l target/model-fetch-20261015/*.gguf*`, `docker ps | grep mimik-fetch`.

## Results at hand-off (2026-10-15T16:23Z)

- B `qwen35-9b`: **verified** — 5,680,522,464 bytes, SHA256 equals pin, in
  1,781.5 s elapsed (cap 3,035.9 s) with the first transport (`curl`); marker
  `Qwen3.5-9B-Q4_K_M.gguf.verified.json` written.
- A `qwen36-35b-a3b`: started 16:21:40Z (`candidate_started_unix`
  1792081300.48); metadata and provenance ok; `curl` transfer in progress
  (282,742,784 bytes at 16:23Z). Candidate deadline 1792092213.7 ≈ 19:23:34Z;
  ETA at measured speed ≈18:12Z plus hashing. Final status is only what
  `candidate_end` in the ledger says — not verified at the time of writing.
- Not verified here: the live deadline-stop path (no phase has hit its
  deadline yet); A's bytes/hash; anything about A loading or running.

## Runtime support

Verdict: the existing pinned runtime supports both architectures; no new image
was pulled.

- Runtime: `ghcr.io/ggml-org/llama.cpp@sha256:217d278eded30ca7b61321f4ce4dd86a0a56157f54b73e5600e2ad46070ddfff`,
  labels `version=b11610`, `revision=091b6a7fc561b78da90a0ed2d524ec995458c657`;
  `llama-server --version`: build 11610, commit 091b6a7fc.
- Source at that commit: `src/llama-arch.cpp` maps `LLM_ARCH_QWEN35` → `"qwen35"`
  and `LLM_ARCH_QWEN35MOE` → `"qwen35moe"`; `src/llama-model.cpp` has
  hparam/tensor/graph cases for both. HF GGUF metadata at the pins reports
  `architecture` `qwen35` (B) and `qwen35moe` (A).
- Live (B only): container `--network none`, no published ports, user 1000,
  `--cap-drop ALL`, `--device /dev/dri --group-add 986 --group-add 27`, model
  bind-mounted read-only, `-c 2048 -ngl 99`. `--list-devices` showed
  `Vulkan0: AMD Radeon RX 6650 XT (RADV NAVI23) (8192 MiB)`. Log reached
  `model loaded`; `/health` 200 via `docker exec curl`; `/props` build
  `b11610-091b6a7fc`; a 1-token greedy `/completion` returned `" Paris"`
  (`stop_type: limit`). Container stopped and removed. Layer-offload split and
  throughput were not recorded; no benchmark run.
- A (qwen35moe) support is established from source and metadata only; it has not
  been loaded. At 19.02 GiB it exceeds the 8 GiB VRAM, so a load would need CPU
  / partial offload (`--n-cpu-moe` or lower `-ngl`) within host RAM (31 GiB);
  that fit is unverified.
