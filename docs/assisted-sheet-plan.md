# Assisted sheet plan: remaining steps

Direction (user, 2026-10-19): "Help the user as much as we can, but don't force anything;
improve paper and pencil, don't replace it." The player writes what they want; the app
computes, warns, suggests and offers Undo. Model output is only ever a confirmable proposal;
legality stays advisory and Rust does all arithmetic.

Decision IDs (D-…, U…, P…) are defined in `docs/decisions.md`. What is built (number entry,
notes and scribbles, switch and Undo, unknown values, per-field save, the busy page, the page
bound, quick-add, the validity flag, the sheet module with save on leaving a field) is described
in `docs/assisted-entry.md`; this plan only adds to it.

Remaining, in order (2b measured and reviewed, `docs/results.md`; ASSIST-27 to ASSIST-31
decided): **2c** interpretation with the native model runtime, **2d** owner model setting,
**3** bundles and reminders, **4** suggestions and "Which Sword?", **5** docs and gate. The
model runs GPU-first with the CPU as the guaranteed baseline (ASSIST-27), as a native
`llama-server` child that Mimik manages, never a container in production (ASSIST-29,
ASSIST-30); budgets come from a measurement at each start within fixed caps (ASSIST-31).
Items marked **proposal** below await the user or the plan review.

Every step: its own branch; failing-first tests with the red run recorded; independent
implementation review before merge; built and tested on 2 cores on weekdays; GPU and live
model runs only at night or at weekends (machine schedule). Each step keeps
the demo (`tools/assist_demo.py`) usable. Any step that adds per-cell markup reruns the
worst-case page fixtures in `src/campaigns/list_tests.rs` (tightest headroom today 2.0%, a new row kept read-only from a full list) and
never enlarges the budget.

**POC cuts** (no security or integrity risk): player `*` and `/`; interpreting text, enum or
boolean cells; notes and Undo for text cells; seals and note verification (D-T); dismissal
records; Undo of creation, row add and moves; forced Undo; live preview; a keyed rate
limiter; Gemma; changes to the native MÖRK BORG editor. Non-goals: legality rulings,
automatic application of model output, chat, manual RAG, `catalogue_ref` membership checks,
hosted providers, measuring or suggesting models above about 4B, gated or private Hugging Face
repositories, split (multi-file) GGUF models.

## 1. Design for the remaining steps

### 1.4 Interpretation service (2c)

POC boundary (2026-10-20, after the plan review of the harness design): one local model, one
bounded request per job, current unresolved integer scribbles only, strict validation, origin
binding, poll and apply, the confirmable UI. Plan review round 2 (2026-10-20) fixed the
expression form, the phrase-number lexer, the start route's note key, `ambiguous` replies and
the breaker outcomes. Deferred to §4 Later: model escalation, model
tools, confidence gating, persistent traces and a known-answer set grown from them, and
catalogue, enum or fuzzy matching before the model (that matcher arrives with step 4, one
implementation for both).

- **Shape:** an `Interpreter` trait, a `StubInterpreter` and a crate adapter; further models
  can sit behind the same trait later. Assistant off is the default (`--assistant` turns it
  on); when off there is no client, no child process, no job route and no "Thinking…", and the
  app is fully usable. The model never blocks
  a save: a job starts only after the save commits.
- **Input:** only a scribble that number entry (`entry::read`, see `docs/assisted-entry.md`)
  could not read, i.e. the target's current `unresolved` note. The phrase is read server-side
  from that note, never sent by the client. A parsed entry is never offered again, and a
  sign-led entry keeps its switch offer (D-Q).
- **Origin binding:** a job is bound to (sheet, target address, host ID for row cells, the
  origin note's revision). Any revision that writes a note for that target through any route
  (unit, legacy full form, switch, Undo, apply) makes the origin non-current (a no-op save
  writes nothing and changes nothing): the job is cancelled if still
  running, its options are dropped, and apply refuses it, even when the stored value is
  unchanged (scribble A → scribble B, or scribble → typed value). A job request for an older
  note never supersedes a newer one. A newer job of the same account, for any target
  on the same sheet, supersedes its running or queued job (the latest edit wins, by origin revision, which is a
  sheet-wide counter, not by request arrival; equal revisions on two targets by arrival).
- **Request builder:** one function renders a typed `Request` (phrase, target, context items).
  The instructions and schema are a fixed prefix, byte-identical across requests (tested), so
  llama-server can reuse its prompt cache; the dynamic part is at most the bound B set at
  start (§1.4a: at most 4 KiB on the GPU path, 1 KiB on the CPU path), measured in bytes after
  escaping, with whole context items dropped from the end, never cut. The phrase, the target's
  ID and its value are never dropped; the target's label is dropped before them; if they still
  exceed B the job ends "too long for the assistant on this machine" (no request, neutral for
  the breaker). Every dynamic string goes inside a fixed data envelope that the instructions
  declare to be data, never instructions.
- **Context (D-F, D-J):** phrase and target first; then other stored integer and enum fields in
  definition order; derived, text, multiline, notes and list cells are excluded from context
  and from `@` references; catalogue entry text comes in step 3 and is dropped first.
- **Related fields only (ASSIST-31):** whenever B is 1 KiB (always on the CPU path) the context
  holds only fields related to the note, in three tiers, each in definition order, then dropped
  from the end as above: (1) fields the phrase names: the field's label or ID, normalised
  (NFKC, case fold, diacritics removed, `_` and punctuation read as spaces), occurs as a run of
  whole words in the normalised phrase ("half my max hp" names Max HP); (2) fields that a
  derived value or check of the pinned formula layer reads together with the target, directly
  or through derived values (none for pins without a formula layer); (3) for a row cell, the
  other integer and enum cells of its row; for a scalar, the integer and enum fields of the
  groups that contain the target. A field already in a higher tier is not repeated; excluded
  types stay excluded; nothing else is sent, and a reference to an unsent field drops the
  option as for any unsupplied field.
- **Output contract:** JSON, checked by llama-server's schema grammar and again in Rust
  (`jsonschema`, then typed Serde decoding, then domain checks), with explicit limits: at most
  3 options, each an expression and an explanation of at most 120 characters, or an
  `ambiguous` reply listing at most 3 readings. A reading is display-only text of at most 120
  characters, rendered escaped inside `<bdi>`, with no Use button; an `ambiguous` reply never
  writes. The expression is a JSON tree, not a string: a node is `{"lit": n}`, `{"ref":
  "<field id>"}` or `{"op": "add" | "sub" | "mul" | "div", "l": node, "r": node}`, and a
  `div` node also carries `"round": "floor" | "ceil" | "exact"` (`exact` drops the option on a
  remainder); depth at most 3 below the root, so at most 8 leaves. A negative `lit` is allowed;
  its magnitude is what provenance checks. References name supplied context fields only.
  Rust evaluates with checked `i128` arithmetic and range-checks the result to `i64` as
  `entry.rs` does; overflow, division by zero, a reference to an unknown value (D-Z), to a field
  not supplied, or to an excluded type drops the option. Literal provenance: the magnitude of
  each literal must equal a number found in the phrase by the phrase-number lexer, or the
  target's current value, or (only when the phrase contains a fraction or percent word from a
  fixed list, matched as whole words after case folding and diacritic folding: EN half,
  third, quarter, percent, `%`; IT metà (also `meta`), mezzo, mezza, terzo, terza, quarto,
  quarta, percento, per cento) one of 2, 3, 4 and 100 (the only other exception is the step-3 catalogue
  provenance extension, §1.5). The **phrase-number lexer** is new code with its own tests,
  separate from `entry::read` (which parses whole entries only) and not changing number-entry
  admission: only decimal digits (Unicode `Nd`, such as full-width digits) are folded to ASCII
  digits, never superscripts or circled digits (no general NFKC); a number is a maximal run of
  ASCII digits whose neighbour on each side is neither a letter nor any other Unicode number
  character (`N*`, such as `²` or `①`), that is not preceded by `.` or `,` at all, is not
  followed by a `.` or `,` that touches a digit (letters and `N*` are tested on the original
  characters, before folding); the separators `.` and `,` include U+066B, U+066C, U+FF0C and
  U+FF0E, and two runs joined by a single NBSP, U+202F or apostrophe (`1 500`, `1'500`) both
  give nothing; so `heal 15` and `15%` give 15, `take −15
  damage` gives 15 (signs are not part of the token; U+2212, NBSP and U+202F are separators
  as in ASSIST-1), `5.` at the end of a sentence gives 5, while `1.5`, `x.5`, `1,000`, `1e2`,
  `3d6`, `15hp` and `15²` give nothing. A leading zero
  (`007`) gives nothing; a run longer than 18 digits gives nothing. This rule and the output
  schema are frozen before the 2b screen. The result then
  passes the integer field's own admission, without the scribble fallback. The display shows
  the current value, the expression rendered with `− × ÷`, and the effect, labelled "the
  assistant's reading". `max_tokens` belongs to the model: 192 for Granite 4.2 3B (the 2b rule
  on the 2b screen, `docs/results.md`), and from the 2d trial by the same rule for any other
  model. Two changes to this rule are open proposals (2c, "Open for review"): derived
  constants and the fraction words "halve" and "dimezza".
- **Confidence:** if the runtime returns token log-probabilities they are recorded on the job
  record for evaluation only; nothing is gated or hidden on them in 2c.
- **Crate (D-H, 2b proposal):** `rig-core` =0.44.0 (0.45 needs Rust 1.97.1),
  default features off plus `reqwest`, on the OpenAI chat route with the llama.cpp
  dialect; the schema goes in `additional_params` as `response_format` (sent byte-exact; the
  native `output_schema` path rewrites `oneOf` to `anyOf`). The body bound is a small Mimik
  transport implementing rig's `HttpClientExt` over the host-built `reqwest` client (plain and
  streaming bodies refused past the bound); no D-H crate offers one. `genai` was rejected (no
  body hook). The per-launch token (§1.4a) is rig's API key, sent as `Authorization: Bearer`.
  The model client uses no TLS; from 2d the binary contains TLS for downloads (§1.4b), and
  Cargo unifies `reqwest` features, so the model client's plain-HTTP property rests on the
  guard below (it refuses `https` and every non-loopback host), not on the dependency tree.
- **STROT** (judged on results, user 2026-10-19): the tested configuration (Julia-1 through
  strot-ai choosing among 16 to 71 generated arithmetic-string candidates plus "none",
  threshold 0.40, one selection) is rejected (2 of 50 correct, 42 false-confident); a design
  with a few semantic labels was not tested.
- **Client policy:** one client built by the host for the one configured endpoint: no
  redirects, proxies disabled whatever the environment says (`HTTP_PROXY`, `ALL_PROXY`), no TLS
  features, no decompression (`no_gzip`, since the feature can be unified in), no retries,
  and before every request the URL host must be an IP literal equal to `127.0.0.1` or `::1`,
  with no userinfo or zone ID (`localhost`, `127.1` and `::ffff:127.0.0.1` refused), checked on
  the raw authority string as well as the parsed URL (the WHATWG parser turns `127.1` into
  `127.0.0.1`). The port is the one the runtime manager confirmed for the current child
  (§1.4a); the token goes only to that endpoint. The model name is configuration, never model
  output. Loopback is only the destination rule; the no-egress property comes from the OS
  hardening of the child (§1.4a), where the platform has it.
- **Runtime:** a native `llama-server` child managed by Mimik (§1.4a). The 2b container
  topology (internal Docker network with `inhibit_ipv4` and a socat forwarder) is development
  and benchmark evidence only (`docs/results.md`, ASSIST-L21) and is not shipped. Model:
  configuration in 2c, the owner setting from 2d (§1.4b); default Granite 4.2 3B (ASSIST-28).
- **Jobs:** one worker; one monotonic budget per job from enqueue, covering queueing,
  rendering, the model call and validation: queue wait Q plus run deadline R, with R, the queue
  depth D, Q and B derived at each start (§1.4a). One job per account (counting running work): a start on another sheet while the
  account has a live job is 429 (origin revisions are per sheet and cannot be compared);
  admission (breaker and capacity) is computed as if the superseded job were already gone, so a
  refused start cancels nothing (Routes); an admitted start cancels the superseded request (the
  connection is closed and its slot freed before the new job is enqueued), and a late answer
  from a cancelled job is discarded. At most 64 job records in all, 128-bit IDs; an evicted or expired
  job answers "ask again". Options expire 10 minutes after the job finishes. Poll and apply
  recheck rights; the `JOBS` permit is not held during the model call. A reminder job (step 3)
  takes its own low-priority slot on the same worker, after interpretations, one per account.
- **Circuit breaker:** one per configured endpoint, in memory (reset on restart). It counts
  only transport errors, HTTP 4xx and 5xx, a response over the body bound, and run-deadline
  expiry. A completed, bounded HTTP 2xx exchange is a success whatever its content (an answer
  whose options are all dropped still resets the count and closes a half-open breaker);
  supersession, cancellation and queue-wait expiry (the model was never called) are
  neutral. Three consecutive counted failures open it for 2
  minutes; then one half-open probe job decides (success closes, failure reopens; a cancelled
  probe clears the in-flight probe flag, leaves it half-open, and the next job becomes the
  probe; while the probe is in flight other starts answer 503 like an open breaker). While open
  no request is sent: a start answers 503 and a job already queued answers on poll, both with
  "the assistant is unavailable right now"; number
  entry, notes and Undo work as usual. A small state machine with an injectable clock:
  `failsafe` 1.3.0 as tested in 2b allows multiple outstanding half-open admissions and its
  future API records a non-counted error as a success; its manual API could express neutral
  outcomes but has no probe flag, its mock clock is thread-local and its last release was
  2024-07. Priming, calibration and the 2d trial are outside the breaker.
- **Routes:** `POST /private-sheets/{id}/interpretations` (exactly `csrf` and `note`, the
  note key already used by switch and Undo and present in the saved page's markup) starts a
  job for that note. A key is resolved against the snapshot at its own revision: a typed key
  directly, an indexed row-cell key (`<revision>:<f>.<r>.<p>`) by mapping the row index to its
  host ID at that revision, giving the typed address with host ID. Order as for the other
  sheet routes: session and CSRF, rights (403), keys and malformed values (400); then, before
  any cancellation or capacity handling, the typed address's current note must be that
  revision's `unresolved` note (409 otherwise; unrelated later revisions are fine, a row moved
  since is found by host ID, a removed row is 409); then a repeat for the origin of an existing
  job that is live or finished with unexpired, unapplied options returns that job (a finished
  job without options, or with expired ones, does not count: the start goes on as a new job);
  then a start whose origin revision is lower than that of the account's live job on the sheet
  is 409 (an equal revision on another target, as from one full-form save, supersedes: the
  later arrival wins); then the
  breaker (503) and capacity (429), computed as if the superseded job were already gone, so a
  refused start never cancels anything (a live job of the account on another sheet is never
  superseded and gives 429); then supersession and enqueue. The in-memory job-table
  lock covers this check-then-enqueue and the post-commit cancellation hook of every save; it
  is never held across a database wait, the permit wait or the model call. The current-note
  check reads the database before the lock, so under the lock the start also compares its
  origin with an in-memory per-sheet map of the latest note-write revision per target (row
  cells keyed by host ID), which every note writer's post-commit hook (saves, Undo, switch,
  row removal) updates and uses to cancel stale jobs in one critical section of the same lock:
  a write newer than the origin gives 409. Entries are dropped once older than the longest
  start (a size-bounded sweep) and with their sheet or row; the map and the job table are
  per process and empty after a restart, when the database check alone suffices. 202 with the job
  ID. `GET
  …/interpretations/{job}` polls (the module polls at 500 ms, backing off to 2 s, and stops on
  unload, supersession or a terminal state); an evicted or expired job answers "ask again".
  Another account's or sheet's job is not found, on poll as on apply.
  Apply: `POST /private-sheets/{id}/proposals/{job}/{n}` (exactly `csrf`), taking the
  definitions permit with the D-X3 bounded wait like the other sheet writes.
- **Flow (JS):** a cell save records `unresolved` and the module starts a job; "Thinking…" in
  `role="status"`; then the options or the readings of an `ambiguous` reply; **Use** posts the
  apply route; ignoring them leaves the scribble. The apply route must not rely on how long an
  offer stays on screen or treat a shown offer as confirmation.
- **Proposal freshness (D-P):** when a job finishes, Rust validates each option and stores it
  with a content hash and its bound read/write set: the origin binding above, every write
  target (address, value at the job's base revision, host ID for row cells), every `@`
  reference (address and value), the displayed effect (before → after per target), and for
  reminders and bundles the supplied context (each list-row name sent, its remembered key, the
  catalogue version and entry key of any entry text, every enum value). Bundle targets must be
  distinct; a duplicate drops the option. The guarantee covers exactly this set, not the
  model's relevance judgement.
- **Apply:** in one `IMMEDIATE` transaction: recheck rights and the active state; check the job
  binding, the option's content hash, its expiry and that the job is not already applied
  (options are alternatives: applying one consumes the job, marked only after commit); require
  the origin note to be current, every bound address to hold its bound value and every bound
  row to exist; re-evaluate and require the effect to equal the displayed one; write with
  `confirmed` notes sharing a bundle number scoped to the sheet and job. Any mismatch or
  expiry: "Something this proposal used has changed: ask again", nothing written, never a
  literal fallback. An unrelated field change does not invalidate it. Undo of an applied
  proposal works like any Undo (`reverted` note).

### 1.4a Model runtime, device and budgets (2c; ASSIST-27, ASSIST-29 to ASSIST-31)

- **Process:** with the assistant on, Mimik runs one official llama.cpp `llama-server` as its
  child; production never needs a container (ASSIST-29). The child is started through a hidden
  subcommand, `mimik assistant-runner`, which applies the OS hardening below to itself and then
  executes llama-server (Linux), so no `unsafe` pre-exec hook is needed. Flags: `--host
  127.0.0.1 --port <p>`, the token (below), `--parallel 1`, `--no-warmup`, `--offline`
  (hygiene only, not confinement), web UI off, `--reasoning off`, context 8192, `--threads` and
  `--threads-batch` from configuration (default 2, as measured), the model path and the device
  flags below; the greedy sampler (temperature 0, top-k 1, seed 0) is sent per request as in
  2b. Every flag is checked against the pinned build's `--help` by a 2c test.
- **Token:** 32 bytes from the OS random source, base64url, new for every launch, held in
  memory; handed to the child through `--api-key-file` on a file readable only by Mimik's user
  in a private runtime directory (deleted once `/health` answers) or the `LLAMA_API_KEY`
  environment variable, whichever the pinned build supports; never on the command line (other
  users can read command lines) and never logged. `/health` needs no token; every other
  request does. 2b did not exercise token auth, so 2c tests both ends.
- **Port:** Mimik binds `127.0.0.1:0` to pick a free port and releases it, then waits for the
  child's own "listening" line on its stdout pipe before sending anything; a child that exits
  first, or no line within the health budget, means a new port (at most 3). On Linux, before
  the first request to a new child, Mimik also checks through `/proc` (`procfs` crate) that the
  listening socket on that port belongs to the child, so a process that took the port first
  never receives the token or a phrase. macOS (`libproc`) and Windows (`GetExtendedTcpTable`)
  have owner-PID lookups for the same check; unverified here.
- **Binary provenance:** a manifest compiled into Mimik pins one llama.cpp build (b11610 as in
  2b unless the review picks a newer one) and, per target, the official GitHub release asset,
  its SHA-256 and the SHA-256 of every file Mimik runs from it: Linux x86_64 Vulkan and CPU
  builds, Windows x64 Vulkan and CPU builds, macOS arm64 (Metal and CPU in one build).
  `package_release.py` downloads the assets in CI, verifies them against the manifest and puts
  the files and llama.cpp's MIT licence beside `mimik` in the runtime archive (`llama/`); the
  source archive does not carry them. Mimik hashes the files before each start and refuses a
  mismatch (assistant unavailable, saying why). A development override (`--assistant-runtime
  <dir>`) is accepted only when its files match the manifest. **Proposal:** bundling in the
  archive (size measured in 2c) rather than owner-supplied files.
- **Device (ASSIST-27, HW-1):** at each start Mimik runs the GPU build's `--list-devices`
  (30 s timeout). `auto` (the default) takes the first listed GPU that is not a software
  renderer (`llvmpipe`, `lavapipe`, `SwiftShader`); `--assistant-device cpu` forces the CPU and
  a device name selects that device. The GPU path offloads every layer (`-ngl 999`, `--device
  <name>`, the build's automatic layer fitting off if it has one), never a partial offload. The
  CPU path uses the CPU build (Linux, Windows) or `-ngl 0 --device none` (macOS). A Vulkan
  build that cannot start (for example no Vulkan loader) is a GPU failure.
- **Fallback:** a GPU failure is any of: no usable device; the GPU child exits before
  `/health` or fails it; priming or calibration fails or expires on the GPU; the GPU child exits
  while ready or is stopped as hung (Lifecycle). On a GPU failure Mimik starts the CPU child at once (not counted as a retry) and
  stays on the CPU path until Mimik restarts or the owner presses "Try the GPU again" on the
  assistant page, which shows the path and the reason. CPU failures follow the retry rule; there
  is no further fallback.
- **Lifecycle:** states `off`, `starting` (spawn, `/health`, priming, calibration), `ready` and
  `unavailable` (with a reason). Budgets per attempt: `/health` 60 s, priming 300 s, calibration
  twice the path's run-deadline cap. A failed attempt is retried after 2 minutes, at most 3
  times; then the assistant stays unavailable until Mimik restarts or the owner presses
  "Restart" (owner only, CSRF). A child that exits while ready is restarted and re-primed; a
  second exit within 10 minutes of becoming ready counts as a failed attempt, so a crash loop
  ends unavailable. After a counted transport error or a run-deadline expiry Mimik checks the
  child: if it has exited, or `/health` does not answer within 5 s, it is stopped and restarted
  (the assistant is `starting` meanwhile). On shutdown Mimik stops the child (SIGTERM, SIGKILL
  after 5 s; Windows: the job object). Orphans: on Linux the runner sets `PR_SET_PDEATHSIG`
  (SIGKILL) and is spawned from one dedicated long-lived thread (the signal follows the
  spawning thread); on Windows the child is in a job object with kill-on-close; on macOS the
  runner stays the child's parent, watches its stdin pipe from Mimik and kills llama-server
  when the pipe closes. A pidfile (PID, process start time, binary path) in the data directory
  lets the next start kill a leftover child only when all three match. While `starting` or
  `unavailable`, a job start answers 503 with the state ("the assistant is starting", "the
  assistant is unavailable right now"), queues nothing, and number entry, notes and Undo work
  as usual. Logs carry states and reasons, never prompt text or the token.
- **Hardening (ASSIST-30)**, one state shown on the assistant page and logged at each start;
  an unconfined child still runs (hardening is "where available"):
  - Linux: the runner sets `no_new_privs`, then applies Landlock through the `landlock` crate:
    network rules (ABI 4 or later, Linux 6.7 or later) allow a TCP bind on the chosen port only
    and no TCP connect; filesystem rules allow reading the runtime files, the model file,
    system libraries, Vulkan driver files and the `/dev/dri` render nodes (GPU path), and
    writing nothing (the exact list is fixed by the 2c tests on both paths); abstract Unix
    sockets are scoped where the ABI has it (6 or later). Landlock does not cover UDP or raw
    sockets, so a `seccompiler` filter also refuses `socket()` for anything but TCP over IPv4 or
    IPv6 and pathname Unix sockets (which the filesystem rules limit). States: "network
    confined (Landlock ABI n, seccomp)"; "not network-confined: Landlock without network rules
    (filesystem only, seccomp)"; "not network-confined: Landlock unavailable (seccomp only)".
    This host's kernel is built without Landlock (`CONFIG_SECURITY_LANDLOCK` not set), so only
    the last state is observable here.
  - macOS: the runner starts llama-server under a sandbox profile (`sandbox-exec`, deprecated
    by Apple but shipped) that denies all network except binding and accepting on the loopback
    port. States: "network confined (sandbox profile)", "not network-confined". Not verifiable
    here.
  - Windows: no network confinement is claimed without administrator rights (firewall rules
    need them; an AppContainer without network capabilities is a candidate whose loopback
    behaviour must be verified first); the job object covers lifetime only. State: "not
    network-confined (Windows)". Not verifiable here.
  - The whole app as one container image (ASSIST-29): the same rules inside; the state reports
    what that kernel and the container's seccomp profile allow.
- **Priming and calibration:** priming as measured in 2b: the server starts with an empty
  prompt cache, and the first request evaluates the whole fixed prefix (about 1,460 tokens:
  117 s for Granite at 2 CPU threads, about 1.3 s on the RX 6650 XT), so after `/health` the
  host sends the fixed prefix with a minimal data envelope and `max_tokens` 1. Then one
  calibration request: a fixed synthetic job shipped with Mimik (no sheet or player text) whose
  dynamic part is filled with synthetic context items to exactly the path's cap (CPU 1,024
  bytes, GPU 4,096), with the model's `max_tokens`. Both use the same client and policy, are
  not jobs and are outside the breaker. From llama-server's `timings` in the calibration reply:
  P (prompt seconds for the dynamic part, prefix cached), G (generated tokens per second,
  `predicted_n / predicted_ms`) and O (wall time minus prompt and generation time). The
  calibration must be schema-valid with `predicted_n` at least 8, or the attempt fails.
- **Budgets (deterministic, recomputed at every start; proposal for the margin and floor):**
  caps from ASSIST-31: run deadline 90 s and dynamic part 1,024 bytes on the CPU path, 60 s and
  4,096 bytes on the GPU path. For a candidate bound B (GPU: 4,096, then 2,048, then 1,024;
  CPU: 1,024 only), the worst case is T(B) = O + P × B / cap + `max_tokens` / G (prompt time
  scaled linearly from the full-cap calibration, generation at the full `max_tokens`). B is the
  largest candidate with 1.25 × T(B) ≤ the deadline cap. Then, reusing the 2b rule: run
  deadline R = max(15, round up to 5 s of 1.25 × T(B)); queue depth D = max(1, min(4,
  floor((120 − R) / R))); queue wait Q = D × R. (The 2b rule took 1.5 × a p95 over a screen; at
  start there is one worst-case measurement, hence 1.25 on a worst case; the 15 s floor leaves
  slack on fast GPUs.) With R above 60 s, D is 1 and Q + R is 2R, up to 180 s on the CPU path.
  If no candidate fits, the assistant is unavailable ("too slow on this machine: about N s
  needed, the cap is M s") and the page shows the measurement; entry is never affected. From
  the 2b numbers: Granite on 2 CPU threads (P about 26 s for 1 KiB, G about 4.7, 192 tokens
  about 41 s) gives T about 67 s, 1.25 × T about 84 s, so B 1,024, R 85, D 1, Q 85; Granite on
  the RX 6650 XT (about 1,100 prompt and 80 generated tokens per second) gives T about 4 s, so
  B 4,096, R 15, D 4, Q 60. 2c pins both as unit tests from recorded timings.

### 1.4b Owner model setting (2d; ASSIST-28)

- **Before 2d:** 2c reads the model from configuration (`--assistant-model`, default
  `local:granite-4.2-3b-Q4_K_M.gguf` with its SHA-256 `e0406663…` compiled in; another `local:`
  file needs `--assistant-model-sha256`), so 2c does not depend on 2d. The file is placed in the
  models folder by hand.
- **Page:** `/admin/assistant` (owner only; built in 2c with the state, path, protection,
  measurements, budgets, Restart and "Try the GPU again") gains one free-text field "Model".
  Models folder: `<data dir>/models/` (`--models-dir`), mode 0700.
- **Grammar** (at most 512 bytes, ASCII, no surrounding white space; anything else is refused
  naming the failed rule, and the field keeps the input):
  - `hf:<org>/<repo>/<path>@<revision>`: `org` and `repo` each 1 to 96 of `[A-Za-z0-9._-]`,
    starting and ending alphanumeric, without `--` or `..`; `path` one or more segments of 1 to
    128 `[A-Za-z0-9._-]` joined by `/`, no segment `.` or `..`, ending `.gguf` and not a split
    file (`-NNNNN-of-NNNNN.gguf`); `revision` exactly 40 lowercase hex digits (a commit, never
    a branch or tag). URLs are built with path-segment encoding, never by concatenation.
  - `local:<name>.gguf`: one name of 1 to 128 `[A-Za-z0-9._-]` ending `.gguf`, not starting
    with `.`, not a Windows device name in any case (`CON`, `NUL`, `COM1`…); opened only inside
    the models folder without following links (`O_NOFOLLOW`; the open handle must be a regular
    file whose canonical parent is the models folder); its SHA-256 is computed and recorded
    when the owner tries it.
- **Download (`hf:`, owner-started only):** (1) metadata from the Hub API at that revision (the
  returned commit must equal it; the licence from the model card; the file's LFS SHA-256 and
  size), JSON bounded at 1 MiB; 401 or 403 (gated or private) is refused with "download it by
  hand and use `local:`". (2) The page shows repository, file, size, SHA-256 and the licence (ID
  and a link to the licence file at that revision, or "not stated"); the owner confirms the
  licence, and the confirmation (owner, time, licence ID) is recorded; nothing downloads before
  it. (3) Size at most 8 GiB (**proposal**) and free space at least the remaining bytes plus
  1 GiB. (4) One download at a time, in the background, into `models/.partial/<sha256>`:
  progress on the page (polled with JavaScript, a reload without), Cancel (keeps the partial),
  Discard (deletes it), Resume with `Range` (anything but a 206 with the matching
  `Content-Range` restarts from zero); the body is bounded by the expected size; the SHA-256
  must match before the file is renamed to `models/hf/<sha256>.gguf` (names never come from the
  spec), otherwise it is deleted. After a restart a partial waits for Resume: nothing downloads
  without an owner action. The active model keeps serving throughout.
- **Download network policy:** a client separate from the model client: HTTPS only (rustls;
  the root store is a review item), API and `resolve` requests only to `huggingface.co`;
  redirects followed only from a `resolve` URL, at most 2 hops, each HTTPS to a host on a fixed
  allowlist of Hugging Face's file hosts, fixed in 2d from Hugging Face's documentation and the
  observed redirects (today `cdn-lfs.huggingface.co`, `cdn-lfs.hf.co`,
  `cas-bridge.xethub.hf.co` and their regional variants are candidates); any other host stops
  the download and names it. No cookies, credentials, proxies (as the model client) or
  `Referer`; a fixed `User-Agent`; a request carries only what the owner's spec names, never
  sheet, note or player text. Connect timeout 30 s, idle read 60 s.
- **Trial and switch:** "Try" on a downloaded or local model verifies its hash, stops the
  current child (the assistant answers 503 "trying a model" like `starting`), starts the
  candidate on the current device path with priming and calibration at `max_tokens` 384 (the
  measurement cap), then sends 5 fixed synthetic phrases shipped with Mimik. It passes when
  every reply is schema-valid and untruncated and the budgets derive (§1.4a). The page shows
  the device, priming and calibration times, prompt and generation speed, the model's
  `max_tokens` by the 2b rule (max(128, round up to 32 of 1.25 × the largest `predicted_n`),
  at most 384), R, D, Q and B, and per phrase whether the expected value was among the options
  (information only, D-D). "Switch" makes it active (recorded with its hash, measurements and
  `max_tokens`); "Keep the current model" restarts the previous one, as does a failed trial.
  "Switch back" restarts the previous active model in one click. Every load verifies the
  recorded hash first; a mismatch makes the assistant unavailable ("the model file changed")
  until the owner tries a model again. A non-active downloaded model can be deleted.
- **Default and suggestion:** default
  `hf:ibm-granite/granite-4.2-3b-GGUF/granite-4.2-3b-Q4_K_M.gguf@c40945d71cd90f249a56985e8155551a9188dc30`;
  on the GPU path the page suggests
  `hf:bartowski/Qwen_Qwen3-4B-Instruct-2507-GGUF/Qwen_Qwen3-4B-Instruct-2507-Q4_K_M.gguf@ae44f08e1392f39c0e474af10c3ff8355c8b6688`
  (the 2b pins; it fits the GPU path on the RX 6650 XT, `docs/results.md`; other GPUs are
  unmeasured). A suggestion only fills the field.
- **Security:** owner only (403 for others, as the other admin pages), CSRF and body bounds on
  every action; one download and one trial at a time; the spec is configuration, never model
  output. A downloaded GGUF is untrusted input to llama-server, contained only by the
  hardening, so the protection state is shown next to "Try". Storage: migration 022 (models:
  spec, SHA-256, size, licence and its confirmation, trial measurements, `max_tokens`; the
  active and previous model).

### 1.5 Bundles and reminders (3)

- **Bundles:** up to 4 changes to integer, enum or boolean addresses (scalars or saved-row
  cells by host ID), no row adds or removes; one revision; notes sharing `bundle`; freshness
  as above. Undo on any member reverts all members, only if every member's current note is
  still that bundle and holds its `after`; otherwise nothing is reverted and the page lists
  what changed.
- **Reminders (D-W):** triggered by a saved decrease of any top-level integer field, with the
  assistant on and JavaScript present; the model's `relevant` flag judges relevance. Context:
  the decrease, numbers and enums, list-row names from properties with the `catalogue`
  member, and linked entry text (≤ 2,000 characters, quoted) when exactly one entry matches or
  the stored key resolves. Output `{relevant, row, hint ≤ 120, option?}`, the option being a
  bundle. Display: a dismissible hint (client-side only), never blocking or auto-applying.
  Injection is contained by the schema, the provenance extension below, the full display and
  confirmation.
- **Catalogue provenance extension (decided 2026-10-19, coordinator default):** in a reminder
  option, a number may also come from the text of the catalogue entry the reminder cites, and
  only if it appears literally in that entry's text. The cited entry is bound by catalogue
  version and entry key (part of the D-P read set). The option is displayed with its source
  ("from Shield: …"). Every other number must pass the step-2c rule (in the phrase, equal to
  the target's value, or a 2, 3, 4 or 100 admitted by a fraction or percent word); outside reminders the step-2c rule stays strict.
- **Catalogue member:** an optional `"catalogue": {"id": <link>, "entry_type": <id>?, "key":
  <property>}` on a text item property. `key` names a declared optional text property of the
  same list holding the remembered entry key (D-R); it is rendered as the link, never a
  control. Checked at the three step-3 catalogue-link sites (draft save, publication
  snapshot, readiness): the key property exists, is optional text, is not itself
  catalogue-backed, and is used by at most one hint. Resolved only through the immutable pin.
  Omitted when absent, so old hashes are kept.
- **Linking on view** (rules 1–2 here, 3–4 in step 4): names are normalised (NFKC, case-fold,
  diacritics removed, whitespace collapsed). (1) If the stored key resolves to an eligible
  entry (linked catalogue, type filter, not retired) whose normalised name equals the cell's,
  link it. (2) Otherwise link exactly one eligible exact match (D-O).

### 1.6 Names, "Which Sword?", suggestions (4)

- (3) Several matches: "Which Sword?" lists the candidates (name · type · catalogue); choosing
  one posts `cells/{list}/{host}/{key}` with the entry key: one revision, no note; the link is
  remembered (D-R). (4) None: "Not in the catalogue" plus Browse; with JS, "Did you mean…?" on
  demand. Saving the name cell with a different name clears the key in the same revision.
  A dangling key just shows no link.
- **Suggest route** (JS): read rights and pin correspondence rechecked per request; no `JOBS`
  permit, a pool of 4, 429 at saturation; query 2–64 scalars and ≤ 256 bytes; prefix match on
  cached names, then FTS, then an on-demand typo pass (±2 characters of length, ≤ 20,000
  comparisons and 20 ms, outside the DB mutex); the type filter on every path; a name cache
  LRU-bounded at 32 MiB, names only; p95 ≤ 50 ms, measured. Picking a suggestion stores the
  name, plus its key when it is one of duplicates.
- **Combobox:** `aria-controls` and unique option IDs; while open, Enter selects and does not
  submit; IME composition respected; debounce, `AbortController` and a generation counter.

### 1.7 Warnings still to add

| Warning | Source | Stored | Shown |
|---|---|---|---|
| Assistant failure, expiry, busy, stale | job / apply | no | inline (JS) |
| Not in catalogue / which one | linking on view | no | next to the cell |

Every warning is non-blocking and tied to its control with `aria-describedby`.

### 1.8 Security summary

Model output passes the schema, the grammar, Rust schema and domain checks, literal
provenance, checked Rust evaluation, origin binding and read/write-set freshness, and is rendered escaped inside `<bdi>`. Rights are rechecked on every
request and inside every write transaction. Limits: raw ≤ 200, notes ≤ 256 per revision,
dynamic prompt ≤ 4 KiB on the GPU path and ≤ 1 KiB (related fields only) on the CPU path, the
measured job budgets within their caps and the record caps, the breaker, the suggest bounds.
The model client talks plain HTTP to a loopback IP literal only, on the port confirmed for its
own child, with a per-launch token; it follows no redirects and ignores proxy settings. The
model server is an official llama.cpp build and a model file, both hash-verified before each
start; its network is confined by Landlock and seccomp on Linux kernels with Landlock network
rules and by a sandbox profile on macOS, and not confined on Windows or without Landlock; the
assistant page states which. Model downloads are owner-started, HTTPS to Hugging Face hosts
only, bounded, licence-confirmed and hash-verified, and carry no sheet or player text. No
prompts, player text or token in logs; no secrets.

## 2. Steps

### 2c Interpretation

Scope: trait, stub, adapter; the client policy and token; the runtime manager (manifest,
runner, device choice and fallback, lifecycle, hardening, priming, calibration and budgets,
§1.4a); the owner's assistant page (state, path, protection, measurements, Restart, "Try the
GPU again"); worker, records and breaker; the job, poll and apply routes with origin and
read/write-set binding; the related-fields context; the UI. The model comes from configuration
(§1.4b, "Before 2d"). Tests (stub, fake server or fake runner binary unless stated):
- output: schema-invalid, grammar-invalid and truncated JSON; more than 3 options; a tree deeper
  than 3 below the root (which is also what refuses a ninth leaf); literal provenance (`84 + 915` dropped; `5` from "heal 15" dropped; U+2212, NBSP);
  references to a derived, text, multiline, notes or list-cell field, to an unsupplied field
  and to an unknown value; `/` with each rounding mode on negatives; `exact` with a remainder;
  division by zero; `i64` boundary and intermediate overflow; a non-tree or string expression;
  `ambiguous` with more than 3 readings or a reading over 120 characters dropped, readings
  escaped and never applicable;
- lexer: `heal 15`, `take −15 damage`, `15%`, `1.5`, `x.5`, `5.`, `1,000`, `1e2`, `3d6`,
  `15hp`, `15²`, `①`, `007`, full-width digits, 19 digits, `1٫5`, `1．5`, `1，000`,
  `1 500` (NBSP and U+202F), `1'500`, `15,` then a space (15), `(15)` (15), `15/20` (15 and
  20); `heal,15` and `1,2,3` give nothing (accepted false rejects, erring towards no option); "half of 84" admits 2, "metà" and
  "meta" admit 2, "halfway" and "quarterly" do not, a phrase without a fraction word does
  not;
- input: a parsed entry and a sign-led entry never start a job; the phrase comes from the
  stored note, not the request; the fixed prefix is byte-identical across requests; the
  dynamic part stays within B (4,096, 2,048 and 1,024) and drops whole items; the target's
  label goes before the phrase, target ID or value; a phrase and target over B end "too long"
  without a request and without a breaker count; excluded field types never appear in the
  request;
- related fields (B 1,024): a field named by label and by ID ("max hp", "max_hp", "Max-HP",
  diacritics) is sent, a partial word ("maximum") is not; a field read with the target by a
  check or derived value is sent, also through a derived value; the same row's integer and enum
  cells for a row cell, the target's groups for a scalar; tier order, no repeats, drop from the
  end; an unrelated field is never sent, and an option referencing it is dropped;
- injection: a phrase containing "ignore previous instructions" yields at most an option,
  never a write without Use; a context value with instructions stays inside the data envelope;
- origin: scribble A → scribble B with the same stored value; scribble → typed value; a save
  through the legacy full form; reversed job-start requests; another edit after the job
  finished; a late answer after cancellation — none may revive or apply the old proposal; a
  delayed start for scribble A neither cancels nor restarts B's job (409); a repeated start for
  the same origin (a double click) returns the same job and cancels nothing; a job for another
  target of the same account supersedes the first; reversed starts across two targets (B's
  start first, then A's older one) → A is 409 and B keeps running; an indexed row-cell key
  after the row was moved resolves to the same host, after it was removed → 409; a save racing
  a start leaves either the job cancelled or the start refused, never a live job for a stale
  note;
- budgets: a queue full of other accounts' jobs → 429, and the refused start cancels nothing;
  a breaker refusal cancels nothing; a start that supersedes the account's running job is
  admitted with the queue otherwise full (counted as if the old job were gone); a note written
  between the start's database check and its enqueue → 409; two targets written in one
  revision → the later start supersedes; a start on a second sheet while the account has a live job → 429; cancellation (the
  fake server sees the disconnect and the slot is free); queue wait and run deadline expiry;
  64 records and the evicted-job answer; option expiry at 10 minutes;
- priming and calibration (fake server): a start before they complete answers 503 "starting"
  and queues nothing; a failure or expiry never counts toward the breaker and is retried after
  2 minutes at most 3 times, then unavailable until Restart; a calibration that is
  schema-invalid or has `predicted_n` below 8 fails the attempt; a restarted child re-primes and
  re-calibrates before admitting jobs; the calibration request carries no sheet text;
- budgets from timings (unit, recorded 2b timings): Granite CPU → B 1,024, R 85, D 1, Q 85;
  Granite GPU → B 4,096, R 15, D 4, Q 60; a GPU measurement that fits only 2,048 or 1,024 picks
  it; a CPU measurement over 90 s at 1,024 → unavailable "too slow" with the numbers, while
  number entry, notes and Undo work; R never below 15 or above the path's cap;
- token: the fake server sees `Authorization: Bearer <token>` on priming, calibration and
  every job, and a new token after a restart; live against the pinned llama-server: no token
  and a wrong token on `/v1/chat/completions` are refused (401), the right one is served,
  `/health` answers without it; the token is in no command line (`/proc/<pid>/cmdline`) and
  no log line; its file is removed once `/health` answers;
- runtime manager (fake runner binary): a manifest hash mismatch on any runtime file or the
  model → unavailable, no child; a development override with other files → refused; a port
  taken before the child binds → a new port; a squatter answering on the port while the child
  failed → no request sent (Linux socket-owner check); child exit while ready → restart and
  re-prime; a second exit within 10 minutes → counted, three → unavailable; a hung child (no
  `/health` within 5 s after a deadline expiry) → restarted; shutdown → the child is gone
  within 5 s; a pidfile matching a live leftover (PID, start time, path) → killed, a pidfile
  whose PID now belongs to another process → left alone; live on Linux: SIGKILL of Mimik kills
  the child (`PR_SET_PDEATHSIG`), also when the spawning request has ended;
- device and fallback (fake runner): `--list-devices` output with only `llvmpipe` → CPU path;
  a GPU child that exits before `/health`, fails priming, or exits while ready → CPU child at
  once, not counted as a retry, the page shows "CPU (GPU failed: <reason>)", and the CPU path
  stays until "Try the GPU again"; `--assistant-device cpu` never lists devices; live on this
  host (night or weekend): Vulkan on the RX 6650 XT reaches `ready` with GPU budgets, and a
  forced GPU failure (the child killed mid-job) ends on the CPU path with CPU budgets;
- hardening (Linux): the state string per case (ABI with network rules, ABI without, Landlock
  unavailable); seccomp: the child cannot create a UDP or raw socket (testable here); with
  Landlock network rules: the child cannot connect over TCP to a host listener, a LAN address or
  a public address and cannot bind another port, while Mimik reaches it on its port; the
  filesystem rules leave the GPU and CPU paths working. This host's kernel has no Landlock, so
  the Landlock cases need a kernel that has it (the CI runner if its kernel and container
  profile allow it, or a VM); until they have run, the Landlock state is reported as
  unverified. macOS and Windows hardening and lifecycle: not verifiable here, not claimed;
- breaker (injectable clock): three transport failures open it, two do not; 100 dropped or
  nonsense answers never open it; supersession and cancellation never count; one half-open
  probe while others are refused; probe success closes, failure reopens; a 2xx with all
  options dropped resets the count and closes a half-open breaker; a cancelled probe clears the
  probe flag and leaves it half-open; a queue-wait expiry does not count; mixed sequences (failure, success, failure) do not open it; while open, number
  entry, notes and Undo work and the poll says the assistant is unavailable;
- client: a host name, `localhost`, `127.1`, `0x7f.1`, `2130706433`, `::ffff:127.0.0.1`,
  userinfo, a zone ID or an `https` URL refused; `HTTP_PROXY` and `ALL_PROXY` set and a 302 from the fake server
  never reach anything but the endpoint; one request per call on a 500 or 429; a gzip-encoded
  reply is not decoded; a body over the bound is refused, plain and streaming, without
  buffering it; the schema arrives byte-exact in `response_format`;
- apply: a referenced field changes while the target does not → refused; a referenced row
  disappears → refused; effect mismatch → refused; content-hash mismatch → refused; a second
  apply, including two concurrent applies (exactly one writes, consumed only after commit) →
  refused; another sheet's or account's job or an invalid option index → refused; an unrelated
  field change → still applied; an expired job → no write and no literal; Undo of an applied
  proposal; apply after rights are revoked or the sheet is frozen → 403 and no write;
- routes (connected router): missing or wrong CSRF, extra or duplicate keys, body limit,
  malformed `note`; apply accepts exactly `csrf`; session or rights revoked between lookup and
  answer; apply checks CSRF and session before its D-X3 wait, answers the styled busy 503 and
  succeeds on retry; poll of another account's or sheet's job → not found; "ask again" on poll
  and on apply;
- browser (JS, 1280 and 390): unresolved save → "Thinking…" → option with current value,
  expression and effect → Use → value and `confirmed` note → Undo; an `ambiguous` reply shows
  readings without Use; supersession while polling; an escaped explanation;
- assistant off: no client, no child process, no device listing, no job route (404), no
  "Thinking…"; the assistant page says "off"; no player text in logs or in
  4xx bodies other than the player's own echoed draft;
- one live smoke, reported as live inference.

Evaluation set: hand-written and independently labelled cases in `tools/assist_eval.py`
(§3), kept local and out of source packages; it is a regression set, not the EVAL-1 held-out
set, and does not close D-D.

Open for review in 2c (**proposals**, not applied; each changes the literal-provenance rule
frozen for 2b, so the adopted form is re-checked by replaying the recorded 2b replies through
it and reporting the changed counts):
- **Derived constants.** Literal provenance checks literals only, so constants built from
  references pass: `hp ÷ ((hp + hp) ÷ hp)` halves without any admitted 2 (2b, fv-02).
  Proposal: every subtree that contains a `ref` is also evaluated with every referenced value
  v replaced by v + 1,009 and by 2v + 1,013; a subtree with the same result in all three
  evaluations is a derived constant, and its magnitude must pass literal provenance like a
  `lit`; an evaluation error in a perturbed run drops the option. It catches
  `(hp + hp) ÷ hp` and `(5 × hp + 5 × hp) ÷ hp` and keeps `hp − hp ÷ 2` with "half"; a
  coincidence (floor division of small values) can drop a legitimate option, erring towards no
  option. Narrower alternative: only subtrees whose leaves are all `ref` are checked, so a
  ref-only constant is rejected unless the phrase has a fraction word admitting it. Tests for
  either: the fv-02 tree dropped without a fraction word and kept with one (if the value is
  admitted); `hp − hp ÷ 2` kept; `hp − (hp − hp)` dropped when the phrase has no 0.
- **Fraction words "halve" and "dimezza".** Add EN `halve`, `halves`, `halved`, `halving`
  and IT `dimezza`, `dimezzare`, `dimezzo`, `dimezzi`, `dimezzato`, `dimezzata`, `dimezzati`,
  `dimezzate`, `dimezzalo`, `dimezzala`, `dimezzali`, `dimezzale`, matched as whole words like
  the others. Open with it: whether these admit the whole set (2, 3, 4, 100) as the existing
  words do, or each word only its own number (half and halve 2, third 3, quarter 4, percent
  100); and whether "double", "twice", "doppio" and "raddoppia" should admit 2. Tests: "halve
  it" and "dimezzala" admit 2; "halftime" does not.

### 2d Owner model setting

Scope: §1.4b (the model field on `/admin/assistant`, validation, background download, licence
confirmation, trial, switch and switch back, the GPU suggestion; migration 022). Depends on 2c.
Tests (fake Hugging Face server on loopback with a test CA unless stated):
- grammar: the default and suggested specs accepted; `@main`, a short or upper-case revision,
  no `@`, `..` or `.` segments, `%2e`, a backslash, white space, a non-ASCII character, a
  split file, a non-`.gguf` file, 513 bytes refused with the rule named; `local:../x.gguf`,
  `local:/etc/x.gguf`, `local:a/b.gguf`, `local:a\b.gguf`, `local:.x.gguf`, `local:CON.gguf`
  and `local:nul.gguf` refused; a `local:` symbolic link, a directory or a FIFO in the models
  folder refused; the field keeps a refused input;
- download: the metadata commit differing from the revision → refused; 401 or 403 → "download
  it by hand"; no download before the licence is confirmed, and the confirmation is recorded;
  a licence absent from the card shows "not stated"; size over 8 GiB or too little free space
  → refused before any byte; progress; Cancel keeps the partial and Resume sends `Range` and
  completes; a 200 to a `Range` request restarts from zero; a body longer than the expected
  size is cut off and refused; a hash mismatch deletes the file and nothing becomes usable; a
  restart leaves a partial waiting (no request until Resume); the stored name is the hash,
  whatever the spec says; one download at a time; the active model serves jobs throughout;
- download network: a redirect to a host off the allowlist, to `http`, or a third hop → stopped
  naming the host; a non-`huggingface.co` API host is impossible to express; `HTTP_PROXY` and
  `HTTPS_PROXY` set → not used; requests carry no cookie, credential or `Referer`; with a
  sheet holding a marker phrase, no download request contains it; a certificate not from the
  root store → refused;
- trial: a hash mismatch before load → refused; a model whose replies are schema-invalid or
  truncated at 384 → fails, and the previous model restarts; a model too slow for the caps →
  fails with the numbers; a pass shows device, priming and calibration times, speeds, B, R, D,
  Q and `max_tokens` by the 2b rule; jobs answer 503 "trying a model" during it; Switch and
  Switch back each restart and re-prime; a model file changed on disk after its trial →
  unavailable "the model file changed";
- suggestion: shown on the GPU path, not on the CPU path; filling the field starts nothing;
- rights and routes: a non-owner → 403 on every action; missing or wrong CSRF; extra or
  duplicate keys; body limits;
- live (reported separately, licence-checked downloads per the project rules): the default
  Granite spec downloads and verifies against `e0406663…`; a trial of Qwen3-4B on the GPU path
  at night or at a weekend.

### 3 Bundles and reminders

Scope: §1.5 (catalogue member with `key`, linking rules 1–2, a demo catalogue, bundles,
reminder jobs and hints). Tests: a bundle of 4 in one revision, a 5th change dropped, duplicate
targets dropped; one member changed → apply refused; contextual freshness (row name,
remembered key, entry version or context enum changed with every number unchanged → refused);
bundle Undo all-or-nothing; a reminder only on a decrease of a top-level integer, none with the
assistant off, not taking the account cap; duplicate names without a key → no entry text in
the prompt; text and multiline never in the prompt (asserted on the stub request); entry text
cut to 2,000; injection in entry text → at most a fully shown option; provenance extension: a
number in the cited entry's text → allowed and shown "from <entry>: …"; a number injected
into the entry text of a different (uncited) entry → option dropped; a number in no source
(neither the phrase, the target's value, a fraction or percent word nor the cited entry text)
→ option dropped.

### 4 Suggestions and "Which Sword?"

Scope: §1.6. Tests: confinement (forged address, unlinked catalogue, other game, native,
unsupported or corrupt pin); the type filter on every path; 2-character prefix; typo distance
1 and 2; diacritics; query, typo-pass and cache-byte bounds; no `JOBS` 503; 429; escaping;
choosing among duplicates stores the key and the link persists; renaming clears the key; a
dangling key shows no link; p95 measured.

### 5 Docs, release scope, gate

- Update `docs/assisted-entry.md`, `architecture.md` (the model runtime is no longer absent),
  `release-scope.md` (schema 22, the sheet module under its strict CSP, the local model as
  confirm-only, the bundled llama.cpp build and its licence, the protection states, model
  downloads over HTTPS, Linux x86_64 as the only verified platform), the runtime definition contract (the
  `catalogue` member), publication and dependency docs; replace the statements this work
  reverses: catalogue Q4 (not in the catalogue is a warning, "Which Sword?" remembered), the
  D9/R9 writer membership check (not built; the key is advisory),
  "no model proposes", the accuracy bar (waived, D-D), and "the LLM does not judge at play
  time" (narrow confirmable proposals and reminders).
- `cargo about`, audit, packaging (the runtime files verified against the manifest), and the
  unpacked smoke with the assistant off and once with the stub endpoint.
- Assistant scope stated as "local endpoint, POC quality, not certified (D-D)", with L1 and the
  other open limitations listed.

## 3. Verification

- Rust gate: fmt, strict Clippy, locked tests (red then green), audit, `cargo about`, release
  build and size delta.
- Browser (Chromium at 1280 × 720 and 390 × 844, demo data, with and without JS): per-cell
  Enter, buttons for selects and checkboxes, row actions, quick-add, notes, switch, Undo,
  unknowns, the two-tab sequence, derived and check refresh, CSP headers and no violations,
  combobox keyboard and IME, no overflow.
- Model runtime (live, reported separately from unit and fake-server evidence): on this host
  the CPU path at 2 threads and the GPU path (Vulkan, RX 6650 XT, night or weekend), each from
  a native child, not a container: start to `ready`, priming and calibration times, derived
  budgets, token refusal, the GPU → CPU fallback, shutdown and orphan cleanup, the seccomp
  state; Landlock on a kernel that has it (not this host). macOS (Metal) and Windows (Vulkan):
  not verifiable here and not claimed; they need a host of each before release scope names
  them.
- Model screen (prototype tier, `tools/assist_eval.py` with unit tests; results note with
  exact scope, denominators and limitations; reviewed; no pass bar, D-D): per path (CPU at 2
  threads, GPU), p50/p95 latency, schema-valid rate, output tokens; per class, correct within the first 3 and
  false-confident. Fast-path inputs never reach the model; results are per class, not general
  accuracy. `tools/assist_eval.py` is new in 2b; its cases are synthetic and independently
  labelled. Extra classes for 2c: valid divisions, overflow, unknown target, provenance
  failure, multi-option, related-field selection at 1 KiB; injection rows expect an option at
  most, never a write. Quality numbers are per backend (CPU and GPU answers differ at
  temperature 0, `docs/results.md`).

  The 2b screen used the six interpretation classes (50 cases); the reminder class (8) belongs
  to step 3, so the table totals 58.

  | Class | n |
  |---|---|
  | Damage and heal phrases (EN/IT) | 16 |
  | Fractions and vague | 8 |
  | Other-field references | 6 |
  | Nonsense | 8 |
  | Injection in the player phrase (a legitimate request, shown as an option) | 4 |
  | Injection hidden in context | 8 |
  | Reminder relevance (step 3) | 8 |

- Unit, connected-router, browser and live-inference evidence are reported separately.

## 4. Risks and later

Risks: the CPU path is slow (Granite: a 1 KiB worst case about 84 s with the margin, and
`starting` takes priming of about 117 s plus calibration of about 67 s plus hashing the model
before the assistant is ready); model quality is unknown and the bar is waived (the display
and confirmation are the safeguard); reminder prompts carry catalogue text (contained, not
prevented); one revision per Enter makes history long (no-ops write nothing, no cap; D-U);
`rig-core` is pre-1.0 with frequent breaking releases (pinned); HTTP client defaults
(proxies, redirects, retries) could send player text off the machine or repeat it, hence the
client policy and its tests. Runtime: many Linux kernels (this host's included) lack Landlock
or its network rules, so the child often runs not network-confined, and Landlock never covers
UDP (hence seccomp); macOS `sandbox-exec` is deprecated and may go; Windows has no
confinement without administrator rights; a malicious GGUF exploiting llama-server is
contained only where the hardening is active; the official Linux builds link against a
distribution's glibc and Vulkan loader, which may not match the host; a local process can race
for the port (the socket-owner check covers Linux only until macOS and Windows are verified);
other applications can take the GPU's memory mid-use (the fallback moves to the CPU); budgets
are measured once per start, so throttling later shows up as deadline expiries; the 2b GPU run
had the fan forced to full, while production uses the driver's fan curve; one GPU (RX 6650
XT) is measured, other GPUs and Metal are not; downloads add TLS and a second network client
to the binary; Hugging Face file hosts can change, which stops downloads until the allowlist
is updated.

Later (interpretation, deferred by the 2026-10-20 plan review): escalation to a second
local model (needs a second pinned endpoint, a thread budget totalling 2, preloading and a
single job deadline); model tools such as `read_field` and `search_catalogue` (need
host-bound capabilities, the Context field policy, results in the bound read set, and an A/B
against plain context); confidence gating (needs a defined signal and calibration on a
held-out set of at least 100); opt-in interpretation traces with offline replay (need
owner-only access, byte caps, retention, purge and deletion on every path; replay means
re-validating a recorded answer, while re-running a model is scored against independently
labelled answers; a confirmed proposal is feedback awaiting adjudication, never automatic
gold).

Later: sealing and verifying notes (before release, L1); a per-account write-rate limit or
storage guard (before release, D-U); player `*` and `/`; text, enum and boolean
interpretation; notes and Undo for text; Undo of moves and creation, forced and arbitrary
restore; owner-reviewed reminder rules without a model (D-I); a model for quick-add; more
threads off-hours; the quality bar; macOS (Metal) and Windows (Vulkan) runtime, hardening and
lifecycle verified on a host of each, and Windows confinement without administrator rights
(AppContainer); measurements on other GPUs; gated Hugging Face repositories (owner token) and
split GGUF files; partial GPU offload; retiring the legacy full-form edit POST; definitions
choosing a fallback for unknown (L-U4).
