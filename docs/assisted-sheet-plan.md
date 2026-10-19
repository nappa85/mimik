# Assisted sheet MVP plan: lenient input, CPU interpretation, suggestions, undo

2026-10-19. **Revision 2.** Base: master `71a7841`. The code is unchanged since `4c8d653`, and
every reference was re-checked there.

Revision 1 (`71a7841`) was reviewed:
- Sol: **NO-GO**, Blocking B1–B3 and Majors M1–M14.
- Sonnet: **GO WITH AMENDMENTS**, Blocking B1, Majors M1–M10 and minors.
- The third chain was unavailable.

The user then decided D-A to D-P (§0). This revision folds in every decision, resolves every
Blocking finding, and gives each Major a contract and failing-first tests or a justified
deferral. The finding map is in §8.

This revision needs a re-review before step 1a starts. It replaces the shelved
[catalogue-step4-plan.md](catalogue-step4-plan.md) as the next product step.

## 0. Direction and decisions (user, 2026-10-19)

"We're improving the paper and pencil paradigm, not replacing it." The player writes what
they want in a field, and the app does the math, warns, suggests and offers an undo. Nothing
is forced.

**Initial direction (U1–U9, rev 1).**
- Free input such as `-15` or `84 -15` is understood.
- The stored value is the result, shown afterwards as "84 − 15 = 69 · Undo", with history.
- JavaScript is allowed on sheets.
- LLM interpretation runs now, on CPU, with a model of about 4B or less, capped at 2 cores on
  weekdays.
- No parser for every phrasing; a tiny arithmetic fast path is enough.
- Ambiguous input gets disambiguated, and the player confirms every model proposal.
- Content never blocks a save; size and security limits still can.
- Catalogue Q4 is reversed (an unknown item gets a warning).
- POC first.

**Decisions on revision 1 (binding).**

| # | Decision | Where |
|---|---|---|
| D-A | A leading sign is relative **only** on fields the definition marks as **pools** (HP, ammo…). Elsewhere `-1` is the literal −1 | §3a |
| D-B | A picked suggestion stores the **name as plain text**, linked by name when the sheet is shown | §3e |
| D-C | **Each field saves on its own when Enter is pressed** (spreadsheet-like). There is no single Save button | §3a, §3g |
| D-D | The model quality bar (100 held-out cases, ≥ 90% correct, ≤ 2% false-supported) is **waived for the POC**. Revisit it before a real release. Engineering gates stay | §7 |
| D-E | Masters see the raw unresolved text and the change notes in history (this relaxes the values-free summary) | §3f |
| D-F | The model reads only the phrase, the target field and the sheet's **number and enum** fields, never text or multiline fields | §3b |
| D-G | The assistant and live suggestions require JavaScript. The sheet itself (typing, arithmetic, notes, Undo) works without it | §3g |
| D-H | The provider client is an established Rust library only (`genai` or `rig-core`). If no library can send the structured-output schema or grammar to llama-server: **stop and ask the user**. No hand-written HTTP call | §3b, step 2b |
| D-I | Consequences across fields (a shield absorbs damage and breaks). Model proposals may be **multi-field bundles**, applied and undone as one. **Reminders** are dismissible hints, never blocks, triggered when a pool field decreases. POC knowledge: sheet item names plus linked catalogue entry text. Later: rules authored by the LLM and reviewed by the owner, so reminders work with the model off | §3c |
| D-J | An exception to D-F: the model may read the names of list-row items and their linked catalogue entry text. Notes, backstory and multiline text stay excluded | §3b, §3c |
| D-K | Order: arithmetic, notes and Undo → model interpretation → bundles and reminders → suggestions | §5 |
| D-L | Negative values on pools must be **disambiguated**, not settled by a hidden syntax. Proposed low-friction form: apply as relative by default and show "84 − 15 = 69 · Undo · Set to −15 instead"; ask first where riskier. Works without JavaScript | §3a (Q1 confirms) |
| D-M | A new list row (or newly present property) with an unreadable required number keeps the row. That number is stored as **unknown**, with the scribble as a note and a warning | §3a |
| D-N | Undo in the POC covers **the last change of each field only**. Older history is viewable; arbitrary restore comes later | §3f |
| D-O | Duplicate catalogue names: link automatically only when exactly one eligible entry matches. Otherwise ask "Which Sword?" with options | §3e (Q2) |
| D-P | Stale or expired proposals never silently become a literal; they need reconfirmation | §3c |

## 1. Goal, user value, scope

**User value.**
- A player presses Enter on "-15" in Current HP and sees "84 − 15 = 69 · Undo · Set to −15
  instead".
- "took 15 fire damage" gets the assistant's "HP 84 → 69 (84 − 15)? [Use]".
- After damage, "You're taking damage: did you consider your Shield?" offers a bundle (HP +3,
  Shield broken) that is applied and undone as one.
- Typing "heal" in an equipment row lists healing items. "sord" gets "Did you mean Sword?",
  and "asfgh" is saved as text with a warning.

**POC scope.**
- Per-field save on Enter, for scalar fields and per list.
- Pool marker and arithmetic fast path.
- Pool disambiguation; unknown for unreadable required numbers.
- Sealed change notes.
- Undo of the last change per field.
- A loadable synthetic demo.
- One JavaScript module under a strict CSP.
- CPU interpretation through a provider crate, behind a bounded job worker.
- Confirmed proposals and bundles, reminders.
- Name suggestions and "did you mean".

**Cut from the POC.** None of these cuts carries a security or integrity risk; each
"later" item is listed in §6.
- No `*` or `/` in the player's fast path; the model's expression language has them.
- No interpretation for text, enum or boolean cells; enums and booleans stay selects.
- No dismissal records; dismissing a reminder is client-side.
- No undo of creation, of new rows, of row removal or moves, and no whole-revision restore.
- No live arithmetic preview; Enter saves and the result shows at once.
- No per-account rate limiter crate; fixed in-flight caps (§3b) bound the work.
- No Gemma 4 E4B in the screen; the user did not clear it as "about 4B".
- No change to the native MÖRK BORG editor (`native_draft`, `web.rs:685`); it keeps its strict
  path.

**Non-goals.**
- No legality rulings by the model.
- No automatic application of model output.
- No chat.
- No manual RAG in prompts; catalogue entry text only, under D-J.
- No `catalogue_ref` keys.
- No hosted provider; the endpoint is loopback only.
- No GPU, and no model above about 4B.

## 2. Facts from the current code (master `71a7841`)

**Save path and values.**
- `values::integer` (`src/campaigns/values.rs:164-174`) accepts exact signed decimal text only.
- `scalar_value` (`:541-552`) and `values::form` (`:614-656`) refuse anything else, and so does
  `reconcile` for list rows (`:558-612`).
- An `Invalid` refuses the whole save with 422 (`web.rs:1210-1230`).
- `apply_save` checks the revision first, so a stale draft is 409 (`:1124-1148`).
- The page uses PRG: a redirect to `/private-sheets/{id}` (`web.rs:1236`).
- A saved value is rendered with `Value::to_string()`, so −1 becomes `-1`, and the full form
  re-posts every field (`web.rs:837-844`, `templates/private_sheet.html:12`). Under rev 1's
  rule this was Sol's and Sonnet's B1.
- Value schema:
  - integer cells are `{"type":"integer"}` (`values.rs:94`);
  - `null` is allowed only on nullable fields (`:142-149`);
  - "`null` never appears inside items" (`:116`).
- Formula inputs refuse `null` on a non-nullable input, which fails the **whole** evaluation
  (`src/definitions/formula/convert.rs:111-118`).
- Definitions: `Field` and `ItemProperty` (`src/definitions/mod.rs:170-202`) and the closed
  schema (`schemas/definition-v1.json:36-80`, `additionalProperties: false`). Optional members
  are omitted when absent (`skip_serializing_if`, `mod.rs:115-128`, `:180-183`), so existing
  definition hashes are kept.

**Writer and history.**
- `private_save_keeping` runs authorization, pin checks and `snapshots::write` in one
  `IMMEDIATE` transaction (`src/campaigns/sheets.rs:266-306`).
- `snapshots::write` is the sole writer (`snapshots.rs:306-406`). Its operations are `create`,
  `edit` and `freeze` (`:332`), it refuses a stale revision with 409 (`:338`), and a removed
  host ID never returns (`:67-70`).
- History triggers (`migrations/012_campaigns.sql:95-127`):
  - an insert must match the live row's revision, hash and snapshot;
  - updates and deletes abort;
  - `private_sheet_events_valid` re-validates every event.
- The snapshot is `deny_unknown_fields` (`snapshots.rs:26-27`), so it cannot carry notes
  without a new encoding.
- The change summary is values-free (`history_diff.rs:1-5`) and is computed from N and N − 1
  (`sheets.rs:44-72`).

**Concurrency.**
- There is one SQLite connection behind a mutex (`src/store.rs:35`, `:107-132`).
- Every `/private-sheets/*` route holds `definitions::JOBS`, a semaphore of 1 taken with
  `try_acquire`; a busy server answers 503 (`definitions/mod.rs:29-37`, `campaigns/web.rs:42-56`,
  `:112-142`).
- Auth mode: 8 global request permits and a **5 s** timeout per request
  (`src/auth/mod.rs:37-38`, `:205-238`).

**JavaScript and CSP.**
- The sheet loads no script (`templates/private_sheet.html:1`), and its form has an inline
  `style="min-width:0"` (`:7`).
- The auth router serves only `app.css` and `logo.png` (`auth/mod.rs:191-193`).
- `normalize` sets `Content-Security-Policy: frame-ancestors 'none'` unconditionally on every
  response (`auth/mod.rs:429-450`). A handler-set CSP would be overwritten (Sol M11).
- The release scope certifies "no script" (`docs/release-scope.md:139`).

**Catalogues.**
- FTS name index with `unicode61 remove_diacritics 2` and `prefix 3`
  (`migrations/019_catalogues.sql:111`).
- `fts_query` bounds pieces, not bytes, and only words of 3 or more letters get a prefix
  (`src/sources/search.rs:66-121`).
- The browse join is confined to one catalogue's rowid range and latest sealed version
  (`src/catalogues/read.rs:221-310`, `:344`). Names are read up to 400 bytes (`:24`).
- Names are **not unique** (`019_catalogues.sql:74-89`).
- Unit review badges are live, so they are not cacheable. Sealed versions are immutable.

**Demo assets.**
- `tests/support/source_fixture.rs` writes `definition.json`, `formula-definition.json` and
  `list-definition.json` for the synthetic game `synthetic-game/first` (`:57-95`).
- It is run by `cargo run --locked --example source_fixture -- <dir>` (CI,
  `.github/workflows/ci.yml:34`).
- `tools/smoke_release.py` already drives the whole setup: `register-ruleset`,
  `import-source`, `import-definition-draft`, compare, approve and publish, accounts and
  campaign over HTTP (`:329-395`, `:478-700`).
- `examples/tool-loop-demo/definition.json` is **not** a loadable runtime definition (Sol M14).

**Model infrastructure** (from rev 1, unchanged).
- Registry `tools/models.json`. On disk and hash-verified at 4B or less: `qwen3-4b-2507`
  (non-thinking), `granite42-3b` (Italian tested), `minicpm5-2b`, `lfm25-2.6b` (always thinks;
  excluded) and `gemma4-e4b` (excluded).
- Pinned image `ghcr.io/ggml-org/llama.cpp@sha256:217d278e…ddfff`, with hardened flags in
  `tools/run_tool_loop_benchmark.sh:476-487` (internal network, non-root, `--cap-drop ALL`,
  `--read-only`, memory, swap and PID limits).
- `response_format: json_schema` was exercised in `tools/local_rag_benchmark.py:385-406`.
- No CPU measurement at 2 threads exists. At 6 threads with long prompts: 72–152 s.
- `strsim` 0.11.1 and `unicode-normalization` are transitive in `Cargo.lock`; `governor` is
  direct. There is no HTTP client crate yet.

## 3. Design

### 3a. Per-field save, the fast path, pools, notes

**Per-field forms (D-C).** On the edit page every top-level field is its own `<form>`.
Without JavaScript, Enter in a single-line input submits only that form.

| Field kind | Route | Admitted keys |
|---|---|---|
| Scalar (text, integer, enum, boolean) | `POST /private-sheets/{id}/fields/{field_id}` | `csrf`, `revision`, `value`, plus `null` if the field is nullable |
| Name, General notes | the same route, with ids `name` and `notes` | as above. Multiline gets a small "Save" button, because Enter is a newline there |
| List | `POST /private-sheets/{id}/lists/{field_id}` | `csrf`, `revision`, `op`, and that list's row keys only (the M4 key grammar restricted to one list) |

- Enter in any row cell saves **that list** (a list is one field). Add, Remove and Move are
  draft ops on that list only (the M4 `Op` code applied to one list).
- Fields not in the form are taken from the saved snapshot, never from the browser.
- Each route admits its own exact key set (a new `field_keys` and `list_keys`, next to
  `sheet_keys`, `values.rs:24`). Dedicated routes need no hidden proposal keys inside sheet
  forms (Sonnet M2, Sol M6).
- **No-JS reload:** a field save does PRG back to `#f-<field_id>`. Text typed into *another*
  field without pressing Enter is lost on that reload. On paper, that is a note never
  written. JavaScript (§3g) saves through `fetch` and keeps the page. This is Q3.
- The **create page stays one full form**, because there is no sheet yet. It uses the same
  interpreter (no base, so there are no relative inputs). With D-M, create never refuses on
  content.
- The existing full-form edit POST stays accepted, for open tabs and the M4 tests. It goes
  through the same interpreter and the unchanged-cell rule below.

**Field-scoped conflicts.** A field or list save carries the page's revision R. The server
answers 409 only if **that address** changed between R and the live revision L. It loads R
through `snapshots::raw_load(…, Some(R))` in the same transaction.
- Otherwise it applies the change to L and writes L + 1. Saving HP and then Armour from the
  same page works, and two tabs editing different fields don't collide.
- The base of a relative input is the value at L, which equals the value at R under this
  rule.

**Pool marker (D-A).** A new optional member `"entry": "pool"` on integer fields and integer
item properties.
- Closed schema; omitted when absent, so old hashes are kept.
- The domain check refuses it on non-integer controls and on formula fields.
- It is not a reference, so it needs no catalogue sites. It is documented in
  `runtime-definition-contract.md`.

**Unchanged-cell rule (B1).** For every submitted integer cell, if `trim(raw)` equals the
canonical rendering of the saved value at that address (decimal, `-` for negatives, host ID
for rows), the cell is unchanged: no interpretation and no note.
- Per-field save submits one field, but list forms re-post all rows, and so does the legacy
  full form. This rule makes re-posting idempotent everywhere.
- Accepted limit: retyping exactly the saved string ("-6" over a saved −6) is a no-op. The
  page shows "No change". Test it.

**Grammar (M1).** Player input is ASCII-trimmed, and the field's `maxlength` is 200 for every
cell. The fast path only reads inputs of 64 scalars or less; longer input is unresolved.

```
int      = DIGIT+                       ; ASCII 0-9 only; leading zeros allowed ("007" = 7)
sign     = "+" | "-" | "−"              ; U+2212 accepted
op       = sign
literal  = [sign] int                   ; non-pool: the literal value; pool: see below
absolute = int 1*( ws* op ws* int )     ; e.g. "84 -15", "84-15+3" (= 72); no signed terms
relative = sign ws* int *( ws* op ws* int )   ; pool fields only: applied to the base
```

- Evaluation is left to right in **i128**. The result must fit i64, otherwise the input is
  unresolved ("too large"). `-9223372036854775808` parses.
- Anything else is **unresolved**: "84--15", "84+-2", "5.", full-width digits, words, `*`,
  `/`, parentheses.
- Non-pool fields: `literal` and `absolute`. A leading sign is part of the literal (`-1` →
  −1, `+3` → 3).
- Pool fields: a sign-led input is `relative`. An unsigned `literal` or `absolute` is a set.
- **Blank** → unknown (D-M), with an "erased" note. On nullable fields the existing
  Known/Unknown select still decides: Unknown means null whatever the text says, and
  Known plus blank means unknown with a warning.

**Pools and negatives (D-L).** For a relative input on a pool with base b and result r:

| Case | Behaviour |
|---|---|
| b ≥ 0 and r ≥ 0 | Applied. Note "84 − 15 = 69 · Undo · Set to −15 instead" |
| r < 0, or b < 0 | **Asked first; nothing is written.** The field re-renders (200) with three submit buttons: "Subtract: 84 − 90 = −6", "Set to −90", "Cancel". Each one posts `raw` + `choice=subtract\|set` + `revision` to the field route. The server recomputes both readings and writes the chosen one with note kind `choice` |
| No base (new row, unknown value) | A sign-led input is the literal (there is nothing to subtract from), with the note "Set to −15 (no previous value)" |

"Set to −15 instead" is a POST with `raw`, `choice=set`, the note's revision and the address.
It goes through the field-scoped conflict rule.

**Unknown for unreadable numbers (D-M).**
- An unreadable integer cell (edit, create, new row or newly present property) stores
  **`null`**, plus an `unresolved` note holding the raw text and a warning. On edit with a
  known saved value, the saved value is **kept** and the note is written.
- D-M covers the cells with no value to keep. Keeping the value is the paper reading of the
  same rule and keeps formulas computing.
- The value schema then admits `["integer","null"]` for every integer cell, including list
  properties. This relaxes `values.rs:94` and `:116`; it only accepts more, so old snapshots
  stay valid.
- **Writer rule:** a `null` at a non-nullable integer address is accepted only if the
  previous revision had `null` there, or this revision's sealed note set has an `unresolved`
  or `erased` note for it.
- **Formulas:** `convert::inputs` (`convert.rs:111-118`) maps such a `null` to CEL `Null`. A
  formula that reads it fails per field with the existing named-dependency message ("Not
  computed: Current HP is unknown"). Formulas that don't read it still compute.
- The sheet shows "?" plus the scribble.

**Change notes (sealed, B3).** Migration 020 adds two tables.
- `private_sheet_notes(sheet_id, revision, ordinal, address, kind, raw, result, explanation,
  bundle, target)`:
  - `kind` ∈ `literal`, `arithmetic`, `choice`, `unresolved`, `erased`, `kept`, `reverted`,
    `confirmed`;
  - `raw` ≤ 200 characters, `result` is the canonical i64 text or empty, `explanation` ≤ 160
    characters (built by the server);
  - `bundle` is a small integer or null; `target` is the reverted revision.
- `private_sheet_note_seals(sheet_id, revision, count, digest)`: the digest is SHA-256 over
  the canonical JSON of the revision's note rows in ordinal order. It uses the existing `sha2`.

Triggers:
- A note insert needs the live `private_sheets` row at `revision = NEW.revision`, its
  `private_sheet_history` row, **no seal yet**, a dense `ordinal` (it equals the current
  count), a well-formed address and kind, and the size caps.
- A seal insert needs the same live revision, `count` equal to the inserted rows, and no
  existing seal.
- Notes and seals: update and delete abort, with replacement guards on every key (the
  008/019 pattern).
- **Late insertion is impossible:** after the commit the revision has a seal, and once a later
  revision exists the live row no longer matches.

Writer and transaction:
- `private_save_keeping` writes the notes and the seal **after** `snapshots::write`, inside the
  same `IMMEDIATE` transaction, for every runtime `create` or `edit`, even when the count is 0.
  A freeze writes none.
- Any note failure rolls back the snapshot, live row and history.
- The notes travel in `SheetView` from the interpreter, which runs in `spawn_blocking` before
  the transaction.

Bounds and verification:
- At most **256 notes and 128 KiB per revision**; above that the save gets 422 as a size
  limit. That only happens with very large list forms.
- **Read-time verification** runs before a note is shown:
  1. The seal exists and its count and digest match.
  2. The note matches history: `arithmetic` and `choice` re-evaluate `raw` over the value
     at R − 1 and must equal the value at R; `literal` and `confirmed` equal the value at R;
     `unresolved`, `erased` and `kept` agree with R versus R − 1 (kept or null); `reverted`
     equals the value at `target − 1`.
  - A failure shows "Note unavailable", like the degraded history summary.
  - `confirmed` proves only "you chose an assistant option" (Sonnet m5), and the wording says
    so.

**Note lifecycle (M3).**
- Every value change writes a note, `literal` included. So the **current note of an address**
  is simply its latest sealed note. A later literal resolves an earlier `unresolved` note.
- `kept` ("Keep 84", a button on an unresolved note) records the input decision without a
  value change. That is a revision with unchanged data, which `snapshots::write` allows; the
  history diff shows no change.
- Unchanged cells, freeze and list structure ops write no note.
- The current notes come from one indexed query, `(sheet_id, address, revision DESC)`, bounded
  by the page's addresses.
- Notes add one bounded line per rendered cell (≤ 600 bytes of markup). Step 1a measures the
  worst publishable page with a note on every cell.

### 3b. Interpretation service (CPU)

**Shape.** A domain trait `Interpreter { propose(Request) -> Proposal }`.
- A deterministic `StubInterpreter` covers tests and the assistant-off build.
- A crate adapter talks to a local llama-server. Mimik owns only the request, the schemas,
  validation and policy.

**Provider crate (D-H).** Step 2b compares `genai` and `rig-core` at pinned stable versions
against an axum stub that mimics llama-server. No model is needed. The bar:
1. A loopback HTTP custom endpoint with no key.
2. **Structured output passed through** (`response_format: json_schema` strict with
   `pattern`, or a GBNF `grammar` through a documented extra-body option).
3. **Redirects refused**, no ambient proxy or environment-derived endpoint or key, and no
   hosted fallback.
4. A bounded response body, a timeout, and **cancellation that closes the connection**. The
   test checks that the stub sees the disconnect.
5. MIT or Apache licence, a minimal-feature `cargo tree` and binary-size delta, and no TLS
   stack if it can be avoided.
6. Maintained.

If neither crate meets item 2: **stop and ask the user** (D-H). The result is recorded in
`dependency-decisions.md`.

**Runtime (Sol, container isolation).** An owner-started dev topology, documented and not
managed by Mimik:
- `mimik-assist`: llama-server on an `--internal` Docker network, so it has no egress.
  - Flags: `--user 1000:1000 --cap-drop ALL --security-opt no-new-privileges --read-only
    --tmpfs /tmp --cpus 2 --memory 6g --memory-swap 6g --pids-limit 256`.
  - The model is bind-mounted read-only after its sha256 is checked against
    `tools/models.json`, with the full image digest.
  - Arguments: `-ngl 0 --threads 2 --ctx-size 4096 --parallel 1 --jinja`.
- A pinned minimal TCP forwarder container sits on the internal network plus a bridge
  published only on `127.0.0.1:8091`. Internal networks cannot publish ports. The forwarder
  image is chosen and pinned in step 2b.
- `--cpus 2 --threads 2` at all hours meets the schedule rule.
- Mimik takes `--assistant-endpoint http://127.0.0.1:8091`. A non-loopback host is refused at
  startup. **Assistant off** is the default and a legitimate outcome everywhere.

**Model.** POC screen of `qwen3-4b-2507` and `granite42-3b` (§7). D-D removes the quality bar,
so the screen picks on latency and schema validity first. `minicpm5-2b` is the fallback if
latency fails.

**Request context (D-F, D-J; M13).** Built only by the server, as JSON data under a static
system prompt, and bounded to 4 KiB:
- the phrase (≤ 200 characters);
- the target: address, label, help, control, current value (or unknown), and the pool flag;
- other **integer and enum fields only**, as `@id`, label and value, at most 32;
- step 3 adds item names and linked catalogue entry text (§3c).

Every dynamic string is marked untrusted data. That includes labels and help (owner-authored)
and item names (player-authored, possibly another player's when a Master asks).

**Output (M13, Sonnet M5).** The JSON schema is strict, and the expression has a `pattern`:
```json
{"status":"proposal|ambiguous|unreadable",
 "options":[{"expression":"@hp - 15","rounding":"none|floor|ceil","reason":"…"}],
 "note":"…"}
```
- Expression grammar for the model: terms are integer literals or `@field` references; the
  operators are `+ − × ÷`; parentheses are allowed, nested at most 4 deep; there are at most
  16 terms.
- Rust evaluates it in i128, with `×` and `÷` before `+` and `−`. **Each `÷` is rounded at
  once** by the option's mode: `floor` rounds toward −∞ and `ceil` toward +∞; `none` requires
  exact division. Division by zero or a result outside i64 → the option is dropped.
- **Literal provenance:** every literal must appear as a number in the phrase, or equal the
  target's current value. Otherwise the option is dropped ("84 + 915" from "15 damage" is
  rejected).
- References must name context fields; Rust reads their saved values.
- The display shows **base, expression with labels, and effect**: "Max HP (20) − 5 = 15 · HP
  84 → 15". `reason` is shown as "the assistant's reading", ≤ 120 characters, with control and
  bidi characters stripped, inside `<bdi>`, via `textContent`.
- **The guarantee is narrow and stated in the UI docs:** Rust computes the proposed expression
  exactly, and the player confirms the interpretation.
- 0–3 options remain after validation; none → `unreadable`.

**Jobs (Sol M4, M5; Sonnet M7).**
- **Admission:** `POST /private-sheets/{id}/interpret` (CSRF and edit rights, checked under
  `JOBS`, then released) answers 202 with a 128-bit random job ID from `getrandom`.
- **Binding:** a job is bound to account, sheet, pin hash, base revision, address and the
  phrase.
- **One worker task and one queue.**
  - Queue capacity: 4 in total and 1 per account. A full queue answers 429 "Assistant busy;
    your text is kept".
  - **Supersession:** a new job for the same account and address replaces a queued one, and
    cancels a running one by dropping its request future (cancellation is verified in step
    2b).
  - A queued job older than 60 s fails as "busy". The 30 s run deadline starts when the model
    call starts.
  - The worker holds **no** DB connection or transaction, no `JOBS` permit and no request
    permit.
- **Records:** at most 64 records in any state, each ≤ 4 KiB. A terminal record lives 10
  minutes. The store is swept on every insert, oldest terminal first. A restart drops
  everything.
- **Poll:** `GET …/interpret/{job}` returns immediately with `queued`, `running`, `done` or
  `failed`. It rechecks the session and current read rights each time with one bounded DB
  read. A job of another account, or one the account can no longer read, answers like a
  missing job. The client polls every 1 s, backs off to 3 s, and gives up after 60 s.
- **Apply (D-P, B2):** `POST /private-sheets/{id}/proposals/{job}/{n}` with `csrf` and
  `revision`.
  - In one `IMMEDIATE` transaction: recheck edit rights and the frozen state; the job binding
    must match; the address must be unchanged since the job's base revision (the field-scoped
    rule); then re-evaluate the option over the **current** saved values and write.
  - Any mismatch, expiry or restart → "The sheet changed / this proposal expired: ask again",
    with nothing written. A proposal **never falls back to a literal**. The page has no hidden
    proposal fields, so a stale token can't ride along with a save.
- **Failure messages** keep the text and the value: assistant off, unavailable, too slow,
  couldn't read, expired, busy. Nothing is labelled as checked when it was not.
- Player text and prompts are not logged by default; logs carry the job outcome and timings
  only.

### 3c. Confirm, bundles, reminders

**Interpretation flow (JavaScript, D-G).**
1. Enter on an integer cell does a field save.
2. If the server records `unresolved` and the assistant is on, the module starts a job for
   that note and shows "Thinking…" in a `role="status"` region.
3. Then it shows 1–3 option buttons (base, expression, effect) plus "Keep as written".
4. `ambiguous` always shows choices, never a default. `unreadable` shows a warning.
5. Use → the apply route. Keep → `kept`.

Without JavaScript, the scribble and "Keep" are shown; the assistant is absent.

**Bundles (D-I item 1, step 3).**
- An option may carry up to **4 changes**. Each targets an integer, enum or boolean address,
  either a scalar or a property of a **saved** row by host ID.
- Integer changes use expressions; enum changes name an option ID; boolean changes are
  true/false. Rows are never added or removed.
- They are applied atomically as one revision. The notes share a `bundle` number, and the
  display lists every change.
- Undo on any member offers "Undo these 3 changes" (§3f).

**Reminders (D-I items 2–3, D-J; step 3).**
- **Trigger:** a saved **decrease of a pool field**, from any origin, when the assistant is on
  and the page has JavaScript.
- The server queues a low-priority `reminder` job, which is superseded by any interpretation
  job.
- **Context:** the decrease, the pool label, other numbers and enums, and the **names of list
  rows** whose text property is catalogue-backed (§3e). Each name comes with its linked entry
  text, **when exactly one eligible entry matches** (D-O), cut to 2,000 characters, quoted as
  untrusted data.
- **Output:** `{relevant, row, hint ≤ 120, option?}`, where the option is a bundle.
- **Display:** a dismissible hint, "You took 15 damage. Did you consider your Shield? [Show
  option] [Dismiss]". Dismissal is client-side only. It never blocks and never auto-applies.
- **Injection:** catalogue text is the highest-risk input. It is contained by the strict
  output schema, literal provenance (§3b; numbers may also come from the quoted entry text,
  which is labelled as such in the display), the full display of every change, and the
  player's confirmation. The eval has hidden-instruction cases (§7).

### 3d. Warnings

| Warning | Source | Stored | Shown |
|---|---|---|---|
| Not understood / erased (value kept or unknown) | `unresolved` or `erased` note | yes (sealed) | next to the cell, until a later note for the address |
| Pool sign asks first | field re-render | no | inline choices |
| Assistant failure, expiry, busy | job | no | inline chip (JS) |
| Proposal literal or base issue | validation | no | the option is dropped, or warned |
| Not in catalogue / which one | §3e on view | no | next to the row cell |
| Advisory check | existing formulas (`web.rs:732-748`) | no | existing section |

Warnings never block. They are tied to the control with `aria-describedby`. A one-line count
at the top links to them. Separate dismissal records are cut from the POC.

### 3e. Catalogue names, suggestions, "did you mean" (step 3 member, step 4 UI)

**Member.** A new optional `"catalogue": {"id": <link id>, "entry_type": <id>?}` on **text**
item properties. It is closed and omitted when absent.
- Checked at the three step-3 sites (draft save, publication, readiness): the link is among
  the definition's `catalogues`, and if `entry_type` is given it exists in the latest sealed
  version.
- It is resolved **only through the validated immutable pin**. The browser never supplies a
  catalogue or version ID (M8).

**Stored value (D-B).** Plain text.

**Linking on view (D-O, Sol M10).**
- The value is normalised: NFKC, case-folded, diacritics removed, whitespace collapsed.
- It is matched exactly against eligible entries (the linked catalogue, the type filter
  applied, retired entries excluded).
- **Exactly one** match → a link to its entry page, with the live badge.
- **Several** matches → "Which Sword?" with each candidate (name · type · catalogue) as a link.
  Nothing is stored (Q2).
- **None** → "Not in the catalogue" plus Browse. With JavaScript, also "Did you mean…?" on
  demand (D-O, U8).

**Name cache.**
- Keyed by `version_id`. It holds normalised name, name, key and type, for **names only**;
  badges are read live (M10).
- A global LRU by **actual retained bytes**, capped at 32 MiB, with a single fill per version.
  Fills run outside the DB mutex from one bounded read.

**Suggestions while typing** (step 4, JavaScript only, D-G).
- **Route:** `GET /private-sheets/{id}/suggest?a=<address>&q=…`.
  - It requires read rights, rechecked per request, plus the pin/resource correspondence of
    the step-3 boundary.
  - It does not take `JOBS`. It uses its own pool of 4 and answers 429 at saturation.
- **Query:** 2–64 scalars and ≤ 256 bytes; anything else answers 400 (M9).
- **Matching:** at most 10 results, each with name, type and catalogue label, and the entry
  URL.
  - Prefix match on the cached normalised names. This covers 2-character queries, which FTS
    `prefix 3` would not.
  - Then FTS words through `fts_query` with `Budget`.
  - Then, only if fewer than 3 results, a **typo pass** using `strsim` Damerau-Levenshtein:
    - distance ≤ 1 for 4–6 characters, ≤ 2 for longer;
    - only over names within ±2 characters of the query length;
    - capped at 20,000 comparisons and 20 ms, outside the mutex.
- **The type filter applies to every path** (M8). Results are memoised per (version, filter,
  query) in a small LRU.
- **Target:** p95 ≤ 50 ms at 6,144 entries × 8 catalogues, measured.

### 3f. Undo (D-N) and history (D-E)

**Eligibility.** Undo is offered on the **current note** of an address when all hold:
- its kind is `literal`, `arithmetic`, `choice`, `confirmed` or `reverted`;
- its revision R > 1;
- the address existed at R − 1 (not a new row and not a newly present property);
- the sheet is active.

Not eligible: `unresolved`, `erased` and `kept` (they offer "Keep" or editing instead),
creation, new rows, and frozen sheets.

**Operation.** `POST /private-sheets/{id}/undo` with `csrf`, `revision` (the page's), `note`
(R and ordinal) and `force` (absent or 1). In one `IMMEDIATE` transaction:
1. Recheck rights.
2. Load L, R and R − 1 through the validated readers.
3. Require that the note is still the current note of the address and the value at L equals
   the value at R.
4. Write L + 1 with the R − 1 value, and a `reverted` note with `target = R`.

- If the address changed since R, nothing is written. The page shows "HP is now 50 (changed
  after this note). Set it back to 84 anyway?" as a fresh `force=1` POST that shows the
  intervening value.
- A double submit → 409 or "already undone".
- **A bundle** reverts every member whose current note is that bundle. With `force` absent,
  any changed member stops the whole undo and lists the members.
- Undo of an undo is allowed: `reverted` is eligible.
- Rows are addressed by host ID. Removed rows stay out of scope (host IDs never return,
  `snapshots.rs:67-70`).

**History (D-E).** The history page lists each revision's verified notes, with raw text and
values, next to the unchanged values-free diff. Readers are the sheet's readers: the custodian
and Masters.

### 3g. JavaScript (D-G)

**Module.** One first-party module, `assets/sheet-assist.js`: vanilla, embedded, served by
the auth router to signed-in users, and loaded with `<script type="module">` on sheet pages
only. There is no inline script or handler.

**CSP (M11).**
- Sheet responses carry a `SheetPage` response extension. `normalize`
  (`auth/mod.rs:429-450`) then sets: `default-src 'self'; script-src 'self'; style-src 'self';
  object-src 'none'; base-uri 'none'; form-action 'self'; frame-ancestors 'none';
  require-trusted-types-for 'script'; trusted-types 'none'`.
- Every other response keeps today's header.
- The inline style moves to `app.css`.
- Tests check the **final** connected-router headers on 200, 409 and 422 sheet responses, and
  that the browser console has no CSP violation.
- `script-src 'self'` is the real protection. Trusted Types is Chromium-only defence in depth.

**Module behaviour.**
- **Field save:** it intercepts form submits and posts with `fetch`. It replaces only that
  field's fragment and updates every form's `revision` from the response, which is safe under
  the field-scoped conflict rule. Unsaved text elsewhere is kept.
- It builds DOM only through `textContent` and `createElement`. URLs come from the server.
- **Integers as strings (M12):** every sheet integer, result and revision crosses JSON as a
  canonical decimal **string**, and never passes through JS `Number`.
- **Combobox:**
  - `role="combobox"`, `aria-autocomplete="list"`, `aria-expanded` and `aria-controls`
    pointing at a `listbox` with unique option IDs, and `aria-activedescendant`;
  - Arrows, Escape and Enter. Enter selects when the list is open and does not submit the
    field;
  - while `compositionstart`…`compositionend` is active, no fetch and no Enter handling;
  - a 150 ms debounce and `AbortController`, plus a generation counter that drops late
    responses.
- **Live region** for "Thinking…" and results. Option buttons are real buttons, and focus
  returns to the field.
- **Mobile:** listbox within the page width, targets of at least 24 × 24 CSS px, no
  hover-only behaviour, checked at 390 × 844.
- **Without JS** the per-field forms, arithmetic, pool choices, notes, Keep and Undo all work.
  The Chromium gate keeps a no-JS run.

### 3h. Security summary

- **Model output is untrusted.** It passes a strict schema, the expression grammar, literal
  provenance and Rust evaluation. It is rendered escaped inside `<bdi>`. It reaches a write
  only through the apply route, with fresh binding checks.
- **Prompt context** is limited by D-F and D-J. All dynamic strings are untrusted data, and
  injection cases are in §7.
- **Auth.**
  - Field, list, undo, interpret and apply need edit rights and CSRF; poll and suggest need
    read rights.
  - Rights are rechecked per request, and again inside every write transaction.
  - The post-origin check applies.
- **Limits.**
  - Raw text ≤ 200; notes ≤ 256 and 128 KiB per revision.
  - Prompt ≤ 4 KiB, `max_tokens` 128.
  - Queue 4 and 1 per account, records ≤ 64, the suggest pool 4.
  - Query bounds as in §3e, the name cache ≤ 32 MiB.
- **Secrets and network.** No secrets: a loopback endpoint and no key. The model server has no
  egress, and the client refuses redirects and proxies. Prompts are not logged.

## 4. What changes versus existing rules and plans

**Overturned or relaxed.**

| Existing | Where | Change |
|---|---|---|
| Q4: item not in the catalogue is invalid | `catalogue-plan.md:29` | Warning with suggestions (U8, D-O) |
| D9/R9 writer membership check, 422 | `catalogue-plan.md:295-307`, `:624-636` | Not built. `catalogue_ref` (step 4) stays shelved and its D8 gate stays closed |
| "No JavaScript on sheets"; htmx denied in auth mode | `catalogue-plan.md:102-103`, `auth/mod.rs:191-193` | One module under a strict CSP set in `normalize`. No-JS editing kept (D-G). htmx stays denied |
| Chromium gate asserts "no script" | `release-scope.md:139` | "Only `sheet-assist.js`, no inline script, strict CSP", plus a no-JS run |
| Full-page PRG save with one Save button; no live recompute | `sheet-presentation.md:15-16` | Per-field forms saved on Enter (D-C); a list is one field. The create page stays one form. Derived values are still computed from saved data |
| Strict integer text; one bad integer refuses the whole save | `values.rs:164-174`, `web.rs:1210-1230` | Lenient grammar (§3a); an unreadable cell keeps its value or becomes unknown with a scribble (D-M). Remaining refusals: keys, CSRF, rights, address conflict, size limits |
| Integer cells are never null unless nullable; `null` never inside items | `values.rs:94`, `:116`, `convert.rs:111-118` | `null` = unknown is admitted for integer cells, under the writer provenance rule. Formulas reading it fail per field |
| Stale-page save → 409 for the whole sheet | `web.rs:1124-1148` | 409 only when the target address changed (field-scoped). The legacy full form keeps whole-sheet 409 |
| History summary values-free | `history_diff.rs:1-5`, catalogue D9 | The diff stays values-free. Verified change notes with values are shown next to it (D-E) |
| Catalogue plan non-goals "any LLM", "revert", "legality warnings" | `catalogue-plan.md:36-47` | LLM interpretation, reminders and Undo are in scope. Legality stays out |
| "The LLM's main job moves to authoring…, not judging at play time" | `definition-language-draft.md:43-48` | Play-time use returns, narrowly: translating text into confirmable proposals and reminders. Still no judging. Rules authored and reviewed later make reminders model-free (D-I) |
| "No model … proposes" | `release-scope.md:149` | A local model proposes, confirm-only. The release scope is updated in step 5 |
| Accuracy bar gates model product work | `discovery.md:177-179` | Waived for the POC (D-D); revisit before a real release |
| Single `JOBS` permit for every sheet route | `definitions/mod.rs:29` | Kept for page and write routes. The model worker and suggest have their own pools and never hold `JOBS` |
| Definition contract: no pool or catalogue name members | `runtime-definition-contract.md` | New optional members `entry: "pool"` and the item-property `catalogue` (omitted when absent, so old hashes are kept) |

**What stays.**
- Model output never silently alters a sheet, and legality is advisory.
- Rust does all arithmetic.
- `snapshots::write` is the sole writer; history is append-only, and rollback is a new
  revision.
- CSRF and authorization, immutable pins, size limits, manuals kept local, runtimes in
  containers, no hosted fallback, and the app fully usable with the assistant off.
- The 2026-10-18 decisions (warn, never block; context-driven advancement).

## 5. Steps

Each step is one branch with failing-first tests, and is independently reviewed before merge.
Weekday builds and tests use 2 cores.

### Step 1a: Per-field save, fast path, pools, unknown, sealed notes, demo (no JS, no model)

**Visible result:** in the demo, Enter on "-15" in HP shows "84 − 15 = 69". Enter on "-90"
asks "Subtract → −6 / Set to −90". Typing "lots" in a new row's Quantity keeps the row, shows
"?" with the scribble, and the Load formula says "Not computed: Quantity is unknown".

**Work.**
- Field and list routes with their exact key sets.
- The field-scoped conflict rule.
- The pool member (schema, decode, domain check, contract doc).
- The grammar module, the unchanged-cell rule, and the pool choices.
- The D-M unknown, the writer provenance rule and the formula null mapping.
- Migration 020: notes, seals, triggers. The notes and seal written in the save transaction,
  verification on read, and current notes on the sheet.
- Legacy full form and create through the same interpreter.

**Demo (Sol M14).**
- `tests/support/source_fixture.rs` gains `assist_definition()`, written as
  `assist-definition.json`: game `synthetic-game/first`, definition `assist-sheet`. Fields:
  - `hp` "Current HP" (integer, pool);
  - `max_hp` "Max HP";
  - `armour` "Armour modifier" (integer; start −1);
  - `luck` "Luck" (nullable integer);
  - `equipment`, a list of `name` (text), `quantity` (integer, pool), `weight` (integer),
    `broken` (boolean);
  - derived `load` = `sum(equipment.map(i, i.quantity * i.weight))`;
  - check `hp <= max_hp`.
- `tools/assist_demo.py` is a dev tool, prototype tier, reusing the `smoke_release.py`
  helpers:
  - it creates a fresh data directory;
  - registers the ruleset, imports the source fixture, imports, compares, approves and
    publishes the definition;
  - bootstraps an owner and a player with **generated synthetic passwords** (printed: they
    are dummy local credentials, not secrets);
  - creates a campaign and a sheet with HP 84 and Max HP 84;
  - serves `--loopback` and prints the URL and logins;
  - stops on Ctrl-C.
- Open instructions:
  ```
  cargo build --locked -j 2
  cargo run --locked -j 2 --example source_fixture -- target/assist-demo/fixture
  python3 tools/assist_demo.py target/debug/mimik --fixture target/assist-demo/fixture \
      --data target/assist-demo/data
  # open the printed http://127.0.0.1:<port>/private-sheets/<id>, log in as the printed player
  ```

**Failing-first tests.**
- **B1:**
  - a saved −1 armour plus a save of another field leaves −1;
  - the legacy full form re-posting −3 leaves −3, including list rows after Move and Remove;
  - "-1" on non-pool `armour` sets −1;
  - "-15" on pool `hp` gives 69;
  - retyping the saved string is "No change".
- **M1, grammar table:**
  - "84 -15" = 69, "84-15+3" = 72, "007" = 7;
  - "84--15", "84+-2", "5." and full-width digits → unresolved;
  - `i64::MIN` literal; overflow → unresolved;
  - over 64 scalars → unresolved;
  - a property test that the parser never panics.
- **D-L:** "-90" on HP 84 → choices and no write; each choice writes its value with a
  `choice` note; a saved negative base asks; "Set to −15 instead" writes −15.
- **D-M and M2:**
  - "lots" on a new row → row kept with `null` and an `unresolved` note;
  - a newly present optional property, the same;
  - create with "lots" → created with unknown;
  - forged `null` without a note → refused;
  - carried `null` accepted;
  - formulas: the reader fails per field and the others compute;
  - blank → `erased`.
- **B3:**
  - a late note insert after commit → abort;
  - an insert after a seal → abort;
  - an insert for a past revision → abort;
  - replacement, update and delete → abort;
  - a digest or count mismatch → "Note unavailable";
  - forged origin or result mismatch (the arithmetic re-evaluation fails) → unavailable;
  - a note failure after `snapshots::write` rolls back the live row and history (test hook);
  - 257 notes → 422.
- **M3:** a literal after `unresolved` resolves it; `kept` writes unchanged data; freeze writes
  no notes and keeps the current notes readable.
- **Conflicts:** two fields from one page revision both save; the same field from a stale page
  → 409 with the input re-rendered.
- **Minor (formulas):** an arithmetic result feeds formulas, a scribble does not, a derived
  field can't be targeted, and historical formulas use historical values.

**Browser.** Chromium at 1280 and 390, no JS: Enter saves a single field, pool choices,
notes, no overflow with notes on every cell.

### Step 1b: Undo, Keep, history notes

**Visible result:** "84 − 15 = 69 · Undo" restores 84. The history page shows the notes.

**Work:** the undo route, Keep, bundle-ready undo (bundles arrive in step 3), and history
rendering.

**Failing-first tests (M7, D-N):**
- undo of `arithmetic`, `literal` and `choice`;
- not offered on create, new row, `unresolved` or frozen;
- changed since → no write and an "anyway" offer; `force=1` writes;
- double submit → 409;
- undo of an undo;
- rights revoked between page and POST → 403;
- the history page shows raw text to a Master (D-E).

### Step 2a: JavaScript foundation (no model)

**Visible result:** Enter saves without a page reload, and other typed text survives.

**Work:** the `SheetPage` CSP in `normalize`, the inline style moved, the asset route, the
module with fetch saves, string integers, and fragment responses.

**Failing-first tests (M11, M12):**
- final headers on 200, 409 and 422;
- other routes unchanged;
- the asset is served only to signed-in users;
- JSON integers are strings (a value of 9007199254740993 round-trips);
- Chromium: no CSP violation, keyboard flow, the live region, 390 px, and a no-JS run still
  green.

### Step 2b: Crate and runtime selection (prototype tier; may run in parallel with 1a)

- The crate comparison and contract tests (§3b, items 1–6). **D-H stop condition.**
- The container topology with the forwarder.
- A latency and schema screen (§7).
- Results note in `docs/`, then review before the result picks the crate and model.

### Step 2c: Interpretation service

**Visible result:** "took 15 fire damage" → "Thinking…" → "HP 84 → 69 (84 − 15) [Use] · Keep
as written".

**Work:** trait, stub and adapter; the endpoint config with loopback refusal; worker, queue,
records and polling; apply route; module UI.

**Failing-first tests (stub and axum fake server):**
- schema-invalid reply → unreadable;
- expression outside the grammar → unreadable;
- a literal not in the phrase is dropped (`84 + 915`);
- a reference to a text field is refused (D-F);
- `÷` floor and ceil on negatives;
- division by zero;
- 5th job → 429; 2nd job per account → 429;
- supersession cancels (the fake server sees the disconnect);
- a queued job past 60 s fails; a run past 30 s fails;
- records capped at 64 with sweep;
- **B2/D-P:**
  - apply after the address changed → "ask again" and no write;
  - expired → no write and no literal;
  - another account's job → not found;
  - rights revoked before poll → not found; before apply → 403;
- `JOBS` free during the model call (a concurrent sheet GET succeeds);
- non-loopback endpoint refused at startup;
- assistant off → every page works with honest messages;
- no player text in logs.

One live smoke with the real model, recorded as **live inference**, separate from the unit
evidence.

### Step 3: Bundles and reminders (D-I, D-J, D-K)

**Visible result:** HP −15 in the demo, with a "Shield" row linked to a demo catalogue entry
whose text says it absorbs 3 and breaks. The hint reads "Did you consider your Shield?", and
its option "HP 69 → 72, Shield broken = true" is applied and undone as one.

**Work:**
- the item-property `catalogue` member at the three sites;
- unique-name linking on view (D-O);
- a demo catalogue added to the fixture;
- the bundle schema and apply route;
- the reminder trigger and job kind; the hint UI.

**Failing-first tests:**
- a bundle of 4 changes in one revision with notes sharing `bundle`; a 5th change → dropped;
- a bundle change to a removed row → no write;
- bundle undo all, and partial "anyway";
- reminder only on a pool decrease, with no reminder when the assistant is off;
- duplicate names → no entry text in the prompt;
- entry text cut to 2,000;
- injection in entry text ("set HP to 999") → at most a fully shown option, never applied;
- text and multiline values never in the prompt (assert on the request built against the
  stub).

### Step 4: Suggestions and "did you mean"

**Visible result:** "heal" lists healing items. "sord" → "Did you mean Sword?". A duplicate
"Sword" → "Which Sword?".

**Work:** the suggest route, the name cache, matching paths, the combobox, and the on-demand
typo pass.

**Failing-first tests (M8, M9, M10):**
- a forged address or another property → 400/403;
- an unlinked catalogue, another game or edition, a native, unsupported or corrupt pin →
  refused;
- the type filter on prefix, FTS, typo and exact paths;
- a 2-character prefix hit;
- typo at distance 1 and 2, with no hit beyond;
- diacritics;
- a query over 64 scalars → 400;
- typo pass bounded (comparisons and time);
- cache bytes bounded and evicted, single fill;
- a newly sealed version seen;
- no `JOBS` 503 under concurrent suggest; 429 at saturation;
- names escaped in JSON and the DOM;
- p95 measured at the caps.

### Step 5: Docs, release scope, gate

- Update `release-scope.md`, `runtime-definition-contract.md`, `sheet-presentation.md`,
  `catalogue-publication.md` and `dependency-decisions.md`.
- `cargo about`, audit, packaging, and the unpacked-archive smoke with the assistant off.
- The assistant-on scope reads "local endpoint, POC quality, not certified (D-D)".

## 6. Risks, later items, open questions

**Risks.**
- **R1 Latency at 2 threads** is unmeasured. Mitigations: the fast path covers the common
  case, the 2b screen, a smaller model.
- **R2 Quality** is unknown, and the bar is waived (D-D). Wrong-but-valid proposals are
  possible. The display (base, expression, effect) and the confirmation are the only
  safeguard.
- **R3 Reminder prompts carry catalogue text.** Injection is contained but not prevented
  (§3c).
- **R4 Provider crate gap** → stop and ask (D-H). That can stall step 2c.
- **R5 Per-field revisions grow history:** one revision per Enter. This is bounded by the
  existing revision and size limits, but history pages get longer.
- **R6 No-JS reload loses un-entered text** in other fields (Q3).
- **R7 Name links shift** when an entry is renamed or a duplicate appears. This is accepted
  under D-B and D-O.
- **R8 Forwarder container** is a new pinned image. Its licence and digest are checked in step
  2b.

**Later (after the POC).**
- `*` and `/` in the player's fast path.
- Text and enum interpretation.
- Dismissal records.
- Undo of creation, new rows and removals; arbitrary per-cell restore.
- Rules authored by the LLM and reviewed by the owner, for reminders with the model off
  (D-I item 3).
- More threads outside weekday hours.
- The quality bar (D-D).
- Retiring the legacy full-form edit POST.

**Decided** (rev 1 Q1–Q6): Q1 → D-A, D-L; Q2 → D-B; Q3 → D-C; Q4 → current-note lifecycle
(§3a, M3); Q5 → Gemma excluded from the POC screen; Q6 → D-D.

**Open questions for the user** (product choices only).
1. **Q1, D-L confirmation:** a relative pool input applies at once when both the old and new
   values are ≥ 0, with "Set to −15 instead" offered afterwards. It **asks first** when the
   result would be negative or the saved value already is. Is that the right threshold?
2. **Q2, duplicate names with plain text:** "Which Sword?" is shown on every view, because
   nothing is stored. Should picking one store the entry's key alongside the name, which is a
   small departure from D-B?
3. **Q3, per-field granularity:**
   - A list saves as a whole when Enter is pressed in any of its cells.
   - Without JavaScript, text typed in another field and not entered is lost when a field
     saves (the page reloads).

   Are both acceptable?

## 7. Verification

**Rust gate** (every step):
- fmt, strict Clippy (`all` + `pedantic`), and locked tests including every failing-first test
  above (red run recorded, then green);
- `cargo audit`, `cargo about`, release build, binary-size delta;
- 2 cores on weekdays.

**Browser** (Chromium at 1280 × 720 and 390 × 844, synthetic demo data):
- per-field Enter with and without JS;
- pool choices; notes; Keep; Undo and "anyway"; history notes;
- CSP headers and no violations;
- combobox keyboard and IME;
- live region; no overflow.

**Process smoke:** the unpacked archive with the assistant off, and once with the stub
endpoint.

**Model screen** (prototype tier, `tools/assist_eval.py` with its own unit tests; a results
note with exact scope, denominators and limitations; reviewed before it picks the crate and
model; **no pass bar**, D-D).
- Measured on 2 threads over 3 seeds:
  - p50/p95 latency with a cached static prefix;
  - schema-valid rate;
  - per class: correct option within the first 3, and **false-confident** (a wrong value as the
    only option).
- In-scope classes, each with its own denominator:

  | Class | n |
  |---|---|
  | Damage and heal phrases (EN/IT) | 16 |
  | Fractions and vague ("about half", "metà") | 8 |
  | Other-field references ("back to max") | 6 |
  | Nonsense | 8 |
  | Injection in the **player phrase** (an actual request to set 999 is legitimate, and the expected result is a shown option) | 4 |
  | Injection **hidden in context** (labels, item names, entry text) | 8 |
  | Reminder relevance (shield cases vs unrelated items), step 3 | 8 |

- Deterministic fast-path inputs are excluded; they never reach the model.
- Reported by class. A small synthetic set is not a measure of general accuracy.
- Unit, connected-router, browser and live-inference evidence are reported separately, and a
  check that was not run is never reported as passed.

## 8. Review finding map (revision 1 → revision 2)

| Finding | Resolution | Section |
|---|---|---|
| Sol B1, Sonnet B1: relative sign corrupts re-posted negatives | Relative only on pools (D-A); unchanged-cell rule on every path; per-field save (D-C) submits one field; proposals applied by a dedicated route, never re-parsed | §3a, §3b |
| Sol B2: stale or expired proposal becomes a literal | No proposal keys in forms; apply route with full binding and the address-unchanged check; no literal fallback (D-P) | §3b |
| Sol B3: notes table lacks integrity | Sealed note sets (count and digest), live-revision-only insert triggers, no insert after the seal, atomic with the writer, read-time verification against history | §3a |
| Sol M1, Sonnet M1: grammar under-specified | Formal grammar, i128 evaluation, rounding modes for model `÷`; `*` and `/` cut from the player path; blank and nullable defined | §3a, §3b |
| Sol M2: unresolved on new rows still blocks | D-M unknown `null` with the writer provenance rule; create included; formulas fail per field | §3a |
| Sol M3, Sonnet M3: note lifecycle and bounds | Every value change writes a note; current note = latest; `kept`; 256 notes and 128 KiB per revision; dismissal cut; freeze defined | §3a |
| Sol M4, Sonnet M7: job store bounds | One worker; queue 4 and 1 per account; 64 records; TTLs; deadlines; supersession with verified cancel; no permits or DB held | §3b |
| Sol M5: auth on poll and use | Rights rechecked per poll and inside the apply transaction; revocation tests | §3b, step 2c |
| Sol M6, Sonnet M2: ops discard drafts; key whitelists | Dedicated per-field, list, undo and apply routes with exact key sets; JS fetch keeps other text; no-JS loss documented (Q3) | §3a, §3g |
| Sol M7: Undo semantics | Eligibility, before states, current-note check, `force` re-confirmation, bundles, double submit | §3f |
| Sol M8, Sonnet M9: suggestion confinement | Hint resolved only through the pin; type filter on every path; rights rechecked; no client IDs | §3e |
| Sol M9, Sonnet M9: suggestion budget | Query bounds, prefix cache for 2 characters, typo pass on demand with comparison and time caps outside the mutex, byte-bounded LRU | §3e |
| Sol M10: duplicate names | Link only on a unique match; "Which Sword?" otherwise (D-O); badges live, not cached | §3e |
| Sol M11, Sonnet M8: CSP overwritten; inline style | CSP set in `normalize` through the `SheetPage` extension; style moved; final-header tests | §3g |
| Sol M12: i64 lost in JS | Integers as decimal strings | §3g |
| Sol M13, Sonnet M5, M6: injection and "never does the math" | `@field` references; literal provenance; display of base, expression and effect; narrow guarantee; all dynamic context untrusted; D-F and D-J context | §3b, §3c |
| Sol M14: no usable demo | `assist_definition()` plus `tools/assist_demo.py` with exact commands | step 1a |
| Sol (container), Sonnet M10: runtime isolation, crate | Internal network plus pinned forwarder, swap and PID limits; crate contract items 1–6; D-H stop | §3b, step 2b |
| Sol Q6 gate | Waived for the POC (D-D); screen has no bar; denominators by class | §7 |
| Sol minor (formulas) | Formula interaction tests | step 1a |
| Sonnet m1–m7 | One 200-character limit and 64-scalar fast path (m1); sha256 check before mount (m2); undo by host ID and "anyway" wording (m3); double-click 409 (m4); "confirmed" wording (m5); injection in labels and other fields (m6); assistant off as an outcome (m7) | §3a, §3b, §3f, §7 |
| Sonnet n2: split step 1 | Steps 1a and 1b | §5 |
| Sonnet cut list | Cut: preview, dismissal, hidden proposal fields, player `*` and `/`, no-JS ask cycle (D-G), keyed limiter. Kept: suggestions (user order D-K) and view-time links (D-B, D-O) | §1, §5 |
