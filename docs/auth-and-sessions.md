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
./mimik --dev-owner-play                  # development: Owner may play
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
- **Serving flags vs subcommands:** `--no-auth`, `--loopback`, `--dev-owner-play`,
  `--lan-address` and `--external-url` are rejected with any subcommand.
  `MIMIK_EXTERNAL_URL` is ignored by subcommands.
- **Conflicts:** `--loopback` with `--lan-address`; `--no-auth` with
  `--dev-owner-play`, `--lan-address`, `--external-url` or `MIMIK_EXTERNAL_URL`.
- **Port:** always 3000. LAN mode binds `0.0.0.0:3000`; loopback and `--no-auth` bind
  `127.0.0.1:3000`. The two modes cannot run at once on overlapping listeners.

### `--no-auth`

Serves the old unauthenticated editor (`src/web.rs`, `/characters`, global source
routes) on loopback only, with a startup warning that account setup does not secure it.
It cannot reach campaign or private-sheet tables. Authenticated mode never falls back
to it: legacy character, global source/image/context and export routes return 403 in
authenticated mode, including for the Owner.

### `--dev-owner-play`

Development only; due for removal (see limitations). It lets the Owner pass gameplay
routes (normally 403), be accepted into campaigns and act as supervising Master; the SQL
guards (`campaign_request_auth` view, Owner triggers in migration 013) admit the Owner
because the request context carries `development_owner_play`, and rosters show the
Owner. Startup prints `DEVELOPMENT ONLY: Owner gameplay override enabled.`; the session
page shows a warning. Without the flag, Owner memberships and custody left from
development stay in the database but give no access and are hidden from rosters.

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
  `/auth/session`, which links the Owner to `/admin` and players to `/campaigns`.
- **Per request** the account is rebuilt from current state: enabled and same epoch, or
  the session is invalid. Any epoch change (reset, disable) revokes all its sessions.
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
