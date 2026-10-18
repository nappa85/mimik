# Read-only catalogue plan (step 1 of the definition-language order of work)

2026-10-18. Status: **revision 1, draft for independent review.** No code yet.
Context:
- [definition-language-draft.md](definition-language-draft.md), with its user decisions and
  product principle;
- the review record [reviews/2026-10-18-definition-language-draft.md](reviews/2026-10-18-definition-language-draft.md),
  findings 4, 5 and 15 and order-of-work step 1;
- [derived-formulas-m4-plan.md](derived-formulas-m4-plan.md) §8, which this milestone
  partly takes over.

## 0. Goal and non-goals

**User value.** A player selects a spell or item on their sheet and reads its full
description and source citation without opening the manual. They can also browse and search
the game's catalogue from the sheet. This is "play almost without reading the manual" at its
smallest.

**In scope**
- A catalogue artifact separate from the definition: drafts, immutable published versions,
  and approvals.
- List-row properties that reference a catalogue entry, validated structurally like enum
  options.
- Entry pages and a catalogue browse/search page reachable from a sheet, for whoever can read
  that sheet.
- Description text materialised from source units, or written by the owner, with a per-entry
  review and staleness badge.

**Out of scope** (later steps, each with its own plan)
- Copying an entry's stats onto rows (projection) and `editable: false`.
- Formulas over entry data or choices filtered by formulas.
- Legality warnings.
- LLM extraction or authoring.
- Page images for non-owners (see the user question Q1).
- Revert.
- Campaign-level browse without a sheet.
- Language pairing (an Italian and an English edition of one entry).

No model is involved anywhere in this step.

## 1. Facts this plan rests on (verified 2026-10-18)

**The definition envelope is closed.** The pipeline is `decode::parse` (duplicate keys, sizes,
nodes), then the embedded `schemas/definition-v1.json`, then serde with
`deny_unknown_fields` (`src/definitions/mod.rs:99-117`), then domain checks, canonicalisation
and the hash (`admission.rs:27-32`). The definition is capped at 256 KiB
(`definitions/mod.rs:26`). Mapping and formula evidence is capped at 1,024 distinct regions
(`admission.rs:181`, `formula_admission.rs:375`). The approval snapshot is capped at 1 MiB
(`publication.rs:9`). Page-scoped invalidation: a unit review advances its page revision
(`migrations/009_review_integrity.sql`). So a catalogue cannot live inside the definition.

**Effective unit text** is the correction for `corrected` units and `raw_text` otherwise;
approval hashes this value (`publication.rs:221-251`). Search indexes only accepted or
corrected units (`017_source_search.sql`). Unit states are unreviewed, accepted, corrected and
rejected.

**List rows.** The row ID `r<N>-<k>` is a slot label, not an identity
(`runtime-definition-contract.md:288-295`). There are at most 256 rows and 32 properties per
list (`formula/mod.rs:32,39`). Publication enforces form budgets of 32,768 render units,
65,536 pairs and 32,768 nodes (`campaigns/values.rs:176-265`, `publication.rs:127-133`).

**Pins.** `private_sheets.pin` binds the definition version id, version and content hash,
enforced by a trigger (`012_campaigns.sql:56-99`). Pins never reinterpret saved values.

**Reading and writing sheets.**
- `authorize_read` allows the custodian or an active Master (`snapshots.rs:271-286`).
- Every request re-checks the session.
- Runtime routes hold the definitions `JOBS` permit, a semaphore of 1 with `try_acquire`,
  so a busy server answers 503 rather than queueing (`campaigns/web.rs:42-56`).

**Auth-mode pages run no JavaScript.** htmx returns 403 there (`auth/tests.rs:1037-1050`).
The only expand pattern is native `<details>` (`templates/private_sheet.html:7-16`).
Headers are no-store with CSP `frame-ancestors 'none'`.

**Players and Masters have no evidence route.** Source pages and images are Owner-only in auth
mode (`auth/materials.rs:29-54`).

**Definition administration is CLI-only** (`src/main.rs`).

## 2. Decisions (proposed; reviewers please challenge)

**D1. A separate artifact.**
- New tables mirror definition drafts, versions, approvals and withdrawals:
  `catalogue_drafts`, `catalogue_versions`, `catalogue_approvals`, `catalogue_withdrawals`,
  plus `catalogue_entry_texts` for the materialised descriptions (see D4).
- Each draft has a revision. A published version is immutable, with content hash
  `sha256("mimik-catalogue-v1\0" + canonical)`.
- SQL triggers prohibit edits and deletions, following migrations 008/009.

**D2. Envelope** (closed, own trusted schema `schemas/catalogue-v1.json`, same decode → schema →
serde → domain → canonical pipeline):

```json
{ "contract_version": 1, "game_id": "…", "edition_id": "…", "catalogue_id": "spells",
  "title": "Spells",
  "sources": [{ "id": 3, "pdf_sha256": "…" }],
  "entry_types": [{ "id": "spell", "label": "Spell",
      "properties": [{ "id": "level", "label": "Level", "control": "integer" },
                     { "id": "school", "label": "School", "control": "enum", "options": ["…"] }] }],
  "entries": [{ "key": "lantern-flare", "type": "spell", "name": "Lantern Flare",
      "properties": { "level": 1, "school": "…" },
      "description": { "units": [{ "source": 3, "page": 41, "unit": 977 }] } },
    { "key": "house-charm", "type": "spell", "name": "House Charm",
      "properties": { "level": 0 },
      "description": { "owner_text": "…" } }] }
```

- Keys follow the registry slug grammar and are unique per catalogue. A key is the
  **entry identity**, stable across versions by convention. This plan only requires that a
  key be immutable within a version. Cross-version remapping is deferred, but a later upgrade
  path depends on this stability.
- Entry property controls are scalar only: text, integer, boolean, enum. They are for display
  and browse filtering in this step.
- The description is either 1–16 unit references (in document order) or an owner-written text
  of at most 20,000 scalars. An entry with no manual source is shown as "written by the
  owner, not from a manual".
- Units must belong to a finalized source in the catalogue's `sources`, with the exact
  registered game/edition pair, as definition mappings require today.

**D3. Limits** (proposed; calibrated in step 6 below):

| Resource | Limit |
|---|---|
| Canonical draft | 4 MiB |
| Entries | 4,096 per catalogue |
| Entry types | 32 |
| Properties | 16 per type |
| Enum options | 128 per property |
| Unit references | 16 per entry; 16,384 distinct per catalogue |
| Name | 100 scalars |
| Materialised description | 20,000 scalars per entry |
| Catalogues per definition | 8 |

- Admission takes the existing `definitions::JOBS` permit (no queueing).
- Decoding enforces the limits before allocation, as `decode.rs` does.

**D4. Materialised text and per-entry staleness.**
- On approval, the host builds each entry's description from the units' effective text,
  joined in order, and stores it in `catalogue_entry_texts`, keyed by (approval, key). Each row
  records `text_hash`, plus `unit_id`, `unit_revision`, `state` and the effective-text hash
  for every unit.
- Approval refuses a materialised description over 20,000 scalars.
- **Invalidation is per entry, not per page.** An entry is stale when any of its units'
  revision differs from the captured one. A stale entry still shows its approved text, with
  "the source was edited after this description was approved".
- Re-approval appends a new approval with fresh texts. The version is unchanged, because the
  content (references) is unchanged.
- The sheet shows the **latest approval's** texts for its pinned catalogue version.
  Descriptions are display aids, not sheet values, so a corrected extraction reaches every
  sheet without a new pin. History stays reproducible: rows store keys, and keys are
  version-pinned.

**D5. Review badges per entry.**
- **Checked:** every unit is accepted or corrected.
- **Unchecked source text:** at least one unit is unreviewed. This is allowed, following
  `source-quality.md` step 7, which lets unresolved sections be shown as unchecked source
  material.
- **Owner-written.**
- **Stale** (D4).
- Rejected units are refused at approval.
- Badges are shown on the entry page and in browse results. This is the "trust a wrong hint"
  safeguard from the product principle.

**D6. Approval workflow.** It mirrors definitions, CLI-only, with an operator label as actor:

```
mimik import-catalogue-draft --file spells.json --expected-revision N
mimik compare-catalogue --id D
mimik approve-catalogue --id D --comparison-hash H --actor … --rationale …
mimik withdraw-catalogue --id D --expected-approval A
```

- The comparison reports counts per badge, entries over limits, and ticket hashes. It
  never prints manual text, following the existing CLI rule.
- No `--originals-compared` assertion is required. Catalogue approval says "these entries
  point at these units". It does not certify them; D5's badges carry that.

**D7. Linking definitions.** The definition envelope gains two optional members, omitted when
absent so existing canonical bytes and frozen hash vectors are unchanged:
- `catalogues: [{ "id": "spells", "version": 4, "content_hash": "…" }]` at the top level;
- a list item property control `catalogue_ref` with `{ "catalogue": "spells", "entry_type": "spell" }`.

Definition admission requires the referenced catalogue version to exist with the matching
hash and the exact game/edition pair. Definition publication requires a published catalogue
version; a current approval is not required, and staleness stays visible. The sheet pin
binds the definition version, which binds the catalogue version, so pins and history stay
reproducible.

**D8. Values and validation.**
- A `catalogue_ref` property value is an entry key string (canonical JSON string, 1–64 ASCII).
- The writer rejects a key that is absent from the pinned catalogue version or has another
  entry type. This is **structural** validation, the same class as enum options, not a
  legality check, so the advisory contract is untouched.
- Formulas read the property as a string (the `text` CEL type). The formula profile is
  unchanged.
- The history diff shows the key and resolves the entry name through the pin.

**D9. Picking an entry without JavaScript.**
- In edit mode, a `catalogue_ref` cell is a text input with `list=` pointing at one shared
  `<datalist>` per (catalogue, entry type). Each option has the entry key as its value and
  the name as its label.
- An unknown key is a normal 422 re-render that preserves the draft.
- The form budget counts each datalist's options **once per page**. This needs a
  `form_budget` change, with publication refusing over-budget definitions as today.
- A shared datalist costs at most 4,096 options; per-row selects would cost 256 × 4,096.
- In view mode, the cell shows the entry name linked to its entry page, plus a `<details>`
  holding the first 300 characters of the description.

**D10. Entry and browse routes**, all under the sheet so authorization is exactly
`authorize_read`:
- `GET /private-sheets/{id}/catalogue/{catalogue}/{key}` shows the full description,
  properties, badges and the source citation: source title, printed page label and PDF page
  number, with no image (Q1).
- `GET /private-sheets/{id}/catalogue/{catalogue}?type=…&q=…&after=…` is a plain GET search
  form. It matches name substrings and exact property filters and returns at most 50 results
  per page with an exclusive cursor, like the other index pages.
- Both routes:
  - read through the bounded SQL `text(column, limit)` guard;
  - hold the runtime permit;
  - run under `run_bounded_auth`;
  - send the existing no-store and CSP headers.
- Askama escapes all text. Manual text is never rendered as HTML, so injected markup or
  prompt text in a manual stays inert. Line breaks are kept with CSS `white-space: pre-wrap`.

**D11. Storage and migration.**
- `019_catalogues.sql`, with `SCHEMA_VERSION` 19.
- Additive and atomic: append-only and immutability triggers, foreign keys to finalized
  sources and units, byte and identity checks, as in 006/008/009.
- Older binaries reject the schema, as today.

## 3. Implementation order (each step: failing-first tests, then code)

1. Catalogue envelope, schema, decoder limits, domain checks, canonical encoding, and a
   frozen hash vector. Draft table and CLI import and show. Tests: duplicate or escaped keys,
   unknown members, every limit plus one, wrong pair, unit outside the sources, rejected
   unit, oversize owner text, hash vector.
2. Compare, approve, withdraw, materialisation, staleness computation, badges. Tests: a unit
   review makes exactly that entry stale and not its page neighbours; re-approval clears it;
   withdrawal; concurrency conflicts (stale ticket, intervening review); the materialised
   size limit; comparison output contains no manual text.
3. Definition linkage (D7) and the `catalogue_ref` control (D8). Tests: frozen definition
   hash vectors still pass unchanged; admission and publication refusal cases; the writer
   rejects an unknown or mistyped key; CEL sees a string; history diff names.
4. Sheet rendering and the datalist (D9), plus the `form_budget` change. Tests: the budget
   counts the datalist once; an over-budget definition is refused; the 422 path preserves the
   draft.
5. Entry and browse routes (D10). Tests: authorization matrix (custodian, Master, other
   player, outsider, Owner, frozen sheet, disabled account); escaping of hostile text; cursor
   paging; the permit is busy.
6. Measurements: a synthetic 4,096-entry catalogue with the maximum units. Measure import,
   approve and browse time and peak RSS in release mode, with `tools/definition_profile.py`
   style accounting. Adjust D3 if needed, before review.
7. Docs (`runtime-definition-contract.md`, `definition-publication.md`, `release-scope.md`,
   `README.md` commands), then the full CI gate: fmt, pedantic Clippy, locked tests, tools
   tests, audit, about, release build and package, `smoke_release.py --auth` extended with a
   catalogue entry page, Chromium at both viewports (datalist pick, entry page, `<details>`),
   and the LAN smoke.

All fixtures are synthetic original text (for example the Lantern Reach demo game). No manual
text enters the repo or the tests.

## 4. Risks

- **Effective text quality.** `raw_text` of an unreviewed unit can be garbled. The badges
  make it visible but don't fix it. Fixing it is source review, outside this plan.
- **Unit granularity.** A description may span page breaks, columns and sidebars. 16 ordered
  unit references cover the common case; a description that needs part of a unit needs an
  owner correction or owner text.
- **Datalist usability** varies by browser, especially on mobile. The Chromium check covers
  one browser. Fallback: the browse page links back with the key to copy, which is clunky
  but works without JavaScript.
- **Errata and key stability** across versions are only a convention here. The upgrade or
  remap path is deferred, with M2 of the draft review.
- **Authoring by hand** is slow for hundreds of entries. A deterministic helper (draft entries
  from a heading range of units) is the natural next step, before any LLM.

## 5. Questions for the user (recommended defaults)

- **Q1. Page images for players.** Players and Masters could see the manual page image or
  region under an entry. Today only the Owner sees page images. Showing them shares pages of
  the owner's manuals with the other players over the network. **Default: citation only
  (book, page) in this step; images are a separate decision.**
- **Q2. Unchecked text.** Entries whose units are not yet reviewed are shown with an
  "unchecked" badge. The alternative is to hide them until reviewed. **Default: show with the
  badge.**
- **Q3. Owner-written entries** (house rules, homebrew items) are allowed, with an
  "owner-written" badge. **Default: allow.**

## 6. Review

Independent adversarial review before code: Sol and Sonnet in parallel, plus the third chain.
The implementation review follows step 7. Records go in `docs/reviews/`.
