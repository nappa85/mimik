# Authentication research and account foundation review

## Initial plan review

Fresh read-only reviewer `ses_ecb33894dffe3qdTuu2ZJbmfPm` conditionally approved
library research/staged design and a limited operator account foundation, requiring
a concrete dependency/schema review before execution. No campaign lifecycle
decision blocks this limited scope. Findings: explicit trusted-OS CLI authority,
durable one-time Owner bootstrap, protected credential delivery rather than
stdout, race-safe reset issuance/completion/disable epoch changes, bounded KDF
work outside database locks, actual session-store/API compatibility evidence and
no inferred gameplay grants. All are incorporated in `auth-campaign-plan.md`.

Research implementer `ses_ecb30d9e9ffepH5pHuQzSr21PZ` produced primary-source
library report and isolated container probes (`target/auth-fit-20261013/`):
published stores conflict with current native SQLite; axum-login0.18/sessions0.15
coexist but connected types fail. A connected0.14 control passes but is not a
release selection pending0.15 fix assessment. Crypto candidates compile; no
actual hashing/session auth or application integration was claimed.

Fresh reviewer `ses_ecb23dee0ffeSmwUooH8laKP7n` accepted the research evidence,
but blocked StageA on B1 file/directory durability, B2 invitation reservation and
SQL transitions, and B3 exact PHC admission. The concrete contract now requires
descriptor-relative private output/directory fsync before DB commit, durable
pending-account login reservation/reissue epochs and one-way token/state SQL
invariants, exact97-byte canonical PHC profile before KDF. Corrected the research
license-inventory statement: crypto copies are not hashed by the upstream-library
inventory. Fresh amended-plan recheck pending. No application dependency/schema
changes at that review stage. Reviews are evidence, not correctness proof.

## Amended-plan GO and implementation

Fresh recheck `ses_ecb1fd9c2ffeS9gbE1nKStc9Np` approved StageA execution after
B1–B3 amendments; no new essential plan blocker. StageB/C not approved by that GO.
Implementer `ses_ecb1ed082ffe3vG8d5w1fZDszG` added crypto/account modules,
protected-file CLI, schema10 and regressions. PHC SQL prefix off-by-one (30 vs31)
and INSERT OR REPLACE bypass of delete-only guards were reproduced and fixed;
permanent identity/collision regressions retained. No new generic/session protocol.

Fresh implementation reviewer `ses_ecaed3551ffeN8Jpx5Gvy7doaa` inspected operations
and independently ran existing synthetic test binaries: account4unit/3integration
passed, but nine historical rewind tests failed with account tables already
existing. R1 medium blocking fixture repair was approved only with all-four-table
emptiness checks before drops, transactions, fixed allowlist, schema admission,
remigration through010 and populated-data preservation regression. Production
migration collision rejection was retained, not weakened.

Fixture fix `ses_ecadd380affe6BL8TAdEYApsc0`: populated safeguard regression failed
against original helper (version became6) and again with security check disabled;
after restoration all106 integration tests passed. Four permanent helper tests
verify rejected populated/unknown/missing schemas, rollback of later failures and
successful restoration. Account coverage fix `ses_ecadd37e1ffejJH6oqfyuRgRf7`
reproduced unbounded child readiness (bounded regression failed in1.01s), fixed
deadline accept/read/child exit and RAII child cleanup. Deterministic independent-
process reset races, zero-KDF invalid-token/PHC admission, wrong-owner descriptor/
restrictive umask and post-sync SQL-orphan tests retained. Wrong-owner chown tests
executed as container root; the same assertions are conditional when nonroot.

Artifact helper implementer `ses_ecaca3de6ffeYn9Z9sH1Bi0dnY` added account process
lifecycle/restart to existing smoke, exact static typed failures and secret-free
output checks. Unexpected diagnostic redaction regression failed before fix.
Initial fixture mount/cache failures were corrected, not counted as passes.

Fresh final implementation reviewer `ses_ecab9117cffempZ0kY1wTE8GYw` approved
StageA/fixes/smoke with no essential reproducible code blocker and independently
ran seven smoke-helper tests. Reviewed original pre-fix evidence was not rerun by
that reviewer. Review is for Linux local operator accounts, not HTTP auth or LAN.

## Verification and actual integration

-130 Rust tests (24unit/106integration) passed, plus8 child-process test entries
  executing race participants (not counted as additional independent test cases).
-23 helper tests passed; fmt/strict all-target/all-feature Clippy, Ruff lint/format,
  actionlint, current audit and actual license notices passed. No new exclusion.
- Locked graph265 all-target entries and246 Linux normal/build entries including
  Mimik. Argon2 alloc/getrandom/password-hash/zeroize enabled; no Rayon or session
  crate integration. Release build passed; binary15,649,304bytes (+542,912 versus
  publication checkpoint, about3.6%).
- Single release process samples: bootstrap53.459ms/29,300KiB; minimum password
  match41.608ms/29,528KiB, mismatch42.866ms/29,648KiB; maximum512-byte enrollment
 56.401ms/29,420KiB and verification41.922ms/29,672KiB. Fresh RSS wrapper per
  child, synthetic inputs only; whole-process costs, no hard bound/KDF-only claim.
- Fresh operator documentation reviewer `ses_ecab1b945ffeNY9LvAVrNEo7Aa` approved
  actual implementation/scope/resource claims with no blocking findings. Its
  stale top-of-TODO navigation observation is fixed in the final handoff.
- Corresponding-source package allowlist passed. Fresh checkpoint
  `target/dist-accounts-20261013/` passed checksums of all three archives and
  unpacked binary equality, complete account/publication/native/source process
  typed rejection/restart/persistence smoke and two actual Chromium scenarios.
- Checkpoint delivered source/vendor rebuilt release/locked with network disabled,
  empty Cargo home and fresh target in1m48s. Handoff-inclusive distribution uses
  `target/dist-accounts-final-20261013/`, regenerated and checked separately.

Owner data/secrets/manuals untouched. Current web service remains unauthenticated
loopback-only; epoch transitions are not proof of live middleware revocation.

## Post-packaging final artifact results

`target/dist-accounts-final-20261013/` passed all three checksums, unpacked binary
equality, account/publication/native/source typed lifecycle/restart smoke and
both Chromium scenarios. Final delivered source/vendor rebuilt release/locked
network-disabled with empty Cargo home and fresh target in1m57s. These verification
notes follow the archived handoff; no application/build inputs changed afterward.
Local revision labels are worktree checkpoints, not Git commits or hosted CI.

Final SHA-256 identifiers:

- Runtime: `c0f958924e615bdecad38121ca0b79db9de2d318c92ecb44fbcf0bf57f19e097`
- Corresponding source: `01ed76d2b057391ea0a601358e557d53b39b9bab5fb5b11cd3b52ddfefd97149`
- Dependency source: `0b5fdc9f421f0667fffb18610090f25c80766d46c890987a474cea0f89bba5fa`
