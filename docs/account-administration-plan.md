# Browser account administration — scoped implementation plan

## Scope and confirmed policy

Continue the installed schema13 loopback auth workspace with Owner-only account
listing, invitation/reissue, player reset issuance and disablement, plus anonymous
enrollment/reset completion forms. Existing accounts remain unchanged. Owner remains
admin-only unless the trusted development override is enabled. Account CLI remains
available. Material/source upload and character lifecycle are subsequent slices.

Use existing durable account transitions, RustCrypto hashing, OS entropy, hex/SHA256,
rusqlite transactions, Askama/Axum and tower-sessions CSRF/session infrastructure.
No new dependencies or vendor protocols. Browser-specific code bridges transport
and authenticated attribution, not a duplicate account/credential engine.

## Domain and persistence

- Separate session-binding recheck from gameplay admission; Owner administration
  requires current enabled durable Owner, captured original session/deadline/epoch,
  expiry sampled after blocking reads. No form/cookie role or dev flag confers Owner.
- Refactor invitation/reset/disable into transaction helpers shared with CLI. Web
  operations use bounded storage workers and recheck Owner in that transaction.
- Credential generation/digest/expiry remain centralized. CLI writes protected
  files and fsyncs before commit as today. Web issuance returns a zeroizing token
  only after commit; the successful no-store HTML page shows the64-hex token once.
  No credentials in URLs, logs, redirects, sessions or audit rows. Issuance whose
  response is lost can be superseded by an explicit reissue/reset using fresh epoch.
- Browser invite creation may only create an absent login; reissue of a pending
  reservation requires its expected epoch. CLI reissue behavior remains compatible.
- Reset issuance and disable in browser target ordinary enabled players only.
  Owner reset issuance remains CLI; a valid CLI-issued Owner reset token may be
  completed through the browser with Owner15 minimum. Disabled-account recovery,
  Owner succession and re-enable remain unsupported.
- Add schema14 append-only web administration attribution linked to existing
  account_events: event ID, authenticated actor ID/epoch and one-way session digest.
  Existing event rows and labels are preserved, documented legacy operator labels
  do not alone establish authenticated attribution. No replayable session/token.
- Refactor credential redemption to bounded byte-based core shared with CLI files;
  purpose-bound64-lowercase-hex token, Owner15/player12 password policy, KDF admission
  and final credential/epoch/state/role checks stay authoritative. Browser completion
  never signs in automatically and never enables a disabled account.

## HTTP and presentation

- Owner `/admin` account directory (50+1 cursor) and forms. Strict POST Origin,
  CSRF,8KiB body, deny-unknown-field structs and duplicate rejection from serde;
  IDs/epochs are decimal strings rendered in native forms, no JS Number transport.
- `/auth/enroll` and `/auth/reset` expose manual token-paste/password forms. No token
  query parameter or return URL. Use existing anonymous ten-minute CSRF admission
  and governor quota for both form issuance and redemption. Existing login forms
  and absolute authenticated expiry remain unchanged.
- Mask password/token controls; never echo submitted token/password on failures.
  Password confirmation must match exact bytes. Redemptions redirect to login only
  after success. Errors remain static status bodies, no credential-bearing cookies.
- Successful Owner issuance shows exact token/purpose/expiry and instructions to
  share it locally. GET cannot repeat a token. Reissue/reset invalidates prior token
  and all target sessions through epoch advancement. Disable retains custody/history.
- Read-only material workflow links remain on admin page. Buttons represent current
  state; unsupported recovery/re-enable/delete/purge actions are not exposed.

## Required verification

Permanent tests cover anonymous/player/forged Owner denial, revoked/expired/captured
Owner race, account pagination, stale target epochs/reissue/duplicate submission,
no partial state/audit on rejection, credentials/purpose/expiry/one-use, valid
player12 enrollment and Owner15 reset via CLI/browser core, wrong confirmation,
disable session revocation and unchanged private sheets/history, no secret leakage,
Origin/CSRF/body/content-type and retained migration state. Reproducible issues get
failing-first regressions before fixes. Independent plan and implementation reviews.

Full locked Rust/helper checks, strict lint/audit/notices/default release/source
allowlist; packaged account admin/enroll/reset/disable/restart process and real
desktop/mobile Chromium; artifact checksums/equality, delivered-source empty-cache
network-disabled default release rebuild, atomic install and operator handoff.
All checks use fresh synthetic data, never the user's database/manuals/credentials.

## Independent review amendments

Reviewer `ses_ec6128f88ffeXM0SlA2nzJvUUy` required these execution contracts:

1. Capture original CSRF session ID/deadline/token and principal (if authenticated).
   Both redemption preflight and final transaction recheck exact durable active
   payload/binding/current account epoch and captured deadline; sample time after
   row retrieval. Expired/revoked/recreated sessions cannot commit. Authenticated
   visitors retain their existing absolute expiry; GET never shortens/extends it.
2. Schema14 attribution is unique by event ID, positive actor ID/epoch, fixed64-hex
   one-way digest, FK-backed and append-only/no replacement. The browser event
   helper inserts authenticated metadata immediately after the account event in
   the same transaction. Its SQL insertion guard validates trusted current Owner
   context and latest matching action/target epoch; invalid linkage fails.
   Domain inserts account event only within the same transition transaction; failure
   of audit insertion rolls back all state/credentials/event. Legacy events retain
   labels; presence of the sidecar establishes authenticated web attribution.
   Anonymous redemption has no fabricated authenticated actor. SQL guards cover
   linkage/shape/current authority/action, domain APIs cover operation semantics;
   database owners remain outside this ordinary-SQL integrity boundary.
3. Owner reset issuance is CLI-only; browser completion of valid CLI-issued Owner
   tokens is explicitly supported. It grants no session and cannot re-enable.
4. Every auth response uses CSP frame-ancestors 'none', X-Frame-Options DENY and
   Referrer-Policy same-origin. Credential responses are full document forms only,
   no htmx/history client persistence. Browser embedding is tested.
5. Shared session binding and separate Owner/gameplay checks, bounded SQL in every
   browser phase, worker-owned KDF permits through final commit, shared quota for
   new CSRF/redemption attempts, and no GET credential mutation/expiry extension.
6. Timeout/cancellation may precede an admitted commit. Retry by inspecting state:
   duplicate invite creation conflicts, explicit reissue/reset needs a fresh epoch;
   a committed redemption consumes its token even if its response is lost. Reset
   issuance revokes prior sessions but old password can still create new sessions
   until reset completion; it is not a password lock.
7. 'Shown once' means no server GET retrieval/session token storage/automatic retry.
   Zeroizing intermediates do not erase response/browser copies/screenshots.
   Tests include exact token shape, expiry/races, malformed/duplicate form fields,
   non-default Owner ID and account pagination with exact large IDs/epochs.

Browser verification demonstrated that Referrer-Policy no-referrer causes Chromium
native form POSTs to send Origin:null, correctly rejected by strict exact-Origin
validation. Use same-origin instead: suppress cross-origin referrers while retaining
native same-origin POST authority. Credentials never occur in URLs. Keep CSP/XFO
anti-framing unchanged; retain real-browser login/enrollment/reset regressions.
