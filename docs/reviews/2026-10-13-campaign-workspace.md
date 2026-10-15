# Stage-C usable campaign workspace review

User feedback: signed-in Owner can only switch accounts; deliver useful campaign
creation and private editable sheets/history next. Fresh preliminary read-only
reviewer `ses_ec9e123e9ffe8gKfw5SV5DFyvp` gave conditional GO, requiring concrete
SQL transitions/transactional session authorization, numeric bounds/freezes,
complete canonical history/pins, private templates and runtime capability gates.
Those constraints are in `campaign-workspace-plan.md`. No transfer/reactivation/
delete/purge decision blocks this restricted core; such operations stay absent.

Capacity-probe implementer `ses_ec9da502bffePcSS0wcgqwc4k5` measured streamed
8/16/32/64×1MiB synthetic snapshots. Provisional32 cap: median1.175s/max1.276s,
whole-child peak41,260KiB, with last-row failures rolling back31 prior updates,
history and membership. Other limits unmeasured. Probe-only independent plan/
results GO reported; final concrete-plan review must assess evidence/actual Rust
acceptance gates, not treat prototype numbers as shipping bounds.

Fresh concrete reviewer `ses_ec9c71099ffedvzjEKr4HDQnvr` blocked code on B1
mandatory-freeze canonical headroom, B2 explicit live/history correspondence and
B3 evidence-independent immutable-pin loader/bounded read pipeline. Plan now
reserves maximum-width revision/state budget and final freeze revision, selects
one shared writer with deferred current-history FK and strict history guard,
and distinguishes immutable content from transactional creation readiness using
the already-held runtime permit. Context-aware data decoder/full read admission
requirements retained; capacity counts frozen/distinct reservations. Fresh amended
plan recheck `ses_ec9bfd9c6ffetokMpeo7PWShns` granted implementation GO after
B1–B3 amendments, with actual Rust workload and full verification required.
The interrupted implementation left substantial application/schema/template/test
changes; they compile but are being verified by the main implementer before an
independent code review. User requested fewer agents; implementation stays in the
main session, with a separate reviewer only for required review gates. Owner
database/manual/credentials are not read or modified by these checks.

## Resumed implementation verification and fixes

The main session resumed the interrupted code without delegating implementation.
Initial check/strict Clippy passed. An existing CLI test failed to launch a cached
executable; recompilation passed and a package-only clean build then passed all
168 existing Rust tests. No owner data or dependency caches were cleared.

Fresh read-only reviewer `ses_ec93289e4ffeIJuFN9utuWNbgw` withheld completion on:

- Medium R1: readiness was read after validation in a second transaction without
  coherent resource/grant revalidation. A deterministic validation-gap regression
  failed before fix: demoted noncustodian still received the private view. Final
  admission now rechecks exact canonical resource, current grants and readiness
  together; historical reads compare their retained event instead of live values.
- Medium R2: predecessor history JSON reached SQLite parsing without its own byte/
  validity guard. A matching-pin oversized predecessor regression failed before
  fix. The view now short-circuits predecessor parsing with byte/JSON-validity CASE
  guards. Malformed predecessor and valid near-cap workload controls retained.
- Essential coverage: added actual256-field/516-pair editor/history, reserved-looking
  value IDs, creation readiness withdrawal after admission, stale editing,
  captured logout/rotation/expiry authority, blocked-worker expiry, stored body/pin/
  definition corruption, final revision with mandatory freeze,64 distinct reservations
  and2048 global private-sheet limits. Existing32-sheet retained/freeze rollback,
  hash vector, overflow/privacy/legacy/cancellation and populated fixture tests stay.

Generic form handling now reuses its admitted pin's compiled validator instead of
compiling a duplicate schema; private native fields reuse the existing dark native
presentation. Authorized422 pages explain safe static validation causes while
preserving the full escaped draft/revision.

## Browser/process/helper evidence

Main-session helper changes exercise actual native/runtime create/edit/history,
Owner-only denial, accepted-player denial, explicit Master grant/revocation,
departure freeze for both roles, sticky rejoin, cross-campaign denial and restart.
Native/runtime sheets stay absent from prototype tables. Chromium verifies the
same full-page actions at1280×720/390×844, long text/overflow, immutable historical
values and escaped409 drafts. The first browser attempt could not fill a collapsed
advanced field; the helper now opens its summary before editing (no app gate bypass).

Form extraction now supports selected nullable/boolean/enum controls; its permanent
test failed before fix with an empty extracted map. The initial workload wrapper
matched only one timing label; a permanent parser regression failed before the
two-label exact check was corrected. Neither failed helper run was counted as pass.

Current main verification:176 Rust tests (66unit/110integration),27 helpers, strict
fmt/Clippy/Ruff/actionlint/current audit/actual notices/default release pass. One
SQLite/dependency graph remains; no new crate, direct serde_urlencoded was already
locked. Release-mode actual32×near1MiB revision9→10 workload sampled rollback
2948.294ms/freeze3051.250ms, whole-child peak86,012KiB. It includes fixture/setup/
verification memory, not only transaction memory; sampled≤5s/≤128MiB targets
passed, not hard bounds. Raw runner `tools/campaign_profile.py` runs one release
test child with fresh RSS accounting. Real default-release process and two actual
Chromium workspace/prototype flows passed; final review/artifact/offline gates pending.

## Final implementation review

Fresh read-only reviewer `ses_ec8ea3c72ffeZNDybBV81IPv9Z` approved code/helpers
with no new essential blocker: R1/R2 closed,176 Rust/27 helper coverage accepted,
actual workload targets met, private/prototype isolation and library reuse sound.
Reviewer performed source inspection, not rerunning the main session's checks.
This is implementation approval; final archive/unpacked/offline gates remain
pending. User requested fewer agents: only required reviewers were used while the
main session implemented fixes, tests, helpers and final verification.

Operator-doc review `ses_ec8e24a23ffeY4Qa6X7kUD3dEI` accepted the workspace
instructions/role/freeze/resource scope but identified stale roadmap wording in
README/release scope and the old unqualified8KiB HTTP cap. Those statements now
distinguish implemented private routes from prototype/deferred lifecycle scope,
route-specific1MiB sheets from8KiB controls, and actual transactional write checks.
Fresh documentation recheck `ses_ec8dfd2bfffe6G2meB9wNMu8ac` approved the corrected
scope/caps/transaction wording with no remaining blocker. No application change
from these corrections.

Checkpoint `target/dist-workspace-20261013/` passed all three checksums/default
binary equality, unpacked campaign/native/runtime/history/role/freeze/restart
process flows plus prior account/publication/prototype checks, both actual Chromium
viewports, and delivered-source/vendor default release rebuild network-disabled
with empty Cargo home/fresh target in1m58s. Handoff-inclusive distribution uses
`target/dist-workspace-final-20261013/` and is regenerated/rechecked separately.

## Post-packaging final artifact results

Handoff-inclusive final passed all checksums and equality with the installed
default binary, complete unpacked process flows, desktop/mobile actual Chromium
workspace/prototype scenarios, and delivered-source/vendor default release rebuild
locked/offline with network disabled, empty Cargo home/fresh target in1m56s.
These post-packaging notes follow the archived handoff; app/build inputs unchanged.

SHA-256 identifiers:

- Runtime: `f463e1ea780aa78c2d1b0b5a30f128659ebbe7c11fea81c32360e588056c8bde`
- Source: `3c35e844b0917a17abb6c79e7952423e68ff1882cced8a07dbb26abef353b7f0`
- Dependencies: `9fe089006d2294a21af2293b0e23771ac677c90614fa9598f443db25d124730f`

Local labels identify worktree checkpoints, not Git commits or hosted green CI.
