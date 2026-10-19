# Assisted sheet MVP plan: lenient input, CPU interpretation, suggestions, undo

2026-10-19. **Revision 3.1.** Base: master `0921f6c`. Code is unchanged since `4c8d653`, and
references were re-checked there.

**Rev 3** (`0921f6c`): Sol and Sonnet both **GO WITH AMENDMENTS**, with no Blocking findings;
the third chain was unavailable. Slice 1.1 may start. Rev 3.1 folds in their amendments and
the user decisions D-V and D-W, and records the main-session defaults D-X1 to D-X4. Rev 3.1
changes are amendments requested by the rev-3 reviews; slices touching them get
implementation review. Review record:
[reviews/2026-10-19-assisted-sheet-plan.md](reviews/2026-10-19-assisted-sheet-plan.md).

Review history:
- **Rev 1** (`71a7841`): Sol NO-GO, Sonnet GO WITH AMENDMENTS. Answered by user decisions D-A
  to D-P and by rev 2.
- **Rev 2** (`cfd3475`): Sol **NO-GO**, with Blocking R1–R3, Majors M1–M7 and minors m1–m2.
  Sonnet **GO WITH AMENDMENTS**, with M-A to M-H and m1–m8. The third chain was unavailable.

User decisions D-Q to D-U then simplified the design:
- sign-led input is literal;
- notes are simple;
- each list cell saves on Enter;
- no-op saves write nothing.

Rev 3 drops the machinery those decisions make unnecessary: the pool marker, the pool choices,
seals and read-time verification, and whole-list saves. It resolves R1 and R2 with contracts
and tests, and slices step 1 into six small demo-able parts. Finding map: §8. This revision
needs a re-review before slice 1.1 starts.

## 0. Direction and decisions

"We're improving the paper and pencil paradigm, not replacing it." The player writes what
they want; the app computes, warns, suggests and offers Undo; nothing is forced.

**Binding decisions** (user, 2026-10-19). D-A and D-L are superseded by D-Q.

| # | Decision | Where |
|---|---|---|
| D-B / D-R | A picked name is stored as **plain text**. When the player answers "Which Sword?", the **entry key is stored next to the name**; the cell still shows the text | §3e |
| D-C / D-S | **Each field and each list cell saves on its own on Enter.** Lists keep Add (+), Remove and Move. A **quick-add line** under each list ("3 torches", "healing potion x2") adds a row, with a deterministic parse in the POC | §3a, §3g |
| D-D | The model quality bar is waived for the POC and revisited before release. Engineering gates stay | §7 |
| D-E | Masters see raw scribbles and change notes in history | §3f |
| D-F / D-J | The model reads the phrase, the target, and the sheet's number and enum fields. It may also read list-row item names and their linked catalogue entry text. Never notes, backstory or multiline text | §3b |
| D-G | The assistant and live suggestions need JavaScript. Typing, arithmetic, notes and Undo work without it | §3g |
| D-H | Provider client from an established Rust library only (`genai` or `rig-core`). If none can send the structured-output schema or grammar: **stop and ask** | §3b |
| D-I / D-K | Multi-field **bundles**, applied and undone as one, and dismissible **reminders**, as their own step right after model interpretation. POC knowledge: item names plus linked entry text | §3c, §5 |
| D-M | An unreadable required number (a new row, a newly present property, or a create) is stored as **unknown**, with the scribble and a warning | §3a |
| D-N | Undo covers **the last change of each field only**. Older history is viewable | §3f |
| D-O | Link a name automatically only when exactly one eligible entry matches; otherwise ask "Which Sword?" | §3e |
| D-P | Stale or expired proposals never become a literal; they need reconfirmation | §3b |
| **D-Q** | **Sign-led input** ("-15", "+3") on **any** number field is applied **literally** (HP becomes −15). Right after, the app asks **"Did you mean 84 − 15 = 69?"**, with one click to switch. Multi-term input ("84 -15") is computed directly. No pool marking | §3a |
| **D-T** | Change notes are **simple**: written in the save's transaction, with no updates or deletes. Sealing and verification come later, before release. The tamper window is a recorded POC limitation | §3a, §6 |
| **D-U** | History has no limit. **A save that changes nothing writes no revision** | §3a |
| **D-V** | **Removing a list row happens at once, with Undo right after** (like crossing out on paper). No confirmation | §3a, §3f |
| **D-W** | **Reminder trigger:** any decrease of a top-level number field asks the model whether something on the sheet could react. The model judges relevance; there is no owner marking | §3c |

**Main-session defaults** (recorded as defaults; the user may revise them).

| # | Default | Where |
|---|---|---|
| D-X1 | A blanked (`erased`) value is Undo-eligible (Sonnet rev-3 M1) | §3f |
| D-X2 | D-U's "no history cap" stands for the POC. A per-account write-rate limit or storage guard is **required before release** (risk register, §6) | §6 |
| D-X3 | No-JS 503 on per-cell saves: sheet writes wait a short, bounded time for the `JOBS` permit, then show a styled 503 page that keeps the draft (Sonnet rev-3 M3) | §3a |
| D-X4 | The "Did you mean" offer after sign-led input stays on every number field (the user chose it). It is non-blocking and dismissible | §3a |

## 1. Goal and POC scope

**User value.**
- "84 -15" + Enter → 69, shown as "84 − 15 = 69 · Undo".
- "-15" + Enter → HP −15, then "Did you mean 84 − 15 = 69? [Yes]".
- "lots" in a new row's Quantity keeps the row, shown as "?" with the scribble.
- "3 torches" + Enter under Equipment adds the row.
- "took 15 fire damage" → the assistant offers "HP 84 → 69".
- After damage, "Did you consider your Shield?" offers a bundle.
- "heal" lists healing items; "sord" → "Did you mean Sword?".

**POC scope.** Steps in user order (D-K):
1. Lenient numbers, notes, D-Q switch, Undo, unknown values, per-cell save, quick-add (slices
   1.1–1.6).
2. JS foundation, crate and runtime selection, interpretation.
3. Bundles and reminders.
4. Suggestions and "Which Sword?".
5. Docs and gate.

**Cut from the POC.** None of these carries a security or integrity risk; each is in §6
"Later".
- Player `*` and `/`.
- Pool marking.
- Interpreting text, enum or boolean cells.
- Notes and Undo for text cells (history keeps every text revision).
- Seals and note verification (D-T).
- Dismissal records.
- Undo of creation, row add and moves. Row **removal** gets Undo (D-V, §3f).
- Forced Undo ("set back anyway").
- Live preview.
- A keyed rate limiter.
- Gemma in the screen.
- Changes to the native MÖRK BORG editor (it keeps its strict path, `web.rs:685`).

**Non-goals.** No legality rulings, no automatic application of model output, no chat, no
manual RAG, no `catalogue_ref` membership checks, no hosted provider, no GPU, nothing above
about 4B.

## 2. Facts from the current code (master `0921f6c`)

**Values and save.**
- `values::integer` (`values.rs:164-174`) accepts exact decimal only. `scalar_value`
  (`:541-552`), `values::form` (`:614-656`) and `reconcile` (`:558-612`) refuse anything else.
- One `Invalid` refuses the whole save with 422 (`web.rs:1210-1230`).
- `apply_save` checks the revision first (409, `:1124-1148`). The save does PRG
  (`web.rs:1236`).
- Saved values render as `Value::to_string()` (`web.rs:837-844`), and the full form re-posts
  every field (`private_sheet.html:12`).
- Name and notes go through `name_notes` (`campaigns/mod.rs:74-84`): an empty name is refused.
- Required enum and boolean cells without a value are refused (`values.rs:541-548`).
- Field IDs follow `valid_field_id` (`definitions/admission.rs:96-103`). A definition field may
  be called `name` or `notes`, so it can collide with the envelope's Name and Notes (Sol rev-2
  M1). The current form avoids this with distinct keys (`name` vs `value-name`).

**Null.**
- Integer cells are `{"type":"integer"}` (`values.rs:94`), and "`null` never appears inside
  items" (`:116`).
- `private_pin::list_values` rejects non-i64 item integers, null included
  (`private_pin.rs:97-117`).
- Formula inputs refuse `null` on non-nullable scalars (`convert.rs:111-118`) and inside
  records (`convert.rs:89-97`). Either refusal fails the whole evaluation.
- The existing wording for `Code::NullValue` is "a value it uses is unknown (null)"
  (`formulas.rs:23`). Dependents say "it uses X, which could not be computed"
  (`formulas.rs:187-195`).

**Writer and history.**
- `private_save_keeping` runs authorization, pin checks and `snapshots::write` in one
  `IMMEDIATE` transaction (`sheets.rs:266-306`).
- `snapshots::write` is the sole writer (`snapshots.rs:306-406`). Operations are `create`,
  `edit` and `freeze`; a stale revision gets 409 (`:338`); a removed host ID never returns
  (`:67-70`).
- History is append-only, and its triggers bind each event to the live row
  (`migrations/012_campaigns.sql:95-127`).
- The change summary is values-free (`history_diff.rs:1-5`).

**Concurrency and transport.**
- One SQLite connection behind a mutex (`store.rs:35`, `:107-132`).
- Sheet routes hold the `JOBS` permit of 1, and a busy server answers 503
  (`definitions/mod.rs:29-37`, `campaigns/web.rs:42-56`).
- Auth mode: 8 global permits and a 5 s request timeout (`auth/mod.rs:37-38`, `:205-238`).
- `post_origin` accepts POST bodies as `application/x-www-form-urlencoded` only, and anything
  else gets 415 (`auth/mod.rs:453-478`). So a `fetch(FormData)` multipart body would fail.

**JavaScript and CSP.**
- The sheet loads no script (`private_sheet.html:1`) and has an inline
  `style="min-width:0"` (`:7`).
- The auth router serves only `app.css` and `logo.png` (`auth/mod.rs:191-193`).
- `normalize` sets `frame-ancestors 'none'` on every response (`auth/mod.rs:429-450`).

**Catalogues.** FTS name index (`019_catalogues.sql:111`). `fts_query` bounds pieces, not
bytes (`search.rs:66-121`). Browse confinement (`read.rs:221-310`, `:344`). Names are not
unique (`019:74-89`). Badges are live; sealed versions are immutable.

**Demo assets.**
- `tests/support/source_fixture.rs` writes the synthetic definitions (`:57-95`), run as
  `cargo run --locked --example source_fixture -- <dir>` (`ci.yml:34`).
- `tools/smoke_release.py` drives registry, import, publish, accounts and campaign
  (`:329-395`, `:478-700`).

**Models.** Unchanged from rev 2:
- screen `qwen3-4b-2507` and `granite42-3b`; fallback `minicpm5-2b`;
- pinned llama.cpp image and the hardened runner flags (`run_tool_loop_benchmark.sh:476-487`);
- `json_schema` was exercised in `local_rag_benchmark.py:385-406`;
- no measurement at 2 threads.

## 3. Design

### 3a. Numbers, notes, unknown values, per-cell save

**Grammar.** Applies to integer cells only.
- It lives in a **new module**, `campaigns::entry`. `values::integer` stays strict, because it
  also parses protocol integers such as submitted revisions (`web.rs:1074`, `:1127`); its test
  at `campaigns/tests.rs:484-486` is kept (Sol slice-1.1 constraint, Sonnet m1).
- Input is trimmed of ASCII spaces and tabs.
- NUL and control characters other than tab are refused as unreadable (Sonnet m6).
- More than 200 characters is refused **server-side**, whatever the `maxlength`.
- The arithmetic parser reads at most 64 scalars.

```
int     = DIGIT+                    ; ASCII only; leading zeros allowed ("007" = 7)
sign    = "+" | "-" | "−"           ; U+2212 accepted
literal = [sign] int                ; the literal value, on every field (D-Q)
sum     = int 1*( ws* sign ws* int ); "84 -15" = 69, "84-15+3" = 72; unsigned terms only
```

- Evaluation is left to right in **i128**, and **each term is checked** as it is parsed and
  added. The result must fit i64, otherwise the input is unresolved. `-9223372036854775808`
  parses.
- **Literals:** "+5" = 5, "−5" (U+2212) = −5, "-0" = 0, "0007" = 7. A sign must touch its
  digits, so "- 5" is unresolved.
- **Unresolved:** "84--15", "84+-2", "5.", "1 2", "5 -", a lone sign, full-width digits, words,
  `*` and `/`.
- **Empty or whitespace-only input** (spaces, tabs) = blank.
- **Blank** = unknown (D-M). On nullable fields the existing Known/Unknown select still rules.

**D-Q switch.**
- A sign-led literal on a cell whose saved value is a known integer `b` is applied literally,
  and the note records `b`.
- The sheet then shows "Did you mean 84 − 15 = 69? [Yes]" as a small form posting
  `/private-sheets/{id}/switch` with `note` and the cell's `base`.
- **Freshness:** in the transaction, the note must be the **current note** of its address and
  the cell's current value must equal the note's `after`. It then writes `b ± n` with op
  `switched`. Otherwise nothing is written: "HP changed since; not switched".
- A switch is single-use, because its own write replaces the current note.
- No offer when there was no known before-value (a new row, or unknown).
- **Checked arithmetic** (Sol minor 1, Sonnet m8): `b ± n` uses checked i64 arithmetic. On
  overflow there is no offer.
- **D-U wins:** entering `-1` when the saved value is already −1 is a no-op. There is no
  revision, no note and no offer.
- **D-X4:** the offer is shown on every number field. It is non-blocking, and a client-side
  dismiss hides it.

**Unknown values (D-M; Sol rev-2 M5, Sonnet M-B, M-G).** `null` becomes a legitimate value for
**every integer cell**, as the "internal empty value" D-M asks for. The full path:
- **Value schema:** integer cells generate `["integer","null"]`, list properties included
  (relaxes `values.rs:94`, `:116`). This only accepts more, so old snapshots stay valid.
  - The nullable wrapper (`values.rs:140-150`) must be **idempotent**: adding `null` to a type
    that already admits it must not build a nested type array (Sol M5).
- **Null formula results** (Sol M5): `convert::result` (`convert.rs:158-175`) classifies a CEL
  `Null` result as `TypeMismatch`. Slice 1.4 maps a `Null` result to `Code::NullValue`, so
  `copy_hp = hp` with an unknown `hp` gets "a value it uses is unknown (null)".
- **CEL equality** (Sonnet m9): in CEL `null == 0` evaluates without error, to `false`. Checks
  written with `==`, `has()` or a ternary can therefore *pass or fail* on an unknown instead of
  showing "not evaluated". Slice 1.4 tests these three forms and documents them in a profile
  note. It also updates `runtime-definition-contract.md:279`, which today says `null` is valid
  "only if the approved field is nullable".
- **Domain:** `private_pin::list_values` accepts `null` for integer properties
  (`private_pin.rs:111-116`).
- **Formulas:** `convert::inputs` and `convert::record` map `null` on integer inputs and
  properties to CEL `Null` instead of refusing (`convert.rs:89-97`, `:111-118`). A formula that
  reads it fails per field with the **existing** wording "a value it uses is unknown (null)",
  and its dependents get "it uses Load, which could not be computed". Rev 2's "Quantity is
  unknown" wording is withdrawn (Sonnet M-G). Other formulas still compute.
- **No provenance rule.** Rev 2 accepted `null` only with a scribble; that is dropped. A player
  may already set any value on their own sheet, so an unknown without a scribble carries no
  integrity risk. This also removes the ordering issue in Sonnet M-B: the rule would have run
  inside `snapshots::write` before any notes existed.
- **Edit with a known saved value and an unreadable input:** the value is **kept**, and an
  `unresolved` note holds the scribble. **New row, newly present property, or create:**
  `null` plus the note.
- **Undo** may restore `null`. **Freeze** copies the data unchanged. **History diff:** null
  versus a number counts as a change.
- The sheet shows "?" plus the scribble.
- **Create still refuses** an empty Name and unselected required enum or boolean controls
  (`campaigns/mod.rs:74-84`, `values.rs:541-548`). D-M covers unreadable numbers only, and the
  rev-2 claim "create never refuses on content" is narrowed accordingly (Sol rev-2 question 5).

**Change notes (D-T).** Migration 020 adds one table:

```
private_sheet_notes(sheet_id, revision, address, op, raw, before, after, bundle, target)
  PRIMARY KEY (sheet_id, revision, address)
```

- `address` is typed (M1 below). `op` ∈ `literal`, `sum`, `sign_literal`, `switched`,
  `unresolved`, `erased`, `confirmed`, `reverted`. The `op` itself records which reading was
  applied (Sonnet M-E).
- `raw` is the typed input, ≤ 200 characters.
- `before` and `after` use a small **typed note value** (Sol M4, Sonnet M2), stored as canonical
  JSON of at most 80 bytes:
  - `{"absent":true}` — an optional property not set; absence is distinct from unknown;
  - `{"unknown":true}` — `null`;
  - `{"int":"-15"}` — i64 as a decimal string;
  - `{"enum":"<id>"}` — up to 64 characters, so the 80-byte bound fits the longest admitted ID;
  - `{"bool":true}`;
  - `{"row":"absent"}` / `{"row":"present"}` — the row-level states of a removed row (D-V).

  A whole removed row is not stored in the note; its Undo reads the row from history, §3f.
- `bundle` is a small integer or null. `target` is the revision an Undo reverted.
- **Scope (Sol rev-2 M4):** notes are written for **integer, enum and boolean cells only**.
  Text and multiline edits write none; history already holds every revision. That removes the
  long-text problem.
- A note is written for **every** value change of an in-scope cell, plus `unresolved` when the
  value is kept. So the **current note** of an address is its latest note.
- **Integrity in the POC:**
  - inserted after `snapshots::write`, in the same `IMMEDIATE` transaction;
  - triggers abort UPDATE and DELETE, with replacement guards;
  - an insert must name the live revision of an existing history row;
  - any failure rolls back the whole save.

  **No seal and no read-time verification (D-T):** a process with database write access could
  add notes to the current revision. This is recorded as limitation L1 (§6), to be fixed before
  release by sealing. Rev-2 R3 (the migration window) and Sonnet M-C (verification cost) fall
  away with it.
- **Bounds** (Sonnet m5):
  - a per-cell save writes 1 note; bundles write ≤ 4;
  - a **new-row form** writes at most one note per integer, enum or boolean property of that row
    (≤ 32, the item-property cap);
  - **quick-add** writes none;
  - a row removal writes one `{"row":…}` note;
  - the legacy full form and the create form write at most one note per in-scope cell, capped at
    256 per revision. Above that the save gets 422 as a size limit.
- The display of a scribble (`raw`) is escaped and inside `<bdi>` (Sonnet m6).

**No-op saves (D-U; Sonnet m2).**
- A cell is unchanged when its **parsed** value equals the saved value. The comparison is per
  control:
  - integer: the parsed i64 (or unknown);
  - text and multiline: exact string;
  - Name: after trim, as `name_notes` stores it;
  - Notes: exact;
  - enum: option ID;
  - boolean: the value;
  - nullable pair: state plus value;
  - optional property: presence plus value.
- A save in which nothing changes writes **no revision and no note**. The page says "No
  change". Rights and the active state are **still rechecked** before that answer (Sol minor 2).
- **Two record-only changes** write a revision with unchanged data and a note. They are the
  only data-unchanged revisions:
  1. An `unresolved` scribble on a kept value.
  2. **Accepting the kept value after a scribble** (Sol M4): when the address's current note is
     `unresolved` and the player enters a value equal to the saved one, a `literal` note
     resolves the scribble. Otherwise "enter 84 to clear it" would be a no-op and the scribble
     would never go away.
- There is no separate "Keep" button (Sonnet m7).

**Typed addresses (Sol rev-2 M1).**
- Forms: `meta:name`, `meta:notes`, `field:<id>`, `cell:<list>/<host-id>/<property>`.
- In routes: `/private-sheets/{id}/meta/{name|notes}`, `/fields/{field_id}` and
  `/cells/{list}/{host_id}/{property}`.
- Notes, conflicts, jobs, Undo and switch all use them, so a definition field called `name` or
  `notes` never collides with the metadata.
- The data part reuses `history_diff::address` (`history_diff.rs:16-22`) for row cells
  (Sonnet m4).
- Routes refuse, with 400 and before any write:
  - derived (formula) fields;
  - IDs that are not in the pin;
  - IDs that fail the slug grammar (`valid_field_id`, `admission.rs:96-103`);
  - host IDs that are not in the live snapshot.

**Per-cell save (D-C, D-S; Sol rev-2 M2, M3; Sonnet M-F, m1).** On the edit page:

| Unit | Form | Admitted keys |
|---|---|---|
| Name, Notes | `meta/…` | `csrf`, `base`, `value` |
| Scalar field | `fields/{id}` | `csrf`, `base`, `value`, plus `null` if nullable |
| Saved-row cell | `cells/{list}/{host}/{prop}` | `csrf`, `base`, `value`, plus `set` if optional |
| New row (Add +) | `lists/{list}/rows` | `csrf`, `base`, and `row-{p}` and `set-{p}` for that one row |
| Quick-add line | `lists/{list}/quick` | `csrf`, `base`, `text` |
| Remove, Move up, Move down | `lists/{list}/rows/{host}/{remove\|up\|down}` | `csrf`, `base` |

- **One form per unit, each with a single visible Save submit button**, placed first in the
  form, so it is the default submitter.
- Enter in a single-line input submits it. Selects, checkboxes, nullable pairs and multiline
  cells save with that button, since they have no Enter (Sonnet M-F). With JS, `change` saves
  selects and checkboxes.
- **Row actions are separate forms**, so Enter can never trigger Remove or Move.
- Remove and Move write at once. **Remove gets "Row removed · Undo" right after** (D-V, §3f).
  Move has no Undo; the reverse move is one click.
- **New row** form: one row's controls, Enter saves the row. A required enum or boolean must
  still be chosen; required numbers may be blank (unknown, D-M).
- New host IDs are allocated inside the transaction (`r<rev>-k`, as today) and returned in the
  response (Sonnet m8).
- **No pending choices exist any more.** D-Q applies a literal and offers the switch afterwards,
  so rev-2 M3 (multiple ambiguous cells in one list save) disappears with the pool choices.
- Each route **refuses native and unsupported pins** (Sonnet m1).
- **The create page stays one full form.** It writes notes for interpreted numbers.
- The legacy full-form edit POST stays accepted, with whole-sheet 409 and D-U (Sonnet m2). With
  D-Q literals, re-posting is safe.

**Conflicts and base revisions (R1; Sol M3, Sonnet m3).**
- Each form carries `base`: the revision at which that unit's **displayed** value was read.
- `base` must be an integer within `1..=L`. A malformed value gets 400; a base above L (forged)
  gets 409.
- **The conflict rule compares values, not intervening history.** The server loads
  `base` with `snapshots::raw_load(…, Some(base))` and the live snapshot L, in the same
  transaction, and answers 409 when the unit's **protected value differs** between them.
  - An ABA change (84 → 69 → 84) is therefore *not* a conflict. That is deliberate: the
    displayed value is still the true one, and checking every intervening revision would decode
    unbounded history under the single DB mutex.
  - Exactly two snapshots are decoded per write.

  | Unit | Protected value |
  |---|---|
  | Metadata, scalar field, saved-row cell | that address's value, plus the row's existence for cells |
  | New row | nothing (it only appends) |
  | Quick-add | nothing (it only appends) |
  | Remove | the row's existence **and all of its property values** (a stale Remove after another tab edited the row gets 409) |
  | Move up / down | the list's **order of host IDs** (a stale Move after a reorder, add or remove gets 409) |

- The patch is always applied to the **freshly loaded live snapshot** L, never to the historical
  base, and L + 1 is written.
- **Store change:** `private_save_keeping` (`sheets.rs:279`) today requires the whole-sheet
  revision to equal the live one. The new routes pass `(base, unit)` and use this rule instead;
  the legacy full-form and create paths keep the whole-sheet check.
- A **stale no-op** (the value already equals the submission) answers "No change" rather than
  409.
- After a 409 the re-rendered form carries `base` = L and the player's draft, so a second
  Enter is a conscious overwrite.
- **Busy server (D-X3):** sheet write routes wait up to 2 s for the `JOBS` permit instead of
  failing at once (`definitions/mod.rs:33-37` uses `try_acquire`). Then they answer a styled
  503 page that re-renders the player's submitted input with "Server busy; press Enter
  again". The wait fits inside the 5 s request timeout. Reads keep `try_acquire`.
- **Without JS**, every save does PRG, and the whole page is re-read, so every `base` equals L.
  Text typed in another field without pressing Enter is lost on that reload. That is accepted
  and documented.
- **With JS (§3g)**, after a save the module updates `base` **only for the saved unit**, and
  only if the response revision is newer than that unit's current base. Every other unit keeps
  its own base, so an unrelated save never rebases a dirty draft.
- Requests per unit are serialised: one in flight, the latest queued. A late response for an
  older request is ignored.
- **Draft generations (Sol M1).** Per unit, the module keeps a draft generation counter that
  increments on every `input` event. A successful response:
  - always updates that unit's acknowledged saved value and `base`;
  - replaces the control's text **only if** the draft is still the generation that was
    submitted.

  A value typed while the save was in flight is kept, and so is a queued second save.
- **Shared read-only projection (Sol M2).** Derived values and checks have their own
  projection revision. A response updates them only when its revision is ≥ the projection's
  current revision. Per-unit acknowledgements stay independent, so out-of-order responses from
  different units never regress Load or the checks.
- **R1 test:** tab A at revision 10 has an unsaved Armour edit; tab B saves Armour (11); tab A
  saves HP (12). Tab A's Armour form still has `base` 10, so saving it gets **409** with the
  stored value and the draft both kept. The same sequence is repeated with a dirty row cell.

**Quick-add (D-S).**
- A list may declare the optional member `"quick_add": {"name": <text property>, "quantity":
  <integer property>?}`.
- Admission refuses it when the list has a required enum or boolean property, because a quick
  add could not fill it, or when it names a property of the wrong type.
- Deterministic parse of `text` (≤ 200 characters):
  - `^(\d{1,6})\s+(.+)$` → quantity and name;
  - `^(.+?)\s*[x×]\s*(\d{1,6})$` → name and quantity;
  - otherwise name = the whole text and quantity = 1;
  - an empty name → "Nothing to add".
- Other required numbers are `null` (unknown); optional properties are absent.
- One revision; no note.

### 3b. Interpretation service (CPU)

Unchanged from rev 2 except where noted.
- **Shape:** an `Interpreter` trait, a `StubInterpreter`, and a crate adapter.
- **Crate selection:** the six contract items, checked against an axum stub: loopback HTTP;
  structured-output pass-through; no redirects, proxy or hosted fallback; a bounded body; a
  timeout and cancellation that closes the connection; licence and footprint; maintenance.
  D-H applies: stop and ask.
- **Runtime:** llama-server on an `--internal` network, plus a pinned forwarder bound only to
  `127.0.0.1:8091`. The flags are `--cpus 2 --threads 2`, plus memory, swap and PID limits.
  The model is hash-checked before it is mounted.
  - The forwarder has a **fixed upstream** and must not act as a general proxy.
  - Step 2b verifies that the inference container really cannot reach the outside.
- **Endpoint (Sonnet m4):** must be an IP literal, `127.0.0.1` or `::1`; host names are
  refused. Assistant off is the default.
- **Context (D-F, D-J):**
  - phrase and target first;
  - then other **stored** integer and enum fields in definition order, until 4 KiB. Derived
    fields are excluded from context and from `@` references (Sonnet m5);
  - catalogue entry text last, in step 3, cut first when space runs out (Sol rev-2 m2).
  - All dynamic strings are untrusted data.
- **Output:**
  - a strict JSON schema whose expression field has a `pattern`;
  - terms are literals or `@field` references;
  - operators are `+ − × ÷` and parentheses;
  - `÷` is rounded at once with `floor` or `ceil`, or must be exact (`none`);
  - every literal must appear in the phrase or equal the target's value;
  - the display shows base, expression and effect, and the reason is "the assistant's
    reading";
  - `max_tokens` 128, measured in 2b and raised to 256 if bundles get truncated.
- **Jobs (Sol rev-2 m2):**
  - one worker; a queue of 4 in total and 1 per account. The per-account limit **counts
    running work**. Supersession is checked **before** the capacity refusal;
  - a 60 s queue deadline and a 30 s run deadline;
  - at most 64 records. Non-terminal records are at most 6 (queue, running, reminder), so
    terminal eviction always makes room;
  - 128-bit IDs; poll and apply recheck rights;
  - a reminder job (step 3) uses its own low-priority slot, bound to the account, and does not
    consume the per-account cap (Sonnet m6).

**Proposal freshness (R2, Sonnet M-A; D-P).** When a job finishes, Rust validates each option
and stores its **bound read/write set**:
- every write target: address, value at the job's base revision and, for row cells, the row's
  host ID;
- every `@` reference: address and value;
- the **displayed effect**: before → after for each target;
- for reminders and bundles (Sol M6), the **supplied context** as well:
  - each list-row name the prompt carried;
  - its remembered key;
  - the identity (catalogue version and entry key) of any entry text sent;
  - every enum value in the context.

  All of these are bounded by the 4 KiB prompt. A changed item name, key or enum therefore
  invalidates the option even when the numbers are unchanged.

Bundle targets must be distinct; a duplicate drops the option. The freshness guarantee covers
exactly this bound set. It does not claim that the model's relevance judgement was right
(injection is contained, not prevented).

`POST /private-sheets/{id}/proposals/{job}/{n}` (`csrf`) then does, in one `IMMEDIATE`
transaction:
1. Recheck rights and that the sheet is active.
2. Check that the job binding matches and that the job is **not already applied**: jobs are
   single-use and marked consumed after the write.
3. Require **every address in the read/write set to still hold its bound value** and every
   bound row to exist.
4. Re-evaluate, and require the **effect to equal the displayed effect**.
5. Write, with notes `confirmed` sharing a `bundle` number.

Any mismatch, or an expired job, gets "Something this proposal used has changed: ask again",
nothing is written, and there is never a literal fallback. A change to an unrelated field does
not invalidate the proposal.

### 3c. Confirm, bundles, reminders

- **Interpretation (JS):**
  1. A cell save records `unresolved`, and the module starts a job.
  2. It shows "Thinking…" in `role="status"`.
  3. Then 1–3 options with base, expression and effect, or the choices of an `ambiguous` reply.
  4. **Use** → the apply route. Ignoring the options leaves the scribble.
- **Bundles (step 3):**
  - up to 4 changes to integer, enum or boolean addresses (scalars, or saved-row cells by host
    ID); no row adds or removes;
  - one revision, with notes sharing `bundle`;
  - freshness as in §3b.
- **Reminders (step 3):**
  - **Trigger (D-W):** a saved **decrease of any top-level integer field**, with the assistant
    on and JavaScript present. There is no owner marking. The model's `relevant` flag judges
    relevance, and the low-priority slot bounds the cost.
  - **Context:** the decrease, numbers and enums, list-row names from properties that carry
    the `catalogue` member, and linked entry text when exactly one entry matches or the stored
    key (D-R) resolves. Entry text is ≤ 2,000 characters and quoted.
  - **Output:** `{relevant, row, hint ≤ 120, option?}`, where the option is a bundle.
  - **Display:** a dismissible hint (client-side only); it never blocks or auto-applies.
  - Injection is contained by the schema, the provenance rule (numbers taken from entry text
    are labelled as such), the full display, and confirmation.

### 3d. Warnings

| Warning | Source | Stored | Shown |
|---|---|---|---|
| Not understood (value kept or unknown) | `unresolved` / `erased` note | yes | next to the cell, until a newer note |
| "Did you mean 84 − 15 = 69?" | `sign_literal` current note | yes | next to the cell |
| Assistant failure, expiry, busy, stale | job / apply | no | inline (JS) |
| Not in catalogue / which one | §3e on view | no | next to the cell |
| Advisory check | existing formulas (`web.rs:732-748`) | no | existing section |

Warnings never block, and each is tied to its control with `aria-describedby`. The top-of-page
count is cut.

### 3e. Names, "Which Sword?", suggestions

**Member.** An optional `"catalogue": {"id": <link>, "entry_type": <id>?, "key": <property>}`
on a **text** item property.
- `key` names a declared **optional text** property of the same list that holds the remembered
  entry key (D-R). It is an ordinary declared property, so the value schema, formulas, history
  and freeze need no special case.
- It is rendered as the link, never as a control.
- Checked at the three step-3 sites, plus local checks: the key property exists, is optional
  text, is not itself catalogue-backed, and is used by at most one hint.
- The member is resolved only through the immutable pin.

**Linking on view.** The name is normalised (NFKC, case-folded, diacritics removed, whitespace
collapsed).
1. If the stored key resolves to an eligible entry (the linked catalogue, the type filter, not
   retired) **and** its normalised name equals the cell's name: link it.
2. Otherwise, exactly one eligible exact match: link it.
3. Several matches: "Which Sword?" lists the candidates (name · type · catalogue). Choosing one
   posts `cells/{list}/{host}/{key}` with the entry key: one revision, no note. The link is
   remembered from then on (D-R).
4. None: "Not in the catalogue" plus Browse. With JS, also "Did you mean…?" on demand.

When the name cell is saved with a different name, the server **clears the key** in the same
revision. A stale key cannot point at the wrong entry.

**Suggestions** (step 4, JS). As in rev 2:
- read rights and pin correspondence rechecked per request;
- no `JOBS` permit, a pool of 4, and 429 at saturation;
- the query is 2–64 scalars and ≤ 256 bytes;
- a prefix match on cached names, then FTS, then an on-demand typo pass;
- the typo pass is bounded: within ±2 characters of length, at most 20,000 comparisons and
  20 ms, outside the mutex;
- the type filter applies on every path;
- the name cache is LRU-bounded by bytes (32 MiB), filled once, and holds names only;
- p95 ≤ 50 ms, measured.

Picking a suggestion stores the name, and its key when the picked entry is one of duplicates.

### 3f. Undo (D-N) and history (D-E)

- **Eligible:** the current note of an in-scope address when:
  - its op is `literal`, `sum`, `sign_literal`, `switched`, `confirmed`, `reverted` or
    **`erased`** (D-X1);
  - its `before` is recorded. An `{"absent":true}` before means Undo makes the optional
    property absent again, so present ↔ absent is undoable;
  - the sheet is active.
- **Not eligible:** `unresolved`, creation, new rows, text cells, frozen sheets.
- **Row removal (D-V):** the removal note `{"row":"present"}` → `{"row":"absent"}` offers
  "Row removed · Undo". Undo:
  1. reads the row from the revision before the removal, through the validated history reader;
  2. requires that no row with that host ID exists now;
  3. re-inserts it at its old index, clamped to the current length, with a **new host ID**
     `r<L+1>-1`, because removed IDs never return (`snapshots.rs:67-70`);
  4. writes a `reverted` note.

  The re-added row is a new row for later notes, so its cells have no Undo until they are edited
  again.
- `POST /private-sheets/{id}/undo` with `csrf` and `note` (revision and address). In one
  transaction:
  1. Recheck rights.
  2. Require the note to still be the current note of the address and the current value to
     equal its `after`.
  3. Write `before`, with op `reverted` and `target`.
- If the cell changed since, there is nothing to undo: the newer change has its own note.
  **No forced Undo in the POC.** That removes Sol rev-2 M6 (binding a forced confirmation).
- A double submit → the second finds the note no longer current: "already undone".
- **Bundles:** Undo on a member reverts **all** members, and only if every member's current
  note is still that bundle and holds its `after`. Otherwise nothing is reverted and the page
  lists what changed. There is no partial Undo.
- **History:** each revision lists its notes (raw, before → after) next to the values-free
  diff. Readers are the sheet's readers: the custodian and Masters.

### 3g. JavaScript (D-G)

- One module, `assets/sheet-assist.js`, loaded with `<script type="module">` on sheet pages.
- **CSP:** a `SheetPage` response extension, applied in `normalize`, gives `default-src 'self';
  script-src 'self'; style-src 'self'; object-src 'none'; base-uri 'none'; form-action 'self';
  frame-ancestors 'none'; require-trusted-types-for 'script'; trusted-types 'none'`.
  - The inline style moves to CSS.
  - The module uses classes, never `setAttribute('style')` (Sonnet m3).
  - Error pages from outer middleware (timeout, 503) keep the default header; documented.
- **Transport (Sol rev-2 m1):**
  - requests are `URLSearchParams` bodies (`application/x-www-form-urlencoded`), which
    `post_origin` accepts;
  - responses are JSON, and the DOM is built with `createElement` and `textContent` only (no
    `innerHTML`);
  - integers and revisions are carried as **strings**.
- **Save response (Sol rev-2 M7):**

  ```
  {revision, unit: {address, value, note?}, derived: [{id, text}],
   checks: {violated, not_evaluated, passed, items: [{id, status, message}]}}
  ```

  - everything is computed from the new saved revision;
  - the module updates the saved unit, every derived value's text and the advisory-checks
    section, all read-only nodes. **No input other than the saved unit is touched**, and no
    other unit's `base` moves (R1).
- **Combobox:**
  - `aria-controls` and unique option IDs;
  - while the list is open, Enter selects and does not submit;
  - IME composition is respected;
  - a debounce, `AbortController` and a generation counter.
- Live region, real buttons, focus returning to the cell, 24 × 24 px targets, 390 px layout.
- **Without JS**, every save, Undo, switch, quick-add and row action works through PRG.

### 3h. Security summary

- Model output passes the schema, the grammar, literal provenance, Rust evaluation and
  read/write-set freshness. It is rendered escaped inside `<bdi>`.
- Rights are rechecked on every request and inside every write transaction.
- **Limits:** raw ≤ 200; notes ≤ 256 per revision; prompt ≤ 4 KiB; the job and record caps of
  §3b; the suggest bounds of §3e.
- Loopback IP only; the model server has no egress; no redirects or proxies; no prompts in
  logs; no secrets.
- **Known POC limitation L1:** unsealed notes (D-T).

## 4. What changes versus existing rules and plans

| Existing | Where | Change |
|---|---|---|
| Q4: item not in the catalogue is invalid | `catalogue-plan.md:29` | Warning with suggestions; "Which Sword?" is remembered (D-R) |
| D9/R9 writer membership check | `catalogue-plan.md:295-307`, `:624-636` | Not built. The remembered key is advisory: a dangling key just shows no link |
| No JavaScript on sheets; "no script" in the gate | `catalogue-plan.md:102`, `release-scope.md:139`, `auth/mod.rs:191-193` | One module under a strict CSP; no-JS editing kept (D-G) |
| One full-page Save; M4 list drafts with Add, Remove, Move and Save | `sheet-presentation.md:15-16` | Per-cell forms saved on Enter. Row actions and quick-add write immediately (D-C, D-S). The create page keeps the full form and the M4 draft ops |
| Strict integers; one bad integer refuses the whole save | `values.rs:164-174`, `web.rs:1210-1230` | Literal, sum, or unresolved (value kept or unknown). A sign-led input is literal, with the switch offered (D-Q) |
| No `null` in non-nullable integers or inside items; formulas refuse it | `values.rs:94`, `:116`, `private_pin.rs:97-117`, `convert.rs:89-97`, `:111-118` | `null` = unknown, admitted for every integer cell; formulas fail per field |
| Stale page → whole-sheet 409 | `web.rs:1124-1148` | Per-unit 409 against the unit's `base`. The legacy full form keeps whole-sheet 409 |
| Every save writes a revision | `snapshots.rs:306-406` | No-op saves write nothing (D-U) |
| History summary values-free | `history_diff.rs:1-5` | The diff is unchanged; notes with values are shown next to it (D-E) |
| Catalogue plan non-goals (LLM, revert) | `catalogue-plan.md:36-47` | In scope; legality stays out |
| LLM authors definitions and doesn't judge at play time | `definition-language-draft.md:43-48` | Narrow play-time use: confirmable proposals and reminders |
| "No model proposes" | `release-scope.md:149` | Local model, confirm-only; updated in step 5 |
| Accuracy bar | `discovery.md:177-179` | Waived for the POC (D-D) |
| No pool, quick-add or name-key definition members | `runtime-definition-contract.md` | New optional `quick_add` (list) and `catalogue` (text item property) members, omitted when absent so old hashes are kept. The rev-2 pool marker is dropped (D-Q) |

**What stays.**
- Model output never silently alters a sheet, and legality is advisory.
- Rust does all arithmetic.
- `snapshots::write` is the sole writer; history is append-only, and rollback is a new revision.
- CSRF and authorization, immutable pins, size limits, manuals local, containers, no hosted
  fallback, the app usable with the assistant off.

## 5. Steps

Each slice and step:
- has its own branch;
- has failing-first tests, with the red run recorded;
- is independently reviewed before merge (implementation review);
- is built and tested on 2 cores on weekdays.

Slices 1.1 to 1.6 go into master one by one, and each leaves the demo usable.

### Demo (lands with slice 1.1)

`tests/support/source_fixture.rs` gains `assist_definition()`, written as
`assist-definition.json` by `fixture_pair` (`:57-95`). It reuses the closures and the pattern of
`list_definition()` (`:97-156`) (Sonnet m12). Game `synthetic-game/first`, definition
`assist-sheet`, fields:
- `hp` "Current HP" and `max_hp` "Max HP";
- `armour` "Armour modifier" (starts at −1);
- `luck` "Luck" (nullable);
- `equipment` with `name` (text), `quantity` (integer), `weight` (integer) and `broken`
  (optional boolean);
- derived `load` = `sum(equipment.map(i, i.quantity * i.weight))`, and the check
  `hp <= max_hp`.

Slice 1.6 and step 3 add their members to it (`quick_add`, `catalogue` plus `key`, a demo
catalogue). The `name`/`notes` collision test in 1.5a reuses `list_definition()`, which already
has a field with id `name`.

`tools/assist_demo.py` is a dev tool (prototype tier) built on the `smoke_release.py` helpers:
1. It starts from a fresh data directory.
2. It registers the ruleset, imports the source fixture and publishes the definition.
3. It creates an owner and a player with generated dummy local passwords, and prints them.
4. It creates a campaign and a sheet (HP 84, Max HP 84, Armour −1).
5. It serves `--loopback` and prints the URL and logins.

```
cargo build --locked -j 2
cargo run --locked -j 2 --example source_fixture -- target/assist-demo/fixture
python3 tools/assist_demo.py target/debug/mimik --fixture target/assist-demo/fixture \
    --data target/assist-demo/data
# open the printed http://127.0.0.1:<port>/private-sheets/<id> and log in as the player
```

Each slice states what the demo can do at that point. After 1.1 it is still the full-page
form with one Save, and unreadable input is still refused.

### Step 1: seven slices (no JavaScript, no model)

#### 1.1 Sums on the existing save — execution-ready

**Visible result.** In the demo, "84 -15" + Save in Current HP stores 69. "-1" stays −1. On a
phone keyboard, the minus key and space are available in integer cells.

**Scope.** Exactly the following; nothing else changes.
1. **New module `src/campaigns/entry.rs`** (pure, no I/O):
   `pub(crate) fn number(raw: &str) -> Result<i64, Unreadable>`.
   - It implements the §3a grammar: literal and sum, ASCII space and tab trimming, the
     200-character server-side cap, the 64-scalar parser cap, NUL and control characters
     refused, checked i128 accumulation per term, and the i64 result range.
   - Blank is `Unreadable` in 1.1; unknown values arrive in 1.4.
2. **`values::integer` is untouched** (protocol integers and revisions, `web.rs:1074`,
   `:1127`), and its test `campaigns/tests.rs:484-486` stays.
3. **Integration:** `scalar_value` (`values.rs:541-552`) calls `entry::number` for
   `Control::Integer`. That single call site serves both `values::form` (scalar fields,
   `:648-652`) and `reconcile` (list rows, `:604-605`). An `Unreadable` maps to the same
   `AppError::Invalid` as today, so behaviour stays 422 with the draft.
4. **`inputmode`** (Sonnet M4): the runtime integer inputs drop `inputmode="numeric"`, which on
   iOS hides the minus key and space:
   - `templates/private_sheet.html:12`;
   - `templates/private_list_field.html:6`.

   They get no `inputmode` (plain text keyboard). The native
   templates (`native_fields.html`, `mork_context.html`) are untouched.
5. The demo (above).

**Not in 1.1:** notes, display of the sum, D-Q switch, unknown values, per-cell forms,
JavaScript, CSP.

**Grammar table** (unit tests on `entry::number`, red first against a stub that returns `Err`):

| Input | Result |
|---|---|
| `69`, `+5`, `-5`, `−5` (U+2212), `-0`, `0007` | 69, 5, −5, −5, 0, 7 |
| `84 -15`, `84-15`, `84 - 15`, `84-15+3`, `\t84 -15\t` | 69, 69, 69, 72, 69 |
| `-9223372036854775808`, `9223372036854775807` | i64::MIN, i64::MAX |
| `9223372036854775807+1`, `-9223372036854775808-1`, a 40-digit literal | Unreadable (range) |
| 32 terms whose running sum leaves i128 never (checked per term) | correct or range error, no panic |
| `84--15`, `84+-2`, `- 5`, `5 -`, `-`, `+`, `1 2`, `5.`, `１２` (full-width), `1e2`, `x`, `2*3`, `6/2` | Unreadable |
| empty, spaces only, tabs only | Unreadable (blank; becomes unknown in 1.4) |
| 201 characters (padded valid sum), 65 scalars, an embedded NUL, `\u{7}` | Unreadable |

Plus a property test: arbitrary strings up to 300 characters never panic, and every accepted
input re-parses to the same value from its canonical decimal output.

**Connected-save tests** (through the router with the existing test harness, red first):
- **Scalar sum:** POST the full form with `value-hp=84 -15` → 303, and the saved snapshot
  `hp = 69`.
- **List sum:** a row's `quantity` = `2+1` → the saved item has `quantity = 3`.
- **Canonical negative re-post (B1 regression):** with `armour = −1` saved, re-post the
  unchanged form (`value-armour=-1`) while editing General notes → `armour` stays −1. Repeat for
  a list-row integer −3.
- **Unreadable still 422:** `value-hp=lots` → 422 with the draft and nothing saved, the same as
  today's behaviour and message class.
- **Native strictness unchanged:** a native MÖRK BORG edit with `hp=84 -15` is still refused
  by `native_draft(...).validate()` (`web.rs:685`), and `values::integer("+1")` is still `Err`.
- **Template:** the rendered runtime integer input has no `inputmode="numeric"`, and native
  templates still do (DOM test with `scraper`).

**Browser.** The existing Chromium gate stays green. One added check at 390 × 844: typing
`84 -15` into Current HP and saving stores 69.

**Review.** Implementation review (Sol and Sonnet, plus the third chain if available).

#### 1.2 Notes and scribbles

**Scope:**
- migration 020 (the simple note table, D-T, with the typed note value of §3a);
- notes for sums, literals, enums and booleans, including present ↔ absent;
- an unreadable integer on a **known saved value** keeps the value with an `unresolved` note;
- no-op saves write nothing (D-U), with rights still rechecked;
- accepting the kept value resolves a scribble;
- note display, with scribbles inside `<bdi>`.

**Not yet** (Sonnet m10): unreadable input on a **new row or create** still gets 422 until 1.4.

**Visible:** "84 − 15 = 69" next to HP; "lots" → "kept 84 · 'lots' not understood".

**Tests:**
- a note is written in the transaction, and a failure after `snapshots::write` rolls back
  everything (test hook);
- UPDATE, DELETE and replacement abort;
- an insert naming a non-live revision aborts;
- notes are read back exactly from the typed note value: `{"absent":true}` versus
  `{"unknown":true}`, a 64-character enum ID within 80 bytes, booleans;
- a later literal replaces a scribble as the current note;
- entering the kept value after a scribble → a `literal` note and a data-unchanged revision;
- a no-op save → no revision, and a no-op with revoked rights → 403, not "No change";
- the per-control no-op comparisons: `007` vs 7, a Name with trailing spaces, an enum, a
  nullable pair, an optional absent vs present;
- 257 notes → 422;
- text edits write no note;
- freeze writes none, and existing notes stay readable;
- a Master sees raw text in history (D-E);
- a scribble with RTL characters renders inside `<bdi>`;
- raw text with NUL or control characters → unreadable.

#### 1.3 Switch and Undo

**Scope:** `sign_literal` notes plus the switch route (checked arithmetic, D-U precedence,
D-X4 dismiss); the Undo route, including `erased` (D-X1) and present ↔ absent. Bundles come
later.

**Visible:** "-15" → HP −15 · "Did you mean 84 − 15 = 69? [Yes]"; "84 − 15 = 69 · Undo".

**Tests:**
- "-1" on armour sets −1 (D-Q);
- re-entering "-1" over −1 → no revision and no offer;
- switch → 69 with `switched`;
- a second switch is refused; a switch after another change is refused;
- no offer without a known before-value;
- no offer when `b ± n` overflows (`b = i64::MIN`, input `-1` → `b − 1` overflows);
- Undo of literal, sum, switched, enum/boolean changes, `erased` (in 1.4) and present → absent;
- Undo not offered on create, a new row, `unresolved`, text or frozen;
- a double submit gets "already undone";
- Undo of an Undo;
- rights revoked → 403.

#### 1.4 Unknown values

**Scope:**
- the `null` path through schema (idempotent nullable composition), domain, formula inputs and
  records, the `Null` result mapping, freeze, history and Undo;
- blank → `erased`;
- new rows and create stop refusing unreadable numbers;
- "?" display;
- the profile note and `runtime-definition-contract.md:279`.

**Visible:** "lots" in a new row's Quantity → "?" plus the scribble; Load says "Not computed: a
value it uses is unknown (null)".

**Tests:**
- a new row or newly present property with an unreadable value → `null` and a note;
- create with "lots" → created with unknown;
- `list_values` accepts null;
- an existing **nullable** integer definition still compiles its schema (no nested type array);
- `convert::record` and `convert::inputs` map null, so a reader fails per field and the others
  compute;
- `copy = hp` with unknown `hp` → `NullValue` wording, not `TypeMismatch`; an item-property
  copy, the same;
- arithmetic on null → `NullValue`;
- **CEL forms** (Sonnet m9): `hp == 0`, `has(...)` and `hp > 0 ? a : b` with unknown `hp` are
  pinned to their actual results and documented; none of them errors silently into a pass;
- a dependent says "it uses Load, which could not be computed";
- Undo restores `null`; Undo of `erased` restores the number;
- freeze with nulls;
- the history diff counts null versus a number;
- an empty Name and a required enum on create are still refused;
- historical formulas use historical values.

#### 1.5a Per-field save: metadata and scalar fields (Sonnet m11)

**Scope:**
- the `meta/…` and `fields/{id}` routes with exact key sets and typed addresses;
- route refusals (derived, unknown and non-slug IDs; native and unsupported pins);
- per-unit `base` with bounds and value-based conflicts;
- a stale no-op answers "No change", and after a 409 the form carries `base` = L;
- the store's whole-sheet check is bypassed only for these routes;
- one form per unit with a first Save button;
- the D-X3 bounded permit wait and styled 503.

The legacy full form is kept, with whole-sheet 409.

**Visible:** Enter in HP saves HP alone; selects and checkboxes save with their button.

**Tests:**
- `list_definition()`'s `name` field plus the metadata Name save separately;
- two fields saved from one page revision both succeed;
- the same field from a stale base whose value differs → 409, with the draft and `base` = L;
- ABA → no conflict (documented);
- `base` 0 or above L → 400 or 409;
- a derived field route → 400;
- a native pin → refused;
- the permit held by another request for 1 s → the save succeeds; held for 3 s → styled 503
  with the input;
- the R1 two-tab sequence (no-JS `base`);
- the legacy full form keeps whole-sheet 409;
- Chromium no-JS run at 1280 and 390 (Enter, select button, multiline button).

#### 1.5b Per-cell save: lists, new row, row actions

**Scope:**
- the `cells/…`, `lists/{list}/rows` and row-action routes;
- Remove protects the row's values, Move protects the order;
- Remove writes a row note, with Undo (D-V);
- the new-row form;
- host IDs returned.

**Visible:** Enter in a row's Quantity saves that cell; Remove → "Row removed · Undo".

**Tests:**
- Enter in the first, middle and last row saves only that cell and never adds, moves or removes
  a row;
- two cells of different rows saved from one page both succeed;
- a stale cell → 409;
- a stale Remove after another tab edited that row → 409;
- a stale Move after a reorder, add or remove → 409;
- a removed row's cell → 400;
- Undo of Remove re-inserts the row with a new host ID at the clamped index;
- Undo of Remove when the list is full (256) → refused with a message;
- a new row with a required enum unset → refused; with a blank number → unknown (1.4);
- the new-row form writes ≤ 32 notes;
- Chromium no-JS run.

#### 1.6 Quick-add

**Scope:** the `quick_add` member (schema, admission refusals) and the parse.

**Visible:** "3 torches" + Enter adds Torches ×3; "healing potion x2" adds the potion ×2.

**Tests:**
- both patterns, a plain name (→ 1), an empty name, a 7-digit count (→ treated as a name), and
  200 characters;
- a required enum in the list → admission refusal;
- unknown values for other required numbers;
- no notes written;
- old definition hashes unchanged.

### Step 2a: JavaScript foundation

- Scope: `SheetPage` CSP, the asset route, fetch saves with per-unit `base`, the save response
  with derived values and checks, string integers.
- Tests:
  - final headers on 200, 409 and 422;
  - other routes unchanged;
  - a multipart body still gets 415 and the module sends URL-encoded;
  - 9007199254740993 round-trips;
  - **R1 with JS:** a saved HP response leaves the dirty Armour `base` alone, and Armour → 409;
  - **Same-unit draft generations (Sol M1):** a delayed HP response arrives after the player
    typed 60 without submitting → the 60 stays and the base updates; repeated with a queued
    second save;
  - **Shared projection ordering (Sol M2):** HP (revision 11) and Equipment (revision 12)
    responses arrive reversed → both units acknowledge, and Load and the checks show
    revision 12;
  - a save that changes `load` and `hp <= max_hp` updates both read-only regions without
    touching other inputs;
  - integers are strings **recursively**: list values, note before/after values and revisions
    (Sol minor 2);
  - the final CSP header after `normalize`, on the new routes and on their refusals;
  - no CSP violations, and no `innerHTML` or style attributes (lint test over the module);
  - the inline style from `private_sheet.html:7` moved to `app.css` (Sonnet m13);
  - the no-JS run is still green.

### Step 2b: Crate and runtime selection (prototype tier; parallel to step 1)

- The crate contract items and D-H stop.
- The topology, with egress tests from inside the model container **and** from the forwarder,
  which must not proxy anywhere but its fixed upstream (Sol minor 2).
- A latency and schema screen (§7); output tokens measured.
- A results note, reviewed before it picks the crate and model.

### Step 2c: Interpretation

- Scope: trait, stub, adapter; IP-literal loopback config; worker, queue and records; poll; the
  apply route with read/write-set binding; the UI.
- Tests:
  - schema-invalid, grammar-invalid, literal provenance (`84 + 915` dropped), a text field
    referenced, `÷` on negatives, division by zero;
  - the queue: the 5th and the account's 2nd → 429; supersession before capacity; cancellation
    (the fake server sees the disconnect); the 60 s and 30 s deadlines; 64 records;
  - **R2:**
    - a referenced field changes while the target does not → refused;
    - a referenced row disappears → refused;
    - effect mismatch → refused;
    - a second apply → refused, including two concurrent applies: exactly one writes, and the
      consumed flag is set only after the transaction commits;
    - another sheet's or another account's job, or an invalid option index → refused;
    - an unrelated field change → still applied;
  - the job store keeps at most 6 non-terminal records under reminder creation, supersession
    and cancellation (Sol minor 2);
  - expired job → no write and no literal;
  - another account's job, or revoked rights → not found or 403;
  - `JOBS` free during the model call;
  - a host name or non-loopback endpoint refused;
  - assistant off is honest everywhere;
  - no player text in logs.
- One live smoke, recorded as live inference.

### Step 3: Bundles and reminders

- Scope: the item-property `catalogue` member with `key`; the unique and key linking on view
  (rule 1–2 of §3e); a demo catalogue; bundles; reminder jobs and hints.
- Tests:
  - bundle of 4 in one revision; a 5th change dropped; duplicate targets dropped;
  - one bundle member changed → apply refused;
  - **contextual freshness (Sol M6):** a reminder bundle whose row name, remembered key, entry
    version or context enum changed, with every number unchanged → apply refused;
  - bundle Undo all-or-nothing;
  - reminder only on a decrease of a top-level integer; none with the assistant off; it does
    not take the account cap;
  - duplicate names without a key → no entry text in the prompt;
  - text and multiline never in the prompt (asserted on the stub request);
  - entry text cut to 2,000; injection in entry text → at most a fully shown option.

### Step 4: Suggestions and "Which Sword?"

- Scope: the suggest route, cache, matching, combobox, the on-demand typo pass, the "Which
  Sword?" chooser, and key clearing on rename.
- Tests:
  - confinement: forged address, unlinked catalogue, other game, native, unsupported or corrupt
    pin;
  - the type filter on every path;
  - 2-character prefix; typo distance 1 and 2; diacritics;
  - query bounds; typo-pass bounds; cache bytes;
  - no `JOBS` 503; 429;
  - escaping;
  - choosing among duplicates stores the key and the link persists;
  - renaming clears the key;
  - a dangling key shows no link;
  - p95 measured.

### Step 5: Docs, release scope, gate

- Contract, presentation, release-scope, publication and dependency docs.
- `cargo about`, audit, packaging, and the unpacked smoke with the assistant off.
- The assistant scope: "local endpoint, POC quality, not certified (D-D)"; L1 listed.

## 6. Risks, limitations, later, questions

**Known POC limitations.**
- **L1:** notes are unsealed (D-T). A process with database write access could add notes to the
  current revision. Notes are help, not evidence, until sealing lands.
- **L2:** without JS, text typed in another field and not entered is lost on reload.
- **L3:** conflicts compare two snapshots, so an ABA change of a unit is not detected
  (§3a). This is deliberate.

**Risks.**
- **R1:** latency at 2 threads is unmeasured.
- **R2:** model quality is unknown, and the bar is waived. The display and confirmation are the
  safeguard.
- **R3:** reminder prompts carry catalogue text (contained, not prevented).
- **R4:** a crate gap would stop step 2c (D-H).
- **R5:** one revision per Enter makes history long. D-U removes no-ops; there is no cap.
- **R6:** the forwarder is a new pinned image.
- **R7 (D-X2):** history has no cap (D-U). **Before release** a per-account write-rate limit or
  a storage guard is required. Sonnet rev-3 M5 is recorded here, not solved in the POC.

**Later.**
- Sealing and verifying notes (before release).
- Player `*` and `/`.
- Text, enum and boolean interpretation.
- Notes and Undo for text.
- Undo of moves and creation; forced and arbitrary restore.
- A write-rate limit or storage guard (D-X2, before release).
- Owner-reviewed rules for reminders without a model (D-I).
- A model for quick-add.
- More threads off-hours.
- The quality bar.
- Retiring the legacy full-form edit POST.

**Decided:**
- rev-1 Q1–Q6 → D-A to D-D (Q5: Gemma excluded);
- rev-2 Q1 → D-Q, Q2 → D-R, Q3 → D-S;
- rev-3 Q1 → D-V (immediate Remove with Undo), Q2 → D-W (any decrease; model judges);
- forced Undo → not in the POC;
- "create never refuses" → narrowed (§3a);
- main-session defaults D-X1 to D-X4 (§0), which the user may revise.

**Open questions for the user:** none blocking. D-X1 to D-X4 are defaults the user may revise.

## 7. Verification

- **Rust gate:** fmt, strict Clippy, locked tests (red then green), audit, `cargo about`, release
  build, size delta; 2 cores on weekdays.
- **Browser** (Chromium at 1280 × 720 and 390 × 844, demo data, with and without JS):
  - per-cell Enter, buttons for selects and checkboxes, row actions, quick-add;
  - notes, switch, Undo, "?" unknowns;
  - R1 two-tab;
  - derived and check refresh;
  - CSP headers and no violations;
  - combobox keyboard and IME;
  - no overflow.
- **Process smoke:** the unpacked archive with the assistant off, and once with the stub
  endpoint.
- **Model screen** (prototype tier, `tools/assist_eval.py` with unit tests; a results note with
  exact scope, denominators and limitations; reviewed; **no pass bar**, D-D):
  - p50/p95 latency at 2 threads, schema-valid rate, output-token use;
  - per class: correct within the first 3, and false-confident.

  | Class | n |
  |---|---|
  | Damage and heal phrases (EN/IT) | 16 |
  | Fractions and vague | 8 |
  | Other-field references | 6 |
  | Nonsense | 8 |
  | Injection in the player phrase (a legitimate request, shown as an option) | 4 |
  | Injection hidden in context | 8 |
  | Reminder relevance (step 3) | 8 |

  Fast-path inputs never reach the model. Results are reported per class; this is not a measure
  of general accuracy.
- Unit, connected-router, browser and live-inference evidence are reported separately, and a
  check that was not run is never reported as passed.

## 8. Review finding map

**Rev 1 → rev 2** (still valid unless noted).

| Finding | Resolution | Section |
|---|---|---|
| Sol B1, Sonnet B1: re-posted negatives | Superseded by D-Q: every sign-led input is literal, so re-posting is idempotent; no-op rule (D-U) | §3a |
| Sol B2: stale proposal → literal | Dedicated apply route, no literal fallback (D-P); extended by R2 | §3b |
| Sol B3: note integrity | Replaced by D-T simple notes plus limitation L1; sealing later | §3a, §6 |
| Sol M1, Sonnet M1: grammar | Formal grammar; player `*` and `/` cut | §3a |
| Sol M2: unresolved new rows | D-M `null` for every integer cell, full path | §3a |
| Sol M3, Sonnet M3: note lifecycle | Every in-scope change writes a note; current note = latest; scribble until a newer note | §3a |
| Sol M4, Sonnet M7: job bounds | One worker, caps, deadlines, records | §3b |
| Sol M5: auth on poll and apply | Rechecks per request and inside the transaction | §3b |
| Sol M6, Sonnet M2: drafts and keys | Per-unit routes with exact keys | §3a |
| Sol M7: Undo semantics | Current-note rule; no force; bundle all-or-nothing | §3f |
| Sol M8–M10, Sonnet M9: suggestions | Pin confinement, bounds, unique or keyed linking | §3e |
| Sol M11, Sonnet M8: CSP | Set in `normalize` | §3g |
| Sol M12: i64 in JS | Strings | §3g |
| Sol M13, Sonnet M5, M6: injection | `@` references, literal provenance, narrow guarantee | §3b |
| Sol M14: demo | Fixture and `assist_demo.py` | §5 |
| Sol (container), Sonnet M10 | Internal network, forwarder, crate contract, D-H | §3b |

**Rev 2 → rev 3.**

| Finding | Resolution | Section, test |
|---|---|---|
| **Sol R1:** an unrelated save rebases dirty drafts | Per-unit `base`; JS updates only the saved unit's base; requests serialised and late responses ignored | §3a, §3g; slices 1.5 and 2a, two-tab test |
| **Sol R2, Sonnet M-A:** freshness binds the target only | Bound read/write set and displayed effect; single-use jobs; unrelated changes harmless | §3b; step 2c R2 tests |
| **Sol R3:** migration window on existing revisions | Moot: no seals in the POC (D-T); recorded as L1, to be solved with sealing before release | §3a, §6 |
| Sol M1: metadata and field addresses collide | Typed addresses in routes, notes, jobs and Undo | §3a; slice 1.5 test |
| Sol M2, Sonnet M-F: Enter submission contract | One form per unit with a first Save button; row actions as separate forms; buttons for selects, checkboxes and multiline | §3a; slice 1.5 tests |
| Sol M3: pool choices in list or legacy saves | Moot: D-Q leaves no pending choices; the switch comes after the save | §3a |
| Sol M4: notes can't represent all changes; long text; choice discriminator | Typed `before` and `after` for integer, enum and boolean; text out of note scope; `op` discriminator (also Sonnet M-E) | §3a; slice 1.2 |
| Sol M5, Sonnet M-B, M-G: unknown path incomplete | Schema, `list_values`, `convert::record` and `inputs`, freeze, history, Undo; provenance rule dropped; existing wording used | §3a; slice 1.4 |
| Sol M6: forced Undo freshness; bundle semantics | No forced Undo; bundle all-or-nothing | §3f; slice 1.3, step 3 |
| Sol M7: derived and checks stale after a fetch save | The save response carries derived values and checks; the module updates only read-only regions | §3g; step 2a test |
| Sol m1: multipart 415; `innerHTML` vs Trusted Types | URL-encoded bodies, JSON responses, DOM built with `textContent` | §3g; step 2a |
| Sol m2: context overflow; record saturation; supersession order; output tokens | Truncation order; ≤ 6 non-terminal; supersession first; tokens measured | §3b; steps 2b, 2c |
| Sol security notes (forwarder upstream, egress check) | Fixed upstream; egress test | §3b; step 2b |
| Sol question 5: "create never refuses" | Narrowed: unreadable numbers only | §3a; slice 1.4 test |
| Sol suggested slicing | Slices 1.1–1.6, the first very small | §5 |
| Sonnet M-C: verification cost | Moot (D-T) | — |
| Sonnet M-D: step 1a too large | Six slices | §5 |
| Sonnet M-H: history growth | D-U: no-op writes nothing; no cap | §3a |
| Sonnet m1–m8 | m1: native pins refused. m2: legacy full form keeps whole-sheet 409. m3: no style attributes, outer error pages documented. m4: IP-literal loopback. m5: derived fields excluded from context and references. m6: reminder slot. m7: no Keep, no data-unchanged revision except a scribble. m8: host IDs returned | §3a, §3b, §3g |

**Rev 3 → rev 3.1** (both reviewers GO WITH AMENDMENTS, with no Blocking findings).

| Finding | Resolution | Section, test |
|---|---|---|
| Sol slice-1.1 constraints: separate parser; connected-save tests | `campaigns::entry`, with `values::integer` strict; scalar sum, list sum, canonical negative re-post, unreadable 422, native strictness, template | §3a; slice 1.1 |
| Sol M1: in-flight save overwrites a newer draft of the same unit | Per-unit draft generation; the control is replaced only if the draft is unchanged | §3a; step 2a |
| Sol M2: cross-unit out-of-order responses regress derived values | A separate projection revision for derived values and checks | §3a; step 2a |
| Sol M3, Sonnet m3: conflict semantics; Remove and Move; base bounds | Value-based rule on two snapshots (ABA accepted, L3); protected values per unit; patch applied to live L; the store bypasses the whole-sheet check only for new routes; `base` in `1..=L`; stale no-op → "No change"; `base` = L after a 409 | §3a; slices 1.5a, 1.5b |
| Sol M4, Sonnet M2: absent vs null; enum size; clearing a scribble under D-U | Typed note value (absent, unknown, int, enum, bool, row) ≤ 80 bytes; accepting the kept value writes a `literal` note | §3a; slice 1.2 |
| Sol M5: nested nullable schema; `Null` result is a `TypeMismatch` | Idempotent nullable composition; `Null` result → `NullValue` | §3a; slice 1.4 |
| Sol M6: contextual freshness for reminders | Bind names, keys, entry identity and enums in the context; narrow guarantee stated | §3b; step 3 |
| Sol minor 1, Sonnet m8: switch overflow; D-U precedence | Checked `b ± n`; an unchanged input → no offer | §3a; slice 1.3 |
| Sol minor 2: executable security tests | No-op rights recheck; concurrent apply; recursive string integers; final CSP; forwarder egress; job-store invariant | slices 1.2, 2a, 2b, 2c |
| Sonnet M1 → D-X1 | `erased` is Undo-eligible | §3f; slice 1.3 |
| Sonnet M3 → D-X3 | Bounded permit wait (2 s), then a styled 503 that keeps the draft | §3a; slice 1.5a |
| Sonnet M4: `inputmode="numeric"` hides the minus key on iOS | Removed on runtime integer inputs; 390 px check | slice 1.1 |
| Sonnet M5 → D-X2 | No cap in the POC; rate limit or storage guard before release (R7) | §6 |
| Sonnet m1 | Grammar table with "+5", "−5", "-0", lone sign, "- 5", "1 2", "5 -", empty, tabs, 201 characters, zero padding, checked per term; `values::integer` test kept | slice 1.1 |
| Sonnet m2 | Per-control no-op comparison, Name after trim | §3a; slice 1.2 |
| Sonnet m4 | Derived, unknown and non-slug IDs refused; `history_diff::address` reused | §3a; slice 1.5a |
| Sonnet m5 | Note counts for the new-row form (≤ 32), quick-add (0) and row removal (1) | §3a |
| Sonnet m6 | NUL and control characters refused; scribbles inside `<bdi>` | §3a; slice 1.2 |
| Sonnet m9 | CEL `==`, `has()` and ternary over null pinned and documented; contract line updated | §3a; slice 1.4 |
| Sonnet m10 | New-row and create input still get 422 until 1.4 | slice 1.2 |
| Sonnet m11 | 1.5 split into 1.5a and 1.5b | §5 |
| Sonnet m12 | `list_definition()` reused for the collision test; demo built on its pattern | §5 Demo, 1.5a |
| Sonnet m13 | Inline style moved to CSS | step 2a |
| User D-V, D-W | Remove with Undo; any-decrease reminder trigger | §3a, §3c, §3f |

## 9. Slice 1.1 implementation notes

Branch `assist-1.1` (from master `4d07932`). Built exactly the slice-1.1 scope of §5: the
`campaigns::entry` parser, its integration at `scalar_value`, the `inputmode` removal, the
demo fixture and launcher, and the 390 px browser check. Nothing from later slices.

**What was built.**
- `src/campaigns/entry.rs`: `number(raw) -> Result<i64, Unreadable>`. It refuses more than
  200 characters (counted as characters, before trimming), then NUL and every control
  character except tab, then trims ASCII spaces and tabs. A blank input is `Blank`. It then
  splits the text into signed digit runs (a sign-led literal, or an unsigned first term
  followed by `ws* sign ws* int` terms, at most 64), and only then evaluates them left to
  right in checked `i128`; the result must fit `i64`. `Unreadable` has six kinds (`Blank`,
  `TooLong`, `Control`, `Syntax`, `TooManyScalars`, `Range`), each with a refusal message.
- `values::scalar_value` calls it for `Control::Integer`, so scalar fields and list rows, on
  create and edit, read sums. An `Unreadable` maps to `AppError::Invalid(message)`: still 422
  with the draft, "No changes saved: …" as before. `values::integer` is unchanged, and its
  test at `campaigns/tests.rs:484-486` stays.
- `templates/private_sheet.html` and `templates/private_list_field.html`: integer inputs
  have no `inputmode`. Text inputs keep `maxlength="8000"`. Native templates are untouched.
- Demo: `assist_definition()` in `tests/support/source_fixture.rs` (written as
  `assist-definition.json` by `fixture_pair`); `publish_assist_definition()` in
  `tools/smoke_release.py`; `tools/assist_demo.py` with `tools/test_assist_demo.py`.
- Browser: `run_assist_browser` in `tools/browser_smoke.py`, run in the auth workspace stage.
- Dev-dependency `proptest =1.12.0` (`default-features = false`, `std`) for the property
  tests. One networked `cargo add`/`cargo fetch` in `mimik-ci` from an allowlisted copy
  (`Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, stub targets); every later build was
  offline. New locked packages: `proptest 1.12.0`, `rand_xorshift 0.5.0`, `unarray 0.1.4`
  (all MIT OR Apache-2.0); the rest of its graph (`rand 0.10`, `regex-syntax`, `bitflags`,
  `num-traits`, `chacha20`, `core_detect`) was already locked. Dev-only; it never enters the
  release binary.

**Deviations from the plan (code reality wins, intent kept).**

| # | Plan said | Done | Why |
|---|---|---|---|
| V1 | Table row "32 terms whose running sum leaves i128 never" | 32 five-digit terms (exact), 9 × i64::MAX (leaves i64, stays in i128, `Range`), 5 adds and 4 subtractions of i64::MAX (back to i64::MAX), three 38-digit terms (the second addition overflows i128, `Range`), a 39-digit term (`Range`) | The 200-character cap admits at most 10 terms of i64::MAX, so "32 terms" and "leaves i64" cannot both hold; the cases split the two properties |
| V2 | `-9223372036854775808-1` is "Unreadable (range)" | Unreadable as `Syntax`; the range case is `0-9223372036854775808-1` | §3a's grammar has an unsigned first term in a sum, so a sign-led input with more terms is not a sum. Both are Unreadable, so the user-visible outcome (422) is the same |
| V3 | "`-5+3`" is not listed | `Syntax` (unreadable) | Same grammar rule: a sign-led input is one literal (D-Q). Recorded so 1.3 can decide whether to offer anything |
| V4 | The existing `list_values_tests::item_integers_exact_i64` was not mentioned | Updated: `01`, `+1` and ` 1` in a list row are now read (1), plus `2+1` and `84 -15`; `1.0`, empty, `9223372036854775808`, `1 2`, `84--15` and `x` are still refused; the i64::MIN edge stays | It pinned the old strict row parsing, which §3a deliberately replaces (zero padding, `+5`, trimming). It is a spec change, not a weakened check |
| V5 | `formula_tests::history_recomputes_identically` asserted `inputmode="numeric"` on a runtime integer | Asserts `value="4" disabled` | Direct consequence of scope item 4 |
| V6 | The browser check runs at 390 × 844 | It runs at both viewports (the workspace stage runs per viewport) | Superset; the 390 px run is the one the plan asks for |
| V7 | Demo command `cargo build --locked -j 2` | Same, plus a container variant for the weekday window | The machine schedule caps builds at 2 cores on weekdays |

**Red → green.**

| Test | Red (before) | Green (after) |
|---|---|---|
| `entry::tests::literals_are_their_value`, `sums_are_computed_left_to_right`, `i64_bounds`, `every_term_is_checked`, `limits_and_control_characters`, `blank_is_unreadable`, `generated_sums_match_i128` | FAILED against the stub `number` (always `Err(Syntax)`), 10:48 | pass |
| `entry::tests::unreadable_shapes`, `arbitrary_input_never_panics`, `grammar_alphabet_never_panics`, `messages_name_the_problem` | pass against the stub (a constant `Err` satisfies them) | pass |
| `entry_save_tests::scalar_sum_saves_its_result` | FAILED (422 on `84 -15`, old strict parser) | pass |
| `entry_save_tests::list_row_sum_saves_its_result` | FAILED (422 on `2+1`) | pass |
| `entry_save_tests::runtime_integer_inputs_have_no_numeric_inputmode` | FAILED (`inputmode="numeric"` present) | pass |
| `entry_save_tests::canonical_negatives_survive_a_re_post` | pass (guard: the strict parser also kept `-1`) | pass; mutation "sign-led literal unreadable" makes it fail |
| `entry_save_tests::unreadable_entry_is_still_422_with_the_draft` | pass (guard) | pass; mutation "unreadable read as 0" makes it fail |
| `entry_save_tests::native_sheets_and_protocol_integers_stay_strict` | pass (guard) | pass; mutation "`values::integer` falls back to `entry::number`" makes it fail |
| `list_values_tests::item_integers_exact_i64` (updated, V4) | — | pass |
| First green run | `every_term_is_checked` and `i64_bounds` failed on two test-design errors (V1, V2) | fixed in the tests, not the parser |

The three mutation runs (10:55–10:56, `--cpus 2`) restored the sources afterwards (checked
with `cmp`). The proptest regression file written by the stub red run was deleted: it held
the stub's failure, not a parser defect.

**Gate** (Monday 2026-10-19, inside the weekday window, so every build and test ran in a
container with `--cpus 2` and `CARGO_BUILD_JOBS=2`; image `mimik-ci:latest`, target volume
`mimik-catalogue-target`, registry volume `mimik-cargo-cache`, `--network none`):
- `cargo fmt --check` and `cargo clippy --locked --all-targets --all-features -- -D warnings`
  (pedantic): clean (10:58–10:59). The first Clippy run found two `useless_vec` in the tests.
- `cargo test --locked --all-targets --all-features` (10:59–11:07): **585 passed**, 0 failed,
  2 ignored (the pre-existing ones), 37 result lines.
- `ruff check tools`, `ruff format --check tools`: clean (11:10).
- Tools unittest (`CI=1`, `MIMIK_FORMULA_EVAL` from the built `formula_eval` example, the
  registry cache and the main repository's git directory mounted), 11:10–11:15: **757 run,
  11 skipped, OK**. The known `test_fake_runner_script` error did not occur.
- Release build (11:07–11:10) and the source fixture example (now also writing
  `assist-definition.json`).
- `smoke_release.py --auth --source-fixture` against the release binary (11:15–11:16):
  passed.
- `browser_smoke.py --auth` in `mimik-browser` (`--network=none --cpus=2 --memory=4g
  --shm-size=1g --pids-limit=1024 --cap-drop=ALL --security-opt=no-new-privileges`, the
  worktree's `tools/` mounted on `/checks`), 11:15–11:17: every stage passed, including the
  new assisted stage at 1280 × 720 and 390 × 844. A copy of the tools with the expected value
  changed to 70 failed with "The typed sum 84 -15 did not save 69", so the stage runs.
- Demo launcher (11:18, in `mimik-ci`, debug binary): `--exit-after-setup` created the sheet
  and stopped cleanly; a serving run stopped by SIGINT after 8 s shut the server down.

**Not run:** `cargo audit`, `cargo about`, packaging and the unpacked-archive checks, the LAN
smoke. **Not yet done:** the independent implementation review (Sol and Sonnet, plus the
third chain), required before merge.

**Demo commands** (from the worktree root; synthetic data only):

```
rm -rf target/assist-demo
cargo build --locked -j 2
cargo run --locked -j 2 --example source_fixture -- target/assist-demo/fixture
python3 tools/assist_demo.py target/debug/mimik --fixture target/assist-demo/fixture \
    --data target/assist-demo/data
```

The launcher prints the sheet URL (`http://127.0.0.1:3000/private-sheets/1`) and the player
and Owner logins with freshly generated dummy passwords. Open the URL, sign in as
`demo-player`, type `84 -15` into Current HP and press Enter: it saves 69. Armour stays −1
on every save. `lots` is still refused with the draft kept (unknown values arrive in 1.4).
Ctrl-C stops the server. Port 3000 must be free. `--exit-after-setup` builds the data and
stops without serving. On weekdays 09:00–18:00 the two cargo commands may run instead in
`mimik-ci` with `--cpus 2 -e CARGO_BUILD_JOBS=2` and the worktree mounted on `/workspace`.

### 9.1 Review fixes (round 1)

Review of `813a9d9`: Sonnet MERGE-READY; Sol FIX THEN MERGE (no functional defect); the third
chain was unavailable. Dispositions:

| Finding | Disposition | Test, red → green |
|---|---|---|
| Sol Minor 1: show the accumulation is checked, not wrapping | Added `entry::tests::accumulation_overflow_is_not_cancelled_by_later_terms`: with A = 38 nines, `A+A-A-A` (exactly 0) is `Range` at the second term; `A-A+A-A` is `Ok(0)` | Under a wrapping mutation (`wrapping_add`/`wrapping_sub` in `number`) only this test failed (11:27, `--cpus 2`); the existing three-38-digit case still passed, because its wrapped result is outside i64 anyway. Restored source (checked with `cmp`): 12/12 pass |
| Sol Minor 2: `Range` doc | Now: a term, or the running sum after a term, overflows i128, or the final result is outside i64; a term or the running sum may exceed i64 when the result fits | doc only |
| Sonnet Minor: demo data directory and account inputs | `fresh_directory` creates the data directory with mode 0700 (and sets 0700 on an accepted empty one). Password files and the invitation token live in `account-inputs` (0700, files 0600) only while the CLI reads them; `create_accounts` removes the directory in a `finally`, so also after a failed step. They are not needed later, because the launcher prints the generated passwords | `test_data_directory_is_private`, `test_inputs_removed_after_setup`, `test_inputs_removed_after_a_failure`: red first (mode 493 ≠ 448; `account-inputs` left behind), then green |
| Sol Major: required release gates | Run against the candidate, below. `cargo audit` found **RUSTSEC-2026-0373** in `ammonia 4.2.3` (case-sensitive CSS `url(` matching when filtering style properties). Fixed by `ammonia 4.2.4` (lockfile only, same dependency set; one networked `cargo fetch` in `mimik-ci` from an allowlisted copy, checksum verified by `--locked`) | Not reproducible in Mimik: the source sanitizer allows no `style` attribute. Guard test `sources::sanitizer_tests::style_attributes_never_survive_in_any_case` passes on 4.2.3 and 4.2.4 (11:29–11:30); recorded in `dependency-decisions.md` with the proptest selection |

**Gate on the candidate** (code commit `3aca432`; this record is folded into the same
commit afterwards and changes only docs and TODO). Monday 2026-10-19, weekday window: every
build and test in a container with `--cpus 2` and `CARGO_BUILD_JOBS=2`. Times are CEST
(container logs are UTC).

| Gate | Time | Cap | Result |
|---|---|---|---|
| `cargo fmt --check`, pedantic Clippy `-D warnings` (crate re-checked after a touch) | 12:00 | 2 | clean |
| `ruff check tools`, `ruff format --check tools` | 12:00 | 2 | clean |
| `cargo test --locked --all-targets --all-features` | 11:32–11:40 | 2 | **587 passed**, 0 failed, 2 ignored, 37 result lines |
| `formula_eval` example, `source_fixture` example | 11:40–11:41 | 2 | built; fixture includes `assist-definition.json` |
| Tools unittest (`CI=1`, `MIMIK_FORMULA_EVAL`, registry cache and git dir mounted) | 11:41–11:45 | 2 | OK, 11 skipped |
| `actionlint .github/workflows/ci.yml` | 11:45 | 2 | clean |
| `cargo audit --deny warnings` (advisory DB fetched in the container from `github.com/RustSec/advisory-db`, tip `999ec89`, 2026-10-17; then `--no-fetch`) | 11:28 / 11:30 | 2 | 1 vulnerability (above), then **clean** after the fix |
| `cargo about generate --locked --fail about.hbs` | 11:45–11:46 | 2 | passed (notices 497,325 bytes) |
| `cargo build --release --locked` | 11:46–11:48 | 2 | built (21,929,144 bytes) |
| `cargo package --locked --allow-dirty` (verify build) and `verify_source_package.py` | 11:48–11:49 | 2 | passed; "Corresponding-source member allowlist verified" |
| `cargo vendor --locked target/vendor`, `package_release.py --version 0.1.0 --revision 3aca432…` | 11:49–11:50 | 2 | archives, source crate, dependency sources and `SHA256SUMS` created |
| Unpacked artifact: `sha256sum --check`, unpack, `smoke_release.py --auth --source-fixture` on the unpacked binary | 11:50–11:51 | 2 | all three checksums OK; smoke passed |
| Offline corresponding-source rebuild (unpacked source crate plus dependency sources, empty `CARGO_HOME`, `--network=none`, `cargo build --release --offline --locked`) | 11:51–11:56 | 2 | built. Not byte-identical to the shipped binary (the build path differs); CI does not claim reproducibility |
| `browser_smoke.py --auth` in `mimik-browser` (CI flags) on the unpacked binary | 11:56–11:57 | 2 | every stage passed, the assisted stage at both viewports |
| `lan_smoke.py` (internal Docker networks, separate Chromium client, HTTPS proxy, QR decode) on the unpacked binary and source | 11:57–12:00 | 2 | passed; no containers or networks left. The script's client and server runs set `--cpus=2`; its denier and offline QR-decoder runs do not, so a local `docker` shim on `PATH` added `--cpus=2 -e CARGO_BUILD_JOBS=2` to any `docker run` without `--cpus` (machine schedule). No physical-phone or router test is claimed |

Nothing in the list was skipped.
