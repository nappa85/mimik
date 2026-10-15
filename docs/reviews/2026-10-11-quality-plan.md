# Independent adversarial review: quality/CI plan

Reviewer: separate read-only general subagent, fresh context, session `ses_ed5f88121ffeChRaki0Nj6zND0`. Reviewed 2026-10-11 worktree, before quality-gate implementation. No secrets/manuals inspected. No Git revision exists in this workspace; package version 0.1.0 and reviewed paths identify scope.

## Reviewed plan

Mandatory independent plan/change-set review, permanent failing-first regressions, locked Rust formatting/lint/tests, Python helper lint/tests, dependency audit, release packaging, unpacked-binary HTTP/restart/persistence smoke, and fail-closed deployment gate. Review also sampled existing web authority checks and helper provenance/schema validation.

## Findings and dispositions

| ID | Severity | Finding | Disposition / permanent checks |
| --- | --- | --- | --- |
| Q1 | Blocking for local deployability | Foreign Host accepted; Origin compared against the same untrusted Host enables a DNS-rebinding path | Add authority validation on every route. `tests/request_authority.rs` reproduced foreign GET/read acceptance and missing/foreign authority acceptance before fix (2 tests failed). Both now pass; existing HTTP fixtures use supported local authority. |
| Q2 | High for optional provenance | A manifest omitting source.json's hash passes verification, although reviewer consumes source.json | Require a supported manifest schema and source.json hash using jsonschema. Added missing-hash regression; observed failure before fix. Changed-artifact regression retained. |
| Q3 | Medium | Reviewer schema permits missing nullable keys and unknown extra keys | Use maintained jsonschema closed schema; retain block-reference domain check. Added missing/extra-field regression; observed failure before fix. |
| Q4 | Release-plan blocker | Runtime archive alone is not a complete corresponding-source delivery plan | Cargo's explicit source-package allowlist plus accompanying source archive, revision/checksums, and build instructions. Generated Rust notices via cargo-about; htmx/SQLite notices preserved. |
| Q5 | CI-plan blocker | Dependent aggregate jobs can be skipped instead of proving deployability | Final `always()` job requires both checks/artifact-smoke success; smoke downloads/unpacks the exact archive. Branch protection is documented as an external setup requirement. |

Reviewer approved the overall direction only for an explicit Linux loopback-local prototype scope after blockers are resolved. It required body/revision/state assertions in htmx smoke tests, not HTTP success status alone; both identity and stats persistence and bidirectional shared revisions must be checked.

## Review verification limitations

Reviewer ran existing prebuilt Rust binaries (11 passed), not a fresh source build. System Python lacked dotenv (one helper dependency error); synthetic fixtures confirmed Q2/Q3. No browser/live API/Marker/hosted CI checked. Findings are not proof of correctness.

## Implementation verification

Failing-first runs: `cargo test --locked --test request_authority` failed 2 tests; Python suite failed the 2 new provenance/schema tests. Following fixes, 13 Rust tests and 6 helper tests passed. Additional release/lint/audit/smoke results and independent implementation review are recorded separately before completion.
