# Assisted sheet plan: remaining steps

Direction (user, 2026-10-19): "Help the user as much as we can, but don't force anything;
improve paper and pencil, don't replace it." The player writes what they want; the app
computes, warns, suggests and offers Undo. Model output is only ever a confirmable proposal;
legality stays advisory and Rust does all arithmetic.

Decision IDs (D-…, U…, P…) are defined in `docs/decisions.md`. What is built (number entry,
notes and scribbles, switch and Undo, unknown values, per-field save, the busy page, the page
bound) is described in `docs/assisted-entry.md`; this plan only adds to it.

Remaining, in order: **1.6** quick-add, **1.7** validity flag,
**2a** JavaScript,
**2b** crate and runtime selection (prototype tier, may run in parallel), **2c**
interpretation, **3** bundles and reminders, **4** suggestions and "Which Sword?", **5** docs
and gate.

Every step: its own branch; failing-first tests with the red run recorded; independent
implementation review before merge; built and tested on 2 cores on weekdays. 1.6 keeps
the demo (`tools/assist_demo.py`) usable. Any step that adds per-cell markup reruns the
worst-case page fixtures in `src/campaigns/list_tests.rs` (tightest headroom today 2.6%) and
never enlarges the budget.

**POC cuts** (no security or integrity risk): player `*` and `/`; interpreting text, enum or
boolean cells; notes and Undo for text cells; seals and note verification (D-T); dismissal
records; Undo of creation, row add and moves; forced Undo; live preview; a keyed rate
limiter; Gemma; changes to the native MÖRK BORG editor. Non-goals: legality rulings,
automatic application of model output, chat, manual RAG, `catalogue_ref` membership checks,
hosted providers, GPU, models above about 4B.

## 1. Design for the remaining steps

### 1.2 Quick-add (1.6)

- A list may declare `"quick_add": {"name": <text property>, "quantity": <integer
  property>?}`; omitted when absent, so existing definition hashes are unchanged.
- Admission refuses it when the list has a required enum or boolean property, when the list
  has a required text or multiline property other than the mapped name (decided 2026-10-19,
  coordinator default: no quick-add line is shown for such lists; regression case required),
  or when it names a property of the wrong type.
- Route `lists/{f}/quick` (the list by index, as the other list routes) with exactly `csrf`,
  `base` and `text`; it protects nothing (append only), like the new-row form.
- Deterministic parse of `text` (≤ 200 characters): `^(\d{1,6})\s+(.+)$` → quantity and name;
  `^(.+?)\s*[x×]\s*(\d{1,6})$` → name and quantity; otherwise the whole text is the name with
  quantity 1; an empty name → "Nothing to add".
- The mapped name and quantity are always filled, even when they are optional properties.
  Other required numbers are `null` (unknown); every other optional property is absent. One
  revision, no note.

### 1.3 JavaScript (2a)

- One module, `assets/sheet-assist.js`, loaded with `<script type="module">` on sheet pages.
  Without it every save, Undo, switch, quick-add and row action keeps working through PRG
  (D-G).
- **CSP:** a `SheetPage` response extension applied in `normalize` gives `default-src
  'self'; script-src 'self'; style-src 'self'; object-src 'none'; base-uri 'none';
  form-action 'self'; frame-ancestors 'none'; require-trusted-types-for 'script';
  trusted-types 'none'`. The inline `style="min-width:0"` in `private_sheet.html` moves to
  `app.css`; the module uses classes, never `setAttribute('style')` or `innerHTML`. Error pages
  from outer middleware (timeout, 503) keep the default header; document that.
- **Asset route:** the auth router serves only `app.css` and `logo.png` today; add the module.
- **Transport:** requests are `URLSearchParams` bodies (`post_origin` refuses multipart with
  415); responses are JSON; integers and revisions are strings, recursively (list values,
  note values, revisions).
- **Save response:** `{revision, unit: {address, value, note?}, derived: [{id, text}],
  checks: {violated, not_evaluated, passed, items: [{id, status, message}]}}`, computed from
  the new revision. The module updates the saved unit, the derived texts and the checks
  section only; no other input and no other unit's `base` moves.
- **Per-unit state:** after a save the module updates `base` only for the saved unit, and
  only when the response revision is newer. Requests per unit are serialised (one in flight,
  the latest queued); a late response for an older request is ignored. A draft generation
  counter per unit increments on `input`; a response always updates the unit's acknowledged
  value and `base`, but replaces the control's text only if the draft is still the submitted
  generation. Derived values and checks have their own projection revision and update only
  from a response at least as new.
- **Save on leaving the field (D-AA):** a changed field also saves on blur/change through the
  same per-unit route as Enter; an unchanged field (equal to its last save or offer) sends
  nothing; leaving a field while its previous save is in flight queues one save of the latest
  value. `change` saves selects and checkboxes. Enter still saves; without JavaScript Enter
  remains the only way.
- Accessibility: live region, real buttons, focus returning to the cell, 24 × 24 px targets,
  390 px layout.

### 1.4 Interpretation service (2b, 2c)

- **Shape:** an `Interpreter` trait, a `StubInterpreter` and a crate adapter. Assistant off is
  the default and the app stays fully usable with it off.
- **Crate selection (D-H):** an established Rust provider library (`genai` or `rig-core`),
  checked against an axum stub for: loopback HTTP; structured-output pass-through; no
  redirects, proxy or hosted fallback; a bounded body; a timeout and cancellation that closes
  the connection; licence and footprint; maintenance. If none can send the schema or grammar:
  stop and ask.
- **Candidates are judged on results, not on project rules** (user, 2026-10-19). Besides
  genai or rig over a local model, screen strot-ai/Project-STROT (MIT, Python: compile-time
  LLM synthesis, deterministic per-record decisions with calibrated confidence), called from
  Rust through pyo3 or ported to Rust. If a winner conflicts with a rule (D-H, Rust-only
  runtime, the lightweight standalone binary), the results note states the conflict and its
  cost and the user decides whether to bend the rule.
- **Runtime:** llama-server on an `--internal` network plus a pinned forwarder bound only to
  `127.0.0.1:8091` with a fixed upstream (never a general proxy); `--cpus 2 --threads 2`,
  memory, swap and PID limits; the model hash-checked before mounting. Model screen:
  `qwen3-4b-2507` and `granite42-3b`, fallback `minicpm5-2b`, on the pinned llama.cpp image
  with the hardened runner flags of `tools/run_tool_loop_benchmark.sh`.
- **Endpoint:** an IP literal, `127.0.0.1` or `::1`; host names refused.
- **Context (D-F, D-J):** phrase and target first; then other stored integer and enum fields
  in definition order until 4 KiB (derived fields excluded from context and from `@`
  references); catalogue entry text last (step 3), cut first when space runs out. Every
  dynamic string is untrusted data.
- **Output:** a strict JSON schema whose expression field has a `pattern`; terms are literals
  or `@field` references; operators `+ − × ÷` and parentheses; `÷` is rounded at once with
  `floor` or `ceil`, or must be exact (`none`); every literal must appear in the phrase or
  equal the target's value (the only exception is the step-3 catalogue provenance extension,
  §1.5); the display shows base, expression and effect, labelled "the
  assistant's reading". `max_tokens` 128, measured in 2b, raised to 256 if bundles truncate.
- **Jobs:** one worker; a queue of 4 in total and 1 per account (counting running work);
  supersession checked before the capacity refusal; 60 s queue and 30 s run deadlines; at most
  64 records, at most 6 non-terminal (queue, running, reminder), so terminal eviction always
  makes room; 128-bit IDs; poll and apply recheck rights; the `JOBS` permit is not held during
  the model call. A reminder job (step 3) uses its own low-priority slot bound to the account
  and does not take the per-account cap.
- **Flow (JS):** a cell save records `unresolved` and the module starts a job; "Thinking…" in
  `role="status"`; then 1–3 options with base, expression and effect, or the choices of an
  `ambiguous` reply; **Use** posts the apply route; ignoring them leaves the scribble. The
  apply route must not rely on how long a switch offer stays on screen or treat a shown offer
  as confirmation.
- **Proposal freshness (D-P):** when a job finishes, Rust validates each option and stores its
  bound read/write set: every write target (address, value at the job's base revision, host
  ID for row cells), every `@` reference (address and value), the displayed effect (before →
  after per target), and for reminders and bundles the supplied context (each list-row name
  sent, its remembered key, the catalogue version and entry key of any entry text, every enum
  value). Bundle targets must be distinct; a duplicate drops the option. The guarantee covers
  exactly this set, not the model's relevance judgement.
- **Apply:** `POST /private-sheets/{id}/proposals/{job}/{n}` (`csrf`), in one `IMMEDIATE`
  transaction: recheck rights and the active state; check the job binding and that it is not
  already applied (single use, marked consumed only after commit); require every bound
  address to hold its bound value and every bound row to exist; re-evaluate and require the
  effect to equal the displayed one; write with `confirmed` notes sharing a `bundle` number.
  Any mismatch or expiry: "Something this proposal used has changed: ask again", nothing
  written, never a literal fallback. An unrelated field change does not invalidate it.

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
  ("from Shield: …"). Every other number must pass the step-2c rule (in the phrase, or equal
  to the target's value); outside reminders the step-2c rule stays strict.
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

Model output passes the schema, the grammar, literal provenance, Rust evaluation and
read/write-set freshness, and is rendered escaped inside `<bdi>`. Rights are rechecked on every
request and inside every write transaction. Limits: raw ≤ 200, notes ≤ 256 per revision,
prompt ≤ 4 KiB, the job and record caps, the suggest bounds. Loopback IP only; the model server
has no egress; no redirects or proxies; no prompts or player text in logs; no secrets.

## 2. Steps

### 1.6 Quick-add

Scope: §1.2 (schema member, admission refusals, parse, route). Visible: "3 torches" + Enter
adds Torches ×3; "healing potion x2" adds the potion ×2. The demo definition gains
`quick_add`. Tests: both patterns, a plain name (→ 1), an empty name, a 7-digit count
(treated as part of the name), 200 characters; a required enum in the list → admission
refusal; a required text or multiline property other than the mapped name → admission refusal
and no quick-add line; an optional mapped name or quantity is still filled; unknown values for other required numbers; no notes written; old definition hashes
unchanged.

### 1.7 Validity flag (RR-3)

Scope (decided 2026-10-19, coordinator default): the sheet shows **"Not validated"** whenever
any check is violated, not evaluated or in error, or when rule coverage is unavailable for the
pin; otherwise it shows nothing extra. Saving is always allowed; the flag never blocks, hides
or changes a value. It is computed on view from the saved revision (like derived values), not
stored. Shown in the sheet header and on each history revision. Tests: one violated check →
flag; a not-evaluated check (unknown input) → flag; an evaluation error → flag; a pin without
rule coverage → flag; all checks passed with coverage → no flag; a save of a flagged sheet
succeeds; history shows the flag per revision; no-JS and 390 px layouts.

### 2a JavaScript foundation

Scope: §1.3 without the combobox. Tests:
- D-AA: Tab out of a changed HP saves once; Tab out of an unchanged HP sends no request;
  Enter followed by blur saves once; the no-JS run still saves only on Enter;
- final headers on 200, 409 and 422; the final CSP after `normalize` on the new routes and
  their refusals; other routes unchanged;
- a multipart body still gets 415 and the module sends URL-encoded;
- 9007199254740993 round-trips; integers are strings recursively;
- the two-tab sequence with JS: a saved HP response leaves the dirty Armour `base` alone, and Armour → 409;
- draft generations: a delayed HP response after the player typed 60 without submitting keeps
  the 60 and updates the base; repeated with a queued second save;
- projection ordering: HP (revision 11) and Equipment (12) responses arrive reversed; both
  units acknowledge, Load and the checks show revision 12;
- a save that changes `load` and `hp <= max_hp` updates both read-only regions without
  touching other inputs;
- no CSP violations; a lint test over the module refuses `innerHTML` and style attributes;
- the inline style moved to `app.css`; the no-JS browser run still green.

### 2b Crate and runtime selection (prototype tier; may run in parallel)

Scope: §1.4 crate items and the D-H stop; the topology with egress tests from inside the model
container and from the forwarder (which must not proxy anywhere but its upstream); a latency
and schema screen (§3) with output tokens measured; the STROT candidate judged on the same
results. Output: a results note, independently reviewed before it picks the crate, model or
approach.

### 2c Interpretation

Scope: trait, stub, adapter; IP-literal loopback config; worker, queue and records; poll; the
apply route with read/write-set binding; the UI. Tests:
- schema-invalid, grammar-invalid, literal provenance (`84 + 915` dropped), a text field
  referenced, `÷` on negatives, division by zero;
- queue: the 5th job and the account's 2nd → 429; supersession before capacity; cancellation
  (the fake server sees the disconnect); the 60 s and 30 s deadlines; 64 records; at most 6
  non-terminal records under reminder creation, supersession and cancellation;
- freshness: a referenced field changes while the target does not → refused; a referenced row
  disappears → refused; effect mismatch → refused; a second apply, including two concurrent
  applies (exactly one writes, consumed set only after commit) → refused; another sheet's or
  account's job or an invalid option index → refused; an unrelated field change → still
  applied; an expired job → no write and no literal;
- another account's job or revoked rights → not found or 403; `JOBS` free during the model
  call; a host name or non-loopback endpoint refused; assistant off is honest everywhere; no
  player text in logs;
- one live smoke, reported as live inference.

Idea to evaluate here (2026-10-19, from Project-STROT): gate suggestions on calibrated
confidence (a warning only below the threshold) and keep a known-answer set every model or
prompt change must pass; the set doubles as the way back to the waived quality bar (D-D).

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
(neither the phrase, the target's value nor the cited entry text) → option dropped.

### 4 Suggestions and "Which Sword?"

Scope: §1.6. Tests: confinement (forged address, unlinked catalogue, other game, native,
unsupported or corrupt pin); the type filter on every path; 2-character prefix; typo distance
1 and 2; diacritics; query, typo-pass and cache-byte bounds; no `JOBS` 503; 429; escaping;
choosing among duplicates stores the key and the link persists; renaming clears the key; a
dangling key shows no link; p95 measured.

### 5 Docs, release scope, gate

- Update `docs/assisted-entry.md`, `release-scope.md` (schema 21, one script on sheet pages under
  a strict CSP, the local model as confirm-only), the runtime definition contract (`quick_add` and
  `catalogue` members), publication and dependency docs; replace the statements this work
  reverses: catalogue Q4 (not in the catalogue is a warning, "Which Sword?" remembered), the
  D9/R9 writer membership check (not built; the key is advisory), "no script on sheet pages" (other
  pages already serve `account-sharing.js` and `source-regions.js`),
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
  accuracy.

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

Risks: latency at 2 threads is unmeasured; model quality is unknown and the bar is waived (the
display and confirmation are the safeguard); reminder prompts carry catalogue text (contained,
not prevented); a crate gap would stop 2c (D-H); one revision per Enter makes history long
(no-ops write nothing, no cap; D-U); the forwarder is a new pinned image.

Later: sealing and verifying notes (before release, L1); a per-account write-rate limit or
storage guard (before release, D-U); player `*` and `/`; text, enum and boolean
interpretation; notes and Undo for text; Undo of moves and creation, forced and arbitrary
restore; owner-reviewed reminder rules without a model (D-I); a model for quick-add; more
threads off-hours; the quality bar; retiring the legacy full-form edit POST; definitions
choosing a fallback for unknown (L-U4).
