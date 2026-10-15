# Login rejection / integrated error UI

Fresh plan reviewer `ses_ec4b26016ffeG0VhYlgdk0osKA`: execution GO with strict document
presentation separate from GET401 redirect, narrow trusted HTML statuses and no-cookie
failure paths, real fresh-form recovery, bounded/escaped login retention, exact password
bytes and early Origin/form/body/admission browser-error coverage. Existing libraries
cover capability; no new dependency. User-reported correct-password failure is not
yet reproduced or diagnosed; nonsecret URL/login/startup details requested. Uppercase/
padded login is source-supported potential mismatch, not asserted actual cause.

Permanent native wrong-then-correct and canonical-login/exact-password integration
regressions added before implementation. Failing-first results pending. Existing API
plain-status checks will be retained separately from native document assertions.
Actual-user database/password/cookies/.env/manuals remain untouched.

Both native-form and padded/case identity integration tests failed before fix:
bare401 had no alert/form, and correct-password ` OWNER ` returned401 instead of303.
They now pass; passwords retain exact bytes including intentional surrounding spaces.
Unknown/disabled/wrong credentials retain the same non-enumerating message.

Preparing recovery coverage reproduced raw CSRF-only login classification: a captured
anonymous form expiring during dummy password verification returned credential401,
not expired-form403. Fresh narrow reviewer `ses_ec4a175baffedpQYZi7yxkFoFO` approved
reuse of existing durable CsrfAdmission before/after Ok verification, bounded storage,
distinct busy/integrity errors, no rotation on invalid admission. Failing-first
regression now passes, plus valid-password expiry/revocation tests prove no replacement
or authenticated records. This is not asserted as the user's specific failure cause.

Fresh implementation reviewer `ses_ec48ff6cfffeb2diOkTN0iFxDr` requested changes:
existing document tests still required short API bodies and new retry/busy coverage
was missing. Both existing browser tests were run and failed before adjustment.
Only document assertions now require branded HTML/alert/status/security headers/no
cookies/no sensitive content; strict plain assert_error remains, with added API
counterparts for each affected route. Controlled document429/KDF503/busy-flag failed-
retry vs success/408/413 and marker status-whitelist tests added and all6 passed.
Four integration UI tests and controlled admission/updated browser regressions,
strict Clippy/Ruff pass. Full gates/browser/LAN/proxy/artifacts/fresh fix review pending.

Fresh fix reviewer `ses_ec4818a19ffeJIVcjUnDA36412`: conditional implementation GO,
R1/R2 resolved, no new blockers; optional body-limit hook counter improvement added
to prove zero auth work on document413. Full source/artifact/delivered-helper and
offline gates required before install. Updated HTTP/account/release-scope docs describe
inline native sign-in errors versus explicit API/fetch bodies and exact passwords.
Actual user's rejection remains undiagnosed pending nonsecret details.

Full runs exposed two verification-contract issues (not counted as passed): old
`tests/http_auth_races` explicitly allowed an in-flight login after its form was
logged out; original durable admission now rejects403. Narrow independent reviewer
`ses_ec4757814ffeID60aoeHc4UjTq` approved the stricter contract and confirmed implementation:
deterministic logout race unchanged, no new bound session/cookie, old ID nonrevivable,
then actual fresh GET/password303 positive control and unchanged PrincipalSave race.
Targeted all-feature race suite passed. No test removed/weakened/ignored.

New integration fixtures shared the process-global production quota, so aggregate
tests could hit429 despite individual success. Narrow reviewer
`ses_ec469267bffen7GpYSPfNDofwN` approved existing `app_with_test_sessions` with real store
and independent same-policy quota plus explicit test target requiring nonshipping
auth-test-support. Standard CI --all-targets --all-features still executes all four
tests; default-feature test runs intentionally omit that feature-dependent target.
Production global quota and all assertions unchanged. Full rerun pending.

Final full source run started2026-10-14T18:09:22Z exited0:222 Rust tests
(105library/3CLI/114integration),32 helpers, strict fmt/Clippy/Ruff/actionlint,
current audit/notices and default release build passed. Prior failed runs recorded
above are not counted as green. Updated source actual desktop/mobile Chromium
native wrong-password401 inline form then correct/padded-name303, plus all prior
account/private/prototype flows passed; real process/API regressions passed.
Fresh packaged delivered-helper LAN/HTTPS error/revoked-login recovery, source
allowlist/checksums/equality/offline/install gates still pending. Actual user case
remains pending nonsecret URL/login/startup details; no user data read/reset.

Packaged first attempt passed legacy process/desktop-mobile Chromium and empty-cache
network-disabled source build2m19s. Its new LAN login recovery test timed out because
the helper awaited a hidden CSRF input's default visible state. Changed that wait to
attached (password/form interaction remains visible and explicit); this is a helper
readiness fix, not a product/auth bypass. Final consistent artifact regenerates and
must pass LAN/proxy/QR plus all delivered-helper gates before installation.

## Final verification / installation

Fresh helper reviewer `ses_ec43569beffeMrrvpwkXngBMgd` approved attached hidden-CSRF
readiness while retaining visible field actionability, actual fresh GET and explicit
POST/session navigation. Final `target/dist-login-ui-final-20261014/` archive checksums,
source allowlist and default/prior/unpacked binary equality passed. Delivered process
and desktop/mobile Chromium passed native wrong-password401 form/alert/username/password
clearance followed by correct/canonical-name303, styled denied pages and all prior
account/private/native/runtime/prototype/restart checks. The same final artifact
separate-client LAN/HTTPS gate passed wrong-then-correct login, real revoked-form403,
fresh-login GET/explicit successful retry on both ordinary HTTP and configured TLS
proxy, Secure cookie and all existing sharing/QR/declined-peer/role checks. Three rendered
QR screenshots independently decoded with delivered-source rqrr helper offline.

Delivered source/vendor rebuilt default release with empty Cargo home/fresh separate
target and no network in3m28s.222 Rust (105lib/3CLI/114integration),32 helpers, full
strict source gates/current audit/notices and independent scoped GO passed.
New login_ui integration target requires the nonshipping auth-test-support feature;
normal CI --all-targets --all-features includes all four tests, unchanged assertions.

Verified binary17,930,520bytes (+70,752), SHA-256
`af33fe6a567fa12639ae7811c4943b44ebe5ce295533241987ee2b4c89a65918`.
Atomic install/equality/hash confirmed at `target/release/mimik`; restart old server.
Schema14 unchanged, user database/passwords/cookies/.env/manuals not read/reset.
Actual user's correct-password rejection remains undiagnosed pending requested
nonsecret URL/login/startup details; do not attribute it to the synthetic findings.
Post-packaging verification records follow archived handoff; app/build/helper inputs
unchanged. No physical-phone/router/hosted-CI/full-product certification claimed.
