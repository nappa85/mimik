# Campaign workspace and private sheets

Implemented in default authenticated mode: campaign creation/joining, creator-controlled
additional Masters, personal native/runtime sheet editing, complete transactional
history and departure freezing. Material ingestion and account invitations remain
the operator workflows in `source-review-setup.md` and `accounts.md`.

## Start using it

Restart the server after installing the new binary (normal auth serves the LAN;
see `deployment.md` for explicit loopback and external_url proxy settings):

```sh
./target/release/mimik
```

Sign in with a separately enrolled **player account** and choose **Open campaign
workspace**, or open the printed LAN address plus `/campaigns`. In normal mode the Owner account
opens **administration** at `/admin` and cannot play, including through older explicit
grants. Enroll the human's player account using `accounts.md`.
Use your existing bootstrapped database. Schemas12/13
is additive; it preserves accounts/sources/prototype records, creates no campaign
automatically and never assigns old sheets implicitly.

For development testing only, start with `--dev-owner-play` (optionally `--loopback`). This explicit
server option restores Owner gameplay through existing explicit grants/custody;
the session page displays a development warning. It preserves existing records
without reassignment or deletion. Restart without the flag to restore admin-only
Owner access. The flag conflicts with `--no-auth` and cannot accompany CLI subcommands.

1. Enter a title and matching registered game/edition. Creating the campaign makes
   you its original Master, independently of technical deployment ownership.
2. For an exactly loaded Italian MÖRK BORG pair, choose **New native exploration
   sheet** and enter actual values. This limited existing adapter is not an approved
   official sheet or complete legality engine. Advanced armor/item count fields
   are under their summary; story/abilities/equipment grouping and calculations
   reuse the existing adaptive native editor.
3. Other games require an immutable published exact-pair runtime version with
   current wiring approval. Its ordered groups/fields and explicit known/null
   controls enforce scalar types/options/bounds, not game legality. Unavailable
   capabilities are explained; registration alone is insufficient.
4. Save the complete sheet. **Full history** shows creation and every successful
   edit, including no-op saves; select a revision to inspect its retained values.

## Joining, grants and privacy

The authenticated directory discloses only campaign ID/title/exact ruleset and
original Master display identity, supporting join requests without exposing roster,
sheet names/counts or history to outsiders. Current Masters accept/decline requests;
acceptance grants player membership. Only the original creator promotes/demotes
additional active Masters. Removing an active additional Master requires the
original creator to demote them first; additional Masters may leave
themselves. Creator departure/removal/succession is not exposed.

Sheets are self-custodied at creation. Only the custodian and current campaign
Masters may read values/history; other players cannot. Production Owner is excluded
even with retained explicit grants and cannot serve as the supervising Master.
Editing requires active custody/membership or current Master authority,
an enabled supervising Master and an active sheet. New private records are separate
from legacy character tables, so the unauthenticated prototype cannot expose them.

Leave/removal freezes all the departing player's active linked sheets atomically,
preserving values/custody/campaign and appending one freeze revision per sheet.
Both player and Masters then have read-only access. Rejoining never thaws them.
Disablement denies account access without silently transferring custody or performing
departure. Transfer/reactivation/rollback/delete/restore/purge and legacy assignment
need their later policy/implementation slice.

## Revisions, interpretation and integrity

Each write rechecks current session/deadline/account epoch/grants in its transaction.
Stale/invalid409/422 submissions preserve the escaped full draft and original
revision after fresh authorization. A valid no-op advances once. Timeout/cancellation
may precede an admitted atomic commit: reload revision/history before retrying.

Native schema/calculation version1 and runtime definition ID/version/hash/schema
are immutable pins. Unknown future native interpretations are read-only. Missing,
stale or withdrawn wiring blocks new runtime creation; existing structurally valid
active sheets/history remain usable with warnings. Corrupt immutable content is
an error. Source/global context/legacy routes remain denied in auth mode.

One host-canonical resulting snapshot/hash is used for live storage and immutable
history, linked by deferred correspondence FKs. Events retain actor/time/epoch and
one-way session attribution, never replayable cookies/CSRF/passwords. Versioned,
domain-separated hashes detect corruption; they are not a database-owner sandbox.

## Bounds and verification scope

Encoded form/canonical body each≤1MiB;256fields/32groups; text8000, multiline/notes
20000 and name100 Unicode scalars. Decimal i64 transports as text without JS Number
rounding; null/empty/false/zero stay distinct. Writes reserve revision-width headroom
so mandatory freeze cannot fail merely because its revision gains a decimal digit.

Caps:128 campaigns total/32 created per account;64 distinct member/request
reservations per campaign;32 retained sheets per custodian/campaign including frozen
ones;2048 private sheets total. Indexed lists paginate≤50 summaries and show Next
only when another authorized row exists. Roster, requests, versions and sheets have
independent cursors; moving one preserves the other sections. Ruleset selectors
explicitly show the first50 registered pairs. History detail
loads one bounded snapshot; history is never silently evicted. Storage failure rolls
back the operation and every preceding freeze.

The actual release-mode32-sheet near1MiB revision9→10/last-row rollback workload
sampled freeze3051.250ms/rollback2948.294ms and86,012KiB whole-child peak, below
reviewed sampled5s/128MiB targets. These are shared-host observations, not hard
bounds; memory includes fixture/setup/verification. `tools/campaign_profile.py`
uses fresh child accounting and requires both exact workload labels.

Unit/connected-Router races are distinguished from real process/Chromium flows in
`reviews/2026-10-13-campaign-workspace.md`. Recovery/lifecycle,
providers/models, whole-manual and full official-layout certification remain outside
this release. Current sources/accounts/personal data stay local and out of artifacts.
