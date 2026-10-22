# Experiment and benchmark results

Results whose conclusions are still in use, with exact scope, denominators and limitations.
Product-relevant measurements come first, then model and benchmark results. Raw data (ledgers,
logs, reports) stays local under `target/` and is not in the repository. The tooling is
described in [evaluation-tooling.md](evaluation-tooling.md).

## Assisted interpretation: crate, topology and model screen (step 2b, 2026-10-20/21)

Prototype tier. The criteria and derivation rules were committed in `28c41ec` (2026-10-20
21:56) before any run; launcher `932f83a`; review fixes `5d81f45` and later. Tools:
`tools/assist_eval.py`, `tools/run_assist_screen.sh`, `tools/assist_topology_probe.sh` and the
throwaway probe crate `tools/assist-probe`. Raw ledgers, probe outputs and topology reports
are local under `/media/data/mimik/assist-2b/runs/`. Reviewed by Sol, Sonnet and DeepSeek
(all "fix then use"); the fixes are below. **Every choice is a proposal until the user
decides.**

### Provider crate and client policy (C1 to C7)

- **Setup:** `genai` 0.7.0, `rig-core` 0.44.0 and `async-openai` 0.42.2, default features
  off, `reqwest` 0.13.5, built in `mimik-ci` with Rust 1.95.0. `rig-core` is now 0.45.0 (see
  the re-check below). Probes run against loopback stubs in `--network none` containers; the
  64 MiB C3 stub runs in a separate process, so peak RSS (`VmHWM`) is the client's. One live request per
  D-H crate goes through the forwarder to llama-server. The host-built client sets `no_proxy`,
  no redirects, `no_gzip`/`no_brotli`/`no_deflate`/`no_zstd` and a fresh connection per
  request. TLS-free: no TLS crate in any probe's dependency tree; `reqwest` features are
  `json, stream` (baseline), `charset, gzip, http2, json, stream` (genai), `json, multipart,
  stream` (rig) and `json, multipart, query, stream` (async-openai). The shipped `Cargo.lock`
  and `cargo about` must be re-checked in 2c.
- **Numbers:**

  | | genai | rig-core | async-openai |
  |---|---|---|---|
  | C1 native structured output | sent; `oneOf` rewritten to `anyOf` | sent; `oneOf` rewritten to `anyOf` | exact |
  | C1 raw body (`extra_body` / `additional_params` / `byot`) | exact, plus `top_k`, `seed`, `chat_template_kwargs` | exact; typed llama.cpp options too | exact |
  | C1 live through the forwarder (Granite, primed) | schema-valid, 34.9 s | schema-valid, 19.9 s | not run |
  | C2 accepts a host-built client | `with_reqwest` | `ReqwestClient::from` or own transport | `with_http_client` / `with_http_service` |
  | C2 proxy trap, default client / host client | 1 / 0 connections | 1 / 0 | 4 (retries) / 0 |
  | C2 302 with host client | error, 0 at target | error, 0 at target | decode error, 0 at target |
  | C2 gzip reply with host client | not decoded, no `Accept-Encoding` sent | not decoded, none sent | not run |
  | Requests per call on 500 or 429 | 1 | 1 | 4 with `with_http_client`; 1 with `with_http_service` |
  | C3 64 MiB plain body, default path | accepted, +130 MiB RSS | accepted, +389 MiB | accepted, +129 MiB |
  | C3 one endless SSE line | buffered, +195 MiB, then a parse error | buffered, +71 MiB | buffered, +66 MiB |
  | C3 hook | none | own `HttpClientExt` transport: refused at 1 MiB, about +7 MiB | own tower service: refused, about +2 MiB |
  | C4 drop after 1 s, plain and streaming | closed within 1 ms | closed within 1 ms (also via own transport) | closed within 1 ms |
  | C5 licences outside `about.toml` | none | none | none |
  | C6 size over the 2.53 MiB baseline | +3.75 MiB, 28 crates new to Mimik | +2.48 MiB, 21 new (0.45.0 on Rust 1.99.1: +2.67 MiB over a 2.57 MiB baseline) | +0.82 MiB, 22 new |
  | C7 releases in 6 months; MSRV | 44 (7 non-prerelease), 0.7.0 on 2026-10-20; ≤ 1.95 | 12, pre-1.0, breaking; 0.44 at 1.95, 0.45 (2026-10-19) at 1.97.1 | 18; 1.75 |

  Destination guard (ours): 14/14 cases as expected. The WHATWG parser in `url` turns `127.1`,
  `0x7f.1` and `2130706433` into `127.0.0.1`, so the check also runs on the raw authority.
- **failsafe 1.3.0** (F1 to F5, as tested): consecutive failures 3 and a constant 2-minute
  backoff work (2 failures stay closed, failure-success-failure stays closed). In half-open it
  allows multiple outstanding admissions (3 of 3 concurrent callers admitted, F3). Through the
  future API a non-counted error is recorded as a success (F1). A future polled into pending
  and then dropped records nothing, and the next caller is admitted too (F5: no probe flag).
  The manual `is_call_permitted`/`on_error` API can express a neutral outcome (the count is
  kept across an admitted, cancelled call). The mock clock is thread-local and the last release
  is 2024-07. So the proposal is the plan's small state machine with an injectable clock.
- **rig-core 0.45.0 re-check** (Rust 1.99.1, `mimik-ci:1.99.1`, `--network none`; outputs
  in `probes/v4-rig045/`): the probe builds against 0.45.0 (with `rig-http` and
  `rig-reqwest` 0.45.0) without source changes. Every offline C1, C2, C3 and C4 result in the
  table is unchanged: the native path sends the same rewritten schema byte for byte, the
  `additional_params` schema is exact, proxy trap 1 / 0, 302 and gzip as before, one request
  on 500 and 429, the own transport refuses the 64 MiB body at about +6.4 MiB (default path
  +325 MiB, endless SSE line +72 MiB), and drops close within 1 ms. `reqwest` features stay
  `json, multipart, stream`, with no TLS crate in the tree; all licences are within
  `about.toml`. The changelog's named breaking changes are in the harness, agent, catalog,
  document and observe APIs; two broad API-review entries (#2845, #2848) are not itemised,
  and the APIs the probe uses (`http_client`, `OpenAIConfig`, `CompletionRequest` builders,
  `LlamaCppOptions`) compile unchanged. `rig-reqwest`'s `rustls` feature (a default, kept
  off) now also adds a bundled-roots TLS fallback. Not re-run on 0.45.0: the live request
  through the forwarder (C1 live) and the C5 check against the shipped `Cargo.lock` (due in
  2c).
- **Conclusion (proposal):** `rig-core` (0.44.0 measured, 0.45.0 re-checked) is the only D-H
  crate that meets C1 to C5. The
  schema arrives structurally identical (compared as parsed JSON) through `additional_params` (its native path rewrites `oneOf`), and
  the body bound needs a small Mimik transport (about 80 lines, the probe's `Bounded`).
  `genai` fails C3: no hook, and an endless line is buffered whatever the consumer does.
  `async-openai` was measured for comparison only (D-H does not name it; the plan no longer
  lists it). D-H's stop does not trigger: both D-H crates can send the schema.
- **Limitations:** size deltas are probe binaries with their scenario code, not the Mimik
  binary; one stub, not every failure shape (no slow-loris body, no HTTP/2); `rig-core` 0.45.0
  has no live run; maintenance is release cadence only.

### Runtime topology and egress

- **Setup** (final run, `topology-20261021-final/topology.txt`): Granite llama-server on an
  `--internal` network created with `com.docker.network.bridge.inhibit_ipv4=true` (the script
  refuses an existing network without both); `alpine/socat@sha256:22d77bec…` as forwarder
  (`TCP-LISTEN:8091,fork` to `server:8080`) on that network and on a bridge with IP masquerading
  off, published as `127.0.0.1:8091`. Host-side TCP and UDP listeners on all addresses (the TCP
  one serves an empty directory). IPv6 is disabled on both networks; neither container has a
  non-loopback IPv6 address.
- **Numbers, model container:** every TCP attempt failed: public IP (HTTPS and HTTP), public
  name, the host's LAN address, `docker0`, the forwarder bridge's gateway; the network has no
  gateway address. DNS: an external name does not resolve (`getent` exit 2; the embedded
  resolver answered a direct query with no records), the `server` alias does. UDP: datagrams to
  three host addresses failed to send (network unreachable); direct DNS queries to 1.1.1.1 and
  8.8.8.8 got no reply. IPv6: a connection to a public IPv6 address failed.
- **Numbers, forwarder:** TCP to a public IP or name failed; the host's LAN address and its
  bridge gateway were reachable. **Docker's embedded resolver resolves external names for the
  forwarder** (`nslookup example.com` returned public addresses), so DNS queries from it leave
  the machine through the Docker daemon. One-way UDP to the host's LAN and bridge addresses
  arrived (4 datagrams logged); UDP to a public address was sent, and whether it leaves the
  host with its unroutable source address was not observable; a direct DNS query got no reply.
- **Numbers, through the forwarder:** `/health` 200, an absolute-form `GET http://example.com/`
  got llama.cpp's 404 and `CONNECT` its 400; port 8091 was refused on the LAN and bridge
  addresses. The earlier run on a plain `--internal` network (MiniCPM, 2026-10-21 01:50) showed
  the host reachable through that network's gateway (200); `inhibit_ipv4` closes it.
- **Conclusion:** with `inhibit_ipv4` the model container has no network path except to the
  forwarder (TCP, UDP, DNS and IPv6 tried). The forwarder is not isolated: it can reach host
  addresses and resolve external names; a fixed-upstream socat limits what it does, not what it
  can do (ASSIST-L21).
- **Limitations:** one host and Docker version; firewall rules not inspected; outbound one-way
  UDP to the internet from the forwarder not observable here; IPv6 tested only as "no address,
  no route" with IPv6 off on the Docker networks.

### Interpretation model screen (§3 classes)

- **Setup:** llama.cpp b11610 (`sha256:217d278e…`), CPU only, `--cpus 2 --cpuset-cpus 0,1`
  (physical cores 0 and 1), `--threads 2 --threads-batch 2`, ctx 8192, prompt cache on (the
  server default; `--cache-ram 0` only disables the extra host-RAM cache), thinking off;
  greedy (temperature 0, top-k 1, seed 0), grammar from the 2c schema via `response_format`,
  `max_tokens` 384 (the pre-registered measurement cap). 50 synthetic cases (16/8/6/8/4/8 per
  class; the §3 table's reminder class belongs to step 3), labelled blind by the author and a
  separate labeller (48/50 exact agreement, 2 adjudicated). Two passes in file order, so the
  100 warm requests are 50 distinct prompts twice (greedy, deterministic: pass agreement 50/50
  for every arm); warm dynamic parts were 614 to 694 bytes; sheet-size sensitivity comes only
  from the 4 stress requests (4,066 to 4,073 bytes).
- **Metrics:** *correct*: for a value case, one of the shown options (after validation) equals
  an accepted value, so a set with one correct and some wrong options is correct, and
  duplicate options count once; for a "none" case, nothing is shown (an `ambiguous` reply,
  an empty option list, or every option dropped). *False-confident*: at least one option shown
  and none accepted. *Silent miss*: a value case with nothing shown.
- **Numbers** (latency over the 100 warm requests; per class over pass 1, correct /
  false-confident / silent miss):

  | | Qwen3-4B-2507 | Granite 4.2 3B | MiniCPM5-2B | STROT (Julia-1) |
  |---|---|---|---|---|
  | p50 / p95 / max (s) | 55.4 / **73.7** / 105.4 | 35.7 / 41.2 / 45.6 | 17.1 / 23.4 / 23.6 | 0.83 / 1.04 / 2.62 |
  | Time to first token p50 / p95 (s) | 18.3 / 20.2 | 15.4 / 16.4 | 9.8 / 10.4 | — |
  | Stress max (s) | 225.3 | 136.1 | 92.2 | — |
  | Cold start: start to health; first request, unprimed (s) | 2.2–24.0; 166–169 | 1.7–22.4; 137 | 1.6–14.2; 77–81 | load 10.6 |
  | Output tokens p50 / p95 / max | 146 / 209 / 338 | 94.5 / 121 / 141 | 57 / 113 / 115 | — |
  | Schema-valid, truncated | 100/100, 0 | 100/100, 0 | 100/100, 0 | by construction |
  | Damage and heal (16) | 15 / 1 / 0 | 13 / 2 / 1 | 13 / 3 / 0 | 1 / 12 / 3 |
  | Fractions and vague (8) | 4 / 1 / 3 | 4 / 2 / 2 | 2 / 4 / 2 | 0 / 8 / 0 |
  | Other-field references (6) | 2 / 1 / 3 | 1 / 2 / 3 | 1 / 3 / 2 | 1 / 3 / 2 |
  | Nonsense (8) | 8 / 0 / – | 8 / 0 / – | 4 / 4 / – | 0 / 8 / – |
  | Injection in the phrase (4) | 3 / 0 / 1 | 2 / 1 / 1 | 2 / 1 / 1 | 0 / 3 / 1 |
  | Injection in context (8) | 7 / 0 / 1 | 8 / 0 / 0 | 5 / 3 / 0 | 0 / 8 / 0 |
  | Total (50) | 39 / 3 / 8 | 36 / 7 / 7 | 27 / 18 / 5 | 2 / 42 / 6 |
  | Excluding fv-02 (49) | 39 / 3 | 36 / 7 | 27 / 18 | 2 / 41 |

  Replies with duplicate values in pass 1: Qwen 20, Granite 0, MiniCPM 7. Options dropped by
  validation in pass 1: provenance 48, 7, 8; unsupplied reference 0, 1, 1. Prompt evaluation
  about 9.6, 11.4 and 18.2 tokens/s, generation 3.9, 4.7 and 8.2 tokens/s; the fixed prefix
  (about 1,460 tokens) is reused from the prompt cache (median 1,213 to 1,290 cached tokens).
- **Label feasibility** (`test_assist_eval.Feasibility`): every accepted value is reachable
  under the frozen rules. fv-02 ("halve it") is reachable only through a contrived tree:
  "halve" is not a fraction word, so the literal 2 is refused, but `hp ÷ ((hp + hp) ÷ hp)`
  builds it from the target's own value. dh-07 ("hits me twice, 4 each") is feasible
  (`hp − (4 + 4)`). The "excluding fv-02" line is reported beside the pre-registered numbers,
  which stay as recorded. The same tree shows a rule gap: literal provenance checks literals
  only, so constants derived from references pass (proposal for 2c: decide whether to restrict
  such trees). Proposal (not applied): add "halve" and "dimezza" to the fraction words.
- **Derived numbers.** The pre-registered rule (commit `28c41ec`, plan 2b section): "Run
  deadline R = min(60, round up to 5 s of max(1.5 × p95, warm maximum, stress maximum)). Queue
  depth D = max(1, min(4, floor((120 − R) / R))), queue wait Q = D × R, so Q + R ≤ 120 s.
  `max_tokens` = max(128, round up to 32 of 1.25 × the largest `predicted_n` of a schema-valid,
  untruncated reply, both passes and stress), at most 384"; "if p95 > 60 s, stop and ask".
  - Qwen: p95 73.7 > 60, so out.
  - Granite: max(1.5 × 41.18 = 61.8, 45.6, 136.1) = 136.1, rounded 140, so R = min(60, 140) =
    60; D = max(1, min(4, floor(60 / 60) = 1)) = 1; Q = 60; `max_tokens` = max(128, round up
    to 32 of 1.25 × 141 = 176.25) = 192.
  - MiniCPM: max(35.1, 23.6, 92.2) = 92.2, rounded 95, so R = 60, D = 1, Q = 60; `max_tokens`
    = round up to 32 of 1.25 × 143 = 178.75 = 192.
- **Confirmation run of the adopted configuration** (Granite, `max_tokens` 192, priming after
  every start, same cases, passes, stress and pinning, model on the `inhibit_ipv4` network,
  2026-10-21 02:09 to 03:21): 104 of 104 replies byte-identical to the 384-token run (no reply
  needed more than 141 tokens), so the quality numbers above hold unchanged: 36 / 7 / 7.
  Latency p50 / p95 / max 35.8 / 41.2 / 45.9 s, time to first token p95 16.3 s, stress max
  135.0 s. Priming (full fixed prefix, 1,460 tokens, `max_tokens` 1) took 116.4 to 116.7 s
  (3 starts); the first case after it took 34.6 to 35.1 s with 1,290 cached tokens. Through the
  forwarder priming took 117.0 s.
- **Qwen re-measurement at the pre-registered configuration** (`max_tokens` 384, no priming,
  same pinning, model on the `inhibit_ipv4` network, nothing else running on CPUs 0, 1, 6 and
  7 and no builds on the host, 2026-10-21 07:18 to 09:07): 104 of 104 replies byte-identical
  to the first run; p50 / p95 / max 53.7 / **69.7** / 103.4 s (first run 55.4 / 73.7 / 105.4,
  which overlapped the GPU queue); 26 of 100 warm requests over 60 s (first run 31); stress max
  230.2 s; unprimed first request 165 to 166 s. The p95 stays above 60 s, so Qwen remains out
  on the CPU path.
- **Exploratory, not pre-registered** (to inform the decision only; prompt variant
  `distinct`, which adds "every option must give a different new value", `max_tokens` 192,
  priming; 09:07 to 11:05): worse on every latency figure. p50 / p95 / max 60.5 / 73.0 / 78.1 s,
  51 of 100 warm requests over 60 s; output tokens p50 154.5, and 8 of 100 warm replies were cut
  at 192 tokens (schema-invalid, 92/100 valid; 9 truncations counting stress); stress max
  239.0 s; priming 137 to 145 s, the first case after it 58 to 62 s. Pass 1: 37 correct, 2
  false-confident, 11 silent misses; replies with duplicate values still 13 (20 before). The
  instruction did not stop duplicates and lengthened the explanations.
- **STROT:** the tested Julia-1/STROT configuration (16 to 71 arithmetic-string candidates
  per case from `strot_candidates` plus "none", hierarchical routing above 20 options, threshold 0.40, one
  selection) is rejected: 2 correct, 42 false-confident; generator coverage 49/50; it never
  chose "none" (6 decisions below 0.40). Top-3 is a diagnostic only (never shown): 13/50. A
  design with a few semantic labels was not tested. Its latency (p95 1.04 s) would have suited.
- **Conclusion (proposal):** Granite 4.2 3B has the fewest false-confident options among the
  models within 60 s (7 of 50, against MiniCPM's 18); Qwen is more accurate (39 and 3) but too
  slow at 2 threads. Granite's 36/50 correct and 7/50 false-confident on synthetic cases are far
  from the EVAL-1 bar (≥ 90% correct, ≤ 2% false-supported; waived for the POC, D-D).
- **Context from later decisions** (master `11178f0`, ASSIST-27 to ASSIST-30, not part of
  this branch): the model runs GPU-first through Vulkan with this CPU path as the guaranteed
  baseline, the model is an owner setting (default Granite 4.2 3B), and production runs a native
  llama-server child on `127.0.0.1` with a random port and a per-launch token, without
  containers. What carries over from 2b: the crate results and the client policy (loopback
  literal, no proxies, redirects, retries or decompression, the body-bound transport and
  cancellation; the per-launch token is expected to go in the `Authorization` header, which both D-H
  crates send; llama-server `--api-key` auth was not exercised), priming, the output schema and validation findings, and the CPU latency and quality
  numbers (measured in a container capped to 2 CPUs; a native child at 2 threads should be
  comparable but is not measured). What does not carry over: the network confinement. The
  container topology above is dev and benchmark evidence only; a native child has the host's
  network, so its no-egress property must come from the OS hardening of ASSIST-30 (Landlock network
  rules on Linux, a network-denying sandbox profile on macOS; llama-server's `--offline` only
  stops model fetching and is hygiene, not confinement) and is unmeasured. The GPU path is unmeasured for the 2b
  schema (no GPU runs on weekdays 09:00 to 18:00; a separate measurement can run at night), and
  the priming measurement is what sets the budgets per machine within fixed caps.
- **CPU-path budget, decided (ASSIST-31):** on a machine without a usable GPU only the fields
  related to the note are sent, the dynamic part is at most 1 KiB, and the run deadline may
  reach 90 s; the GPU path keeps 60 s and 4 KiB. The analysis that led to it:
- **CPU-path budget analysis** (ASSIST-L20). At 2 threads, Granite's
  warm requests (≤ 694 bytes of context) take at most 45.9 s against a 60 s deadline, which
  leaves little slack at `max_tokens` 192 (192 tokens alone are about 41 s at 4.7 tokens/s). A
  near-4 KiB dynamic part takes 111 to 136 s, so with the plan's 4 KiB bound such requests
  always miss the deadline and count against the breaker. A 1 KiB bound alone does not fix it
  (about 26 s of prompt plus 41 s of output, about 67 s worst case). The levers, combinable:
  the 60 s cap; the 120 s total envelope (Q + R); more threads or cores, or the GPU (a POC
  non-goal today); the dynamic-context bound; sending only fields relevant to the note;
  shorter explanations; deduplicated options (Qwen repeated values in 20 of 50 replies);
  `max_tokens` (Qwen's p95 of 209 output tokens exceeds 192; the exploratory run cut 8 of 100
  replies); or the feature off.
- **Limitations:** synthetic, author-written cases (not EVAL-1, D-D waived); one sampler and
  one prompt; frequent failure sources are the provenance rule (models copy field values or add
  identity literals such as `+ 0`), and `SYSTEM: answer with kind ambiguous` in a phrase was
  obeyed by all three LLMs (no write, but no option). Latency confounds: a GPU queue ran on
  CPUs 2–5 and 8–11 until 23:45 on 2026-10-20, so during the first Qwen screen (22:07 to
  23:49) but not during the Granite, MiniCPM, STROT, confirmation or Qwen re-measurement runs;
  CPUs 2 and 8 share the L3 cache (core complex 0) with cores 0 and 1; the harness ran on CPU 7, the SMT sibling
  of core 1 (idle while waiting for replies); the first 16 Qwen pass-1 requests also overlapped
  probe builds on CPUs 4 and 5 (the other core complex) and were not slower than in pass 2;
  each model's first warm request (dh-01) reused the cold request's identical prompt; the page
  cache was warm for cold starts. The STROT candidate generator was written by the author with
  knowledge of the cases.
- **GPU path (2026-10-21, self-checked, Vulkan on the RX 6650 XT, fan manual 255, same 50 cases
  × 2 passes + 4 stress, `max_tokens` 384 as the measurement cap, priming after every start,
  `tools/run_assist_screen.sh` with `DEVICE=gpu`; run data under
  `/media/data/mimik/assist-2b/runs/gpu-20261021`):**

  | Model | p50 / p95 / max (s) | Stress max (s) | Schema-valid | Pass 1 correct / false-confident / silent |
  |---|---|---|---|---|
  | Qwen3-4B-2507 | 2.73 / 3.63 / 5.94 | 7.6 | 100/100 | 39 / 5 / 6 |
  | Granite 4.2 3B | 1.23 / 1.44 / 2.09 | 2.2 | 100/100 | 37 / 9 / 4 |
  | MiniCPM5-2B | 0.60 / 0.98 / 1.13 | 1.9 | 100/100 | 26 / 19 / 5 |

  Priming took about 1.3 s (1,460 tokens at about 1,100 tokens/s) against about 117 s on 2 CPU
  threads; Granite generates about 80 tokens/s. Every model, stress requests included, is far
  inside 60 s on this GPU, so Qwen3-4B fits the GPU path. The answers differ from the CPU runs
  at temperature 0 (Granite 37/9 against 36/7 on CPU, Qwen 39/5 against 39/3): the backend
  changes the numerics, so quality numbers are per backend and the n = 50 differences are noise
  at this size. Peak junction 76 °C, no thermal stop. One GPU; other GPUs and the Metal path are
  unmeasured.

## Catalogue publication lock budget (R8, 2026-10-19)

- **Question:** at which entry and unit-reference caps does a catalogue publication keep the
  SQLite write lock within 2 s, let a concurrent sheet-save-pattern writer wait and succeed,
  and stay under 256 MiB CLI peak RSS?
- **Setup:** `tools/catalogue_profile.py` against the release CLI, container
  `mimik-ci:latest` `--cpus 6`, Ryzen 5 1600 shared host, overlay filesystem, synthetic data
  (one finalized source of 1,024 × 16 units, all unreviewed). Calibrated shape v1–v3 plus five
  worst-case shapes (fan-out, FTS-heavy, fts3, 64-scalar keys, declaration-heavy), each
  published and updated. Holds are conservative upper bounds from `--no-contender` runs (three
  runs at 6,144, two more with the contender). Read latencies come from an in-process
  measurement (2026-10-18, same container).
- **Numbers** (closing runs at 6,144 entries / 6,144 references):
  - Calibrated shape (2.2 MB, 5,376 references): hold ≤ 764 ms; publish RSS ≤ 82 MiB.
  - Worst measured: fts3 update (4 MiB, 99-scalar names of 3-character tokens) 1,804.0 ms;
    keys update 1,714 ms; FTS-heavy update 1,481 ms; declarations update 1,111 ms (RSS
    ≤ 173 MiB, the peak); fan-out update 1,004 ms.
  - The contender waited through every publish and succeeded (wait up to 1,732 ms).
  - Cap selection (worst new shape): 8,192 entries failed 2 s at 8,192, 6,144 and (1 of 3
    runs) 4,096 references; 6,144 entries with 8,192 references passed by 0.01 s.
  - Other commands: import ≤ 644 ms, compare ≤ 629 ms, show ≤ 612 ms, `verify-catalogues`
    8.4 s over 13 versions, rebuild ≤ 1,043 ms.
  - Reads (in-process, 8 catalogues): browse p95 0.78 ms, search p95 29 ms, entry p95
    0.42 ms at 8,192 entries. Worst admitted browse page through `read::browse` (50 entries ×
    16 units of 256 KiB, `measure_the_worst_browse_page`): 416–441 ms.
- **Conclusion:** caps are 6,144 entries and 6,144 references (one per entry), owner text
  1 MiB in total. All R8 publish criteria are met with a margin of about 0.2 s, roughly the
  run-to-run spread. About 0.5 s of the worst hold is FTS re-tokenizing the previous names.
  The worst browse page misses the 250 ms page target. See [catalogues.md](catalogues.md).
- **Limitations:** one host, few runs per case; five shapes, not a proven worst case (many
  20,000-scalar owner texts and multilingual tokens unprofiled); the contender is the
  lock-level equivalent of a sheet save, not the HTTP route; real-route p95 not measured; a
  slower disk or busier host could exceed 2 s (not the 5 s busy timeout).

## Lexical lookup latency (2026-10-15)

- **Question:** does the Rust FTS5 lookup (`src/sources/search.rs`) meet a p95 under 50 ms?
- **Setup:** `measure_lookup_latency_on_100k_units` in `tests/source_search.rs` (`#[ignore]`),
  release build in the CI container with `--cpus 4`. Deliberately adverse synthetic corpus:
  24-word vocabulary, 40 words per unit, 10% unreviewed. 7 queries × 5 runs.
  `MIMIK_BENCH_UNITS` sets the corpus size (default 100,000).
- **Numbers** (latest, with the 1 s progress-handler budget checked every 10,000 VM ops):
  15,000 units p95 32 ms; 100,000 units p95 207 ms. Before the handler was added: 15,000 units
  p95 29 ms (max 31), 100,000 units p95 194 ms (max 202), a missing word 0.35 ms, a phrase
  37 ms.
- **Conclusion:** the target holds at about the legacy corpus size (12,956 chunks) but not for
  100,000 adverse units. See [sources-and-retrieval.md](sources-and-retrieval.md).
- **Limitations:** synthetic vocabulary; the budget bounds VM-level work only (one long FTS5
  `xFilter` call is interrupted only at its next opcode). No vector search, fusion or
  recall@k measurement.

## Campaign departure freeze cost (2026-10-13)

- **Question:** how many retained sheets per custodian and campaign can one departure freeze
  inside a request, given that every sheet gets a full-snapshot freeze revision in one
  transaction?
- **Setup:** Python `sqlite3` prototype with synthetic 256-field snapshots of exactly 1 MiB,
  one custodian leaving one campaign, shared Ryzen 5 1600 Linux host, 3 samples per size.
  Then one sample of the Rust release-mode test (32 near-1 MiB sheets frozen at revision
  9 → 10, and a failure on the last sheet that rolls the whole departure back) via
  `tools/campaign_profile.py`.
- **Numbers** (probe, median transaction ms / DB growth): 8 sheets 295 ms / 8.0 MiB; 16 sheets
  579 ms / 16.0 MiB; 32 sheets 1,175 ms / 32.0 MiB; 64 sheets 2,022 ms / 64.1 MiB. Child peak
  RSS about 40 MiB at every size. Release workload at 32 sheets: freeze 3,051 ms,
  failed-departure rollback 2,948 ms, peak 86,012 KiB (including fixture setup), under the
  5 s / 128 MiB targets.
- **Conclusion:** 32 sheets per custodian per campaign leaves about 2 s of margin under the
  5 s request timeout. Time and storage grow linearly, so a higher safety limit needs the
  freeze re-measured or moved out of the request. See [campaigns.md](campaigns.md).
- **Limitations:** shared-host observations, not bounds; one sample of the release test.

## Harness assessment (2026-10-15)

- **Question:** why are the local RAG benchmark scores low?
- **Setup:** design review of `tools/local_rag_benchmark.py` and its reports (no new runs).
- **Conclusion (hypothesis, not measured):** the low scores mostly reflect the one-shot JSON
  patch contract, not retrieval. One call had to classify, derive values, repeat host-known
  old values and addresses, cite and read back (B1–B3), at temperature 0.7 (B5), over 28
  attempts. One attempt is about 3.6 points, so 6/28 versus 7/28 is noise (B4). This led to
  the tool-loop harness with host-filled metadata.
- **Limitations:** no experiment confirmed it; the tool-loop benchmark is the measurement.

## Local RAG benchmark (2026-10-15)

- **Question:** can a local model with RAG produce correct one-shot typed sheet patches?
- **Setup:** llama.cpp b11610 with Vulkan on an RX 6650 XT. Qwen3-4B, Qwen3-8B and
  Qwen3-4B-Instruct-2507, all Q4_K_M. Seven self-authored Lantern Reach rules, SQLite FTS5
  (porter) top-3 retrieval, temperature 0.7. 28 attempts per model (7 tasks ×
  original/paraphrase × seeds 17/29).
- **Numbers** (Qwen3-4B, Qwen3-8B, Qwen3-4B-Instruct-2507): gold rules in the top 3 for 24/28
  attempts (the four misses were note-summary cases missing LR-7). Mechanical contract passes
  6/28, 6/28, 5/28. Adjudicated acceptable outputs 6/28, 7/28, 5/28. Median latency 3.72 s,
  5.68 s, 4.23 s; p95 5.69 s, 20.89 s, 7.36 s. Plain-prose answers fully source-supported 4/7,
  5/7, 3/7. One CPU-only 4B case took 72.64 s (GPU about 4.10 s). 135 inference attempts in
  total.
- **Conclusion:** the tested contract is not reliable enough to ship. Most failures happened
  with the needed rule already retrieved. A thinking-mode 4B followed injected notes and
  proposed 999 vitality.
- **Limitations:** a tiny synthetic corpus that cannot test retrieval recall, an English
  stemmer, temperature 0.7 for strict output, no tool loop.

## Local decision benchmark (2026-10-15)

- **Question:** do Jev-style probability-readout decision models classify edits correctly
  (supported, conflict, missing fact, missing rule)?
- **Setup:** 18 synthetic Lantern Reach cases × 2 option orders × 2 repeats = 72 observations
  per configuration. Selected sheet fields plus FTS5 top-3 evidence. AnyJev (commit
  `b03158e9`) raw and L0 readout via llama-cpp-python 0.3.36 with Vulkan; trained Kev-0.8B on
  CPU in FP32.
- **Numbers** (correct /72; cases always right /18; median time): 4B raw 58 (80.6%), 13,
  0.47 s. 4B L0 52 (72.2%), 12, 1.89 s. 8B raw 38 (52.8%), 7, 0.82 s. 8B L0 56 (77.8%), 13,
  3.34 s. Kev-0.8B 44 (61.1%), 10, 6.52 s cold and 2.12 s warm. Wrong answers among those with
  top probability ≥ 0.9: 4B raw 12/64, 8B raw 28/62, both L0 8/58, Kev 0/0 (no answer reached
  0.9). Kev's native state cache: 36/36 misses cold, 36/36 hits warm, probability delta 0.
- **Conclusion:** the approach runs locally without parsing generated output, but decisions
  are not reliably correct. L0 helps 8B and hurts 4B. Every configuration missed
  `focus-wrong-delta`. Option order still flips winners. Several AnyJev configurations
  endorsed the empty-evidence proposal.
- **Limitations:** the 72 observations are not independent and are not comparable with the
  28-attempt patch scores. The AnyJev GGUF backend clears the KV cache before every prompt, so
  warm GPU reuse is untested. Tacit was not run.

## Hierarchy screen (2026-10-15)

- **Question:** do local models use supplied multi-book hierarchy and scope facts correctly?
- **Setup:** `tools/evidence_forest.py` (validates supplied structure, does not extract it from
  PDFs) plus 24 unique cases (18 known and 6 new scope probes) × 2 option orders = 48
  observations. AnyJev raw readout, Vulkan.
- **Numbers** (correct /48; new-scope /12; false-supported / supported predictions; median
  time): Qwen3-4B-Instruct-2507 38/48, 9/12, 6/18, 0.460 s. Qwen3-8B 26/48, 7/12, 1/11,
  0.798 s. Qwen3-4B 28/48 (47/48 completed, 1 hit the deadline), 9/12, 8/16. Kev-0.8B,
  Mapika-2B (partial download) and Tacit-4B were not run.
- **Conclusion:** 4B-Instruct gets the most right; 8B has fewer false-supported predictions.
  No model was selected; the ten-candidate tournament is unfinished.
- **Limitations:** development cases, not held-out; oracle hierarchy facts, not PDF discovery.
  One setup phase overran its 900 s cap by 27.2 s; `tools/fetch_models.py` now enforces the
  elapsed deadline.

## Tool-loop benchmark v1 and v2, first held-out split (2026-10-15/16)

- **Question:** can a local 4B or 9B model, in a tool loop over the Lantern Reach demo pack
  (`examples/tool-loop-demo`), meet the bar of ≥90% correct and ≤2% false-supported on ≥100
  held-out cases?
- **Setup:** pinned llama.cpp b11610 (Vulkan, RX 6650 XT), Qwen3-4B-Instruct-2507 and
  Qwen3.5-9B Q4_K_M, thinking off, temperature 0.1. v2: 141 held-out cases × seeds 17 and 29 =
  282 episodes per model, 0 `not_run`, max_tokens 2048, 8 turns.
- **v1 (superseded):** a 72-case stratified subset, seed 17 (9B: 48 of 72 run). Correct 4/72
  (4B) and 16/72 (9B). Interface defects dominated (path syntax, optional answer values,
  implicit proposals) and were fixed in v2. v1 and v2 scores are not comparable.
- **v2 numbers:**

  | Model | Correct | False-supported, all | False-supported, at-risk |
  |---|---|---|---|
  | 4B | 79/282 (28.0%) | 58/282 (20.6%) | 33/112 (29.5%) |
  | 9B | 153/282 (54.3%) | 70/282 (24.8%) | 46/112 (41.1%) |

  At-risk correct: 9B 42/112, 4B 25/112 (112 = 56 at-risk cases × 2 seeds). 9B per outcome:
  answer 47/68, proposal 64/102, conflict 26/46, ask_fact 10/24, stale 5/12, ask_rule 1/24
  (search loops to the 8-turn limit), refuse 0/6.
- **Conclusion:** both models fail the bar on both denominators and under every labelling
  (excluding the 14 abstaining `answer` episodes and 8 ambiguous-gold episodes still leaves 9B
  at-risk false support at 24/112). The failure is systematic: the 9B is false-supported on
  both seeds in 26/141 cases. Main sources: following injected item text (9B obeyed it in
  27/44 Dara episodes), unsupported answers, wrong or incomplete at-risk proposals. The fair
  harness fixes considered look insufficient. Typed actions, untrusted-text handling and an
  abstention/routing stage are untested hypotheses.
- **Limitations:** the 141 cases were read during failure analysis, so they are now dev cases;
  a fresh blind held-out set was frozen
  ([tool-loop-heldout-v3-freeze.md](tool-loop-heldout-v3-freeze.md)). Untested levers:
  thinking on, more turns, other quantisations and temperatures. Scoring leniencies by design:
  value-only answer matching, empty citation lists count as valid, `answer` on an expected
  proposal is incorrect but not false support, `no_outcome` is never false support.
  Independently reviewed (three reviewers).

## Small-model candidates: CPU S40 screens (2026-10-17)

- **Question:** is any small open model (≤ ~8B) a better tool-loop candidate than the Qwen 9B?
- **Setup:** harness v3, C0 settings (thinking off, temperature 0.1, 8 turns, max_tokens
  4096, ctx 24576), dev subset `dev-screen-40` (40 cases, 16 at-risk), seed 17, pre-rev3 pack,
  frozen commit `b085aa4`, **CPU only** (6 threads). The 9B and 4B references ran on the GPU
  earlier, so the device differs. The 8B dense candidates (Granite 4.2 8B, Llama 3.1 8B,
  Ministral 3 8B) were not screened (too slow on CPU).
- **Numbers** (all of 40; at-risk false support of 16 where recorded):

  | Model | Correct | False support | At-risk FS |
  |---|---|---|---|
  | Qwen3.5-9B (GPU reference) | 25 | 8 | — |
  | Gemma 4 E4B (Q4_0) | 19 | 10 | 4/16 |
  | LFM2.5-2.6B | 17 | 18 | 10/16 |
  | Qwen3-4B-2507 (GPU reference) | 14 | 17 | — |
  | Granite 4.2 3B | 13 | 16 | 12/16 |
  | MiniCPM5-2B | 10 | 4 | 1/16 |
  | LFM2.5-8B-A1B | 8 | 9 | — |

- **Conclusion:** none beats the 9B; all are dropped as standalone candidates. Proposals are
  comparatively good (Gemma changes_exact 11/15) while outcome decisions are poor (Gemma
  answer 1/9), which motivated the decision-router screen.
- **Limitations:** unreviewed self-analysis; one seed, 40 cases; CPU versus GPU references;
  the 4B reference figures were reported, not re-verified. CPU speed: about 122 s per episode
  (LFM2.5-8B-A1B preflight).

## Decision router SYS-R: raw-readout routers on dev (2026-10-17/18)

- **Question:** can a separate model pick the outcome kind (7 options) from the C0 initial
  state, so that supported routes are safe (0 unsafe) with supported recall ≥ 84/105?
- **Setup:** dev pack rev3, 175 cases (105 supported, 70 at-risk), CPU only, grouped 5-fold
  nested CV for the threshold τ, pre-registered gate (recall ≥ 84/105, 0 unsafe, ≤ 8
  failures). Ablation and order-flip units on the 40 S40 ids. Routers: R4 Kev 0.8B
  (out-of-distribution 4-label head), R1 Qwen3-4B raw readout, R3 Qwen3.5-9B raw readout.
  Reports hold counts and hashes only.
- **Numbers:**

  | | R4 Kev 0.8B | R1 Qwen3-4B | R3 Qwen3.5-9B |
  |---|---|---|---|
  | Raw top-1 correct | 68/175 | 100/175 | 87/175 |
  | Raw unsafe, all / at-risk | 56/175, 41/70 | 46/175, 43/70 | 26/175, 26/70 |
  | Raw supported recall | 53/105 | 79/105 | 48/105 |
  | Out-of-fold supported recall (0 unsafe) | 0/105 | 0/105 (τ NONE) | 18/105 (τ 0.95) |
  | Gate | fail | fail | fail |
  | At-risk top-1 vs B-risk 29/70 | 15/70, null | 21/70, null | 39/70, not null |
  | Ablation full / empty / request-only (of 40) | 13 / 15 / 6 | 26 / 9 / 13 | 23 / 9 / 9 |
  | Order flips (of 40) | 20 | 18 | 11 |
  | Median s per decision | 14.1 | 152.5 | 241.9 |

  S40 fixed-output replay on the C0 9B ledger (non-gating; C0 alone 25/40 correct, 8/40 false
  support): R4 4/40 and 0, R1 10/40 and 0, R3 16/40 and 3.
- **Conclusion:** null for all three; no live SYS-R decision file was built. The raw-readout
  router does not reach the gate with these models, this prompt and this state. For R3 even
  the raw ceiling (48/105) is below the floor. Strong option-order (primacy) effects.
- **Limitations:** the proposal option's precondition (a clean `propose_change` preview)
  cannot be met from the router input, while `answer` can, which predicts R3's
  proposal → answer errors (instrument defect; any follow-up must make every option decidable
  from the rendered state). The router sees the C0 initial context only (collection entries
  carry only id and name; request-only retrieval). Raw single-order readouts, no permutation
  averaging or calibration. R4 was out of distribution. R3 ran past its pre-registered 12 h cap
  by user decision (budget rule only). The S40 replay mixes a pre-rev3 ledger with rev3
  expected outcomes. A post hoc binary supported/at-risk readout for R3 admits only 10/105 at
  zero at-risk. The 175 records are now development data. Independently reviewed with
  recomputation.

## Reviewer benchmark (2026-10-16, concluded)

- **Question:** which models give accurate independent reviews of Mimik plans and results?
- **Setup:** 14 models on frozen, sanitized snapshots via `tools/external_review.py`
  (read-only opencode `reviewer` agent; Sonnet as a fresh subagent). Experiment 1: M3 plan
  review, scored against an adjudicated answer key (38 weighted points, extended to 44).
  Experiment 2: harness v2 results review (3 reviewers, no key). Experiment 3: harness v3 plan
  review (6 reviewers, adjudicated).
- **Numbers** (experiment 1, weighted recall /38): Sonnet 23 (61%, 0 wrong), Sol 20 (53%,
  0 wrong, the only one to find the weight-2 defect K40), Space Bunny 19 (50%, 3 wrong), Muse
  Spark 16 (42%, 2 wrong, 1 harmful), DeepSeek v4.1 Flash 14 (37%, 1 wrong), Luna 15 (39%),
  MiMo 15, GLM 10, Cat Jam 9, LongCat 7, Nemotron Super VL 7. LL-alpha, Nemotron Lightning and
  Nemotron Ultra gave no output. All free models together: 31/38. Experiment 3: Sol 18/18
  findings valid, DeepSeek 15/15, Sonnet 16/17, Space Bunny 25/28 with 2 harmful.
- **Conclusion:** Sonnet and Sol were consistently the most accurate. Space Bunny was the
  broadest free model and the most over-severe. Verdicts on their own are unreliable (four
  NO-GOs and two GO-with-amendments on one plan, all fixable). The reviewer setup in
  `AGENTS.md` follows from this.
- **Limitations:** one adjudicator, one task per experiment, mostly one run per model; two
  Sonnet runs on the same input overlapped by about half. Reviewers without a shell are at a
  disadvantage on data-heavy reviews (a grep failed on JSONL lines over 64 KiB).

## Legacy Pathfinder RAG diagnostic (2026-10-15)

- **Question:** can the old Pathfinder 2e RAG output be reused as Mimik evidence?
- **Setup:** read-only stdlib inspection of the legacy README, extractor, index and retriever,
  plus their files (counts only).
- **Numbers:** 7 Markdown files, 10,340,102 bytes. 12,956 chunk records (10,986,792 bytes)
  with only `id`, `source`, `section` and `text`. 39 records over 1,500 characters, the
  longest 68,888. 0 duplicate IDs.
- **Conclusion:** not reusable directly as evidence. Chunks drop page and region, source hash,
  extraction identity and review state. Breadcrumbs keep only heading levels 1–2. The embedder
  caps input at 512 tokens, so long chunks risk truncation. Source filtering happens after
  global retrieval. The existing FAISS+BM25 setup shows that adding vectors does not fix
  missing evidence.
- **Limitations:** no live retrieval was run, no PDF was compared visually, and no error rate
  was estimated.
