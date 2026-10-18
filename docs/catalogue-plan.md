# Read-only catalogue plan (step 1 of the definition-language order of work)

2026-10-18. Status: **revision 2.1** (rev 2 re-review amendments R1–R16 in §7, binding over the body). Revision 2 Revision 1 (`3b63274`) was reviewed by Sol
(NO-GO), Sonnet (GO WITH AMENDMENTS) and Space Bunny (GO WITH AMENDMENTS). The record is
[reviews/2026-10-18-catalogue-plan.md](reviews/2026-10-18-catalogue-plan.md); §6 maps every
finding to this revision. No code yet.

Context:
- [definition-language-draft.md](definition-language-draft.md): its user decisions and the
  product principle "assist, don't police";
- the draft review [reviews/2026-10-18-definition-language-draft.md](reviews/2026-10-18-definition-language-draft.md),
  order-of-work step 1;
- [derived-formulas-m4-plan.md](derived-formulas-m4-plan.md) §8 (item identity), which this
  plan partly takes over.

## 0. Goal, non-goals, user decisions

**User value.** A player picks a spell or item on their sheet from the game's catalogue and
reads its full description and source citation without opening the manual. They can also
browse and search the catalogue from the sheet.

**User decisions (2026-10-18).**

| # | Question | Decision |
|---|---|---|
| Q1 | Manual page images for players | **No.** Citation only (book, printed page, PDF page). Images are a later decision |
| Q2 | Entries whose source text is not yet reviewed | **Shown**, with an "unchecked" badge |
| Q3 | Owner-written entries (house rules, homebrew) | **Allowed**, with an "owner-written" badge |
| Q4 | A sheet row naming an item that is not in the catalogue | **Invalid.** The sheet must match a rule: the owner adds house items to the catalogue, so the app always knows an item's details |
| Q5 | Restricting catalogue text from players | **No switch.** What is in the owner's manuals is readable by the users who can read a sheet linked to that catalogue (custodian and campaign Masters). The owner's own server and own group count as local. The restriction already in force stays: no packaging and no repo |

**Consequence of Q4 + Q3 (design driver D3).** The owner must be able to add a house item
later and have existing sheets pick it. Today a sheet pins an exact definition version
forever, so a catalogue pinned the same way would strand every existing sheet. The catalogue
is therefore **append-only** and is linked by identity, not by version (D3).

**Out of scope** (later plans):
- copying entry stats onto rows, `editable: false`, and formulas over entry data;
- choices filtered by rules, and legality warnings;
- any LLM;
- page images (Q1), and full-text search of descriptions;
- revert;
- campaign-level browse without a sheet;
- pairing an Italian and an English edition of one entry;
- GM-only or spoiler entries (every entry of a linked catalogue is visible to the sheet's
  readers).

No model is involved anywhere in this plan.

## 1. Facts this plan rests on (verified 2026-10-18; rev 1 errors corrected)

- **Definition decoder.**
  - `decode::parse` uses `Seed` with per-member `limits()`: the default is an array of 256 and
    a string of 5,000 (`decode.rs:21-37`). `parse_sheet` shares `Seed`, but sheet `data` has its
    own 20,000-scalar string branch (`decode.rs:116-129`).
  - Nodes are capped by `MAX_NODES` = 32,768 (`decode.rs:9`). The same constant is the list
    form-budget node bound (`values.rs:196`).
  - Raw input is capped at 256 KiB for definitions and 1 MiB for sheets (`decode.rs:173-177`).
  - Serde may allocate an escaped string before the visitor checks its length, so the raw cap
    is what bounds that allocation (`definition-drafts.md`).
- **Closed envelope.** Schema, then serde `deny_unknown_fields`, then domain checks, then
  canonical encoding and the hash (`mod.rs:99-117`, `admission.rs:27-32`). Optional members
  with `skip_serializing_if` leave the frozen hash vectors unchanged (`mod.rs:107-111`,
  `admission.rs:252-265`).
- **Why a separate artifact.** The closed envelope alone doesn't forbid a catalogue member. The
  scale and lifecycle do: 256 KiB per definition (`mod.rs:26`), 1,024 evidence regions
  (`admission.rs:181`), the 1 MiB snapshot (`publication.rs:9`), and page-scoped wiring
  invalidation.
- **Source text.**
  - Effective unit text is the correction for `corrected` units, else `raw_text`
    (`publication.rs:221-251`, which stores hashes only, never text).
  - Raw text is at most 256 KiB and a correction at most 80,000 (`publication.rs:233`).
  - Reviewing a unit advances its revision and, by trigger, its page's revision and clears
    `complete` (`migrations/007_source_finalization.sql:88-98`). Page completion and
    `printed_label` change only the page revision (`sources/store.rs:325-349`).
  - Finalized sources, pages and units cannot be removed (`007:55-68`).
- **Search infrastructure.** `sources/search.rs` provides:
  - `fts_query` (quoted pieces, at most 16);
  - the guarded reads `guarded` and `read_guarded`;
  - `Budget`, a SQLite progress handler with a 1 s wall-clock budget (`:11-15`, `:174-188`);
  - `interrupted`.
  The FTS tokenizer is `unicode61 remove_diacritics 2`. `guarded`, `read_guarded`, `Budget`
  and `interrupted` are private today, so reusing them needs a narrow `pub(crate)` widening.
  `guarded` is an all-or-nothing size check, not a prefix read (`search.rs:125-140`).
- **Sheets.**
  - `private_read` loads and authorizes inside one transaction; it decodes the immutable pin
    and validates values outside the database mutex in `spawn_blocking`
    (`campaigns/sheets.rs:125-171`).
  - `validate_for_write` drops formulas for freeze writes of up to 32 sheets
    (`private_pin.rs:46-56`).
  - The value schema is capped at 256 KiB (`values.rs:155`).
  - Draft operations `Op::{Add, Remove, Up, Down}` re-render an unsaved draft without writing
    (`values.rs:389-398`). The page-markup bound is calibrated as 2,048 bytes per render unit
    (`list_tests.rs:2384-2390`).
- **Authorization.**
  - `authorize_read` allows the custodian or an active Master (`snapshots.rs:271-286`); a
    `NotFound` is answered as 403 (`web.rs:123-125`).
  - Every request re-checks the session.
  - Runtime routes hold `definitions::JOBS`, a semaphore of 1 with `try_acquire`; a busy
    server answers 503 (`campaigns/web.rs:42-56`).
- **No JavaScript on sheets.** Private sheet pages load no script; htmx is denied in auth mode.
  Owner material pages have their own script. Headers are no-store with CSP.
- **History diff** names what changed, never old or new values, and is pure
  (`history_diff.rs:1-5`).
- **Administration** is CLI-only. The existing CLI pretty-prints whole draft structures
  (`main.rs:157-159`, `:326-345`).

## 2. Decisions

**D1. A separate, append-only artifact.**
- Tables: `catalogue_drafts` (revisioned, editable), `catalogue_versions` (immutable),
  `catalogue_entries`, `catalogue_entry_units`, `catalogue_names_fts`, `catalogue_seals`.
- A published version is immutable. Its content hash is
  `sha256("mimik-catalogue-v1\0" + canonical)`.
- Drafts stay editable by revision, like definition drafts. Everything published is
  insert-only, and is guarded against UPDATE, DELETE and `INSERT OR REPLACE`, following
  `008:33-41` and `009:11-36`.

**D2. Envelope.**
- The envelope is closed and has its own trusted schema, `schemas/catalogue-v1.json`.
- It is decoded with a **separate decode profile**, `decode::parse_catalogue`, which has its
  own constants for bytes, nodes, arrays, depth and strings, and its own member-limit table.
- `MAX_NODES`, `parse` and `parse_sheet` stay unchanged. A regression test asserts that
  `MAX_NODES == FORM_LIMITS.nodes` still holds.

```json
{ "contract_version": 1, "game_id": "…", "edition_id": "…", "catalogue_id": "spells",
  "title": "Spells",
  "sources": [{ "source_id": 3, "pdf_sha256": "…" }],
  "entry_types": [{ "id": "spell", "label": "Spell",
      "properties": [{ "id": "level", "label": "Level", "control": "integer" },
                     { "id": "school", "label": "School", "control": "enum",
                       "options": [{ "id": "light", "label": "Light" }] }] }],
  "entries": [
    { "key": "lantern-flare", "type": "spell", "name": "Lantern Flare",
      "properties": { "level": 1, "school": "light" },
      "description": { "units": [{ "source_id": 3, "page_id": 41, "unit_id": 977 }] } },
    { "key": "house-charm", "type": "spell", "name": "House Charm",
      "properties": { "level": 0, "school": "light" }, "retired": false,
      "description": { "owner_text": "…" } } ] }
```

Rules:
- **Identifiers.** IDs, keys and option ids use `registry::valid_id`: a leading lowercase
  letter, then lowercase letters, digits or `-`, 1–64 characters in all (`registry.rs:41-46`). Names use the definition's sources and options shapes.
- **Entries.**
  - `type` must name a declared entry type.
  - `properties` must be exactly the declared keys of that type with matching controls, all
    scalar (text, integer, boolean, enum).
  - A description is either 1–16 unit references or `owner_text`.
  - `(source_id, page_id, unit_id)` must be consistent: the unit is on that page, the page is
    in that source, and the source is finalized, listed in `sources`, and of the exact
    game/edition pair.
  - Duplicate units within one entry are rejected.
  - **Unit order is the author's order** and is semantic. It is never sorted.
- **Canonical order.** Entry types and entries are kept in author order. Properties within an
  entry are written in their type's declared order. Sources are sorted by id.

**D3. Append-only identity, linked by catalogue id.**
- A catalogue is identified by `(game_id, edition_id, catalogue_id)`, and its published
  versions are numbered 1, 2, …
- Publication (D6) refuses a version that **drops a key or entry type, or changes an entry's
  type**, compared with the previous version. It may:
  - add entries and types;
  - change names, properties, descriptions and owner text;
  - mark an entry `retired: true`. A retired entry stays valid on existing rows but is no
    longer offered by the picker. A retired entry can be un-retired.
- Entry-type properties may change between versions. They are display data.
- A definition links a catalogue by id: `catalogues: [{ "id": "spells" }]`. An item
  property `catalogue_ref: { "catalogue": "spells", "entry_type": "spell" }` refers to it.
  Neither the link nor the reference names a version.
- Readers always use the **latest sealed version**. Because keys and types never disappear,
  a key that was valid when a sheet was saved stays valid in every later version.
- **Reproducibility contract**, written into `runtime-definition-contract.md`:
  - stored values (keys), definition pins and formula results stay exactly reproducible;
  - entry names, properties, descriptions, badges and citations are **current reference
    aids**, shown from the latest version and the current review state, and labelled so.
  - History pages show today's description for an old key. This is deliberate, and the entry
    page says "current text".

**D4. Live descriptions and badges.** Nothing is materialised.
- **Text.** The entry page reads each unit's current effective text (correction or
  `raw_text`) through a guarded read. It stops at **64 KiB of displayed text per entry**, then
  shows "continues in the manual at p. X".
- **Badges** are computed live:

| Badge | Condition |
|---|---|
| Owner-written | `owner_text` |
| Reviewed extraction | every unit is accepted or corrected, and every cited page is `complete` |
| Unchecked source text | any unit is unreviewed, or any cited page is incomplete |
| Description unavailable | any unit is `rejected` now. The text is never shown; the citation and the badge stay |

- Every unit-sourced entry page also says: "Extracted from your manual. Not compared with
  the original page by Mimik." Catalogue publication certifies references, not text (D6).
- **Citation:** source title, the current `printed_label` and the PDF page number, read live.

**D5. Limits** (provisional; step 0 calibrates them before any schema or migration is
frozen):

| Resource | Limit |
|---|---|
| Raw and canonical draft | 4 MiB each |
| Decoder nodes (own profile) | 524,288 |
| Depth | 8 |
| Entries | 4,096 |
| Entry types | 32 |
| Properties per type | 16 |
| Options per enum | 128 |
| Units per entry | 16 |
| Distinct units per catalogue | 16,384 |
| Name | 100 scalars |
| `owner_text` | 20,000 scalars per entry, **total ≤ 2 MiB** per catalogue (so 4,096 × 20,000 can't be claimed) |
| Linked catalogues per definition | 8 |
| `catalogue_ref` properties per definition | 16 |
| Displayed text per entry page | 64 KiB |
| Browse page | 50 results |
| Picker panel | 20 results |
| Search query | 100 scalars, at most 16 pieces |

Publication holds `definitions::JOBS` only for the decode, compare and insert of one draft.
There is no text expansion, so the work is bounded by the draft size. A long publication
makes sheet requests answer 503, as definition admission already does. This is accepted and
documented, and step 0 measures it.

**D6. Publication workflow** (CLI only, operator label as actor; no withdrawal command):

```
mimik import-catalogue-draft --file spells.json --expected-revision N
mimik show-catalogue-draft --id D          # metadata only
mimik compare-catalogue --id D             # counts + ticket
mimik publish-catalogue --id D --comparison-hash H --actor … --rationale …
mimik show-catalogue-version --game G --edition E --catalogue C [--version V]
```

- **Comparison** reports counts only: entries, types, owner-written, units and badge counts,
  plus a diff against the latest version, as **counts only**: added keys, retired keys and
  keys whose description references changed. It also reports refusals (dropped or retyped keys) and the ticket.
- **The ticket hash** covers:
  - the draft id, revision and content hash;
  - the latest sealed version (number and hash), or "none";
  - for every referenced unit: `(unit_id, unit_revision, state, page_id, page_revision)`.
- **Publish** recomputes the comparison inside one `IMMEDIATE` transaction and refuses a
  mismatched ticket. It then inserts the version, entries, entry units and FTS names, and
  finally a `catalogue_seals` row. Identical content to the latest version publishes nothing
  and reports the existing version.
- **Withdrawal is replaced** by publishing a new version that retires or fixes entries. An
  owner who needs text gone from players' view corrects or rejects the unit (D4), or replaces
  the owner text.
- **Redaction.** No command prints names, keys, owner text, descriptions or unit text, and
  error messages are static strings. Sentinel tests check this with distinctive synthetic
  strings across every command and every failure path. Drafts are read with the existing
  bounded reader (`local_file.rs`).

**D7. SQL integrity** (`migrations/019_catalogues.sql`, `SCHEMA_VERSION` 19, additive and
atomic):
- `catalogue_versions` has UNIQUE `(game_id, edition_id, catalogue_id, version)`, a foreign
  key to the registry pair, `json_extract(content, …)` identity checks and byte checks.
- `catalogue_entries` has primary key `(version_id, key)`, a type column and a `retired`
  column. `catalogue_entry_units` has primary key `(version_id, key, ordinal)`, with
  composite foreign keys to the entry and to `(source_id, page_id, unit_id)`.
- A trigger refuses inserts into a sealed version.
- The `catalogue_seals` insert trigger checks:
  - the entry count against the version content;
  - the append-only rule against the previous sealed version (no key or type missing, no
    entry with a changed type);
  - that seals are created in version order.
- Every table rejects UPDATE and DELETE except drafts.
- Every trigger is named, and each has a failing-first regression.
- Rust re-verifies the content hash and identity on every read of a version.

**D8. Definition linkage.**
- **Schema.** Two optional definition members, omitted when absent so the frozen vectors are
  unchanged:
  - top-level `catalogues`;
  - an `ItemProperty` member `catalogue_ref`, required exactly when `control` is
    `catalogue_ref`. The control is legal only on list item properties, never on top-level
    fields.
- **`decode.rs` limits.** New entries in `limits()`: `catalogues` (8 items, 64 characters).
- **Checks.** A linked catalogue must have a sealed version of the same pair, and the referenced
  entry type must exist in it. Since types never disappear, the check stays true once it
  passes. It runs in:
  - `definitions::store::references`, at draft save, so authors get the error early;
  - the publication snapshot and its `IMMEDIATE` transaction;
  - `private_evidence_available`, the readiness check.
  So a catalogue must be published before a definition draft that links it can be saved.
- **Formulas.** A `catalogue_ref` value is lowered to `ValueType::Text(64)` in
  `formula_admission::value_type`. `formula_profile` stays 1. It is never lowered to an enum
  of keys.
- **Publication gate.** Definitions with a `catalogue_ref` property are refused at admission
  until step 4 lands, so a half-integrated control can't reach sheets.

**D9. Values and validation.**
- A `catalogue_ref` value is a key string. In the value schema it is just
  `{type: string, 1–64}`, so there is no key list inside the 256 KiB schema.
- **Writer membership check.** On save (and only on save) the writer looks up the keys present
  in the new data, inside the write transaction. It runs one bounded query per linked
  catalogue, `WHERE key IN (…)` against the latest sealed version. It rejects a missing key,
  or a key of the wrong entry type, with a 422 that preserves the draft (Q4).
- **No check on other paths.** Reads, history and freeze writes don't re-check membership,
  because append-only makes membership monotonic (D3). `validate_for_write` and the 32-sheet
  freeze stay unchanged. A test proves that a freeze of 32 sheets holding catalogue
  references is unaffected.
- This check is structural, in the same class as enum options. It is not a rule-legality
  check, so the advisory contract is untouched (Q4).
- **History diff** keeps its values-free invariant: it names the address and never resolves
  names.

**D10. Picking and viewing on the sheet, without JavaScript.**
- **Cell display.** Each `catalogue_ref` cell shows the current name of the saved key as a
  link to its entry page (opened with `target="_blank"` and `rel="noopener"`, so the
  unsaved draft stays in the original tab), plus its badge. An unsaved choice shows its name
  without a link until it is saved. Names come from one bounded lookup of the keys on the page.
- **Pick flow.** A "Choose…" button (`op=browse-f-r`) re-renders the unsaved draft, like
  Add or Remove, with a picker panel for row r:
  - a search field (`q`) and up to 20 non-retired results of the entry type, each with its
    badge;
  - each result is a submit button `op=pick-f-r:<key>`, which sets the cell and re-renders.
  The `Op` grammar is extended with `Browse` and `Pick`, validated against the definition and
  rows. The key is checked against the catalogue on Pick, and again on Save.
- **Accessibility.** Pick and Browse focus targets go into `Focus`. A `role="status"` message
  announces "Lantern Flare chosen. Not saved yet."
- **Budget.** A `catalogue_ref` cell costs a text value, plus one button and one link. The page
  adds a fixed picker-panel term of 20 results × (button + label + badge). `form_budget` and
  `FORM_LIMITS` gain a per-page byte term. The three page-markup assertions
  (`list_tests.rs:2691`, `:2887`, `:3290`) are extended to a `catalogue_ref` definition.
- **No datalist and no per-row select.** These avoid the size, budget and accessibility
  problems the reviews found.

**D11. Entry and browse routes**, both reached only through a sheet:
- `GET /private-sheets/{id}/catalogue/{catalogue}/{key}` shows the full entry page (D4).
- `GET /private-sheets/{id}/catalogue/{catalogue}?type=…&q=…&after=…` shows 50 results per
  page, ordered by `(name, key)`, with an exclusive `(name, key)` cursor.
- **Resolution**, inside the same read transaction as `auth.recheck` and `authorize_read`:
  1. the sheet's pin is decoded;
  2. `{catalogue}` must be one of the pinned definition's `catalogues` entries, with the same
     pair;
  3. the latest sealed version is used.
- Native pins, unsupported pins, unknown catalogues, unknown keys and unauthorized sheets all
  answer the same 404/403 shape as today, so it can't be used to probe what exists.
- **Input validation.** `Path<String>` segments and `type` are checked with
  `registry::valid_id` before any SQL. `after` is parsed by a `deny_unknown_fields` query
  struct. `q` is limited to 100 scalars. Every value is a bound parameter.
- **Search** reuses `fts_query`, `Budget`, `interrupted` and the guarded reads over
  `catalogue_names_fts`, which indexes names only with `unicode61 remove_diacritics 2`.
- **Cost.** Both routes hold the runtime permit and run under `run_bounded_auth`. They are not
  rate-limited; the single permit bounds them, as with sheets. This is documented.
- Askama escapes everything, and text is shown with `white-space: pre-wrap`. Hostile-markup
  sentinels are used in tests.

**D12. Data lifecycle.**
- Catalogue rows hold manual-derived names and owner text. They are added to the local-data
  inventory in `docs/local-setup.md` and `docs/sources.md`, next to the source tables, for
  backup and future purge.
- A CI assertion checks that no test fixture or packaged artifact contains a catalogue table
  row or draft outside synthetic fixtures.
- Live descriptions add no second copy of unit text.

## 3. Implementation order

Each step uses failing-first tests, then code, and gets its own review.

0. **Calibration spike.** This is prototype code, not merged.
   - Build a synthetic 4,096-entry catalogue at the D5 maxima.
   - Measure, in release mode with fresh child accounting (`tools/definition_profile.py`
     style):
     - decode and publish time and peak RSS;
     - the browse and entry query time;
     - sheet render bytes with 16 `catalogue_ref` properties × 256 rows, with the picker open.
   - Fix D5 from these numbers before writing the schema or the migration.
1. **Artifact.**
   - The catalogue decode profile, schema, domain checks, canonical encoding and a frozen hash
     vector.
   - The draft table and CLI `import` and `show`, with redaction sentinels.
   - Tests: limits + 1 for each limit; duplicate and escaped keys; unknown members; property
     key-set and type mismatches; unit, page and source inconsistency; a rejected or
     non-finalized source; the owner-text total; `MAX_NODES` unchanged.
2. **Publication.**
   - `compare` and `publish`, migration 019, seals, the append-only rule and FTS.
   - Tests:
     - every trigger, against UPDATE, DELETE, replace, insert into a sealed version and
       out-of-order seals;
     - dropped or retyped keys refused;
     - an intervening unit review refuses the ticket;
     - identical content publishes nothing;
     - a cross-process race;
     - upgrading a populated v18 database to v19;
     - redaction.
3. **Linkage and read-only routes.**
   - The `catalogues` member, the three-site checks, and the entry and browse routes, with no
     `catalogue_ref` yet.
   - Tests:
     - frozen definition vectors unchanged;
     - the authorization matrix: custodian, Master, other player, outsider, Owner, frozen
       sheet, disabled account, and revocation mid-request;
     - guessed catalogue slugs and other games' catalogues return 404;
     - live badges: review, rejection, page completion and a page label change;
     - the 64 KiB cut-off;
     - the search budget is interrupted;
     - cursor order with duplicate names.
   - **Value:** players can already browse and read the catalogue from their sheet after this
     step.
4. **`catalogue_ref`.**
   - The control, `Text(64)` lowering, the writer membership check, the Browse and Pick ops,
     cell rendering, and the budget byte term. This step lifts the D8 gate.
   - Tests:
     - an unknown key, a wrong type, and a retired-but-valid key;
     - freeze of 32 sheets;
     - op grammar edges;
     - 422 preserves the draft;
     - focus and status messages;
     - the extended markup assertions;
     - history diff unchanged.
5. **Docs and the full gate.**
   - Docs: `runtime-definition-contract.md` (the reproducibility contract), a new
     `catalogue-publication.md`, `release-scope.md`, `local-setup.md`/`sources.md` (D12),
     `README.md`.
   - Gate:
     - fmt, pedantic Clippy, locked tests, tools tests, audit, about;
     - release build and package, unpacked-artifact startup, edit and persistence smoke;
     - offline source/vendor checks;
     - `smoke_release.py --auth` extended with browse, entry and pick;
     - Chromium at both viewports with keyboard-only pick;
     - the LAN smoke.

All fixtures are synthetic original text. No manual text enters the repo, the tests or any
output.

## 4. Risks

- **Effective text quality.** Unreviewed `raw_text` can be garbled. Badges and the "not
  compared" note make that visible but don't fix it; source review is outside this plan.
- **Unit granularity.** A description may need part of a unit, or text across columns.
  Corrections or owner text cover this.
- **Append-only grows forever.** Retired entries accumulate, and a large mistake can't be
  removed, only retired and hidden. This is accepted as the price of never stranding sheets.
- **No historical text.** Old revisions show current descriptions (D3). This is accepted and
  documented. Revisit if an audit need appears.
- **Authoring by hand** is slow. The next plan is a deterministic helper (draft entries from a
  heading range of units), before any LLM.
- **Permit contention.** Busy browse traffic shares the single runtime permit with sheet saves;
  step 0 measures it.

## 5. Review

Re-review of this revision by Sol and Sonnet in parallel, plus the third chain, before step 0.

## 6. Finding map (rev 1 review → rev 2)

| Finding | Resolution |
|---|---|
| B1 (history and withdrawal), B2 (materialisation integrity), B5 (materialisation bounds), Sonnet F2/F3/F4/F9, Space Bunny M2/M11/M12, Sonnet F20 (owner-text correction) | D3 + D4 + D6: no materialisation; live text; reproducibility contract stated; no withdrawal; append-only versions fix owner text; ticket covers unit and page revisions, recomputed in `IMMEDIATE` |
| B3 (confinement), Sonnet F7 | D11 resolution through the pin, plus the uniform 404 |
| B4 (decoder), Sonnet F8 | D2 separate decode profile; `MAX_NODES` regression |
| B6 (disclosure), Sonnet F6/F19 | Q5 user decision; D4 "not compared" note; D12 inventory |
| Sol 4, Sonnet F14, Space Bunny M1/m6 (citation freshness, rejected units) | D4 live citation and live badges; page `complete`; rejected text never shown |
| Sol 6, Sonnet F17 (lifecycle) | D6: draft → sealed versions; no withdrawal; ticket contents |
| Sol 7, Sonnet F1, Space Bunny m11/m12 (relational integrity) | D1 + D7 |
| Sol 8, Space Bunny M7, Sonnet F11/F21 (edit-mode path, datalist, accessibility) | D10 pick flow via draft ops; links in a new tab |
| Sol 9, Space Bunny M5 (page budget) | D10 budget byte term, extended assertions |
| Sol 10, Sonnet F18 (CLI leaks) | D6 redaction and sentinels |
| Sol 11, Space Bunny M3/M4 (validation path and loader) | D9 write-time bounded `IN` lookup; monotonic membership; freeze unchanged |
| Sonnet F10, Space Bunny M3 (where linkage checks run) | D8, at three named sites |
| Sonnet F12, Space Bunny M13, Sol 13 (history diff) | D9 values-free invariant kept |
| Sonnet F13, Space Bunny M6, m5, m10 (search and routes) | D11 |
| Space Bunny M8, m1–m4, Sonnet F15/F16, Sol 13 (envelope details, ordering) | D2 |
| Space Bunny M10, Sonnet F22 (order) | §3 calibration first; read-only routes before `catalogue_ref`; D8 gate |
| Sonnet F5 (homebrew) | Q4 user decision: invalid; D3 append-only lets the owner add items later |
| Sonnet F23, Space Bunny M9 (missing tests) | §3 per-step test lists |
| Sol 12 (§1 errors) | §1 rewritten |

## 7. Revision 2.1 amendments (rev 2 re-review; binding over the body)

Re-review of rev 2:
- Sonnet: GO WITH AMENDMENTS.
- Sol: NO-GO, with one Blocking finding.
- Third chain: see the review record.

**R1 (Sol 1, Blocking). Seal invariant: projections equal the hashed content.**
- `catalogue_seals` is inserted last in the publish transaction. Its insert trigger requires
  **exact two-way set equality**, computed with `json_each` over the version's canonical
  `content` (once, at seal time only), between the content and:
  - `catalogue_entry_types(version_id, type_id)`;
  - `catalogue_entries(version_id, key, type_id, name, retired)`;
  - `catalogue_entry_units(version_id, key, ordinal, unit_id)`.
- Each table must also have the content's row count.
- Before inserting, Rust verifies that the canonical content re-hashes to the version hash.
- Only after the equality check does the trigger apply the append-only comparison with the
  previous sealed version (no type or key missing, no changed type). It compares projections,
  which are now proven equal to the content.
- Inserting into any projection of a sealed version is refused.
- Tests:
  - a same-count substitution (`a, c` in place of `a, b`);
  - a wrong type, a wrong retired flag, a wrong unit or ordinal;
  - an extra or a missing row in each table;
  - a content/projection mismatch with matching counts;
  - an empty type retained across versions;
  - a crash or rollback mid-publish leaves no sealed or partial version.

**R2 (Sol 2, Sonnet M4/m1). Deployable foreign keys; FTS as a derived index.**
- `catalogue_entry_units` stores only `unit_id`, with a foreign key to `source_units(id)`.
- The envelope's `source_id` and `page_id` are checked against `source_units.page_id` and
  `source_pages.source_id` in Rust at admission. An insert trigger re-checks them through
  joins, and page and source are derived through joins at read time. No composite foreign key
  is created.
- `catalogue_entry_types` is added (Sonnet M4). The D8 check "the referenced type exists"
  reads it, never the JSON.
- `catalogue_names_fts` is a **derived, latest-only index**:
  - the publish transaction deletes that catalogue's FTS rows and inserts the new version's
    names;
  - its columns are `name`, plus `UNINDEXED` `catalogue_row_id` and `key`;
  - queries join back to `catalogue_entries` of the latest sealed version, with the FTS table
    first (CROSS JOIN, as in `search.rs:152`).
- FTS is exempt from the immutability triggers, since virtual tables can't have them. A
  `rebuild-catalogue-index` CLI command and an integrity check (FTS key set = latest
  projection) repair it.
- Index `catalogue_entries(version_id, type_id, name, key)`, and
  `catalogue_entry_units(unit_id)` for badge joins (Sonnet N7).
- The v18 → v19 upgrade test, on a populated database, also publishes and browses afterward.

**R3 (Sol 3, Sonnet M1). Property-addressed ops.**
- New ops: `browse-f-r-p`, `search-f-r-p` and `pick-f-r-p:<key>`, where p is the item
  property index.
- Parsing:
  1. split on the first `:`;
  2. parse the left side with the existing grammar, plus a third index;
  3. validate the key with `valid_id`.
- The whole op is at most 100 bytes.
- `Op` loses `Copy` and keeps `Clone`.
- The op must name a `catalogue_ref` property of an existing row. Anything else answers 400.
- Tests:
  - keys containing `-`;
  - a missing or extra `:`;
  - an out-of-range index;
  - a non-`catalogue_ref` property;
  - an op over the length limit;
  - Save round-trip of a picked key.

**R4 (Sol 4, Sonnet M2/M3). Picker form state and the Enter key.**
- Two new form keys:
  - `catalogue-q`, the query, at most 100 scalars;
  - `catalogue-open`, which records which (f, r, p) panel is open and must match a valid
    address.
- Both are admitted by `sheet_keys` only for definitions with a `catalogue_ref`. `values::form`
  ignores them, so they are never stored, and they count against the pair and `ECHO_BYTES`
  budgets.
- While a panel is open, the first submit control in tree order is a visually hidden default
  button `op=search-f-r-p` (`tabindex="-1"`, outside the accessibility tree), so Enter in the
  query field searches instead of saving.
- Tests:
  - keyboard-only search with Enter;
  - Enter in an ordinary field with no panel open still saves;
  - the query is never stored;
  - forged `catalogue-open` values.

**R5 (Sonnet M5). No per-request re-verification of the 4 MiB content.**
- The content hash and the projection equality are checked:
  - at publish (R1);
  - by `show-catalogue-version`;
  - by an Owner integrity check command, `verify-catalogues`.
- Requests check only that the version is sealed and that its identity columns (pair,
  catalogue id) match.
- D7's "Rust re-verifies … on every read" is withdrawn.

**R6 (Sol 7, Sonnet M6, N3). Request phases and the pin boundary.** Catalogue routes and
catalogue-aware sheet renders follow `private_read`'s pattern, keeping the permit through
every phase:
1. **Transaction A:** `recheck`, `authorize_read`, load the raw snapshot and pin.
2. **`spawn_blocking`:**
   - `private_pin::validate` (or `validate_for_write` where no formulas are needed);
   - `match_pin`;
   - unsupported pins refused (Sonnet N3);
   - the draft form parsed, for ops.
3. **Transaction B:**
   - `recheck` and `authorize_read` again;
   - the catalogue id checked against the validated pin's `catalogues` and pair;
   - bounded lookups:
     - names and badges for the keys on the page, grouped by (catalogue, type);
     - picker results;
     - Pick key validation.
4. **Render.**

Tests: corrupt pin, hash or pair; an unsupported pin; revocation between A and B; the permit
held across all phases, for Browse, Search and Pick.

**R7 (Sol 5, Sonnet m4). Bounded prefix reads.**
- First, one metadata query covers **all** referenced units: state, page `complete`,
  `printed_label`, PDF page, and the effective byte length via `length(CAST(… AS BLOB))`. Any
  rejected unit means "description unavailable", even when it sits after the cut-off point.
- Then the units are read in order with `substr(effective, 1, remaining_chars)`. The display
  cap is **20,000 characters per entry**, at most 80 KB of UTF-8, which replaces "64 KiB". The
  cut is character-safe, because SQLite's `substr` on TEXT counts characters.
- When the text is cut, a "continues in the manual at p. X" note names the page of the
  first unit that was not fully shown.
- Owner text is at most 20,000 characters, so it is never cut.
- Tests:
  - an oversized first unit;
  - a cumulative cut;
  - a multibyte character at the cut;
  - a rejected unit after the cut;
  - owner text.

**R8 (Sol 6, Sonnet m8). Contention, measured correctly.**
- Two separate measurements:
  - **CLI against server:** a separate process, contending through the SQLite write lock
    (busy timeout 5 s, `store.rs:72`). Not `JOBS`, which is per process.
  - **Within the server:** contention for the `JOBS` permit.
- **Acceptance criteria for step 0**, on this machine, in release mode:
  - the publish write transaction holds the lock for **≤ 2 s** at the maximum size;
  - a concurrent server save waits and succeeds, rather than failing;
  - CLI peak RSS **≤ 256 MiB**;
  - browse, entry and pick page server time **p95 ≤ 250 ms** with 8 linked catalogues at
    their maxima.
- If a criterion fails, D5 limits are lowered before the schema is written.
- D5's sentence about 503 is replaced by these two measurements.

**R9 (Sol D9, Sonnet m3). Writer and retired keys.**
- The membership check lives in `snapshots::write` (`snapshots.rs:306`), for create and edit,
  after revision-conflict precedence.
- Queries are grouped by (catalogue, entry type), with at most 4,096 bound keys in total.
- **A retired key is accepted only if** the same list property already held it somewhere in
  the previous revision. New picks of retired entries are refused.
- Freeze, reads and history stay unchecked (monotonic).
- Tests:
  - direct writer calls (not only the web handler);
  - forged POST values;
  - a retired key that is kept versus newly added;
  - the largest IN query.

**R10 (Sol D3). Readiness stays independent of catalogue versions.** The three D8 checks
verify existence and integrity only. Catalogue state never enters the definition wiring
snapshot or its hash. The approval envelope keeps its 8 keys (`009:11-36`, Sonnet m7), so a
catalogue publication never makes a definition approval stale.

Regression: catalogue v1 → v2 with existing sheets and history; definition and snapshot hashes
unchanged, formula results unchanged, new-sheet readiness intact.

**R11 (Sol D3). Capacity.** Retirement doesn't free capacity. A catalogue at 4,096 keys can't
accept another house item. This is documented in `catalogue-publication.md`. Step 0 checks
whether 8,192 entries also meets R8. If it does, D5's entry limit becomes 8,192.

**R12 (Sonnet N5, Space Bunny B3 rev 1). Badge wording.**
- **Reviewed** entries say: "Source text reviewed. Which passages form this entry was chosen by
  the catalogue author."
- **Unchecked** entries say: "Extracted from your manual; not yet reviewed."
- The always-on "not compared" sentence is dropped as contradictory.

**R13 (Sonnet m9). Browse cursor and order.**
- Results are always ordered by `(name, key)`, including with `q`, where FTS is a filter
  only, never a rank.
- The cursor is `after=<key>`, validated with `valid_id`. The server looks up that key's
  `(name, key)` in the latest version and seeks from there; an unknown key restarts from the
  beginning.
- Tests: duplicate names, a cursor key from another type.

**R14 (Sonnet m5). Every `Control` match site gets a `CatalogueRef` arm:** `values.rs:92`,
`:201` and `:543`, `web.rs:572`, `formula_admission.rs:219`, `history_diff.rs:304` and
`private_pin.rs:139`. Clippy's non-exhaustive match makes a missing arm a compile error;
tests cover each behaviour.

**R15 (Sonnet N1, N2, N4).**
- The history page says that descriptions show the current text.
- `campaign-workspace.md` documents that a departed custodian keeps reading catalogue text
  through a frozen sheet, consistent with Q5.
- Formula literals naming catalogue keys are unchecked (a note in the contract).

**R16 (Sol 8, Sonnet N6).** The docs list in step 5 adds `definition-publication.md` (the
disclosure boundary) and `campaign-workspace.md` (R15). The search helpers get the
`pub(crate)` widening noted in §1.

**Additional tests** (Sonnet), on top of those above:
- a catalogue linked by two definitions;
- colliding keys across two catalogues;
- a frozen-sheet custodian reading the catalogue.
