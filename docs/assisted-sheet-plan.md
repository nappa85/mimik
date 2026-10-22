# Assisted sheet plan: remaining steps

Direction (user, 2026-10-19): "Help the user as much as we can, but don't force anything;
improve paper and pencil, don't replace it." The player writes what they want; the app
computes, warns, suggests and offers Undo. Model output is only ever a confirmable proposal;
legality stays advisory and Rust does all arithmetic.

Decision IDs (D-…, U…, P…) are defined in `docs/decisions.md`. What is built (number entry,
notes and scribbles, switch and Undo, unknown values, per-field save, the busy page, the page
bound, quick-add, the validity flag, the sheet module with save on leaving a field) is described
in `docs/assisted-entry.md`; this plan only adds to it.

Remaining, in order (2b measured; its choices await independent review): **2c**
interpretation, **3** bundles and reminders, **4** suggestions and "Which Sword?", **5** docs
and gate.

Every step: its own branch; failing-first tests with the red run recorded; independent
implementation review before merge; built and tested on 2 cores on weekdays. Each step keeps
the demo (`tools/assist_demo.py`) usable. Any step that adds per-cell markup reruns the
worst-case page fixtures in `src/campaigns/list_tests.rs` (tightest headroom today 2.0%, a new row kept read-only from a full list) and
never enlarges the budget.

**POC cuts** (no security or integrity risk): player `*` and `/`; interpreting text, enum or
boolean cells; notes and Undo for text cells; seals and note verification (D-T); dismissal
records; Undo of creation, row add and moves; forced Undo; live preview; a keyed rate
limiter; Gemma; changes to the native MÖRK BORG editor. Non-goals: legality rulings,
automatic application of model output, chat, manual RAG, `catalogue_ref` membership checks,
hosted providers, GPU, models above about 4B.

## 1. Design for the remaining steps

### 1.4 Interpretation service (2b, 2c)

POC boundary (2026-10-20, after the plan review of the harness design): one local model, one
bounded request per job, current unresolved integer scribbles only, strict validation, origin
binding, poll and apply, the confirmable UI. Plan review round 2 (2026-10-20) fixed the
expression form, the phrase-number lexer, the start route's note key, `ambiguous` replies and
the breaker outcomes. Deferred to §4 Later: model escalation, model
tools, confidence gating, persistent traces and a known-answer set grown from them, and
catalogue, enum or fuzzy matching before the model (that matcher arrives with step 4, one
implementation for both).

- **Shape:** an `Interpreter` trait, a `StubInterpreter` and a crate adapter; further models
  can sit behind the same trait later. Assistant off is the default; when off there is no
  client, no job route and no "Thinking…", and the app is fully usable. The model never blocks
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
  llama-server can reuse its prompt cache; the dynamic part is at most 4 KiB, measured in
  bytes after escaping, with whole context items dropped from the end, never cut. Every
  dynamic string goes inside a fixed data envelope that the instructions declare to be data,
  never instructions.
- **Context (D-F, D-J):** phrase and target first; then other stored integer and enum fields in
  definition order; derived, text, multiline, notes and list cells are excluded from context
  and from `@` references; catalogue entry text comes in step 3 and is dropped first.
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
  assistant's reading". `max_tokens` 192, from the 2b measurement of this schema (pending
  review; `docs/results.md`, assisted interpretation).
- **Confidence:** if the runtime returns token log-probabilities they are recorded on the job
  record for evaluation only; nothing is gated or hidden on them in 2c.
- **Crate (D-H, proposal pending review):** `rig-core` =0.45.0 (ASSIST-39; 2b's offline
  checks re-run on it), default features off plus `reqwest`, no TLS, on the OpenAI chat
  route with the llama.cpp dialect; the schema goes in `additional_params` as
  `response_format` (sent byte-exact; the native `output_schema` path rewrites `oneOf` to
  `anyOf`). The body bound is a small Mimik
  transport implementing rig's `HttpClientExt` over the host-built `reqwest` client (plain and
  streaming bodies refused past the bound); no D-H crate offers one. `genai` was rejected (no
  body hook).
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
  `127.0.0.1`). The model name is configuration, never model output. Loopback is only the
  destination rule; the no-egress property comes from the runtime topology.
- **Runtime** (the 2b dev and benchmark topology; production is a native llama-server child on
  loopback with a random port and a per-launch token under ASSIST-27 to ASSIST-30 on master, so
  the network confinement below does not carry over): one llama-server on an `--internal`
  network created with
  `com.docker.network.bridge.inhibit_ipv4=true` (a plain internal network still reaches host
  services through its gateway, measured in 2b), plus the forwarder
  `alpine/socat@sha256:22d77becaeeea8ef56b8c4bb2312323acafe85354be6a85714ff5050e3537842`
  (`TCP-LISTEN:8091,fork` to `server:8080`) on that network and a bridge with IP masquerading
  off, published only on `127.0.0.1:8091`; `--cpus 2 --threads 2 --threads-batch 2`,
  `--parallel 1`, prompt cache on, `--reasoning off`, greedy sampler, memory, swap and PID
  limits; the model hash-checked before mounting and preloaded at start (no load inside a job).
  Model (proposal pending review): `granite42-3b`; `qwen3-4b-2507` exceeded the 60 s p95 and
  `minicpm5-2b` had 18 of 50 false-confident.
- **Priming:** the server starts with `--no-warmup` and an empty prompt cache, and the first
  request evaluates the whole fixed prefix (about 1,460 tokens: 117 s for Granite at 2
  threads), which always misses the run deadline. So after `/health` answers, the host sends
  one priming request through the same client and policy: the fixed prefix with a minimal data
  envelope and `max_tokens` 1. It is not a job, is never counted by the breaker, and has its own
  budget, at most 300 s (at 2 CPU threads the 1,460-token prefix took about 117 s; the
  measured priming time also sets the job budgets, ASSIST-27). Until it completes the assistant is "starting": a start answers 503 ("the
  assistant is starting"), queues nothing, and number entry, notes and Undo work as usual. On
  failure or expiry the host retries after 2 minutes, at most 3 times, then the assistant stays
  unavailable until restart (logged without prompt text). A transport error followed by a
  `/health` that shows a restarted server (a different start time in `/props`) re-primes
  before jobs are admitted again.
- **Jobs:** one worker; one monotonic budget per job from enqueue, covering queueing,
  rendering, the model call and validation: queue wait plus run deadline. From 2b (pending
  review, CPU path): run deadline 60 s, queue depth 1, queue wait 60 s, `max_tokens` 192.
  Open user decision on the CPU-path budget (`docs/results.md`, ASSIST-L20): a dynamic part near the
  4 KiB bound needs 111 to 136 s with Granite at 2 threads. One job per account (counting running work): a start on another sheet while the
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
  2024-07. The priming request is outside the breaker.
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
dynamic prompt ≤ 4 KiB, the job budgets and record caps, the breaker, the suggest bounds. Loopback IP only; the model server
has no egress; the client follows no redirects and ignores proxy settings; no prompts or player text in logs; no secrets.

## 2. Steps

### 2c Interpretation

Scope: trait, stub, adapter; the client policy; worker, budgets, records and breaker; the job,
poll and apply routes with origin and read/write-set binding; the UI. Tests (stub or fake
server unless stated):
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
  dynamic part stays within 4 KiB and drops whole items; excluded field types never appear in
  the request;
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
- priming (fake server): a start before priming completes answers 503 "starting" and queues
  nothing; a priming failure or expiry never counts toward the breaker and is retried at most 3
  times; a server restart re-primes before admitting jobs;
- breaker (injectable clock): three transport failures open it, two do not; 100 dropped or
  nonsense answers never open it; supersession and cancellation never count; one half-open
  probe while others are refused; probe success closes, failure reopens; a 2xx with all
  options dropped resets the count and closes a half-open breaker; a cancelled probe clears the
  probe flag and leaves it half-open; a queue-wait expiry does not count; mixed sequences (failure, success, failure) do not open it; while open, number
  entry, notes and Undo work and the poll says the assistant is unavailable;
- client: a host name, `localhost`, `127.1`, `0x7f.1`, `2130706433`, `::ffff:127.0.0.1`,
  userinfo or a zone ID refused; `HTTP_PROXY` and `ALL_PROXY` set and a 302 from the fake server
  never reach anything but the endpoint; one request per call on a 500 or 429; a gzip-encoded
  reply is not decoded; a body over the bound is refused, plain and streaming, without
  buffering it; the schema arrives byte-exact in `response_format`;
- topology (live, reported separately): no egress from the model container to a public
  address, a public name or any host address;
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
  4xx bodies other than the player's own echoed draft;
- one live smoke, reported as live inference.

Evaluation set: hand-written and independently labelled cases in `tools/assist_eval.py`
(§3), kept local and out of source packages; it is a regression set, not the EVAL-1 held-out
set, and does not close D-D.

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

- Update `docs/assisted-entry.md`, `release-scope.md` (schema 21, the sheet module under its strict
  CSP, the local model as confirm-only), the runtime definition contract (the
  `catalogue` member), publication and dependency docs; replace the statements this work
  reverses: catalogue Q4 (not in the catalogue is a warning, "Which Sword?" remembered), the
  D9/R9 writer membership check (not built; the key is advisory),
  "no model proposes", the accuracy bar (waived, D-D), and "the LLM does not judge at play
  time" (narrow confirmable proposals and reminders).
- `cargo about`, audit, packaging, and the unpacked smoke with the assistant off and once with
  the stub endpoint.
- Assistant scope stated as "local endpoint, POC quality, not certified (D-D)", with L1 and the
  other open limitations listed.

## 3. Verification

- Rust gate: fmt, strict Clippy, locked tests (red then green), audit, `cargo about`, release
  build and size delta.
- Browser (Chromium at 1280 × 720 and 390 × 844, demo data, with and without JS): per-cell
  Enter, buttons for selects and checkboxes, row actions, quick-add, notes, switch, Undo,
  unknowns, the two-tab sequence, derived and check refresh, CSP headers and no violations,
  combobox keyboard and IME, no overflow.
- Model screen (prototype tier, `tools/assist_eval.py` with unit tests; results note with
  exact scope, denominators and limitations; reviewed; no pass bar, D-D): p50/p95 latency at 2
  threads, schema-valid rate, output tokens; per class, correct within the first 3 and
  false-confident. Fast-path inputs never reach the model; results are per class, not general
  accuracy. `tools/assist_eval.py` is new in 2b; its cases are synthetic and independently
  labelled. Extra classes for 2c: valid divisions, overflow, unknown target, provenance
  failure, multi-option; injection rows expect an option at most, never a write.

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

Risks: latency at 2 threads is tight (Granite p95 41 s, a near-4 KiB request over 2 minutes);
model quality is unknown and the bar is waived (the
display and confirmation are the safeguard); reminder prompts carry catalogue text (contained,
not prevented); one revision per Enter makes history long (no-ops write nothing, no cap;
D-U); the forwarder is a new pinned image that can reach host addresses (it only ever connects
to its upstream); `rig-core` is pre-1.0 with frequent breaking releases (pinned); HTTP client
defaults (proxies, redirects, retries) could send player text off the machine or repeat it,
hence the client policy and its tests.

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
threads off-hours; the quality bar; retiring the legacy full-form edit POST; definitions
choosing a fallback for unknown (L-U4).
