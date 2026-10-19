# Catalogue publication

Read-only game catalogues: spells, items and other entries that a sheet can cite. Plan:
[catalogue-plan.md](catalogue-plan.md). **Implemented so far: steps 1–3** (drafts; the
comparison, publication, integrity check and name-index repair of sealed versions; and the
definition link with the read-only routes reached through a sheet, under
[Reading through a sheet](#reading-through-a-sheet)). Picking entries into sheet rows
(`catalogue_ref`) is step 4; until it lands, catalogues are read, never stored in a sheet.

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
  changes nothing. The trigger `catalogue_draft_revision_advances` refuses any draft update
  that does not advance the revision by exactly one, so draft content cannot be rewritten
  under an unchanged revision even from a raw SQLite shell (published versions keep their own
  content copy, so their integrity never depended on it).
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
- The guarantee covers stored catalogue content. Argument-parse errors (a non-numeric `--id`,
  an unknown flag) are clap's own diagnostics before any catalogue code runs; they echo the
  operator's own command-line input, which is not stored content, and are out of scope.
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
- Labels, names, the title and owner text may not be blank: at least one scalar must be
  neither whitespace, blank-looking (the Braille pattern blank U+2800 and the Hangul fillers
  U+115F, U+1160, U+3164, U+FFA0, which render as nothing), an invisible format character
  (Unicode category Cf) nor a nonspacing mark (Mn: combining marks such as U+0301 and U+034F,
  variation selectors such as U+FE0F, and U+17B4/U+17B5, which alone render as nothing or as
  a dotted circle). Mixed with visible text they stay allowed (Braille text needs U+2800;
  owner text keeps Cf for bidi prose; marks combine with a base). Text is not normalised.
- A text property value may be empty (the explicit "no value"); a non-empty one follows the
  same rule (`blank-text-property-value`), so `"   "` or a lone U+2800 is refused.
- Names, labels, the title and text property values may not contain control characters
  (`char::is_control`, C0 and C1), invisible format characters (Unicode category Cf: bidi
  overrides and isolates, zero-width characters, the byte-order mark, soft hyphen, tags) or
  line and paragraph separators (Zl, Zp). A name made only of such characters is therefore
  refused too. Codes: `control-character-in-text`, `format-or-separator-character-in-text`.
  Refusing Cf also refuses the joiners ZWNJ/ZWJ (U+200C/U+200D) and the soft hyphen, which
  some scripts and hyphenation use in names: accepted, since short labels can be written
  without them. Categories come from `unicode-general-category` 1.1.0 (Unicode 16.0); a
  character assigned Cf in a later Unicode version is not known to be Cf and passes.
- Owner text allows line feed and tab as its only control characters and refuses Zl and Zp.
  **Normalise owner text to LF line endings before import**: CR (and so CRLF) is refused.
  Format characters (Cf) stay allowed in owner text, which right-to-left prose needs. Owner
  text can therefore carry bidi overrides; plan step 3 must render it in an isolated bidi
  context (`<bdi>` or `unicode-bidi: isolate`, with `white-space: pre-wrap`) so it cannot
  reorder the surrounding names, citations and controls, with a browser test.
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
| Entries | 6,144 |
| Units per entry | 1–16 |
| Unit references per catalogue (with repetition) | 6,144 (`unit-reference-limit`) |
| Distinct units per catalogue | 16,384 (unreachable while references are capped at 6,144) |
| Names, labels, title | 100 scalars |
| Text property value | 1,000 scalars |
| `owner_text` | 20,000 scalars per entry, 1 MiB (UTF-8 bytes) in total |

The entry and reference caps bound publication's write-lock hold together with the byte cap
(the seal compares every reference row; the name index re-tokenizes every name). The worst
measured case on the profiling host, 6,144 entries with 99-scalar names of 3-character
tokens updating the previous version at 4 MiB with every reference used, held the lock for
at most 1.81 s (conservative upper bound, plan §14). At 8,192 entries a shape with 64-scalar
keys exceeded the plan's 2 s budget with 6,144 references, the 3-character-token shape still
exceeded it in one of three runs with 4,096, and 6,144 entries with 8,192 references reached
1.99 s. This is host-specific
evidence, not a bound proven for every admitted shape.

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
   `"published": false`. **Exception:** refusals are checked first, so identical content
   citing a unit that was rejected after publication answers `rejected-units` instead (the
   published version is unchanged; its badge shows the entry as unavailable);
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
capacity: a catalogue at 6,144 keys cannot take another entry.

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

Each version also records its `admission_profile` (currently `catalogue-admission-1`, the only
value the column's CHECK admits). Verification re-admits stored content under the profile it
was published with. Tightening any admission rule means a new profile id with its own
validator and a migration widening the CHECK; old profiles keep validating old versions, so a
stricter rule never turns an existing version into an integrity failure. A profile this build
does not know is reported as `unknown-admission-profile`.

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
transaction: the version is sealed (`version-not-sealed`), its content is within bounds,
re-admits under its recorded admission profile to the same canonical bytes and hash, matches
its identity and title columns (`version-identity-mismatch`), and its projections equal those
built from the content in Rust (`projection-mismatch`; an implementation independent of the
seal SQL). Every projection row of the version is read table by table without joins, so a
row the content does not account for, such as a unit row whose entry does not exist, is a
mismatch. Then the structure across versions: version numbers run 1, 2, … per catalogue
(`version-sequence-gap` at the first break), every catalogue identity has a version
(`catalogue-without-versions`), and `PRAGMA foreign_key_check` finds nothing in any catalogue
table (`foreign-key-violation`; one statement per table). Then the index, in an `IMMEDIATE`
transaction that is rolled back: its definition first (`name-index-missing`,
`name-index-definition-mismatch`; the row checks are then skipped and `index_rows` is 0), then
its rows against the latest projections in identity, name and multiplicity
(`name-index-mismatch`), then FTS5 `integrity-check` (`name-index-integrity-check-failed`).
Busy, locked, I/O, out-of-memory, interrupted and disk-full conditions there are storage
errors and fail the command; any other SQLite failure is reported as a damaged index. It
prints counts and a list of problems (`catalogue` and `version` numbers with a static code;
`catalogue: 0` means the whole index or the whole schema) and exits non-zero when any problem
exists. Cost grows with stored history (all versions, and the foreign-key scan of every
catalogue table); at 13 versions (6 catalogues) of up to 6,144 entries it took about 8.4 s.

**Operator note.** The application only ever writes a version and its seal in one
transaction. A version inserted by hand without a seal (outside the application) blocks that
catalogue: the order trigger refuses any later version while an unsealed one exists. Verify
reports it as `version-not-sealed`; recovery is restoring the database from a backup.

`show-catalogue-version` re-verifies the shown version the same way (R5). Requests (later
steps) check only that a version is sealed and its identity columns match.

`rebuild-catalogue-index` drops and recreates the index with the migration's definition and
fills it from the latest sealed projections, in one transaction.

### Redaction

As for drafts: no command prints names, keys, labels, the title, owner text, unit text, the
actor or the rationale (argument-parse errors, which echo the operator's own input, are out
of scope as described under Drafts). `tests/catalogue_publication.rs` places sentinels in all of them and
checks stdout and stderr of every command on success and on failure (stale ticket, refusals,
malformed ticket, actor or rationale, unknown draft or version, malformed ids, tampered
versions and index).

### Measured cost (release, real CLI, 6,144 entries)

Plan §14 (supersedes §11-§13). The write lock is bounded by a non-blocking probe started
before the publish process: the hold lies between the last free probe that still read the
old version count and the first that read the new one, a conservative upper bound whatever
else ran in between. On the profiling host publication holds it for at most about 0.76 s on
the calibrated shape (2.2 MB, 5,376 references) and at most 1.81 s in the worst measured case
(4 MiB, 6,144 references, 99-scalar names of 3-character tokens, update); the
declaration-heavy shape (32 types x 16 enum properties x 128 options) held it for at most
1.12 s. Peak RSS at most about 173 MiB (publish of the declaration-heavy shape). These are the
worst measured cases, not a proof that no admitted shape costs more. The profiler's R8 gate
(`--max-hold-ms`, finite and positive, 2,000 by default) fails when no publication was gated,
a hold's upper bound exceeds the budget or cannot be bounded, the contender was requested and
did not wait through every publish and succeed, or the run is not complete at the caps (the
Rust entry and reference caps, every shape, the calibrated cases) unless `--allow-partial` is
given for cap selection; a partial run is never R8 evidence. A `--no-contender` run gates the
hold only (`scope: "hold only"`): R8 needs a passing run of each kind. A concurrent writer
using the server's pattern (`BEGIN IMMEDIATE`, 5 s busy timeout, one insert, commit: the
lock-level equivalent of a sheet save) waits through the publish and succeeds in every case.
The 5 s busy timeout (`src/store.rs`) is the real failure point: a publish holding the lock
longer than that would make concurrent sheet saves fail with a storage error. Commit cost includes `fsync`, so it depends on the disk.

## Reading through a sheet

Plan §3 step 3 (D4, D8, D11, R5–R7, R10, R12, R13, R20, R21, R27).

### Linking a catalogue to a definition

A definition links catalogues by id with its optional `catalogues` member (at most 8,
`[{"id": "spells"}]`; see [runtime-definition-contract.md](runtime-definition-contract.md#linked-catalogues)).
The link names no version and always means the catalogue of the definition's own game and
edition. **Publish the catalogue first:** a definition draft that links a catalogue without a
sealed version is refused at save (`/catalogues/N linked-catalogue-not-published`), again in
the definition comparison and its approval transaction, and the readiness check reports the
definition's wiring as stale if the catalogue ever lacks a sealed version (only damage can
cause that: sealed versions are append-only). The check is existence only; catalogue
versions never enter the definition's wiring snapshot or its hash, so publishing a new
catalogue version never makes a definition approval stale.

### Routes

- `GET /private-sheets/{id}/catalogue/{catalogue}` browses: 50 entries per page, ordered by
  `(name, key)`, with an optional entry type (`type`), name search (`q`) and an exclusive
  cursor (`after=<key>`; an unknown key restarts from the beginning).
- `GET /private-sheets/{id}/catalogue/{catalogue}/{key}` shows one entry.

The sheet page lists its definition's catalogues with links that open in a new tab, so an
unsaved draft stays in the sheet's tab. Both routes are reached only through a sheet the
reader may read (its custodian, including a departed custodian of a frozen sheet, or an
active Master of its campaign; never the production Owner) and the sheet's immutable runtime
pin must link the catalogue. Native and unsupported pins, unlinked, unknown or other games'
catalogues, unknown keys and unknown entry types all answer exactly like an unreadable sheet
(403), so the routes cannot probe what exists; malformed ids, cursors, unknown query
parameters and queries over 100 scalars answer 400 before any lookup.

Each request holds the single runtime permit through every phase: a first read transaction
(session recheck, read authorization, the snapshot and pin), validation outside the
database lock (snapshot decode, write-only pin validation, pin match, the link), a second
read transaction (session recheck and authorization again, the sheet must still correspond
to what was validated, the pin row unchanged, then the bounded catalogue lookups), and the
render. The routes are not rate-limited; the single permit bounds them, as it bounds sheet
reads, and a busy server answers 503. Pages load no script and no images (players see
citations only, plan Q1).

Readers use the latest sealed version, chosen through `catalogue_seals`, and only its
projections; a request checks that the version is sealed and that its identity columns
match. Content hashes and projection equality are checked at publication, by
`show-catalogue-version` and by `verify-catalogues` (R5).

### What an entry page shows

Names, descriptions, badges and citations are **current reference aids**: they always come
from the latest catalogue version and today's source review, and the page says so ("Current
text"). Nothing is materialised.

| Badge | When |
|---|---|
| Owner-written | The entry has owner text (a house rule or homebrew entry) |
| Reviewed extraction | Every cited unit is accepted or corrected and every cited page is complete. The page adds: "Source text reviewed. Which passages form this entry was chosen by the catalogue author." |
| Unchecked source text | Any cited unit is unreviewed or any cited page is incomplete: "Extracted from your manual; not yet reviewed." |
| Description unavailable | Any cited unit is rejected now. No passage of the entry is shown, even one before the cut-off; the citation stays |

Unit text is the correction for corrected units, otherwise the raw text. The metadata of every
cited unit is read first; the texts follow in author order, cut at **20,000 characters per
entry** (character-safe; at most 80 KB of UTF-8). A cut entry ends with "Continues in the
manual at p. X (PDF page N)", naming the page of the first unit not fully shown. A unit whose
text is not text, contains NUL or exceeds its bound shows "Description unavailable (source
text malformed)" instead of any text. Owner text (at most 20,000 characters) is never cut.
Each citation is the source title, the current printed page label and the PDF page number,
read live.

All catalogue and manual text is escaped, kept with its line breaks (`white-space:
pre-wrap`) and wrapped in `<bdi>`, so right-to-left text and bidirectional controls in owner
or unit text (owner text keeps Unicode Cf for right-to-left prose) cannot reorder the
surrounding names, citations or controls. A Chromium check covers an unterminated
right-to-left override in owner text (`tools/catalogue_browser.py`).

### Search

Name search uses the name index (`unicode61 remove_diacritics 2`, 3-character prefixes)
through the existing query builder: raw text never reaches `MATCH`; every word or quoted
phrase becomes a quoted string joined with a generated `AND` (operators, column filters and
`NEAR` are plain words), at most 16 pieces and 100 scalars. Results are filtered, never
ranked: the order stays `(name, key)`. The index is joined back on the latest sealed version
of the same catalogue and restricted to its rowid range, so keys shared by two catalogues
cannot leak. Every browse statement runs under the search time budget (1 s); a query that
exceeds it, has no letters or digits, contains control characters or has more than 16 pieces
re-renders the search form with a static message (422).

Retired entries stay listed with a "Retired" badge and their entry pages stay readable: they
are valid on existing rows and only stop being offered for new picks (step 4).
