# Derived-field formulas: milestone M4 plan (server-rendered list editing, browser and package gates)

2026-10-16. Status: **revision 3.1, ready for execution.** Revision 2 was re-reviewed (Sol
GO-with-amendments RR1–RR5, Sonnet GO-with-amendments N1–N10, third unavailable); revision 3
resolved those findings and took in two changed user decisions (reorder and a change summary
are now in M4). Revision 3 was re-reviewed (Sol GO-with-amendments RE3-1 and RE3-2, Sonnet
GO-with-amendments N-A to N-H, third unavailable); revision 3.1 applies those amendments in
place (D5, D7, D8, Q5 and tests 20, 30–34, 42). **Revision 3.2 (implementation review,
2026-10-17)** amends D5, D7, D11 and tests 25 and 42 in place: op buttons carry no `#list-f`
fragment (it blocks autofocus; parent plan §17 negative control, accepted by all three
implementation reviewers), the submitted revision is excerpted in visible text, textareas get
a sacrificial newline, and D7 states what the page bound covers. Review records:
[reviews/2026-10-17-derived-formulas-m4-impl.md](reviews/2026-10-17-derived-formulas-m4-impl.md) and
[reviews/2026-10-16-derived-formulas-m4-plan.md](reviews/2026-10-16-derived-formulas-m4-plan.md).
Not implemented. §1 was re-verified read-only against master `39cb421` (no `src/`, `templates/`,
`tools/` or `tests/` change since `1ddbf17`). The worktree `../mimik-wt/formula` starts with
`git merge --ff-only master`. Parent plan: [derived-formulas-plan.md](derived-formulas-plan.md)
§6, §7, §9, §14.1 item 18, §16. Pattern: [M3 plan](derived-formulas-m3-plan.md) §10–§10d.

**Scope (user decisions 2026-10-16):** M4 ships **plain list editing**. Every list property the
current schema declares is editable. M4 includes Add, Remove, **Move up and Move down** draft
operations (Save persists), a 5 MiB create/edit POST limit, and a **read-only per-revision
change summary** in history. Non-editable properties and the item catalogue are a later
milestone (§8).

## 1. Current state (verified)

- **Values.** `values::list_schema` (`src/campaigns/values.rs:26-49`): array of ≤ 256 items
  (`MAX_LIST_ITEMS`, `formula/mod.rs:32`), each with a required 1–64 character `id`, the
  non-optional properties required, `additionalProperties:false`, no null.
  `private_pin::list_values` (`private_pin.rs:97-121`) requires unique registry-slug IDs and
  exact i64 integers. The 1 MiB cap in `validate_values` (`:127-131`) applies only to pins with
  a `formula_profile`. `ItemProperty` (`definitions/mod.rs:173-182`, `deny_unknown_fields`) and
  the schema (`definition-v1.json:72-79`) have no `editable` member.
- **Writer and decoder.** `snapshots::write` (`snapshots.rs:260-345`) accepts any valid slug
  as an item ID. It loads the previous row and checks the revision (`:290-293`, 409) before
  incrementing it (`:310`). It checks the schema and the encoded size (`encode`, `:50-56`,
  1 MiB, also for the frozen reserve at `:326-334`), **not** the decoder limits. Reopening uses
  `decode::parse_sheet` (`snapshots.rs:111-114`): a **32,768-node** cap (`decode.rs:38`), and
  256 keys per data object and 256 elements per array (`:80-112`).
- **Transport.** `values::form` refuses lists (`values.rs:107-111`, `:137`); `values::integer`
  (`:85-95`) takes exact decimal i64. `transport::Pairs` (`transport.rs:13-37`) is a
  `Deserialize` impl whose visitor hard-codes ≤ 516 pairs, keys ≤ 70 bytes and no decoded
  duplicates (`:25`); `parse` calls `serde_urlencoded::from_bytes` (`:40-42`). `admit`
  (`:50-59`) uses `keys.contains` per pair, so it is O(n·m), and it runs on the async worker
  after `load_editor` (`web.rs:791-815`); only `parse` is in `spawn_blocking` (`:784-788`).
- **Body limits and pre-body checks.** One `DefaultBodyLimit::max(1 MiB)` layer covers the
  whole sheet router, GET and POST alike (`web.rs:70-78`); other routes have 8 KiB (`:86`).
  Before a handler buffers its body, a request passes the 5 s `boundary` timeout and the
  `REQUESTS` permit (`auth/mod.rs:205-217`), network admission, `post_origin` (`:197-201`), the
  session layer, and `runtime`, which takes the JOBS permit (`web.rs:42-55`). Nothing checks the
  session principal before the body is read: CSRF needs the body (`web.rs:789`).
- **Save.** `save_private` (`web.rs:773-880`) validates the values and **then** checks the
  revision (`:830`), so a stale invalid draft answers 422. It serves the private runtime and
  private native sheets only; the legacy `/characters/...` editor is a separate handler in
  `src/web.rs`. `rejected_editor` (`:882-911`) re-renders with the save's own permit, except on
  the `None` path, which reacquires a permit and can answer 503 without the draft (`:896-901`).
  `rendered` marks only 409/422 responses `PrivateTrustedEditor` (`:663-668`).
- **Existing 422 assertions (N6 audit, read-only at `39cb421`).** None submits a stale revision
  to `save_private`: `formula_tests.rs:394, 479, 545, 684, 759` post the current revision (1, 1,
  2 after an edit, and 0 on create); `campaigns/tests.rs:1408` posts 0 on create; `tests.rs:2930`
  is the join-request route; `tests/sheet_layout.rs`, `foundation.rs` and `creation_flow.rs`
  use the legacy `/characters` editor. So no existing test flips; test 10 is new. Step 2
  re-runs this audit (`grep UNPROCESSABLE_ENTITY` over `src/` and `tests/`) and records the list.
- **Render.** `groups()` maps lists to `"list"` (`:602`). `private_sheet.html` has no list
  branch, so a list renders as an unsaveable `value-<list>` input (latent bug). The page has no
  JS, Name is `required` (line 6), there is one bottom `<button>Save full private sheet</button>`
  with no `name` or `value` (line 9), and the "Computed from saved revision N" heading is in the
  template (line 10). The test helper `submitted` (`formula_tests.rs:794-827`) ignores submit
  buttons and disabled controls.
- **Publication.** `publishable` (`publication.rs:139-152`) refuses lists. The test bypass has
  three parts: the thread-local `UNSHIPPED_FOR_TESTS` (`:12-17`), `publish_unshipped_for_tests`
  (`:19-23`, re-exported at `definitions/mod.rs:16`; one caller, `formula_tests.rs:629`, which
  seeds IDs `i-{n}`) and the early return (`:140-143`). `publication_tests.rs:188-217` asserts
  the refusal. **No shipped database holds a list sheet.** The admitted maxima are 256 fields,
  32 lists, 32 properties and 128 options (`formula/mod.rs:37-40`).
- **History and pack.** `/history/{rev}` (`read_history`, `web.rs:700-707`) reuses the sheet
  template read-only through `private_read` (`sheets.rs:90`), which loads one revision; there is
  no diff. The `{list, entry_id, field}` address (parent §6) is used only by
  `tools/tool_loop_benchmark.py:587-591`. The pack has `equipment` (name, kind enum of 8,
  quantity, weight, equipped, optional armor, optional category enum of 4) and `conditions`
  (kind enum of 4, marked `editable: false`, and stacks). The translator
  (`src/definitions/formula_pack_tests.rs:37-108`) drops `editable`. No expression reads
  `item.id`.
- **Dependencies.** `serde_urlencoded 0.7.1` exposes `Deserializer::new(form_urlencoded::Parse)`;
  `form_urlencoded 1.2.2` is already locked. `html5ever` 0.39 and 0.40 are already locked (via
  `html2text` and `ammonia`); `cssparser 0.38` is locked and accepted in `about.toml` as MPL-2.0.
  `scraper` is not locked. In the local registry, `scraper 0.25.0` (ISC) needs `selectors 0.33`
  (**MPL-2.0**), `cssparser 0.36`, `html5ever 0.36` and `servo_arc 0.4.3` (**MIT OR
  Apache-2.0**).
- **Smoke.** `smoke_release.py:943-984` publishes two versions, and the generic step picks
  `versions[-1]` (`:1132`). `browser_smoke.py` locates "Save full private sheet" by exact name
  (`:461`–`:661`). The CI browser runs with `--network=none --cpus=2` (`ci.yml:114-120`).

## 2. Design

**D1. Form encoding.** For list field index `f` (its index in `definition.fields`, stable
because pins are immutable), row `r` (0-based, contiguous) and property index `p`:
- `id-f-r` is hidden: the saved item ID, or empty for a new draft row;
- `row-f-r-p` is the value of property `p`;
- `set-f-r-p` is `present` / `absent` (optional properties only);
- `op` comes only from the clicked submitter: `save`, `add-f`, `remove-f-r`, `up-f-r` or
  `down-f-r`.

Indices use the canonical grammar `0|[1-9][0-9]{0,2}`. `f` must name a list field, `r < 256`,
and `p` must be below that list's property count. The longest key, `set-255-255-31`, is 14 bytes.
Rows are stored in ascending `r`, as submitted, so the submitted row order is the saved order.

**D2. Parsing and admission: O(n), set-based, after CSRF and authorization.**
- **Pair limit as a seed (N1).** The `Pairs` visitor takes its limit from a
  `PairsSeed { limit: usize }` that implements `serde::de::DeserializeSeed`. `parse_limited`
  runs it as `PairsSeed { limit }.deserialize(serde_urlencoded::Deserializer::new(
  form_urlencoded::parse(bytes)))`; `form_urlencoded` becomes a direct dependency at the
  locked 1.2.2 (no new crate, no fetch). `Pairs::parse` is `parse_limited(bytes, 516)`; sheet
  POSTs use `SHEET_FORM_PAIRS = 65_536`. The decoded-duplicate and 70-byte checks are unchanged.
  A unit test (test 8) checks that the limit comes from the seed: limits 3 and 516, a pair over
  each, and `parse` still refusing 517.
- **Order (N2).** `save_private` keeps parse → CSRF → `request_auth` → `load_editor` →
  `editable`. Admission then runs in **its own** `spawn_blocking` step, separate from the
  validation closure at `web.rs:818`. It returns `Result<BTreeMap<..>, StatusCode>`, so a bad
  key set answers 400 and never reaches `rejected_editor`. A CSRF failure or a caller without
  edit rights still answers 403 whatever the key set is (test 11).
  1. One scan of the `id-` keys parses their indices. A bad grammar, a non-list `f`, `r ≥ 256`
     or a gap → 400. This bounds `n_f ≤ 256` **before** any key is generated.
  2. The expected keys go into a `HashSet<String>`: the scalars, the rows `0..n_f`, and an
     optional `op`.
  3. The form is admitted only if the counts match and every pair is in the set; otherwise → 400.
- `Pairs::admit` takes that set, so every route (`web.rs:201`, `save_private`) shares one path.

**D3. Host-managed IDs, enforced by the writer (decided: deterministic format).**
- A new row's ID is `r<N>-<k>`: N is the revision being written, and k = 1..m numbers the
  **newly allocated** rows of that list in row order. The scope is one list of one sheet.
- **Writer invariant** (`snapshots::write`, kind `runtime`, `create` and `edit` only; `freeze`
  already requires identical data). It runs **after** the previous row is loaded and the
  revision-conflict check passes (`snapshots.rs:290-293`), and it uses the post-increment
  revision `previous.checked_add(1)` (N7). Every ID absent from the previous revision's list
  (empty on create) must be `r<previous + 1>-<k>`, with k contiguous from 1. So a stale direct
  write answers 409, never 422; there are no client-chosen IDs; and a removed ID never returns.
- IDs label **row slots, not item identity**. Overwriting every property of a row, or a forged
  swap of hidden IDs between rows, is accepted and documented in the contract. A move (D5)
  carries a row's ID with it. Item identity belongs to §8. Future import and tool-loop writers
  must re-ID through the host.
- `values::form` reconciliation: an empty ID gets the next `r<N>-<k>` (N = the current revision
  + 1); a non-empty ID must exist in the saved list at the current revision; duplicates → 422.

**D4. Status precedence (`save_private`: private runtime and private native saves).** The legacy
`/characters` editor is unchanged. In order:
1. 413 body limit (D7: the pre-body principal check runs first on the 5 MiB routes and answers
   403 without reading the body); 400 parse (pair cap, key length, duplicates).
2. 403 CSRF; authorization (`load_editor`); 403 if not `editable`.
3. 400 admission (D2), op grammar and op range (including an edge move, D5).
4. An op (a non-integer `revision` → 400) → 200 draft re-render (D5); Add at 256 rows → 422.
   No value, ID or revision-match validation. The echo cap (D7) applies.
5. Save: a malformed `revision` → 422; **revision ≠ current → 409**. This now runs before values,
   so a stale draft with invalid values or unknown IDs answers 409. That is a deliberate change
   from `web.rs:830` for scalar runtime and private native sheets too, documented in the contract.
6. 422: `values::form`, reconciliation (an unknown ID at the current revision is tampering),
   `validate_values`.
7. Writer: 409 for a concurrent writer (checked before the ID invariant, D3); 422 for an
   undecodable snapshot (D7) or the ID invariant.
- Every re-render (op, 409, 422) keeps the **submitted** hidden revision; nothing rebases a draft.

**D5. Draft operations (user decisions: Add, Remove, Move up and Move down re-render an unsaved
draft; Save persists).**
- Buttons, each with visible text that includes the row number (WCAG 2.5.3):
  - `add-f`: "Add <List label> row";
  - `remove-f-r`: "Remove row R of <List label>";
  - `up-f-r`: "Move row R of <List label> up"; `down-f-r`: "Move row R of <List label> down".
  All op buttons carry `formnovalidate`, so the `required` Name and half-typed rows never block
  an op; Save keeps browser validation.
- **Edges.** Every row renders both move buttons, so the button row never shifts under the
  pointer. Row 1's Move up and the last row's Move down are `disabled` (excluded from
  submission; no claim is made about how assistive technology announces them, N-G). A submitted edge move (`up-f-0`, `down-f-(n−1)`) or any `r ≥ n`
  is forged → 400.
- An op parses the draft into `ListDraft` (raw strings, never validated), applies itself and
  re-renders with **200** in `FormulaMode::Draft` (never evaluated). Add appends an empty row
  with an empty ID; Remove deletes row r; a move swaps row r with its neighbour, values, `set-`
  states and hidden ID together. `auth::normalize` already sets `no-store` on every response
  (`auth/mod.rs:426-429`; test 20 guards it). The Add-at-256 422 goes through `rendered`, so it
  carries `PrivateTrustedEditor` and is not replaced by the error document. The page shows a
  `role="status"` message: "Row added to Equipment. Not saved yet.", "Row 3 of Equipment
  removed. Not saved yet.", "Row 3 of Equipment moved up to position 2. Not saved yet." An op
  never writes, never reveals saved IDs the draft lacked, and still needs CSRF, authorization,
  `editable` and the JOBS permit.
- At 256 rows the Add button is disabled with "limit reached" text. If the revision differs from
  the current one, an advisory warning reads "The saved sheet changed (revision N → M); saving
  this draft will be refused."
- **Focus and scroll (revision 3.2).** Op buttons post to the form's own action with **no URL
  fragment**: Chromium does not run autofocus when the URL fragment names an element (parent
  plan §17: the browser stage fails with `#list-f` and passes without it), so the earlier
  `formaction="<action>#list-f"` was dropped at implementation review; the autofocused control
  scrolls into view instead. Exactly one `autofocus` per page:
  - after Add, on the new row's first control;
  - after Remove, on the list's Add button;
  - after Move up, on the moved row's Move up button at its new position, or on its Move down
    button if it is now first; after Move down, symmetrically.
  - The focus target's accessible name carries the row's **new** position, because every
    button name includes the current row number ("Move row 2 of Equipment up" after row 3 moved
    up), together with the `role="status"` message (N-G).
- **Layout.** The row's button group (Move up, Move down, Remove) is a flex-wrapping row. Each
  target is at least 24 × 24 CSS px (WCAG 2.5.8). At 390 px the 32-property row and its buttons
  must not overflow (browser `no_overflow`, step 8).

**D6. Implicit submission.** List sheets also render a `<button name="op" value="save">Save full
private sheet</button>` **before** the groups, so Enter saves (the default button is the first
submitter in tree order). The bottom button also gets `name="op" value="save"`. Scalar sheets
keep a single button and their browser locators; list-stage locators are scoped.

**D7. Limits, large forms and the publication budget.** Owners: rows, `MAX_LIST_ITEMS` (schema,
`values::form`, D2); i64, slugs and uniqueness, `values::integer` and `list_values`; text
length, the schema (8,000 / 20,000) plus `maxlength`; stored bytes, `snapshots::encode`
(1 MiB); decodability, `snapshots::encode`; render units, `values::form_budget` called by
`publishable`.
- **Writer decodability (Sol B3).** `encode` re-parses its output with `decode::parse_sheet`.
  On failure it returns `Invalid("Private snapshot exceeds the sheet decoder limits.")` → 422,
  atomically. The frozen reserve is checked the same way, so an active sheet can always freeze.
  The decoder cap is not raised.
- **Publication budget (RR2).** `form_budget(definition)` returns three counts for full lists
  with every optional property present, and a pure `within(counts, limits)` refuses any count
  over its limit with `list-form-exceeds-limit`:
  - render units = Σ scalars (1 + choices, + 2 if nullable) + Σ lists (4 + 256 × (4 +
    Σ props (1 + choices, + 2 if optional))) ≤ `RENDER_UNITS = 32_768`. Choices are the enum
    options + 1, or 3 for a boolean. A unit is one control, option or row element with its
    label, so its markup is bounded (labels ≤ 100 characters, escaped);
  - pairs = 5 + Σ non-derived scalars (1, or 2 if nullable) + Σ lists 256 × (1 + props +
    optional props) ≤ `SHEET_FORM_PAIRS`;
  - stored-JSON nodes = `ENVELOPE_NODES` + 1 + Σ non-derived scalars + Σ lists (1 + 256 × (2 +
    props)) ≤ `decode::MAX_NODES` (the 32,768 literal becomes a shared const).
  - **Only render units bind under the admitted maxima.** Per term, units ≥ pairs (a list row
    costs 4 + p + 2·opt units and 1 + p + opt pairs; a scalar costs ≥ its pairs), so pairs ≤
    units + 5 ≤ 32,773 < 65,536. For a definition with a list, units − nodes ≥ 515 −
    `ENVELOPE_NODES` − 1 > 0 while `ENVELOPE_NODES` < 514; without lists, nodes ≤
    `ENVELOPE_NODES` + 257. The pair and node checks are **redundant safeguards**: they are
    tested through `within` with small test limits and by a dominance test over extremal
    definitions, not by real publication at their limits, which is unreachable.
  - `ENVELOPE_NODES` bounds the snapshot's non-data nodes. A test counts them on the maximal
    snapshot with the decoder's own counter and asserts ≤ the constant; the writer check stays
    authoritative.
  - The pack is about 10,000 units, 3,400 nodes and 3,400 pairs. Four lists × 256 rows × 32
    integers is 34,820 nodes, about 250 KB of JSON, and is refused by both the render and node
    budgets (and by the writer if it were reached directly).
- **Body limit 5 MiB, on the create and edit POST methods only (step 7).** Stored values are
  ≤ 1 MiB, URL encoding expands them ≤ 3×, and keys add ≤ 65,536 × 16 bytes: ≤ 4 MiB, plus
  margin. **Layering (N10):** the router-wide 1 MiB layer at `web.rs:76` stays; the two POST
  method handlers get their own inner layer, `post(create_sheet.layer(DefaultBodyLimit::max(
  SHEET_POST_BYTES)))` and the same for `edit_sheet`, built by one helper. The inner layer sets
  the limit after the outer one, so the POST extractor sees 5 MiB and the GET handlers (which
  extract no body) keep the router value. GET body limits are not observable through the current
  handlers, so no test or mutation claims them (RR3).
- **Pre-body principal check (N3).** On the two 5 MiB POST methods, a `route_layer` middleware
  reads the session principal (cookie only, no body) before the body extractor runs. Without a
  principal it answers 403, the status an unauthenticated save gets today, and never polls the
  body. CSRF is still checked after buffering. **Accepted residual risk:** any logged-in
  principal, editor or not, can make the server buffer ≤ 5 MiB per request without a valid
  CSRF token, holding one `REQUESTS` permit and the JOBS permit (taken by `runtime` before the
  body) for at most the 5 s `boundary` timeout. The LAN admission and `post_origin` checks still
  apply first. §6 measures that hold time for a slow 5 MiB upload. Moving the JOBS permit after
  body buffering is a recorded follow-up, not M4.
- **Accepted limit: the 5 s `boundary` timeout (N-H).** The whole request, body included,
  must arrive within the 5 s `boundary` timeout (`auth/mod.rs:217`). A 5 MiB body therefore
  needs about 8.4 Mbit/s of sustained upload (5 × 8 × 1.048576 / 5 s). Slower clients get the
  timeout response and nothing is saved. This is recorded as an accepted limit of M4, not
  raised; the realistic pack form is about 100 KB.
- **Echo cap (N8).** A re-render (op, 409 or 422) echoes the draft only if its decoded values
  total ≤ `ECHO_BYTES = 1 MiB`. A larger draft could never be stored: JSON encoding only adds
  bytes, so its snapshot would exceed the 1 MiB writer cap. Such a draft answers 422 with the
  saved revision's values, no echo, and a `role="alert"`: "This draft is larger than the 1 MiB
  sheet limit and was not kept. Nothing was saved; copy large text before submitting." Every
  response is `no-store`, so the plan promises no Back-button recovery (N-B).
- **Echo multiplicity (RE3-1).** Each submitted string is echoed **in full at most once**. The
  contexts that repeat a value are bounded instead:
  - an invalid enum, boolean, known/unknown or Set/Not set value is echoed in full once, as the
    extra option's `value`, and its visible text shows at most the first 64 characters plus
    "…";
  - a row's hidden ID is echoed in full once, as the hidden `value`; the row legend shows at
    most the first 64 characters (a valid ID is ≤ 64);
  - text inputs, textareas, Name and General notes are echoed once;
  - the submitted revision is echoed in full once, in the hidden control; the page's
    "submitted/form revision" text shows a 64-character excerpt (revision 3.2, Sol Major 1).
  Every textarea emits one sacrificial newline after its opening tag, because HTML parsing
  drops the first newline there; values that start with newlines show and resubmit
  intact (revision 3.2, Sol Major 2). Separately, browsers serialise textarea line breaks as
  CR LF on submission; the sheet transport normalises CR LF and CR to LF before validation, so
  canonical storage is LF and an unchanged Save is byte-identical (final implementation
  review).
  So a re-render is ≤ `FIXED_PAGE_BYTES` + units × `UNIT_MARKUP_BYTES` + 5 × `ECHO_BYTES`
  (Askama escapes `"` as the 5-byte `&#34;`, the widest escape), where `UNIT_MARKUP_BYTES`
  includes one 64-character excerpt escaped (≤ 320 bytes). Step 7 derives both constants from
  the template and test 42 asserts the bound on a draft that fills the maximum-amplification
  contexts (invalid enum and boolean values, invalid states and hidden IDs made of `"`), so a
  body of 5 MiB of quotes cannot produce a 25 MiB page. A save over 1 MiB stored, but within
  the echo cap (or over the profile cap), → 422 with the draft kept.
- **What the page bound covers (revision 3.2, third reviewer F2).** `FIXED_PAGE_BYTES` covers
  the editor chrome only (headings, pin JSON, notices, the Name and notes controls), and the
  units cover the input controls, options and row elements. It is **not** a bound on the
  formula sections of a formula definition: derived-field expressions, check messages and
  explanations are bounded by the definition's 256 KiB content cap (each rendered at most
  twice, escaped at ≤ 5 bytes per character), and each derived value by its field's text
  limit (20,000 characters). The tests that assert the bound use formula-free fixtures.

**D8. History: ID-labelled rows and a read-only change summary (user decision Q4).**
- **Rows.** `/history/N` shows each row as a read-only fieldset, legend "Row 2 · ID r1-2",
  disabled controls (M3 L1), no op buttons and no top Save. The contract defines the address
  textually as `<list id>/<entry id>/<property id>`, scoped to one sheet and its immutable pin.
  It uses IDs, never form indices.
- **Change summary**, a section "Changes in revision N" above the rows, for runtime sheets with
  a supported pin. `private_read` loads revision N − 1 from `private_sheet_history` in the same
  read transaction. `history_diff::diff(prev, cur, definition)` runs host-side, in the existing
  blocking read path under the JOBS permit, and compares stored data only (derived results are
  recomputed, not diffed).
- **Revision N − 1 is validated like N (N-C).** It goes through the same `RawSnapshot::decode`
  (bounded parse, integrity, hash and event correspondence), the same identity checks against
  the sheet row, `match_pin` against the same immutable pin, and `validate_values`. Any failure
  of N − 1 degrades the section to "Change summary unavailable for this revision." and never
  fails the page; revision N itself keeps its current checks. Entries, in this order:
  - name and general notes changed (labels only);
  - scalar fields changed: one entry per field whose stored value differs, including known ↔
    unknown (null), shown by label and field ID;
  - per list, matching rows by ID:
    - **added**: IDs in N absent from N − 1, `equipment/r3-1`;
    - **removed**: IDs in N − 1 absent from N;
    - **edited**: retained IDs whose properties differ, a set ↔ not set change included, with
      the changed property labels, `equipment/r1-2/quantity`;
    - **moved**: retained rows outside one longest increasing subsequence of their N − 1
      positions taken in N order. The moved set is a **deterministic minimal explanation of
      the net order change**, not a claim about which row the user clicked (RE3-2). Ties are
      broken so that the rows reported as moved are the ones that ended up **earlier**: among
      the maximum-length subsequences, the one whose N positions are lexicographically
      **greatest** is kept (built greedily: the largest feasible N position first, then the
      largest feasible next one). Consequences, pinned in test 30:
      - moving one row to the top reports exactly that row (`[A,B,C,D]` → `[D,A,B,C]`: D);
      - an adjacent swap reports the row that moved up (`[A,B]` → `[B,A]`: B;
        `[A,B,C]` → `[B,A,C]`: B; `[A,B,C]` → `[A,C,B]`: C), which is also the row a single
        Move up click moves;
      - moving one row to the bottom by two or more places reports that row (`[A,B,C]` →
        `[B,C,A]`: A).
  - **Reused slots.** IDs are slot labels (D3), and a removed ID never returns, so an ID always
    names the same slot. For a list with **two or more properties**, when every property of a
    retained ID changed, the entry reads "Row r1-2 of Equipment: all properties changed (the row
    slot may have been reused)". For a one-property list a changed row is an ordinary edit line
    with neutral text, because changing its only property is not evidence of reuse (N-A). The
    section notes that row IDs label positions in this sheet's history, not items.
  - Revision 1 (create) reads "Sheet created with K rows in L lists"; a freeze reads "Frozen; no
    value changes". An unsupported pin, or a native sheet, shows "Change summary unavailable for
    this sheet."
- **Markup (N-G).** The summary is `<section id="change-summary" aria-labelledby=
  "change-summary-heading">` with an `<h2 id="change-summary-heading">`, one `<ul>` of `<li>`
  lines, and every address in a `<code>` element. Test 32 asserts this structure on the parsed
  DOM.
- **Bounds (N-E).** At most `SUMMARY_LINES = 200` lines are rendered, in the order above; each
  line lists at most **8** property labels with their addresses, then "and M more". Past the
  cap, one overflow line reads "and K more changes" with exact per-list counts of added,
  removed, edited and moved rows, which are always complete. The bound is derived per part,
  with every label 100 characters escaped at 5 bytes (`&#34;`, ≤ 500 bytes) and IDs ≤ 64 ASCII
  bytes:
  - one line ≤ the list label (500) + 8 × (a property label 500 + its address ≤ 194 + markup
    ≤ 40) + the entry address (≤ 129) + fixed text and markup (≤ 400) = 6,901 bytes, so
    `SUMMARY_LINE_BYTES = 8,192`;
  - the overflow line ≤ 32 lists × (label 500 + four counts and fixed text ≤ 200) + 100 =
    22,500 bytes, so `SUMMARY_OVERFLOW_BYTES = 32,768`;
  - the heading, the slot note and the link ≤ `SUMMARY_FIXED_BYTES = 4,096`;
  - so the section is ≤ `SUMMARY_BYTES` = 200 × 8,192 + 32,768 + 4,096 = **1,675,264
    bytes**, the exact constant test 34 asserts.
  The diff is O(rows × properties) per list plus O(n²) for the subsequence, n ≤ 256 (at most
  65,536 steps).
- It is read-only: no revert, restore or copy-to-draft action. Everything is escaped by Askama
  defaults (D11). Values are not shown; the full revision is on the same page, and a link goes
  to `/history/N−1`.

**D9. Derived values.** No engine change. Derived fields cannot be lists. Op and rejected
re-renders show the saved revision's results in the M3 block. `item.id` reaches formulas as
the host ID.

**D10. Markup and accessibility.** A list is `<fieldset class="list-field" id="list-f">` with
its label as the legend. Each row is a nested `fieldset id="row-f-r"`, legend "Row R of N · ID x"
or "New row · ID assigned on save". Every control has a `<label for>`; IDs are unique per page.
Optional properties get a labelled Set/Not set select (decided: explicit state), and required
boolean and enum selects start on "Select a value". An empty list shows "No rows yet.". No
tables: rows stack, with `fieldset{min-width:0}` and the flex-wrapping button row (D5).

**D11. Escaping and the DOM test helper.** Askama defaults; no `|safe` (the source check stays).
Witnesses go in their real contexts: `</textarea>` in item text; `"` and `'` in attribute
values (the hidden-ID echo, text `value=`, invalid enum and Set/Not set echoes); `<script>` in
list, property and option labels, in the status message and in change-summary labels. These run
on the view, op, 422, 409, history and change-summary pages. No op button carries a
`formaction`; the form action must equal the generated same-origin path (revision 3.2).
- **Parser selection (N9, RR1).** Tests parse the DOM with the maintained `scraper` crate, a
  dev-dependency only, adopted at the start of step 3 with a recorded selection note. At
  adoption, pick the newest `scraper` release whose `html5ever` matches an already-locked
  version if one exists, to avoid a third copy. Its new licences: `selectors` is MPL-2.0 and gets
  a per-crate `about.toml` acceptance with the same rationale as the existing `cssparser` entry
  (and `cssparser` is re-checked if a second version enters); `servo_arc` is MIT OR Apache-2.0
  and needs no exception. `cargo about` and `cargo audit` cover it because dev-dependencies are
  not ignored (`about.toml:4`).
- **One-time fetch.** Adding a crate needs one networked `cargo fetch` (or `cargo update -p`) in
  the `mimik-ci` container with an allowlisted context, recorded in the step-3 commit with the
  new `Cargo.lock` checksums. After it, every build and test is offline again; the rule becomes
  "`cargo audit` plus the recorded one-time dependency fetch are the only networked runs".
- **Fallback.** If the licence or audit check fails, the fallback is a dev-only DOM walk over
  the already-locked `html5ever` tokenizer, recorded as a demonstrated gap; it needs no fetch.
- The `submitted` helper is rebuilt on the parser in step 3. It includes exactly the clicked
  submitter's `name=value` and excludes disabled controls (including those inside a disabled
  fieldset) and the other buttons.

## 3. Decisions and open questions

User decisions (2026-10-16): Q1 Add, Remove and moves re-render an unsaved draft, and Save
persists; Q2 Move up and down are in M4 with tests and accessibility; Q3 a 5 MiB create/edit
POST limit with the render budget; Q4 a read-only per-revision change summary in M4.

Decided by review, with no product trade-off: explicit Set/Not set for optional properties,
deterministic `r<N>-<k>` IDs, the D4 precedence, the pre-body principal check, and the
render-unit budget as the only binding publication count.

Open (the plan applies the default until answered):
- **Q5 Oversized drafts.** A draft over 1 MiB decoded (which can never be saved) answers 422
  without echoing it, plus a notice to copy large text before submitting (D7). *Default: yes.* The alternative is to echo
  it and accept pages up to about 30 MiB.
- **Q6 Summary content.** The change summary names what changed (labels and addresses), not the
  old and new values (D8). *Default: names only.* The alternative is truncated values, which
  needs its own bound.

## 4. Failing-first tests (in order)

"Guard" = passes when written; it is killed by the named mutation. Every other test runs red
first with the stated failing assertion, after a paired positive baseline on the same fixture
passes, so it is red for the intended reason. "Compile red" = the test names an item that does
not exist yet. Tests 1–11 use direct fixtures (hand-built snapshots, `Pairs` and form vectors);
from step 3, page tests submit through the rebuilt `submitted` helper. Route tests publish
through the bypass until step 8.

| # | Step | File | Test | Red because (failing assertion) |
|---|---|---|---|---|
| 1 | 1 | `campaigns/list_tests.rs` (new) | `writer_refuses_client_chosen_new_ids` (create and edit) | `write` returns Ok |
| 2 | 1 | same | `writer_refuses_removed_id_reintroduced`; `…_wrong_or_noncontiguous_k` (`r2-2` alone; `r2-2,r2-1`) | Ok |
| 3 | 1 | same | `writer_accepts_mixed_retained_new_rows_per_list_reuse_and_swaps` (new IDs `r<previous+1>-k`) | guard |
| 4 | 1 | same | `departure_and_removal_freeze_keep_arbitrary_valid_ids`; `stale_write_with_bad_ids_is_409_not_422` | guard |
| 5 | 1 | same | `writer_refuses_undecodable_snapshot_atomically` (4 lists × 256 × 32 integers) | Ok; reopen then fails |
| 6 | 1 | same | `decoder_bound_snapshot_writes_reopens_and_freezes` (3 lists) | guard |
| 7 | 2 | `campaigns/tests.rs` | `list_row_keys_admit` (unit: `sheet_keys` + `admit`); then `…_rejects_gaps_aliases_overflow_substitution` (`id-01-0`, `id-+1-0`, non-list `f`, `r=256`, same-count swap, `%2D` duplicate, 70/71 bytes) | positive: 400, list keys unknown. Negatives: guards before step 2 (no list key is admitted), run against the mutation "admit any well-formed list key" |
| 8 | 2 | `transport.rs` tests | `parse_limited_takes_its_limit_from_the_seed` (limits 3 and 516; `parse` refuses 517; 65,536 for sheets) | compile red |
| 9 | 2 | same | `admission_of_65536_pairs_is_linear` (fixture `Pairs::from_pairs` `#[cfg(test)]`, bypassing the parser; debug, < 5 s) | old `contains` admission > 5 s |
| 10 | 2 (lists: 3) | `campaigns/tests.rs` | `stale_invalid_draft_conflicts` (scalar runtime, private native; list case added in step 3) | 422 ≠ 409 |
| 11 | 2 | same | `bad_keys_after_csrf_and_authorization` (bad CSRF + bad keys → 403; non-editor + bad keys → 403; good + bad keys → 400) | guard |
| 12 | 3 | `values.rs` tests | `list_rows_parse_in_order_new_rows_get_host_ids`; `optional_state_is_explicit`; `item_integers_exact_i64`; `unknown_or_duplicate_ids_refused`; `257_rows_refused` | `form` returns the M2 refusal |
| 13 | 3 | `list_tests.rs` | `list_sheet_rendered_form_round_trips` (303, revision + 1, > 516 pairs) | renders `value-<list>`, 422 |
| 14 | 3 | same | `list_rows_render_as_labelled_fieldsets_unique_ids` | no `fieldset.list-row` |
| 15 | 3 | same | `failed_save_rerender_keeps_draft_and_submitted_revision` | the 422 page has no `row-` controls |
| 16 | 3 | same | `submitted_helper_sends_only_the_clicked_submitter` (helper unit test on a parsed page) | compile red |
| 17 | 3 | same | `first_submitter_in_tree_order_is_op_save_before_groups` (includes `input type=submit`) | the first submitter has no `name="op" value="save"` and follows the groups |
| 18 | 4 | same | `competing_writer_loser_unchanged`; `op_409_op_save_never_rebases` | `op=add-0` answers 400 |
| 19 | 4 | same | `add_rerenders_200_no_store_without_writing`; `remove_rerenders_without_writing` | 400 |
| 20 | 4 | same | `move_up_down_rerender_new_order_without_writing`; `move_carries_id_values_and_set_state` (an invalid text value is kept through the move); `save_after_moves_persists_order`, with new rows moved around retained rows: the new IDs `r<N>-k` follow the **final** row order (N-D) | 400 |
| 21 | 4 | same | `edge_move_buttons_disabled`; `edge_or_out_of_range_move_is_400` | buttons absent; the 400 cases are guards, run against the mutation "edge move accepted" |
| 22 | 4 | same | `add_at_256_is_422_with_message`; `bad_op_or_index_is_400`; `two_ops_is_400` | Add at 256 answers 400, not 422; the others are guards |
| 23 | 4 | same | `ops_need_csrf_and_edit_rights`; `stale_op_warns`; `ops_never_evaluate_draft`; `op_never_reveals_saved_ids` | the warning text is absent; the CSRF and rights cases are guards |
| 24 | 4 | same | `unknown_id_draft_op_rerenders_then_save_422` (current) and `409` (stale) | the op answers 400 |
| 25 | 4 | same | `op_buttons_have_formnovalidate_and_no_fragment` (revision 3.2; was `…_and_formaction`); `single_autofocus_after_add_remove_and_each_move` (incl. a move to the edge) | attributes absent |
| 26 | 4 | same | `op_button_names_and_status_messages` (row number in each name; three message forms) | absent |
| 27 | 4 | same | `create_page_adds_rows_then_creates` | the op answers 400 |
| 28 | 5 | same | `list_history_rows_have_stable_ids_and_no_ops` (incl. the test 6 snapshot); `history_address_grammar` | no row fieldsets in history |
| 29 | 5 | `campaigns/history_diff.rs` tests (new) | `diff_added_removed_edited_scalars_name_notes`; `diff_optional_presence_is_an_edit` | compile red |
| 30 | 5 | same | `diff_moved_is_lis_with_deterministic_ties` (one row to the top; to the bottom by two; the RE3-2 cases `[A,B]`→`[B,A]`, `[A,B,C]`→`[B,A,C]` and `[A,B,C]`→`[A,C,B]` with their pinned results) | compile red |
| 31 | 5 | same | `diff_reused_slot_all_properties_changed` (two properties), `diff_one_property_list_change_is_a_plain_edit` (neutral text, N-A); `diff_create_and_freeze_lines` | compile red |
| 32 | 5 | `list_tests.rs` | `history_page_shows_change_summary_for_revision_n_against_n_minus_1` (and a link to N − 1; DOM: `section[aria-labelledby]` → `h2`, one `ul`, addresses in `code`, N-G) | no summary section |
| 33 | 5 | same | `change_summary_unavailable_for_native_and_unsupported_pins`, and for a corrupt or pin-mismatched revision N − 1 (the page still renders N, N-C) | no summary text |
| 34 | 5 (again 8) | same | `change_summary_large_diff_is_bounded` (all rows replaced in all lists; labels of 100 `"`): ≤ 200 lines + the overflow line, exact counts, ≤ 8 labels per line, section ≤ `SUMMARY_BYTES` = 1,675,264 | no summary; in step 8 rerun on the exact-budget fixture |
| 35 | 5 | same | `derived_values_over_lists_update_after_save` | guard (no engine change, D9) |
| 36 | 5 | same | `list_payloads_escaped_in_context` (D11, six pages incl. the summary; DOM-parsed) | guard |
| 37 | 5 | existing native and scalar tests | view, save and browser locators unchanged | guard |
| 38 | 6 | `tools/test_smoke_release.py` | form chosen by version ID and logged, not `versions[-1]` | picks last |
| 39 | 7 | `list_tests.rs` | `sheet_post_accepts_5mib_and_413s_5mib_plus_1`; `non_sheet_posts_keep_8kib` | 413 at 1 MiB |
| 40 | 7 | same | `unauthenticated_large_post_is_403_without_reading_the_body` (a counting body stream: ≤ 64 KiB polled) | the whole body is read before 403 |
| 41 | 7 | same | `stored_over_1mib_is_422_with_draft` | 413 at 1 MiB |
| 42 | 7 | same | `oversized_draft_is_not_echoed` (5 MiB of `"` → 422, the exact alert wording "Nothing was saved; copy large text before submitting", page ≤ bound); `one_mib_of_quotes_is_echoed_within_bound`, with the quotes placed in the maximum-amplification contexts (invalid enum and boolean values, invalid states, hidden IDs) and the 64-character excerpts asserted (RE3-1) | the page echoes the whole draft |
| 43 | 8 | `publication_tests.rs:188` renamed | `list_definition_publishes` | `list-editing-unsupported` |
| 44 | 8 | `publication.rs` tests | `within_refuses_each_count_with_test_limits` (pairs, nodes and units each over its own small test limit, the others under); `form_budget_counts_match_arithmetic` | compile red |
| 45 | 8 | same | `render_budget_boundary_publishes_and_plus_one_refuses` (real publication); `pairs_and_nodes_are_dominated_by_render_units` (extremal definitions: 32 lists × 32 props, all integers, all enums of 128, all optional; no lists) | compile red |
| 46 | 8 | `list_tests.rs` | `exact_budget_sheet_saves_reopens_and_freezes` (the test 45 boundary definition; every row and optional present, values up to the stored cap; non-data nodes ≤ `ENVELOPE_NODES`) | refused by the gate |
| 47 | 8 | same | `lantern_reach_list_sheet_end_to_end` (real publication; pack IDs re-ID'd to `r1-k`; derived results unchanged) | refused by the gate |
| 48 | 8 | `tools/browser_smoke.py` | list stage with a negative control | — |

**Mutations for reviewers** (each must be killed; the killing test is named):

| Mutation | Killed by |
|---|---|
| writer invariant skipped; wrong `k` accepted | 1; 2 |
| invariant checked before the revision conflict; invariant uses the pre-increment revision | 4; 1 and 3 |
| writer decode check removed | 5 |
| reconcile accepts an unknown or duplicate ID; `set-` ignored | 12 |
| admission accepts a row gap, or any well-formed list key | 7 |
| admission reverted to `contains` | 9 |
| the seed ignored (hard-coded limit); the sheet cap reverted to 516 | 8; 13 |
| admission moved before CSRF or authorization | 11 |
| the revision check moved back after values | 10 |
| the op path writes; an op silently does nothing | 19, 20 |
| a move swaps the wrong neighbour, or loses the ID or `set-` state | 20 |
| an edge move accepted | 21 |
| the draft is evaluated; an op skips CSRF | 23 |
| `formnovalidate` dropped; a second `autofocus` | 25, 48 |
| the first button is not Save; the top Save has the wrong `op` | 17 |
| `|safe` on item text or a summary label | 36 |
| a dropped kept permit; a re-render that rebases the hidden revision | existing `REJECTED_EDITOR_KEPT_PERMIT` test, 15; 18 |
| the summary compares N with N − 2; `moved` by index instead of the subsequence; the tie-break reversed | 32; 30; 30 |
| revision N − 1 used without validation | 33 |
| a repeated context echoes the full value | 42 |
| optional presence ignored by the diff; the summary cap removed | 29; 34 |
| 5 MiB applied to non-sheet POST routes; the pre-body check removed | 39; 40 |
| the echo cap removed | 42 |
| one budget term removed from `within` or `form_budget` | 44 (each term); 45 (units) |
| the gate restored | 43, 47 |

## 5. Commit-sized steps

Every commit passes fmt and strict clippy (`all` + `pedantic`, `-D warnings`) with no `#[allow]`.
Every new item lands with its production caller (named below). **List publication stays
disabled until step 8.** Earlier steps change shipped shared paths on purpose, each documented
in its commit (RR5, N10):
- step 1, the writer's decoder check (all runtime sheets);
- step 2, the D4 precedence (stale invalid saves answer 409 on scalar runtime and private
  native sheets) and set-based admission;
- step 7, the 5 MiB create/edit POST limit, the pre-body principal check and the echo cap (all
  private sheets).

1. **Writer** (tests 1–6). `snapshots::host_item_ids` (private; caller `write`, after the
   conflict check); the `parse_sheet` round check in `encode` (caller `write`). Seeds `i-{n}` →
   `r1-{n+1}`.
2. **Transport and precedence** (tests 7–11). `PairsSeed` and `Pairs::parse_limited` (callers:
   `parse`, and `save_private` for sheet POSTs); `form_urlencoded` as a direct dependency;
   set-based `Pairs::admit` (callers: the form helper at `web.rs:201` and `save_private`);
   `values::sheet_keys` (caller: the new admission `spawn_blocking` step in `save_private`). The
   revision check moves first. The N6 audit is re-run and recorded. List rows are admitted and
   then refused by `form` until step 3.
3. **List save and render** (tests 12–17, test 10's list case). First the `scraper` selection
   note, the one-time fetch, the `about.toml` entry and the rebuilt `submitted` helper (test
   16). Then `ListDraft` and `values::reconcile` (callers `values::form`, and
   `groups`/`render_private`); the template list branch and CSS; the `op=save` buttons, top and
   bottom, on list sheets; the M2 refusal strings removed.
4. **Draft ops** (tests 18–27). `values::Op` parse and apply for add, remove, up and down
   (caller `save_private`); `formnovalidate`, `autofocus` (no `formaction`, revision 3.2), edge rules, messages,
   the 256 limit, the stale warning, the button-row CSS.
5. **History rows and change summary, derived values, escaping** (tests 28–37). History legends
   (caller `render_private`); `history_diff::diff` (caller `private_read`'s history path, which
   also loads revision N − 1); the summary section and its bounds.
6. **Docs and smoke preparation** (test 38). The contract gains the ID format and slot
   semantics, the address grammar, the optional state, the D4 precedence, op and move behaviour,
   the change summary, the limits and the accepted large-body risk. The sheet-presentation and
   campaign-workspace docs are updated, but none of them claims list publication.
   `smoke_release.py` selects by version ID and logs it. `source_fixture.rs` writes
   `list-definition.json`: a derived `load` sum, a formula reading `item.id`, `load <= 10`, and
   a second list of 32 properties for 390 px. Its caller is a bypass route test.
7. **Large-form transport** (tests 39–42). The per-method 5 MiB layer through one helper, the
   pre-body principal `route_layer`, `ECHO_BYTES` and its alert, the 64-character excerpts of
   repeated echo contexts (RE3-1). A shipped change for every
   private sheet, documented in the contract.
8. **Gate lift, its own final commit** (tests 43–48, test 34 rerun):
   - `values::form_budget`, `within`, `ENVELOPE_NODES` and `RENDER_UNITS` (caller
     `publishable`); `decode::MAX_NODES` (callers: the decoder and `form_budget`); the page-size
     constants `FIXED_PAGE_BYTES` and `UNIT_MARKUP_BYTES` already land with test 42 in step 7;
   - `list-editing-unsupported` removed;
   - **deleted:** `publish_unshipped_for_tests`, its re-export, `UNSHIPPED_FOR_TESTS` and the
     early return; its callers move to real publication;
   - the smoke publishes a third version;
   - a `browser_smoke.py` list stage at both viewports: Add on an empty create page before Name
     (B6); fill, Enter saves; IDs; Remove; Move up and down by keyboard (Tab to the button,
     Enter and Space), checking `document.activeElement` after each op and that edge buttons are
     disabled; an op keeps an invalid row value; a violation; read-only history with the change
     summary; no script; `no_overflow` at 390 px with a 3,000-character note and the 32-property
     row with its buttons;
   - the scope docs flipped.

   Steps 1–7 are committed with their gates green. Step 8 (Sol M3, as M3 §10b.17) is assembled
   in the worktree. The **full §6 set, the smoke run and both browser viewports run on the
   uncommitted candidate**, and it is committed only when all pass, with the tree hash recorded.
   Any later fix re-runs the set.
9. **Implementation review** (Sol + Sonnet + the third chain) and a record with a red→green
   table (executed red, compile red, guards with their killing mutations). Not complete while a
   blocker is open.

## 6. Container gates

The `mimik-ci` set of M3 §8: fmt, strict clippy, full tests, release formula tests, the
`formula_eval` build, `source_fixture`, ruff and unittest, actionlint, `cargo about` (including
`scraper` and `selectors`), a release build against a re-measured baseline, and `cargo audit`.
Networked runs: `cargo audit` and the recorded one-time dependency fetch in step 3; nothing
else. On the step 8 candidate, also: `cargo package --offline` plus verify; `cargo vendor` and
the release package; checksums, unpack and binary equality; `smoke_release.py --auth`;
`mimik-browser` with the CI flags; the fresh offline rebuild with an empty `CARGO_HOME`; and
`lan_smoke.py`. Record the bytes of the worst-budget GET, op, 422 and change-summary pages
(asserted ≤ the D7 and D8 bounds), plus their time and permit hold (report-only). Also record
the release-build time of a 5 MiB save and the JOBS hold of a slow 5 MiB upload: if a legitimate
worst-budget save exceeds 2.5 s (half the 5 s `boundary` timeout), stop and revisit the budget.
Use `--cpus 2` on weekdays 09:00–18:00. Report every gate not run as not run.

## 7. Risks

1. Enter fires an op (D6, test 17, browser Enter check). Size is bounded by the D7 budget and
   measured.
2. Positional keys rely on immutable pins. Reloading after an op asks to resubmit, which is
   harmless (`no-store`, no writes).
3. JOBS = 1 contention means more 503s. The M3 rejected-save `None` path can still lose a draft
   on 503 (`web.rs:896-901`, documented).
4. D4 changes a scalar and native corner case (tested and documented). The writer invariant
   binds future writers (§8). The bypass must be fully deleted (mutation check).
5. A logged-in principal can make the server buffer 5 MiB without CSRF for ≤ 5 s while holding
   the JOBS permit (D7, accepted, measured). An oversized draft is not echoed (Q5).
6. Reorder adds a move per click, so long reorders take many round trips without JS; the change
   summary reports the net move, not each click.
7. The change summary describes slots, not items (D3, D8): a reused slot reads as "all
   properties changed", never as a removal plus an addition.
8. Scope creep into the catalogue, value diffs or revert (§8). Third reviewer: retry at
   implementation review.

## 8. Deferred to the item-catalogue milestone

- A per-property `editable: false` flag (Sol B1):
  - the schema and an `ItemProperty` field whose default keeps existing canonical bytes and
    hashes;
  - translator support and read-only rendering;
  - exact key sets for existing versus new rows;
  - reconstruction by saved ID, including optional absence;
  - writer-enforced immutability.
- An **item catalogue** in the definition. A new row picks an item and gets its fixed stats,
  under an authority the host checks. This settles new-row provenance (Sol B2) and the pack's
  acquisition rule (`ask_rule`, `tool-loop-pack-adjudication-2026-10-16.md:112-121`). Item
  identity beyond slot IDs (D3) belongs here too.
- Until then, every list property is editable, including pack `conditions.kind` and the
  equipment stats. The M4 pack test claims neither item-stat immutability nor a working
  acquisition workflow.
- Also deferred: value-level diffs, revert or restore from history, a summary on the history
  index, moving the JOBS permit after body buffering, and the import and tool-loop writers.
