# Password, pagination and Owner scope feedback

User confirmed Owner is admin-only outside development. Password question resolved:
Owner15/player12 Unicode scalar minimum, no mandatory character categories, original
128scalars/512UTF-8bytes/control restrictions and Argon2/PHC profile retained.
Existing passwords/verifiers/data are not rewritten or reset.

Fresh plan reviewer `ses_ec6a7f14dffear6NVPGs30SKuH` gave conditional GO with
independent50+1 cursors, trusted explicit dev flags, production Owner exclusion
including supervising authority, target-bound minimum validation/recheck, additive
schema13 and regression/migration evidence. Main implements, no delegated coding.

Failing-first evidence: roster Next appeared with only1member; new pagination
regression rejected that before fix, now1/49/50/51-member boundaries and sheet-
section preservation pass. Each section has independent exclusive cursor and
authorized51-row lookahead, renders50, Next only when extra row exists and links
preserve other cursors. Directory/personal/history apply the same logic.

Production Owner create-campaign regression failed before gate fix. Default safe
AuthOptions + RequestAuth and schema13 view/extra supervision/membership guards now
deny Owner gameplay even existing grants, and do not treat Owner as a supervising
Master. `--dev-owner-play` is trusted server config, requires --auth/no subcommands,
explicitly labelled development; it is not stored in cookie or accepted from forms/
queries. Existing campaigns/sheets/grants remain stored; dev mode preserves their
explicit access, production mode remains restricted. Admin landing is readonly
Owner-only instructions for existing operator account/material workflows, not newly
implemented credential-bearing web mutation forms.

12-letter player enrollment failed before role policy fix. Shared password policy
now derives from permanent Owner binding in preflight, rejects short target values
before KDF and rechecks role/epoch/state at commit. BootstrapOwner15, enrollPlayer12,
reset/verify/login target-bound; tests include legitimate OwnerID101 and all-lowercase
12-character player password. Hashing, expiry, one-use, revocation and history unchanged.

Earlier tests/default release passed184 Rust and27 helpers; the expanded full run
passed189 Rust (79unit/110integration) and27 helpers, strict
fmt/Clippy/Ruff/actionlint/audit/actual notices. Current-source process/browser flows
use ordinary collaborators for campaign grant tests, and separately prove normal
Owner admin landing/denials. Earlier actual desktop/mobile Chromium and process pass;
the final expanded source/artifact gates are pending. Initial version-assertion
fixtures were corrected to13/unknown14 with preservation checks retained. A tool
partial write was restored before compilation; no tests or guards were removed.

Owner database/passwords/manuals/cookies are not read/reset by verification.

## Independent final implementation reviews (2026-10-14)

Fresh read-only reviewer `ses_ec62f357dffeTq5yqLm4l45Sej`: scoped implementation GO,
no blocking correctness/authorization findings. New connected tests cover directory,
requests, approved versions and campaign sheet boundaries1/49/50/51, authorized
personal lookahead, history49/50/51 and last rendered exclusive cursors, and nonzero
cross-section cursor preservation. SQL Owner insertion/regrant/supervision, trusted
false/missing context and populated schema12→13 retention checks are retained.

Low findings/disposition:
- Retained development Owner request acceptance returned500 in production. The
  permanent domain regression failed before fix. Production request queries now
  exclude Owner before lookahead; acceptance explicitly returns Invalid before any
  mutation. Connected HTTP regression verifies422/no cookie, hidden production
  request, retained development pending revision. Targeted checks passed.
- Collaborator diagnostic/sheet labels corrected; some local variable names remain
  historical, with actual ordinary-player login explicit. No authority confusion
  in the tested operation.
- High-ID Owner exact15 verification/authentication and14 rejection added and passed.

Fresh read-only followup reviewer `ses_ec628594cffe9h00WIsgWXcTSv`: scoped GO, no
blocking findings in Owner request fix, boundary tests or development process smoke.
Low suggestions: history smoke initially compared only count, now compares ordered
complete retained-sheet history rows; direct decline positive-control coverage is
optional followup (source review confirmed unchanged transactional decline path).
Review is not proof of correctness. Full final gates and new artifact-mode smoke,
checksums/source allowlist, browser and delivered-source offline rebuild remain
required before completion.

## Verification checkpoint

Final application gates passed190 Rust (80unit/110integration) and27 helpers in
`mimik-feedback-gates-20261013` run started2026-10-14T09:56:16Z: strict Clippy,
rustfmt, Ruff/actionlint, current audit, actual license notices and default release.
The helper-only strengthening subsequently passed all28 helpers and strict Ruff.

Fresh narrow reviewer `ses_ec62055c5ffeFBCUB9wmNXCMqD` blocked the first complete-row
history comparison because an empty baseline could compare equal. Permanent fault-
injection regression failed before fix (expected assertion absent), then passed
after explicit nonempty history/existing snapshot admission. Fresh narrow reviewer
`ses_ec61e3c8cffevo3GjsH0V7HDK0` gave GO; its targeted regression also passed.
This reviewed helper change does not alter application/build inputs.

Checkpoint `target/dist-feedback-20261014/`: checksums, unpacked/default-build binary
equality, corresponding-source allowlist and real process startup/edit/persistence/
restart/auth/native/runtime/privacy passed. Development process acceptance passed:
invalid CLI flags reject before creating a database parent, Owner explicit gameplay
under dev option, same-cookie production denial, renewed dev access with identical
live snapshot and ordered complete nonempty history rows. Strengthened smoke was
rerun successfully. Actual Chromium desktop/mobile prototype/auth/workspace passed.
Delivered source/vendor default release rebuilt with empty Cargo home/fresh target
and network disabled in1m57s. An initial Chromium invocation mounted the helpers
at the wrong path and rejected --auth; corrected `/checks` mount ran current helpers
and passed both sizes. No unsuccessful invocation counted as verification.

Binary17,286,392bytes, SHA-256
`03b935103c63bf604850df25b113d18b1b752e56dab32d18d26cc1d083d57c77`.
Handoff-inclusive final package/recheck and atomic installation are next.

## Final artifact verification and installation

`target/dist-feedback-final-20261014/` checksums, unpacked/checkpoint/default binary
equality, source allowlist and fresh unpacked process startup/edit/persistence/
restart/auth/native/runtime/privacy all passed with delivered helpers. Explicit
development invalid-flags and dev→production→dev restart test passed, including
same-session production denial and exact live snapshot/nonempty ordered complete
history preservation. Delivered desktop/mobile Chromium prototype/auth/workspace
passed. Delivered source/vendor rebuilt default release without network or dependency
cache/fresh target in2m03s. The terminal call reached120s while the container was
building; explicit docker wait returned0 and completion log confirmed success.

Verified binary was atomically installed at `target/release/mimik`; byte equality,
17,286,392byte size and SHA-256 above confirmed. Running old processes need restart.
Feedback slice complete for documented loopback scope, with190 Rust/28 helper
tests and fresh independent scoped GO. No hosted CI or LAN/TLS/inference/manual
certification claimed. These post-packaging result notes follow archived handoff;
application/build/helper inputs unchanged. Owner real files/data remain untouched.
