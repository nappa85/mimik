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

The CLI commands trust local OS/database access and record no account attribution.
Campaign Masters and players have no registry permission.

## Owner browser registry

User decision (2026-10-15): in authenticated mode the Owner may register rulesets
from the browser **in addition to** the CLI; neither path replaces the other. Sign
in as Owner and open **Owner administration → Rulesets** (`/admin/rulesets`). The
page lists 50 editions per page ordered by `(game_id, edition_id)` with an exclusive
composite cursor (`after_game` and `after_edition`, both or neither, each a valid
ID). It offers the same two transitions as the CLI, implemented by the same shared
code, so validation, idempotency, immutability and alias rules are identical:

- **Register edition** (`POST /admin/rulesets`): game ID/label and edition ID/label.
  An exact repeat succeeds and changes nothing.
- **Add exact legacy alias** (`POST /admin/rulesets/aliases`): same-game exact label
  to a registered edition, with matching unresolved-record backfill in the same
  transaction. The native edition still accepts only its seeded alias.

Only the Owner is admitted; players, Masters and anonymous requests are denied.
Forms use strict Origin, durable CSRF rechecked inside the immediate transaction and
before commit, the default 8 KiB body limit and `deny_unknown_fields` (a submitted
`actor` is rejected). Rejected input returns 422 with the submitted values retained
and escaped. Browser work uses the bounded auth workers, so concurrent browser load
can return 503; retry after inspecting the listing. The CLI does not use those workers;
CLI and browser writes contend only through SQLite locking (the 5-second busy timeout),
and a write that loses returns a storage error rather than partial state.

Schema16 adds append-only `registry_events` (kind `ruleset` or `alias`, game/edition
IDs, game label, edition or alias label, timestamp), written by the shared CLI and
browser transitions only when a new edition or alias is created. Exact repeats write
no event. Registrations from earlier schemas keep no invented history. The insert trigger checks
that an event matches an existing edition or alias but cannot know when that row was
created, so someone with direct SQL access could add an event for a pre-history
edition; the application never does, and such a row could not gain a browser sidecar
unless it was the latest event during an authenticated Owner request. This was left as
documented, not tightened, because database administrators can bypass any trigger. Browser writes
also append an immutable `registry_web_events` row (Owner account ID/epoch and one-way
session digest), checked by an insert trigger against the authenticated Owner request
and the latest registry event; audit failure rolls back the registration. CLI events
have no sidecar and carry no account attribution. (The registry history migration is
planned as number 017 at merge; this branch applies it as 016 until the parallel
retrieval migration lands.)

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
