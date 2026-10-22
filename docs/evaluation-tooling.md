# Evaluation tooling (prototype tier)

Python evaluation tooling under `tools/`. Results are in [results.md](results.md).

## Scope

- **The Rust application has no provider or model integration.** `Cargo.toml` has no LLM
  client crate, and nothing in `src/` calls a model. Every model use today is prototype tooling
  under `tools/` that runs in containers; it is not part of the runtime binary, but ships in
  the corresponding source package.
- The provider direction is a decision (D-H in [decisions.md](decisions.md)), not code.

## Model registry and downloads

### `tools/models.json` and `tools/model_registry.py`

Read by the tool-loop runner and the queue through `model_registry.py` (`list`, `get`,
`check`, `shell`). Standard library only. A new model is a registry entry, never a runner
change.

- **Top level** (strict, schema `1`): `schema`, `model_root` (absolute, currently
  `/media/data/mimik/models`; `MIMIK_MODEL_ROOT` overrides it), `path_convention`, `models`.
- **Entry** (unknown or missing keys are refused; `notes` optional):
  - identity and pin: `label`, `path`, `sha256`, `size`, `source` (`repo`, `revision`,
    `file`), `server_image`, `licence`, `downloaded`;
  - runtime: `server` (ctx, layers, threads, KV cache, flash attention, `extra_args`),
    `memory_limit`, `prewarm`, `health_wait_seconds`,
    `request_timeout_seconds`, `recommended_sampling`;
  - capabilities: `capabilities` (`tools`, `thinking`, `thinking_evidence`,
    `decision_readout`);
  - budgets: `seconds_per_episode`, `vram_budget_bytes` (each with a `*_source` text).
- **Validation:** key `^[a-z0-9][a-z0-9.-]{0,63}$`; unique labels; relative `.gguf` `path`
  without `..`; 64-hex `sha256`; revision 12–40 hex, full commit once `downloaded`; `server_image` pinned as `name@sha256:<digest>`; `ctx`
  512–262,144; a quantised V cache needs `flash_attn: on`; `extra_args` may not contain
  runner-managed flags (`MANAGED_FLAGS`: model, endpoint, hardening, sampling, grammar,
  templates) or single-dash flags.
- **Capability values:** `tools` is `handler`, `unverified` or `none`; `thinking` is
  `toggle`, `always` or `never`; `decision_readout` is `verified`, `untested` or
  `unsupported`.
- **Server vector:** `server_args` always passes `--offline --no-webui --no-slots --no-mmproj
  --jinja --fit off --parallel 1 --no-warmup`. With `device: cpu` it uses `--n-gpu-layers 0`,
  6 threads (`CPU_THREADS`), a request timeout of at least 900 s and a health wait of at least
  1,800 s; the default CPU episode estimate is 360 s (`CPU_SECONDS_PER_EPISODE`). A model with
  `n_cpu_moe_required` needs `--n-cpu-moe` on the GPU.
- **`shell KEY`:** prints shell-quoted `reg_*` assignments; refuses a model that is not
  downloaded or has `tools: none`.
- **Registered:** 11 GGUF models, all `downloaded: true`: Qwen3.5-9B, Qwen3-4B-Instruct-2507,
  Qwen3.6-35B-A3B (needs `n_cpu_moe`), MiniCPM5-2B, LFM2.5-2.6B, LFM2.5-8B-A1B, Granite 4.2 3B
  and 8B, Llama 3.1 8B, Ministral 3 8B and Gemma 4 E4B. Server image: the llama.cpp b11610
  Vulkan image (`sha256:217d278e…`). The 9B, 4B and 35B entries hold ctx 16384, the others
  24576.

### Pinned download: `tools/fetch_models.py`

Outer elapsed-deadline controller for GGUF transfers.

- **One clock per candidate:** every phase takes its timeout from
  `screen_limits.setup_remaining` with the same candidate start, so a transport switch never
  restarts the clock. Phases: HF metadata (pinned commit, size, etag = SHA256), provenance, up
  to four transport attempts (`curl`, `aria2`, `curl`, `aria2`) and SHA256 verification.
- **Containment:** an exhausted phase is not launched. Containers (`mimik-model-screen:20261015`,
  user 1000, `--cap-drop ALL --read-only --memory 2g`; hashing `--network none`) are stopped at
  the deadline, with an in-container `timeout -s KILL` backstop.
- **Verification:** a `<file>.verified.json` marker is written only after the exact size and
  SHA256 match.
- **Defaults:** the global cutoff must lie within 5 h (`MAX_GLOBAL_HORIZON`); caps are
  size / (0.6 × measured throughput), with a 900 s default per candidate.
- **Not tested:** the live deadline-stop path.

## Assisted interpretation screen (step 2b)

- **`tools/assist_eval.py`** (stdlib plus optional `jsonschema`; unit tests in
  `tools/test_assist_eval.py`): the 2c output schema (`SCHEMA`), request builder (fixed prefix,
  JSON data envelope with `<`, `>` and `&` escaped, dynamic part ≤ 4 KiB dropping whole context
  items from the end), phrase-number lexer and fraction words, reply validation and checked
  evaluation, scoring (correct within the shown options, false-confident, top-1), the STROT
  candidate generator, `reachable_values` (label feasibility under the frozen rules), and the
  subcommands `run-llm` (streamed llama-server requests; ledger opened exclusively; eval, case
  and prefix sha256 in the header; `--variant` selects an exploratory prompt variant, recorded
  with its prefix hash), `prime` (the priming request: full fixed prefix, `max_tokens` 1),
  `run-strot` (inside the STROT image), `summarize` (also silent misses, duplicate values and
  totals excluding fv-02) and `check-cases`.
- **Cases:** `tools/assist-eval/cases.json`, 50 synthetic cases in six classes with blind
  labels and the labelling record; not in source packages.
- **`tools/run_assist_screen.sh <out> <model-key>...`:** hash check, 3 cold starts, the warm
  screen (2 passes, 4 stress requests); server on an internal network with `inhibit_ipv4`
  (an existing network without it is refused), pinned to `CPUS` (default 0,1) with 2 threads,
  harness on `HARNESS_CPU` (7); `DEADLINE_EPOCH` stops new requests; `MAX_TOKENS`, `VARIANT`,
  and `PRIME=1` (priming after every start). Run it from a frozen copy (`git archive`) so edits
  cannot change a running screen.
- **`tools/assist_topology_probe.sh <out> <model-key> [probe-volume]`:** the forwarder
  topology (same fail-closed network), TCP, DNS, one-way UDP and IPv6 egress probes from both
  containers, requests through the forwarder, priming, and one live request per D-H crate.
- **`tools/assist-probe/`:** the throwaway probe crate (its own workspace, not part of the
  application or its packages): loopback stubs, the destination guard, the host-built client,
  one probe per crate (`p-genai`, `p-rig`, `p-oai`), a baseline (`p-base`, which also serves the
  C3 stub as a separate process) and `p-failsafe`. `schema.json` is `assist_eval.SCHEMA` as
  JSON. Built in `mimik-ci` after a networked `cargo fetch`, run with `--network none`.
- **STROT image:** built locally from `python@sha256:a6e34c59…` with torch 2.14.1+cpu,
  transformers 5.0.0, strot-ai 0.11.1 and Julia-1's own package; weights mounted read-only from
  `/media/data/mimik/models/julia-1` (manifest `mimik-verified.json`).

## External review wrapper: `tools/external_review.py`

Implements the reviewer setup defined in `AGENTS.md`.

- **Chains:** `--chain main` is `openai/gpt-6.1-sol`. `--chain third` tries
  `opencode/minimax-m3.1-flash-free`, then `opencode/deepseek-v4.1-flash-free`, then
  `opencode/beam-free` (reviewer benchmark 2). Without `--chain` or `--model` it uses the
  third chain.
- **Free-tier limits:** free models (`opencode/*`) have daily token limits and are throttled
  when several run at once. Free runs take a machine-wide lock
  (`~/.cache/mimik-review/free.lock`), so they never overlap, also across processes. A run
  killed by the idle timeout is `stalled`; after a stall, or an opencode log error matching a
  limit (`~/.local/share/opencode/log/opencode.log`, printed with the run's log lines), the
  model is recorded in `~/.cache/mimik-review/limits.json` and skipped until the next UTC day.
- **Snapshot:** excludes `.env*`, `data`, `target`, `.git`, `.venv*`, caches, `*.sqlite3*`,
  `*.pdf` and `*.gguf`.
- **Read-only reviewer:** the opencode `reviewer` agent has write, edit, bash, web, task and
  skill denied, plus `external_directory: deny`.
- **Timeouts:** a 45 s preflight ("PONG", up to 3 attempts with backoff), then 1,800 s total
  and 120 s idle by default; reviews pass `--timeout 2400 --idle-timeout 900`, because
  MiniMax and DeepSeek take 16–18 minutes. `--diff` adds `REVIEW_DIFF.patch` and `REVIEW_STATUS.txt`.
- **Exit codes:** 0 a review was written; 3 every model was unavailable (including a run that
  exits 0 without a verdict: a tool trace or an unfinished summary); 2 usage error.

## Tool-loop pack and harness

### Pack: `examples/tool-loop-demo` (Lantern Reach)

`rules.md` (rules revision 3, LR2-1 to LR2-45), `definition.json` (CEL formulas, editable
flags, `edition_id` `demo-3`), `characters/`, `cases-dev.json` (175 dev cases: proposal 67,
answer 38, conflict 29, ask_rule 15, ask_fact 14, stale 8, refuse 4; 105 supported, 70
at-risk) and an empty `cases-heldout.json` placeholder. The README defines the outcomes, the
precedence list (points 1 to 8) and the blind writer scope. 141 dev ids are named `ho-*`; they
are dev cases. The 40-case screen subset is `tools/case-subsets/dev-screen-40.json` (rule
`interleave_by_outcome_prefix`, 16 at-risk).

### Harness: `tools/tool_loop_benchmark.py`

Stdlib only (sqlite3 FTS5 retrieval, subprocess). Formulas go through the Rust `formula_eval`
CLI (`examples/formula_eval.rs`, CEL profile `cel-mimik-1`, `--formula-cli` or
`MIMIK_FORMULA_EVAL`). `BENCHMARK_VERSION = 3`.

- **Tools offered:** `search_rules`, `read_fields`, `propose_change`, `submit_proposal`,
  `answer` (typed values required, at most len(expected) + 1 values, `ANSWER_EXTRA_VALUES`),
  `respond` (conflict, stale, refuse), `ask_user`. A reply with no finishing call is
  `no_outcome`; prose is never parsed into actions.
- **Sampling defaults:** temperature 0.1, top_p 0.95, `--max-tokens` 4096, `--max-turns` 8
  (1 to 16), `--top-k` 5, `--thinking off|on|default`, `--cache-prompt` on, request timeout
  300 s.
- **Behaviour flags (C0 defaults):** `--nudge` on (H1), `--turns-remaining` off (H2a),
  `--search-notes` off (H2b), `--sanitize-echo` on (H3), `--pending-current` on (H5),
  `--untrusted-marking` off (H6).
- **Selection:** `--split dev|heldout`, `--cases`, `--cases-file` (hashed subset),
  `--write-case-subset N`, `--heldout-cases` (frozen file, pin-gated).
- **Identity and provenance:** `--arm`, `--model-label`, `--device gpu|cpu`,
  `--server-config`, `--model-sha256`, `--thinking-capability`, `--ctx`,
  `--provenance KEY=VALUE`. Ledgers with different identity never merge; CPU and GPU ledgers
  never merge.
- **Run control:** `--budget-seconds`, `--deadline-epoch` (no episode starts at or after it),
  `--quiet-progress` (forced on held-out).
- **Offline:** `--validate-pack` (pack load, prompt audit, expected-outcome replay, notes
  check), `--dry-run`, `--dry-run-expected`, `--summarize LEDGER...` (summaries, bar verdict,
  Wilson intervals, leniency exposure), `--episodes-csv`.
- **Live system arm:** `--router-decisions FILE --router-decisions-sha256 HEX` (dev only, off
  by default; C0 output is byte-identical when off).
- Ledgers are opened exclusively (never overwritten); held-out ledgers are mode 0600.

### Runner: `tools/run_tool_loop_benchmark.sh <model-key> <smoke|dev|heldout> [seeds] [ctx]`

The script header lists every environment variable and exit code.
- Server and harness run in hardened containers (uid 1000, all capabilities dropped,
  read-only root, `--init`, no pulls, internal network with no published ports, names
  prefixed `mimik-toolloop-v3-`, label `mimik.toolloop=v3`); every docker call is bounded and
  cleanup is verified (exit 11 if unresolved).
- Model and CLI are hashed in a locked-down container before load (or matched in `HASH_CACHE`).
- One run at a time: a lock in the main checkout's `target/`, shared by worktrees (exit 9).
- Output: `target/tool-loop-benchmark-<date>/<key>/<split>-<time>/` with `provenance.txt`
  (`failure=<class>` on failure), server log and ledger.

## Unattended runs

### Schedule

Heavy steps are refused Monday to Friday 09:00–18:00 local time (runner exit 10), re-checked
before each step and every 15 s; no episode starts within 900 s of the cutoff.
`SCHEDULE_OVERRIDE=user-approved` or a valid `target/schedule-override.json` (`until`, at most
4 days after its mtime; `tools/toolloop_ops.py`) lifts it.

### CPU mode

`DEVICE=cpu`: no `/dev/dri` in the server container, `-ngl 0`, `--threads` (default 6), no fan
precondition; the server container still gets `--cpus 6`. GPU arms need the fan in manual mode
at pwm ≥ 230 (exit 14 otherwise).

### CPU pinning

`CPUSET` (a cpuset list such as `2-5,8-11`) adds `--cpuset-cpus` to every container the
runner starts (hash checks, server, harness) and is recorded in `provenance.txt`; llama.cpp
`--threads` above its CPU count is refused (exit 2). The queue inherits `CPUSET` from its
environment, so `CPUSET=... tools/launch_frozen_queue.sh ...` pins a whole session. Unset
means no pinning. The router runner ignores it.

### Queue: `tools/run_queue.py run QUEUE.json` (also `check`, `window-check`)

Queue files are in `tools/queues/`. Arms run in order through the runner (or the router runner
for `kind: router`). Each arm gets a `CHECK_ONLY` pre-pass, a fit check against the window,
and at most one retry for infrastructure failures. An arm is complete only with runner exit 0,
verified cleanup and a ledger that passes the harness's strict validation against the
requested job set. An arm stopped at the window edge is rerun whole later. Session output:
`target/tool-loop-queue-<name>-<stamp>/manifest.json`. Start detached, never interactive.

### Thermal guard

`toolloop_ops.THERMAL_DEFAULTS`, overridable per queue under `thermal`: junction ≥ 90 °C stops
at once, edge ≥ 80 °C for 20 s, any sensor at crit − 5 °C, fan fault for 10 s, cooldown to
edge ≤ 60 °C held 5 min (cap 45 min), 2 thermal stops end the session, sampling every 5 s.
CPU arms record temperatures but skip the GPU and fan stops. The CPU temperature is not
readable on this host.

### Watchdog: `tools/run_watchdog.py`

Started by the queue as a separate process, real clock only. It removes owned containers and
the network from 240 s before the window edge, outside the window, and as orphans when no lock
holder exists. Its own hard limits (crit − 5 °C, junction ≥ 95 °C) apply
to every arm.

### Frozen-copy launches

`tools/launch_frozen_queue.sh [--print] [--session-end "YYYY-MM-DD HH:MM"] <commit>
<queue.json>...` creates a clean detached worktree `<main>/../mimik-wt/run-<sha12>`
(`FROZEN_BASE`) and starts the queues there with `setsid nohup`. Edits elsewhere cannot affect
the run, and the harness sha in each ledger is the frozen commit's. It refuses
`MIMIK_MODEL_ROOT`, a non-default Docker endpoint, and an already running queue or watchdog.
Analyse a run (`--summarize`, `decision_router.py select`) from the same frozen copy; code
identity checks refuse other code.

## Held-out gate

The v3 blind held-out set is pinned by `tools/heldout-v3-pin.json` and the code anchors
`HELDOUT_FROZEN_SHA256` and `HELDOUT_PIN_SHA256` in `tools/tool_loop_benchmark.py`; harness,
runner and queue refuse a missing file or a hash or count mismatch. Pin contents, ledger and
access policy, and gold quality are in
[tool-loop-heldout-v3-freeze.md](tool-loop-heldout-v3-freeze.md).

## Decision router tooling (dev only)

- `tools/decision_models.json` / `tools/decision_models.py`: router backbones
  (`r4-kev-0.8b`, `r1-qwen3-4b`, `r3-qwen35-9b`), CPU only, per-artefact sha256, separate from
  `models.json`.
- `tools/router_state_builder.py`: isolated state rendering and sentinel scan;
  `tools/router_tokens.py`: vocab-only token counts.
- `tools/run_decision_router.sh <router-key> <probe|screen|ablation>` with
  `tools/router_driver.py` in container `mimik-toolloop-v3-router` (`--network none`). Same
  lock, schedule and cleanup contract as the tool-loop runner. It also has a hard deadline
  fixed at 2026-10-19 09:00 (`HARD_DEADLINE`), so it refuses every run after that date until
  the constant is changed.
- `tools/decision_router.py` subcommands: `oracle`, `prepare`, `folds`, `select`, `choose`,
  `overlay`, `decision-file`, `timeouts`. Decision-file integrity is checked by
  `verify_decision_file`, which the harness also runs; there is no CLI subcommand for it.
- Queues: `tools/queues/router-cpu-dev.json` (R4, R1, R3 under a 12 h projection cap) and
  `router-cpu-dev-r3.json` (R3 without the cap).

## Pre-registered evaluation design (not yet run)

Binding for the remaining v3 benchmark work. Change it only before the first episode of the
arm concerned, and record who authorised any deviation.

**Bar and verdict.** The unit is the case; per-seed episode rates are reported beside it. The
bar: correct ≥ 90% over all cases, and false-supported ≤ 2% over all cases and over at-risk
cases, on ≥ 100 unique held-out cases with no `not_run`. The verdict has three values:
- *refuted*: a point estimate misses the bar;
- *met empirically*: both point estimates meet it;
- *statistically confirmed*: the case-level (any-seed) Wilson bounds also meet it. This needs
  about 188 independent at-risk cases with zero false support, so A0 (78 at-risk) cannot
  confirm.

Code differs: `bar_verdict` (`BAR`, `tools/tool_loop_benchmark.py:69`) still judges the
all-cases and at-risk variants separately, labelled "pending denominator choice", and has no
three-valued output.

**Scoring.** Strict scoring is the v2 rules unchanged and is the only verdict, selection and
product input. A differential test must keep reproducing the v2 ledgers (153/70/46). Lenient
scoring is informational. It removes only the eligible reasons below; `unreported_violation`
and any other reason survive.
- **S1 `wrong_answer`:** *contradicted* means a reported value has the same type as a missing
  expected value but a different value; otherwise the answer is *incomplete or mistyped*.
  Strict: both are false support. Lenient: incomplete is incorrect but not false support. The
  int hedge stays count-only, and the strict hedge guard is not extended.
- **S2 `wrong_proposal`** on an expected proposal: *incomplete* means a strict subset of the
  expected changes, each exact; *contradicted* means any extra or changed value. Strict: both
  are false support. Lenient: incomplete is not.
- **S3 `answer` used to abstain** on an at-risk case. Strict: `outcome_error`. Lenient:
  excluded only when a blind adjudicator labels it "abstains or declines". The adjudicator is
  a fresh agent that sees the request, message and values only, and is blind to arm, model,
  score and gold. The rubric covers mixed refusal plus claims, conditional claims and filler
  values, and is hashed before freeze. Labels come as a validated CSV through
  `--summarize --labels`; without labels, lenient equals strict. User vetoes are logged,
  published, and apply to lenient only.
- **S4 nudge:** scores as run, and as "first attempt", where a nudged episode counts as
  `no_outcome`. This is a stopped-prefix diagnostic, not a no-nudge control.
- Leniencies kept by design, with exposure counts in every summary: value-only answers, empty
  citations, `answer` on an expected proposal, `no_outcome` never false support.

Not built yet: S1 to S4, lenient, `--labels`, the S3 rubric file and the selection script.

**Base C0.** H1 nudge, H3 sanitised echo, H5 current values and H7 reporting are on. H2a
(`turns_remaining`) and H2b (search notes) are off and are measured as their own arms.
Thinking off, temperature 0.1, `max_tokens` 4096, `max_turns` 8. The code and queues match
this.
- ctx is frozen at 24576 for every arm, from the offline prompt audit in `--validate-pack`;
  the queues override the registry's 16384 for the 9B and 4B.
- v3 results are not comparable to v2 (H11 `max_tokens` 4096).

**Arms.** The 9B is the main model. Dev arms run seed 17 on all 175 dev cases; "dev-half" is
the first half of `case_order` (the plan was written for 174 cases, giving 87). Each dev arm
changes one variable against C0.

| Arm | Model | Split | Change vs C0 |
|---|---|---|---|
| D0 | 9B | dev | none |
| D1 | 9B | dev-half | thinking on (valid only as "thinking at 4096 tokens, T 0.1"; record truncations) |
| D2 | 9B | dev | `max_turns` 12 |
| D3 | 9B | dev | H6 untrusted marking |
| D4a / D4b | 9B | dev | H2a / H2b |
| A0 | 9B | held-out `full`, seeds 17 and 29 | none: **the only verdict arm** |
| A1 | 35B-A3B | held-out `h1-prefix-100`, seed 17 | none: a descriptive reference, never compared to the bar |
| A2 | 9B | held-out, seeds 17 and 29 | C0 plus the selected features: exploratory |

- **Selection** is a frozen script, built and committed before any held-out episode. Deltas
  are paired per case on identical case sets, and D0 is recomputed on dev-half for D1. A
  feature qualifies only with Δcorrect ≥ +5 pp and false support no worse over all cases and
  at-risk cases. Each candidate also runs dev seed 29, and the gain must hold on both seeds.
  The exact combined configuration runs on dev before A2. A2 is dropped if nothing qualifies.
  If A2 passes and A0 fails, A0 is the verdict.
- **Held-out is used once.** All dev arms and the selection finish before any held-out
  episode. No result-dependent reruns, additions or cuts after the first held-out episode.
  During held-out runs the main session sees only the final summary.
- A0 changes both cases and flags against v2 at once (not controlled).
- H9 (an offline stale-router overlay) needs an applicability predicate using only
  product-available inputs, defined before freeze; otherwise H9 is cut. It is not built. H10
  is cut; the dev-only SYS-R router track replaces it and enters the held-out set only if it
  is pre-registered before A0's first episode.

**Budget and order.** Order: G0 (35B load test, frozen list `--n-cpu-moe` 40/36/32/28/24, 8 h
deadline; pick the lowest s/token with VRAM ≤ 7.5 GB and no swap growth) plus smokes, then
D0, D4a/D4b, D2, D1, D3, the seed-29 confirmations, selection, A0, A2, and A1 last. Recompute
the per-arm worst case from the measured smoke throughput before launch. If the budget does
not fit, skip arms in this order: A2, D3, D1 (cut to 40 cases), then A1 (already a 100-case
prefix). Never cut A0. Fix the A1 case list from dev smoke speed, never from held-out speed.

**After the runs.** An independent review of the v3 results, with the per-episode
classification published as a data file and a v2/v3 non-comparability statement.
