# Creation/layout/logo closure and runtime plan review

## Independent plan review

Reviewer: fresh read-only subagent `ses_ed0f19fdbffe66AI6KOuMaNsWm`.
Scope: closing creation/layout/logo work, transport regressions, CI-equivalent gates, documentation; separate proposed runtime contract.

Closure approved with medium completion conditions: test actual htmx 403/415 draft retention, verify a fresh matching deliverable, and reconcile obsolete creation/layout documentation. Low recommendation: document maintained extension selection and logo provenance.

Disposition: added permanent Rust tests for mismatched Origin, cross-site Fetch Metadata and unsupported content type; assert status/retarget headers and unchanged saved snapshot. Added Chromium scenarios that fetch the actual server response under controlled altered headers and deliver it through htmx. Chromium cannot reliably override protected headers through ordinary request continuation. The first interception callback had the wrong arity; corrected to accept Playwright's request argument. Both desktop/mobile scenarios now pass. No application defect was found in these added cases; existing transport fix remains covered by unknown/duplicate-field regressions.

README, sheet presentation and coverage now describe loaded-source creation gating, adaptive blank-sheet-informed grouping, manual note fields, legacy compatibility and unknown-versus-zero miseries.

## Runtime proposal review (blocked)

High: immutable definition hashes do not bind changing evidence/review state; specify atomic approval snapshots and readiness invalidation. High: define closed bounded untrusted-definition profile and evaluate a maintained Rust validator. Medium: separate stable edition IDs/display labels/legacy aliases, protect native IDs. Medium: specify patch/replacement, null/omission/empty/boolean/enum/integer behavior. These block runtime implementation, not current UI closure. No runtime code implemented.

## Verification

Container `mimik-ci`: `sha256:9578747a34f5e02cb7561c8e2a8903b75100b96ce1b0587d5fa4b687be0d3a9f`.
Browser container: `sha256:894398ec9fdbcabf61068d42e9d2983800fedffcc993de4a38990e08c5fb88a8`, with current browser helper mounted read-only.

2026-10-12: rustfmt, strict all+pedantic Clippy, Ruff lint/format, 41 Rust tests, 13 Python helper tests, actionlint, dependency audit, notices generation and release build passed. Updated helper desktop/mobile Chromium passed against release binary. Fresh corresponding-source packaging, unpacked artifact gates and offline rebuild results are recorded in the session handoff after execution. Local verification is not hosted CI or live provider/source approval.

Independent implementation reviewer `ses_ed0e83464ffe4AaEho3c8M0F6l` approved the bounded local prototype with two low findings: runtime README omitted creation's registration prerequisite; non-htmx transport errors falsely promised the displayed draft remained. The correction plan was independently approved. A permanent missing/false HX-Request regression failed on the retention promise before fixing it. Messages now distinguish htmx retained drafts from ordinary requests with no saved changes; status/headers remain unchanged. Runtime README clarifies registration versus review and existing-record editing. Final independent recheck confirmed both findings resolved, no new blockers, and approved implementation. Final 42 Rust/13 helper tests and strict lints passed; release rebuilt. Fresh artifact verification remains pending at this checkpoint.

### Artifact checkpoint and final handoff

`target/dist-ui-20261012/` passed `sha256sum --check SHA256SUMS`, unpacked binary `cmp` against `target/release/mimik`, network-disabled `tools/smoke_release.py`, current mounted `tools/browser_smoke.py` at both viewports, and delivered source/vendor `cargo build --release --offline --locked` with `CARGO_HOME=/tmp/empty-cargo` and a fresh target. The offline build completed in 1m39s. Packaging used `cargo package --locked --allow-dirty`, source-member verification, `cargo vendor --locked`, and `tools/package_release.py`.

Final handoff documentation is regenerated into `target/dist-ui-final-20261012/` under honest label `local-20261012-ui-reviewed` (not a Git revision). `SHA256SUMS` beside the archives is the artifact identity; hashes are not embedded here to avoid a self-referential source hash. Final gates rerun against that deliverable. Public release remains blocked by unconfirmed logo rights; hosted CI and runtime-contract approval remain unavailable/pending. Review approval is not proof of rule fidelity.
