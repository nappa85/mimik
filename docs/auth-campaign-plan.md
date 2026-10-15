# Account and campaign delivery plan

Status: Stage-A local operator foundation independently approved, implemented and
checkpoint-verified. Handoff-inclusive archives are checked separately. StageB
HTTP sessions are implemented/fix-reviewed and checkpoint artifact-verified;
StageC workspace/sheets/history is implemented and independently approved, with
final artifacts being checked. Deferred lifecycle operations remain future work. Confirmed
gameplay policies are in `collaboration-and-history.md`; library evidence is in
`auth-library-fit.md`. This is staged implementation, not network-sharing approval.

## Stage A — local operator account foundation

Use existing rusqlite storage/migrations and maintained RustCrypto `argon2=0.6.0`
with alloc/password-hash/getrandom/zeroize (no Rayon), `getrandom=0.4.3` and
`zeroize=1.9.1`. Verify the actual integrated locked features, licenses, audit,
resource profile and artifact gates. Only crypto dependencies enter this stage.
The session libraries remain research candidates: published axum-login0.18 and
sessions0.15 are incompatible, and published SQLite stores conflict with current
SQLite. No bespoke session protocol substitutes for that unresolved boundary.

The CLI is trusted local OS/database operator authority, not authenticated web
Owner authority. User-supplied IDs/names never authenticate the caller. Bootstrap
creates one initial Owner account and a durable singleton administrative binding;
concurrent bootstrap has one winner. No rebootstrap, Owner deletion/succession,
account re-enable or web administrative endpoint is exposed in this stage.
Owner administration is separate from all gameplay grants.

Schema10 domain tables: accounts (immutable ID/login, PHC verifier, enabled flag,
positive monotonic epoch), singleton Owner binding, purpose-bound one-time invite/
reset credentials (only SHA-256 digest of 32 OS-random bytes), and non-secret
append-only operator account events. Keep bounded ASCII login IDs (3–64 bytes,
lowercase letters/digits/hyphen/underscore, first alphanumeric) separate from
display names. Credential lifetime is fixed/bounded; timestamp checks use trusted
clock, `now < expires_at` (equality expires). No email delivery/provider is needed.

Password input uses protected files; never command-line password/token values.
Retain at least 15 Unicode scalar passwords, cap at 128 scalars/512 UTF-8 bytes,
reject invalid UTF-8/control characters, preserve exact bytes (no trimming or
normalization). Argon2id v19, 19,456 KiB memory, t2, p1, 32-byte output and random
16-byte salt; use PHC serialization/verification provided by RustCrypto. Before
verification, accept only the exact supported algorithm/version/profile/salt/hash
lengths; reject hostile PHC costs before expensive work. One nonqueued password
worker per process, permit retained through cancelled blocking workers and DB
handoff. Expensive work happens outside the connection mutex/write transaction.
Password buffers use Zeroizing; errors/output never contain inputs or verifiers.

Credential output uses a new exclusive regular file with mode0600, not stdout or
an unusable enrollment URL. Input descriptors are bounded regular files opened
NOFOLLOW/NONBLOCK, owned by the effective user and with no group/other permissions.
Caller must choose a trusted private destination directory. Creation rejects
symlink/collision; file data is written/flushed/synced before database commit.
On write failure, no credential issuance/epoch mutation commits. A crash or DB
failure can leave an uncommitted file; it is not redeemable and explicit reissue
to a new path is the recovery procedure. Never overwrite or silently reuse it.
Audit does not store output paths, raw secrets or verifiers.

### Concrete transition and admission contract

- Reserve logins in the accounts table itself: `pending`, `enabled`, `disabled`
  states, immutable login/ID, unique login across reserved and enrolled accounts.
  Pending has no verifier; enabled/disabled have the bounded verifier. An expired
  invite does not release its reservation. Reissue for an existing pending login
  advances its epoch and revokes previous active invites; enrolled logins cannot
  receive enrollment tokens. Initial epoch is1; enablement advances once.
- Bootstrap requires absent durable Owner binding; account/token/event mutations
  use `BEGIN IMMEDIATE`, conditional exact-row checks and one transaction. Owner
  binding references an enabled account at insertion and cannot be deleted or
  rebound; accounts cannot be deleted or change ID/login. No reset/disable
  operation confers an administrative role. Disable is irreversible in this
  stage. Every account state/verifier change advances integer epoch exactly once;
  checked i64 overflow fails atomically, never SQLite REAL promotion.
- Output credential is exactly64 lowercase ASCII hexadecimal bytes encoding32
  OS-random bytes, with no newline. DB digest is SHA-256 of the decoded32 bytes in
  a32-byte BLOB. Token purpose `enroll`/`reset` binds account ID and issued epoch;
  only an active token with matching current epoch/state can redeem. All tokens
  are active→consumed/revoked only, immutable purpose/digest/target/epoch/expiry;
  reissue revokes old active tokens in the same transaction. Invite and reset
  lifetimes are respectively24h and1h. Times are UTC Unix seconds; checked
  arithmetic, positive bounded times, `issued_at <= now < expires_at`, exactly
  prescribed purpose lifetime. Account IDs/types/FKs and token digest uniqueness
  are SQL-enforced. Retain append-only events with explicit `local_operator` or
  `credential_redemption` actor category, never claimed authenticated web actors.
- PHC admission bounds bytes before parsing: require the97-byte ASCII canonical
  RustCrypto-generated form for Argon2id/v19 with parameters exactly once in
  `m=19456,t=2,p=1` order, no missing/duplicate/additional keyid/data parameters,
  decoded16-byte salt and32-byte hash. Use maintained PHC parsing/serialization to
  validate these properties before invoking the verifier. Malformed/profile-
  unsupported hashes fail as stored-integrity errors, not password mismatches.
- Protected output creation pins the trusted destination directory via descriptor
  and uses relative basename openat with EXCL/NOFOLLOW/CLOEXEC, mode0600. Verify
  file descriptor owner/type and set exact0600 despite restrictive umask; input
  descriptors must be effective-user-owned regular files with no group/other
  permissions. Output parent must be effective-user-owned private directory (no
  group/other permissions); caller controls its ancestor path. Write/file sync
  **and directory sync** finish before credential/epoch/event DB commit. File and
  directory sync failure injection must prove unchanged DB state. Supported
  durability boundary is Linux local filesystems supporting file/directory fsync;
  tests establish control flow, not physical power-loss guarantees. No automatic
  orphan deletion by a racy pathname; failed/uncommitted files are harmless and
  operator recovery is explicit issuance to a new private path.
- Password work admission is process-global; across processes SQL guards provide
  correctness but do not cap aggregate KDF memory. Coherent final verification
  recheck rejects reset/disable completed before that boundary; later changes
  cannot retroactively retract a returned result. Reset issuance revokes epochs
  and outstanding tokens but does not itself change the existing password.

Operations: bootstrap Owner; issue enrollment credential bound to reserved login;
redeem enrollment with chosen password; issue password-reset credential for an
enabled account; redeem reset; disable account; show/list nonsecret summaries;
verify password against current account state as a domain primitive, not login.
Enrollment grants no Owner/campaign role. Reset issuance increments epoch and
supersedes resets atomically; completion consumes token, replaces verifier,
increments epoch again and invalidates resets. Disable increments epoch and
revokes resets; reset redemption cannot re-enable. Expected epoch checks guard
operator reset/disable races. After hashing, commit rechecks clock, account state,
epoch, token purpose/digest/unused/expiry. Independent-process redemption has
one winner. Auth result is rechecked after verification, so concurrent disable/
reset cannot return successful old credentials. Epochs are revocation primitives,
not proof of middleware session invalidation yet.

Retain SQL identity/epoch/Owner/event invariants and secret-free projections.
Add regression tests for bootstrap and one-use races, wrong-purpose/expired/
superseded/disabled tokens, changes during hashing, filesystem failures/collisions,
input/admission/PHC bounds, migration rollback/reopen and secret-free CLI. For
every reproducible issue, demonstrate failure before fixing. Independently review
each material implementation/fix set and resolve blockers before completion.

## Stage B — supported session/auth middleware

Source/connected probes select maintained released tower-sessions0.15, including
the memory-ordering fix, over unfixed0.14/unreleased Git auth APIs. Published store
conflicts justify a narrow SessionStore persistence adapter over existing Store;
session IDs/cookie/lifecycle/rotation/expiry remain library-owned, passwords remain
RustCrypto-owned, and Mimik supplies typed account/epoch binding/reconstruction.
This explicitly revises the earlier blanket authentication-library ownership
statement; concrete integration plan `http-auth-plan.md` received amended-plan GO.
Bound stored records, enforce expiry/collision-safe create/observed cleanup and
surface storage errors. Read current enabled status and epoch on every request;
recheck them transactionally on protected mutations. Rotate IDs explicitly on
login/account changes; configure HttpOnly/SameSite/expiry and HTTP/TLS cookie modes
and supported origins. Test storage-save failures and concurrent revocation.

## Stage C — campaign authorization and generic editing

Campaign binds exact registered ruleset; creator is original Master and only
creator promotes/demotes additional Masters. Membership is request/accept/decline.
Separate technical Owner from current campaign grants and personal custody. Sheet
access checks apply to all routes, lists, images/context/export/history and writes.
Departure/removal freezes sheets while retaining exact campaign linkage and
player/current-Master read rights; no former-role historical grants or implicit
reassignment/reactivation. Transferring personal custody requires owner's action;
effects on frozen sheets/reactivation require a later explicit policy.

Generic editor pins immutable matching definition versions and requires reviewed
readiness, with deterministic server-side shape/value constraints and revisions.
Publication is not game legality. Every mutation needs transactional authenticated
history; native and generic paths share the same authorization boundary. Legacy
unassigned records are not silently made shared. Unfinished history/delete/purge
policies remain deferred; do not expose operations whose permission/retention
contracts are unresolved.

## Release gates

Stage A can be released as a loopback local operator capability only, clearly
separate from the current unauthenticated web prototype. Extend unpacked artifact
process smoke for protected-file enrollment/reset/disable/restart and non-secret
output, while keeping existing native/source/publication/browser scenarios.
Strict fmt/Clippy, locked tests, helpers, audit/notices, release/source allowlist,
archive equality/checksums and delivered-source network-disabled empty-cache
rebuild remain required. Record actual helper versus process/browser verification.
Network sharing additionally requires staged session/campaign/history/lifecycle
gates in `collaboration-and-history.md`; no new listening address in Stage A.
