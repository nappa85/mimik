# Owner materials independent review

## Plan review

Scope: `docs/owner-material-plan.md` and focused auth/source/import/migration code. Fresh read-only reviewer `ses_ec4195eacffeFZEJC4b8hV6SfR`; no user data or tests accessed. Initial verdict BLOCKED.

- B1/high: existing8KiB authenticated form bound cannot support20,000 correction/5,000 rationale Unicode scalars. Plan amended to route-specific320KiB and real-router maximal-input/413 regressions.
- B2/medium: SQL text-length guards do not bound byte allocation and stop at NUL. Plan amended to BLOB byte guards on every returned field and early-NUL/valid-max regressions.
- B3/medium: generic error boundary would replace draft-retaining409/422 HTML. Plan amended to narrow trusted material response marker and real-router retention/denial tests.
- Acceptance conditions incorporated: fresh post-crop transaction; final Owner plus CSRF rechecks; sidecar exact actor/kind/state/revision; coherent read linearization and immutable publication fidelity.
- Nonblocking index recommendation incorporated: `(page_id,id)` history index.

Disposition: amended plan independently approved GO by the same reviewer; no remaining plan blockers. Review findings are not proof of correctness.

## Initial implementation review

Fresh read-only reviewer `ses_ec4117de7ffeJfW4P6FsdNs4ta`, source/migration/template/test scope; BLOCKED pending fixes and coverage/delivery.

- R1/high: ruleset IDs bypassed returned-field byte guards. Permanent early-NUL oversized game/edition regression failed before fix; both present IDs now use64-byte type/byte guards.
- R2/medium: NUL review strings disagreed with SQLite text length, yielding500 instead of structured validation. Permanent leading/interior NUL regression failed before fix; shared domain rejects NUL in reviewer/rationale/correction/printed label. Further document-retention coverage pending.
- R3/medium: region blob type not guarded and polygon overflow substituted empty text. Permanent wrong-type/overflow regression failed before fix; shared region read checks BLOB/TEXT and bytes, explicit NULL errors. Additional bound coverage pending.
- R4/medium: native maxlength counts UTF-16, restricting supplementary Unicode below shared Rust scalar limits. Material form uses documented server scalar limits without conflicting native maxlength; actual browser regression pending.

Initial full-suite run exposed the future-schema fixture's old15 sentinel, now a supported schema. Retained unknown-version rejection test moves its future sentinel to16; checks remain fail-closed. Historical rewinds assert empty schema15 sidecars and remove corresponding index before reapplying migrations.

Current verification: container locked all-target/all-feature tests228 passed (111lib/3CLI/114integration), strict Clippy passed. Six material tests pass, three new review regressions established red before fix. Additional acceptance tests, independent fix approval, helper/browser/process/LAN/artifact/offline gates pending. No installed binary update or user-data access.

## Fix review and expanded acceptance

Same independent implementation reviewer rechecked R1–R4 and all authenticated returned fields, transaction authority/atomicity and templates: source-fix GO, no remaining blocking implementation defect; acceptance/delivery still pending. Reviewer evidence is static inspection, not an executed gate.

- Additional normalization defect: real-document409/422 regression failed because `normalize` still stripped the new response marker. Both document boundary and normalization now use a409/422-only trusted predicate; no cookies on retained error forms. Regression green.
- R4 browser regression retained in shared `tools/material_browser.py`: native keyboard input of20,000 supplementary correction/5,000 rationale scalars, native POST, retained conflict revision and original previews/completion. Restored old maxlength temporarily, rebuilt default release, ran real Chromium and established failure `Native form truncated Unicode scalars`; removed conflicting attributes again. Final restored-build verification pending.
- Shared browser helper runs in loopback desktop/mobile plus separate-client LAN/proxy; browser Docker COPY includes helper.
-16 material tests now pass: directory/page/unit/history cursors; offscreen completion/invalidation; authenticated publication becomes stale while approval/content stay immutable; populated14→15 source/account/private history retention; maximum400-byte edition/512-byte block/64KiB rawHTML and early-NUL history guard;413 no revision/event/sidecar changes; both completion409/422 draft retention; missing/wrong/revoked CSRF and queued expiry/revocation on both mutation kinds; final expiry atomically rolls back; fresh post-crop revocation denies returned bytes.
- Process source build smoke and both desktop/mobile Chromium passed before the temporary R4 red build; full final gates and same delivered-artifact process/browser/LAN/offline checks remain required.

## Final verification and delivery

Independent final source/helper/docs re-review by `ses_ec4117de7ffeJfW4P6FsdNs4ta`: GO, no remaining blocking implementation findings. Reviewer inspected retained acceptance and helper/operational changes; execution evidence below is implementer-run, not inferred from review.

- Final locked all-target/all-feature Rust suite:238 tests (121lib/3CLI/114integration), including16 material tests.32 helper tests; rustfmt, strict Clippy all+pedantic, Ruff lint/format and actionlint passed. One formatting check and test-function length lint caught unfinished formatting/organization and were fixed before the final successful gate; no exclusions introduced.
- Default RustSec Git fetch failed streaming its full pack; bounded retry timed out. Explicit shallow official Git clone using HTTP/1.1 fetched current commit `edb58f5974b83eb8302332821cb6d389777aa351` (`2026-10-14T22:18:32+02:00`). `cargo audit --db /tmp/rustsec-current --no-fetch --deny warnings` passed with1329 advisories/302 dependencies. Reviewer accepted this fresh-fetch transport adaptation: no stale allowance/advisory ignore or matching change.
- Fail-closed notices and default shipping release passed. Network-disabled packaging initially attempted index refresh and was stopped; `cargo package --offline --locked --allow-dirty` then passed normal package compilation verification. Source-member allowlist passed; reviewer accepted offline/locked packaging without disabling checks.
- Artifact `target/dist-materials-20261014/`: all three archive checksums passed in container and host copy. Fresh unpacked binary equals default build. Delivered source helpers exercised account/private/native/runtime/prototype/publication/source review process flows, including Owner material decisions/conflict/crops/completion and persisted canonical attribution.
- Actual delivered-source helper Chromium desktop1280×720/mobile390×844 passed restored final form: original full/crop previews, full supplementary-Unicode native entry/POST, retained escaped stale draft/revision, acceptance/completion and responsive layout; prior login/account/workspace/source flows retained.
- Same delivered binary/source helpers passed isolated separate-client LAN HTTP and configured HTTPS proxy workflows, including material browser acceptance, anonymous redirect/player denial, prior rejected-peer/forwarding-spoof controls, native Origin/Secure cookies, invitation recovery/copy/date and independent rendered QR screenshot decoding.
- Delivered source/vendor default shipping build succeeded network-disabled with empty Cargo home and fresh target:2m05.611s. No shipping runtime dependencies added.
- Installed `target/release/mimik` atomically from checksum-verified unpacked default artifact:18,210,072bytes, SHA256 `420fa9b141efeea297d216555ddf60720f2c28274329df61887ae9fe1664f510`. Running old processes require restart; additive schema15 is applied on normal startup, no reset. Verification never accessed user-owned database/passwords/cookies/.env/manuals.

Disposition: scoped Owner inspection/evidence-review implementation verified and installed. Material upload/extraction management, browser registry/definition publication, model-assisted sheet wiring and character recovery/lifecycle remain later slices. Hosted CI, physical devices/router configuration, real-manual fidelity and live model/provider verification are not claimed. These final result records follow the archived handoff; application/build/helper inputs are unchanged from the verified artifact.
