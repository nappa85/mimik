# Stage C — usable campaign workspace and private sheets

Status: amended plan received independent GO; resumed implementation/fix review
approved and full gates passed, with final artifacts pending. User reported that
the verified session checkpoint has no useful actions. This slice makes `--auth`
usable for campaign creation/joining, personal sheet editing and private history;
it stays loopback-only. Policies: `collaboration-and-history.md`; sessions:
`http-auth.md`; definition limits: `runtime-definition-contract.md`.

## Delivery and maintained-library boundaries

Reuse Axum/Askama/rusqlite/session/serde/jsonschema/SHA-256; no new protocol, ORM,
rules engine or generic infrastructure dependency. Any decoder generalization
retains Serde tokenization and the existing demonstrated duplicate-key guard.
Use a shared native field/calculation partial with parameterized private action/
CSRF wrapper, not copied validation/calculation code. MÖRK BORG is the existing
exploration fixture; runtime definitions enable other games without native code.

Auth session page links to a real `/campaigns` workspace. Any enabled enrolled
user can create a campaign with exact registered game/edition and becomes its
original Master. Authenticated discovery exposes only ID/title/ruleset/original
Master display identity, not roster, requests, sheet names/counts or history.
This minimal directory is a proposed inference needed for join requests, not a
claim that broader campaign content visibility was confirmed.

Campaign detail exposes metadata/request status to outsiders; current members
see roster and their own sheets, current Masters see campaign sheets and pending
join requests. Departed custodians find their frozen sheets in a personal list.
Native sheets are available only for an exactly loaded implemented Italian pair;
generic creation lists immutable matching currently approved wiring versions.
Empty capability states explain source/definition prerequisites without fake
approval or an identity-only exception. A new native draft uses existing default
values with an explicit 'enter actual values' badge, never automatic game legality.
Generic creation presents all pinned fields as an explicit full submitted body.

## Concrete schema and campaign transitions

Migration12 is additive and stores campaigns, memberships, join requests,
non-secret administrative campaign events, **separate private sheets and sheet
history**. Never insert private sheets into legacy `characters`/native tables:
the unauthenticated prototype must not list/read them. Never attach old sheets
implicitly. Registry/published-version/account FKs retain exact identities.

- Campaign: positive immutable ID/creator/ruleset; title1–100 nonblank scalars;
  revision1 immutable for now (rename/ruleset moves unsupported). One creator
  membership active/master is created in the same immediate transaction. Deletion,
  creator departure/removal, succession and ruleset changes are not exposed.
- Membership: unique campaign/account pair, positive revision; state active or
  inactive, role player/master. Only active role grants authority. Only original
  creator can promote/demote an active noncreator member, with expected revision.
  Targets must be enabled for new grants; disabled accounts do not gain access.
- Request: unique campaign/account row, pending/accepted/declined and revision.
  A first request starts at1. A pending duplicate conflicts; an inactive user may
  resubmit a terminal request by advancing revision exactly once to pending.
  Accept/decline requires a current Master and exact pending revision; acceptance
  enables/re-enables membership as player, never restores a historical Master
  grant. Requests are not membership. Existing active members cannot request.
- Leave: any active noncreator may leave; inactive role value can remain historical
  but provides no authority. Removal: current Master can remove active players;
  an active additional Master must first be demoted by the original creator.
  Creator cannot be removed. Rejoining does not thaw a frozen sheet.
- Leave/removal deactivates membership and freezes every active sheet owned by
  that account in the campaign, with full history, atomically. Already frozen
  sheets are not re-revised. No custody/campaign change occurs. Account disable
  denies authentication but does not implicitly mean departure or freeze.

SQL guards enforce identity/FK/role/transition/revision/overflow invariants and
append-only events; authorization also remains explicit in domain transactions.
Role changes do not depend on a cached Owner/session role. Original creator
administrative authority is distinct from technical deployment Owner.

## Private records, pins and history

Private sheet: immutable ID, campaign/ruleset/custodian (self at creation), kind
and interpretation pin; positive revision; active/frozen; canonical snapshot text
and domain-separated hash. Frozen is irreversible in this slice. Read/history:
custodian or current linked-campaign Master, even after custodian departure;
never other players, demoted Masters or Owner without a separate grant. Write:
active custodian who is an active campaign member or current Master, with an
enabled supervising current Master in the campaign. Frozen denies both roles.
Transfer/reactivation/delete/rollback/purge/import operations remain absent.

Native pin: `mork-borg-it-2025`, canonical private schema1 and calculation/adapter
version1. Reuse Sheet/SheetDraft validation and deterministic calculations, with
full replacement transport despite legacy omission compatibility. Future native
versions must retain the old interpreter or show an unsupported pin read-only;
never silently reinterpret saved history with changed calculations.
Runtime pin: immutable definition row ID/version/content hash and schema1, exact
campaign pair. Creation rechecks current wiring approval in the write transaction;
stale/withdrawn pinned sheets remain structurally editable with an explicit warning.
Registration/publication is not game legality or official-layout certification.

Snapshot encoding1 host-serializes fixed metadata order (sheet/campaign/ruleset/
custodian/kind/pins/revision/state/name/notes/data), field values in sorted ID order,
no floats/unknown data. Canonical bytes and SHA-256 over
`b"mimik-private-sheet-v1\0" + bytes` are retained. Creation revision1, each valid
edit including no-op advances exactly once, each newly frozen sheet advances once.
Every corresponding history row includes full reconstructable snapshot/hash,
operation create/edit/freeze, actor account/current epoch, trusted Unix time,
previous/resulting revisions and one-way SHA-256 session provenance digest
(domain `mimik-history-session-v1\0` over library ID). No replayable cookie/CSRF/
password value appears in audit, pages or logs. No global draft cache.

History unique sheet/revision and append-only guards. Live updates and history
inserts commit together through the concrete shared writer below. Full reconstruction verifies pin/hash/
canonical identity; digest is corruption evidence, not a database-owner sandbox.
History lists paginate metadata only; a selected revision loads one full snapshot.
Do not evict history to satisfy capacity or claim preexisting edits reconstructed.

## Transactional auth and data privacy

Introduce an opaque request authorization context derived only from the library
Session: captured original ID/deadline, account ID/epoch. Before any protected
mutation, its immediate transaction rechecks active matching session/unchanged
deadline/current clock, enabled account/epoch, exact current membership/role,
sheet custody/state/revision/pin and current creation capability. Recheck time
after potentially blocking resource fetches. Logout/rotation/reset/disable/
demotion/removal cannot commit before this boundary and retain old write authority.
CSRF/Origin precede work but do not establish domain permissions.

Reads authorize and fetch data from one coherent SQL read snapshot, with final
session/deadline admission at that boundary. No authorize-then-fetch gap across
connections. Already admitted reads cannot be retroactively retracted. All new
routes use bounded auth SQL; missing/unauthorized private IDs return the same
static denied/not-found response rather than leaking names or existence.
Campaign metadata is allowlisted independently of sheet/roster permissions.

Only campaign/private-sheet/history routes are added to auth mode. Legacy
character and global source/context/image routers remain denied. Native cited
page numbers are plain references, not unauthorized manual links. Source previews
or situational advice need a future authorized sheet→exact-source/page/unit chain;
never merge global source routes wholesale. Default prototype uses only old tables.

## Transport, validation, admission and bounds

Keep auth/login/logout and campaign control forms ≤8KiB; sheet URL-encoded
submission and decoded canonical snapshot independently ≤1MiB. Acquire the
existing nonqueued runtime work permit before sheet body allocation/validation
scheduling; hold it through detached blocking parsing/compilation and SQL handoff.
Reuse bounded SQL16/request8/timeouts5s; one runtime compile/admission per process,
503/Retry-After on saturation. Cancel/timeout may precede an atomic commit; reload
revision/history before retry, never assume failure means no commit occurred.

Use maintained serde_urlencoded parsing to retain pairs, then reject duplicate/
unknown/missing form keys before map conversion. JSON snapshot decoding uses
existing bounded Serde duplicate-key/token/depth/node guard with a sheet profile;
do not add a tokenizer. Integers transport as exact text, i64 only without floating
or exponent coercion; explicit full replacement preserves null/empty/false/zero.
Generic nullable fields have explicit null controls; booleans use true/false
selects rather than duplicate hidden/checkbox keys. Host-generated offline
jsonschema2020-12 validates admitted pinned controls/options/nullability/bounds;
no schema splicing, remote references, model approval or arbitrary rules.

Limits match runtime contract:256fields/32groups, depth12/nodes32768, derived
schema≤256KiB, text8000/multiline20000 scalars, name100/notes20000, diagnostics32
items×256scalars. Native fields retain their existing stricter domain ranges.
Private full replacement requires all native fields; missing optional legacy
fields cannot silently retain old values. Canonical overflow fails before write.

Lists use indexed exclusive-ID pagination≤50 rows: campaigns/members/requests/
accessible sheets/history summaries. History detail is one bounded snapshot.
Operational caps:128campaigns total,32created per account,64membership/request
reservations per campaign,32sheets per custodian/campaign,2048private sheets total.
Departure freezes at most32 records, streaming snapshots/history sequentially.
Retained history has no eviction quota; disk/SQL failure rolls back the operation.
Other than the32-sheet workload these numeric policies are unmeasured planning
bounds, not throughput/latency claims.

Container capacity probe `campaign-capacity-probe.md`:32×1MiB snapshots streamed
in one Python/SQLite prototype transaction had median1.175s/max1.276s across3
samples, peak child RSS41,260KiB. It is preliminary lower-fidelity cost evidence,
not Rust/security/durability certification. Before release, measure actual Rust
maximum32-sheet freeze and rollback; acceptance target≤5s sampled transaction and
≤128MiB sampled process RSS on the documented shared host, never a hard bound.
If this fails, revise the reviewed capacity rather than weakening history/guards.

## Mandatory canonical headroom, shared writer and immutable-pin pipeline

- Every active create/edit budgets canonical size with maximum-width i64 revision
  and the largest supported state spelling, including native data's mirrored
  revision. Reject consumption of reserved space before persistence; do not pad,
  truncate or alter values during freeze. Active revision must remain below
  i64::MAX so a mandatory freeze still has one representable revision; further
  edits at that boundary reject, while freeze may reach i64::MAX. A near-cap
  revision9→10 freeze preserves every value. Actual32-sheet workload must exercise
  digit growth/headroom, not only1→2. Sheet caps count frozen retained sheets;
  request/member capacity counts distinct campaign/account reservations, not both
  rows for one accepted account. Rejoin releases neither capacity nor freezes.
- Select a narrow shared domain writer for create/native edit/generic edit/freeze.
  It receives trusted transaction-local authorization, expected old revision,
  admitted pin and resulting typed state; produces one host-canonical snapshot
  and domain-separated hash, and uses those exact bytes/hash for live and history.
  Creation0→1, edit/freeze r→r+1; validate canonical identity/pin/state/revision
  against immutable live metadata. Actor/epoch/session digest/time are trusted
  context outputs, never submitted form values. Allocate new positive sheet ID
  inside BEGIN IMMEDIATE with checked maximum+1 (rows cannot be deleted), before
  encoding the snapshot containing that ID.
- Enforce live→current-history correspondence with a deferred FK from live
  `(id,revision,hash)` to a unique history `(sheet_id,revision,hash)` key, plus
  history's FK to its live sheet. All Store connections require foreign_keys ON;
  missing history causes commit failure. History INSERT guard requires identical
  live snapshot/hash/state/pin/identity, valid create/edit/freeze previous revision
  semantics and preceding history for noncreation. Immutable rows/append-only
  SQL guards enforce shapes, identities and sequence; the shared writer owns
  SHA-256/canonical validation and transaction-local authorization. Exactly one
  live mutation and one history append per transition; any row-count/trigger/
  history failure rolls back, including all prior streamed freezes. Reads verify
  event metadata against the canonical snapshot and latest correspondence, not
  only recompute digest. Direct FK-disabled administrator tampering is not a
  sandbox guarantee; bounded reads still reject missing/mismatched history.
- Define a connection-scoped immutable version loader, distinct from the existing
  async definition_version API: bounded SQL projection reads ID/ruleset/version/
  hash/canonical definition bytes. Under the already-held shared runtime permit,
  validate retained definition schema/canonical hash/identity in a blocking worker
  outside the connection mutex; no source readiness prerequisite. Revalidate
  immutable row identity/bytes/hash on the operation's transaction. Do not acquire
  a second runtime permit or reuse a separate-transaction badge.
- Separate creation capability/readiness on that same connection/transaction:
  verify retained approval integrity, withdrawal/current snapshot using the
  admitted pin. Expected missing source/page/unit/finalization is an explicit
  evidence-unavailable warning for existing sheets/history and blocks creation;
  stale/withdrawn approval similarly warns without blocking structural editing.
  Distinguish absent evidence from present corrupted metadata or immutable-content
  corruption, which remain errors. History reconstructs only its immutable pin,
  independent of current evidence availability and without private source links.
- Full sheet/history reads and editor rendering acquire shared runtime admission
  before full payload retrieval/validator work, not only POSTs. SQL projects NULL
  on over-cap bodies before allocating strings. Coherent authorized raw snapshot/
  pin retrieval hands the permit into detached blocking validation; cancellation
  cannot release it early. Context-aware bounded decoder profiles distinguish
  sheet envelope metadata from data IDs:256 fields named `label`, `title`, `help`
  use value limits, not definition-metadata limits. Preserve the old definition
  guard's32-key/256KiB limits and use maintained Serde for tokenization. Retain
  exact256-field, reserved-looking-ID, stale/withdrawn/missing-evidence history/edit,
  oversized/corrupt DB payload and cancelled-reader tests before completion.

## Private rendering and verification

Private full-page templates use campaign/roster/private-sheet view models, scoped
actions/CSRF and no global source navigation. Shared native field/calculation
partials may be extracted once from current templates. Preserve escaped raw drafts
and their old revision on409/422 using a private-editor response marker only after
a fresh authorized read; auth/middleware/body/SQL errors remain static and no-store.
Successful edit redirects to its private sheet with new revision/history. Display
Master/custodian/frozen/pin/readiness clearly, with history links and useful empty
states. Use bundled CSS and regular forms; adapt mobile/long-text layout.

Retain failed-before-fix regressions for every reproduced issue. Acceptance matrix:
cross-campaign/other-player/Owner/legacy ID privacy, role/rejoin/creator guards,
session/account/role/departure races, full no-op/stale edits, immutable pins/hash
vectors, history reconstruction/overflow/SQL failure, atomic32-sheet freeze,
runtime readiness races/stale pinned edits, malformed/duplicate/oversize/large-i64/
null forms, bounded cancellation/SQL work and protected draft preservation.
Fresh independent implementation/fix/docs review before completion; strict full
CI/audit/notices/release/source allowlist, actual unpacked/browser at both sizes,
restart/private persistence/isolation, and delivered-source network-disabled empty-
cache rebuild required. No real accounts/manuals/owner data used by verification.
