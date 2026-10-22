# Assisted sheet plan: remaining steps

Direction (user, 2026-10-19): "Help the user as much as we can, but don't force anything;
improve paper and pencil, don't replace it." The player writes what they want; the app
computes, warns, suggests and offers Undo. Model output is only ever a confirmable proposal;
legality stays advisory and Rust does all arithmetic.

Decision IDs (D-…, U…, P…) are defined in `docs/decisions.md`. What is built (number entry,
notes and scribbles, switch and Undo, unknown values, per-field save, the busy page, the page
bound, quick-add, the validity flag, the sheet module with save on leaving a field) is described
in `docs/assisted-entry.md`; this plan only adds to it.

Remaining, in order (2b measured and reviewed, `docs/results.md`; ASSIST-27 to ASSIST-40
decided): the Rust 1.99.1 and `rig-core` 0.45.0 upgrade (ASSIST-39, its own change), **2c-1**
interpretation against a hand-started local server, **2c-2** the model runtime (process,
token, confinement, budgets), **2d-1** the owner model setting with local models, **2d-2**
model downloads, **3** bundles and reminders, **4** suggestions and "Which Sword?", **5** docs
and gate; each sub-step has its own branch, tests and reviews. The model runs GPU-first with
the CPU as the intended baseline (ASSIST-27; whether the default model meets the CPU caps is
open, §5), as an official `llama-server` build that Mimik
starts as a child and confines with bubblewrap where it can, never a container in production
(ASSIST-29, ASSIST-30, ASSIST-34); budgets come from a measurement at each start within fixed
caps (ASSIST-31, ASSIST-32). Linux x86_64 is the platform built and verified now.

Every step: its own branch; failing-first tests with the red run recorded; independent
implementation review before merge; every new dependency recorded with its licence, MSRV and
maintenance in the dependency decisions (for example `landlock`, `seccompiler`, `procfs`,
`rustls`, `rustls-platform-verifier`, and `hyper` if the Unix connector needs it); built and tested on 2 cores on weekdays; GPU and live
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

### 1.4 Interpretation service (2c-1)

POC boundary: one local model, one bounded request per job, current unresolved integer
scribbles only, strict validation, origin binding, poll and apply, the confirmable UI.
Deferred to §4 Later: model escalation, model
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
  The instructions and schema are a fixed prefix, byte-identical across requests (tested), and
  every request sets `cache_prompt: true` and `stream: true` as in 2b, so llama-server reuses
  the cached prefix and notices a closed connection between tokens. The dynamic part is at most
  the bound B set at start (§1.4a: at most 4 KiB on the GPU path, 1 KiB on the CPU path),
  measured in bytes after escaping, and at most N(B) tokens, counted with llama-server's
  `/tokenize` on the rendered dynamic part; whole context items are dropped from the end until
  both hold, never cut. The phrase, the target's ID and its value are never dropped; the
  target's label is dropped before them; if they still exceed a bound the job ends "too long
  for the assistant on this machine" (no completion request, neutral for the breaker). Every dynamic string goes inside a fixed data envelope that the instructions
  declare to be data, never instructions.
- **Context (D-F, D-J):** phrase and target first; then other stored integer and enum fields in
  definition order; derived, text, multiline, notes and list cells are excluded from context
  and from references (`ref`) (the one exception: same-row cells under the related-fields rule
  below); catalogue entry text comes in step 3 and is dropped first.
- **Related fields (ASSIST-31, ASSIST-37), for interpretation jobs:** the formula layer gives
  each derived value and each check a read set (below). (1) On every path, a row-cell target
  (list property `p` of the row with host ID `h`) also sends the integer and enum cells of row
  `h` only whose properties a read set reads together with `p` in the same iteration; cells of
  other rows, and text, multiline or derived properties, are never sent. Such a cell is a
  context item addressed by its typed address `cell:<list>/<h>/<property>` (the row's stable
  host ID and the property ID, as the switch and Undo keys and the orphan rules address cells,
  ASSIST-7, ASSIST-12); in the request it carries a short request-local alias like every
  context item (below), and Rust maps the alias back to the row by host ID, binds it in the
  read set (address, host ID, value), and drops the option if the row or the cell is not the
  one supplied. (2) Whenever B ≤ 1,024 bytes (always on the CPU path; on
  the GPU path only if the measurement lowers B to 1,024), the scalar context is only the
  stored integer and enum fields that some read set contains together with the target (for a
  row-cell target, together with `p`); fields the phrase names or that share the target's
  group are not added. With B above 1,024 the scalar context is the D-F/D-J list above.
  Related items go in definition order (scalars, then the row's cells in property order),
  count towards B, and are dropped from the end as above; pins without a formula layer send
  no related items. Candidates come only from the snapshot the job is bound to, which the
  account may read; relatedness never widens what is read. Reminder jobs (step 3) have their
  own context (§1.5).
- **Read sets (a 2c-1 deliverable):** today `Formula` keeps its reads, item reads and
  dependencies private and flat (`src/definitions/formula/mod.rs`, `profile.rs`), with no
  iteration identity. 2c-1 adds a public accessor on `Compiled` giving, per derived value and
  per check, its scalar reads and its `(list, property)` reads grouped by comprehension binder,
  with the transitive closure through derived dependencies. It is a structure separate from
  `reads` and `item_reads`, which keep driving `reads_unknown` and the U2 unknown guard, so
  U2, the formula profile bytes and the definition hash are untouched (D-Y); unlike
  `item_reads` it counts `has()` presence tests. A binder is identified by a per-comprehension
  ID, not by its variable name (sibling comprehensions both bind `item`), and the closure
  merges read sets without merging binder IDs: a derived value iterating with one binder and
  a dependent iterating with another stay two iterations. "The same iteration" means the
  same binder ID; two separate iterations over one list are not co-reads (tested).
- **Output contract:** JSON, checked by llama-server's schema grammar and again in Rust
  (`jsonschema`, then typed Serde decoding, then domain checks), with explicit limits: at most
  3 options, each an expression and an explanation of at most 120 characters, or an
  `ambiguous` reply listing at most 3 readings. A reading is display-only text of at most 120
  characters, rendered escaped inside `<bdi>`, with no Use button; an `ambiguous` reply never
  writes. The expression is a JSON tree, not a string: a node is `{"lit": n}`, `{"ref":
  "<alias>"}` (an alias from the supplied list, below) or `{"op": "add" | "sub" | "mul" | "div", "l": node, "r": node}`, and a
  `div` node also carries `"round": "floor" | "ceil" | "exact"` (`exact` drops the option on a
  remainder); depth at most 3 below the root, so at most 8 leaves. A negative `lit` is allowed;
  its magnitude is what provenance checks. Every context item carries a short request-local
  alias (`f1`, `f2`… for fields, `c1`, `c2`… for same-row cells, plus `t` for the target),
  which Rust maps to its typed address (`field:<id>` or `cell:<list>/<host>/<property>`); a
  `ref` takes only an alias from the supplied list (output schema v2), so references stay
  short. References name supplied integer items only: an enum field or cell is context for
  reading, and a `ref` to it, or to an alias not supplied, drops the option.
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
  assistant's reading". The fraction words and the literal-provenance rule stay as frozen for
  2b (ASSIST-38); constants derived from references, such as `hp ÷ ((hp + hp) ÷ hp)`, pass it
  (ASSIST-L22). Schema v2 changes the request and the replies, so the 2b `max_tokens`
  evidence is re-checked in 2c-1 before 192 is relied on: the 2b screen is re-run under v2 on
  the dev host (CPU, 2 cores, the 50 cases) and the reply lengths compared. `max_tokens` belongs to the model file: 192 for the Granite 4.2 3B file whose SHA-256
  is the compiled-in `e0406663…` (the 2b rule over the 104 replies of the 2b screen,
  `docs/results.md`); any other file, whatever its name, gets the 2d trial's rule (§1.4b),
  never below 256.
- **Confidence:** if the runtime returns token log-probabilities they are recorded on the job
  record for evaluation only; nothing is gated or hidden on them in 2c.
- **Crate (D-H, ASSIST-39):** `rig-core` =0.45.0 on Rust 1.99.1; the toolchain and crate
  upgrade is its own reviewed change, lands before 2c-1 and re-runs 2b's rig-core checks (C1
  to C5). Default features off plus `reqwest`, on the OpenAI chat route with the llama.cpp
  dialect; the schema goes in `additional_params` as `response_format` (sent byte-exact; the
  native `output_schema` path rewrites `oneOf` to `anyOf`), with `cache_prompt`, the sampler
  and `stream` beside it. The body bound is a small Mimik transport implementing rig's
  `HttpClientExt` (plain and streaming bodies refused past 1 MiB, as probed in 2b); no D-H
  crate offers one. The same transport carries the Unix-socket connection of the bubblewrap path
  (§1.4a): over `reqwest`'s Unix-socket connector if the upgraded `reqwest` has one with the
  client policy below (checked in the upgrade change), otherwise over `hyper`'s HTTP/1
  client connection on a `tokio` `UnixStream`; either way one transport, tested against a
  fake server on a Unix socket and on TCP. `genai` was rejected (no body hook). The
  per-launch token (§1.4a) is rig's API key, sent as `Authorization: Bearer`. The model client
  uses no TLS; from 2d-2 the binary contains TLS for downloads (§1.4b), and Cargo unifies
  `reqwest` features, so the model client's plain-HTTP property rests on the guard below, not
  on the dependency tree.
- **STROT** (judged on results, user 2026-10-19): the tested configuration (Julia-1 through
  strot-ai choosing among 16 to 71 generated arithmetic-string candidates plus "none",
  threshold 0.40, one selection) is rejected (2 of 50 correct, 42 false-confident); a design
  with a few semantic labels was not tested.
- **Client policy:** one client built by the host for the current child only: no redirects,
  proxies disabled whatever the environment says (`no_proxy()` on the client, so `HTTP_PROXY`,
  `HTTPS_PROXY`, `ALL_PROXY`, `NO_PROXY` and their lowercase forms are all ignored), plain HTTP only
  (an `https` URL is refused), no decompression (`no_gzip`, since the feature can be unified
  in), no retries. On the bubblewrap path the destination is the current generation's socket
  path (§1.4a) and nothing else; on the TCP paths, before every request the URL host must be
  an IP literal equal to `127.0.0.1` (the runtime binds only that, and the endpoint check reads
  only `/proc/net/tcp`; `::1` is accepted only by the 2c-1 development endpoint, which is
  dev-only and has no ownership check),
  with no userinfo or zone ID (`localhost`, `127.1` and `::ffff:127.0.0.1` refused), checked on
  the raw authority string as well as the parsed URL (the WHATWG parser turns `127.1` into
  `127.0.0.1`), and the port is the current child's. Every connection is checked to reach the
  current child before its first byte (§1.4a, "Endpoint"); the token goes only there. The model name is configuration, never model
  output. Loopback is only the destination rule; the no-egress property comes from the OS
  hardening of the child (§1.4a), at the level the page states.
- **Runtime:** a `llama-server` child managed by Mimik (§1.4a, 2c-2); in 2c-1 a server started
  by hand on the dev host, reached through the development endpoint (2c-1), which exists only
  in builds with the `dev-endpoint` Cargo feature (never in release builds) and has no
  endpoint check. The 2b container topology (internal Docker network with
  `inhibit_ipv4` and a socat forwarder) is development and benchmark evidence only
  (`docs/results.md`) and is not shipped. Model: configuration in 2c, the owner setting from
  2d (§1.4b); default Granite 4.2 3B (ASSIST-28).
- **Jobs:** one worker; one monotonic budget per job from enqueue, covering queueing,
  rendering, the model call and validation: queue wait Q plus run deadline R, with R, D, Q, B
  and N(B) derived at each start (§1.4a). D is the number of jobs that may wait (the running
  job is not counted). A start is admitted only if its worst-case wait fits Q: the running
  job's remaining run deadline plus R for each job already waiting; otherwise it answers 429
  "the assistant is busy, try again shortly", which cancels nothing (ASSIST-32). One job per account (counting running work): a start on another sheet while the
  account has a live job is 429 (origin revisions are per sheet and cannot be compared);
  admission (breaker and capacity) is computed as if the superseded job were already gone, so a
  refused start cancels nothing (Routes); an admitted start cancels the superseded request (the
  connection is closed and its slot freed before the new job is enqueued), and a late answer
  from a cancelled job is discarded. At most 64 job records in all, 128-bit IDs; eviction
  takes the oldest finished record and never a running or queued job (with 64 live jobs a start
  answers 429 busy); an evicted or expired job answers "ask again". When the child changes
  generation (restart, GPU → CPU fallback, a trial), every queued and running job ends with "the
  assistant restarted: ask again", neutral for the breaker. Options expire 10 minutes after the job finishes. Poll and apply
  recheck rights; the `JOBS` permit is not held during the model call. A reminder job (step 3)
  takes its own low-priority slot on the same worker, after interpretations, one per account.
- **Circuit breaker:** one per endpoint configuration (the managed runtime, or the
  development endpoint), never keyed by the per-generation socket path or port, in memory
  (reset on app restart). It counts
  only transport errors and invalid replies (HTTP 4xx and 5xx, a response over the body bound,
  a stream that is not a well-formed completion) while the child is alive. A child exit is a
  runtime event, never a breaker event: it ends the generation's jobs neutrally and feeds the
  restart and failure counter of §1.4a, whichever order the exit and the job's transport error
  arrive in: a counted transport failure is provisional while its generation is alive and is
  withdrawn if that generation's exit arrives within 2 s; a breaker opened only by withdrawn
  counts closes again. A completed,
  bounded HTTP 2xx exchange is a success whatever its content (an answer whose options are all
  dropped still resets the count and closes a half-open breaker); supersession, cancellation,
  queue-wait expiry (the model was never called) and run-deadline expiry are neutral: deadline
  expiries drive only the restart-and-re-measure rule of §1.4a. Three consecutive counted failures open it for 2
  minutes; then one half-open probe job decides (success closes, failure reopens; a cancelled
  probe clears the in-flight probe flag, leaves it half-open, and the next job becomes the
  probe; while the probe is in flight other starts answer 503 like an open breaker). While open
  no request is sent: a start answers 503 and a job already queued answers on poll, both with
  "the assistant is unavailable right now"; number
  entry, notes and Undo work as usual. A small state machine with an injectable clock:
  `failsafe` 1.3.0 as tested in 2b allows multiple outstanding half-open admissions and its
  future API records a non-counted error as a success; its manual API could express neutral
  outcomes but has no probe flag, its mock clock is thread-local and its last release was
  2024-07. Priming, calibration and the 2d trial are outside the breaker. Precedence: a start
  or poll first reports the runtime state (`starting`, `unavailable` with its reason, §1.4a),
  then the open breaker; the assistant page shows both. A new child does not reset the
  breaker; only the owner's Restart and the app's start do.
- **Routes:** `POST /private-sheets/{id}/interpretations` (exactly `csrf` and `note`) starts a
  job for that note. The note key has the form switch and Undo use (`<revision>:<address>`,
  the indexed `<revision>:<f>.<r>.<p>` for row cells), but today an `unresolved` note has
  none: the page carries keys only for switch offers and Undo, and the JSON answer has
  `"actions": null` (`Actions::of`, `sheet_json.rs`). 2c-1 adds `key` to the JSON answer's
  `note` object whenever the note is `unresolved`, additively (existing ASSIST-22 clients
  ignore it), built from the stored note with `assist::key` for scalars and row cells alike
  (row cells in the typed host-ID form `cell:<list>/<host>/<property>`, never the indexed
  form, which would mix the note's revision with the answer's row positions), so it carries
  the note's own revision and stable address, and with no page markup (the assistant needs
  JavaScript, D-G); the module starts the job with it. A key is resolved against the snapshot at its own revision: a typed key
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
  a write newer than the origin gives 409. A start registers its origin revision in the map's
  sheet entry before its database check; an entry for a target is dropped only when its
  revision is below the smallest origin revision registered or live on that sheet, and with its
  sheet or row; the map and the job table are
  per process and empty after a restart, when the database check alone suffices. 202 with the job
  ID. `GET
  …/interpretations/{job}` polls (the module polls at 500 ms, backing off to 2 s, and stops on
  unload, supersession or a terminal state); an evicted or expired job answers "ask again".
  Another account's or sheet's job is not found, on poll as on apply.
  Apply: `POST /private-sheets/{id}/proposals/{job}/{n}` (exactly `csrf`), taking the `JOBS`
  permit with the D-X3 bounded wait like the other sheet writes.
- **Flow (JS):** a cell save records `unresolved` and the module starts a job; "Thinking…" in
  `role="status"`; then the options or the readings of an `ambiguous` reply; **Use** posts the
  apply route; ignoring them leaves the scribble. The apply route must not rely on how long an
  offer stays on screen or treat a shown offer as confirmation.
- **Proposal freshness (D-P):** when a job finishes, Rust validates each option and stores it
  with a content hash and its bound read/write set: the origin binding above, every write
  target (address, value at the job's base revision, host ID for row cells), every reference
  (`ref`: address and value), the displayed effect (before → after per target), and for
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

### 1.4a Model runtime, device and budgets (2c-2; ASSIST-27, ASSIST-29 to ASSIST-34, ASSIST-40)

Linux x86_64 is the only platform built and verified in 2c-2; macOS (Metal) and Windows
(Vulkan) runtime code is deferred with no platform designed out (ASSIST-34, §4 Later).
Production never needs a container (ASSIST-29); on the dev host the pinned official build runs
natively for these tests (ASSIST-40).

- **Pinned build (ASSIST-33):** a manifest, the data file `schemas/llama-runtime-v1.json`
  embedded into Mimik with `include_str!` (a test asserts the embedded copy equals the file),
  also read by `llama-fetch` and by packaging, so CI and the binary verify the same set, pins one llama.cpp build (b11610
  as in 2b unless the 2c-2 review picks a newer one) for Linux x86_64: the official GitHub
  release assets for the Vulkan and the CPU build, each asset's SHA-256, and the path and
  SHA-256 of every file Mimik runs from them. Before 2c-2 depends on them, the manifest entry
  records the exact asset names, the system libraries each binary needs (from its ELF
  dependencies: glibc version floor, `libvulkan.so.1`, others), how the CPU backend is chosen
  for the host's instruction set, and the documented minimum host; the release scope names
  that host. The runtime directory must hold exactly the manifest's files (an extra or missing
  file refuses the start), and neither it nor its files may be writable by group or others.
  On levels 2 and 3 the shared libraries beside llama-server are loaded by path from that
  verified directory, inside the same-user boundary below.
- **Model and runtime files (verified descriptors):** the model is opened relative to the
  models folder's descriptor with `openat2` (`RESOLVE_BENEATH`, `RESOLVE_NO_SYMLINKS`,
  `RESOLVE_NO_MAGICLINKS`) and `O_NOFOLLOW | O_NONBLOCK` (a FIFO cannot block), then `fstat`:
  a regular file owned by Mimik's user, not writable by group or others. Its SHA-256 is
  computed from that descriptor before the first load and again whenever its `fstat` identity
  (device, inode, size, modification and change times) differs from the last hashed one, so a
  retry does not re-read a 2 GiB file; that descriptor stays open and is what the child loads: level 1 binds it read-only (`--ro-bind-fd`); levels 2 and 3 pass it inherited, as
  `--model /proc/self/fd/<n>` (a live check confirms that the pinned server's open and `mmap`
  accept that path). Runtime files are hashed the same way; level 1 binds them by descriptor
  and levels 2 and 3 execute the verified llama-server descriptor (`execveat`). So the model and
  the executable cannot be swapped between check and load on any level; on levels 2 and 3 the
  shared libraries are still loaded by path, so the runtime directory and every ancestor
  directory must not be writable by group or others (checked at start), and the library
  guarantee there is the same-user boundary below. Threat boundary: a process running as
  Mimik's user could still rewrite an inode in place; that is outside the boundary (it could
  rewrite Mimik itself). Windows reparse points and path rules come with the Windows runtime.
- **Flag vector:** the 2b vector (`tools/model_registry.py` `server_args` plus the screen's
  extra flags), with only the transport and device changed (ASSIST-30): `--model <verified
  descriptor path>`, `--alias <label>`, `--host <transport>` (the socket path or
  `127.0.0.1`), `--port <p>` (TCP levels only), `--offline --no-webui --no-slots --no-mmproj
  --jinja`, on the GPU path `--n-gpu-layers 99 --device <name>` (the selected device, below),
  on the CPU path `--n-gpu-layers 0`, `--fit off`, `--threads 2 --threads-batch 2`
  (configurable, 2 as measured), `--ctx-size 8192 --parallel 1 --no-warmup --cache-ram 0
  --reasoning off`, and the token (below). Measurement-relevant (a change needs a new
  measurement before it ships): `--jinja`, `--n-gpu-layers`, `--fit`, threads, `--ctx-size`,
  `--parallel`, `--cache-ram`, `--reasoning`, plus the per-request `cache_prompt: true`,
  `stream: true` and the greedy sampler (temperature 0, top-k 1, seed 0). `--parallel 1`: jobs
  run one at a time (ASSIST-32). The pinned b11610 `--help` (checked on the dev host,
  2026-10-22) lists `--api-key-file`, `--api-key` with the environment variable
  `LLAMA_API_KEY`, `--ignore-eos`, `--slots`/`--no-slots`, `-dev`/`--device`,
  `--list-devices`, and `--host` accepting Unix socket paths. The runtime check (CI, below)
  runs the pinned binary's `--version` and `--help` for every flag; behaviour that help text
  cannot show is checked live against the pinned binary, and a mismatch stops 2c-2 for a
  decision: serving and the token over the Unix socket, `/health` without a token, the
  per-request `ignore_eos`, the `/proc/self/fd` model path, the device list format and the
  offload line.
- **Packaging and CI (ASSIST-33, REL-2):** `package_release.py` gains an allowlist entry
  `llama/` holding exactly the manifest's files (anything else refuses the package), the
  llama.cpp build number in `BUILD.txt`, and `THIRD-PARTY-NOTICES.md` entries for llama.cpp
  and ggml (MIT) and for every other file shipped from the asset, by its own licence. REL-2
  requires corresponding source for every shipped binary, so the release also carries the
  pinned llama.cpp source: always a `git archive --format=tar` of the pinned commit from a
  shallow clone at that commit (GitHub's generated tag tarballs are not byte-stable), its tar
  SHA-256 recorded in the manifest and compressed afterwards with a pinned `gzip -n` for
  shipping (the tar hash is what is verified), staged at the release root, never inside
  `llama/`, listed in `SOURCE.txt` and `SHA256SUMS`. The `dev-endpoint` feature is absent from the release
  binary because the staged binary comes from `cargo build --release --locked` without
  features (tests build with `--all-features`); `smoke_release.py` in `artifact-smoke` checks
  the staged binary: it contains no `assistant-dev-endpoint` string, and the option is
  refused as unknown.
- **Test model (CI only, never shipped):** Hugging Face `ggml-org/models-moved` at revision
  `499bc8821c6b12b4e53c5bffcb21ec206f212d81`, file `tinyllamas/stories260K.gguf`, 1,185,376
  bytes, LFS SHA-256 `270cba1bd5109f42d03350f60406024560464db173c0e387d91f0426d3bd256d`. That
  repository states no licence; its upstream, `karpathy/tinyllamas` at revision
  `0bd21da7698eaf29a0d7de3992de8a46ef624add`, is MIT. Both are recorded in the workflow and the
  manifest's test section. Only `llama-fetch` downloads it.
- **CI workflow (2c-2 deliverables, `.github/workflows/ci.yml`):**
  - a new required job `llama-fetch` (in the `mimik-ci` container like `checks`, with
    `timeout-minutes: 15`): fetches the
    pinned assets, builds the source archive with `git archive` of the pinned commit, fetches
    the test model, verifies every hash, unpacks the assets (with `unzip`, installed in the
    image, under `target/`), and uploads the verified `llama/` files, the source archive and
    the test model as artifacts;
  - `checks` gains `needs: llama-fetch`, downloads that artifact before packaging, packages
    only it (since `upload-artifact` drops file modes, every job that unpacks re-hashes each
    file against the manifest and reapplies the manifest's modes), and uploads the
    `linux-release` tarball with its `SHA256SUMS` (as today) and the confinement test binary,
    built with the workflow's own flags (`cargo test --locked --all-features --no-run
    --message-format=json`) and located from that output (built in the Ubuntu 22.04 image, so
    it also runs on 24.04);
  - a new required job `llama-runtime` with `needs: [llama-fetch, checks]` (so there is no
    cycle: `checks` never waits for `llama-runtime`), on a runner pinned by label
    (`ubuntu-24.04`), with `timeout-minutes: 30`, as host steps, not inside the `mimik-ci`
    container (Docker's default seccomp profile refuses the namespace flags `bwrap
    --unshare-all` needs). It uses no Rust toolchain. It verifies the `linux-release` tarball
    against `SHA256SUMS`, unpacks it under `target/`, reapplies the manifest's modes, and runs
    everything from that unpacked release. It installs `bubblewrap` from the 24.04 package and
    records its version; if that package lacks any option the probe needs, the job builds a
    pinned upstream bubblewrap release (tarball SHA-256 and the meson and ninja build
    dependencies pinned in the workflow) and installs it to `/usr/bin` with `sudo`, so the
    fixed-path rule holds and no release-capable override exists. It installs `libvulkan1`
    with Mesa's lavapipe; if `kernel.apparmor_restrict_unprivileged_userns` exists it logs its
    value and lifts it with `sudo sysctl` for the job's duration only (an absent key is logged
    and skipped). It then runs:
    - the flag check;
    - the start-time probe for the CPU build only, as `target/rel/mimik assistant-runner
      --probe --runtime target/rel/llama --models-dir target/models --model
      local:stories260K.gguf --model-sha256 270cba1b…`, which prints one JSON line (level
      reached, per-resource results, process shape) and exits 0 only at level 1; anything
      lower fails the job (CI covers the CPU build only; the GPU path is a dev-host
      measurement, and release builds never accept software renderers);
    - the real CPU-build llama-server under the level-1 profile with the test model, started
      by the confinement test binary through the runner with the same arguments: `/health`
      without a token, token refusal and acceptance over the Unix socket in
      `target/data/run/<generation>/`, the model bound from the verified descriptor
      (`--ro-bind-fd`), one completion, the peer-shape assertion, and cancellation
      mid-generation. The test model's `n_ctx_train` is recorded with its pin, and the CI run
      uses a `--ctx-size` and a short prompt that fit it (the fixed prefix is not sent); its
      first run is itself a live check of the model's behaviour under `--jinja` and that
      context;
    - the confinement test binary (seccomp, endpoint and adversarial-helper tests) against
      the unpacked release's runtime; the adversarial helper is that same binary re-executed
      with a helper flag; the binary reads nothing from the source tree (fixtures embedded);
      it is run as `target/confinement-tests --exact` with the confinement test filter. If the
      runner's kernel offers Landlock ABI 4 or later (probed and logged), the level-2 tests
      run there too; otherwise they stay the VM measurement;
  - `deployable` requires `llama-fetch`, `checks` and `llama-runtime` with the existing jobs;
    every new action is pinned by commit SHA like the existing ones;
  - stub-transport integration coverage runs in a test build with the `dev-endpoint` feature,
    labelled as such and never called verification of the shipped binary;
  - not CI-covered, and said so: the GPU path (probe, device listing, live GPU checks),
    mid-prompt cancellation timing and the
    per-request `ignore_eos` with the real default model are dev-host measurements, repeated at
    every pin change and recorded in `docs/results.md`; Landlock-only tests that need a kernel
    the runner lacks are a host or VM measurement, recorded in the release scope and in
    `docs/limitations.md` (ASSIST-L24).
- **Runner:** Mimik spawns `mimik assistant-runner` (a hidden subcommand) from one dedicated,
  long-lived thread (the parent-death signal follows the spawning thread). The runner sets
  `PR_SET_PDEATHSIG` (SIGKILL), then checks that its parent is still the Mimik PID it was given
  and exits otherwise (the race before the signal is set); sets `RLIMIT_CORE` to 0 and
  `RLIMIT_FSIZE` to 64 MiB (a write past it raises `SIGXFSZ`, left at its default action, so
  the server is killed and the exit is handled as a child exit); clears its environment except a fixed minimal set (`PATH`,
  `LANG`, `HOME` pointing at an empty private directory, the token variable when that channel
  is used); keeps only the descriptors it was handed (Rust opens everything close-on-exec; the
  hand-over descriptors are listed by number); then executes the confinement level's command
  in place (`execve` of bwrap on level 1, `execveat` of the verified llama-server descriptor on
  levels 2 and 3; either way the parent-death signal stays set). On levels 2 and 3 the runner sets
  `PR_SET_PDEATHSIG` and `PR_SET_NO_NEW_PRIVS` before it installs the seccomp filter, and the
  filter refuses any later `prctl(PR_SET_PDEATHSIG)` and `prctl(PR_SET_NO_NEW_PRIVS)` call,
  so a compromised server cannot clear the signal; level 1 keeps bubblewrap's own setup
  sequence (its `--die-with-parent` and the PID namespace contain the tree). The preflight refuses a `bwrap`
  that is set-user-ID or set-group-ID (that exec would clear the signal, and it is not the
  unprivileged setup this design assumes).
- **Confinement levels (ASSIST-34), tried in order; the active level and the reason for each
  skipped level are shown on the assistant page and logged:**
  1. **bubblewrap**: `bwrap` from a fixed absolute path (`/usr/bin/bwrap`, `/bin/bwrap`; never
     `PATH`), unprivileged (user namespaces). Arguments: `--unshare-all` (user, network, PID,
     IPC, UTS, cgroup; no `--share-net`), `--die-with-parent`, `--new-session`, `--clearenv`
     plus the minimal environment, `--cap-drop ALL`; `--ro-bind` of the system library
     directories, `/etc/ld.so.cache`, the Vulkan loader and driver files (`/usr/share/vulkan`,
     `/etc/vulkan` where present) and the read-only parts of `/sys` the driver enumerates; the
     runtime files and the model bound read-only from their verified descriptors; `--dev /dev`
     (a new minimal `/dev`), on the GPU path plus `--dev-bind` of the selected device's render
     node only (read-write, as Mesa needs); private `--tmpfs /dev/shm` and `--tmpfs /tmp`
     (read-write, each with `--size`, 256 MiB and 64 MiB, when the probe finds the option;
     otherwise bounded only by `RLIMIT_FSIZE` per file, shown on the page); `--proc /proc`; the
     socket directory bound read-write; no bind of the host `/`; the token through
     `--ro-bind-data <fd>`; the seccomp program through `--seccomp <fd>`; `--json-status-fd
     <fd>`, which reports `child-pid` (bwrap's PID-1 reaper inside the new PID namespace) and
     `pid-namespace`, and later the `exit-code`. The exact path list is fixed by the 2c-2 tests
     on both paths (CPU and GPU reach `ready`) and listed in the reference doc; nothing outside
     it is visible. The child has only its own loopback interface, so it listens on a Unix
     socket at `/run/llama/llama.sock` inside the sandbox, a bind of a per-generation
     directory in Mimik's own data directory (`<data dir>/run/<generation>/`), created with
     `mkdir` mode 0700 and opened with `O_NOFOLLOW | O_DIRECTORY`, then `fstat` must show a
     directory owned by Mimik's user with mode 0700, or the start is refused; removed when the
     generation stops. Only the socket is expected there, but a compromised server can write
     files into it, each bounded by `RLIMIT_FSIZE` but not in number, so exhaustion would hit
     Mimik's data disk, not the user's session (ASSIST-L26). Mimik connects through `/proc/self/fd/<dir fd>/llama.sock`, so the
     host path's length never meets the 108-byte `sun_path` limit. A pathname socket is found
     by the inode its path resolves to, not by the `sun_path` string it was bound with, so the
     host-side path through the directory descriptor reaches the socket the child bound as
     `/run/llama/llama.sock`; the `llama-runtime` job performs that real connect against the
     pinned binary. Abstract Unix sockets are
     per network namespace and pathname sockets are limited to what is bound in, so the child
     reaches no host service. OpenFirma's bubblewrap setup is the reference; no code is copied
     and `firma-run` is not a dependency.
  2. **Landlock with seccomp** (TCP on `127.0.0.1`, random port): the runner applies Landlock
     through the `landlock` crate, then the seccomp filter, then executes the verified
     llama-server descriptor. There is no bind mount here, so the rules name host paths (a
     `/proc/self/fd/<n>` open is checked against the file it resolves to). The ruleset:
     - network (ABI 4 or later): no TCP connect; TCP bind on the chosen port only;
     - read: the models folder (the model's resolved file lies in it), the system library
       directories, `/etc/ld.so.cache`, the Vulkan ICD manifests (`/usr/share/vulkan`,
       `/etc/vulkan`), the read-only `/sys` parts the driver enumerates,
       `/sys/devices/system/cpu`, `/proc/self`, `/proc/cpuinfo`, `/proc/meminfo`;
     - read and execute: the runtime directory (llama-server `dlopen`s its ggml backend
       libraries from its own directory, found through `/proc/self/exe`), the library
       directories, the Vulkan driver libraries and the ELF interpreter;
     - read and write: `/dev/null`, `/dev/zero` and `/dev/urandom` (read for the last), the
       selected render node (GPU path, plus `IOCTL_DEV` on it from ABI 5) and the card node
       only if the driver needs it, `/dev/shm`, and a private temporary directory;
     - scoping (ABI 6 or later): abstract Unix sockets and signals;
     - nothing else.

     Level 2 is verified only by the VM measurement of ASSIST-L24 until a host with Landlock
     exists; the list is completed there.

     The shown
     state comes from the crate's `RestrictionStatus`, never from the requested rules.
     Landlock binds ports, not addresses, and does not filter `connect()` to pathname Unix
     sockets, so the honest state is "partly confined: outbound TCP, UDP and raw sockets
     denied; it could listen on another address on its port and reach local services over Unix
     sockets"; with a Landlock ABI below 4 (no network rules): "not network-confined: TCP egress
     allowed; filesystem restricted"; partial filesystem enforcement is shown as such; with no
     Landlock at all (`RestrictionStatus` none) the level is not reached, and level 3 is used
     with its own wording.
  3. **Unconfined** (seccomp only, TCP on `127.0.0.1`): "not confined: TCP egress allowed, and
     the whole filesystem readable by Mimik's user is readable by the model server".
  The assistant runs at every level (hardening is "where available", ASSIST-30); the page
  shows the level next to the assistant state and next to the 2d "Try" button (a downloaded
  GGUF is untrusted input to llama-server, contained only by this).
- **Seccomp (every level):** pure Rust only, adding no runtime shared-library dependency to
  the shipped binary: `seccompiler` (pure Rust, it does not link `libseccomp`) or hand-written
  BPF; `libseccomp` is not an option. A 2c-2 fit check confirms `seccompiler` can express an
  architecture check as the first rule and a raw x32 range refusal (`nr & 0x40000000`), which
  a default-allow filter needs so x32 calls cannot bypass the `clone` rule; otherwise those
  first instructions are hand-written BPF. Default action allow with the listed refusals returning `EPERM`
  (`ENOSYS` for `clone3`); the filter first checks the architecture (`AUDIT_ARCH_X86_64`,
  anything else killed), then refuses every x32 syscall number; `AF_NETLINK`, `AF_PACKET` and every family not named below are
  refused. `socket()` is allowed only for
  `AF_INET` or `AF_INET6` with `SOCK_STREAM` and protocol 0 or `IPPROTO_TCP`, and for
  `AF_UNIX` with `SOCK_STREAM` (the listening socket; flags `SOCK_CLOEXEC` and
  `SOCK_NONBLOCK` masked); `socketpair()` is refused unless the pinned server needs it (then
  `AF_UNIX` only), fixed by test; `fork`, `vfork` and `clone` without `CLONE_THREAD` are
  refused and `clone3` returns `ENOSYS` (on x86_64 glibc builds `fork` and `vfork` on `clone`
  or `clone3`, so the `clone` flag rule and the `clone3` refusal are what operate; `clone3`'s
  flags cannot be inspected; glibc then falls back to `clone` for threads), so the server cannot start other processes: process-tree
  containment on levels 2 and 3, where the parent-death signal covers only the direct child
  (an `exec` without a fork only replaces the server in place). Exec after start is restricted
  too, because a non-leader thread that calls `exec` becomes the process leader without the
  parent-death setting (it is per task): `execve` is refused, and `execveat` is allowed only on
  the verified runtime descriptor's number with `AT_EMPTY_PATH`, that descriptor being
  `O_CLOEXEC` so it is gone after the first exec; `dup2`, `dup3` and `fcntl(F_DUPFD*)`
  targeting that number are refused if needed. If 2c-2's live check shows this cannot be made
  sound, lifetime containment on levels 2 and 3 is best effort (the pidfile sweep at the next
  start) and the page says so (ASSIST-L27). `pidfd_open`, `pidfd_getfd`, `process_madvise`
  and `kcmp` are refused (they would let the server reach Mimik's descriptors). TCP Fast Open
  is refused on every level: `sendto` whose flags carry `MSG_FASTOPEN`, `sendmsg` and
  `sendmmsg` whose flags carry it, and `setsockopt(TCP_FASTOPEN_CONNECT)` return `EPERM`, since
  Landlock's connect rule is defined on `connect()` and a fast-open send on an unconnected
  TCP socket must not be a way around it; `io_uring_setup`,
  `io_uring_enter`, `io_uring_register`, `ptrace`, `process_vm_readv`/`writev`, `bpf`,
  `perf_event_open`, `keyctl`, `add_key`, `request_key`, `mount`, `umount2`, `pivot_root`,
  `unshare`, `setns`, module and `kexec` calls are refused. Seccomp sees no `connect()`
  address, so it is never described as network confinement. Refused-call discovery on the
  dev host uses `strace` in a container on the GPU path (no dependency on the kernel log or
  root); calls the Vulkan driver needs are decided before 2c-2 completes.
  `SECCOMP_FILTER_FLAG_LOG` is optional.
- **Preflight** (every start, before level 1): bwrap present at a fixed path, not set-ID; its
  support for every option used above is probed (no remembered minimum version: bubblewrap
  0.13.0 on the dev host has `--unshare-all`, `--clearenv`, `--bind-fd`/`--ro-bind-fd`,
  `--bind-data`/`--ro-bind-data`, `--perms`, `--seccomp` and `--die-with-parent`, checked
  2026-10-22); every option the profile uses is probed, including `--json-status-fd`,
  `--dev-bind`, `--tmpfs` with `--size`, `--proc`, `--new-session` and `--cap-drop`; then a
  real probe run, `mimik assistant-runner --probe`: the Mimik binary re-executed under the
  full level profile (the same binds, descriptors, seccomp program and token channel as a real
  start, plus Mimik's own libraries bound) exercising each resource once: open the model
  through its descriptor path (including `--ro-bind-fd` of a regular-file descriptor), bind
  the socket path in its directory (level 1) or the port (levels 2 and 3), read the token
  channel, and on the GPU path open the render node and enumerate the device; on level 1 it
  also asserts the process shape of the endpoint check (below). It does not prove
  llama-server's own system calls; the real server start does (CI with the test model, the dev
  host with the real model). It runs once for the CPU build and once for the GPU build (a
  level may work for one and not the other; each path uses the highest level its own build
  passed). A failure names its cause where it can be told: bwrap
  missing, set-ID or lacking an option (named), user namespaces disabled
  (`user.max_user_namespaces`, `kernel.unprivileged_userns_clone`), the Ubuntu AppArmor
  restriction on unprivileged user namespaces (`kernel.apparmor_restrict_unprivileged_userns`),
  or the probe's own error. Then level 2 is probed the same way, then level 3. A level that
  passes the probe can still fail at the real start; that failure is recorded with its cause
  and shown on the page, and the transitions below move down only for sandbox-attributable
  failures. Memory: the child has no memory cap (`RLIMIT_AS` is
  unusable with GPU drivers' address-space mappings, ASSIST-L23); the preflight refuses a model
  file larger than `MemAvailable` minus 1 GiB ("not enough memory for this model").
- **Token:** 32 bytes from the OS random source, base64url, new for every launch, held in
  memory. Channels, in order: level 1, `--ro-bind-data` from a pipe to a file inside the
  sandbox read with `--api-key-file`; levels 2 and 3, `--api-key-file` on a 0600 file in a
  0700 private directory, deleted once `/health` answers (`LLAMA_API_KEY` in the runner's
  environment, readable only by Mimik's user, is the documented alternative), never in an
  argument. A pinned build without either channel would not start the assistant ("the model
  server cannot receive its token safely"); b11610 has both. It never runs without a token
  and never passes it on a command line. `/health` without a token is a live-checked property
  of the pinned build; every other request carries it.
- **Endpoint check, per connection, before its first byte:** level 1: `SO_PEERCRED` on the
  connected Unix socket (the kernel reports the peer PID in Mimik's PID namespace) must give
  Mimik's UID and a PID that is either the `child-pid` bwrap reported for this generation or
  a process whose `PPid` (from `/proc/<pid>/status`) is that `child-pid`: with `--unshare-all`
  bwrap 0.13.0 on this host reports its PID-1 reaper as `child-pid` and runs the command as
  its child, while a bwrap whose sandbox init executes the command in place gives the other
  shape; the preflight asserts which shape the installed bwrap has. In both shapes the
  namespace test must pass: the `st_ino` of `/proc/<pid>/ns/pid` equals the `pid-namespace`
  number bwrap reported, cross-checked by `NSpid` in `/proc/<pid>/status` (two entries, the
  inner one small); where `ns/pid` is not readable (it needs ptrace-read access, which a
  non-dumpable process denies), the `NSpid` and `PPid` tests decide. Mimik holds a pidfd on
  `child-pid` so its PID cannot be reused during the generation. A UID-only match is never
  accepted. Levels 2 and 3: before and after `connect()`, every `LISTEN` entry for
  `127.0.0.1:<port>` in `/proc/net/tcp` (`procfs` crate) must have its socket inode open in
  the current child's `/proc/<pid>/fd` (with `SO_REUSEPORT` several listeners can share a
  port, so one foreign entry fails the attempt). While the child holds that listener no other process
  can listen on the port, so the connection reaches it; the accepted socket is not required
  (it enters the child's table only on `accept()`). No `LISTEN` entry yet means "not ready"
  (retried until the `/health` timeout); an entry owned by any other process fails the attempt
  at once. The port is picked by binding
  `127.0.0.1:0` and releasing it; a child that fails to bind it exits, and a new port is tried
  (at most 3 per attempt). The child's "listening" line is a hint for when to connect, never
  the proof.
- **Output:** stdout and stderr are each drained by a reader task with a 4 KiB line bound
  (longer lines truncated) and are never copied into Mimik's log. Only fixed patterns become
  events: listening, the build number, "offloaded N/M layers", model-load and device errors.
  The child runs at the pinned build's default verbosity; a live test sends a job with a
  marker phrase and checks the marker appears in neither stream.
- **Generations:** every launch has a generation number; the socket directory, the pidfile
  entry, the endpoint check, jobs and every exit or timeout event carry it, and an event from an
  older generation is ignored. One manager task serializes transitions: a child is stopped
  (SIGTERM, SIGKILL after 5 s) and the next one starts only after it is gone: on level 1 when
  bwrap's status descriptor reports `exit-code` and a pidfd on the reaper signals exit (the PID
  namespace, and with it every process and its GPU memory, is gone); on levels 2 and 3 when a
  pidfd on the child signals exit. So two model servers never coexist (a GPU → CPU fallback and
  a trial included). On every generation change, every queued and running job ends with "the
  assistant restarted: ask again", neutral for the breaker. The pidfile (generation, PID,
  process start time, binary path) is held under an exclusive `flock`, so a second Mimik on
  the same data directory cannot run the assistant; at start a leftover is killed only when
  PID, start time and path all match. On shutdown Mimik stops the child as above.
- **Device (ASSIST-27, HW-1):** software ICDs (`llvmpipe`, `lavapipe`, `SwiftShader`, by
  description and driver) are excluded first, before any count of devices. Enumeration names
  (`Vulkan0`, `Vulkan1`) change with the set of visible devices, so the name passed to
  `--device` always comes from the listing in the same confinement the server runs in. Level 1:
  at each start the GPU build's `--list-devices` (30 s timeout; output format pinned by a test
  against the pinned build) runs inside the level-1 profile once per render node with only that
  node bound; after excluding software ICDs each list shows exactly one hardware GPU, and that
  in-sandbox name is what the real start passes. Whether binding one node really isolates
  enumeration is a 2c-2 live measurement: Mesa enumerates through sysfs and libdrm, so if the
  ICD still lists other cards, the level-1 profile also binds only the matching
  `/sys/class/drm` entries, or selects by PCI address inside the sandbox. Levels 2 and 3 cannot isolate nodes (level 3
  does not restrict device opens at all): the GPU is used there only if exactly one hardware
  GPU is visible, counted from sysfs (`/sys/class/drm/renderD*` with their PCI devices), not
  from `--list-devices`, otherwise the CPU path is used with a page note (multi-GPU without
  bubblewrap is a limitation, ASSIST-L25). The node's PCI address (sysfs) is its stable
  identity across restarts and distinguishes two identical cards. Ranking (`auto`, level 1):
  a device counts as dedicated when sysfs shows `mem_info_vram_total` above 0 (amdgpu) or its
  PCI class and `boot_vga` mark a discrete card where available; dedicated devices rank before
  the others, then by reported memory, ties by PCI address; where sysfs says nothing, no
  ranking claim is made beyond memory. `--assistant-device cpu` forces the CPU and a PCI
  address selects that device. The real start checks that the sandboxed list still shows one
  hardware GPU with the recorded description. The GPU path loads every layer on the GPU: the
  child's offload line must show N = M, otherwise it is a GPU failure. A Vulkan build that
  cannot start (for example no Vulkan loader) is a GPU failure. The CPU path uses the CPU
  build.
- **Transitions** (attempt = spawn, endpoint check, `/health` within 60 s, priming within
  300 s, calibration within twice the path's run-deadline cap; owner actions are owner-only
  and CSRF-checked):

  | Event | Action | Failure count |
  |---|---|---|
  | Attempt fails on the GPU path (budgets that do not derive included), or the GPU child exits, or is stopped as hung | stop and wait until gone, start on the CPU path at the same level at once, showing the reason; CPU until Mimik restarts or the owner presses "Try the GPU again" | not counted |
  | Attempt on the CPU path fails with a sandbox-attributable failure at level 1 or 2 (a bwrap or Landlock setup error, a status-descriptor failure before exec, the level's preflight failing) | next attempt at once on the next level that passed its probe; the skipped level and its reason are recorded and shown | not counted |
  | Attempt fails on the CPU path, otherwise (model-load errors, hash mismatches, exits after exec keep the level) | retry after 2 minutes | +1 |
  | Child exits while ready (CPU path) | restart now | +1 only within 10 minutes of `ready` |
  | Two consecutive run-deadline expiries with no completed exchange between | stop as hung, wait until gone, restart, re-measure (whatever `/health` says); expiries never count toward the breaker | as a child exit |
  | Failure count reaches 4 (the first attempt plus 3 retries) | `unavailable` until Mimik restarts or the owner presses "Restart" | — |
  | Budgets do not derive on the CPU path ("too slow") | `unavailable` with the numbers; no retry; "Restart" measures again | — |
  | `ready` for 10 minutes | failure count back to 0 | reset |
  | Owner presses "Restart" (or Mimik starts) | stop, wait until gone, new attempt from level 1, the only way back up from a lower level; breaker reset | reset |

  While `starting` or `unavailable`, a job start answers 503 with the state, queues nothing,
  and number entry, notes and Undo work as usual. Logs carry states, levels and reasons, never
  prompt text, URLs with tokens or the token.
- **Cancellation:** a superseded or expired job's streamed request is dropped, which closes
  its connection; the pinned server is expected to stop at the next token, and during prompt
  evaluation at its next check of the connection. Live tests measure both: a superseding job's
  first token after cancelling a job mid-generation and mid-prompt (a 1 KiB prompt on the CPU
  path). If the server finishes prompt evaluation before noticing, the superseding job waits
  that long inside its own budget; only two consecutive deadline expiries stop the child.
- **Priming and calibration:** after `/health`, priming as in 2b (the fixed prefix with a
  minimal data envelope, `max_tokens` 1, which fills the prompt cache: about 117 s for Granite
  at 2 CPU threads in a container, about 1.3 s on the RX 6650 XT). Then two calibration
  samples: fixed synthetic jobs shipped with Mimik (no sheet or player text), each with a
  dynamic part of exactly the path's byte cap (CPU 1,024, GPU 4,096) built from filler whose
  token density matches real sheets (labels and values of the shipped demo and 2b synthetic
  sheets; the density is recorded), since every request is separately held to N(B) tokens;
  the two fillers differ from their first byte so neither reuses the other's cache;
  `max_tokens` the model's, `ignore_eos: true` and no schema, so each sample generates exactly
  `max_tokens` tokens. From each sample: wall time W, `timings.prompt_n`, `prompt_ms` and the
  cached prompt tokens (the evaluated prompt tokens include any uncached tail of the fixed
  prefix and the envelope and template tokens around the filler), `predicted_n` and
  `predicted_ms`; and from `/tokenize`, each filler's token count N₁ and N₂ (equal byte
  lengths do not give equal token counts). All must be present,
  finite and positive, `predicted_n` must equal `max_tokens`, each tail tᵢ = `prompt_n` − Nᵢ
  must not be negative, and the cached tokens must be at least 75 % of the fixed prefix's
  tokens measured by priming (2b saw 83 to 88 %), or the attempt fails (a timing criterion,
  not a content one). Both are outside the breaker. The grammar question is settled once, in
  2c-2, not at each start: a live measurement compares per-token generation time with and
  without the grammar on both paths; if the grammar costs more than 10 %, the shipped
  calibration measures g from schema-carrying samples (then `predicted_n` need only be at
  least 32 and generation time is g × `max_tokens`), otherwise it stays as above.
- **Budgets (ASSIST-31, ASSIST-32; recomputed at every start, a latency estimate within hard
  caps, not a worst-case bound):** per sample i: per-token prompt time pᵢ = `prompt_ms` /
  `prompt_n`, per-token generation time gᵢ = `predicted_ms` / `predicted_n`, overhead oᵢ =
  max(0, W − `prompt_ms` − `predicted_ms`), tail tᵢ. The shared request limit uses the
  smaller count, N_cal = min(N₁, N₂) (conservative: a request may not exceed the leaner
  filler's density). For a candidate bound B, N(B) = floor(N_cal × B / cap), Tᵢ(B) = oᵢ + pᵢ ×
  (tᵢ + N(B)) + gᵢ × `max_tokens` with the same N(B) in both, and T(B) = max(T₁(B), T₂(B)). Candidates: GPU 4,096, 2,048, 1,024; CPU 1,024, 768, 512. B is the
  largest candidate with R(B) = max(15, round up to 5 s of 1.25 × T(B)) within the path's
  limits: GPU R ≤ 60 s; CPU R ≤ 90 s. Queue: GPU D = max(1, min(4, floor((120 − R) / R))) and
  Q = D × R (the 2b rule; Q + R ≤ 120 holds since R ≤ 60); CPU D = 1 and Q = 120 − R, so Q +
  R ≤ 120 s, and the admission rule (§1.4, Jobs) refuses as busy a start that cannot begin in
  time. Every request also obeys N(B) tokens (§1.4), so its prompt is no longer than the
  calibration's; the hard run deadline holds when the estimate is wrong, and two consecutive
  expiries restart and re-measure (the transitions above). On the GPU path, no fitting
  candidate is a GPU failure (CPU fallback). On the CPU path, if 1,024 does not fit, 768 and
  then 512 are taken; only when 512 does not fit is the assistant unavailable ("too slow on
  this machine: about N s needed, the cap is M s"), with the measurement shown; entry is never
  affected: "too slow" on the CPU path turns the assistant off for that machine until a
  Restart measures again. Expected outcomes, with the prompt term written out as p × (t +
  N(B)): t is the uncached tail of the fixed prefix (2b saw 170 to 250 of its about 1,460
  tokens uncached; the 75 % cache rule lets t reach about 365), N(B) the dynamic part. On the
  GPU path the 2b numbers (about 1,100 prompt and 80 generated tokens per second on the
  RX 6650 XT) give T about 4 s, so B 4,096, R 15, D 4, Q 60. On the CPU path the outcome is
  set by the first native calibration on this host, a measurement step of 2c-2 recorded in
  `docs/results.md`. An illustration from the 2b container figures (p about 88 ms and g
  about 213 ms per token at 2 threads, o about 1 s, about 3.5 bytes per token, Granite's 192
  tokens about 41 s), not taken with this procedure or filler:

  | B | N(B) | T with t 170 to 250 | 1.25 × T | Fits 90 s? |
  |---|---|---|---|---|
  | 1,024 | about 296 | about 83 to 90 s | about 104 to 113 s | no |
  | 768 | about 222 | about 77 to 84 s | about 96 to 105 s | no |
  | 512 | about 148 | about 70 to 77 s | about 88 to 96 s | only at the low end (R 90, D 1, Q 30) |

  With t at the 75 % bound, or with `max_tokens` 256 or more (any model but the default
  Granite file), nothing fits, so the CPU baseline of ASSIST-27 is not assured with the
  default model at 2 threads (§5, open question). In 2c-2 these are formula fixtures, not
  evidence.

### 1.4b Owner model setting (2d-1, 2d-2; ASSIST-28, ASSIST-35, ASSIST-36)

- **Before 2d:** 2c reads the model from configuration: `--assistant-model` (the spec,
  default `local:granite-4.2-3b-Q4_K_M.gguf` with its SHA-256 `e0406663…` compiled in, which
  equals the Hub's LFS SHA-256 of that file at revision `c40945d…`), `--assistant-model-sha256`
  (required for any other `local:` file) and `--assistant-model-alias` (the `--alias` the
  runtime gives the server and the request's model name, default derived from the spec), so 2c does not depend on 2d. The file is placed in the models
  folder by hand. From 2d-2 the default is the `hf:` spec below; a file already present with
  that hash is used without a download.
- **Page:** `/admin/assistant` (owner only; built in 2c-2 with the state, path, confinement
  level, measurements, budgets, "Restart" and "Try the GPU again") gains one free-text field
  "Model". Models folder: `<data dir>/models/` (`--models-dir`), mode 0700, owned by Mimik's
  user.
- **Grammar** (at most 512 bytes, ASCII, no surrounding white space; anything else is refused
  naming the failed rule, and the field keeps the input):
  - `hf:<org>/<repo>/<path>@<revision>` (2d-2): `org` and `repo` each 1 to 96 of
    `[A-Za-z0-9._-]`, starting and ending alphanumeric, without `--` or `..`; `path` one or
    more segments of 1 to 128 `[A-Za-z0-9._-]` joined by `/`, no segment `.` or `..`, ending
    `.gguf` and not a split file (`-NNNNN-of-NNNNN.gguf`); `revision` exactly 40 lowercase hex
    digits (a commit, never a branch or tag). URLs are built with path-segment encoding,
    never by concatenation. In 2d-1 an `hf:` spec is refused ("downloads arrive later").
  - `local:<name>.gguf` (2d-1): one name of 1 to 128 `[A-Za-z0-9._-]` ending `.gguf`, not
    starting with `.`, not a Windows device name in any case (`CON`, `NUL`, `COM1`…).
- **Model file handling (both forms):** as in §1.4a ("Model and runtime files"), built in
  2c-2: the SHA-256 is recorded when the owner tries the model and checked before every load.
- **Download (`hf:`, 2d-2, owner-started only):** (1) metadata from the Hub API at that
  revision, JSON bounded at 1 MiB, admitted strictly: the returned commit equals the
  revision; the file is listed with LFS data; its SHA-256 is 64 lowercase hex and its size an
  integer from 1 byte to 8 GiB (ASSIST-35); anything missing or malformed refuses the
  download. 401 or 403 (gated or private) is refused with "download it by hand and use
  `local:`" (gated repositories later). (2) The page shows repository, file, size, SHA-256 and
  the licence (ID, a link to the licence file at that revision, and a note that a third-party
  quantisation's licence can differ from the upstream model's). A model without a stated
  licence is allowed with a warning (ASSIST-35). The owner confirms; the confirmation records
  owner, time, licence ID and the SHA-256 of the licence file at that revision (or "none");
  nothing downloads before it. The licence file is the one the model card names, or else a
  root `LICENSE`, `LICENSE.md` or `LICENSE.txt` listed in the metadata; it is fetched through
  the same client and host rules from the `resolve` URL at the pinned revision, bounded at
  1 MiB, and shown as text; "none" when the card names no file and none of those is listed. (3) Free space at least the remaining bytes plus 1 GiB.
  (4) One download task, at most one partial file at all (starting another download asks to
  discard the existing partial), in the background, never holding the `JOBS` permit:
  `models/.partial/<sha256>`; progress on the page (polled with JavaScript, a reload without);
  Cancel (keeps the partial), Discard (deletes it), Resume with `Range`: the reply must be 206
  with `Content-Range: bytes <offset>-<end>/<total>` where offset is the partial's length and
  total the expected size, or the partial restarts from zero once, then the download fails;
  the body is bounded by the expected size. Cancel, Discard, Resume and completion are
  serialized in the download task. Completion: the SHA-256 must match, then `fsync`, an atomic
  rename to `models/hf/<sha256>.gguf` (names never come from the spec) and an `fsync` of the
  directory; a mismatch deletes the file. Timeouts: connect 30 s, idle read 60 s, and a
  progress floor (at least 1 MiB per 5 minutes) after which the download stops and waits for
  Resume. After a restart a partial waits for Resume: nothing downloads without an owner
  action. The active model keeps serving throughout.
- **Download network policy (ASSIST-35):** a client separate from the model client: HTTPS
  only (rustls with the OS certificate store through `rustls-platform-verifier`); the system
  proxy is used for downloads only, never by the model client; no cookies, credentials or
  `Referer`; a fixed `User-Agent`; `Accept-Encoding: identity` on every request, so `Range`
  offsets and the size bound refer to the file's own bytes. API and `resolve` requests go to `huggingface.co` only;
  redirects are followed only from a `resolve` URL, at most 2 hops, each HTTPS to a host in an
  enumerated list of exact host names (no suffix or wildcard rule), checked on every hop. The
  list is fixed in 2d-2 from Hugging Face's documentation and the observed redirects of the
  default and suggested files at their pinned revisions; candidates to confirm:
  `cdn-lfs.huggingface.co`, `cdn-lfs-us-1.huggingface.co`, `cdn-lfs-eu-1.huggingface.co`,
  `cdn-lfs.hf.co`, `cdn-lfs-us-1.hf.co`, `cdn-lfs-eu-1.hf.co`, `cas-bridge.xethub.hf.co`,
  `transfer.xethub.hf.co`. Any other host stops the download and names the host. A request
  carries only what the owner's spec names, never sheet, note or player text; redirect URLs
  are presigned and are never logged (only their host). The base URL, host list and root store
  are constructor parameters of the download client so tests can use a fake server with a
  test CA; the shipped values are fixed in code with no configuration knob, and a test asserts
  the shipped constructor uses them. Until 2d-2 confirms the list, no download code ships
  (the list lands with 2d-2). Updating the list later is a code change with a checklist:
  observe the redirects of the default and suggested files at their pinned revisions, check
  Hugging Face's documentation, update the list and its tests, and note the change in the
  release notes.
- **Trial and switch (2d-1; ASSIST-36):** "Try" verifies the model's hash, stops the current
  child (players see "starting"), starts the candidate on the current device path and primes
  it, then (1) sends 5 fixed synthetic phrases shipped with Mimik with the schema at
  `max_tokens` 384 (the measurement cap): every reply must be schema-valid and untruncated;
  (2) sets the model's `max_tokens` = min(384, max(256, round up to 32 of 1.25 × the largest
  `predicted_n`)) (five phrases cannot measure a tail, hence the 256 floor; the default
  Granite file, identified by its compiled-in SHA-256, keeps 192 from its 104-reply screen); (3) runs the calibration of §1.4a with that `max_tokens` and
  derives the budgets. It passes when (1) and (3) succeed. Bounds: each phrase has twice the
  path's run-deadline cap (180 s on the CPU path, 120 s on the GPU path), the whole trial
  30 minutes on the CPU path and 20 on the GPU path (priming and calibration included; each
  covers the worst case of priming 300 s, five phrases at their deadline and two calibration
  samples at twice the cap); the
  owner can Cancel at any time. Expiry, Cancel or failure stop the candidate and restore the
  previous model automatically. Results are bound to a trial ID (the candidate's hash and the
  generation it ran in), and "Switch" accepts only the latest finished trial's ID for that
  hash. The page shows the device, priming
  and calibration times, speeds, `max_tokens`, B, R, D and Q, and per phrase whether the
  expected value was among the options (information only, D-D). "Switch" makes it active,
  recording its hash, `max_tokens` and the final calibration; "Keep the current model"
  restarts the previous one, as does a failed trial. "Switch back" restarts the previous
  active model in one click. A load whose hash no longer matches makes the assistant
  unavailable ("the model file changed") until the owner tries a model again. A non-active
  downloaded model can be deleted.
- **Default and suggestion (2d-2):** default
  `hf:ibm-granite/granite-4.2-3b-GGUF/granite-4.2-3b-Q4_K_M.gguf@c40945d71cd90f249a56985e8155551a9188dc30`;
  on the GPU path the page suggests
  `hf:bartowski/Qwen_Qwen3-4B-Instruct-2507-GGUF/Qwen_Qwen3-4B-Instruct-2507-Q4_K_M.gguf@ae44f08e1392f39c0e474af10c3ff8355c8b6688`
  (the 2b pins; it fits the GPU path on the RX 6650 XT, `docs/results.md`; other GPUs are
  unmeasured). A suggestion only fills the field.
- **Security:** owner only (403 for others, as the other admin pages), CSRF and body bounds on
  every action (Restart, "Try the GPU again", Try, Switch, Keep, Switch back, licence
  confirmation, Cancel, Discard, Resume, delete); one download and one trial at a time; the
  spec is configuration, never model output. Storage: migration 022 (models: spec, SHA-256,
  size, licence and its confirmation, trial measurements, `max_tokens`; the active and
  previous model).

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
  one posts the cell route `cells/{f}/{r}/{p}` (ASSIST-6; the row index read at `base`
  and mapped to its host ID) with the entry key: one revision, no note; the link is
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
The model client talks plain HTTP only to its own child: through the generation's Unix socket
(bubblewrap) or a loopback IP literal, every connection checked to reach the child before its
first byte (the peer in the child's PID namespace under the reaper; the port's listener in
the child), with a per-launch token that never appears on a command line; it follows no
redirects and ignores proxy settings. The one exception is the development endpoint of 2c-1,
which has no endpoint check and exists only in builds with the `dev-endpoint` feature, never
in a release. The model server is a pinned official llama.cpp build and a model file, both
hash-verified and loaded from the verified descriptors on every level. Its confinement is one
of these levels, shown on the assistant page: bubblewrap (private namespaces, so it has no
network beyond its own loopback and sees only an allowlist of read-only paths plus the GPU
node; seccomp); Landlock ABI 4 or later with seccomp (outbound TCP, UDP and raw sockets
denied, but listening on another address and local Unix sockets are not prevented); Landlock
below ABI 4 (filesystem restricted, TCP egress allowed); or unconfined (TCP egress allowed and
the user's whole filesystem readable). On every level seccomp refuses new processes and
non-TCP sockets; it is never described as network confinement. So Mimik sends player text
only to its child, but below the bubblewrap level a compromised model server (for example
through a malicious GGUF) could send it on. Model downloads are owner-started, HTTPS to an
enumerated list of Hugging Face hosts, bounded, licence-confirmed and hash-verified, and carry
no sheet or player text. No prompts, player text, presigned URLs or token in logs; no secrets.

## 2. Steps

### Upgrade (ASSIST-39)

Scope: Rust 1.99.1 in `rust-toolchain.toml`, `containers/ci.Dockerfile` and the `BUILD.txt`
line of `package_release.py`; `rig-core` =0.45.0 and the resulting `Cargo.lock`; Clippy and
fmt findings of the new toolchain fixed, not suppressed; 2b's rig-core checks C1 to C5 re-run
against 0.45.0 (schema pass-through, host-built client policy, body bound through the own
transport, cancellation, licences); and the answer to whether the upgraded `reqwest` has a
Unix-socket connector that keeps the client policy (§1.4, Crate). Its own branch and reviews;
2c-1 starts on it.

### 2c-1 Interpretation

Scope: trait, stub, adapter; the client policy with both connectors and the token; the
request builder with the byte and token bounds and the related-fields context; the runtime
event interface the worker consumes (`generation started`, `generation ended` with its
reason, `ready` with the budgets B, N(B), R, D, Q), fed in 2c-1 by the development endpoint's
fixed configuration and by test doubles, and in 2c-2 by the runtime manager; the formula
layer's read-set accessor (§1.4, Read sets); output schema v2, reviewed before use: `ref`
accepts only a request-local alias from the supplied list (`f1`, `c1`, `t`), the instructions
describe aliases and same-row cells, the 2b screen re-run under v2 on the dev host to re-check
`max_tokens` 192, the
Rust copy lives under `schemas/`, the fixed prefix is re-measured (tokens and priming time),
and a live check confirms the pinned build's grammar conversion accepts the alias enum; the note key in the JSON answer of an
`unresolved` note (§1.4, Routes); worker, admission, records and breaker; the job, poll and
apply routes with origin and read/write-set binding; the UI. The model server is started by
hand on the dev host (ASSIST-40) and reached through a development endpoint setting
(`--assistant-dev-endpoint`, a loopback literal or a socket path, with a token file; the model
is `--assistant-model` (the spec, §1.4b), `--assistant-model-sha256` and
`--assistant-model-alias` (the request's model name, default derived from the spec's file
name, `granite-4.2-3b-q4_k_m` for the default), and the hand-started server must be given the
same `--alias`), compiled
only with the `dev-endpoint` Cargo feature, which release builds never enable (`smoke_release.py`
checks the staged binary refuses the option); it has no endpoint check. Budgets come from
flags of the dev build (development placeholders, not evidence): `--assistant-dev-bytes`
(B, default 1,024), `--assistant-dev-token-limit` (N(B), default 384, above the about 296
tokens a 1 KiB part takes at 3.5 bytes per token),
`--assistant-dev-deadline` (R, default 85 s), `--assistant-dev-queue` (D, default 1) and
`--assistant-dev-queue-wait` (Q, default 35 s). In a 2c-1 release build, which has neither
the endpoint nor the runtime, `--assistant` is refused at start ("the assistant is not
available in this build"), so the assistant stays off and no assistant page exists. Until 2c-2 lands the assistant is a
development feature, outside the release scope, and the release scope's "no language model"
statement stays true because the release binary has no endpoint. Tests (stub or fake server unless
stated):
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
  dynamic part stays within B (4,096, 2,048, 1,024, 768, 512) and within N(B) tokens (the fake
  server's `/tokenize` returning a high count drops items that the byte bound kept), dropping
  whole items; the development token limit (`--assistant-dev-token-limit`, default 256)
  is the N(B) 2c-1 enforces; the target's label goes before the phrase, target ID or value; a phrase and
  target over a bound end "too long" without a completion request and without a breaker count;
  excluded field types never appear in the request;
- read sets: per derived value and check, scalar reads and `(list, property)` reads grouped by
  binder, closed through derived dependencies; `item.a` and `item.b` in one comprehension are
  co-read, while `a` in one comprehension and `b` in another over the same list are not, also
  when both bind `item` and when one of them is inside a derived value the other reads; a
  `has()` presence test counts as a co-read; `reads_unknown`, U2 and the formula profile bytes
  are unchanged by the accessor (asserted on existing fixtures);
- related fields (B ≤ 1,024): a field read with the target by a check, or through a derived
  value it reads, is sent; a field only in the same group, or named in the phrase but not
  co-read, is not; a row-cell target sends the scalars co-read with its property; definition
  order and drop from the end; a pin without a formula layer sends none; with B 2,048 the full
  D-F/D-J scalar list is sent; a `ref` to an unsent field or to an enum field drops the
  option;
- same-row cells (every path, B 4,096 and B 1,024 alike): for a target cell of row `h`, an
  integer or enum cell of row `h` co-read with the target's property is sent with an alias
  (`c1`) that Rust maps to `cell:<list>/<h>/<b>`, a text, multiline or derived property never
  is, a cell of another row with the same property never is, nor a same-row cell not co-read;
  a `ref` to that alias evaluates with its value; a `ref` to an alias not in the supplied list
  (or a typed address written out instead of an alias) drops the option; aliases are
  request-local (the same field may have another alias in the next request); the row moved since the job →
  found by host ID; the cell changed or the row removed before apply → apply refused;
- note key: a cell save that records `unresolved` answers with `note.key` in the JSON (the
  typed host-ID form for row cells too), the page markup is unchanged, a cell whose note
  predates the answer's revision yields a key with the note's revision that starts a job; a
  row moved before the JSON answer builds the key → the key still names the original host ID
  and the start finds that row; a
  start with that key is accepted, and a key of a parsed or sign-led note is refused (409);
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
  64 records and the evicted-job answer; option expiry at 10 minutes; busy admission (§1.4,
  Jobs): with R 85 and Q 35 a start while a job has more than 35 s left → 429 "busy", with less
  → queued; a second waiting job beyond D → 429; with 64 records of which some are finished,
  a new start evicts the oldest finished one and never a running or queued job; with 64 live
  jobs a start → 429; the note-write map: a start registered with an old origin keeps the
  entries it needs until it ends (a delayed start after many writes still gets 409);
- breaker (injectable clock): three transport failures open it, two do not; 100 dropped or
  nonsense answers never open it; supersession and cancellation never count; one half-open
  probe while others are refused; probe success closes, failure reopens; a 2xx with all
  options dropped resets the count and closes a half-open breaker; a cancelled probe clears the
  probe flag and leaves it half-open; queue-wait and run-deadline expiries never count (ten in
  a row leave it closed); a malformed completion stream counts; a child exit never counts,
  whether it arrives before or after the job's transport error: error first, then the exit
  within 2 s → the provisional count is withdrawn, and a breaker that this count opened closes
  again; error first and no exit → the count stays; a new child leaves the breaker as it was, and only Restart and app start
  reset it;
  mixed sequences (failure, success, failure) do not open it; while open, number
  entry, notes and Undo work and the poll says the assistant is unavailable; a start reports
  `starting` before an open breaker;
- client: a host name, `localhost`, `127.1`, `0x7f.1`, `2130706433`, `::ffff:127.0.0.1`,
  userinfo, a zone ID or an `https` URL refused; `::1` refused on the runtime paths; `HTTP_PROXY`,
  `HTTPS_PROXY`, `ALL_PROXY` and their lowercase forms set and a 302 from the fake server
  never reach anything but the endpoint; one request per call on a 500 or 429; a gzip-encoded
  reply is not decoded; a body over the bound is refused, plain and streaming, without
  buffering it; the schema arrives byte-exact in `response_format` with `cache_prompt: true`
  and `stream: true`; the same tests over the Unix-socket connector (a path other than the
  configured socket refused);
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
- assistant off: no client, no job route (404), no "Thinking…"; no player text in logs or in
  4xx bodies other than the player's own echoed draft; a release build has no
  `--assistant-dev-endpoint` option;
- live (hand-started pinned llama-server, ASSIST-40; reported as live inference): one smoke;
  the second request of a session reports the cached prefix tokens in `timings`; a job
  cancelled mid-generation frees the slot (the next job's first token within its usual time).


Evaluation set: hand-written and independently labelled cases in `tools/assist_eval.py`
(§3), kept local and out of source packages; it is a regression set, not the EVAL-1 held-out
set, and does not close D-D.


### 2c-2 Model runtime

Scope: §1.4a on Linux x86_64: manifest, packaging and the CI runtime check, runner,
confinement levels, preflight, token, endpoint check, output drain, generations and
transitions, device choice and fallback, priming, calibration and budgets; the owner's
assistant page (state, path, level, measurements, budgets, "Restart", "Try the GPU again").
The development endpoint of 2c-1 stays for tests only. Tests (fake runner binary or fake server
unless stated; live means the pinned official build on this host, reported separately; GPU at
night or at a weekend):
- manifest and packaging: an extra, missing or changed runtime file → no child, unavailable
  with the reason; a group-writable runtime file → refused; a development override
  (`--assistant-runtime <dir>`) with other files → refused; `package_release.py` stages
  exactly the manifest's files and refuses others, and after the artifact hand-off re-hashes
  them and reapplies the manifest's modes; the `llama-runtime` job fails closed when the asset
  cannot be fetched, its hash differs, or the probe reaches less than level 1; the shipped
  binary has no `--assistant-dev-endpoint` (asserted by the packaging check and
  `smoke_release.py`);
- flags (CI, pinned build): `--version` reports the pinned build; `--help` lists every flag of
  the vector; live: `/health` without a token, the per-request `ignore_eos` and the
  device-list and offload-line formats behave as §1.4a assumes, or the step stops for a
  decision;
- runner: the child's environment holds only the allowed names (`/proc/<pid>/environ`); no
  inherited descriptor beyond the listed ones (`/proc/<pid>/fd`); `RLIMIT_CORE` 0 and
  `RLIMIT_FSIZE` 64 MiB (a write past it kills the child with `SIGXFSZ`, handled as a child
  exit); a helper that calls `prctl(PR_SET_PDEATHSIG, 0)` on levels 2 and 3 is refused, and
  SIGKILL of Mimik still kills it; a set-ID `bwrap` → refused by the preflight;
  a runner
  whose parent died before the parent-death signal was set exits (parent PID mismatch); live:
  SIGKILL of Mimik during startup and while ready kills the whole tree;
- bubblewrap level (live on this host, where user namespaces work): the CPU and GPU paths reach
  `ready`; inside, the child cannot connect over TCP to a host listener, a LAN address or a
  public address, cannot send UDP, cannot reach a host abstract or pathname Unix socket (an
  adversarial helper binary in place of llama-server, run with the same profile), cannot see
  host paths outside the allowlist, cannot create a UDP, raw or `io_uring` instance, cannot
  fork or start another process; with the probed `--size` it cannot fill `/tmp` or
  `/dev/shm` beyond their sizes in many small files, and without it only the per-file limit
  holds and the page states "temporary space unbounded";
  Mimik reaches it only through the socket, connecting through the directory descriptor (a
  socket directory whose host path is longer than 108 bytes still works); the socket
  directory is removed when the generation stops; preflight causes: bwrap absent, user namespaces
  disabled, a probe failure (each reported by name and falling to the next level); the
  AppArmor restriction is reported by name on an Ubuntu VM;
- Landlock level: the state comes from `RestrictionStatus` (full, partial, none) and the
  page text matches the table in §1.4a; seccomp refusals as above (testable here); with
  Landlock network rules: no TCP connect to a host listener, LAN or public address, no bind
  on another port, the listening socket on `127.0.0.1` works. This host's kernel has no
  Landlock (`CONFIG_SECURITY_LANDLOCK` not set), so these run on a VM with Landlock ABI 4 or
  later; until they have run, the level is labelled unverified in the results and the release
  scope; the "could listen on another address on its port" gap is demonstrated by the
  adversarial helper and recorded; on this host: the helper cannot fork, `vfork` or `clone` a
  process (`clone3` → `ENOSYS`) while threads still start, so no descendant outlives a SIGKILL
  of the child;
- token: the fake server sees `Authorization: Bearer <token>` on priming, calibration and
  every job, and a new token after a restart; live: no token and a wrong token on
  `/v1/chat/completions` → 401, the right one is served, `/health` answers without it; the
  token is in no command line (`/proc/<pid>/cmdline`), no log line and no file after
  `/health` (TCP levels); a fake build without a safe channel → the assistant does not start;
- socket directory: a pre-created `<data dir>/run/<generation>/` that is a symbolic link, owned
  by another user or of another mode → the start is refused; files written into it by a helper
  are each bounded by `RLIMIT_FSIZE`;
- exec and descriptors (levels 2 and 3, adversarial helper): a thread that calls `execve` →
  refused; `execveat` on any descriptor but the runtime one, or on a duplicate of it → refused;
  thread → exec → SIGKILL of Mimik → nothing survives (or, if 2c-2 finds this unsound, the
  pidfile sweep removes it at the next start and the page says lifetime containment is best
  effort); a ptrace-accessible cooperative helper holding a forbidden descriptor → the child's
  `pidfd_open` and `pidfd_getfd` are refused, also with Yama scope 0; a `sendto`, `sendmsg`
  or `sendmmsg` with `MSG_FASTOPEN` to a host listener and `setsockopt(TCP_FASTOPEN_CONNECT)`
  → `EPERM` on every level, and the listener sees no connection;
- endpoint: level 1 (live): the peer of llama-server's socket has the reaper as parent and the
  reported `pid-namespace`, and is accepted; a peer with Mimik's UID that is not in that
  namespace (a helper listening on a socket swapped into the directory) → nothing sent; a
  UID-only match is never accepted; levels 2 and 3: a second `SO_REUSEPORT` listener on the
  child's port → nothing sent; a squatter listening on the port → its
  `LISTEN` inode is not the child's → nothing sent; a child that accepts late (after a delay)
  is still accepted; a child that exits between the check and the request → the connection
  fails, nothing sent to anyone else; a child that never prints its listening line but listens
  → reached; a port taken before the child binds → a new port, at most 3;
- model and runtime files, per level: a symbolic link, a link in a parent position, a
  directory, a FIFO (no block) and a file writable by others in the models folder → refused;
  a model swapped for another file after the hash and before the load → the hashed inode is
  loaded, at level 1 through `--ro-bind-fd` and at levels 2 and 3 through `/proc/self/fd`
  (live: the pinned server loads from that path);
- output: a child writing endless lines and a line of 1 MiB → bounded memory, nothing in
  Mimik's log; live: a job with a marker phrase → the marker in neither stream;
- generations and transitions (fake runner, injectable clock), one test per table row: GPU
  failure at each phase → CPU at once, not counted, the next child started only after the old
  one is gone (bwrap's `exit-code` and the reaper's pidfd); GPU budgets that do not derive →
  CPU path, the page saying why; a CPU attempt with a sandbox-setup failure at level 1 (a
  bwrap setup error, a status-descriptor failure before exec, the level's probe failing) →
  level 2 at once, the skip recorded, and the same at level 2 → level 3; a model-load error
  before the first successful `/health`, a hash mismatch or an exit after exec → same level,
  retry path; a lower level goes back up only on Restart or
  app start; a queue with a running and a waiting
  job at a generation change → both end "the assistant restarted: ask again", the breaker
  unchanged; CPU failures
  retried after 2 minutes, four failures → unavailable; a second exit within 10 minutes counts,
  after 10 ready minutes it does not; two consecutive deadline expiries while `/health`
  answers 200 → stopped as hung and restarted; a late exit event of an old generation changes
  nothing; Restart resets the failure count and the breaker, a new child alone does not; a second Mimik on the same data
  directory cannot take the pidfile lock; a leftover child matching PID, start time and path
  is killed, a reused PID is left alone; shutdown → the tree is gone within 5 s;
- device and fallback: per-node sandboxed listings (format pinned from the real build): one
  hardware GPU plus lavapipe → the hardware GPU, lavapipe excluded before the count; only
  `llvmpipe` → CPU; a host where the selected card is `Vulkan1` but its sandboxed list names it
  `Vulkan0` → `--device Vulkan0`; enumeration order changing between discovery and launch → the
  launch name still comes from the in-sandbox listing; two identical cards at level 1 → told
  apart and remembered by PCI address; two identical cards at level 3 → CPU path with the page
  note; dedicated before integrated only when sysfs says so; no `--device` on the CPU path; a
  mismatch at the real start → GPU failure; an offload line with N < M → GPU failure; a level
  that passes for the CPU build but not the GPU build → each path at its own level; a model
  larger than `MemAvailable` minus 1 GiB → refused with the reason; `--assistant-device cpu`
  never lists devices; live (dev host): Vulkan on the RX 6650 XT reaches `ready` with GPU
  budgets, refused calls checked with `strace` once, and killing the child mid-job ends on the
  CPU path with CPU budgets and the page saying why;
- probe: `mimik assistant-runner --probe` at each level opens the model through its
  descriptor (a regular-file `--ro-bind-fd` at level 1), binds the socket or port, reads the
  token channel and, on the GPU path, opens the render node; a removed bind, a missing token
  channel or a refused open fails the probe with that cause; the probe's JSON line and exit
  status match its level; no build accepts a software renderer;
- priming and calibration (fake server): a start before they complete answers 503 "starting"
  and queues nothing; they never count toward the breaker; a sample with a missing, zero or
  non-finite timing, `predicted_n` different from `max_tokens`, a negative tail or cached
  tokens below 75 % of the prefix fails the attempt; the two samples differ from their first
  dynamic byte; fillers of equal bytes but unequal token counts → N_cal is the smaller and the
  same N(B) enters both estimates; T(B) is the larger of the two samples' estimates for each candidate (a sample
  slower in prompt but faster in generation changes the choice for a small B);
  a restarted child re-primes and re-calibrates before admitting jobs; neither carries sheet
  text;
- budgets (unit; formula fixtures, not evidence): the 2b container figures with t 170 → B 512,
  R 90, D 1, Q 30, and with t 365 → unavailable "too slow"; the 2b GPU figures → B 4,096, R 15, D 4, Q 60; a GPU measurement that fits only
  2,048 or 1,024 picks it, and one that fits none → GPU failure and CPU path; a CPU
  measurement that fits only 768 or 512 picks it, and one that fits none → unavailable "too
  slow" with the numbers while number entry, notes and Undo work; R never below 15 or above
  the path's cap; CPU Q + R never above 120;
- assistant off: no child process, no device listing, no preflight; the page says "off";
- rights (connected router): "Restart" and "Try the GPU again" → 403 for a non-owner, CSRF
  checked, a second press while a transition runs → no second transition, a session revoked
  meanwhile → refused;
- live checks (stop for a decision on a mismatch): llama-server serving and checking the token
  over the Unix socket at level 1; `/health` without a token; the per-request `ignore_eos`;
  the `/proc/self/fd` model path; cancellation mid-generation and mid-prompt (the superseding
  job's first token);
- measurement step (to `docs/results.md`, before 2c-2 is marked complete; 2c-2's CPU-baseline
  claim also waits for the user's answer to §5, while 2c-1 does not): the first native
  calibration on this host on both paths (start to `ready`, priming, the two samples, the
  filler's density, the derived B, R, D and Q), which sets the expected CPU outcome; grammar
  against no-grammar per-token generation time, which settles the calibration form; a job's
  real prompt tokens against N(B); the unpacked official CPU asset starting on the documented
  minimum host (a VM), as the guaranteed baseline.

### 2d-1 Owner model setting (local models)

Scope: §1.4b without downloads: the "Model" field with the `local:` form (file handling is
2c-2's), trial with its bounds and Cancel, Switch, Keep and Switch back, migration 022.
Tests:
- grammar: `local:../x.gguf`, `local:/etc/x.gguf`, `local:a/b.gguf`, `local:a\b.gguf`,
  `local:.x.gguf`, `local:CON.gguf`, `local:nul.gguf`, a non-`.gguf` name, white space, a
  non-ASCII character and 513 bytes refused with the rule named; an `hf:` spec refused
  ("downloads arrive later"); the field keeps a refused input;
- file handling (on top of 2c-2's): a `local:` file changed after its trial → unavailable "the
  model file changed";
- trial: a reply schema-invalid or truncated at 384 → fails and the previous model restarts;
  `max_tokens` from the phrases' largest `predicted_n` with the 256 floor and the 384 cap; the
  calibration uses that `max_tokens`; a model too slow for the caps → fails with the numbers;
  a pass shows device, times, speeds, `max_tokens`, B, R, D and Q; players see "starting"
  meanwhile; Switch, Keep and Switch back each restart and re-prime; the default Granite file
  (by hash) keeps 192, and a different file with the same name does not; a phrase past its
  deadline, the whole trial past its bound, and the owner's Cancel each stop the candidate and
  restore the previous model; Switch with a stale trial ID (another hash, an older trial or an
  older generation) → refused;
- rights and routes: a non-owner → 403 on every action; missing or wrong CSRF; extra or
  duplicate keys; body limits; a second trial while one runs → refused; session revoked during
  a trial → its result is not applied.

### 2d-2 Model downloads

Scope: §1.4b downloads: the `hf:` form, metadata admission, licence confirmation, the download
task, the download client and its host list, the `hf:` default and the GPU suggestion. Tests
(fake Hugging Face server on loopback with a test CA through the download client's
constructor unless stated):
- grammar: the default and suggested specs accepted; `@main`, a short or upper-case revision,
  no `@`, `..` or `.` segments, `%2e`, a backslash, a split file, a non-`.gguf` file refused;
- metadata: a commit differing from the revision, a missing LFS entry, a malformed hash or
  size, a size of 0 or over 8 GiB, JSON over 1 MiB → refused before any byte; 401 or 403 →
  "download it by hand";
- licence: no download before confirmation; the record holds the licence file's hash; the
  licence file named by the card, else a root `LICENSE*`, is fetched under the same host rules
  and bound (over 1 MiB → refused); none listed → "none"; a model without a stated licence
  shows the warning and can be confirmed;
- transfer: progress; Cancel keeps the partial and Resume sends `Range` and completes; a 200,
  a wrong start, a wrong total or a malformed `Content-Range` → restart from zero once, then
  failure; a body longer than the expected size is cut off and refused; a hash mismatch
  deletes the file; a crash between write and rename leaves only a partial; a stalled server
  below the progress floor → stopped, waiting for Resume; Cancel, Discard and Resume racing
  completion → one serialized outcome; a second download asks to discard the existing
  partial; a restart leaves a partial waiting; the stored name is the hash; the download never
  holds the `JOBS` permit and the active model serves jobs throughout;
- network: a redirect to a host off the list, to `http` or a third hop → stopped naming the
  host; `HTTPS_PROXY` set → used by the download client and never by the model client;
  requests carry no cookie, credential or `Referer` and send `Accept-Encoding: identity`; with a sheet holding a marker phrase, no
  download request contains it; presigned redirect URLs appear in no log line; a certificate
  not from the root store → refused; the shipped constructor uses the fixed base URL, host
  list and OS store, with no configuration knob;
- suggestion: shown on the GPU path, not on the CPU path; filling the field starts nothing;
  a present file with the default's hash is used without a download;
- rights and routes: as 2d-1 for licence confirmation, Cancel, Discard, Resume and delete;
- live (reported separately, licence-checked per the project rules): the default Granite spec
  downloads and verifies against `e0406663…` and its redirect hosts are recorded for the host
  list; a Qwen3-4B trial on the GPU path at night or at a weekend.

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
  `evaluation-tooling.md` (its "no provider or model integration" scope),
  `release-scope.md` (schema 22, the sheet module under its strict CSP, the local model as
  confirm-only, the bundled llama.cpp build, its licence and source, the confinement levels and
  which were verified, model downloads over HTTPS, Linux x86_64 as the only verified platform,
  the GPU path measured on one RX 6650 XT), the runtime definition contract (the
  `catalogue` member), publication and dependency docs; replace the statements this work
  reverses: catalogue Q4 (not in the catalogue is a warning, "Which Sword?" remembered), the
  catalogue writer membership check (`catalogue_ref`, not built; the key is advisory),
  "no model proposes", the accuracy bar (waived, D-D), and "the LLM does not judge at play
  time" (narrow confirmable proposals and reminders).
- `cargo about`, audit, packaging (the runtime files verified against the manifest), and the
  unpacked smoke with the assistant off and once on the production runtime path (§1.4a, CI
  workflow); stub-endpoint coverage stays in the feature-enabled test build.
- Assistant scope stated as "local endpoint, POC quality, not certified (D-D)", with L1 and the
  other open limitations listed.

## 3. Verification

- Rust gate: fmt, strict Clippy, locked tests (red then green), audit, `cargo about`, release
  build and size delta.
- Browser (Chromium at 1280 × 720 and 390 × 844, demo data, with and without JS): per-cell
  Enter, buttons for selects and checkboxes, row actions, quick-add, notes, switch, Undo,
  unknowns, the two-tab sequence, derived and check refresh, CSP headers and no violations,
  combobox keyboard and IME, no overflow.
- Model runtime (host measurements, reported separately from unit, fake-server and CI
  evidence, never part of the CI gate except the `llama-runtime` job): on this host the CPU
  path at 2 threads and the GPU path (Vulkan, RX 6650 XT, night or weekend), each from a native
  child under bubblewrap: start to `ready`, priming and calibration samples, derived budgets,
  token refusal, cancellation, the GPU → CPU fallback, shutdown and orphan cleanup, the
  confinement tests with the adversarial helper. Landlock on a VM whose kernel has ABI 4 or
  later (this host's kernel has none) and the AppArmor preflight on an Ubuntu VM; until run,
  labelled unverified. macOS (Metal) and Windows (Vulkan): deferred, not claimed.
- Model screen (prototype tier, `tools/assist_eval.py` with unit tests; results note with
  exact scope, denominators and limitations; reviewed; no pass bar, D-D): per path (CPU at 2
  threads, GPU), p50/p95 latency, schema-valid rate, output tokens; per class, correct within the first 3 and
  false-confident. Fast-path inputs never reach the model; results are per class, not general
  accuracy. `tools/assist_eval.py` is new in 2b; its cases are synthetic and independently
  labelled. Extra classes for 2c: valid divisions, overflow, unknown target, provenance
  failure, multi-option, related-field selection at 1 KiB, same-row cell references; injection rows expect an option at
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

Risks: the CPU path may not meet its caps at all (§1.4a: from the 2b container figures, with
the uncached prefix tail included, Granite at 2 threads fits only B 512 at best, and a model
with `max_tokens` 256 or more fits nothing), and `starting` takes priming of about 117 s plus
two calibration samples of about 85 s each plus hashing the model before the assistant is
ready; all from container measurements; model quality is unknown and the bar is waived (the display
and confirmation are the safeguard); reminder prompts carry catalogue text (contained, not
prevented); one revision per Enter makes history long (no-ops write nothing, no cap; D-U);
`rig-core` is pre-1.0 with frequent breaking releases (pinned); HTTP client defaults
(proxies, redirects, retries) could send player text off the machine or repeat it, hence the
client policy and its tests. Runtime: bubblewrap needs unprivileged user namespaces, which
some distributions disable or restrict (Ubuntu's AppArmor rule), and older distributions'
bubblewrap may lack an option the probe needs; those machines fall back to Landlock or to
unconfined, as shown on the page; and Landlock is missing or
lacks network rules on many kernels (this host's included), so a machine can end on a lower
level; the Landlock level leaves inbound listening on other addresses and local Unix sockets
open; a malicious GGUF exploiting llama-server is contained only as far as the active level
goes; the official Linux builds link against a distribution's glibc and Vulkan loader, which
may not match the host; the bubblewrap path list is driver-specific (Mesa on AMD measured;
other drivers may need more paths, and then fall back to the CPU); other applications can take
the GPU's memory mid-use (the fallback moves to the CPU); budgets are estimates from two
samples per start, so throttling later shows up as deadline expiries and restarts; the 2b GPU
run had the fan forced to full, while production uses the driver's fan curve; one GPU (RX 6650
XT) is measured, other GPUs and Metal are not; downloads add TLS and a second network client
to the binary; Hugging Face file hosts can change, which stops downloads until the host list
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
threads off-hours; the quality bar; macOS (Metal) and Windows (Vulkan) runtime, confinement,
lifecycle, endpoint check (no token to an unverified listener) and model-file rules, verified
on a host of each, and Windows confinement without administrator rights (AppContainer);
measurements on other GPUs; parallel slots on the GPU path (ASSIST-32); fraction words and number words across
languages (ASSIST-38); gated Hugging Face repositories (owner token) and split GGUF files;
partial GPU offload; retiring the legacy full-form edit POST; definitions
choosing a fallback for unknown (L-U4).

## 5. Open questions for the user

- **CPU baseline with the default model (ASSIST-27, ASSIST-31, ASSIST-32):** the §1.4a
  illustration from the 2b container figures suggests that Granite 4.2 3B at 2 CPU threads
  fits at best a 512-byte dynamic part within the 90 s cap, and nothing if the uncached prefix
  tail is near its bound; any model with `max_tokens` 256 or more fits nothing. If the first
  native calibration confirms it, the CPU path is off on such machines. Options: allow more
  threads on CPU-only machines (outside the weekday 2-core rule, which governs this dev host),
  a smaller default model on the CPU path, or accept the assistant being off there. The
  decision wording of ASSIST-31 and ASSIST-32 is unchanged until the user decides.
