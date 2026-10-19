# Runtime definition contract

A runtime definition describes one game sheet as data: fields, groups, source wiring and,
optionally, formulas, constraints and catalogue links. Mimik renders and validates sheets from
it without game-specific Rust code. A definition goes through a ruleset registry entry, a
revisioned draft, and an immutable published version with an explicit human wiring approval.
Only a published version with current wiring can create new sheets.

Code: `src/definitions/` (`mod.rs` types, `decode.rs` bounded parser, `admission.rs` and
`formula_admission.rs` checks, `store.rs` drafts, `publication.rs` versions and approvals,
`private_pin.rs` sheet pins), `src/registry.rs`, `src/registry_admin.rs`,
`src/auth/rulesets.rs`, trusted schema `schemas/definition-v1.json`, synthetic example
`schemas/definition-example.json` (registered pair `synthetic-game / first`, not a real
sheet). Formulas: [formulas.md](formulas.md). Catalogues: [catalogues.md](catalogues.md).
Sheets, revisions and history: [campaigns.md](campaigns.md); entry and per-unit saves: [assisted-entry.md](assisted-entry.md).

## Envelope

The host schema `schemas/definition-v1.json` (JSON Schema Draft 2020-12) is Mimik's own and
fixed; uploaded JSON Schema is never compiled. Every object is closed (`additionalProperties:
false`, `deny_unknown_fields`). Members, in canonical order:

| Member | Required | Content |
|---|---|---|
| `contract_version` | yes | `1` |
| `game_id`, `edition_id` | yes | A registered, non-native ruleset pair |
| `definition_id` | yes | Slug, unique within the pair |
| `title` | yes | Label |
| `sources` | yes | 0–32 `{source_id, pdf_sha256, roles}` |
| `fields` | yes | 1–256 fields, ordered |
| `groups` | yes | 1–32 groups, ordered |
| `formula_profile` | no | `1` (`cel-mimik-1`); present exactly when a formula, constraint or list exists |
| `constraints` | no | 0–128 advisory constraints; `[]` is accepted and canonicalised away |
| `catalogues` | no | 1–8 catalogue links; `[]` is refused |

Optional members are omitted when absent, so definitions without them keep their canonical
bytes and hash.

### Identifiers and labels

- Stable IDs (`game_id`, `edition_id`, `definition_id`, group, enum option, constraint, rule
  and catalogue IDs) use the registry slug grammar `[a-z][a-z0-9-]{0,63}`
  (`registry::valid_id`).
- Field and item-property IDs also allow `_` (`admission::valid_field_id`). Formulas can only
  reference CEL identifiers (`[a-z][a-z0-9_]*`); a hyphenated scalar field is valid but
  invisible to formulas. List field and item-property IDs must be CEL identifiers, and an
  item property cannot be named `id`.
- Field IDs and group IDs are separate namespaces. Choice IDs are unique per field.
- IDs are data, never paths, templates or CSS selectors. Labels are escaped Unicode text.
- Labels and the title are 1–100 Unicode scalar values and may not be whitespace-only. Text
  is not normalised or trimmed.

### Fields and controls

A field has `id`, `label`, `control`, `nullable`, `mapping`, and optionally `help`
(≤ 2,000 characters), `options`, `items` and `formula`.

| Control | Value | Notes |
|---|---|---|
| `text` | string ≤ 8,000 characters | |
| `multiline` | string ≤ 20,000 characters | |
| `integer` | exact signed i64 or `null` (unknown) | Every integer accepts `null`, nullable or not |
| `boolean` | `true` / `false` | `null` only if nullable |
| `enum` | option ID | Needs 1–128 `options`; 2,048 choices per definition; `null` only if nullable |
| `list` | array of ≤ 256 records | Formula profile only; needs `items`; never nullable, never derived |

`options` is accepted only on enums. Omitted or `null` `help` and non-enum `options`
canonicalise to explicit `null`. `nullable` is a storage and form choice, not a statement
about the game's rules. No decimals, scripts, nested groups, nested lists or uploaded schema
hooks exist.

**Lists.** `items.properties` declares 1–32 properties, each `{id, label, control, optional,
options?}` with a scalar control (no nested list). Each stored record also has a host-managed
`id`. Item integers are i64 or `null`; no other property holds `null`. Row IDs, addressing and
the list form are described in [formulas.md](formulas.md#lists-and-plain-list-editing).

**Formulas and constraints.** A field with `formula: {expr, rule_ids, evidence}` is derived:
computed on saved-sheet views and history views (including the GET that follows a save) from
stored values and the immutable pin, never stored or submitted; a successful save POST
evaluates nothing. A derived field must not be nullable or a list. Constraints are
`{id, expr, rule_ids, message ≤ 500, evidence}`; violations are shown, never enforced. `expr`
is 1–1,024 characters; `rule_ids` is up to 16 slugs, canonicalised as a sorted set. Formula
evidence must cite selected `core` or `supplement` sources; a sheet source alone does not back
a rule. Expressions are compiled at admission. The language, evaluation, result codes and
limits are in [formulas.md](formulas.md).

### Groups and layout

A group is `{id, label, fields, column_hint}` with 1–256 unique field references. Every field
appears in exactly one group. Field and group order are part of the definition identity and
are the display order. `column_hint` is `auto`, `main` or `sidebar`, an advisory reflow hint
for the adaptive sheet layout.

### Sources and mappings

`sources` entries carry a positive database source ID, the lowercase 64-character PDF
SHA-256 and 1–3 unique roles from `core`, `sheet`, `supplement`. Source IDs are unique.

Each field's `mapping` is either `{"state": "mapped", "regions": [...]}` with 1–4 unique
`{source_id, page_id, unit_id}` database references, or `{"state": "unresolved", "reason"}`
with a non-blank reason ≤ 5,000 characters. A mapped region must belong to a selected
`sheet`-role source. Field wiring and formula evidence together reference at most 1,024
distinct regions. Unreviewed source units can be referenced; a mapping is not an approval of
the evidence.

### Linked catalogues

`catalogues` lists `{"id": slug}` links to read-only catalogues of the same game and edition,
unique, in author order (the sheet's display order, part of the identity). A link names no
version; readers use the latest sealed version. The link is checked for existence at draft
save, at publication comparison and approval, and at sheet readiness. Catalogue state never
enters the wiring snapshot or its hash. Details: [catalogues.md](catalogues.md).

## Admission

`admission::validate` runs the same admission for draft import, draft reads, publication and
pin loads.

1. **Bounded decode** (`decode.rs`, over Serde's tokenizer): input ≤ 262,144 bytes
   (`definitions::MAX_BYTES`); depth ≤ 12 containers; ≤ 32,768 nodes (`decode::MAX_NODES`);
   ≤ 32 keys per object, keys ≤ 64 characters; per-key array and string caps applied while
   decoding. Duplicate keys (including escaped equivalents), fraction or exponent tokens,
   integers outside i64 and trailing data fail. All decode failures report the single code
   `malformed-json-or-duplicate-key-or-input-limit`.
2. **Schema validation** with `jsonschema` (Draft 2020-12, `.offline()`, embedded validator
   initialised once): at most 32 diagnostics, each a JSON pointer and keyword escaped and cut
   to 256 characters, plus a truncation flag. Values and raw error messages are not echoed.
3. **Domain checks**: ID grammars, non-blank labels, lowercase PDF hash, unique sources,
   fields, groups and choices, each field in exactly one group, mapping sources and roles,
   the 2,048 choice and 1,024 region limits, catalogue link uniqueness, then the formula
   profile (marker present exactly when used, list shapes, rule IDs, evidence roles, CEL
   compilation). Each failure is one diagnostic with a path and a kebab-case code.
4. **Canonical encoding**: sources sorted by ID, roles sorted (`core`, `sheet`,
   `supplement`), regions sorted, rule IDs sorted and deduplicated; fields, groups, choices,
   properties, constraints, catalogue links and expression text keep their order. The
   canonical bytes must also fit 262,144 bytes (`canonical-byte-limit`).
5. **Hash**: lowercase hex SHA-256 over `b"mimik-definition-v1\0"` plus the canonical bytes.
   This is a Mimik encoding, not JCS. A frozen hash vector of the example definition guards
   it (`admission.rs` tests).

**Database references** (`store::references`, inside the write transaction): the pair must be
registered, not native and without a compiled adapter (`registered-nonnative-ruleset-required`);
each source must exist with that PDF hash and be associated with the same pair; each region's
unit must belong to that page and source; each linked catalogue must have a sealed version.

**Concurrency.** One admission job per process (`definitions::admit`, a one-permit
semaphore, no queue), acquired before the input file is read (`local_file::read_path`: no
symlinks, regular files, size-checked) and owned by the blocking worker, so a cancelled
caller holds it until the work ends. A second job fails with `Busy`; sheet writes wait up to
2 s for it (`admit_within`). Across processes, SQLite transactions, uniqueness and revisions
decide; a loser gets a conflict, busy or storage error.

## Limits

| Resource | Limit | Code |
|---|---|---|
| Definition input and canonical bytes | 262,144 bytes each | `definitions::MAX_BYTES` |
| Decoder nodes / depth | 32,768 / 12 | `decode::MAX_NODES`, `decode.rs` |
| Keys per object / key length | 32 / 64 characters | `decode.rs` |
| Fields / groups / sources | 256 / 32 / 32 | schema, `decode::limits` |
| Options per enum / per definition | 128 / 2,048 | schema, `admission::fields` |
| Distinct referenced regions | 1,024 | `admission::mappings`, `formula_admission::check` |
| Item properties per list / rows per list | 32 / 256 | schema, `formula::MAX_LIST_ITEMS` |
| Title, labels / help / reasons | 100 / 2,000 / 5,000 characters | schema |
| Text / multiline values | 8,000 / 20,000 characters | `formula_admission::TEXT_CHARS`, `MULTILINE_CHARS` |
| Diagnostics | 32, 256 characters each | `admission.rs`, `definitions::safe_text` |
| Generated value schema | 256 KiB | `campaigns/values.rs` |
| Publication form budget | 32,768 render units, 65,536 form pairs, 32,768 nodes | `campaigns::values::FORM_LIMITS` |
| Wiring snapshot | 1 MiB; 32 sources, 1,024 pages, 1,024 units, 256 unresolved fields | `publication.rs` |

Character counts are Unicode scalar values.

## Values

A sheet pins one published version. Values are canonical JSON: every declared key present and
no unknown key; `null` means unknown (every integer, otherwise only nullable fields); text is
kept exactly (`""` is not null); booleans are JSON booleans; integers use the token grammar
`-?(0|[1-9][0-9]*)` and are range-checked without floats; enums store the option ID. Each pin
compiles a host-generated value schema from its typed fields (`campaigns/values.rs`). A sheet
stays on its pin; definition updates never reinterpret saved values. Saves and transport:
[assisted-entry.md](assisted-entry.md); revisions and history: [campaigns.md](campaigns.md).

## Ruleset registry

The registry identifies games and editions. Registration identifies materials only: it does
not approve evidence, enable rules or make a sheet available.

- **Games and editions.** A game is `(game_id, label)`; an edition is `(game_id, edition_id,
  label)`. IDs are slugs (`registry::valid_id`); labels are 1–100 characters without
  surrounding whitespace. Edition IDs and labels are unique within a game. Rows are
  insert-only: SQL triggers forbid update and delete (`migrations/005_registry.sql`).
  Registering exactly the same identity again succeeds and changes nothing; a different label
  for an existing ID, or an existing label for a different ID, fails.
- **Native pair.** `mork-borg / it-2025` (label `MÖRK BORG — IT 2025`) is seeded with its exact
  legacy alias and marked native. Compiled adapters activate only on the canonical pair
  (`games::adapter_for_ruleset`), never on labels. Definitions cannot target a native pair,
  and the native edition accepts no further aliases.
- **Legacy aliases.** `(game_id, exact old edition label) → edition_id`, same game only,
  insert-only. Matching is exact, without case folding or filename inference. An alias may not
  point a registered label of another edition elsewhere. Creating an alias backfills the
  unassociated characters and sources with that exact stored label in the same transaction;
  existing associations are never overwritten.
- **Associations.** `character_rulesets` and `source_rulesets` hold the pair per record; a
  missing row means unresolved. Unresolved legacy records stay readable and are never
  reassigned automatically.
- **History.** `registry_events` (`migrations/016_registry_administration.sql`) is
  append-only and gets one row per newly created edition or alias; exact repeats write
  nothing, and registrations made before schema 16 have no events. Browser writes also append
  a `registry_web_events` row (Owner account ID, epoch, session digest) checked by trigger
  against the authenticated request and the latest event; an audit failure rolls back the
  registration. CLI events carry no account attribution.

### Registry CLI

```sh
mimik register-ruleset --game-id my-game --game-label "My game" \
  --edition-id first --edition-label "First edition"
mimik register-ruleset-alias --game-id my-game --edition-id first --legacy-edition "Old exact label"
mimik list-rulesets
```

`list-rulesets` prints every edition as JSON ordered by `(game_id, edition_id)`, with labels,
`native`, `native_adapter` and `generic_sheet_available` (always `false`; see limitations).
All subcommands accept the global `--database PATH` (default `data/mimik.sqlite3`, or
`MIMIK_DATABASE`). The CLI trusts local OS and database access.

### Owner browser

In authenticated mode the Owner can also use **Owner administration → Rulesets**
(`/admin/rulesets`, `src/auth/rulesets.rs`), which calls the same `register_in` and `alias_in`
transitions: 50 editions per page with an exclusive composite cursor (`after_game` and
`after_edition`, both or neither), `POST /admin/rulesets` and `POST /admin/rulesets/aliases`.
Only the Owner is admitted. Forms use strict Origin, durable CSRF rechecked inside the
transaction, the 8 KiB body limit and closed field sets; rejected input returns 422 with the
escaped values kept. Browser work can return 503 when the auth workers are busy; CLI and
browser writes contend only through SQLite (5 s busy timeout).

### Prepared-source identity (bundle version 2)

A version-2 prepared bundle must carry `system_id` (the registered game ID), `edition_id` (a
registered edition of that game) and `edition` (its registered label, or an exact alias of
the same pair). The pair must be registered before import; missing, null, malformed or
contradictory IDs fail, and an alias never overrides explicit IDs (`registry::source_identity`,
`sources/import.rs`). Version 1 rejects any `edition_id` member; v1 imports for the built-in
legacy systems may remain unresolved, and other v1 systems need an exact alias. Import never
approves evidence. The rest of the bundle contract is in
[sources-and-retrieval.md](sources-and-retrieval.md).

## Draft and publication workflow

### 1. Drafts

```sh
mimik import-definition-draft --file proposal.json --expected-revision 0
mimik show-definition-draft --id 1
mimik list-definition-drafts --game-id my-game --edition-id first --after-id 0
mimik import-definition-draft --file corrected.json --expected-revision 1
```

A draft is identified by `(game_id, edition_id, definition_id)` and has a positive database
row ID (`--id`, distinct from the slug). Expected revision `0` creates only; a positive value
must equal the current revision. Each successful save, including a no-op, increments the
revision by one; stale or overflowing writes change nothing (`Conflict`). Identity is
immutable (`definition_drafts`, `migrations/006_definition_drafts.sql`). `show` re-admits the
stored bytes and checks hash and identity. `list` returns up to 100 summaries in ascending ID
order after an exclusive cursor. Drafts never enable sheets. CLI JSON integers are exact i64
tokens; consumers must not round them through JavaScript `Number`.

### 2. Finalized sources

Publication requires every selected source to be finalized (`source_finalizations`, schema 7):
new prepared imports verify and freeze the original PDF, extraction, manifest, page images,
units and ruleset association; reviews and corrections stay editable. A legacy source needs
an exact reimport with its original inputs. See
[sources-and-retrieval.md](sources-and-retrieval.md).

### 3. Compare, then approve

```sh
mimik compare-definition --id 1
mimik approve-definition --id 1 --comparison-hash HASH --actor "Local reviewer" \
  --rationale "Compared field mappings against original pages" --originals-compared
mimik show-definition-version --id VERSION_ID
```

`compare-definition` captures a wiring snapshot: definition hash, source identity and
artifact hashes, every referenced page (PDF page, printed label, review revision,
completeness, image hash) and unit (block, polygon, revision, state, hashes of raw and
effective text), and the unresolved field IDs. Formula and constraint evidence pages are
captured with the wiring. Output is redacted (hashes and metadata, no manual text) and lists
original page and region image URLs (`/sources/pages/{id}/image`,
`/sources/pages/{page}/units/{unit}/image`). The ticket (`comparison_hash`) binds the target
and its revision, the snapshot, the latest published version, its latest approval and
withdrawal state. Publication needs at least one mapped field.

`approve-definition` requires `--originals-compared`, an actor (1–100 characters) and a
rationale (1–5,000), both free local labels (`sources::review_identity`), not authenticated
identities. Inside an IMMEDIATE transaction it recomputes the ticket; any difference is a
conflict with no partial write. Content identical to the latest version reuses that version
and appends an approval; changed content allocates the next version number. It also refuses
definitions whose worst-case form exceeds the publication budget (`list-form-exceeds-limit`).
Model confidence or an API review cannot stand in for the human comparison.

Output: version ID and number, content hash, approval ID, `wiring_status`,
`unresolved_fields`, `unresolved_formulas` and `unresolved_constraints` (omitted when empty),
`deterministic_rules` (`formulas-advisory` with a formula profile, else `unsupported`) and
`advisory_tasks` (`unsupported`). Unresolved items do not block publication.

### 4. Status, reapproval, withdrawal

```sh
mimik compare-definition --id VERSION_ID --published
mimik approve-definition --id VERSION_ID --published --comparison-hash NEW_HASH \
  --actor "Local reviewer" --rationale "Recompared after source review" --originals-compared
mimik withdraw-definition --id VERSION_ID --expected-approval APPROVAL_ID \
  --actor "Local reviewer" --rationale "Wiring needs revision"
```

`show-definition-version` reports `current`, `stale`, `withdrawn` or `never_approved`,
computed by re-taking the snapshot and comparing it with the stored one. Invalidation is
page-scoped: any review on a captured page advances its revision and makes the approval
stale, even for an unmapped unit; pages not captured do not matter. Restoring the old text
does not revive an approval. Read-time integrity failures are errors, not states.

Reapproval appends a new approval to an existing version. Withdrawal targets only the latest
approval, keeps history and never falls back to an older approval; an old withdrawal token
then conflicts.

### Effect on sheets

New sheet creation requires `current` wiring, all wired evidence present and every linked
catalogue sealed (`publication::private_readiness`, `campaigns/sheets.rs`), plus a campaign of
the exact same pair. Existing sheets keep their pin, stay readable and editable, show a
warning when wiring is not current, and keep evaluating formulas as advisory results.

### Storage integrity

`definition_versions`, `definition_approvals` and `definition_withdrawals`
(`migrations/008_definition_publication.sql`, guards in `009_review_integrity.sql`) are
insert-only. Triggers check approval targets, snapshot envelope identity, source artifact
membership, review revision advancement and page completion prerequisites; Rust re-verifies
canonical bytes, hashes and provenance on every read. Deleting an approval's draft by direct
SQL makes the approval an integrity error; there is no draft deletion. Older binaries refuse
a newer schema.
