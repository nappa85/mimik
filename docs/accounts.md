# Local operator account foundation

Implemented: schema10 local CLI/domain/storage for one-time Owner bootstrap,
reserved-login invitations, enrollment, password resets, disablement and
secret-free summaries. **The existing web editor remains an unauthenticated,
loopback-only prototype only with explicit `--no-auth`.** Normal startup provides a separate
login/session router using these accounts and live epoch revocation, with the
authorized private workspace in `campaign-workspace.md`. Legacy/global source
routes remain denied. See `http-auth.md`.
Authenticated trusted-LAN startup and configured proxy sharing are described in `deployment.md`.

## Browser account workflow

Sign-in feedback stays in the page rather than bare text. It distinguishes a failed
login from expired form/session, throttling and busy service, without exposing account
existence. Use the account's login name (not display name); surrounding spaces and
ASCII case are ignored for that identity only. Password spaces remain significant.

In normal authenticated mode, sign in as Owner and open **administration** (`/admin`). The
directory lists50 accounts per page with a real next-page cursor. Create an
invitation for a new login; for a retained pending reservation use **Reissue
invitation**. Reissue binds to the displayed epoch and supersedes the old token.

The successful issuance response contains a shareable link, **Copy link** button and
QR code. Send the link or let the recipient scan it from their phone. The appropriate
form opens with its masked token filled automatically. The token travels in the URL
fragment, not GET requests/referrers; the local script removes it from the address.
Optional manual token entry is available. The server does not retain retrievable raw tokens or put them
in sessions; GET cannot recover a previous issuance. Browser copies/screenshots are
not erased by server headers. Closing/lost responses requires inspecting the current
account epoch and explicitly reissuing, not repeating an old form.

For an enabled player, **Issue password reset** produces a one-hour token for
`/auth/reset`. Issuance invalidates old sessions immediately but the old password can
still create new sessions until completion. Completing enrollment/reset redirects to
sign-in without logging the recipient in automatically. Both token and password are
masked on entry; exact confirmation is required. Invalid/replayed/stale tokens and
failed confirmation cannot change accounts. Expired/revoked CSRF sessions cannot
commit after password work. Recoverable errors retain the token only in the current
page, clear passwords and offer a genuine fresh-form GET when admission expired.

**Disable account permanently** irreversibly disables an enabled player in this
slice. It revokes sessions/credentials, retaining personal sheets/custody/history.
Owner reset issuance/disablement remains local CLI; a valid CLI-issued Owner reset
token may be completed in the browser with the15-scalar minimum. Disabled-account
recovery/re-enable and Owner succession are not supported.

Account changes and authenticated Owner attribution commit atomically. Schema14
preserves legacy account events and adds immutable `account_web_events` rows with
actor ID/epoch and one-way session digest. Presence of this sidecar identifies web
administration; legacy `local_operator` labels alone do not identify an authenticated
browser actor. No password/raw token/replayable session enters the audit.

All browser forms use strict Origin/CSRF, bounded input/workers, no-store/nosniff,
CSP frame-ancestors 'none', XFO DENY and Referrer-Policy same-origin. This referrer
policy suppresses cross-origin referrers while preserving native POST Origin;
no-referrer was incompatible with strict Chromium native-form Origin admission.
New anonymous forms/redemption attempts share the existing auth quota;429 asks for
a six-second retry. A timeout can precede an already-admitted atomic commit: inspect
state before retrying; a completed token is consumed even if its response was lost.

## Authority and protected files

The CLI trusts local OS/database access. Passing an account ID/name is not
authentication as that person. Bootstrap establishes exactly one durable Owner
binding; enrollment grants no Owner or campaign role. Technical administration
never implicitly confers sheet/history rights.

Passwords and redemption credentials are accepted only as bounded private files,
never raw command-line values. Files must be effective-user-owned regular files,
with no group/other permissions; final-component symlinks are rejected. Password
content is exact UTF-8: Owner passwords require15–128 non-control Unicode scalars,
player passwords12–128, with at most512 bytes for either role. No uppercase,
lowercase, digit or symbol category is mandatory. There is no trimming/normalization
or trailing newline. Reset and login use the target account's durable Owner binding.
Existing verifiers are not rewritten. Prepare files using a trusted
local input method; commands do not print password/verifier/token contents.

Choose an effective-user-owned private directory (for example mode0700) for
credential output. Each output basename must be new: no overwrite or symlink
following. Created files are exactly0600 and contain64 lowercase hex bytes, no
newline. Descriptor-relative creation pins the directory; file and directory
fsync finish before issuance commits. Caller controls the trusted ancestor path.
Support is Linux local filesystems with working file/directory fsync; tests verify
control flow, not physical power-loss guarantees.

Database rollback or process interruption after file sync may leave an orphan
file. An uncommitted credential cannot redeem; reissue explicitly to a new path.
Do not retry by overwriting the old output. No racy pathname cleanup is performed.
Input/output files are local credentials: keep them outside source releases and
remove them locally when no longer needed.

## Operator workflow

Use the same `--database` for every command. The default remains
`data/mimik.sqlite3`; `MIMIK_DATABASE` is also supported.

```sh
./mimik --database data/mimik.sqlite3 bootstrap-owner --login owner --display-name "Local Owner" --password-file private/owner.password
./mimik --database data/mimik.sqlite3 invite-account --login player --display-name "Player" --token-output private/player.invite
./mimik --database data/mimik.sqlite3 enroll-account --token-file private/player.invite --password-file private/player.password
./mimik --database data/mimik.sqlite3 show-account --id PLAYER_ID
./mimik --database data/mimik.sqlite3 list-accounts --after-id 0
```

Bootstrap cannot be repeated, even after disablement. Invitations reserve a
unique login as `pending`; expired reservations are retained. Reissuing the same
pending login/display name to a new output path increments its epoch and revokes
old invites atomically. Enrollment consumes the one-use credential, enables that
reserved account and advances epoch. It does not select an administrative role.
Invites expire after24h, resets after1h; equality at expiry is already expired.
Issued invites remain independently redeemable if the Owner is later disabled;
disablement revokes credentials belonging to the disabled target account.

Logins are3–64 lowercase ASCII letters/digits/hyphen/underscore, starting with a
letter/digit. Display names are1–100 non-control scalars. Identity/display changes
and reservation deletion are not exposed. Lists return at most100 ascending
summaries after an exclusive ID cursor. Keep CLI integer IDs/epochs exact.

## Reset, verification and disable

Read the target's current `epoch` from a summary; stale values conflict.

```sh
./mimik issue-account-reset --id PLAYER_ID --expected-epoch CURRENT_EPOCH --token-output private/player.reset
./mimik complete-account-reset --token-file private/player.reset --password-file private/new.password
./mimik verify-account-password --id PLAYER_ID --password-file private/new.password
./mimik disable-account --id PLAYER_ID --expected-epoch CURRENT_EPOCH
```

Reset **issuance** advances epoch and supersedes outstanding credentials before
redemption. It does not yet replace the old password. Completion consumes the
credential, replaces the verifier, advances epoch again and invalidates remaining
credentials. Disable advances epoch and revokes that account's credentials;
redemption cannot re-enable it. Owner disablement prevents further administrative
commands; re-enable, Owner succession and disabled-Owner recovery are unsupported
in this stage. No character custody is changed by account disablement.

`verify-account-password` returns only `{"matched": true/false}`. It is an operator
verification primitive, not login, session creation or a network authentication
endpoint. Missing/disabled accounts do not match. Malformed stored PHC values
return integrity errors. Successful verification has a final coherent account
state/epoch recheck, so changes committed before it defeat an old result; a later
change cannot retroactively retract a returned result.

## Security, concurrency and storage boundary

Maintained RustCrypto Argon2id/v19, m19456KiB/t2/p1, random16-byte salt and32-byte
output produces canonical97-byte PHC strings. Full profile/canonical admission
precedes expensive verification. Password buffers and Argon2 memory use zeroize;
this does not promise erasure from unrelated editor copies, swap or backups.
Invite/reset entropy is32 OS-random bytes; only SHA-256 digests are stored.

One process-global nonqueued KDF job is permitted; cancellation retains its permit
through detached blocking work/database handoff. Hashing runs outside SQLite
locks. Issuance synchronizes tiny credential files inside the serialized write
transaction; slow filesystem sync can delay other writes and surfaced busy
failures may require retry to a new path. Separate processes are not an aggregate
memory limiter. Every committing redemption rechecks state/purpose/epoch/time;
independent-process bootstrap/redemption has one winner.

Schema10 STRICT constraints and guards retain account identities, Owner binding,
purpose/epoch-bound credentials, one-way transitions, checked integer epochs and
append-only non-secret account events. Events identify `local_operator` versus
`credential_redemption`, not authenticated web actors. SQL envelope checks are
supplemented by full Rust PHC admission. Database administrators can still alter
or replace storage; ordinary-SQL guards are not a database-owner sandbox.

Migration is additive, atomic and serialized. Historical schemas1/6/7/8/9 upgrade
without character/source/publication changes; collision failures roll back.
Older binaries reject schema10. Back up local data before upgrades.

## Account foundation measurement snapshot (schema10)

Crypto selections: argon2=0.6.0, getrandom=0.4.3, zeroize=1.9.1; actual Argon2
features alloc/getrandom/password-hash/zeroize, no Rayon or session crates. Locked
graph265 all-target entries (including Mimik); Linux normal/build246 entries
(including Mimik), measured by unique package/version tree identities. Actual
audit/notices passed; no new license exclusions.

Release binary15,649,304 bytes versus prior publication15,106,392: +542,912 bytes
(about3.6%). Single synthetic shared-host Linux samples from release CLI, each
with a fresh child RSS wrapper (KiB), include launch/schema/read/KDF/SQL/output/
exit rather than KDF-only cost:

| Operation | Password bytes | Process ms | Sample peak RSS KiB |
| --- | ---: | ---: | ---: |
| Registry baseline, no KDF | 0 | 29.373 | 14,576 |
| Owner bootstrap | 15 | 53.459 | 29,300 |
| Password match | 15 | 41.608 | 29,528 |
| Password mismatch | 15 | 42.866 | 29,648 |
| Invitation/file synchronization | 0 | 20.246 | 14,576 |
| Enrollment,128 four-byte scalars | 512 | 56.401 | 29,420 |
| Password match,maximum bytes | 512 | 41.922 | 29,672 |

These are observed samples, not hard memory/latency bounds, worst-case measurements
or concurrent-process capacity guarantees. Raw sample runner is local ignored
`target/account-profile-20261013.py`. Reviewed scope/gates:
`reviews/2026-10-13-auth-foundation.md`, `release-scope.md`, and `../TODO.md`.
