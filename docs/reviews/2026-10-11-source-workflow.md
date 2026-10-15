# Independent source-workflow reviews and disposition

## Initial implementation review

Fresh read-only reviewer `ses_ed5aa720affebWIgXpNrSjYq6E` reviewed source import/store/UI, contract/preparation, regressions and release source delivery. Verdict: blocked until the concrete issues below were resolved. No secrets/manuals/local databases were read.

| Finding | Severity | Disposition and retained regression evidence |
| --- | --- | --- |
| Sanitization removed table colspan/rowspan | High | Added table-span regression, observed failure, retained bounded numeric spans via Ammonia; executable/remote attributes still removed, unsupported spans rejected. |
| Completeness conflict refreshed token and lost draft | High | Browser-facing POST regression failed with HTTP200; preserve token/label/decision/rationale, return409/422, require explicit reassessment. |
| Preparation verified paths then consumed replacements | High | Synthetic post-verification JSON/PDF replacement tests both failed before fix. Freeze/recheck snapshots and render the verified temporary PDF; preserve verified raw JSON identity. |
| MPL dependency licenses outside policy | High release gate | License gate failed as intended. Independent review approved narrow cssparser0.38/dtoa-short0.3.5 MPL-2.0 allowance with preserved notices and corresponding sources. |
| Own source+lockfile did not robustly supply dependency source | High release gate | Cargo vendor dependency archive/config accompanies own source, preferred-form htmx source included; clean-cache network-disabled offline rebuild passed. |
| Archive verifier was only a denylist | Medium | Valid required members plus unapproved text reproduced acceptance; use maintained pathspec over root Cargo includes, reject duplicates/unknowns, retain forbidden-path checks. |
| Provenance objects could be empty | Medium | Added failing metadata regression; typed name/version/settings are required. |
| Idempotent CLI said existing source unreviewed | Low | Neutral wording correctly explains preserved review state. |
| Displayed history omitted unit/correction | Low | Structured escaped history exposes recorded unit, correction, actor/rationale, prior revision and timestamp. |

Self-check found original zero-sized PDF boxes could pass rounding tolerance. A matching-hash/raw/PNG regression reproduced acceptance; actual dimensions must now independently be positive and within bounds. The permanent test passes.

## Corrected implementation review

Fresh read-only reviewer `ses_ed38445a1ffeyoeU45o78w57ci` approved source workflow, coverage migration, browser/CI wiring and source-release implementation with no blocking defects. Reviewer independently checked archive validation behavior but did not claim to rerun full suites.

Two nonblocking findings: preferred-form htmx was included but not required by verifier (added failing missing-source regression, then required it); tracker stale (updated below). Actual archive now contains the required preferred source and the verifier passes.

## Coverage and browser plan reviews

- `ses_ed55c5d13ffebAlQPffcMnYzZ6` approved original-book coverage metadata/unknown legacy backfill, preserving all decisions/history and per-run—not guessed union—coverage. A 1-of-2-page complete-selection scenario failed its UI warning before fix; migration/backfill/persistence and full-selection cases pass.
- `ses_ed5644a97ffecevMgdVyuQxuoZ` approved bounded offline Chromium checks conditional on real htmx/header/no-navigation evidence, attempted-request/XSS observers, source image load, reviewed-only gates and explicit fail-closed prerequisite. Those conditions are implemented. Its suspected dependency-archive upload omission was disproved: existing `target/dist/*.tar.gz` already covers the archive; this is documented, not treated as a defect.
- Chromium reproduced long-name whole-document overflow (1280px viewport, 2237px scroll width). Body-level wrapping fixed it; the same permanent scenario passes at1280×720 and390×844. Source hash wrapping and long text are covered too.

## Verification at this checkpoint

All new installations were inside containers. Local equivalents passed:24 Rust tests,12 Python helper tests, Clippy all+pedantic with warnings denied, Rust/Ruff formatting/lint, actionlint, dependency audit, fail-closed license notices, source package compilation/member verification, shipped archive process smoke with source-review state transitions, two actual Chromium scenarios, and clean-cache/network-disabled rebuild using delivered dependency sources. Real manual sample import succeeded and remains unreviewed. Hosted GitHub CI/required-branch status is not available in this non-Git workspace.

This approves the documented local prototype implementation and synthetic checks, not whole-manual fidelity, real source approvals, general OCR/provider inference or official-sheet accuracy.
