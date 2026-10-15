# Owner material workspace — implementation plan

Status: independently approved plan, implemented, independently source/fix reviewed, verified and installed. Executed gates and scoped limitations: `docs/reviews/2026-10-14-owner-materials.md`.

## Supported slice

Provide `/admin/materials` to the signed-in technical Owner: imported source directory, source page directory, original full-page/region images, safe extracted units, append-only unit decisions and whole-page completeness review. Keep exact source/ruleset identities, original coverage and finalization visibly distinct from reviewed evidence. Link from account administration.

Local CLI retains registry registration, prepared import, finalization and definition publication. Uploaded manuals, model-assisted blank-sheet wiring and lifecycle policies are later slices. No new library is needed: Axum, Askama, rusqlite, ammonia, PNG cropping and shared keyset pagination already cover this slice. Existing source SQL mutations become shared transaction helpers rather than parallel browser implementations.

## Authorization and attribution

All document and image reads use the existing captured `RequestAuth`, bounded auth workers and a coherent transaction with current Owner/session/epoch/deadline rechecks before and after reading. Region rendering additionally rechecks after bounded CPU work, before returning bytes. Owner authority does not confer campaign/sheet access; players/Masters cannot administer materials.

POSTs require strict existing Host/Origin boundary, durable CSRF admission, current Owner request and expected source revision. Mutations use an immediate transaction under `with_domain_request`, rechecking Owner and CSRF in the transaction and immediately before commit. Derive actor as a stable authenticated account label (`account:<id>`); never accept browser-supplied reviewer identity.

Add schema15 `source_web_events`, an immutable sidecar keyed to the existing source-review event, retaining authenticated Owner ID/epoch/session digest. A narrow insertion trigger checks the existing current Owner request view, exact newest event, `account:<id>` actor, supported kind and current state/revision correspondence. Capture event ID immediately after event insertion. Shared mutation returns the event ID; browser mutation and sidecar insertion commit atomically. Explicitly recheck both Owner and submitted durable CSRF after the sidecar insertion and before commit. Local CLI/prototype historical labels/events remain valid and receive no fabricated browser attribution. Audit failure rolls back the entire decision. Index history by `(page_id,id)`.

Read transactions linearize at coherent-snapshot admission: their final check catches elapsed expiry but cannot observe revocation committed later by another connection. Post-crop authority recheck uses a fresh bounded storage transaction with the original captured request, never the pre-crop snapshot. Crop semaphore stays inside the blocking task through completion, including cancellation.

## Bounds and presentation

Source/page/unit/history directories use independent nonnegative exclusive ID cursors and authorized 50+1 rows via `Listing`. Source directory uses aggregate imported/complete counts instead of loading every source's pages. Page detail paginates units and history independently, while completion checks all units in SQL, never only the visible page. Original full-page comparison stays visible on each unit page.

Use SQL size guards before copying every returned text field, including event actor/rationale/correction and metadata. Measure UTF-8 bytes with `length(CAST(column AS BLOB))`, including embedded NUL, with explicit type/NULL handling; never use SQLite text length as an allocation bound. Match admitted fields (raw HTML64KiB, block ID512bytes, correction80,000bytes, rationale20,000bytes, reviewer400bytes, v2 edition400bytes) and guard PNG/prepared JSON separately. Return explicit errors on overflow, never empty substitutions or truncated evidence. Region keeps existing24MiB input/4MiB output/semaphore limits. Material review POSTs override the outer8KiB authenticated body bound with320KiB, enough for worst-case percent-encoded20,000 correction plus5,000 rationale four-byte Unicode scalars and bounded metadata. Test real-router layer precedence and413/no mutation; deny unknown form fields and retain shared scalar validation.

Extract shared page/image/unit-read and source-review transition helpers where useful; prototype behavior remains tested. Browser templates use exact `/admin/materials` links, bundled source preview retry/date adapters, escaped metadata and only ammonia-sanitized HTML as safe. Errors preserve correction/rationale/expected revision and show unsaved/stale draft guidance, without silently updating revision. Add a narrow trusted material-editor response marker recognized by the existing error boundary; only authorized server-rendered draft-preserving409/422 responses carry it. Recovery reads recheck current Owner authority and distinguish submitted from stored revision. No automatic approval or confidence-based review.

## Verification and delivery

Permanent tests: anonymous/player/Owner route matrix including both images; bounded pagination with51+ records/independent cursors; canonical attribution and hostile actor fields; missing/wrong/expired/revoked CSRF; queued expiry/revocation; stale revisions; completeness over offscreen units; unit changes invalidate completeness and publication evidence; sanitized extraction; audit failure rollback/append-only guards; populated schema14 upgrade preserving histories/private records; original image limits and post-crop authority recheck; maximum Unicode form plus oversized413; early-NUL text allocation guards and maximum valid imported fields; real-router409/422 escaped draft/expected-revision retention. Reproducible issues discovered during work receive failing-first regressions.

Independent implementation/fix review gates precede completion. Run fmt, strict Clippy, locked all-feature/all-target Rust tests, helper lint/tests, actionlint, audit, notices, default release and packaging allowlist. Extend synthetic process/browser fixtures for Owner material flows, player denial, mobile original comparison, edit/conflict/completion/restart and actual LAN/proxy routing. Verify delivered-artifact startup/edit/persistence, source/vendor offline rebuild and install only the verified default binary. Never access user-owned data or credentials during verification.
