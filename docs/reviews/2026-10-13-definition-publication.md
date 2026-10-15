# Resumed source finalization / immutable wiring publication

## Reviewed scope and plan

2026-10-13 fresh read-only reviewer `ses_ecbe66d74ffeMEbC4U7yZ2wsdY`
conditionally approved resuming schema7/8, fixing findings with permanent
failing-first regressions, documenting operator boundaries, then full scoped
checks and unpacked/browser/offline gates before completion. Scope: migrations,
import/finalization, draft/version admission, snapshot/ticket/approval/withdrawal,
CLI and release helpers. Existing rusqlite transactions, serde/jsonschema and
SHA-256 remain suitable; no new infrastructure dependencies were needed.

## Findings and disposition

| Finding | Severity | Disposition / regression |
| --- | --- | --- |
| F1 revision-correct SQL page completion/event accepted unresolved units | Medium, blocking | Reproduced failure; prerequisite guards and permanent completion/event regressions. |
| F2 SQL approval accepted unrelated/missing targets and meaningless envelope | Medium, blocking | Invalid inserts failed first (15 initially accepted); relational/envelope guards plus retained read provenance; ordinary-SQL rejection variants retained. |
| F3 shipped smoke lacked publication lifecycle | Medium, blocking | Extended process smoke through source HTTP review, explicit original images, stale conflicts, version reuse/allocation, reapproval/withdrawal/restart. Two helper regressions distinguish typed conflict from unrelated failure. |
| F4 migration version read before serialization | Medium | Deterministic busy-callback/channel regression failed with existing-table error; immediate migration transaction reads version after lock acquisition. |
| F5 imported unit IDs exceed publication 512-byte bound | Low | 513-byte import failed first; importer aligned, ASCII/multibyte boundaries retained. |
| Existing schema7/8 keep weaker guards | High, blocking upgrade | Independent follow-up review identified retrofit need; additive schema9 with six failing-first reopen regressions, schema7/8 and provenance-present/absent, retained records and repeat open. |

Agent fix reports: `ses_ecbe14265ffeym3KSZNs08cLz0` (source/migration),
`ses_ecbe14241ffez1lXwrA18BnwcL` (publication). Fresh independent reviewer
`ses_ecbaa221affesGZ4dXoAJw6TuJ` approved those fixes and helper on inspection,
required schema9 before completion and approved its implementation plan.
Publication implementer also reported fresh read-only re-review approval;
coordinating review above is the recorded independent evidence.

Schema9 regression report `ses_ecba53338ffe8uBdUH7yvmv0EL`: disabling only
Store's 009 execution yielded 2 publication and 4 source retrofit failures;
restoring it passed all six. Upgrade fixtures may remove guards solely to
construct actual legacy schemas; ordinary-SQL guard regressions keep guards on.
Full suite caught a remaining registry latest-version assertion (8 vs9); updated
to9, retaining all data-preservation assertions.

Additional lifecycle regressions cover same-page versus uncaptured-page edits,
restoring effective text without reviving approval, page reconfirmation and stale
tickets without partial writes, post-withdrawal reapproval, version overflow and
snapshot UTF-8/counter budgets. Review findings are not proof of correctness.

## Final independent implementation review

Fresh read-only reviewer `ses_ecb7fc00bffeFzWRBP1pmpaVfg` approved the final
implementation, schema9 retrofit/regressions, publication helper and operator
documentation with no blocking findings. Review did not run the artifact gates;
those were performed separately below. One earlier reviewer-tool invocation
failed to return a result and is not counted as approval.

## Verification

- Full locked all-target/all-feature Rust suite: 114 tests passed.
- Helpers: 18 passed; rustfmt/strict Clippy, Ruff check/format, actionlint passed.
- Current advisory audit and actual locked license notices passed; release build
  and corresponding-source package allowlist passed.
- First command exceeded tool timeout while container continued; that incomplete
  run is not counted. Fresh fixture path used after existing-fixture rejection.
- Fresh checkpoint `target/dist-publication-20261013/`: all three archive
  checksums and unpacked binary equality passed. Unpacked process startup/native
  edit/rejection/draft/publication/source review/restart/persistence smoke passed,
  including exact typed conflicts and mapped original PNGs.
- Same checkpoint archive passed two actual Chromium scenarios at supported
  desktop/mobile viewports: htmx/persistence/overflow/source review/lookup,
  sanitizer and region preview/download/recovery scenarios retained.
- Delivered source/vendor rebuilt locked/release with networking disabled, empty
  Cargo home and fresh target in 2m44s. No host cache or application source used.
- Handoff-inclusive archive path: `target/dist-publication-final-20261013/`;
  regenerated and checked separately after these checkpoint results. The local
  revision label is a worktree checkpoint, not a Git commit or hosted CI run.

Local tests use synthetic inputs. No real source approval/provider inference,
hosted CI, campaign/account/history or generic sheet functionality certified.

## Post-packaging final artifact results

Handoff-inclusive final archives passed all checksums, unpacked binary equality,
full process/publication restart smoke and both Chromium scenarios. The final
delivered source/vendor rebuilt network-disabled with empty Cargo home/fresh
target in 2m20s. These post-packaging verification notes follow the archived
handoff; no application/build inputs changed after packaging.

Final SHA-256 identifiers:

- Runtime: `1e945de726851566abe1ac0ad70e0930426590f0ddce480f95f4e60e1a78478f`
- Corresponding source: `73b7d876daa9b69a3596cbf16af7f55864753fe26e0e0cefeaa448b00102a453`
- Dependency source: `05c5ac9ae2be58086fa983d4e7cdb7ae1e024606966f6871a1b8ab0485d52780`
