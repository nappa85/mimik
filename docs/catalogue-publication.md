# Catalogue publication

Read-only game catalogues: spells, items and other entries that a sheet can cite. Plan:
[catalogue-plan.md](catalogue-plan.md). **Implemented so far: steps 1–2** (drafts, and the
comparison, publication, integrity check and name-index repair of sealed versions).
Definition linkage and the sheet routes are later steps; until they land, nothing published
here is visible to players or changes a sheet.

## Drafts

### Operator commands

Register the exact nonnative game/edition pair first (`runtime-registry.md`) and import every
cited source (`sources.md`). Then:

```sh
./target/release/mimik import-catalogue-draft --file spells.json --expected-revision 0
./target/release/mimik show-catalogue-draft --id 1
./target/release/mimik import-catalogue-draft --file spells.json --expected-revision 1
```

- Revision 0 creates only; an existing `(game, edition, catalogue)` draft conflicts. A matching
  revision updates and increments it once, including for identical content; a stale revision
  changes nothing.
- The row ID is a positive database ID, distinct from the catalogue id. Draft identity
  (`game_id`, `edition_id`, `catalogue_id`) is immutable.
- `show-catalogue-draft` re-decodes the stored draft and checks its canonical bytes, hash and
  identity. It does not re-check source references, which are checked at every save.
- Commands share the definition admission permit: one nonqueued job per process. A second job
  in the same process answers busy. A save takes the SQLite write lock (`BEGIN IMMEDIATE`)
  before its reference checks and revision lookup, so concurrent saves from other processes
  wait (5 s busy timeout); saves of different catalogues all succeed, and of two saves of one
  catalogue with the same expected revision one wins and the other answers a conflict.

### Redaction

Catalogue rows hold manual-derived names. **No command prints names, entry keys, property ids
or values, labels, the title, owner text or unit text** (plan D6).

- Success prints only `id`, `game_id`, `edition_id`, `catalogue_id`, `revision`,
  `content_hash`, `status` and `counts` (sources, entry types, entries, retired, owner-written,
  unit references, distinct units).
- Errors are static codes with index-only paths such as `/entries/12/description:
  unit-page-source-mismatch`. Schema paths are walked against the envelope structure: indices
  are kept only at array positions and member names only where that member exists; the first
  other segment and everything after it become `*`. Entry-property keys are always redacted,
  whatever their spelling.
- Sentinel tests (`tests/catalogues.rs`, `src/catalogues/admission_tests.rs`) place distinctive
  synthetic strings in every author-chosen member and check stdout and stderr on success and on
  failure paths: decode, schema, domain, source references, a non-finalized source, conflicts,
  reads of unknown and corrupt drafts, and oversize and symlink inputs.

### Envelope

Trusted host schema `schemas/catalogue-v1.json` (JSON Schema 2020-12, compiled offline),
then serde with `deny_unknown_fields`, then domain checks. `schemas/catalogue-example.json` is
an original synthetic example.

```json
{ "contract_version": 1, "game_id": "synthetic-game", "edition_id": "first",
  "catalogue_id": "spells", "title": "Spells",
  "sources": [{ "source_id": 3, "pdf_sha256": "…64 lowercase hex…" }],
  "entry_types": [{ "id": "spell", "label": "Spell", "properties": [
      { "id": "level", "label": "Level", "control": "integer" },
      { "id": "school", "label": "School", "control": "enum",
        "options": [{ "id": "light", "label": "Light" }] }] }],
  "entries": [
    { "key": "lantern-flare", "type": "spell", "name": "Lantern Flare",
      "properties": { "level": 1, "school": "light" },
      "description": { "units": [{ "source_id": 3, "page_id": 41, "unit_id": 977 }] } },
    { "key": "house-charm", "type": "spell", "name": "House Charm", "retired": false,
      "properties": { "level": 0, "school": "light" },
      "description": { "owner_text": "…" } }] }
```

- Every object is closed (`additionalProperties: false`), except an entry's `properties`, which
  is a map keyed by the type's property ids; its values are closed to string, integer and
  boolean, and the domain check requires exactly the declared key set.
- IDs, keys and option ids follow the registry slug grammar: a leading lowercase letter, then
  lowercase letters, digits or `-`, 1–64 bytes.
- Each entry's `type` must be declared; its properties must be exactly that type's properties,
  each matching its control (`text` string, `integer`, `boolean`, `enum` one of the option
  ids). Options are allowed only on `enum`, which needs 1–128 unique options.
- A description is either 1–16 unit references or `owner_text` (non-blank). An entry may not
  cite the same unit twice. Every cited source must be listed in `sources`.
- Labels, names, the title and owner text may not be blank. Text is not normalised.
- Names, labels, the title and text property values may not contain control characters
  (`char::is_control`, C0 and C1), invisible format characters (Unicode category Cf: bidi
  overrides and isolates, zero-width characters, the byte-order mark, soft hyphen, tags) or
  line and paragraph separators (Zl, Zp). A name made only of such characters is therefore
  refused too. Codes: `control-character-in-text`, `format-or-separator-character-in-text`.
- Owner text allows line feed and tab as its only control characters and refuses Zl and Zp.
  **Normalise owner text to LF line endings before import**: CR (and so CRLF) is refused.
  Format characters (Cf) stay allowed in owner text, which right-to-left prose needs.
- `retired` absent and `retired: false` are the same state.

### Source references (checked at every save, in the write transaction)

- The pair is registered, nonnative and has no built-in adapter (as for definition drafts).
- Every listed source exists with that PDF hash, belongs to exactly that pair, and is
  finalized.
- Every `(source_id, page_id, unit_id)` names a unit on that page of that source.
- A unit's review state is not checked: rejected and unreviewed units are allowed in drafts.
  Publication (step 2) refuses rejected units.

### Limits

| Resource | Limit |
|---|---|
| Raw input and canonical encoding | 4 MiB each (4,194,304 bytes) |
| Decoder nodes (catalogue profile) | 524,288 |
| Container depth | 8 levels |
| Members per object / key length | 32 / 64 scalars |
| Sources | 32 |
| Entry types | 1–32 |
| Properties per type | 16 |
| Options per enum | 128 |
| Entries | 8,192 |
| Units per entry | 1–16 |
| Distinct units per catalogue | 16,384 |
| Names, labels, title | 100 scalars |
| Text property value | 1,000 scalars |
| `owner_text` | 20,000 scalars per entry, 1 MiB (UTF-8 bytes) in total |

The catalogue profile is separate from the definition decoder: `definitions::decode::MAX_NODES`
(32,768), its 256 KiB input cap and the list form budget are unchanged, and a regression test
asserts it. Duplicate keys, including escaped-equivalent spellings, fractions, exponents,
integers outside i64 and trailing JSON are rejected.

Every canonical spelling is no longer than any accepted input spelling (no member is added by
canonicalisation), so the canonical cap is a defensive check that admitted input cannot reach.

### Canonical identity

Hash: SHA-256 over `b"mimik-catalogue-v1\0"` plus the canonical bytes, lowercase hex.

- Members are written in a fixed order (the schema's order above). Optional members are
  omitted when absent; `retired: false` is omitted.
- Sources are sorted by id. Entry types, properties, options and entries keep the author's
  order. Each entry's `properties` are written in its type's declared order.
- **Unit order is the author's order and is part of the identity; it is never sorted.**
- A permanent frozen hash vector guards the encoding
  (`canonical_encoding_has_a_frozen_independent_hash_vector`).

### Storage

Schema 19 (`migrations/019_catalogues.sql`) adds the STRICT table `catalogue_drafts`:
positive autoincrement ids and revisions, a registry foreign key, NUL-free slug checks, a JSON
object of at most 4 MiB whose `contract_version` and identity match the columns, an exact
64-byte lowercase-hex hash, a unique identity and the trigger
`catalogue_draft_identity_immutable`. The publication tables below are in the same migration,
which is additive and atomic: a failure anywhere leaves schema 18 with no catalogue object.

## Publication

### Operator commands

```sh
./target/release/mimik compare-catalogue --id 1
./target/release/mimik publish-catalogue --id 1 --comparison-hash <ticket> \
    --actor "Owner" --rationale "First spell list"
./target/release/mimik show-catalogue-version --game synthetic-game --edition first \
    --catalogue spells [--version 2]
./target/release/mimik verify-catalogues
./target/release/mimik rebuild-catalogue-index
```

Commands share the definition admission permit (one nonqueued job per process).

### Comparison and the ticket

`compare-catalogue` re-admits the stored draft, re-checks its source references and compares
it with the latest sealed version of the same `(game, edition, catalogue)`, in one read
transaction. It prints counts only:

- `counts`: the draft's counts (as `show-catalogue-draft`);
- `units`: distinct referenced units by review state (unreviewed, accepted, corrected,
  rejected);
- `badges`: entries by the badge they would show now (owner-written; reviewed: every unit
  accepted or corrected and every cited page complete; unavailable: any unit rejected;
  otherwise unchecked);
- `changes` against the latest version: `identical`, added entries and entry types, newly
  retired and un-retired entries, entries whose unit references changed, entries whose owner
  text changed;
- `refusals`: dropped entry keys, retyped entry keys, dropped entry types, rejected units;
- `publishable` (no refusal) and `comparison_hash`, the ticket.

The ticket is SHA-256 over `b"mimik-catalogue-comparison-v1\0"` and the canonical JSON of:
the draft id, revision and content hash; the latest sealed version number and content hash, or
`null`; and, for every distinct referenced unit in id order, `(unit_id, unit revision, state,
page_id, page revision)`. A unit review, a page completion or a printed-label change advances a
bound revision, so any of them, a new draft revision or another publication makes the ticket
stale.

### Publishing

`publish-catalogue` validates the actor (1–100 characters) and rationale (1–5,000), like
definition approvals, and the ticket format (64 lowercase hex). Then, in **one `BEGIN
IMMEDIATE` transaction**:

1. the draft row must still have the admitted revision and hash (else a conflict);
2. the comparison is recomputed; a different ticket answers `StaleComparison` and publishes
   nothing;
3. any refusal answers its static codes (`dropped-entry-keys`, `retyped-entry-keys`,
   `dropped-entry-types`, `rejected-units`) and publishes nothing;
4. content identical to the latest version publishes nothing and reports that version with
   `"published": false`;
5. otherwise the canonical content is re-hashed, and the catalogue identity row (first time
   only), the version (number = latest + 1, with draft id and revision, ticket, actor and
   rationale), the projections, the name-index rows and finally the seal are inserted. Every
   insert must affect exactly one row.

Any error rolls everything back: no partial or unsealed version remains (tests cover an
injected failure before each stage, a failing seal and a process killed mid-publication).
A concurrent publication of the same catalogue from another process waits for the write lock
(5 s busy timeout) and then answers `StaleComparison`, since the latest version changed.

Append-only (plan D3): a version may add entries and entry types, change names, properties,
descriptions and owner text, and retire or un-retire entries. It may not drop an entry key or
an entry type (even one with no entries), or change an entry's type. Retirement does not free
capacity: a catalogue at 8,192 keys cannot take another entry.

Output (publish and show): `version_id`, `game_id`, `edition_id`, `catalogue_id`, `version`,
`latest_version`, `content_hash`, `draft_id`, `draft_revision`, `created_at`,
`status: "sealed"`, `counts`, and for publish `published`. Actor and rationale are stored
but never printed.

### Sealed versions and their projections

| Table | Holds | Keys |
|---|---|---|
| `catalogues` | integer surrogate per `(game, edition, catalogue)`, below 2^43 | id; identity |
| `catalogue_versions` | content, hash, identity and title columns checked against the content, draft provenance, ticket, actor, rationale | id; `(catalogue, version)` |
| `catalogue_entry_types` | type id, ordinal, label, property declarations (JSON) | `(version, type)`; `(version, ordinal)` |
| `catalogue_entries` | key, ordinal, type, name, retired, `desc_kind`, owner text, properties (JSON) | `(version, key)`; `(version, ordinal)` |
| `catalogue_entry_units` | key, ordinal (author order, 0-based), unit id (foreign key), page and source ids | `(version, key, ordinal)` |
| `catalogue_seals` | the sealed versions | version |

Indexes: `catalogue_entries(version_id, type_id, name, key)` and
`catalogue_entry_units(unit_id)`. Readers use the projections only, never the content, and
choose "latest" through `catalogue_seals`.

Triggers (all named, each with a regression test in `tests/catalogue_publication.rs`):

- **UPDATE and DELETE** are refused on all six tables.
- **Replacement guards** refuse an insert that collides with any primary or unique key, so
  `INSERT OR REPLACE` cannot delete a published row. Each key is probed by its own index.
- **Sealed versions** refuse new types, entries and units.
- **Version order and provenance**: version numbers are contiguous, a new version starts only
  when every earlier one is sealed, and its content and hash equal the named draft revision's.
- **Unit rows** must name a unit on that page, a page in that source, and a finalized source
  of the catalogue's pair.
- **The seal** requires its version to be the latest sealed + 1 with no unsealed lower version;
  then exact two-way equality (two `EXCEPT` queries each way over `json_each` of the content,
  plus equal row counts) between the content and each projection, covering every column above;
  then the append-only rule against the previous sealed version.

### Integrity check and the name index

`catalogue_names_fts` (FTS5, `unicode61 remove_diacritics 2`, prefix 3) is a derived,
latest-only index of entry names with `catalogue_row_id` and `key` unindexed; the rowid is
`catalogue surrogate × 2^20 + entry ordinal`. Publication replaces that catalogue's rows by
rowid range. Queries join back on `(version, key)` of the latest sealed version of that
catalogue, so keys that collide across catalogues cannot mix. It has no immutability triggers.

`verify-catalogues` is the Owner integrity check. For every version, in its own short read
transaction: the version is sealed, its content is within bounds, re-admits to the same
canonical bytes and hash, matches its identity and title columns, and its projections equal
those built from the content in Rust (an implementation independent of the seal SQL). Then the
index: its definition equals the migration's, its rows equal the latest projections in
identity, name and multiplicity, and FTS5 `integrity-check` passes. It prints counts and a
list of problems (`catalogue` and `version` numbers with a static code; `catalogue: 0` means
the whole index) and exits non-zero when any problem exists.

`show-catalogue-version` re-verifies the shown version the same way (R5). Requests (later
steps) check only that a version is sealed and its identity columns match.

`rebuild-catalogue-index` drops and recreates the index with the migration's definition and
fills it from the latest sealed projections, in one transaction.

### Redaction

As for drafts: no command prints names, keys, labels, the title, owner text, unit text, the
actor or the rationale. `tests/catalogue_publication.rs` places sentinels in all of them and
checks stdout and stderr of every command on success and on failure (stale ticket, refusals,
malformed ticket, actor or rationale, unknown draft or version, malformed ids, tampered
versions and index).

### Measured cost (release, real CLI, 8,192 entries)

See plan §11: publication peaks at about 125 MiB RSS; a separate process's write transaction
waits about 1.5 s and then succeeds.
