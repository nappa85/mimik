# Removing the native MÖRK BORG game (C11)

Owner decision 2026-11-23: Mimik reproduces the sheet, not the table (PROD-1, PROD-5,
PROD-24).
- **Advice:** the situation/action checklist (`src/mork_advice.rs`) is deleted with no
  replacement.
- **Sheet:** the hardcoded MÖRK BORG sheet, form, adapter and registry seed go too.
- **What remains:** sheets come only from runtime definitions the Owner registers. Games and
  editions are never in code.

Inventory: a read-only Opus survey on 2026-11-23. File references below are from that survey;
the implementer re-checks them.

## Slice 1: the advice checklist

- **Delete:**
  - `src/mork_advice.rs`;
  - the `/characters/{id}/context` route, its handler and template (`src/web.rs`,
    `templates/mork_context.html`);
  - `tests/advice.rs`;
  - `tools/smoke_release.py` `smoke_positive_advice` and its call;
  - the CSS used only by it.
- **Docs:**
  - **New decision line in `docs/decisions.md`:** "Mimik reproduces the sheet, not the table:
    no situation or action evaluation and no native games; games come only from
    Owner-registered runtime definitions."
  - **`docs/limitations.md`:** remove PROD-L3, keeping "dice are notation only" if still true
    elsewhere, and ROAD-3 ("situational companion").
  - **`TODO.md`:** drop "situation input" from the pending questions.
  - **Other references:** amend any doc that cites the advice, such as `assisted-entry.md` and
    `architecture.md`.

## Slice 2: the native sheet, the registry's native concept and the seed

Former slices 2 and 3 land together. `src/games.rs` is still called from `registry.rs`,
`definitions/store.rs` and `catalogues/store.rs`, so neither half compiles alone.

### Code
- **Delete:**
  - `src/mork_borg.rs`, `src/mork_form.rs` and `src/games.rs`;
  - the `lib.rs` module declarations and `has_mork_borg_adapter`;
  - the hardcoded `SYSTEMS` label list (mork-borg, dnd, pathfinder, vampire);
  - templates `mork_borg.html`, `mork_editor.html`, `native_fields.html`,
    `native_calculations.html`, the native links in `index.html`, `character.html`,
    `campaign.html` and `private_sheet.html`, and the native CSS.
- **Legacy unauthenticated prototype:** its create form goes, because `create` only
  redirected to the native sheet. Existing legacy characters still open and save their notes
  through the generic page (CAMP-8). No system field remains.
- **Campaigns:**
  - **What goes:**
    - the `version == 0` native create path and the `native_available` rechecks
      (`sheets.rs`);
    - `Pin::Native` and its snapshot code (`snapshots.rs`);
    - the native branches in `campaigns/web.rs`;
    - the `native_available` fields (`domain.rs`).
  - **Security, each with a failing-first regression test:**
    - The `admit_form` branch that admits `NATIVE_KEYS` also serves unsupported pins
      (`campaigns/web.rs` ~2146). It becomes an explicit 400.
    - A create with no immutable pin (today stopped only by `sheets.rs:482`) is refused
      explicitly (400 or 404).
  - **Unsupported pins:** they render through the native form today
    (`campaigns/web.rs` ~1658-1665, `mork_borg::Sheet::default()`). They get a generic
    read-only view.
- **Registry:**
  - `src/registry.rs` loses `native`/`native_adapter` and the "native accepts only the seeded
    alias" rule.
  - The CLI JSON loses `native_adapter`; `templates/admin_rulesets.html` loses its native
    column.
  - `definitions/store.rs` and `catalogues/store.rs` stop refusing the formerly native pair.
    The `registered-nonnative-ruleset-required` error code is retired, and `docs/catalogues.md`
    and `runtime-definition-contract.md` are updated.
  - The search default `fn system() -> "mork-borg"` (`src/sources/web.rs`) and the upload
    placeholder (`templates/upload_form.html`) stop naming a game.

### Tests
- **Delete:** the tests that assert native behaviour:
  - `tests/mork_borg.rs`, `tests/sheet_layout.rs` and `tests/creation_flow.rs`;
  - the native part of `tests/form_newlines.rs`; its CRLF coverage moves to a runtime
    textarea;
  - the native cases in campaigns `tests.rs`, `entry_save_tests.rs`, `suggest_tests.rs` and
    `catalogues.rs`.
- **Rebuild, don't delete:**
  - The `native()`/`insert_native` helpers serve as generic sheets in the custody, freeze,
    privacy, list, home, material, administration and catalogue tests. Rebuild them on a
    published runtime definition (`published_fixture`).
  - The unsupported-pin fixture (`campaigns/tests.rs` ~1584, a `Pin::Native` with calculation
    version 2; assertions in `catalogue_tests.rs` and `list_tests.rs`) is rebuilt on a runtime
    pin with an unsupported schema.
  - The commit message lists every rebuilt test.
- **Seeded pair:** tests and tools that rely on it register the ruleset first, as
  `tests/registry.rs` already does:
  - `source_search.rs`, `source_finalization.rs`, `source_documents.rs`, `home_tests.rs`,
    `registry_tests.rs`;
  - `sources/search.rs` tests, `tests/catalogues.rs`, `tests/definitions.rs`;
  - `tools/material_browser.py`, `tools/admin_ruleset_browser.py`, `tools/smoke_release.py`
    and its `--source-fixture` (CI).
- **Edition column:** the positional `INSERT INTO registry_editions VALUES(...,0)` statements
  (registry.rs, campaigns `tests.rs`, `material_tests.rs`, `catalogue_tests.rs`,
  `tests/registry.rs`, `tests/definitions.rs`, `tests/source_search.rs`) and
  `UPDATE ... SET native=0` (`tests/catalogues.rs`) follow the dropped column.
- **Test data:** tests that merely use "mork-borg" as a system id or real-manual data stay.

### Tools and CI
- `tools/browser_smoke.py`: its native create/edit/htmx/409 flow becomes the same flow on a
  campaign private sheet of a published runtime definition.
- `tools/smoke_release.py`: native routes, the "adapter-bundle" and the create-and-save check
  move to a runtime sheet.
- `tests/support/source_fixture.rs`: the adapter bundle drops `mork_borg::EDITION`.

### Docs
- **Remove the native sheet:** `README.md` (features) and `docs/release-scope.md`.
- **Close:** C11 and CAMP-L5 in `docs/limitations.md`.
- **Amend:** PROD-5, ASSIST-17 and EVAL-5 in `docs/decisions.md`.
- **Remove native-sheet or adapter descriptions:** `architecture.md`, `campaigns.md`,
  `formulas.md`, `runtime-definition-contract.md`, `sources-and-retrieval.md` (adapter
  citations only; the manual data stays), `assisted-sheet-plan.md`, `assisted-entry.md`,
  `source-upload-plan.md`, `source-route-a-plan.md` and `source-import-plan.md`.
  `results.md` stays.

## Schema: migration 038 with a Rust guard

Applied migrations stay unchanged: they are replayed through `include_str!` and pinned by
`migration_files_are_contiguous_and_match_the_applied_chain`, and edits would never reach
databases already at version 37.

- **Guard in `migrate()`:** it runs before step 38, in Rust, like the 037 special case
  (`store.rs` ~145-155). `RAISE` only works inside triggers, so plain SQL cannot refuse with a
  message.
  - **Refuses** with an operator message saying pre-release data must be recreated when native
    data exists: rows in `mork_borg_sheets`, or private sheets of kind `native`.
  - **Seed (owner decision 2026-11-23, keep if used):** if anything references the seeded
    `mork-borg`/`it-2025` pair, the guard keeps it as an ordinary registered game. A reference
    is a row in `campaigns`, `character_rulesets` other than the seeded alias mapping, sources,
    `definition_drafts`, definitions, `catalogues`, `catalogue_drafts` or `registry_events`
    other than its seeding. Otherwise the pair is deleted, so a fresh database ends with no
    game.
- **The migration:**
  - drops `mork_borg_sheets`;
  - drops the `native` column (`ALTER TABLE ... DROP COLUMN` is valid, since no index, trigger
    or view names it);
  - rewrites the `private_sheets_insert` trigger to reject any kind but `runtime`. The column
    CHECK stays: narrowing it needs a table rebuild, which the FKs, triggers, generated columns
    and the transaction with `foreign_keys=ON` rule out;
  - deletes the seed, when unused: drop the immutable-delete triggers on games, editions and
    aliases (005), delete the alias mapping, the alias, the edition and the game, then
    recreate the triggers.
- **Tests:**
  - each refusing dependent;
  - the seed kept when referenced and deleted when not;
  - a clean upgrade from 037;
  - a fresh database.

## Checks

Each slice runs:
- the full container gate (fmt, both clippy runs, all tests, Python tests, ruff, actionlint);
- the browser check of the tools it changes;
- `tools/smoke_release.py` against a release build.

Every regression found gets a failing-first test. One independent review per slice.
