# Read-only catalogue plan (step 1 of the definition-language order of work)

2026-10-18. Status: **revision 2.2: cleared for step 0.** The rev 2 re-review amendments R1–R16 and the rev 2.1 final-check amendments R17–R28 in §7 are binding over the body. Revision 2 Revision 1 (`3b63274`) was reviewed by Sol
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
  - Nodes are capped by `definitions::decode::MAX_NODES` = 32,768 (`decode.rs:9`; the
    catalogue decode profile has its own `MAX_NODES`, 524,288, §13). The same constant is the list
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
frozen; the values in force are in §9 and §14: entries 6,144, owner text 1 MiB in total):

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
| Unit references per catalogue (with repetition; added §12, set in §14) | 6,144 |
| Distinct units per catalogue | 16,384 (unreachable below the reference cap; defence in depth) |
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
     - cursor order with duplicate names;
     - name search never passes raw user text to `MATCH`: tokens are quoted as phrases (the
       existing `fts_query` builder) and their count is bounded (§12);
     - owner text is rendered in an isolated bidi context (`<bdi>` or `unicode-bidi:
       isolate`, `white-space: pre-wrap`), with a browser test that a bidi override in owner
       text cannot reorder the surrounding name, citation or controls (§12).
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

## 8. Revision 2.2 amendments (rev 2.1 final check; binding)

Final check of rev 2.1:
- Sol: **GO WITH AMENDMENTS**, no Blocking findings; "no further plan-review round is
  necessary".
- Sonnet: **GO WITH AMENDMENTS**, no Blocking findings. It asks for a short check of the
  concrete seal-trigger SQL; that happens in step 2's implementation review.

**R17 (Sonnet 1). Projections carry everything readers use, and the seal binds all of it.**
- `catalogue_entries` gains:
  - `desc_kind` (`units` | `owner_text`);
  - `owner_text`;
  - `properties`, the per-entry canonical JSON, bounded.
- `catalogue_entry_units` gains `source_id` and `page_id`.
- R1's equality covers every one of these columns.
- A `BEFORE INSERT` trigger on `catalogue_entry_units` checks, from columns only:
  - the unit is on that page and the page is in that source;
  - the source is finalized and has the catalogue's pair.
- Entry pages and the picker read projections only, never `content`.

**R18 (Sonnet 2, Sol R1 note). The seal SQL is linear.**
- Set equality is two `EXCEPT` queries in each direction, over `json_each` flattened once.
  Entry units use `json_each(e.value, '$.description.units')` in a join.
- Correlated `NOT EXISTS` scans over `content` are forbidden.
- Absent `retired` is normalised with `COALESCE(json_extract(…), 0)`. Ordinals are 0-based.
- Step 0 measures the seal trigger alone at maximum size.

**R19 (Sonnet 3, Sol R1). Seal order and guards.**
- A seal's version must equal the maximum sealed version + 1, and no lower version may be
  unsealed.
- Readers choose "latest" through `catalogue_seals`.
- Pre-insert replacement guards cover every primary and unique key of every immutable table,
  including the projections, the type table and the seals (the pattern of `008:33-41`).
- Rust checks rows-affected on every insert.

**R20 (Sonnet 4, Sol A5). FTS.**
- A surrogate table `catalogues(id INTEGER PRIMARY KEY, game_id, edition_id, catalogue_id)`
  gives an integer catalogue id. The FTS rowid encodes `catalogue_id * 2^20 + entry ordinal`, so
  a catalogue's FTS rows are deleted with a rowid range.
- FTS is joined back on `(version_id, key)` of the latest sealed version of **that**
  catalogue, so keys that collide across catalogues can't leak.
- The integrity check compares identity, name and multiplicity, then runs FTS5
  `integrity-check`.
- The repair command rebuilds the index from the latest sealed projections in one transaction.

**R21 (Sonnet 5). Prefix reads.**
- Metadata and text are read in one read transaction.
- Effective text is `correction` when the state is `corrected`, else `raw_text`
  (`publication.rs:238-242`).
- Keep the `typeof(…) = 'text'` guard (`search.rs:125-140`).
- Byte bounds come from the BLOB length. A unit containing NUL is shown as "description
  unavailable (source text malformed)".

**R22 (Sol A1, Sonnet 7). Draft-op submitters.**
- Every catalogue draft-op button has `formnovalidate`. This includes the hidden default
  button, which stays enabled and form-associated.
- The hidden default comes before every field in tree order. It is hidden with the
  clip-based visually-hidden style, not `aria-hidden`.
- Save keeps browser validation.
- While a panel is open, Enter in any single-line field searches. This is documented.
- `SHEET_FORM_PAIRS` and its assertion (`transport.rs:158`) are raised by 2.
- Tests:
  - keyboard-only search on a sheet with an empty required Name;
  - Chromium at both viewports.

**R23 (Sol A2). Exact picker state.**
- When the panel is closed, `catalogue-q` and `catalogue-open` are both absent. When it is
  open, both are present.
- `Pairs::admit` admits them conditionally from the submitted key set (it requires equality,
  `transport.rs:86-94`), then their values are validated. One without the other answers 400.
- Add, Remove, Up and Down **close and clear** the picker.
- Publication-time `form_budget().pairs` counts the two controls in the worst case.

**R24 (Sol A3). Pick sets presence.** Picking into an optional `catalogue_ref` sets
`set-f-r-p=present`. Test Pick → Save starting from an absent optional property. Only Pick takes
a `:` suffix; Save, Browse, Search and the existing ops refuse one.

**R25 (Sol A4, Sonnet 8). The second boundary.**
- R6's transaction B extends the existing `private_badge` boundary (`sheets.rs:309-345`):
  - it reloads the resource;
  - `RawSnapshot::corresponds` checks it against the validated snapshot;
  - it recomputes editability and readiness;
  - it keeps the separate branch for creating a sheet;
  - it re-checks auth before commit.
- Catalogue lookups run in that transaction.
- `draft_op` gains this DB phase for Browse, Search and Pick, with the permit threaded through
  rendering and error re-renders.
- The existing `rejected_editor` re-acquire (a 503 can lose a draft) is recorded as an existing
  limitation. Picker traffic makes it more likely, and step 0 measures that.

**R26 (Sol R14, Sonnet 6). Match-site audit instead of a compiler claim.**
- Wildcard arms at the sites listed become explicit `CatalogueRef` arms: `values.rs:201`,
  `history_diff.rs:304`, `private_pin.rs:139`, `formula_admission.rs:70-75` and
  `admission.rs:120-140`. Where a wildcard is intentional, it is kept and commented.
- Negative tests:
  - a top-level `catalogue_ref` field;
  - `catalogue_ref` without its member;
  - `catalogue_ref` with `options`;
  - a `catalogue_ref` member on another control.

**R27 (Sonnet 9, 10; Sol R9). Writer and readiness.**
- The membership check runs after `host_item_ids` and conflict detection in
  `snapshots::write`, for custodian and Master edits alike.
- A retired key is refused on create, since there is no previous data. On edit it is allowed
  only if the same list property already held it. This is not a quantity rule. Freeze skips
  the check.
- Readiness means a sealed version exists and the type exists in `catalogue_entry_types`. It
  does no content verification (R5).

**R28 (Sonnet 12, Sol R7/R8). Calibration covers the full request path.**
- Step 0 timings include `private_pin::validate` per catalogue request. Routes that don't
  evaluate formulas use `validate_for_write`.
- Step 0 also covers the full `substr` query path, the publish transaction including the seal
  and FTS, and server-save latency during a CLI publish.

## 9. Step 0 results (binding over D5)

Calibration: [catalogue-calibration-2026-10-18.md](catalogue-calibration-2026-10-18.md).
Every R8 criterion was met at both 4,096 and 8,192 entries with 8 linked catalogues.
- **D5 entries become 8,192** (R11); superseded by §14: **6,144**.
- The owner-text total becomes **1 MiB** (at most 20,000 scalars per entry), so the canonical
  draft stays under 4 MiB.
- Step 2 must re-measure the real CLI publish RSS and the separate-process contention, and
  falls back to 4,096 entries if the CLI exceeds 256 MiB.

## 10. Step 1 implementation notes (2026-10-18)

Branch `catalogue`. Scope: §3 step 1 only (decode profile, schema, domain checks, canonical
encoding and hash, `catalogue_drafts` in migration 019, `import-catalogue-draft` and
`show-catalogue-draft`). Operator documentation: [catalogue-publication.md](catalogue-publication.md).
Code: `src/catalogues/{mod,decode,admission,store}.rs`, `schemas/catalogue-v1.json`,
`schemas/catalogue-example.json`, `migrations/019_catalogues.sql`.

**Decisions taken in step 1 (not fixed by the plan):**
- A text property value is at most 1,000 scalars.
- At least one entry type is required; zero entries are allowed.
- The pair must be registered, nonnative and adapter-free, mirroring
  `definitions::store::references`.
- Entry `properties` is the one object without `additionalProperties: false`: it is a map
  keyed by declared property ids, closed by the exact-key-set domain check.
- The decoder lives in `src/catalogues/decode.rs`, a sibling of `definitions::decode`, with a
  position-aware member table (entry `properties` are keyed by author ids, so key-only limits
  as in `definitions::decode::limits` would not work).
- The owner-text total counts UTF-8 bytes; the per-entry cap counts scalars.
- The canonical cap is checked but unreachable from admitted input (canonicalisation never
  lengthens an accepted spelling), so it has no input-level test.
- Commands share the definition admission permit (`definitions::JOBS`).

**Red → green.** The red runs used the final tests against (a) the implementation with its
checks stubbed out (decoder replaced by plain `serde_json`, schema and domain checks skipped,
source references returning `Ok`), and (b) the tree without migration 019. The green run is the
real implementation.

| Test | Red (a) stubbed checks | Red (b) no migration 019 | Green |
|---|---|---|---|
| `admission_tests::canonical_encoding_has_a_frozen_independent_hash_vector` | fail | n/a | pass |
| `…::source_order_and_property_spelling_are_normalised_but_author_order_is_identity` | fail | n/a | pass |
| `…::decoder_rejects_duplicate_and_escaped_equivalent_keys_at_every_level` | fail | n/a | pass |
| `…::unknown_members_are_rejected_at_every_level` | fail | n/a | pass |
| `…::identifiers_follow_the_registry_grammar_with_a_leading_letter` | fail | n/a | pass |
| `…::duplicate_ids_keys_sources_and_units_are_rejected` | fail | n/a | pass |
| `…::entry_types_and_property_key_sets_and_controls_must_match` | fail | n/a | pass |
| `…::descriptions_are_units_xor_owner_text` | fail | n/a | pass |
| `…::string_limits_count_scalars_and_reject_one_more` | fail | n/a | pass |
| `…::array_limits_accept_the_limit_and_reject_one_more` | fail | n/a | pass |
| `…::entry_count_accepts_8192_and_rejects_8193` | fail | n/a | pass |
| `…::owner_text_total_is_capped_at_one_mebibyte_of_utf8` | fail | n/a | pass |
| `…::distinct_units_are_capped_per_catalogue` | fail | n/a | pass |
| `…::decoder_node_depth_and_byte_caps_reject_one_more` | fail | n/a | pass |
| `…::every_rejection_in_this_suite_is_redacted_including_schema_paths` | fail | n/a | pass |
| `…::definition_decoder_constants_are_unchanged_by_the_catalogue_profile` | pass (guard: the definition decoder was never changed) | n/a | pass |
| `tests/catalogues.rs::cli_import_and_show_print_metadata_only` | fail | fail | pass |
| `…::cli_failure_paths_are_redacted_and_save_nothing` | fail | fail | pass |
| `…::cli_conflict_unknown_and_corrupt_draft_paths_are_redacted` | split out of the previous test after the red runs | — | pass |
| `…::units_must_be_on_their_page_in_a_listed_finalized_source_of_the_pair` | fail | fail | pass |
| `…::sources_must_be_finalized` | fail | fail | pass |
| `…::revisions_are_checked_and_reads_reverify_content` | pass (no check stubbed) | fail | pass |
| `…::storage_rejects_malformed_rows_and_identity_rebinding` | pass (SQL constraints) | fail | pass |
| `…::cross_process_create_has_one_winner` | pass (SQL uniqueness) | fail | pass |
| `…::populated_schema18_database_upgrades_to_19_atomically` | pass (migration present) | fail | pass |

The first green run also found two test defects, fixed before the final run: the hash constant
was a placeholder (frozen from the run, after the canonical bytes and an in-test SHA-256 of them
both matched), and the node-cap document exceeded 4 MiB (member names shortened). Six
existing schema-rewind fixtures (`tests/support/schema_fixture.rs`, `tests/source_search.rs`
and four `src/campaigns/*_tests.rs`) now drop `catalogue_drafts`, which is newer than every
rewind target, as they already did for registry history.

**Measurements** (release build, container `mimik-ci:latest`, `--cpus 6`, Ryzen 5 1600 shared
host, Sunday evening, single runs; fresh child accounting per process). Synthetic drafts: one
type with 16 integer properties, 100-scalar names, owner text totalling about 1 MiB, no unit
references (so the per-unit reference queries are not exercised).

| Case | Input bytes | Command | Wall ms | Peak RSS KiB |
|---|---:|---|---:|---:|
| 8,192 entries | 3,638,306 | import, create (rev 0) | 325 | 94,820 |
| 8,192 entries | 3,638,306 | import, update (rev 1, rev 2) | 333, 320 | 94,632, 94,436 |
| 8,192 entries | — | show | 255 | 63,000 |
| 4,096 entries | 2,343,970 | import (rev 0, 1, 2) | 199, 207, 209 | 63,244, 63,156, 62,980 |
| Baseline `list-rulesets` | — | — | 15 | 12,896 |

The 8,192-entry import stays far below the R8 CLI ceiling of 256 MiB. Step 2 measures the
publish path (projections, seal, FTS) and separate-process contention, as §9 requires.

**Review round 1 fixes** (Sol: FIX THEN MERGE; Sonnet: FIX THEN MERGE; third chain pending).
Each regression was run against the pre-fix tree (only a no-op `after_read` hook added to
`save` for the race tests) or, where the check already existed, against a narrow mutation of
that check, then against the fix.

| Item | Regression | Red | Green |
|---|---|---|---|
| Sol F1 redaction by position | `schema_paths_redact_author_keys_by_position_not_spelling`, `cli_schema_paths_never_echo_author_property_keys` (keys `314159265358979`, `name`, `properties`, `0`, `units`, `label`), `redaction_walks_the_envelope_structure` | both fail on the pre-fix tree (numeric and member-named keys kept) | pass; the path walker keeps indices only at array positions, members only where they exist, and always redacts entry-property keys |
| Sol F2 / Sonnet 2 IMMEDIATE save | `store::tests::concurrent_saves_of_different_catalogues_both_succeed`, `…_of_one_catalogue_give_one_winner_and_a_conflict` (separate connections, a rendezvous after the read phase, create and update) | both fail: the loser gets `Storage(DatabaseBusy)` | pass with `TransactionBehavior::Immediate`; the cross-process CLI race now also requires the loser to report `Conflict` |
| Sol F3 / Sonnet 1 escaped duplicates | `decoder_rejects_duplicate_and_escaped_equivalent_keys_at_every_level` now builds `\uXXXX` spellings in code and asserts the backslash survives (the earlier fixture strings had lost their escapes) at root, entry type, entry, entry properties, description and unit | mutation: duplicate-key check removed → fails | pass |
| Sol F3 / Sonnet 1 cross-wired sources | `cross_wired_real_sources_are_rejected`: two real finalized sources of the pair, every id exists, four wrong relationships | mutation `p.source_id=?3` removed → fails; mutation unit→page join replaced by `ON 1` → fails | pass |
| Sonnet 3 control characters | `control_characters_are_rejected_in_names_labels_and_text_values` (U+0000, U+0007, LF, TAB, ESC, U+0085 in title, labels, option labels, names, text values; owner text allows LF and TAB only) | fails (accepted) | pass (`control-character-in-text`) |
| Sonnet 4 adapter pair | `a_nonnative_pair_with_a_built_in_adapter_is_refused` (test DB drops the registry immutability trigger and flips `native`; no product path builds this state) | mutation: adapter predicate removed → fails | pass |
| Sonnet 4 coverage | `identical_content_with_the_current_revision_still_advances_it`, `containers_where_scalars_belong_are_rejected` | pass on the pre-fix tree (behaviour already present; coverage only) | pass |

Accepted without code change: Sol F4 (the admission worker lifecycle duplicates
`definitions::store`). The definition draft save keeps its deferred transaction; the
coordinator records that as a known pre-existing issue.

**Not yet done:** re-review of these fixes.

## 11. Step 2 implementation notes (2026-10-19)

Branch `catalogue`. Scope: §3 step 2 only (migration 019 extended with versions, projections,
seals and the name index; `compare-catalogue`, `publish-catalogue`, `show-catalogue-version`,
`verify-catalogues`, `rebuild-catalogue-index`), plus step-1 review items 7–8. Operator
documentation: [catalogue-publication.md](catalogue-publication.md#publication). Code:
`src/catalogues/publication.rs`, `migrations/019_catalogues.sql`, `src/main.rs`; tests
`tests/catalogue_publication.rs`, `src/catalogues/publication_tests.rs`,
`src/catalogues/admission_tests.rs`; shared fixture `tests/support/catalogue_fixture.rs`;
profile `tools/catalogue_profile.py`.

**Decisions taken in step 2 (not fixed by the plan):**
- `catalogue_entry_types` also carries the type's ordinal, label and property declarations
  (JSON), and `catalogue_versions` a `title` column, so readers never need `content` (R17's
  intent); the seal's equality covers all of them.
- Versions record draft provenance (draft id and revision), the ticket, actor and rationale. A
  version insert trigger requires contiguous numbers, every earlier version sealed, and
  content and hash equal to the named draft revision's (the `009` provenance pattern).
- Refusals are checked before "identical": identical content whose units were rejected after
  publication answers `rejected-units` rather than reporting the existing version.
- The source-reference check (`references`) runs again inside the publish transaction; its
  cost is inside the measured lock time.
- The name index has `prefix = '3'`, matching `source_search_fts` and the prefix pieces of
  `fts_query`. The calibration spike had no prefix index; the measurements below include it.
- `rebuild-catalogue-index` drops and recreates the index from `NAME_INDEX`, a Rust constant
  equal to the migration's statement (a test compares `sqlite_master` after both), so it also
  repairs corrupt shadow tables.
- `verify-catalogues` re-admits stored content with the current admission rules. A rule
  tightened after release would report older versions as `content-not-admissible`; no such
  versions exist yet (the branch is unmerged), and a future tightening needs a versioned
  profile. *Superseded in §12: versions record their admission profile.*
- Step-1 review item 7: names, labels, the title and text property values refuse Unicode Cf,
  Zl and Zp (`format-or-separator-character-in-text`); owner text refuses Zl and Zp but keeps
  Cf for right-to-left prose. Library: `unicode-general-category` 1.1.0 (Apache-2.0), already
  in the shipped tree through `jsonschema`, now a direct pinned dependency. Item 8: "normalise
  owner text to LF" is documented.
- The seal and version replacement guards are shadowed by the order checks (a replacement
  always also breaks the order); they stay as defence in depth, and their own refusals are
  tested in a test database without the order trigger.

**Found during the profile:** replacement guards written as one `EXISTS(... key=NEW.key OR
ordinal=NEW.ordinal)` could not use an index, so each insert scanned the version and
publication was quadratic: 10.5 s at 8,192 entries. Each key now has its own `EXISTS`.
Regression `publication::tests::projection_inserts_scale_linearly` counts SQLite VM steps for
1,024 and 2,048 entries: red at 3.92× with the old trigger, green below 2.6×.

**Red → green.** Red runs used the final tests against (a) migration 019 without any step-2
trigger; (b) the Rust checks stubbed (ticket bound to the draft only, no refusals, never
identical, no projection, hash or index comparison in verify, no index row delete at publish,
rebuild a no-op); (c) publication without its transaction (autocommit); (d) entry properties
encoded through `serde_json::Value` (sorted keys); (e) admission without the Cf/Zl/Zp check.
Green is the real implementation.

| Test | Red | Green |
|---|---|---|
| `tests/catalogue_publication.rs::seal_requires_entries_equal_to_the_content` (same-count substitution, wrong type, retired, name, owner text, properties, order, extra, missing) | (a) | pass |
| `…::seal_requires_entry_types_equal_to_the_content` | (a) | pass |
| `…::seal_requires_entry_units_equal_to_the_content` (wrong unit, swapped ordinals, extra, missing) | (a) | pass |
| `…::seal_refuses_dropped_or_retyped_keys_and_dropped_types` (empty type included) | (a) | pass |
| `…::seals_follow_version_order_and_versions_follow_their_draft` | (a) | pass |
| `…::published_rows_reject_update_delete_replace_and_sealed_inserts` | (a) | pass |
| `…::replacement_guards_cover_every_key_of_an_unsealed_version` | (a) | pass |
| `…::unit_rows_must_match_their_page_finalized_source_and_pair` | (a) | pass |
| `…::tampered_versions_fail_show_and_verify` | (a) setup, (b) | pass |
| `…::cli_compare_publish_show_verify_and_rebuild_print_metadata_only` | (b) | pass |
| `…::cli_identical_content_publishes_nothing` | (b) | pass |
| `…::cli_failure_paths_are_redacted_and_publish_nothing` | (b) | pass |
| `…::dropped_or_retyped_keys_and_types_are_refused` | (b) | pass |
| `…::empty_types_are_retained_and_changes_are_counted` | (b) | pass |
| `…::intervening_changes_stale_the_ticket` (unit review, page completion, label change, draft revision, another publication) | (b) | pass |
| `…::name_index_is_checked_and_rebuilt` (colliding keys across catalogues) | (b) | pass |
| `…::name_index_corruption_and_definition_drift_are_reported_and_repaired` | (b) | pass |
| `…::cross_process_publish_race_has_one_winner` | (b): the stubbed ticket does not bind the latest version, so the loser publishes again; not red under (c), where the outcome of this race is the same | pass |
| `…::crash_mid_publish_leaves_no_partial_or_sealed_version` (process killed inside the transaction, hot journal rolled back) | (c) | pass |
| `…::failing_seal_rolls_back_the_whole_publication` | (c) | pass |
| `…::escaped_and_multilingual_text_seals` | (d) | pass |
| `src/catalogues/publication_tests.rs::a_failure_before_any_stage_rolls_back_the_whole_publication` | pass in every red run (guard: it calls the transactional core directly) | pass |
| `…::rust_projections_equal_the_stored_rows_in_author_unit_order` | pass in every red run (guard) | pass |
| `…::projection_inserts_scale_linearly` | old OR trigger (3.92×) | pass |
| `admission_tests::format_and_separator_characters_are_rejected_in_names_labels_and_text_values` | (e) | pass |
| `tests/catalogues.rs::populated_schema18_database_upgrades_to_19_atomically` (now rewinds every catalogue object, fails on a late conflicting object, then publishes and runs an FTS query) | pass in (a)/(b) (the upgrade itself is unchanged) | pass |

Some tests were split or tightened after the first red runs (seal equality into three tests,
the identical-content CLI case and index corruption into their own tests, guard messages
that SQLite's trigger order can mask); the red runs above were repeated with the final tests.

**Measurements** (`tools/catalogue_profile.py`, release build, container `mimik-ci:latest`,
`--cpus 6`, Ryzen 5 1600 shared host, Monday 00:20–00:28, two runs at 8,192 and one at
4,096). Each command ran in a fresh wrapper process (peak RSS by `RUSAGE_CHILDREN`). The draft
follows the calibration: 8 types × 2 properties, every 8th entry owner-written (1 MiB in all),
the others citing 4 units of one finalized 1,024-page × 16-unit synthetic source. v1 holds
N − 2 entries; v2 and v3 each add one (append-only), so v3 is at the cap.

| Case (N = 8,192) | Wall ms | Peak RSS KiB |
|---|---:|---:|
| Baseline `list-rulesets` | 42–47 | 16,100 |
| v1 import (3,543,860 bytes) | 324–392 | 101,000–101,400 |
| v1 compare (no previous version) | 342–346 | 71,700–72,500 |
| v1 publish | 1,712–1,813 | 127,000–127,600 |
| v2 compare (with previous version) | 660–684 | 71,900–72,300 |
| v2 publish | 2,220–2,298 | 126,700–127,200 |
| v3 publish (at 8,192) | 1,764–1,843 | 127,000–127,300 |
| show-catalogue-version | 566–620 | 77,500–78,600 |
| verify-catalogues (3 versions) | 1,914–1,926 | 79,800–80,500 |
| rebuild-catalogue-index | 145–170 | 16,100–16,400 |

At 4,096: v1–v3 publish 802–989 ms, peak RSS about 86,200 KiB.

**Separate-process contention (R8).** During the v3 publish, a second Python process (system
SQLite 3.50.4) probed the write lock without waiting from 150 ms after start (past the CLI's
start-up and draft load) and, on the first refusal, ran `BEGIN IMMEDIATE`, an insert and a
commit with a 5 s busy timeout. It **waited 1,531–1,632 ms and succeeded** in all three runs at
8,192 (830 ms at 4,096). The wait spans the publish transaction from its comparison to its
commit, plus up to about 100 ms of SQLite busy-handler sleep granularity, so the lock is held
for at most about 1.6 s.

*The contention method and the R8 conclusion below are superseded by §12* (the probe measured
only the remaining hold, and only for v3).

**R8 at 8,192 entries:** lock held ≤ 2 s (about 1.5–1.6 s), a concurrent writer waits and
succeeds, CLI peak RSS about 125 MiB ≤ 256 MiB. **The D5 limit stays 8,192**; no fallback to
4,096. The margin on the lock is thinner than in step 0 (0.9–1.1 s there) because the ticket
recomputation, including the source-reference and unit-state reads and the previous
projection, now runs inside the lock. In-server `JOBS` contention does not apply to the CLI
(another process).

**Limitations:** single machine, few runs, shared host; units all unreviewed; the contended
writer is a plain insert, not a server sheet save; the crash test relies on the rollback
journal (`-journal` file), as noted in the step-1 review (re-check if WAL is enabled).

**Author gate** (container, `--cpus 6`, Monday 00:33–00:47): `cargo fmt --check` and pedantic
Clippy clean; `cargo test --locked --all-targets --all-features` all passed (335 library tests,
21 publication and 13 draft integration tests among them; 1 pre-existing ignored); `ruff check`
and `ruff format --check` clean; tools unittest 727 run, 11 skipped, **1 error:
`test_run_queue.ProcessLauncherTests.test_fake_runner_script`**, which fails identically on the
step-1 tools tree (`548cc84`) in the same container and is untouched by this step.

**Not yet done:** independent review of this step (including the concrete seal SQL, as the
plan review asked).

## 12. Step 2 review fixes (2026-10-19)

*Superseded in part by §13: the reference cap is now 8,192, the lock-probe bounds below
under-state the uncertainty (start times only), and every "worst" below means the worst
measured case on this host, not the worst admitted shape.*

Review of `aa23ace`: Sol FIX THEN MERGE, Sonnet FIX THEN MERGE, third chain unavailable.
Adjudicated items and what changed (code: `src/catalogues/{admission,publication}.rs`,
`migrations/019_catalogues.sql` edited in place since it is unmerged; tests
`tests/catalogue_publication.rs`, `tests/catalogues.rs`, `src/catalogues/admission_tests.rs`,
`src/catalogues/publication_tests.rs`; profile `tools/catalogue_profile.py`; operator docs
[catalogue-publication.md](catalogue-publication.md)).

**Must fix.**
1. *Orphan unit rows hidden by a join (Sol F2, Sonnet F4).* `read_projections` now reads every
   `catalogue_entry_units` row of the version by `version_id` alone, ordered by `(key,
   ordinal)`; the Rust projections sort unit rows the same way. A unit row whose entry does
   not exist makes the projections differ (`projection-mismatch`).
2. *R8 lock-hold evidence (both F1).*
   - (a) New admission cap: **24,576 unit references per catalogue**, counted with repetition
     (`unit-reference-limit`, `admission::MAX_UNIT_REFERENCES`), tested at the cap and cap + 1.
     The brief proposed 32,768; measured below, the worst measured case at 32,768 held the
     lock for 1.99–2.07 s on update (over 2 s in the full run), so the cap was lowered to
     24,576 (3 per entry at 8,192 entries). The entry cap stays 8,192.
   - (b) `tools/catalogue_profile.py` now measures the hold with both endpoints: a poller
     thread probes the write lock without waiting (`BEGIN IMMEDIATE`, zero busy timeout,
     `ROLLBACK`) every ~1 ms from before the publish process is launched until it exits. The
     first refused probe bounds the acquisition, the first free probe after it the release;
     the report gives the hold as a lower and an upper bound (they differ by one poll period,
     about 1 ms; the largest gap between polls was 12.9 ms, once).
   - (c) Every publish is probed: v1, v2 (update) and v3 of the calibrated shape, and a
     maximum fan-out catalogue at the caps, published (`fanout_v1`) and updated (`fanout_v2`,
     one property changed). The calibrated shape now cites 3 units per unit-backed entry
     (21,504 references at 8,192; it cited 4, 28,672, which the new cap refuses).
   - The contender that waits on the lock runs the server's write pattern on its own
     connection: `BEGIN IMMEDIATE` with the server's 5 s busy timeout (`src/store.rs`), one
     insert, `COMMIT`. **This is the lock-level equivalent of a sheet save**: a sheet save is
     an `IMMEDIATE` write transaction (`src/campaigns/sheets.rs`) on a connection with the same
     5 s busy timeout; the
     probe does not exercise the HTTP route or the sheet's own work inside the transaction.
     The 5 s busy timeout, not the 2 s plan budget, is where a concurrent save would actually
     fail. Commit includes `fsync`, so the hold depends on the disk.
3. *Verification under current rules (Sonnet F2).* `catalogue_versions.admission_profile TEXT
   NOT NULL CHECK (admission_profile IN ('catalogue-admission-1'))`, written at publish
   (`admission::ADMISSION_PROFILE`); the column is immutable with the rest of the row
   (`catalogue_versions_update`). Verify and show dispatch re-admission on the stored profile
   (`admission::validate_profile`); an unknown profile is `unknown-admission-profile`.
   Tightening a rule means a new profile id, its validator and a CHECK widened by migration;
   old profiles keep validating old versions.
4. *Index damage aborted the report (Sol F4).* Verify checks the index definition first:
   missing → `name-index-missing`, different → `name-index-definition-mismatch`; the row
   comparison and FTS5 `integrity-check` run only on the migrated definition. A non-storage
   SQLite failure in them is reported as `name-index-integrity-check-failed`.
5. *Verify additions (Sonnet F4).* `PRAGMA foreign_key_check` on each catalogue table
   (`foreign-key-violation`, one statement per table), version numbers contiguous from 1 per
   catalogue (`version-sequence-gap`), no catalogue identity without a version
   (`catalogue-without-versions`); unsealed versions were already reported as
   `version-not-sealed` (code kept). The FTS5 integrity check now propagates busy, locked,
   I/O, out-of-memory, interrupt and disk-full errors instead of reporting them as index
   damage. Tamper tests: extra projection row, identity mismatch, missing seal, orphan row of a
   missing version, renumbered version, catalogue without versions.

**Document only, and the cheap code items taken.**
- Redaction guarantee narrowed (Sol F3): stored catalogue content is never printed; clap's
  argument-parse errors echo the operator's own command-line input and are out of scope.
- Owner text keeps Cf for bidi prose (Sonnet F3): step 3's checklist (§3) now requires isolated
  bidi rendering with a browser test. Names refusing Cf also refuse ZWNJ/ZWJ/soft hyphen:
  accepted and documented; Unicode 16.0 tables, later-assigned Cf unknown. **Blank-looking
  text refused** (trivial): names, labels, the title and owner text need a scalar that is
  neither whitespace nor U+2800/U+115F/U+1160/U+3164/U+FFA0; mixed with visible text these
  stay allowed.
- Draft rewrites (Sonnet F5): **trigger `catalogue_draft_revision_advances`** refuses any draft
  update that does not advance the revision by exactly one (cheap; versions keep their own
  content either way). Three tamper statements in `tests/catalogues.rs` now bump the revision
  so they still reach the check they target.
- Unsealed manual versions block the catalogue (Sonnet F6): operator note in the docs.
- `DROP_SCHEMA` duplication (Sonnet F7): unit test `drop_schema_matches_the_integration_fixture`
  requires the fixture's `DROP_CATALOGUE_SCHEMA` to equal it (integration tests cannot see the
  crate-private constant).
- Identical content citing a now-rejected unit answers `rejected-units`: accepted, the
  exception is documented next to the identical-content promise, with an explicit test. Test
  added: a failed replacement publication keeps the previous index rows.
- Search (step 3): never pass raw user text to `MATCH`; quote tokens as phrases and bound
  their count (§3 checklist).

**Red → green.** Run A (01:01): the new tests written first, against the unfixed `aa23ace`
code. Run B (01:05): after the fixes, with the tamper connection corrected to switch foreign
keys off (the bundled SQLite enables them by default, so in run A two tests failed at setup,
not on the defect), the fix under test temporarily reverted: the unit-row join and unsorted
Rust units restored, `structure()` skipped, and the integrity outcome mapped back to
`is_ok()`. Green: final tree, both runs repeated by the full gate.

| Test | Red | Green |
|---|---|---|
| `tests/catalogue_publication.rs::orphan_unit_rows_fail_show_and_verify` | B: show succeeded ("unexpected success") | pass |
| `…::verify_reports_foreign_keys_sequence_seals_identity_and_extra_rows` | B: `foreign-key-violation` missing (first failing case: catalogue identity renamed) | pass |
| `…::versions_record_their_admission_profile_and_verify_dispatches_on_it` | A: no `admission_profile` column | pass |
| `…::verify_reports_a_missing_or_reshaped_name_index_without_aborting` | A: verify returned a storage error | pass |
| `…::identical_content_citing_a_now_rejected_unit_is_refused` | pass in A (behaviour existed; documentation item) | pass |
| `…::a_failed_replacement_publication_keeps_the_previous_index_rows` | pass in A (behaviour existed; Sol's requested test) | pass |
| `tests/catalogues.rs::storage_rejects_malformed_rows_and_identity_rebinding` (content rewrite without revision bump, revision skip) | A: rewrite accepted | pass |
| `src/catalogues/admission_tests.rs::total_unit_references_are_capped_per_catalogue` | A: cap + 1 accepted | pass |
| `…::blank_looking_names_labels_and_text_are_rejected` | A: blank-looking title accepted | pass |
| `src/catalogues/publication_tests.rs::integrity_check_storage_errors_propagate_and_failures_are_reported` | B: busy reported as a failed check | pass |
| `…::drop_schema_matches_the_integration_fixture` | guard (both constants already equal) | pass |
| `tools/test_catalogue_profile.py::LockProbeTests` (2), `…::test_fanout_sits_at_the_reference_property_and_byte_caps` | new helper code (prototype tier) | pass |

**Measurements** (`tools/catalogue_profile.py`, release build, container `mimik-ci:latest`,
`--cpus 6`, Ryzen 5 1600 shared host, Monday 01:12–01:15; two full runs at 8,192 and one at
4,096; database on the container's overlay filesystem). Each command ran in a fresh wrapper
process (peak RSS by `RUSAGE_CHILDREN`). Hold = write-lock hold of the publish, lower–upper
bound from the probe, over both runs; "after launch" is when the publish acquired the lock.
Contender waits are coarse (SQLite's busy handler sleeps up to 100 ms between retries).

| Case (N = 8,192) | Draft bytes | References | Publish wall ms | Lock acquired after launch ms | **Lock hold ms** | Contender wait ms, result | Peak RSS KiB |
|---|---:|---:|---:|---:|---:|---:|---:|
| v1 (8,190 entries, new) | 3,215,441 | 21,498 | 1,519–1,580 | 257–275 | **1,287–1,322** | 1,331–1,332, ok | 116,860–117,068 |
| v2 (8,191, update) | 3,215,719 | 21,501 | 1,820–1,849 | 263–275 | **1,584–1,609** | 1,631–1,632, ok | 116,924–117,004 |
| v3 (8,192, update) | 3,215,997 | 21,504 | 1,422–1,444 | 258–267 | **1,187–1,205** | 1,231–1,232, ok | 116,736–117,996 |
| fan-out v1 (new) | 4,173,017 | 24,576 | 2,153–2,154 | 398–428 | **1,736–1,755** | 1,732, ok | 143,892–145,220 |
| fan-out v2 (update) | 4,173,018 | 24,576 | 2,228 | 381–400 | **1,841–1,845** | 1,832, ok | 144,204–144,404 |

Other commands at 8,192 (both runs): import 302–515 ms, peak RSS ≤ 121,320 KiB; compare
333–567 ms, ≤ 86,828 KiB; `show-catalogue-version` 348–350 ms, 70,532–70,868 KiB;
`verify-catalogues` over 5 versions (2 catalogues, structure and foreign-key checks included)
2,490–2,507 ms, 92,024–92,648 KiB, status ok; `rebuild-catalogue-index` 199–244 ms; baseline
`list-rulesets` 41 ms, 16,740 KiB. At 4,096: holds 567–642 ms (calibrated) and 1,456–1,512 ms
(fan-out at the full 24,576 references and 4 MiB), RSS ≤ 129,616 KiB.

Cap selection, fan-out only (one run each, a fresh database without the calibrated catalogue,
so about 50–70 ms faster than in the full run):

| References | fan-out v1 hold ms | fan-out v2 hold ms | Peak RSS KiB |
|---:|---:|---:|---:|
| 16,384 | 1,382 | 1,497 | 137,940 |
| 24,576 | 1,586 | 1,682 | 147,068 |
| 32,768 | 1,819 | 1,993 | 153,960 |

In the first full run at 32,768 (before the cap change), fan-out v2 held the lock for
2,065–2,067 ms and fan-out v1 for 1,909–1,910 ms; the calibrated v2 (then 28,672 references)
1,766–1,767 ms.

Every CLI process also takes the write lock for about 30–40 ms when it opens the store
(`Store::migrate` runs in an `IMMEDIATE` transaction even when nothing migrates); the probe
reports it separately and the contender only starts after 50 ms of refusal, so it is not
attributed to publication.

**R8 at 8,192 entries, with the caps above:** worst measured hold 1,845 ms ≤ 2 s (margin about
0.15 s on this host), a concurrent server-pattern writer waits and succeeds in every case, peak
CLI RSS about 142 MiB ≤ 256 MiB. The margin is thin: a slower disk or a busier host could
exceed 2 s, though not the 5 s busy timeout at these sizes. If later measurements on the
target hardware exceed 2 s, lower `MAX_UNIT_REFERENCES` to 16,384 (1.5 s here) before the
entry cap.

**Limitations:** one host, two runs; the database lives on the container's overlay filesystem;
units all unreviewed; the contender is the lock-level equivalent of a sheet save, not the HTTP
route; the fan-out shape trades owner text for references and properties to stay within
4 MiB (owner text 54 bytes per owner-written entry), and other distributions within the caps
were not profiled.

**Author gate** (container, `--cpus 6`, Monday 01:16–01:26): `cargo fmt --check` and pedantic
Clippy (`--all-targets --all-features -D warnings`) clean; `cargo test --locked --all-targets
--all-features` all passed: 525 tests (339 library, 27 publication and 13 draft integration
tests among them), 1 pre-existing ignored; `ruff check tools` and `ruff format --check tools`
clean; tools unittest 730 run, 11 skipped, **1 error:
`test_run_queue.ProcessLauncherTests.test_fake_runner_script`**, the known pre-existing error
(identical on `548cc84` and `aa23ace`), untouched here.

**Not yet done:** independent re-review of these fixes.

## 13. Step 2 round-2 review fixes (2026-10-19)

Re-review of `cb9ddb8`: Sol FIX THEN MERGE, Sonnet FIX THEN MERGE, third chain unavailable.
Adjudicated items (brief `cat-step2-fix2`) and what changed. Code:
`src/catalogues/{admission,decode}.rs`; tests `src/catalogues/admission_tests.rs`,
`tests/catalogue_publication.rs`; profile `tools/catalogue_profile.py` and
`tools/test_catalogue_profile.py` (prototype tier); operator docs
[catalogue-publication.md](catalogue-publication.md). Wording rule from this round: every
"worst" figure is the **worst measured case on this host**, not the worst admitted shape;
§12's figures are superseded by the ones below.

**Must fix.**
1. *Probe bounds (Sol 1).* §12's probe stamped each observation with the time taken before
   `BEGIN IMMEDIATE`, so a probe descheduled across the release could make the "upper" bound
   shorter than the hold, and one descheduled across the acquisition, or refused by the
   contender, could make the "lower" bound longer. Each probe now records its start and its
   completion; its lock attempt happened at an unknown instant of that window
   (`publish_hold`, documented in the code). For a refusal run `first..last` between free
   probes `before` and `after`:
   - acquisition in `(before.start, first.end]`, release in `[last.start, after.end)`;
   - **hold ≥ `last.start − first.end`** (0 if negative) and **≤ `after.end − before.start`**.
   The contender can hold the lock only after its `BEGIN` is issued, so a refused probe ending
   after that may be the contender's and is not attributed; and since it acquires only after
   the publish releases, the time its `BEGIN` returned also caps the release. Excluding those
   probes left the lower bound at about 50 ms (the contender starts 50 ms into the hold), so
   each refused probe also reads `count(*)` of `catalogue_versions`: a read that does not yet
   see the new version ran before the commit, hence before the release, contender or not.
   Reads are refused while the publish holds `EXCLUSIVE` (rollback journal, after a cache
   spill), so with the contender the lower bound stays loose (83–177 ms below). Each case
   therefore runs twice: **with the contender** (its wait and success, upper bound) and
   **without** (`--no-contender`, both bounds within about 1.5 ms).
2. *Worst-case shapes (Sol 2, Sonnet A).* Two shapes added beside the fan-out, each published
   (`_v1`) and updated (`_v2`, one property changed, replacing that catalogue's index rows), at
   8,192 entries and just under 4 MiB:
   - **FTS-heavy** (`fts_heavy`): 99-scalar names of 20 four-character ASCII tokens, every
     token used once in the catalogue (163,840 distinct tokens, spread 3-character prefixes;
     ASCII because the index folds diacritics), every reference used, owner text squeezed to
     80 KB;
   - **declaration-heavy** (`declaration_heavy`): 32 types x 16 enum properties x 128 options
     (65,536 declared choices, shortest ids and labels), every entry holding 16 enum values,
     and the most references that fit (9,056 at the old cap; all 8,192 now).
   At the 24,576 cap the FTS-heavy update held the lock for **2,353–2,354 ms** (no contender),
   over the 2 s budget. Caps lowered as the brief orders, references first, measured without
   the contender:

   | References (entries 8,192) | fts v1 hold ms | fts v2 hold ms | fan-out v2 hold ms |
   |---:|---:|---:|---:|
   | 24,576 | 1,985–1,987 | 2,353–2,354 | 1,795–1,797 |
   | 16,384 | 1,773–1,775 | 2,143–2,144 | 1,481–1,483 |
   | 12,288 | 1,613–1,615 | 1,922–1,923 | 1,327–1,329 |
   | 8,192 | 1,434–1,436 | 1,759–1,760 | 1,220–1,221 |

   Full runs except 12,288 (fan-out shapes only, a fresh database without the calibrated
   catalogue, so somewhat faster), one run each, before the ASCII filler fix (item 3).

   **Decision: `MAX_UNIT_REFERENCES` = 8,192** (1 per entry at 8,192 entries); the entry
   cap stays 8,192. 12,288 fits on paper but by 0.08 s, inside the run-to-run spread seen here
   (up to 0.2 s). Recorded alternative for the user: entries 6,144 with references 16,384
   (fts v2 1,791–1,793 ms, one run). The distinct-unit cap (16,384, D5) is now unreachable and
   kept as defence in depth (`const` assertion that the reference cap does not exceed it).
   The calibrated shape now cites 1 unit per unit-backed entry (7,168 references at 8,192).
3. *Profiler hardening (Sol 3).* `LockProbe`: the poller keeps any failure and hands it to the
   main thread; startup waits at most `ready_timeout` (10 s) for 20 free probes; the publish
   runs in `try`/`finally` with stop and join; only `SQLITE_BUSY`/`SQLITE_LOCKED` count as
   contention, any other error fails the measurement (`ProbeError`), in the poller and the
   contender alike. Also: the text-property filler is ASCII, so the byte budget is exact (a
   4,096-entry run overflowed 4 MiB with multibyte filler).
4. *Blank-looking owner text (Sonnet B).* Required text now needs a scalar that is neither
   whitespace, blank-looking nor Cf. Owner text of only Cf (ZWSP, LRM/RLM, BOM, isolates,
   soft hyphen, ALM, word joiner), whitespace and blank-looking scalars is refused
   (`description-needs-units-xor-owner-text`); Cf mixed with visible text stays allowed.
   Names, labels and the title already refused every Cf. This tightens
   `catalogue-admission-1` in place: nothing has been published under it outside tests
   (step 2 is unmerged), so no new profile id is needed yet; from merge on, the pin below
   applies.
5. *Comments and docs aligned (Sonnet D):* `admission.rs` cap comment and
   catalogue-publication.md now quote §13's measurements and the host-specific wording.
6. *Profile 1 pinned (Sonnet note):* `admission_profile_one_is_pinned` asserts every decode
   and admission limit, the blank-looking set and the SHA-256 of `schemas/catalogue-v1.json`,
   with the rule that changing any of them needs a new profile id.
7. *Ordering regression (Sol):* two unit-backed entries in reverse lexical key order, each
   citing the higher unit id first, with an owner-written entry between; publish, CLI show and
   verify agree, stored unit rows keep author ordinals, and an update that only reorders one
   entry's references counts one `description_references_changed` and verifies.

**Optional items taken:** CLI-level redaction test for `unit-reference-limit` (8,208
references; nothing saved). Plan line 56 was correct (it describes the definition decoder's
`MAX_NODES`, 32,768); reworded to name it and point to the catalogue profile's 524,288. Not
taken: a test for `name-index-integrity-check-failed` on a real corrupted FTS table.

**Red → green.**

| Test | Red | Green |
|---|---|---|
| `tools/test_catalogue_profile.py::LockProbeTests::test_delayed_probes_still_bracket_the_hold` | old start-time method behind the new signature: upper 1.51 s for a 2.0 s hold | pass |
| `…::test_probes_refused_by_the_contender_are_not_attributed` | old method: lower 2.04 s for a 2.0 s hold | pass |
| `…::test_random_delays_always_bracket_the_hold` | old method: error (nothing attributed in a trial; the test now also checks that an unobserved hold yields no run) | pass |
| `…::test_uncommitted_reads_keep_the_lower_bound_tight_under_contention`, `…::test_a_hold_without_a_later_free_probe_has_no_upper_bound` | new with the version-read refinement / guard | pass |
| `…::ProbeLifecycleTests` (4: unopenable and non-database files raise promptly without running the command; a never-free lock times out; a failing command stops the poller; a real 0.3 s hold is bracketed and the contender waits) | old `probe_publish` on an unopenable database hung (killed by `timeout 15`, exit 124) | pass |
| `…::WorstShapeTests::test_every_shape_fits_at_4096_entries_too` | the 4,096-entry profile run failed: draft 4,205,971 bytes | pass |
| `…::WorstShapeTests` (fts and declaration shapes within every limit, decoder nodes ≤ 524,288) | new helper code | pass |
| `src/catalogues/admission_tests.rs::owner_text_of_only_invisible_characters_is_rejected` | accepted an invalid catalogue (ZWSP-only owner text) | pass |
| `…::admission_profile_one_is_pinned` | reference cap 24,576, expected 8,192 (written first) | pass |
| `…::total_unit_references_are_capped_per_catalogue` (now 512 x 16 over 4,096 distinct units) | at cap counted (8,192, 4,096), cap was 24,576 | pass |
| `…::distinct_units_are_capped_per_catalogue` | adapted: the distinct cap is unreachable at the new reference cap; all-distinct accepted up to 8,192, then `unit-reference-limit` | pass |
| `tests/catalogue_publication.rs::reverse_key_order_entries_with_unsorted_references_publish_show_and_verify` | passed on `cb9ddb8` (fixed in round 1); mutation red: Rust unit-row sort removed → `projection-mismatch` | pass |
| `…::cli_reference_limit_rejection_is_redacted` | guard (behaviour existed) | pass |

**Measurements** (`tools/catalogue_profile.py`, release build of this tree, container
`mimik-ci:latest`, `--cpus 6`, Ryzen 5 1600 shared host, Monday 02:20–02:25; at 8,192 entries
two runs with the contender and two without, at 4,096 one of each; database on the
container's overlay filesystem). Each command in a fresh wrapper process (peak RSS by
`RUSAGE_CHILDREN`). Hold bounds in ms: "no contender" = lower–upper over both runs (each run's
bounds differ by about 1.5 ms); "contender" = lowest lower and highest upper over both runs.
Contender waits are coarse (SQLite's busy handler sleeps up to 100 ms between retries).

| Case (N = 8,192) | Draft bytes | Refs | Publish wall ms | Hold, no contender | Hold, contender | Contender wait ms, result | Publish RSS KiB |
|---|---:|---:|---:|---:|---:|---:|---:|
| v1 (8,190, new) | 2,567,041 | 7,166 | 919–944 | 734–739 | 83–738 | 730, ok | 94,676–96,432 |
| v2 (8,191, update) | 2,567,229 | 7,167 | 1,181–1,205 | 987–996 | 172–1,013 | 1,030–1,031, ok | 94,700–95,204 |
| v3 (8,192, update) | 2,567,417 | 7,168 | 960–985 | 779–781 | 176–782 | 730–731, ok | 94,536–96,996 |
| fan-out v1 (new) | 4,170,091 | 8,192 | 1,485–1,504 | 1,145–1,148 | 104–1,147 | 1,130–1,131, ok | 131,504–132,112 |
| fan-out v2 (update) | 4,170,092 | 8,192 | 1,578–1,631 | 1,228–1,271 | 138–1,258 | 1,231, ok | 131,620–132,300 |
| FTS-heavy v1 (new) | 4,170,806 | 8,192 | 1,742–1,765 | 1,416–1,423 | 104–1,412 | 1,431, ok | 132,084–132,288 |
| **FTS-heavy v2 (update)** | 4,170,807 | 8,192 | 2,139–2,164 | **1,797–1,819** | 136–**1,836** | 1,832–1,833, ok | 131,684–132,208 |
| declarations v1 (new) | 4,177,663 | 8,192 | 1,644–1,693 | 1,009–1,017 | 115–1,003 | 1,031, ok | 185,504–186,300 |
| declarations v2 (update) | 4,177,663 | 8,192 | 1,982–2,057 | 1,342–1,351 | 154–1,378 | 1,331, ok | 185,796–186,124 |

Other commands at 8,192 (four runs): import 232–757 ms, peak RSS ≤
160,296 KiB (declarations); compare 233–742 ms, ≤ 127,320 KiB; `show-catalogue-version`
270–287 ms (calibrated) to 674–692 ms (declarations), ≤ 132,180 KiB; `verify-catalogues`
over 9 versions (4 catalogues) 7,487–7,635 ms, ≤ 133,092 KiB, status ok (one short read
transaction per version, §12); `rebuild-catalogue-index` 611–648 ms; baseline `list-rulesets`
40–46 ms, ≤ 17,208 KiB. Largest probe window 12.9 ms, largest gap between probes 2.4 ms; the
store-open lock of each CLI process (33–42 ms) is a separate run, not attributed.

At 4,096 entries (one run each way): calibrated holds 324–401 ms; fan-out 858–929; FTS-heavy
v1 1,000–1,008, v2 1,207–1,212; declarations 920–971; publish RSS ≤ 169,360 KiB.

**R8 at 8,192 entries, with the caps above:** worst measured hold on this host 1,797–1,819 ms
without the contender and at most 1,836 ms with it (FTS-heavy update), ≤ 2 s with a margin of
about 0.16 s; a concurrent server-pattern writer waits and succeeds in every case; peak CLI
RSS about 182 MiB ≤ 256 MiB. The margin is as thin as §12's: a slower disk or busier host
could exceed 2 s (not the 5 s busy timeout at these sizes). If measurements on the target
hardware exceed 2 s, the next steps are the entry cap (6,144 with 16,384 references measured
1.79 s here) or the name index's cost on update (deleting a version's rows re-tokenizes every
old name; a contentless or `detail=column` index, or no prefix index, are untested options).

**Limitations:** one host; two runs per mode at 8,192, one at 4,096; overlay filesystem; units
all unreviewed; the contender is the lock-level equivalent of a sheet save, not the HTTP
route; three shapes were profiled, chosen to stress names/FTS, declarations and references
— other admitted distributions were not, so these are the worst measured cases, not a proven
worst case.

**Author gate** (container, `--cpus 6`, Monday 02:38–02:49, final tree): `cargo fmt --check`
and pedantic Clippy (`--all-targets --all-features -D warnings`) clean; `cargo test --locked
--all-targets --all-features` all passed: 537 summed over the test binaries (341 library, 29
publication integration tests among them), 1 pre-existing ignored; `ruff check tools` and
`ruff format --check tools` clean; tools unittest 740 run, 11 skipped, **1 error:
`test_run_queue.ProcessLauncherTests.test_fake_runner_script`**, the known pre-existing error,
untouched here.

**Not yet done:** independent re-review of these fixes.

## 14. Step 2 round-3 review fixes (2026-10-19)

Re-review of `339b23c`: Sonnet MERGE-READY, Space Bunny (third chain) MERGE-READY, Sol FIX
THEN MERGE. Adjudicated items (brief `cat-step2-fix3`) and what changed. Code:
`src/catalogues/{admission,decode}.rs`, `schemas/catalogue-v1.json` (entries `maxItems`);
tests `src/catalogues/admission_tests.rs`, `tests/catalogue_publication.rs`; profile
`tools/catalogue_profile.py` and `tools/test_catalogue_profile.py` (prototype tier); operator
docs [catalogue-publication.md](catalogue-publication.md). §13's measurements and caps are
superseded by the ones below; the wording rule stays (**worst measured on this host**).

**Must fix.**
1. *Publication identity in the lock probe (Sol 1, Space Bunny M1/M2/N6).* §13 picked the
   refusal run with the largest lower bound and capped its release at the contender's
   acquisition. If the CLI's store-open lock launched the contender, the contender could
   acquire *before* the publish, and a merged run was then capped far below the publish's
   hold; an observed store-open run could also be reported when the publish itself went
   unobserved. Replaced by `publish_span`, which identifies the publish by the version it adds
   instead of by run shape: every probe, free or refused, reads `count(*)` of
   `catalogue_versions`, and a free probe reads it while holding the write lock itself, so
   - a free probe reading the old count took the lock before the publish acquired it;
   - a free probe reading the new count took it after the publish released.
   The hold therefore lies between the start of the **last free old-count probe** and the end
   of the **first free new-count probe**; between them every probe is refused, so this is the
   one run that contains the publish, chosen by construction. Its length is a **conservative
   upper bound** whatever else ran inside (store-open lock, contender): they only widen it.
   No contender cap is applied any more. The lower bound (refused probes inside the span) is
   reported as indicative only. After the command the probe waits (≤ 5 s) for the closing
   new-count probe; if either free probe is missing the hold is reported as
   `bounded: false` with a reason, never as a null number. R8 hold evidence now comes from
   `--no-contender` runs; runs with the contender report its result and bounds labelled
   loose. The contender reads the version count inside its own transaction:
   `waited_through_publish` is true when it began before the release (some probe starting
   after its `BEGIN` still read the old count) and acquired after it (it read the new count).
   **R8 gate:** `--max-hold-ms` (default 2,000) makes the profiler exit 1 when any publish's
   upper bound exceeds it or is unbounded, or the contender did not start, fail, or not wait
   through the publish (`r8_failures`, reported as `r8_gate` in the JSON).
2. *Outward rounding (Sol 2).* `floor_ms` for lower bounds and earliest instants, `ceil_ms` for
   upper bounds and latest instants (0.1 ms), probe window and gap maxima ceiled.
3. *Comments and docs (Sol 3, Sonnet):* the `admission.rs` cap comment and
   catalogue-publication.md quote the closing measurements below.
4. *Profiler caps bound to Rust (Space Bunny M5).* `RUST_LIMITS` in the profiler names each
   limit's Rust constant; `RustLimitTests` parses every `const MAX_*: usize` from
   `src/catalogues/{decode,admission}.rs` and fails on any difference. The shapes and the
   shape tests use these constants instead of literals (also N2: per-entry owner text, text
   property, key and name caps are now asserted).
5. *Text property values (Space Bunny M3).* A non-empty text property value must pass the
   same `label` rule as names (`blank-text-property-value`): whitespace-only, U+2800-only and
   similar values are refused. The empty string stays allowed as the explicit "no value"
   (the schema example's `old-glow` entry uses it); recorded as a deliberate exception.
6. *Lone marks (Sonnet).* `label` also excludes nonspacing marks (Mn: combining marks such as
   U+0301 and U+034F, variation selectors U+FE0E/U+FE0F, U+17B4/U+17B5). Text of only
   whitespace, Cf, Mn and blank-looking scalars is refused in names, labels, the title, owner
   text and text property values; mixed with a base it stays allowed. Tightens
   `catalogue-admission-1` in place under §13's premise (step 2 unmerged, nothing published
   outside tests).
7. *Naming and D5 (Space Bunny M4, M6).* `distinct_units_are_capped_per_catalogue` renamed
   `all_distinct_units_are_bounded_by_the_reference_cap` (it checks that the reference cap
   refuses first; `distinct-unit-limit` is unreachable). D5's table gains the
   unit-reference row and the unreachable note, and points to §9/§14 for the values in force.
8. *Two more shapes (Sonnet).* Both at the caps, just under 4 MiB, every reference used:
   - **fts3** (`fts3_heavy`): 99-scalar names of 25 three-character ASCII tokens (each token
     its own prefix-index entry), cycling all 46,656 base-36 tokens so each appears several
     times;
   - **keys** (`long_keys`): 64-scalar entry keys on the FTS-heavy names (keys are stored in
     entry, unit, seal and name-index rows).
   **Both exceeded 2 s at the §13 caps**, so the caps were lowered as the brief orders,
   references first, then entries (conservative upper bounds, no contender, worst of the two
   new shapes, ms; "fresh" = fan-out shapes only, a database without the calibrated
   catalogue):

   | Entries | References | Runs | fts3 v1 | fts3 v2 | keys v2 | Gate |
   |---:|---:|---|---:|---:|---:|---|
   | 8,192 | 8,192 | 1 full | 2,031.9 | 1,993.4 | 2,070.9 | fail |
   | 8,192 | 6,144 | 1 fresh | 1,596.4 | 1,932.2 | 2,068.2 | fail |
   | 8,192 | 4,096 | 1 fresh + 2 full | 1,452.4–2,024.1 | 1,791.1–1,845.8 | 1,793.1–1,933.6 | fail (1 of 3) |
   | 6,144 | 8,192 | 2 full | 1,556.5–1,565.7 | 1,823.4–1,990.0 | 1,749.4–1,776.6 | pass, margin 0.01 s |
   | 6,144 | 6,144 | 3 full (old binary) + 3 closing | 1,471.0–1,544.9 | 1,704.3–1,804.0 | 1,650.5–1,714.4 | pass |

   The reference cap is a weak lever (Space Bunny's slope, below): at 8,192 entries even 4,096
   references failed once. **Decision: entries 6,144 (`MAX_ENTRIES`, schema `maxItems`) and
   unit references 6,144 (`MAX_UNIT_REFERENCES`, 1 per entry)**; 6,144 entries with 8,192
   references came within 0.01 s of the budget. The calibrated shape now has 5,376
   references at 6,144 entries. Pin and tests updated (pin red first, below).
9. *Where the hold goes (Space Bunny).* From §13's cap-selection table the FTS-heavy update
   at 8,192 entries is about **1.27 s + 35 µs per reference**, so references were never the
   main cost. Fan-out v2 and FTS-heavy v2 differ only in names at equal bytes and
   references, and FTS-heavy held about 0.5 s longer: roughly **0.5 s of the hold is FTS
   re-tokenizing the previous version's names** when its rows are deleted. That is the next
   lever: a contentless or `detail=column` name index, or no prefix index, as a measured
   experiment before any cap is raised again. Alternatives for the user: §13's "6,144
   entries with 16,384 references" is **no longer supported** (8,192 references at 6,144
   entries already reached 1.99 s with fts3); the measured options are 6,144/6,144 (chosen)
   or the index experiment to win back 8,192 entries.
10. *Inert `# noqa: BLE001` removed (Space Bunny N8):* ruff selects `E4, E7, E9, F, I, B`, not
    `BLE`.

**Red → green.**

| Test | Red | Green |
|---|---|---|
| `tools/test_catalogue_profile.py::PublishSpanTests::test_startup_contender_then_publish_with_missed_free_gaps` | §13 method (`339b23c` `publish_hold`, max-lower run): hold (1.95, **0.096**) s for a 2.0 s hold | pass |
| `…::test_observed_startup_with_unobserved_publication` | §13 method: hold (0.0, **0.100**) s, the store-open run reported, for a 2.0 s hold | pass |
| `…::test_an_unbounded_hold_is_reported_and_fails_the_gate` | §13 `run_ms`: `hold_ms_upper: null` with no failure | pass |
| `…::test_bounds_round_outward` | §13 `run_ms`: upper 1,819.0 for 1,819.049 ms, lower 1,818.0 for 1,817.99 | pass |
| `…::test_random_schedules_always_bound_the_hold_from_above` (500 trials with store-open lock and contender before or after the publish), `…::test_delayed_probes_still_bracket_the_hold` | ported to `publish_span` | pass |
| `…::R8GateTests::test_the_gate_fails_over_the_budget_and_on_a_failed_contender` | new gate (`r8_failures` absent at `339b23c`: the module failed to import) | pass |
| `…::ProbeLifecycleTests::test_a_real_hold_is_bracketed_and_the_contender_waits` | now runs without and with the contender; checks `waited_through_publish` | pass |
| `…::RustLimitTests::test_profiler_limits_match_the_rust_constants` | absent `RUST_LIMITS` at `339b23c` (import error); mutations: Python reference cap 16,384 → `16384 != 8192`, Rust reference cap 12,288 → `8192 != 12288`; then Python caps at 6,144 before the Rust change → `6144 != 8192 : MAX_ENTRIES` | pass |
| `…::WorstShapeTests` (fts3 and keys shapes within every limit) | new helper code | pass |
| `src/catalogues/admission_tests.rs::blank_text_property_values_are_rejected` | accepted an invalid catalogue (U+2800-only value) | pass |
| `…::text_of_only_nonspacing_marks_is_rejected` | accepted an invalid catalogue (lone U+034F) | pass |
| `…::admission_profile_one_is_pinned` | written first with 6,144/6,144: left `[…, 8192, …, 8192, …]` | pass (schema hash updated) |
| `…::entry_count_accepts_6144_and_rejects_6145`, `…::total_unit_references_are_capped_per_catalogue` (384 x 16 over 3,072 units), `tests/catalogue_publication.rs::cli_reference_limit_rejection_is_redacted` (385 x 16) | adapted to the new caps | pass |

**Closing measurements** (`tools/catalogue_profile.py`, release build of this tree, container
`mimik-ci:latest`, `--cpus 6`, Ryzen 5 1600 shared host, Monday 03:19–03:26; 6,144 entries,
three runs without the contender and two with it, one without at 4,096; database on the
container's overlay filesystem). Each command in a fresh wrapper process (peak RSS by
`RUSAGE_CHILDREN`). "Upper, no contender" is the conservative R8 figure (largest over three
runs, ceiled); "loose, contender" is the largest span with the contender inside. All five
runs passed the R8 gate.

| Case (N = 6,144) | Draft bytes | Refs | Publish wall ms | Upper, no contender | Loose, contender | Contender wait ms, result | Publish RSS KiB |
|---|---:|---:|---:|---:|---:|---:|---:|
| v1 (6,142, new) | 2,186,462 | 5,374 | 658–669 | 497.9 | 483.1 | 530, waited ok | 81,432–83,528 |
| v2 (6,143, update) | 2,186,651 | 5,375 | 737–775 | 585.0 | 577.6 | 530, waited ok | 81,896–83,704 |
| v3 (6,144, update) | 2,186,843 | 5,376 | 905–932 | 764.3 | 752.6 | 730, waited ok | 81,680–82,384 |
| fan-out v1 (new) | 4,157,306 | 6,144 | 1,179–1,262 | 957.6 | 892.5 | 931, waited ok | 121,980–122,924 |
| fan-out v2 (update) | 4,157,307 | 6,144 | 1,266–1,308 | 1,003.8 | 992.1 | 931–1,030, waited ok | 122,500–122,624 |
| FTS-heavy v1 (new) | 4,158,988 | 6,144 | 1,428–1,449 | 1,137.1 | 1,129.5 | 1,131, waited ok | 125,144–125,328 |
| FTS-heavy v2 (update) | 4,158,989 | 6,144 | 1,690–1,795 | 1,480.9 | 1,407.0 | 1,431, waited ok | 125,172–125,468 |
| declarations v1 (new) | 4,177,285 | 6,144 | 1,582–1,623 | 1,050.9 | 1,049.4 | 1,030–1,031, waited ok | 176,092–176,692 |
| declarations v2 (update) | 4,177,285 | 6,144 | 1,618–1,668 | 1,111.3 | 1,095.9 | 1,130–1,131, waited ok | 176,612–177,080 |
| fts3 v1 (new) | 4,158,989 | 6,144 | 1,780–1,846 | 1,538.2 | 1,520.8 | 1,531–1,532, waited ok | 124,916–125,404 |
| **fts3 v2 (update)** | 4,158,990 | 6,144 | 2,024–2,125 | **1,804.0** | 1,775.2 | 1,732, waited ok | 124,736–125,308 |
| keys v1 (new) | 4,165,131 | 6,144 | 1,591–1,667 | 1,378.8 | 1,315.9 | 1,331–1,332, waited ok | 125,168–125,292 |
| keys v2 (update) | 4,165,132 | 6,144 | 1,956–2,010 | 1,714.4 | 1,715.6 | 1,632–1,732, waited ok | 124,884–125,236 |

Indicative lower bounds (no contender) were within 1.6 ms of the upper bounds in every case.
Other commands (five runs): import 195–644 ms, peak RSS ≤ 150,120 KiB; compare 191–629 ms,
≤ 116,672 KiB; `show-catalogue-version` 232–247 ms (calibrated) to 570–612 ms
(declarations), ≤ 120,608 KiB; `verify-catalogues` over 13 versions (6 catalogues)
8,367–8,443 ms, ≤ 120,796 KiB, status ok; `rebuild-catalogue-index` 935–1,043 ms; baseline
`list-rulesets` 41–47 ms, ≤ 17,508 KiB. Largest probe window 8.6 ms, largest gap between
probes 2.8 ms. At 4,096 entries (one run, no contender): calibrated ≤ 400.7 ms; worst fts3 v2
1,280.0 ms; publish RSS ≤ 165,988 KiB.

**R8 at 6,144 entries, with the caps above:** worst measured hold on this host at most
1,804.0 ms (conservative upper bound, fts3 update), ≤ 2 s with a margin of about 0.2 s; the
concurrent server-pattern writer waited through the publish and succeeded in every case;
peak CLI RSS about 173 MiB ≤ 256 MiB. The margin is about the run-to-run spread seen in
this round (fts3 v2 1.70–1.80 s at the same caps): a slower disk or busier host could
exceed 2 s (not the 5 s busy timeout). Next lever: the name index (item 9).

**Limitations:** one host; three runs without the contender and two with it at 6,144, one
at 4,096; the cap-selection rows are one to three runs each and the first rows ran on the
binary before the cap change (identical publication code); overlay filesystem; units all
unreviewed; the contender is the lock-level equivalent of a sheet save, not the HTTP route;
five shapes were profiled — other admitted distributions were not (for example
Space Bunny's N1, 52 owner texts at 20,000 scalars beside 6,092 unit-backed entries, and
multilingual token alphabets), so these are the worst measured cases, not a proven worst
case.

**Author gate** (container, `--cpus 6`, Monday 03:28–03:40, final tree): `cargo fmt --check`
clean (after `cargo fmt` of the new tests) and pedantic Clippy (`--all-targets
--all-features -D warnings`) clean; `cargo test --locked --all-targets --all-features` all
passed: 539 summed over 37 test binaries (343 library, 29 publication integration tests
among them), 1 pre-existing ignored; `ruff check tools` and `ruff format --check tools`
clean; tools unittest (discover in `tools/`) 745 run, 11 skipped, **1 error:
`test_run_queue.ProcessLauncherTests.test_fake_runner_script`**, the known pre-existing
error, untouched here — so the helper suite is not fully green.

**Not yet done:** independent re-review of these fixes; the step-2 review record in
`docs/reviews/`.

## 15. Step 2 round-4 review fixes (2026-10-19)

Round 4 reviewed `4a7c399`:
- Sonnet: MERGE-READY.
- Space Bunny: MERGE-READY.
- Sol: FIX THEN MERGE. Its one Major is in the profiler's R8 gate, not the application.

The record is `docs/reviews/2026-10-19-catalogue-step2.md`.

| Item | Change | Red → green |
|---|---|---|
| Sol 1 (Major, tool): the gate passed with zero gated publications (`--fanout-only --no-fanout`) or a NaN/inf budget (`hold > nan` is always false) | `r8_gate()` fails when no publication was gated, or when the contender was requested but no publication observed it. It reports `publications`, `contender_observed` and `scope` (a `--no-contender` run gates the hold only; R8 needs a passing run of each kind). `hold_budget` is the `--max-hold-ms` type and rejects non-finite and ≤ 0 values | Red: the test module failed to import (`r8_gate`, `hold_budget` absent). On the old code, the inline gate passed with no publications and argparse's `float` accepted `nan`. Green: `test_the_gate_fails_closed_without_publications_or_contender_evidence`, `test_the_budget_must_be_finite_and_positive` |
| Sol 2: the endpoint intervals can belong to the store-open lock or the contender | `span_after_ms`, the outer span, is the reported bound. The endpoint intervals become `indicative_acquired_after_ms` / `indicative_released_after_ms` | Red: `test_bounds_round_outward` asserted the new keys. Green |
| Sol 3, Sonnet, Space Bunny M1: stale "8,192 cap" docstrings and a wrong token count. The comment and docs said the keys shape failed at 4,096 references, but it failed at 6,144 | Docstrings corrected. `admission.rs` and `catalogue-publication.md` now say: keys failed at 6,144 references; fts3 failed once in three runs at 4,096 | Prose only |
| Space Bunny M2, Sonnet: `MAX_DISTINCT_UNITS` was hard-coded in Python; a `MAX_*` name defined in both Rust modules would silently overwrite | `MAX_DISTINCT_UNITS` added to `RUST_LIMITS` and used by the test. `rust_limits()` raises on a duplicate name | Guard: it passes on this tree |
| Space Bunny N4: the profile-1 pin did not freeze the required-text predicate | `label` is now `pub(super)`. `admission_profile_one_is_pinned` asserts it on a frozen accept/reject table: Mn, Cf, blank-looking characters, Me (U+20E3 accepted) and isolated RTL | Guard |
| Sol 4 (Minor): the real-lock profiler test records "released" before COMMIT | **Open, follow-up.** The gate's bound does not depend on it, because the closing free probe must take the write lock itself. A controlled commit-phase regression is listed in the review record | — |

**Gate** (container, `--cpus 6`, Monday 03:53–04:03 and 04:04–04:09):
- `cargo fmt --check`: clean.
- Pedantic Clippy (`--all-targets --all-features -D warnings`): clean.
- `cargo test --locked --all-targets --all-features`: 539 passed, 0 failed, 1 pre-existing ignored, over 37 binaries.
- `ruff check tools` and `ruff format --check tools`: clean.
- Tools unittest (`discover -s tools` from the root): 747 run, 11 skipped, **1 error, the known `test_run_queue.ProcessLauncherTests.test_fake_runner_script`**.

There were no profiling reruns. No cap or shape changed, and the gate's bound computation (`publish_span`) is unchanged.

## 16. Step 2 round-5 review fixes (2026-10-19)

Round 5 reviewed `5d3c538`:
- Sol: MERGE-READY.
- Space Bunny: MERGE-READY.
- Sonnet: FIX THEN MERGE. Its points were profiler coverage and the `TODO.md` merge conflict.

No Blocking findings. The record is `docs/reviews/2026-10-19-catalogue-step2.md`, rows 17–21.

| Item | Change | Red → green |
|---|---|---|
| Sonnet 1, Space Bunny F1: the gate passed reduced runs (`--entries 64`, `--references 0`, `--shapes fanout`, `--fanout-only`) | `r8_gate` records `coverage` (entries, references, shapes, calibrated) and `at_caps`, and fails unless the run is complete at the Rust caps. `--allow-partial` is for cap-selection runs and is never R8 evidence. `--shapes` is validated by `shape_list`, so an empty or unknown shape is refused (empty components too, after round 6) | Red: `test_the_gate_requires_a_complete_run_at_the_caps` (`r8_gate` had no `coverage`). Green |
| Space Bunny F2: the contender was checked once per run, not per publication | `r8_failures(..., contender_expected)` fails a publication that has no contender when one was requested | Red: `test_a_requested_contender_is_required_on_every_publication`. Green |
| Space Bunny F7: migration 019 bounded entry ordinals at 8,191 | Changed to 6,143. `migration_ordinal_bounds_follow_the_caps` ties the three ordinal CHECKs (types, entries, units) to the Rust caps | Red on the old migration (`left: 0, right: 1`). Green |
| Space Bunny F5, F8, F9; Sonnet 3 | The record names `5d3c538` and gains the round-5 rows. The 3,000-trial simulation is attributed to Sonnet's own scratch run. The `RUST_LIMITS` comment is corrected. The operator docs describe every gate condition and the hold-only scope | Prose only |
| Sonnet 1, merge | The `TODO.md` conflict with master `e6ecc30` is resolved at merge time | — |

There were no profiling reruns: caps, shapes and `publish_span` are unchanged. The recorded closing runs were complete runs at the caps (§14).

**Gate** (container, `--cpus 6`, Monday 04:16–04:27):
- `cargo fmt --check`: clean after `cargo fmt` of the new test.
- Pedantic Clippy (`-D warnings`): clean.
- `cargo test --locked --all-targets --all-features`: 540 passed, 0 failed, 1 pre-existing ignored, over 37 binaries.
- `ruff check tools` and `ruff format --check tools`: clean.
- Tools unittest: 749 run, 11 skipped, **1 error, the known `test_fake_runner_script`**.

**Round 6** (`fe93021`): Sol, Sonnet and Space Bunny all **MERGE-READY**, with no Blocking
or Major findings. Before the merge, the cheap Minors they named were folded in:
- `shape_list` refuses empty components (`fanout,,fts`).
- Coverage needs an explicit `calibrated: True`, so a missing key no longer counts as calibrated.

Both have failing-first tests (`test_shape_lists_refuse_empty_and_unknown_components`,
`test_coverage_without_an_explicit_calibrated_flag_is_partial`).

Left as noted follow-ups: the ordinal pin test matches bounds by value rather than by table,
and the coverage reported in the gate comes from the CLI arguments, not from the cases
actually run.

**Merge.** Branch `catalogue` (steps 1+2, through `daa3d2f`) was merged into master on 2026-10-19. The only conflict was textual, in `TODO.md`.

Gate on the merge result (container, `--cpus 6`, Monday 04:32–04:43):
- `cargo fmt --check`: clean.
- Pedantic Clippy (`-D warnings`): clean.
- `cargo test --locked --all-targets --all-features`: 540 passed, 0 failed, 1 pre-existing ignored.
- `ruff check tools` and `ruff format --check tools`: clean.
- Tools unittest: 751 run, 11 skipped, 1 error, the known `test_fake_runner_script`.

The release build, the packaging smoke tests, Chromium and LAN checks were not run, because step 2 adds no route, page or template. They belong to step 5's full gate.

## 17. Step 3 implementation notes (2026-10-19)

Branch `catalogue-step3` from master `b640fca`. Scope: §3 step 3 (the `catalogues`
definition member, the three-site checks, the browse and entry routes reached through a
sheet, with no `catalogue_ref`), the §12 carry items (quoted `MATCH`, isolated bidi
rendering with a browser test) and review row 22 (ordinal pin by table). Operator
documentation: [catalogue-publication.md](catalogue-publication.md#reading-through-a-sheet),
[runtime-definition-contract.md](runtime-definition-contract.md#linked-catalogues),
[definition-drafts.md](definition-drafts.md).

Code:
- `src/definitions/{mod,decode,admission,store,publication}.rs`, `schemas/definition-v1.json`:
  the member (`CatalogueLink`, last, omitted when empty), decode limit (8 links), domain check
  (registry slugs, unique), and `linked_catalogues` in `store::references` (sites 1 and 2: draft
  save, and the comparison and approval transaction through `snapshot`) and in
  `private_evidence_available` (site 3: a missing sealed version makes readiness `Stale`).
- `src/catalogues/read.rs`: latest sealed version (through `catalogue_seals`, identity columns
  checked), browse (50 per page, `(name, key)` order and exclusive cursor, optional type and
  name search through `fts_query`, joined back on the same catalogue's latest version and
  restricted to its rowid range, every statement under `Budget`), entry (metadata of every
  cited unit first, then `substr` prefixes up to 20,000 characters, live badges and
  citations). `src/sources/search.rs`: `guarded`, `read_guarded`, `Budget`, `interrupted`
  widened to `pub(crate)` (R16).
- `src/campaigns/catalogue.rs`: `Store::private_catalogue`, the R6/R25 phases under the runtime
  permit; `links` for the sheet page. `src/campaigns/catalogue_web.rs`, templates
  `catalogue_browse.html`, `catalogue_entry.html`, `private_sheet.html` (a "Catalogues"
  section, links in a new tab), `assets/app.css`.
- Browser: `tools/catalogue_browser.py` (called from `browser_smoke.py`'s workspace stage);
  `smoke_release.py` publishes a synthetic catalogue through the CLI and links it from the
  generic workspace definition; `containers/browser.Dockerfile` copies the new module.

**Decisions taken in step 3 (not fixed by the plan; conservative choices):**
1. *Refusal shape.* The test list says "return 404"; D11 says "the same 404/403 shape as
   today". Today an unreadable sheet answers 403 (`AppError::NotFound` maps to 403 in the
   campaign routes). Every refusal (unreadable or unknown sheet, native or unsupported pin,
   unlinked, unknown or other games' catalogue, unknown key, unknown entry type) returns
   `NotFound`, and the tests require the **byte-identical response of an unreadable sheet**
   (403). Malformed path segments, cursors, types, unknown or duplicate query parameters and
   queries over 100 scalars answer 400 before any lookup (syntax only, nothing probed).
2. *Member spelling.* `catalogues: []` is refused (`minItems` 1) rather than canonicalised
   away, so there is one spelling of "no links". Author order is kept and is identity (the
   sheet's display order), as for fields.
3. *Readiness and reading are separate.* Catalogue routes require a readable sheet and a
   sealed version of the linked catalogue, not current wiring: a sheet whose approval went
   stale stays readable with its catalogues, as the sheet itself does.
4. *Retired entries* stay in browse results with a "Retired" badge and their entry pages stay
   readable; only picking (step 4) stops offering them.
5. *Malformed units.* If any cited unit is not text, holds NUL or exceeds 256 KiB, the whole
   description shows "Description unavailable (source text malformed)", like a rejection;
   no partial text.
6. *A concurrent sheet save* between the two read transactions answers 409 (the
   `private_badge` rule); the reader reloads.
7. *A query that cannot run* (no letters or digits, control characters, more than 16 pieces,
   the time budget) re-renders the search form with the static message, status 422, marked
   as a trusted page so the auth layer keeps its body.
8. *Badge of a unit-sourced entry with no unit rows* (only damage causes it) is "Unchecked",
   never "Reviewed" (found in self-review, test first).
9. The sheet page lists catalogue links on view and history pages, not on the create page
   (the routes need a saved sheet).

**Red → green.** Red runs used the final tests against: (S) a skeleton (definition schema,
decoder, domain check and readiness site at master, the save-site call removed, browse,
entry, the store method and the sheet links stubbed to empty or `NotFound`); or one narrow
mutation of the final code each, named below. Green is the final tree (05:37–05:43 run).

| Test | Red | Green |
|---|---|---|
| `definitions::catalogue_link_tests::catalogue_links_are_closed_bounded_and_unique` | S; mutation: domain check accepts any id (invalid and duplicate links admitted) | pass |
| `…::catalogue_links_are_the_last_member_and_part_of_the_identity` | S (member refused by the schema) | pass |
| `…::frozen_definition_vectors_are_unchanged_without_catalogues` | guard in S; mutation: member serialised when empty (`"catalogues":[]` appended, hash changed) | pass |
| `catalogues::read::tests::browse_orders_by_name_then_key_with_an_exclusive_cursor` | S; mutation: cursor seek removed | pass |
| `…::name_search_quotes_every_token_and_filters_without_ranking` | S; mutation: raw `q` passed to `MATCH` | pass |
| `…::colliding_keys_across_catalogues_never_leak_and_only_the_latest_names_match` | S; mutation: index join not restricted to the catalogue's rowid range (the other catalogue's row joined the colliding key) | pass |
| `…::browse_is_interrupted_past_its_time_budget` | S; mutation: budget raised to an hour | pass |
| `catalogues::read::badge_tests::unit_badges_follow_states_and_page_completion` | mutation: no cited unit counts as reviewed (the pre-fix behaviour) | pass |
| `campaigns::tests::catalogue_tests::sheet_links_its_catalogues_and_routes_follow_the_sheet_read_matrix` (custodian, Master, other player, outsider, Owner, frozen sheet, disabled account) | S | pass |
| `…::unlinked_unknown_foreign_and_native_catalogues_answer_like_an_unreadable_sheet` | S | pass |
| `…::entry_badges_descriptions_and_citations_are_live` (review, correction, page completion, label change, rejection) | S; mutations: rejection does not hide text; printed label not read | pass |
| `…::descriptions_are_cut_at_20000_characters_and_owner_text_never` (oversized first unit, cumulative cut, multibyte cut, NUL, owner text, hostile markup, rejected unit after the cut) | S; mutations: no cut; badge from the first unit only; `<bdi>` removed from owner and unit text; `<bdi>` removed from owner text only | pass |
| `…::catalogue_reads_hold_the_permit_and_recheck_grants_after_validation` (Entry, Search, and revocation between the transactions) | S; mutation: the second transaction's recheck and authorization removed | pass |
| `…::definition_links_are_checked_at_save_publication_and_readiness` | S; mutations: save-site call removed (sites 1 and 2 share `references`); check moved from `references` to the draft save only (site 2 red at the approval); readiness site removed (site 3 red) | pass |
| `…::catalogue_publication_never_stales_definitions_or_sheets` (R10: v1 → v2 with existing sheets; two definitions link one catalogue) | S | pass |
| `…::browse_pages_link_to_the_next_page_with_the_filters` | S | pass |
| `…::corrupt_pins_and_snapshots_are_refused` | S, at setup only (a guard: the corrupt cases are refused by existing checks) | pass |
| `catalogues::admission_tests::migration_ordinal_bounds_follow_the_caps` (row 22: bounds matched by table, swapped bounds refused) | mutation: the previous by-value matching (accepted swapped bounds) | pass |
| Chromium `tools/catalogue_browser.py` in `browser_smoke.py --auth`, 1280×720 and 390×844 | release binary built with `<bdi>` removed from the entry template: "Owner text is not in an isolated pre-wrap bidi context" | pass, both viewports |

The first green run also found test defects, fixed before the final runs: a tamper statement
needed foreign keys off; Askama escapes `&` and `<` as `&#38;` and `&#60;`, so one owner-text
assertion was tightened to that spelling (it had an alternative that did not require
`<bdi>`; the owner-only `<bdi>` mutation above was run after the fix).

**Gate** (container `mimik-ci:latest`, `--cpus 6`, Monday 2026-10-19, before 09:00):
- `cargo fmt --check`: clean (05:37). Pedantic Clippy (`--all-targets --all-features -D
  warnings`): clean (05:37).
- `cargo test --locked --all-targets --all-features` (05:37–05:43): **557 passed**, 0 failed,
  1 pre-existing ignored, over 37 binaries (master: 540; 17 new tests).
- `ruff check tools`, `ruff format --check tools`: clean. Tools unittest (05:43–05:52): 751
  run, 11 skipped, **1 error: the known `test_run_queue.ProcessLauncherTests.test_fake_runner_script`**,
  so the helper suite is not fully green, as on master.
- Release build of the final tree (05:52) and `browser_smoke.py --auth` in `mimik-browser`
  (`--network=none --cpus=2`, the CI flags, the worktree's `tools/` mounted on `/checks`),
  05:53–05:54: every stage passed, the catalogue stage at both viewports.
- `smoke_release.py --auth` against the release binary (`--cpus 2`, 05:28–05:29, tree before
  the badge fix, which does not touch the CLI paths it runs): passed.

**Not run:** `cargo audit`, `cargo about`, packaging and the unpacked-archive checks, the LAN
smoke (it now also publishes the synthetic catalogue through `prepare_workspace_materials`),
and any profiling: the R8 page-time criterion (p95 ≤ 250 ms with 8 linked catalogues) was met
by the step 0 spike, but the real routes (two transactions, write-only pin validation) were
not re-measured. Step 5's docs (`release-scope.md`, `local-setup.md`/`sources.md` inventory,
`campaign-workspace.md` R15 notes, README) are not written yet.

**Carry items.** Done: quoted `MATCH` with bounded pieces; isolated bidi rendering with the
Chromium check; ordinal pin by table (review row 22). **Open:** review row 16 (a controlled
commit-phase regression for the profiler's real-lock test), prototype tier, not attempted.

**Open questions** (recorded, not blocking):
- Whether "404" in the step's test list was meant literally; the routes return the existing
  403 refusal shape (decision 1).
- Whether a sheet with stale wiring should still read its catalogues (decision 3 keeps them
  readable).
- Whether retired entries belong in browse results (decision 4 lists them with a badge).

**Not yet done:** independent review of this step.
