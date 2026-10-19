# Catalogues

Read-only game catalogues (spells, items, other entries) that players browse and read through
a sheet. The Owner publishes them from the CLI. Sheets do not store catalogue references yet
(`CAT-L9` in [limitations.md](limitations.md)). Decisions: "Catalogues" in
[decisions.md](decisions.md).

Code: `src/catalogues/` (`decode`, `admission`, `store`, `publication`, `read`),
`migrations/019_catalogues.sql`, `schemas/catalogue-v1.json` (example
`schemas/catalogue-example.json`), the definition link in `src/definitions/`, routes in
`src/campaigns/catalogue.rs` and `catalogue_web.rs`, templates `catalogue_*.html`.

## Envelope

```json
{ "contract_version": 1, "game_id": "synthetic-game", "edition_id": "first",
  "catalogue_id": "spells", "title": "Spells",
  "sources": [{ "source_id": 3, "pdf_sha256": "…64 lowercase hex…" }],
  "entry_types": [{ "id": "spell", "label": "Spell", "properties": [
      { "id": "school", "label": "School", "control": "enum",
        "options": [{ "id": "light", "label": "Light" }] }] }],
  "entries": [
    { "key": "lantern-flare", "type": "spell", "name": "Lantern Flare",
      "properties": { "school": "light" },
      "description": { "units": [{ "source_id": 3, "page_id": 41, "unit_id": 977 }] } },
    { "key": "house-charm", "type": "spell", "name": "House Charm", "retired": true,
      "properties": { "school": "light" }, "description": { "owner_text": "…" } }] }
```

Admission (`admission::validate`): the catalogue's own decode profile
(`decode::parse_catalogue`, separate from the definition decoder), the trusted schema (JSON
Schema 2020-12, offline), serde `deny_unknown_fields`, domain checks, canonical encoding.

- Objects are closed, except an entry's `properties`: exactly its type's declared ids, each
  matching its control (`text`, `integer`, `boolean`, `enum` with 1–128 unique options).
- IDs, keys and option ids are registry slugs (`registry::valid_id`), unique in their scope;
  an entry's `type` must be declared.
- A description is 1–16 unit references (no unit twice, every source listed in `sources`) or
  non-blank `owner_text`.
- Names, labels, the title and non-empty text values need a visible scalar (not whitespace,
  U+2800, U+115F, U+1160, U+3164, U+FFA0, Cf or Mn) and refuse controls, Cf, Zl and Zp; `""`
  is "no value". Owner text allows only LF and tab as controls (CR is refused: normalise to
  LF), refuses Zl/Zp and keeps Cf for right-to-left prose. Text is not normalised.
- Duplicate keys (escaped spellings too), fractions, exponents, integers outside i64 and
  trailing JSON are refused. `retired` absent equals `false`.

Identity: SHA-256 over `b"mimik-catalogue-v1\0"` plus the canonical bytes (schema member
order, optional members and `retired: false` omitted, sources sorted by id, everything else in
author order, **unit order included**; properties in declared order). A frozen hash vector
guards it.

## Limits

Constants in `src/catalogues/decode.rs` and `admission.rs`. `tools/test_catalogue_profile.py`
fails when the profiler's copies differ; `migration_ordinal_bounds_follow_the_caps` ties the
migration's ordinal CHECKs to them.

| Resource | Limit |
|---|---|
| Raw input and canonical encoding | 4 MiB each |
| Decoder nodes / depth / members per object / key length | 524,288 / 8 / 32 / 64 scalars |
| Sources / entry types / properties per type / options | 32 / 1–32 / 16 / 128 |
| Entries (`MAX_ENTRIES`) | 6,144 |
| Unit references per entry / per catalogue with repetition (`MAX_UNIT_REFERENCES`) | 1–16 / 6,144 |
| Distinct units per catalogue | 16,384 (unreachable below the reference cap) |
| Names, labels, title / text property value | 100 / 1,000 scalars |
| Owner text | 20,000 scalars per entry, 1 MiB (UTF-8) in total |
| Linked catalogues per definition | 8 |

Retirement does not free capacity. The entry, reference and byte caps keep a publication's
write-lock hold under 2 s on the profiling host (see "Lock budget").

## Commands

All print metadata only and share the definition admission permit (one nonqueued job per
process; a second answers busy).

- `import-catalogue-draft --file F --expected-revision N`: admits a bounded regular file and
  saves it in one `BEGIN IMMEDIATE` transaction. Revision 0 creates only; a matching revision
  advances by exactly one (trigger `catalogue_draft_revision_advances`); else conflict.
  References are checked at every save: a registered nonnative pair without a built-in
  adapter; each source of exactly that pair, with that PDF hash, finalized; each unit on its
  page in its source. Review states are not checked in drafts.
- `show-catalogue-draft --id D`: re-admits the draft and checks bytes, hash and identity.
- `compare-catalogue --id D`: in one read transaction, counts only: draft counts, units by
  review state, entries by would-be badge, changes against the latest sealed version,
  refusals (dropped keys, retyped keys, dropped types, rejected units), `publishable` and the
  ticket `comparison_hash` (SHA-256 over the draft id, revision and hash, the latest sealed
  version and hash, and every cited unit's `(id, revision, state, page id, page revision)`).
- `publish-catalogue --id D --comparison-hash H --actor A --rationale R` (actor 1–100,
  rationale 1–5,000 characters), in one `BEGIN IMMEDIATE` transaction: unchanged draft; the
  recomputed ticket must match (`StaleComparison`); refusals answer static codes; identical
  content publishes nothing (`"published": false`; refusals are checked first); otherwise
  identity row, version, projections, name-index rows and seal, each insert exactly one row.
  Any error rolls everything back.
- `show-catalogue-version --game G --edition E --catalogue C [--version V]`: re-verifies and
  shows a sealed version.
- `verify-catalogues`: the Owner integrity check; exits non-zero on any problem.
- `rebuild-catalogue-index`: recreates the name index from the latest sealed projections.

**Append-only:** a version may add entries and types, change names, properties, descriptions
and owner text, and retire or un-retire; it may not drop a key or type (even an empty one) or
change an entry's type. No withdrawal: retire, replace owner text, or correct or reject the
cited unit.

**Redaction:** no command prints names, keys, property ids or values, labels, the title,
owner text, unit text, actor or rationale. Errors are static codes with index-only paths
(`/entries/12/description: unit-page-source-mismatch`); schema paths stop at the first
author-chosen segment (`*`). Sentinel tests: `tests/catalogues.rs`, `tests/catalogue_publication.rs`.

## Storage, seal and verification

Migration 019: `catalogue_drafts`; `catalogues` (integer surrogate < 2^43);
`catalogue_versions` (content, hash, identity and title checked against content, draft
provenance, ticket, actor, rationale, `admission_profile`); projections
`catalogue_entry_types`, `catalogue_entries`, `catalogue_entry_units` (only `unit_id` has a
foreign key); `catalogue_seals`. Readers use projections only and pick "latest" through
`catalogue_seals`.

Triggers refuse UPDATE/DELETE on published tables, any insert colliding with a primary or
unique key (`INSERT OR REPLACE`), inserts into sealed versions, out-of-order versions or a
content differing from the named draft revision, and unit rows not on that page, source and
finalized pair. The seal requires latest sealed + 1, exact two-way equality (`EXCEPT` both
ways over `json_each`, plus counts) of content and every projection column, then the
append-only rule.

`admission_profile` is `catalogue-admission-1` (the only CHECK value). Content is re-admitted
under its recorded profile; any admission change needs a new profile id.

`verify-catalogues`: per version (own short read transaction) sealed, bounded, re-admits to
the same bytes and hash, identity columns, projections equal to those built in Rust (table by
table, so orphan rows count); then version contiguity, no catalogue without versions,
`foreign_key_check`; then the name index (definition, rows against the latest projections,
FTS5 `integrity-check`). Problems are `(catalogue, version, code)`.
A hand-inserted unsealed version blocks its catalogue (`version-not-sealed`; restore a
backup).

Name index `catalogue_names_fts`: FTS5 `unicode61 remove_diacritics 2`, prefix 3, latest-only,
derived, no triggers; rowid = surrogate × 2^20 + ordinal; publication replaces a catalogue's
rowid range.

## Linking from a definition

The optional `catalogues` member lists 1–8 unique slugs (`[{"id": "spells"}]`, display
order; `[]` is refused, absent leaves hashes unchanged). A link names no version and means
the catalogue of the definition's own pair. `definitions::store::linked_catalogues` requires
a sealed version at three sites: draft save (`/catalogues/N linked-catalogue-not-published`),
the publication snapshot (comparison and approval) and readiness
(`private_evidence_available`; wiring becomes stale). Existence only: catalogue state never
enters the wiring snapshot, so a new catalogue version never stales an approval.

## Reading through a sheet

- `GET /private-sheets/{id}/catalogue/{catalogue}`: browse, 50 per page by `(name, key)`;
  optional `type` (empty = all), `q`, `after=<key>` (exclusive cursor; unknown key restarts).
- `GET /private-sheets/{id}/catalogue/{catalogue}/{key}`: one entry.

The sheet view page links its definition's catalogues (new tab); create and history pages
list none. A damaged title shows the link as unavailable. Readers: the sheet's custodian
(also a departed custodian of a frozen sheet) or an active Master, never the production
Owner; the pin must be a supported runtime pin that links the catalogue. Pages load no
script and no images.

`Store::private_catalogue` holds the single runtime permit through: transaction A (session
recheck, authorization, snapshot and pin); pin decode, write-only validation, match and link
check outside the mutex; transaction B (recheck and authorize again, sheet still corresponds,
pin unchanged, latest sealed version, lookups, final recheck); render. A sheet saved in
between answers 409. Requests check only sealed state and identity columns. No rate limit; a
busy server answers 503.

| Status | When | Page |
|---|---|---|
| 400 | Malformed id, catalogue, key, `type` or `after`; unknown or repeated parameter; `q` > 100 scalars. Before any lookup or session check | "Invalid catalogue address" with the valid syntax |
| 403 | Every refusal: unreadable or unknown sheet, production Owner, native or unsupported pin, unlinked, unknown, other pair's or unsealed catalogue, revocation mid-request | One uniform "Catalogue unavailable" page; only the sheet link varies |
| 404 | All checks passed; key or `type` not in the latest sealed version | "No entry “key” in Title." / "No entry type …", links to browse and sheet |
| 422 | The search cannot run | Browse form with a static message |

The 400/403/404 pages carry `CatalogueNotice` so the auth layer keeps them. An ended session
gets the session layer's 401.

**Badges** (`read::Badge`, one rule for browse and entry pages, computed live):

| Badge | When |
|---|---|
| Owner-written | Owner text |
| Reviewed extraction | Every cited unit accepted or corrected, every cited page complete |
| Unchecked source text | Any other unit-sourced entry |
| Description unavailable | A cited unit rejected now, or its text not text, with NUL or over 256 KiB, or (damage) no cited unit. No passage shown; citations stay |

**Entry text:** the correction for corrected units, else raw text. Metadata of every cited
unit first, then `substr` prefixes in author order, cut at **20,000 characters per entry**
(`read::DISPLAY_CHARACTERS`), ending "Continues in the manual at p. X (PDF page N)". Owner
text is never cut. Citations: source title, current printed label, PDF page. Retired entries
stay listed with a "Retired" badge and stay readable. The page says "Current text": names,
descriptions, badges and citations come from the latest version and today's review.

**Bidi:** all catalogue and manual text is escaped, wrapped in `<bdi>`, `white-space:
pre-wrap`; type options carry `dir="auto"`. Chromium check `tools/catalogue_browser.py` (from
`tools/browser_smoke.py`), both viewports.

**Search:** `q` goes through `sources::search::fts_query` (quoted pieces joined with `AND`, at
most 16; raw text never reaches `MATCH`). A filter, never a rank. Joined back on the same
catalogue's latest sealed version and restricted to its rowid range, so keys shared across
catalogues cannot leak. Every browse statement runs under the 1 s `QUERY_BUDGET`. Only
`fts_query` refusals and an interrupted name search answer 422; other interruptions (plain
browse, types, cursor, badge probe) are errors.

## Lock budget (R8) and the profiler

R8 on the profiling host, release build: publish lock hold ≤ 2 s at the caps; a concurrent
server-pattern writer (`BEGIN IMMEDIATE`, 5 s busy timeout) waits and succeeds; CLI peak RSS
≤ 256 MiB; page p95 ≤ 250 ms. The 5 s busy timeout (`src/store.rs`) is the hard failure
point for concurrent sheet saves. Numbers: [results.md](results.md), "Catalogue publication
lock budget".

`tools/catalogue_profile.py BINARY` (prototype tier, synthetic data) seeds one finalized
source (1,024 pages × 16 units), publishes v1–v3 of the calibrated shape and five shapes at
the caps (`fanout`, `fts`, `declarations`, `fts3`, `keys`), each published then updated, and
times show, verify and rebuild, each command in a fresh process for its own peak RSS. A lock
probe bounds each hold between the last free probe reading the old version count and the
first reading the new one (conservative upper bound). `--no-contender` gives hold evidence;
with the contender it reports wait and success. The gate (`--max-hold-ms`, default 2,000)
exits 1 when nothing was gated, a hold is unbounded or over budget, a requested contender did
not wait through and succeed, or the run is not complete at the Rust caps (`--allow-partial`
is for cap selection, never R8 evidence). R8 needs a passing run of each kind. The worst
browse page: ignored test `catalogues::read::tests::measure_the_worst_browse_page`.
