# Accounts, sessions and serving

Mimik serves an authenticated campaign workspace by default. Accounts are created by
the Owner (no open sign-up); the Owner administers and players play. Campaign roles
and sheet access are in `campaigns.md`.

Code: `src/main.rs` (flags), `src/account_cli.rs` (account commands), `src/accounts/`
(domain, crypto, protected files, browser helpers), `src/auth/` (router, sessions,
administration, materials, rulesets, error pages), `src/network.rs` (LAN/proxy
admission), `migrations/010`, `011`, `013`, `014`.

## Starting the server

```sh
./mimik                                   # authenticated, LAN, port 3000
./mimik --lan-address 192.168.1.20        # pick the LAN interface explicitly
./mimik --loopback                        # authenticated, 127.0.0.1 only (development)
./mimik --external-url https://mimik.example   # canonical origin behind a proxy
./mimik --no-auth                         # unauthenticated prototype, loopback only
```

- **Database:** `--database PATH`, else `MIMIK_DATABASE`, else `data/mimik.sqlite3`
  relative to the working directory. The parent directory is created if missing. Use
  the same setting for every command. `--database` is global (works with subcommands).
- **Owner binding required.** Authenticated startup refuses to serve until an Owner
  has been bootstrapped, and prints the `bootstrap-owner` command to run, naming the
  database by its absolute path. The check runs right after the database opens and
  before the assistant (the model server) starts, so a refused launch starts nothing.
  Every error Mimik raises itself (startup or a subcommand) is printed to stderr as
  `Error: <message>` and exits with status 1; command-line argument errors are clap's own
  (exit status 2).
  The binding must exist; the Owner account need not be enabled for players to log in.
- **Serving flags vs subcommands:** `--no-auth`, `--loopback`,
  `--lan-address` and `--external-url` are rejected with any subcommand.
  `MIMIK_EXTERNAL_URL` is ignored by subcommands.
- **Conflicts:** `--loopback` with `--lan-address`; `--no-auth` with
  `--lan-address`, `--external-url` or `MIMIK_EXTERNAL_URL`.
- **Port:** always 3000. LAN mode binds `0.0.0.0:3000`; loopback and `--no-auth` bind
  `127.0.0.1:3000`. The two modes cannot run at once on overlapping listeners.

### `--no-auth`

Serves the old unauthenticated editor (`src/web.rs`, `/characters`, global source
routes) on loopback only, with a startup warning that account setup does not secure it.
It cannot reach campaign or private-sheet tables. Authenticated mode never falls back
to it: legacy character, global source/image and export routes return 403 in
authenticated mode, including for the Owner.

### Owner and gameplay

The Owner is administration-only: gameplay routes answer 403 and the SQL guards
(`campaign_request_auth` view, Owner triggers in migration 013) refuse Owner memberships
and supervision. Owner memberships and custody left in an old database stay but give no
access and are hidden from rosters. There is no flag that lifts this.

## Pages and menu

Every page of the authenticated app extends `templates/layout.html`: the logo (a link to
`/`), a menu and the account menu on the right. `GET /` is the home page for a signed-in
account (signed out it goes to the sign-in form); `/auth/session` redirects to `/`. The
menu entries depend on the role (`src/auth/nav.rs`, one `Nav` per request, built from the
account `principal` already loaded):

- **Play** (players): Campaigns, My sheets.
- **Materials** (Owner): Sources and review, Upload PDF, Rulesets.
- **Administration** (Owner): Accounts, Assistant.
- **Account** (everyone, labelled with the display name): Switch account (the sign-in
  form), Sign out (a POST form with the CSRF token).

The menus are `<details>` drop-downs: they open without a page load and without script,
under every page's CSP. `assets/nav.js` (same origin, no inline code) closes an open menu
on an outside click or Escape; the layout's `strict` flag leaves it out of sheet pages (also
their history revisions and catalogue pages) and owner material and upload pages, so the
script invariants of sheet pages hold. The current entry
carries `aria-current="page"`. Pages without a session (error documents, sign-in, enrolment
and reset, catalogue notices, the sheet-busy page, the non-Linux upload stub) keep a
logo-only header. On sheet pages `sheet-assist.js` holds back form submissions while saves are pending; the Sign out form is one of the page's forms, so signing out waits until pending saves finish.

## Home page

`GET /` (`src/auth/home.rs`) has quick links (the role's menu entries), a **Pending** panel
and a panel of what is **new**. Every section is its own query of at most 11 rows (10 shown,
a note and a link to the full list when there are more) inside one read transaction that
rechecks the session before and after. Each query is bounded by an index or an explicit
window: `account_events (action, occurred_at)`, `campaign_events (target_id, occurred_at)`
and `(campaign_id, occurred_at)` (one query per campaign the account runs, at most 128
exist), `source_suggestion_reports (imported_at)`, `source_pages (source_id, complete,
pdf_page)`, `account_credentials` (partial, active rows) and `campaign_requests (account_id,
state)`. "Open" suggestions (neither dismissed nor applied) cannot be indexed, so that block
looks at the newest 1,000 suggestions only and says when older ones may exist. The Owner's
home reads Owner tables only (no campaign table), through the Owner admission; a player's home
reads campaign tables only (no Owner table), through the request admission. The one
cross-reading is the guard `NOT EXISTS (account_owner)` that keeps an Owner membership left in
an old database out of the Master's join-request list, as the roster does. Nothing shows sheet
contents or another player's private data: counts, titles, links and times; a sheet is a link
by number (the snapshot is not read), at most three per campaign and then a link to the
campaign page.

- **Owner, pending:** sources with pages not complete (page-level counts, a link to the first
  incomplete page), uploads running or failed, open suggestions, unredeemed invitations,
  assistant model downloads confirmed but not finished.
- **Owner, new:** uploads finished or failed (`source_jobs.finished_at`), invitations
  redeemed (`account_events`), suggestion reports imported (`source_suggestion_reports`).
- **Player and Master, pending:** campaigns they run and play in, each with a link to the
  campaign and to their own active sheets there; their pending join requests; for a Master
  the requests to answer.
- **Player and Master, new:** campaign events whose target is the account, and for a Master
  the events of the campaigns they currently run; the account's own actions are left out.

The mark is a per-account time in `account_home_marks` (a table of its own: `accounts`
changes only with an epoch bump, which ends the account's other sessions). The new panel
lists the newest items after the mark, or the latest ones while there is no mark. "Mark as
seen" (`POST /home/seen`, CSRF) sends the newest time shown (`seen`) and the second the page
was rendered (`shown_at`). The server accepts both only if they are at most now and `seen` is
at most the newest new item the home shows at that moment (clamped to now, so a clock
difference between SQLite's timestamps and the auth clock cannot refuse a mark). It stores
`min(seen, shown_at - 1)`: the mark never reaches the render second, so an item arriving later
in that second stays new. A value at or below the mark changes nothing (two tabs), and a
trigger refuses any backwards or future value. The Owner's write uses the CSRF admission and
the in-transaction Owner recheck like the other Owner writes; a player's runs in the request
context. Older items beyond the 10 shown are marked with it.

## LAN and reverse-proxy deployment

**LAN selection** (`Network::discover`): enumerates private IPv4 interfaces with a
contiguous netmask (`if-addrs`) and prefers the one the kernel would route through
(UDP connect, no packet sent). If that is ambiguous, it uses the sole candidate or
requires `--lan-address` (which must match a local private interface). It never picks
by enumeration order. The startup line prints the address to open.

**Peer admission** (`network::admission`): before any session work, the socket peer
must be loopback or a private IPv4 address in the selected subnet; a missing peer
fails in LAN mode. Forwarded/X-Forwarded headers are never used for peer, host or
scheme.

**Host and Origin:** exactly one `Host`, which must be `127.0.0.1:3000`,
`localhost:3000`, the LAN `IP:3000`, or the configured external authority. Every POST
needs an `Origin` equal to the scheme and Host of that same request.

**External URL** (`--external-url` overrides `MIMIK_EXTERNAL_URL`): an HTTP(S) origin
with optional port; credentials, path prefix, query and fragment are rejected; origin
at most 320 bytes. An HTTPS external URL must use a different authority from the
direct HTTP listener. It is used for invitation/reset links, QR codes and the startup
address. Without it the LAN address is used. There is no `internal_url`.

**Proxy:** Mimik provides no TLS. A root-origin reverse proxy terminates TLS and
forwards to `http://127.0.0.1:3000` (or the LAN address), preserving the external Host
including any non-default port:

```nginx
location / {
    proxy_pass http://127.0.0.1:3000;
    proxy_set_header Host $http_host;
}
```

Requests on the HTTPS external authority get `Secure` session cookies (also on
rotation and deletion); direct LAN HTTP stays usable without `Secure`. The proxy itself
is the backend peer, so original-client network restrictions must be configured in
the proxy. For containers, put proxy and backend on the same private network or use
host networking. Subpath proxying is not supported.

Phone cannot connect: check same Wi-Fi, router client isolation, firewall on TCP 3000;
restart after the server's address changes.

## Owner bootstrap and account CLI

The CLI runs with local OS and database authority; passing an ID is not authentication.

```sh
./mimik bootstrap-owner --login owner --display-name "Owner" --password-file private/owner.password
./mimik invite-account --login player --display-name "Player" --token-output private/player.invite
./mimik enroll-account --token-file private/player.invite --password-file private/player.password
./mimik show-account --id ID
./mimik list-accounts [--after-id 0]
./mimik issue-account-reset --id ID --expected-epoch EPOCH --token-output private/player.reset
./mimik complete-account-reset --token-file private/player.reset --password-file private/new.password
./mimik verify-account-password --id ID --password-file private/new.password
./mimik disable-account --id ID --expected-epoch EPOCH
```

- **Bootstrap** creates the single permanent Owner binding; it cannot be repeated,
  even after the Owner is disabled. Concurrent bootstraps have one winner.
- **Protected input files** (`src/accounts/files.rs`): regular, owned by the effective
  user, no group/other bits, no final symlink. Content is byte-exact (no trailing
  newline).
- **Credential output files:** a new basename in an effective-user-owned private
  directory; created exclusively with mode 0600, containing 64 lowercase hex characters
  and no newline. File and directory are fsynced before the database commit. A crash
  can leave an orphan file that cannot redeem; reissue to a new path.
- `invite-account`, `issue-account-reset`, `disable-account`, `show-account` and
  `list-accounts` require an enabled Owner. Enrollment and reset completion do not, so
  issued invites stay redeemable if the Owner is later disabled.
- `verify-account-password` prints only `{"matched": true|false}`; it is not a login.
- Lists return at most 100 summaries after an exclusive ID cursor. Output is JSON and
  never contains passwords, verifiers or tokens.

## Account rules

| Item | Rule (code) |
| --- | --- |
| Login | 3–64 bytes of `a-z 0-9 - _`, first character a letter or digit; immutable |
| Display name | 1–100 non-control Unicode scalars |
| Owner password | 15–128 non-control Unicode scalars, ≤512 UTF-8 bytes |
| Player password | 12–128 non-control Unicode scalars, ≤512 UTF-8 bytes |
| Character classes | none required |
| Invitation lifetime | 24 h (86,400 s); reset 1 h (3,600 s); expiry instant counts as expired |
| Token | 32 OS-random bytes as 64 lowercase hex; only SHA-256 stored; one use |
| Hash | Argon2id v19, m=19456 KiB, t=2, p=1, 16-byte salt, 32-byte output (97-byte PHC) |

Password rules: `accounts/crypto.rs` `password_for`. The Owner minimum applies to any
account bound as Owner, including reset and login.

**States and epochs.** Accounts are `pending` (reserved by an invitation, no password),
`enabled` or `disabled`. Each state or password change increments the account `epoch`
exactly once (checked i64). Expired reservations are kept. Reissuing a pending login
increments its epoch and revokes earlier invitations. Enrollment enables the account and
grants no role.

**Resets.** Issuance increments the epoch (all sessions end) and supersedes older
tokens, but the old password still works for new logins until the reset is completed.
Completion replaces the password, increments the epoch again and consumes the token.
Neither re-enables a disabled account.

**Disable** is permanent: it increments the epoch, revokes credentials and ends
sessions. Personal sheets, custody and history are not changed.

**Hashing:** one process-wide job at a time (busy → 503), outside database locks.
Stored hashes must match the exact canonical PHC profile before verification (otherwise
an integrity error). Password buffers use `Zeroizing`.

## Browser administration

Owner-only, under `src/auth/administration.rs`, `materials.rs`, `rulesets.rs`:
- `/admin`: account directory (50 per page, next-page cursor), invite a new login,
  reissue a pending invitation, issue a password reset or disable an enabled
  **player**. Owner reset issuance and Owner disablement are CLI-only.
- Issuance shows one link (`/auth/enroll#token=…` or `/auth/reset#token=…`), a Copy
  button and a local QR code. The fragment token is never sent to the server; the page
  fills the masked field and strips it. Opening the link consumes nothing; the server
  keeps no retrievable token, so a lost response needs a reissue.
- `/auth/enroll`, `/auth/reset`: anonymous forms; password and exact confirmation;
  success redirects to sign-in (no automatic login). A CLI-issued Owner reset token can
  be completed here with the Owner minimum. Recoverable errors keep the token in the
  current page, clear passwords and offer a fresh-form link.
- `/admin/materials`: imported source inspection, page images, unit decisions and page
  completeness (review POST body limit 320 KiB). `/admin/rulesets`: ruleset and legacy
  alias registration.
- Browser actions add immutable sidecars (Owner ID, epoch, one-way session digest) to
  the domain event: `account_web_events`, `source_web_events`, `registry_web_events`.
  CLI events say `local_operator` or `credential_redemption`. No secrets in events.

Owner administration grants no campaign, sheet or history access.

## Sessions

Library: `tower-sessions` 0.15 (session ID, cookie, rotation, expiry) with a Mimik
`SessionStore` over the existing rusqlite store (`src/auth/persistence.rs`,
table `auth_sessions`, migration 011).

- **Cookie** `mimik_session`: opaque library ID, `HttpOnly`, `SameSite=Strict`,
  `Path=/`, no Domain; `Secure` only on the HTTPS external authority.
- **Lifetimes (absolute):** anonymous CSRF session 10 min (600 s); signed-in session
  8 h (28,800 s). Reads and failed logins do not extend them.
- **Login** canonicalizes the login (trim, ASCII lowercase); password bytes are exact.
  The CSRF session is rechecked before and after password verification. Success rotates
  the session ID (`cycle_id`), stores only account ID and epoch, and redirects to
  `/`, the home page.
- **Per request** the account is rebuilt from current state: enabled and same epoch, or
  the session is invalid. Any epoch change (reset, disable) revokes all its sessions.
- **Read-only admission** (`read_only_request_auth`, `src/auth/mod.rs`) for GET Master data
  paths (the snapshot probe and the event stream). It bypasses the library `Session` and
  `request_auth`, so it cannot create, rotate, flush, save or extend a session, or write a
  last-seen time or tombstone: it parses the single `mimik_session` cookie, reads the row with
  one `SELECT`: a missing or malformed cookie, an unknown, expired or revoked session or one
  without a principal is 401 with nothing written, a corrupt session row is 500 (503 when
  storage is busy), as `load` errors map. Two `mimik_session` cookies are refused with 401
  here, stricter than the normal path. It checks the
  account is enabled at the bound epoch (Owner: 403) and yields the usual `RequestAuth`
  whose deadline is the session's absolute expiry, so `recheck` applies unchanged. `Origin`,
  when present, must be the single expected origin; `Sec-Fetch-Site` must be absent,
  `same-origin` or `none` (`cross-site` and `same-site` are refused, 403); no CORS header is
  ever sent. POST rules are unchanged.
- **Event stream lifecycle** (`GET /campaigns/{id}/master/events`, `docs/campaigns.md` for the
  events). The *handshake* is an ordinary request: read-only admission, a current-Master check,
  the slot, the subscription and the first projection inside `boundary`'s 5 s, on `REQUESTS` and
  `AUTH_WORKERS` like the probe; the response then streams a lazy body, and the permit goes back as
  for any response. Nothing in `boundary` is exempted or lengthened for it. The *stream* holds no
  `REQUESTS` or `AUTH_WORKERS` permit, mutex or transaction while it waits; it owns a connection
  slot and one producer task, and borrows a permit of its own **2-permit FIFO stream-worker pool**
  (`src/streams.rs`) for one short operation at a time: the wait counts inside the operation's 1 s
  deadline, the worker keeps its permit until it exits even after a timeout (and skips its SQL if
  its deadline passed before it held the database mutex), and a busy or timed-out operation holds
  the output and retries every 250 ms (a stream can therefore briefly own both permits: the
  timed-out worker still running and its retry). Stream tasks run outside `boundary`'s task-locals, so busy
  is detected explicitly and the SQL guards get their request context through
  `with_domain_request`.
  - *Revalidation:* the `RequestAuth` captured at the handshake (its deadline is the session's
    absolute expiry, never extended) is rechecked in one read transaction (session `active` with
    the exact expiry, epoch, enabled account, Owner exclusion, current active Master role) at the
    start and end of every projection, once more after the commit and before the data is queued,
    every 5 s while idle and at the expiry instant. Failure by revocation (logout, CLI reset, disable,
    epoch change), expiry or demotion closes the stream with no notice and discards whatever was
    queued; nothing is emitted without a fresh successful recheck, and a stream with no success
    for 10 s closes. Rechecks run on the producer's own clock, independent of whether the body is
    polled, so a non-reading socket neither delays revocation nor keeps its slot (an output queue
    unread for 1 s closes the stream). Bytes already handed to the network cannot be retracted.
  - *Capacities:* 1 live stream per Master account (newest wins), 16 per campaign (429 +
    `Retry-After: 6`), 128 per instance (503), maximum age 15 min (the client reconnects with a full
    snapshot), SQL reconcile every 30 s. Shutdown (Ctrl-C or a cleanup failure) signals every stream
    to flush `closing` and end, and the process leaves after a 500 ms drain.
  - *Proxy notes:* the stream needs unbuffered, uncompressed delivery (the route sends
    `X-Accel-Buffering: no`; turn off response buffering and compression for the path) and a proxy
    read timeout above the 15 s heartbeat (for example 60 s). The proxy's `Host` and the Secure
    cookie rules are as for every route; forwarded headers are never trusted. Direct LAN HTTP,
    loopback and the root-origin HTTPS proxy are exercised in slice (c), not here.
- **Mutations** recheck session, deadline, epoch and grants inside their transaction.
- **Logout** revokes the record (state `revoked`, payload cleared); the tombstone stays
  until original expiry so an old ID cannot be revived.
- **CSRF:** 64-hex token bound to the session, required on every POST with the exact
  Origin check above.
- **Store bounds:** payload ≤4 KiB; at most 8 ID-collision retries on create; total
  retained rows ≤10,000; expired rows deleted 100 per batch every 60 s. Cleanup failure
  stops the server.

## Request limits and error pages

| Limit | Value |
| --- | --- |
| New anonymous sessions + login/redemption attempts | burst 10, one slot per 6 s (`governor`); 429 with `Retry-After: 6` |
| In-flight requests | 8, no queue (503) |
| Auth SQL workers | 16 (503 when busy) |
| Master event streams | 1 per Master, 16 per campaign (429, `Retry-After: 6`), 128 per instance (503); stream workers 2 (FIFO, 1 s per operation), separate from the auth SQL workers |
| Request timeout | 5 s (408); SQLite busy timeout 5 s |
| Body | 8 KiB for auth and campaign forms (sheet routes: `campaigns.md`) |

Process-wide, not per user. A timeout can follow a committed change: check state
before retrying.

All responses: `Cache-Control: no-store`, `nosniff`, CSP including `frame-ancestors 'none'`,
`X-Frame-Options: DENY`, `Referrer-Policy: same-origin` (`no-referrer` makes Chromium
send `Origin: null` on native form POSTs).
Sheet pages and sheet JSON answers use `SHEET_CSP`. Owner material pages, scripts, images,
successful POST redirects and retained 409/422 draft pages use the same strict policy.
Rebuilt refusals and the unauthenticated navigation's login redirect retain only
`frame-ancestors 'none'` (`docs/sources-and-retrieval.md`).

**Login errors** (`src/auth/errors.rs`, `templates/auth_login.html`): a failed browser
sign-in re-renders the form with an accessible alert, keeps the (bounded, escaped) login
name and clears the password. Unknown, disabled and wrong-password accounts share one
message. Expired/revoked forms, throttling (429) and busy (503) get distinct messages; an
expired form offers a real fresh-form GET. Other browser document errors use a branded
error page; API/fetch clients keep plain status responses. A protected browser GET that
gets 401 redirects to sign-in; `/` goes to sign-in or the session page.

Backups: stop the server, then copy the SQLite file. Keep `data/`, password and token
files local and outside releases.
