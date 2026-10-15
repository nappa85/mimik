# LAN/proxy login and persistent sessions

Implemented: an explicit local HTTP authentication mode using released
`tower-sessions0.15`, existing RustCrypto account verification and a reviewed
SQLite SessionStore adapter. Schema11 retains bounded session records and
revocation tombstones. The authorized campaign/private-sheet workspace is now
implemented separately in `campaign-workspace.md`.

## Run the appropriate mode

First bootstrap/enroll local accounts using the protected-file CLI in `accounts.md`.
No real Owner account or credential is created by development verification.

```sh
./mimik
```

Authentication/LAN are defaults; the database defaults to `data/mimik.sqlite3`.
Open the LAN address printed at startup (localhost still works on the server).
`--loopback` is explicit development-only serving; `--lan-address` selects a local
private interface. `--external-url` / `MIMIK_EXTERNAL_URL` configures the canonical
HTTP(S) proxy origin for public links/QR. No separate internal_url is needed.
See `deployment.md` for straightforward startup and proxy configuration. A durable
Owner binding must exist; it need not be enabled for other enabled users to log
in. Account CLI commands continue to use local OS/database authority; explicit serving
options cannot be combined with those subcommands. Owner lands at `/admin` for account
listing/invite/reissue/player-reset/disable and is admin-only; enroll a separate
player account for gameplay. Invitation/reset links and QR codes prefill `/auth/enroll`
or `/auth/reset` from a fragment, remove it from the address and require explicit
password submission; no automatic sign-in or GET consumption.
`--dev-owner-play` explicitly enables development-only Owner gameplay under
existing grants, with a visible warning; normal startup excludes Owner again.
See `accounts.md` for the browser workflow, current recovery limitations and
authenticated audit boundary. Owner reset issuance and disablement remain CLI.

Opening the root address now sends anonymous/expired sessions to sign-in and
valid sessions to their session/workspace entry. Protected browser GET bookmarks
also return to sign-in on401. Explicit document Fetch Metadata plus acceptable
HTML selects this presentation; an HTML Accept-only fallback supports older clients
without Fetch Metadata but cannot distinguish all fetch requests. JSON/wildcard/
explicit fetch requests and failed POSTs retain status errors. This does not change
authorization, redirect403/500, or reflect a return URL.

**`--no-auth`** selects the unauthenticated local editor prototype; normal startup
is authenticated. The prototype startup says account setup does not secure it.
Both modes use port3000 and cannot run together on overlapping listeners.
HTTP auth mode exposes sign-in/out/session and authorized campaign/private-sheet/
history routes, Owner-only `/admin/materials` inspection/review/image routes and bundled assets. Legacy character/global source/image/context and
unsupported export paths remain denied, including to Owner. It never falls back
to the anonymous editor or implicitly
assigns old sheets. Authenticated trusted IPv4 LAN and configured root-origin proxies
are supported; the anonymous prototype remains loopback-only. The proxy governs
original-client LAN admission and preserves external Host; forwarded headers confer
no authority. Native TLS termination is provided by the proxy, not this binary.

## Browser/session contract

- Forms use exact Host authority and mandatory matching Origin for POST, session-
  bound CSRF and URL-encoded input. Auth/campaign-control forms have an8KiB body
  cap; private-sheet submissions allow1MiB with independent canonical limits.
  Forwarded headers confer no
  trust. Passwords are masked, not echoed in pages/errors/logs. All auth responses
   are no-store/nosniff with anti-framing CSP/XFO and same-origin referrer policy;
   browser document errors use a branded status page; explicit API/fetch errors retain
   static text/statuses. Native sign-in errors stay in the form with an accessible
   alert, bounded/escaped login name and cleared password. Unknown/disabled/wrong
   credentials share one message. Login names ignore surrounding spaces/ASCII case;
   password bytes stay exact. Expired/revoked forms offer an actual fresh-form GET,
   and captured durable admission is rechecked around password verification.
   Trusted credential retry pages retain only the
   supplied valid-shaped token/current form, clear passwords and emit no session cookie.
- Library cookie `mimik_session` is an opaque generated ID with HttpOnly,
  SameSite:Strict, path/, no Domain. Cookies are Secure for the configured HTTPS
  authority (including rotation/deletion), and usable on direct trusted-LAN HTTP.
- Anonymous CSRF records have an absolute10minute deadline; authenticated records
  have an absolute8hour deadline. Status reads and failed login do not extend
  expiry, mutate records or refresh cookies. Equality at expiry is expired.
- Successful login always rotates ID, including switching accounts. Principal
  data stores only current account ID/epoch, not cached roles or sheet access.
  Logout revokes that session; old-ID replay and late saves cannot revive it.
- Reset issuance/completion and disablement advance account epochs. Existing
  cookies fail current-state reconstruction after those changes; no account
  re-enable or automatic gameplay custody change is introduced.

Global limiter quota is burst10 with one slot replenished per6seconds. New
anonymous sessions and login attempts consume it; existing read-only session
status does not.429 includes Retry-After:6. One process-global KDF worker, eight
nonqueued in-flight HTTP requests and sixteen cancellation-retained auth SQL
workers bound admission separately; busy responses503. Five-second timeout covers
body/response work (408), while SQLite retains its five-second busy timeout.
This is aggregate process control, not distributed/per-user rate protection.

HTTP decoding can allocate bounded temporary password copies; the account verifier
uses Zeroizing ownership and Argon2 zeroization. Do not infer complete erasure
from swap, browser/editor allocations or backups. Unknown/disabled accounts use
bounded dummy-profile verification, with the same generic401 response as wrong
passwords; no constant-time timing guarantee is claimed.

## Persistence and admitted-read boundary

The adapter uses existing bundled rusqlite, not a second database stack. It admits
only closed CSRF/principal payloads (max4KiB), library-parsed IDs and bounded
integer-second deadlines. SQL clock `mimik_auth_now()` is registered on every
Store connection, nondeterministic and fail-closed. Raw connections lacking it
cannot perform guarded transitions. Corrupt payloads/profile/clock/storage
failures are errors, never silently authenticated success; even explicit-null
principal is rejected rather than normalized to anonymous state.

Create is INSERT-only with at most8 library-generated collision attempts. Save
is UPDATE-only on an active, unexpired, identical binding/deadline and current
enabled account epoch. Delete revokes and clears payload, retaining its tombstone
until original expiry. Expired-only physical cleanup removes at most100 indexed
rows/batch, with a total retained-row cap10,000. Cleanup is observed every60s;
failure terminates auth serving rather than being discarded.

Clock is sampled after potentially blocking record retrieval. Identity reads then
recheck session availability/deadline after the account query, retaining the
original request-admitted ID/deadline so same-ID recreation cannot substitute a
fresh deadline. Account/session changes after that admitted-read boundary cannot
retroactively retract an already returned response. Implemented campaign/private-
sheet mutations revalidate accounts, captured sessions, grants and revisions
inside their domain transaction.

Explicit login persistence can be followed by a second middleware save. Repeated
unchanged saves are idempotent; intervening failure/revocation returns a static
error with no successful login cookie response. It does not roll back the first
commit: an unreachable record may persist until expiry cleanup. Similarly,
detached bounded SQL/KDF work may complete atomically after request timeout or
cancellation; no successful response is promised afterward.

## Library selection and verification scope

Published axum-login0.18 requires sessions0.14, which lacks the exact upstream
#254 mutation-before-release-store fix. The released0.15 session abstraction was
selected over an unreleased Git auth API. Its ID/cookie/rotation/expiry protocol
remains library-owned; RustCrypto owns password verification; Mimik supplies
account/epoch binding/reconstruction and the narrow persistence adapter. Published
SQLite stores have demonstrated native-link conflicts with Mimik's current SQLite.
Generic rate limiting uses maintained governor0.10.4; std enables futures/timer/
parking-lot dependencies despite the direct nonwaiting API. Sources/probes are
in `auth-session-fit.md`, plan in `http-auth-plan.md`.

`auth-test-support` exposes deterministic test rendezvous only when explicitly
enabled; default release excludes it. Required CI includes all-feature tests and
default release builds, packaged authenticated process checks and both Chromium
viewports. Expiry/corruption/race/resource-admission/fault interleavings are
unit/connected-router evidence, distinct from real-browser and process evidence.
Black-box checks disclose that controlled503 contention/clock races are not
exercised there; conditional429 retry only proves live refill if encountered.

Integrated release/request resource samples and exact graph:
`http-auth-measurements.md`. Review findings/dispositions and artifact verification:
`reviews/2026-10-13-http-auth.md`, `release-scope.md`, `../TODO.md`. These checks do
not approve real manuals/providers, campaign access, LAN or whole-product readiness.
