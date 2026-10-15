# Stage B — reviewed HTTP session integration proposal

Status: amended plan independently approved, implemented and fix-reviewed;
checkpoint artifact gates passed; handoff-inclusive archives checked separately.
Scope remains
Linux loopback HTTP at127.0.0.1:3000. Source/connected-request evidence is in
`auth-session-fit.md`; schema10 accounts are verified in `accounts.md`.

## Library selection and domain boundary

Select released `tower-sessions=0.15.0`, defaults disabled, `axum-core` enabled,
without MemoryStore, signed/private cookie or background deletion-task features.
Use its Session/SessionManagerLayer/Id/Record/cookie/rotation/expiry lifecycle.
Do not select unfixed sessions0.14 or unreleased Git axum-login. Mimik supplies
only typed account-ID/epoch binding and current-state reconstruction against its
existing RustCrypto account verifier; this explicitly revises the prior plan's
blanket 'authentication remains library-owned' statement. No custom session ID,
cookie parser, password codec or crypto protocol is introduced.

Published SQLite stores demonstrably conflict with current bundled SQLite, so
implement the library SessionStore trait over existing Store/rusqlite. Generic
serialization uses serde_json, times use the library's time types. Add minimal
`async-trait`, `time` serde support and `subtle` constant-time equality only as
needed by those APIs. Evaluate maintained `governor=0.10.4` (MIT, release2026-09-05,
Rust GCRA limiter, defaults off/std only) for nonqueued direct global throttling;
no bespoke rate-limiter infrastructure. Actual combined feature/license/audit and
cost gates remain mandatory; no second SQLite stack or keyed/forwarded-IP map.
Sources: https://docs.rs/governor/0.10.4/governor/ and its published manifest.

## Explicit modes and route authority

Default `mimik` preserves the clearly documented local unauthenticated prototype.
Opt-in `mimik --auth` starts a distinct router on the same fixed loopback listener,
requiring an existing durable Owner binding (disabled status need not prevent
enabled players logging in). --auth with operator subcommands is rejected.
Authenticated-mode routes: GET/POST `/auth/login`, POST `/auth/logout`, GET
`/auth/session`, root redirect to `/auth/session`, and explicitly shared bundled
CSS/logo assets. All gameplay/source/image/context/export/history/other paths
deny403, including for Owner, until campaign resource authorization exists.
No anonymous editor fallback in auth mode, no implicit legacy assignment, no LAN
listen/TLS/proxy feature. Prototype and auth routers cannot run together on3000.

Reuse exact authority checks (one Host:127.0.0.1:3000 orlocalhost:3000). Auth POST
requires one Origin exactly `http://<matching Host>`; no wildcard, cross-host
localhost equivalence or forwarded-header trust. Only urlencoded forms accepted,
body cap8KiB. Errors return static messages; never log cookies/tokens/passwords,
echo password inputs, redirect to supplied URLs or expose verifier Debug values.
All auth/error responses Cache-Control:no-store; password field autocomplete
current-password, escaped display labels, regular full-page forms (no htmx need).

## Login, CSRF and revocation

Cookie is library-owned opaque ID, HttpOnly, SameSite:Strict, path/, no Domain,
Secure:false solely for supported loopback HTTP. Name fixed `mimik_session`.
Anonymous CSRF sessions expire absolutely after10minutes; authenticated sessions
absolutely after8hours, no sliding extension. Handlers set library AtDateTime
expiry using integer Unix seconds; no cookie key or signed/private claim.

Session data closed shape: required `csrf`64lowercase hex representing32 OS-random
bytes; optional typed `principal={id,epoch}` positive exact integers, no roles or
verifiers/custody/grants. CSRF compares using subtle; tokens stay in form bodies
and protected browser pages, never query strings. GET login generates/saves a
token only when absent; successful login ALWAYS cycles ID (including account
switch), clears prior data, sets new principal/CSRF/absolute expiry and explicitly
saves before303. Logout requires current session CSRF+Origin and flushes/revokes
before redirect. Anonymous/wrong/disabled credentials have uniform401 message.
Input buffers use Zeroizing at the existing account verifier boundary; bounded
form decoding may temporarily allocate copies, no claim of complete memory erasure.

Add Accounts.authenticate(login,Zeroizing<Vec<u8>>) -> optional secret-free current
principal and Accounts.current(id,epoch). Reuse exact password/PHC admission and
process-global KDF permit outside DB locks through final state check. Unknown or
disabled login does bounded verification against a public dummy valid PHC profile
to avoid deliberately skipping password work; generic invalid-password401 has no
constant-time guarantee. Stored-integrity/storage errors500, global KDF busy503.
Direct governor quota: shared-process burst10, replenish1request/6seconds, no
queue,429+Retry-After. Apply before KDF and before new anonymous-session creation;
known-session readonly status requests do not mutate or consume KDF allowance.
Quota is aggregate local-process control, not distributed/per-user protection.

Only store ID/epoch. Every identity reconstruction uses current enabled account
state; stale/disabled/missing bindings are flushed, never silently authenticated.
Session create/save rechecks principal state/epoch in the same SQLite transaction
as persistence. Reset/disable committed before that boundary cannot create a
valid session; later changes defeat next reconstruction, not already returned
responses. No gameplay writes exist in this router. Future protected mutations
must revalidate accounts/grants inside their domain transaction.

## Persistence, anti-resurrection and bounded cleanup

Schema11 STRICT sessions table: library ID string, issued_at, immutable expires_at,
active/revoked state, <=4KiB serialized closed data payload; optional account/epoch
projection with FK and matched payload identity. Rows validate type/identity/expiry.
Adapter rejects oversized payload before serialization expansion; only two keys
and bounded scalar strings/integers allowed. Payload encoding via maintained
serde is domain storage, not a cookie/session-ID wire protocol. On load parse ID
with library, reconstruct Record with library time; match column ID/expiry and
shape. Corrupt oversized/invalid records and storage errors fail closed500, never
treated as absent authentication. Expected expiry/revocation returns None.

Create is INSERT-only (including tombstone collisions), with at most8 retries
using library Id::default on collisions, under BEGIN IMMEDIATE. Total retained
rows cap10,000; saturation fails closed, no unbounded queue. Save is UPDATE-only:
existing active, not expired, same immutable deadline and unchanged principal/
CSRF binding; it cannot insert missing/deleted/revoked IDs or change identity.
Fixed session data means the library's known cross-request lost-update limitation
does not carry authority/state mutations. Login changes ALWAYS get a new ID.

Delete soft-revokes and clears payload; retain tombstone through original absolute
expiry. Repeated delete is idempotent. A request that loaded before logout/rotation
cannot save the old ID afterward: UPDATE rejects revoked/missing, and even after
cleanup its captured deadline has expired. Old cookie replay is absent. SQL
guards reject replacement/identity/deadline change/revoked revival; deletion only
for expired rows in bounded cleanup. No physical deletion of account history.

Cleanup is supervised every60seconds, <=100 expired rows/batch, and optionally
one bounded batch before create capacity checks. An observed task error ends the
auth service rather than being discarded. Use existing Tokio task/signal boundary;
no cleanup task started by test Router constructors unless explicitly requested.
Now supplied by trusted system clock, injectable for tests, equality expires.
Clock failures/overflow are errors; no dependency on ignored owner files.

## Required library-lifecycle and SQL admission amendments

- Fixed deadlines: library request Session configuration does not inherit a
  loaded Record deadline. Only new anonymous sessions and successful login/new
  IDs set AtDateTime and explicitly save. Existing login-form reads, failed
  login and valid status reads neither set expiry nor mutate/save. Logout and
  invalid reconstruction flush only. Adapter save accepts the unchanged stored
  deadline; an arbitrary loaded Session save proposing layer expiry is rejected,
  not extended. Reload/save, repeated reads, failed login, restart and in-flight
  expiry tests assert this distinction and fixed cookie expiry.
- Explicit save leaves the library modified flag set, so response middleware may
  save again. Repeated unchanged saves are idempotent while current/notexpired;
  second-save failure or intervening account/logout revocation produces an error,
  not a successful login response, and does not undo the first commit. An outer
  response normalization layer covers all inner middleware/extractor/authority/
  body errors, replaces error bodies with static status-specific text, removes
  Set-Cookie on errors, and applies no-store/nosniff headers on every response.
  Store errors passed to the library are static and secret-free because the
  library can log them. Inject second-save failure/revocation independently from
  initial create and assert response/cookie/old-ID behavior.
- Register a nondeterministic `mimik_auth_now()` SQLite function on every Store-
  initialized connection using rusqlite's maintained functions feature. System
  UTC seconds must be1..253402300799; clock failure/overflow raises an error,
  never zero/NULL. Adapter and SQL guards read that same function. Tests replace
  it with a shared injected clock on their synthetic connection. Raw connections
  without registration cannot perform guarded transitions and fail closed.
  Record deadlines must have zero subsecond component. Issued/deadline columns
  are integer/bounded/immutable; deadline>issued, anonymous lifetime<=600seconds,
  authenticated<=28800seconds. Inserts require issued<=now<deadline; expired is
  deadline<=now. Missing function/clock failure, equality and fractional values
  have permanent tests.
- Active payload is a closed JSON object containing csrf and optional principal;
  optional account_id/epoch columns both absent or positive and match the payload.
  Revoked rows retain immutable ID/timestamps/projections, with NULL payload.
  Active saves cannot change payload/binding/deadline; only active→revoked clears
  payload. INSERT OR REPLACE/collisions, ID/deadline change or revival abort. SQL
  expired-only DELETE uses the same clock function, and an expiry/ID index bounds
  ordered cleanup to100rows. Capacity and one cleanup batch are inside the same
  immediate transaction as creation. Collision semantics: at most8 INSERT
  attempts total; subsequent candidate IDs are library-generated. All failures
  roll back, exact affected-row checks distinguish missing/revoked saves.
- Concurrency admission is separate from governor rate quota. Auth router accepts
  at most8 in-flight requests nonqueued before body/middleware work; timeout5s
  covers body read and handler response work. Each session/identity SQL operation
  independently acquires a process-global16-worker permit before Store scheduling;
  that owned permit moves into the blocking closure and survives cancellation
  until work completes. Add a bounded Store auth execution boundary reusable by
  identity reconstruction/session adapter. Accounts authentication acquires its
  existing KDF permit before preflight and retains ownership through detached
  auth SQL/KDF/final recheck. SQL retains the existing5s busy timeout. No unbounded
  auth queue, but detached work may finish an atomic commit after HTTP timeout;
  no successful cookie response is claimed then. Governor std feature includes
  futures/timer/parking_lot dependencies despite direct nonwaiting API use.
  Flood known/unknown IDs under held DB locks, slow bodies and cancellation;
  assert16-worker bound,8-request bound, static503/408 and recovery. Observe
  supervised cleanup error/shutdown; never ignore task results.
- Label prototype startup explicitly 'unauthenticated prototype — not secured by
  account setup'. Auth-mode startup is separately labeled; no claim of campaign
  protection or LAN support from successful login.

## Permanent verification and release gates

Retain migration10→11/reopen/rollback/collision, SQLite record codec bounds/expiry,
collision8-limit/capacity, immutable binding and cleanup checks. Extend synthetic
historical fixtures only after asserting new session table empty; never discard
user sessions to rewind normal databases. Test actual connected Router login,
rotation/account switch/logout/replay, disable/reset-issuance/reset-completion and
missing-account reconstruction, concurrent save/logout/rotation/delete/recreate,
unknown/hostile PHC pre-KDF integrity, KDF admission cancellation, and account
change between verify/save. Inject store load/save/delete errors: static500 and
no claimed valid login on failed explicit persistence. Failed response cannot
undo previously committed account changes; revoked old ID stays revoked.

Test authority/Origin/CSRF/type/size/duplicate fields/escaping/no password echo,
quota nonqueue with fake clock, all unaffiliated gameplay routes denied for Owner
and player in auth mode, prototype behavior retained by existing suite. Every
reproducible issue has permanent failing-first regression. Fresh independent
implementation/fix review required before completion.

Actual unpacked process/browser checks must exercise persistent cookie login,
restart and subsequent epoch revocation/logout/replay/rotation plus blocked source
and character paths at both supported viewports; existing prototype/native/source/
publication/account CLI checks remain. Full strict checks, audit/notices, resource
samples, source allowlist, archive equality/checksums and network-disabled empty-
cache delivered-source build required. Network sharing remains uncertified.
