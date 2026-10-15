# Anonymous browser entry routing regression

User report: opening the app shows `Invalid credentials or session.` even in an
anonymous browser. Main reproduced failing-first: anonymous root redirected to
`/auth/session` rather than sign-in, leading to bare401. Owner data, cookies and
passwords were not inspected/reset.

Independent plan reviewer `ses_ec709c8b0ffeveBuGBQykVxA79` approved a presentation-
only fix, requiring validated principal at root, protected GET-only401 navigation
redirects, fixed destination, strict MIME quality and Fetch Metadata handling,
unchanged API/POST/403/500 and fail-closed corruption/storage behavior.

Root now sends validated sessions to `/auth/session` and anonymous/expired/revoked
sessions to `/auth/login`. Explicit protected browser GET navigations receive a
303 sign-in redirect rather than bare401; POST failures and API/fetch requests
retain authentication errors. MIME media-range tokenization uses the existing
MIT/Apache-compatible `mime0.3` crate (direct declaration, already locked/transitive).
Mimik-specific q≤3-digit weight and navigation policy is narrow presentation logic,
not a generic MIME parser or authorization bypass. Malformed/wildcard/q0/duplicate
ranges and explicit non-document Fetch Metadata fail closed. With no Fetch
Metadata, explicit acceptable HTML is a compatibility fallback; such legacy clients
cannot be reliably distinguished from fetch. It grants no rights or private body.

Permanent tests cover cookie-free root/bookmarks/form, valid principal/root/private
denials, expiry equality and no loop, corrupted records500, media quality/metadata
vetoes, API401, and unchanged HTML-accepting failed login POSTs. The latter runs
in the existing integration test's single KDF context rather than racing the library
unit test's process-global KDF owner. Initial full run exposed that test-isolation
error (503 instead of401); no production busy guard was weakened or test ignored.
Actual Chromium helpers now open `/` in a fresh context and expect revoked browser
sessions to return to the login form, while process/API replay checks remain401.

Main180 Rust/27 helper full gates, strict Clippy/format/Ruff/actionlint/audit/notices
and default release passed. Actual Chromium now reaches sign-in from fresh root and
from revoked cookies at both viewports; complete workspace/prototype flows passed.

Fresh implementation reviewer `ses_ec6ebf8bcffeojZArQP9ptRryi` identified P2:
MIME's decoded values erase quotes, accepting malformed HTTP weights `q="1"`.
Expanded permanent regression failed before fix (303 instead of401). A maintained
existing `regex1` static, bounded raw-syntax check now rejects quoted numeric
weights before MIME-decoded weight admission; no arbitrary regex compilation.
Exact MIME essence also prevents treating `text/html+xml` as HTML. Positive quoted
charset/extension controls retain valid header support. Both crates were already
transitive/locked; no new package or licensing exclusion. Targeted tests/strict
checks pass; fresh fix review/default release/artifact verification pending.
Runtime/data schema unchanged. Fresh narrow fix reviewer
`ses_ec6de89a3ffe2umKREN1nAgByv` approved P2 closure and unchanged authorization/
API/error behavior with no essential finding. One earlier review-tool invocation
failed from server overload and was not counted as approval.

Final code passed180 Rust tests (70unit/110integration),27 helpers and strict
fmt/Clippy/Ruff/actionlint/audit/notices/default release checks. Default binary
17,248,920bytes was atomically installed without stopping the user's old process.
`target/dist-entry-fix-20261013/` passed all three checksums and installed-binary
equality, complete unpacked API/workspace/prototype process flows, actual fresh-root
and revoked-cookie browser sign-in at both Chromium viewports, and default delivered-
source/vendor rebuild locked/offline, network disabled, empty Cargo home/fresh
target in2m00s. Owner DB/credentials untouched. Post-packaging notes follow the
archived handoff; app/build inputs unchanged afterward.

SHA-256 identifiers: runtime
`239f60615ca769aa5300ef83abc5a8cddb2b3b6285f7da20deb38f4d7f0c0b97`, source
`4b33a464f4be7b716854370dfc56fe2c7ae2e6532e2a2784e8ce1341bb18f540`, dependencies
`36476c544c26cfde12371e57516fe1236f8d0169baac0837cd9bb8c6f84122cb`.
Worktree checkpoint labels are not Git commits or hosted CI certification.
