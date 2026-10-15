# Stable runtime ruleset registry

Registry/source-identity slice implemented; generic definitions, campaign binding,
accounts and generic sheet creation remain future work. Registration identifies
materials; it does not approve evidence or enable unsupported game rules.

## Operator CLI

From the project directory, using a local database (default `data/mimik.sqlite3`):

```sh
./target/release/mimik register-ruleset \
  --game-id my-game --game-label "My game" \
  --edition-id first --edition-label "First edition"
./target/release/mimik list-rulesets
```

Listing is deterministic JSON with stable IDs, labels, native adapter identity
when implemented, and `generic_sheet_available: false` for this slice. Exact
repeated registration is idempotent; IDs/labels cannot be changed or reassigned.
IDs are lower-case ASCII slugs up to64bytes, beginning with a letter; labels are
1–100 Unicode scalar values without surrounding whitespace. Edition IDs and
labels are unique within a game. No inference from filenames or approximate labels.

For a v1 source/legacy character carrying an old exact edition label:

```sh
./target/release/mimik register-ruleset-alias \
  --game-id my-game --edition-id first --legacy-edition "Old exact label"
```

Aliases are same-game, exact and immutable; labels cannot ambiguously refer to
another edition. Registration checks existing aliases and alias creation checks
registered labels. Alias insertion and matching unresolved-record backfill are
one transaction; existing associations are never overwritten. Seeded native
`mork-borg / it-2025` accepts only its exact seeded Italian label alias.

These are local operator commands, not authenticated Owner/Master permissions.
The technical Owner and campaign Master are separate planned roles.

## Prepared bundle identity

Version2 retains v1 artifact/page/unit fields and adds required `edition_id`.
`system_id` is the registered game ID, `edition_id` the exact stable edition,
and `edition` its registered label or an exact alias agreeing with the complete
stable pair. Register the pair before importing. Missing, null, malformed or
contradictory IDs fail; aliases never override explicit IDs. Label limits use
Unicode scalar counts. Version1 rejects any `edition_id` member, including null.

```json
{
  "contract_version": 2,
  "system_id": "my-game",
  "edition_id": "first",
  "edition": "First edition"
}
```

This shows identity fields only, **not a complete import bundle**. All original
PDF/JSON/PNG/hash/geometry/provenance checks and local artifact restrictions still
apply. Import with `mimik import-source --bundle PATH --pdf PATH`.

For transition, v1 legacy `dnd`, `pathfinder`, `vampire`, `mork-borg` imports can
remain unresolved; unknown families need an explicit alias or v2 identity.
Unresolved compatibility imports cannot enable a native adapter from their
labels. Different source runs remain separate unreviewed evidence. Duplicate
bundle import checks the same association before idempotency/coverage backfill;
any mismatch rejects without changing artifacts or review state.

## Lookup selectors

Preferred HTTP query: `/sources/search?system=my-game&edition_id=first&query=...`.
Use `edition=Exact+label` instead for a legacy/display-label query; the form offers
both selectors and requires exactly one for a nonempty lookup. Stable IDs never
fall back to labels. A valid unregistered pair yields the unsupported-source
state; malformed IDs or conflicting/missing selectors are rejected.

Display labels resolve exact aliases first, then unique registered labels. A
label with no registered resolution searches only unassociated legacy sources.
Registering a coincident label does not implicitly attach a v1 source: explicit
alias backfill is needed before canonical lookup can include it. All counts,
coverage and reviewed matches share the same identity predicate/read snapshot.
Registered lookup aggregates accepted aliases within one exact edition, never
another game/edition. Query `%`, `_`, `\` remain literal, not wildcards.

## Migration and compatibility

Schema5 adds registry/alias and source/character junction tables. Missing junction
row means unresolved; associated pairs are NOTNULL and composite foreign-key
checked. This avoids rebuilding the existing character/source tables. Atomic
migration seeds the native exact alias and backfills only exact stored strings;
payloads, revisions, PNG/PDFs, corrections/completeness and events are unchanged.
Unknown legacy records remain unassigned. Registry/alias mutations are constrained
to immutable insert-only identities; no deletion/reassignment API is introduced.

Compiled adapter activation uses the canonical pair, not a label or `native`
metadata flag. Existing native fields/checklist remain usable; registry entries
for new games only unlock source identity/review/lookup in this slice. Current
legacy creation is still the existing native local prototype flow. Future new
generic sheets require a campaign with a matching exact game/edition; no automatic
campaign assignment or authentication is claimed.
