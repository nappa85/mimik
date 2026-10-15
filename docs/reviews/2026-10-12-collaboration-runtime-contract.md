# Collaboration roadmap, runtime contract revision and validator fit

## Scope and plan review

User requested deferred shared-user management, protected character editing,
distinct technical Owner/campaign Master, full sheet-edit history/rollback,
logical deletion and Owner-only physical purge; continue runtime-definition
priority rather than implement collaboration now.

Fresh plan reviewer `ses_ed085d682ffeaSu7Ve4SbogM4E` approved documentation work,
requiring no inferred Owner gameplay grants, explicit future visibility/deletion/
restore/rollback questions and no promise of permanent rollback after erasure.
R1–R4 original contract blockers remained until actual revision review.

## Revised-document review

Fresh reviewer `ses_ed079ec05ffetAvueggSB2b6Ip` approved collaboration requirements
and runtime contract rev2 **plan-ready**, resolving original high evidence lifecycle
and bounded-schema findings plus medium legacy-ID/value-semantics findings at
specification level. Publication snapshots, atomic rechecks, explicit stale
invalidation/reapproval and pinned editing are specified; host-owned bounded
schemas/offline library boundary; exact aliases/native reservation; complete
replacement with null/empty/false/zero/i64/browser/revision semantics.

Low findings: reconcile TODO's old blockers, make runtime definitions the explicit
current roadmap priority, and label Master-only knowledge a proposal. All corrected.
Collaboration implementation and remaining user policy choices are deferred;
current service still loopback-only with no authenticated sheet audit trail.

## Isolated probe review and results

Pre-execution reviewer `ses_ed0760ff1ffeN8JspAtlDgmvJq` required clearer integer
Schema-versus-admission assertions, independent diagnostic reasons/paths,
locked builds, explicit pedantic for both configurations and precise metrics.
Corrections applied before final verification; no application manifest/lockfile
or runtime code changed. Probe remains outside application at
`/tmp/opencode/mimik-validator-fit-20261012`.

Locked probe checks passed: Rust1.95 build, rustfmt, strict all+pedantic Clippy
baseline/schema, dependency audit, feature graph, exact i64 and broader Schema
integers, offline HTTP/existing-file denial/local refs, independent/combined
library diagnostics. Three network-disabled runtime samples passed. Observed
Linux graph66 packages versus6 baseline, no jsonschema default features or
reqwest/rustls/tokio; executable6,186,848bytes versus381,000bytes.
Sample workload256 fields/2,048 enums/963,891-byte valid ASCII body. See
`../runtime-validator-fit.md` for commands, timing ranges and limitations.

Concrete license gate failed initially on missing original-probe license and
`borrow-or-share`0.2.4 MIT-0. Reviewer `ses_ed06ed686ffeSIxz16e0jApuZi` independently
checked published/SPDX primary licenses and approved AGPL probe declaration plus
**probe-only package-specific** MIT-0 acceptance; corrected locked notices passed.
Application license policy unchanged; actual combined-graph approval required
before integration. No unknown license was ignored or globally suppressed.

Fresh fit-result reviewer `ses_ed0693a12ffeZ0fkOTfuwu8gvQ` approved **GO for candidate
selection**, no blockers. Low clarifications: probe strings have no newlines,
and sampled cost is not exhaustive admitted-workload acceptance. Both corrected.
Probe `.take(32)` is library iteration, not an implemented bounded formatter or
proof of bounded diagnostic internals; performance is sampled, no peak RSS or
worst-case proof. No live provider, browser/runtime-feature or real-sheet test
is claimed for this documentation/probe slice.

## Handoff

Proceed with independently reviewed generic registry/admission implementation,
then definition publication and generic editor. Verify actual application feature/
license/security/resource profile and retain acceptance-matrix regressions in CI.
Authentication/history/deletion remain roadmap items; ask user about their open
policy choices when that slice becomes active. Review findings are not proof of
correctness; design/candidate approval does not certify implemented features.
