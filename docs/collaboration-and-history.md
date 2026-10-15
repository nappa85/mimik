# Shared campaigns, permissions and character history

Status: user-confirmed product direction, deferred implementation. No accounts,
authorization, character history, rollback or deletion UI is implemented today.
Current character revisions prevent stale writes; source-review events are not a
character audit trail. The shipping service remains loopback-only.

## Confirmed requirements (2026-10-12)

- Owner-issued account invites: users set their own passwords; no open signup.
- Deployment is private LAN only. TLS is Owner-configurable and optional; do not
  force local-CA setup. HTTP and TLS modes need explicit supported-origin/session
  configuration; explain transport differences in setup documentation.
- Any enrolled user can create a campaign and becomes Master of that campaign.
  Joining is a request accepted/declined by that campaign's Master, not implicit
  membership or an invite-based campaign join flow.
- Sheets and edit history are private to the character's player and campaign
  Master; other campaign members and technical Owner have no automatic read/edit.

- An instance will be shared by players; introduce user management and protect
  characters from edits by other players.
- Users belong to explicit subsets of campaigns, not every campaign. Master
  grants are per campaign: being Master in one does not grant rights in another.
- Every new sheet belongs to a campaign. A campaign binds an exact registered
  `(game_id, edition_id)` ruleset: reject D&D sheets in MÖRK BORG campaigns and
  D&D3.5 sheets in D&D5 campaigns. Sheet layout/definition versions must belong
  to that same pair; never infer compatibility from a family/display label.
- **Player/character custodian:** can edit their active campaign character with
  Master supervision. Departure/removal freezes the linked sheet as read-only.
- **Master:** directs a campaign and can edit its characters. This is a
  campaign-scoped role, not deployment administration.
- **Owner:** the technical person who owns/administers the deployment. Owner and
  Master are distinct roles. One person uses separate Owner and player accounts;
  the production Owner account is administration-only even with retained grants.
- Preserve the full history of all sheet edits, with rollback available.
- Add a character **Delete** button that performs logical/soft deletion.
- Only Owner may physically purge character records. Master is not entitled to
  purge just by directing the campaign.

These are requirements for a future slice, not an instruction to expose the
present unauthenticated service to the network.

### Confirmed policy clarification (2026-10-13)

- Subsequent user feedback supersedes the earlier combined-role allowance for the
  Owner account: in normal deployment Owner is admin-only and cannot participate
  in gameplay even through explicit old grants/custody. The human uses a separate
  enrolled player account. An explicit development-only override preserves testing,
  without automatic reassignment/deletion of existing development records.

- A campaign normally has a single Master: its creator. Multiple Masters are
  supported; the original creator promotes or demotes enrolled campaign players.
  Additional Masters may serve temporarily (for example while the creator is ill)
  or long-term in a large campaign. Automatic expiry is not specified. Additional
  Masters do not inherit promotion/demotion authority from this decision.
- Characters are personal to their owning player. The owning player decides
  whether to transfer a character to another player; departure, removal or account
  disablement does not authorize automatic reassignment.
- Owner may disable accounts and issue password-reset links that invalidate
  existing sessions. These administrative powers provide no implicit sheet/history
  read or edit access.
- After departure/removal, the owning player always retains read-only sheet
  access. The sheet remains linked to its campaign and exact ruleset, but is
  frozen: neither the player nor the campaign Masters can edit or progress it.
  Current Masters of that linked campaign retain read-only sheet/history access.
  This does not give demoted/revoked Masters a historical access grant. Existing
  player/Master-private history requirements remain in force.
- Departure/removal does not detach the sheet, reassign ownership or transfer it
  to another campaign/player. Personal ownership and player-decided transfer
  authority remain unchanged. Automatic reactivation and cross-campaign transfer
  behavior are not specified by this clarification.

## Proposed design and acceptance boundaries

- Authenticate users with maintained Rust session/authentication facilities;
  evaluate library maintenance, licensing, capability coverage and runtime cost
  before implementation. Server-side authorization applies to every mutation,
  direct HTTP access, history, exports and model task context, not just buttons.
- Separate instance administration, campaign membership/role grants and character
  custody. Evaluate each request against current grants; stale sessions or tabs
  cannot retain revoked permissions. Actor identity comes from authentication,
  not a form's reviewer/name field.
- Put every successful identity/sheet mutation and its history event in the same
  transaction. Include operation, actor, time, expected/resulting revisions,
  immutable definition/schema pin and complete before/after reconstruction data.
  Cover native and generic editors, imports, migrations and administrative paths.
  Failed/stale/unauthorized edits do not produce a successful-edit event.
- **Rollback is a new revision:** preserve the original events, bind the target
  event and current expected revision, validate restored data against its version,
  and record who restored what. Never overwrite old history or decrement the live
  revision. Incompatible definitions require an explicit version/migration policy.
- Soft deletion retains the record and its history; hide it from ordinary active
  lists and reject normal editing while deleted. Restore and purge are explicit
  separate operations with reviewed authorization and concurrency rules.
- No ordinary edit/soft-delete/rollback operation physically removes history.
  A future Owner purge may deliberately end recoverability; the UI must make the
  confirmed purge scope explicit. Database purge cannot promise removal from
  previously exported files or backups.
- Introduce schema/mutation boundaries now that can later carry authenticated
  actor and history metadata, but do not label the current local prototype's
  untrusted actor labels as authenticated attribution.

## Decisions to ask the user before implementation

1. Visibility answered: player and campaign Master only. Soft-delete, restore and
   rollback grants still need a decision when that slice is active.
2. Owner has no implicit sheet/history read/edit grant; enrollment uses Owner-issued
   invites. Owner account disable/reset-link powers and session invalidation are
   confirmed; implementation mechanics require a reviewed design.
3. Multiple Masters, creator-controlled promotion/demotion and personal
   owner-player-controlled transfers are confirmed. After departure/removal the
   linked sheet is frozen and current campaign Masters retain read-only
   sheet/history access. Existing unaffiliated prototype
   sheets stay unassigned until explicit migration, not silently shared. Changing
   a campaign ruleset or moving a sheet across rulesets needs a separate policy.
4. What does "always possible" mean after an explicit Owner purge? Proposed
   interpretation: recoverable retained history survives ordinary edits and soft
   deletion; deliberate physical purge is the explicit exception. Confirm rather
   than silently promise both erasure and permanent recoverability.
5. Does purge remove character history, attachments, advice outputs and linked
   references? Which minimal administrative audit, if any, survives? How do
   backup retention and former/deleted user attribution work?
6. Should rollback restore a soft-deleted record, change its definition version,
   or only restore values compatible with the current version? Historical edits
   made before history was introduced cannot be retroactively reconstructed.

## Delivery order

Keep runtime unknown-game definitions as the current priority. Before enabling
shared-network operation, deliver reviewed campaign/account authorization,
transactional complete character history and rollback, soft deletion/restoration,
and Owner-only purge with the retention policy settled. Retain denial/isolation,
revocation, revision race, restart/recovery and cross-version regressions in normal
CI; extend browser/artifact gates for the explicitly supported sharing scope.
