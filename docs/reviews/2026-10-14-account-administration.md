# Browser account administration review ledger

Plan scope: Owner-only account list/invite/reissue/player reset issuance/disable,
manual-token enrollment/reset forms, shared account transitions, schema14 web
attribution and existing bounded maintained auth/session/crypto infrastructure.

Fresh independent plan reviewer `ses_ec6128f88ffeXM0SlA2nzJvUUy`: initial NO-GO;
B1 high final durable CSRF admission, B2 high enforceable transactional attribution,
B3 high ambiguous Owner reset completion, B4 medium anti-framing credential controls.
Amended `docs/account-administration-plan.md` records each required disposition:
captured CSRF rechecked preflight/final, immutable FK/Owner-bound audit with atomic
same-transaction insertion, CLI-only Owner issuance/browser token completion supported,
CSP/XFO/restrictive-referrer policy. Mandatory bounded workers, auth/gameplay separation, lost-response
semantics, old-password reset window, shared quota and exact large IDs retained.
No new dependencies; existing libraries cover generic capability. No implementation
or release correctness is inferred from review. Fresh amended-plan reviewer
`ses_ec60f3e3dffe7VNPKoRhKIyOLW` gave execution GO.

Implementation progress: shared reserve/reset/disable/token issue and byte-based
redemption core; Owner and CSRF durable captured bindings; schema14 sidecar audit,
full-document forms and global auth anti-framing/referrer headers. Generic50+1
pagination moved to shared module unchanged. Existing raw-SQL fixture regression
failed when an unconditional account-event trigger required auth UDF registration
on CLI-only raw connections. Fix: browser event helper explicitly inserts audit
in same transaction, guard validates exact Owner/action/latest-event/target epoch;
CLI events require no new UDF. Permanent historical fixture test passed after fix.
No weakened guard/test exclusion. This narrow design adjustment needs implementation
review. Expanded authority/atomicity/redemption/migration/race tests being verified.

Fresh implementation reviewer `ses_ec5f23e00ffe7pv6kCy66jK4lE` found one medium
usability blocker: masked issued token had no reveal/copy path. Retained Chromium
regression failed before fix with missing reveal/copy control. Native details now
reveals a selectable readonly text field; Chromium verifies visible selection and
actual clipboard copy followed by separate-jar enrollment. No new JavaScript.

Additional real-browser failing-first discovery: no-referrer policy made Chromium
native form POST Origin:null, rejected403. Restrictive same-origin referrer policy
retains exact native Origin while suppressing cross-origin referrers; credentials
never appear in URLs, anti-framing unchanged. Plan records this demonstrated gap.
Reset issuance invalidates the recipient's old session; the reset form initially
redirected that stale visitor to login instead of returning to redemption. Connected
regression failed before fix; invalid sessions now flush and redirect to the same
static redemption route for new anonymous CSRF admission. Browser regression retained.
Process helpers explicitly exercise429/shared-quota Retry-After6 without suppressing
other failures. Followup independent review and full artifact gates remain pending.

Fresh followup reviewer `ses_ec5d109e6ffeiqZQ74YwdDAnau`: scoped source/fix GO, no
blocking findings. Native reveal/clipboard, same-origin referrer, fixed redemption
redirect and genuine separate-loopback framing probe accepted. Low log-scanning
suggestion addressed by including every synthetic invitation/reissued/reset token
and browser-entered passwords in helper log checks (existing generic64hex/PHC checks
also remain). Ninth targeted OwnerID101 test passed captured logout/reset/disable/
expiry denial with no new account/audit mutation. Strict Clippy/Ruff and28 helper
tests passed. Full gates running; fresh packaged/delivered-helper/browser/offline
checks still required before installation/completion.

First full parallel unit run exposed test-fixture contention: the pre-existing
single KDF race suite owned no shared auth lock, while new redemption tests require
that nonqueued global slot. Four new tests received503/PasswordBusy instead of their
intended credential/race outcomes. Fix: existing KDF race suite now takes the same
test-only auth resource lock as new tests. Production admission, race rendezvous,
assertions and coverage are unchanged; no retries/ignored tests/expected failures.
Full gates rerun pending. Current-source actual process and both Chromium flows
passed, including a real other-loopback-origin framing probe. An initial intercepted
origin probe did not establish framing behavior; replaced with stdlib local HTTP
fixture plus authenticated top-level positive control and browser block evidence.

Fresh narrow reviewer `ses_ec5c067f4ffeL5fJGTijkoX4I6` approved explicit-token log
scanning and shared test-only resource isolation: no nested-lock cycle, internal
race/assertion coverage preserved, production nonqueued KDF unchanged.

Final full run started2026-10-14T11:49:09Z in `mimik-feedback-gates-20261013` exited0:
199 Rust tests (89unit/110integration),28 helper tests, strict rustfmt/Clippy/Ruff,
actionlint, current audit, actual licensing/notices and default release passed.
Earlier failed full run is recorded above and not counted as passed. Current-source
process and actual desktop/mobile Chromium passed all previous flows plus Owner
invite/reissue/enrollment/player-reset/disable, visible clipboard copy, revoked reset
form recovery and separate-loopback-origin framing rejection. Packaged/default binary
equality, source allowlist, delivered-helper smoke and empty-cache offline source
rebuild remain final artifact gates before installation.

## Artifact verification and installation

`target/dist-account-admin-20261014/`: all three archive checksums, source-member
allowlist and unpacked/default Docker-build binary equality passed. Fresh unpacked
process used the delivered helpers: Owner invite/reissue/enroll/reset/disable and
authenticated audit, static rejection/secret scans, original workspace/native/runtime/
source/prototype/restart and trusted Owner development-mode preservation all passed.
Delivered actual Chromium desktop/mobile passed visible reveal/select/clipboardcopy,
separate-jar enrollment/reset/disable, revoked reset-form recovery, directory reflow,
all prior auth/workspace/prototype checks, and genuine other-loopback-origin blocked
framing with authenticated top-level positive control. Delivered source/vendor rebuilt
default release network-disabled, empty Cargo home/fresh target in3m48s.

Binary17,565,720bytes (+279,328 versus feedback), SHA-256
`091e473c55fab8a24f9aa9d614f10f67ceb2bcae93cc8adf265964bc32f8e151`.
Atomically installed `target/release/mimik`; byte equality/size/hash reconfirmed.
199 Rust/28 helper tests, independent scoped GO and all required local release gates
passed for the documented loopback scope. Running old processes need restart.
No real Owner database/passwords/cookies/manuals were read or reset; no hosted CI,
LAN/TLS/provider/full-manual/official-layout certification claimed. Post-packaging
result records follow archived handoff; application/build/helper inputs unchanged.
