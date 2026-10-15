# Owner browser ruleset registry (Phase A) — implementation review

Scope: Phase A of `docs/owner-registry-definition-plan.md` with its binding plan-review
amendments, as implemented on branch `admin-registry`. Covers the Owner-only
`/admin/rulesets` listing (composite cursor), registration and exact legacy alias forms;
shared `register_in`/`alias_in`/`rulesets_page` in `src/registry.rs` (CLI wrappers
unchanged); `src/registry_admin.rs`; `src/auth/rulesets.rs`; migration
`016_registry_administration.sql` (`registry_events`, `registry_web_events`); the
migration array/`SCHEMA_VERSION` refactor and contiguity test; downgrade/rewind helper
updates; templates; docs; and `tools/admin_ruleset_browser.py`. Definition views
(Phase B) and `src/definitions/` are out of scope.

Reviewer: independent read-only implementation reviewer with a fresh context (run by the
coordinating session). Verdict: **APPROVE-with-fixes, no blocking findings.** The review
does not prove correctness.

## Findings and disposition

| # | Severity | Finding | Disposition |
|---|----------|---------|-------------|
| 1 | Medium | The `e.id=(SELECT max(id) FROM registry_events)` clause in the `registry_web_events` insert trigger was untested; removing it left every test green. | Fixed. Added `ruleset_web_attribution_rejects_non_latest_event_even_for_valid_owner_request`: with a valid Owner request context, attributing an earlier CLI event is rejected and attributing the latest one succeeds (positive control). Mutation check: with the clause removed this test fails; restored. |
| 2 | Medium | The in-transaction `csrf.recheck` calls in `owner_write` were untested. | Fixed. Added `ruleset_stale_csrf_admission_rejects_registration_with_valid_owner`: the Owner binding still passes `recheck_owner`, but the durable CSRF admission no longer matches (`stale_csrf_fixture`, test-only). Ruleset and alias writes both return NotFound with no rows. Mutation check: with both rechecks removed this test fails; restored. A queued `$.csrf` payload rotation was tried first and dropped, because the session-transition trigger rejects that update. |
| 3 | Medium | The migration has to be renumbered 016 → 017 at merge and there was no checklist. | Fixed with the checklist below. |
| 4 | Low | `tests/support/schema_fixture.rs` drops `registry_events` without asserting that it is empty. | Documented as an intentional discard. CLI fixtures legitimately populate it, older schemas never had it, and remigration starts empty. The attributed sidecar `registry_web_events` is still required to be empty. |
| 5 | Low | `docs/runtime-registry.md` implied that CLI/browser contention returns 503, but the CLI does not use `AUTH_WORKERS`. | Fixed. The doc now says the browser can return 503 when its workers are busy, and that CLI and browser writes contend only through SQLite locking (5 s busy timeout). |
| 6 | Low | Direct SQL can insert `registry_events` for editions registered before history existed. | Documented, not tightened. The application never does this, and such a row cannot gain a browser sidecar unless it is the latest event during an authenticated Owner request. Database administrators can bypass triggers anyway. |

## Renumber checklist (016 → 017 after the retrieval branch's 016 lands)

- Rename `migrations/016_registry_administration.sql` to `017_…`. Set its final line to
  `PRAGMA user_version=17;` and update its header comment.
- `src/store.rs`: put retrieval's 016 entry before this one in `MIGRATIONS`, extend the
  array length and set `SCHEMA_VERSION = 17`. The contiguity test enforces file order,
  contents and the `user_version` line.
- `tests/support/schema_fixture.rs` `rewind_finalization`: include both 016 and 017
  `include_str!` lines in order. `remove_empty_accounts` must also drop or validate
  retrieval's 016 objects. Retrieval's helpers must drop `registry_web_events` and
  `registry_events`.
- Downgrade DROP lists in `src/campaigns/tests.rs` (schema-12 upgrade, about line 2446),
  `src/campaigns/administration_tests.rs` (about line 393) and
  `src/campaigns/material_tests.rs` (about line 361) must also drop retrieval's 016 objects.
- `populated_schema15_upgrade…` in `src/campaigns/registry_tests.rs` rewinds to
  `SCHEMA_VERSION - 1` (16 after the merge), which is correct as written. Rename the test
  if desired.
- Docs: change "Schema16" to "Schema17" in `docs/release-scope.md` and
  `docs/runtime-registry.md`, and remove the "planned as 017" notes.
- Databases created on this branch at `user_version` 16 are disposable. After the merge
  they fail closed (wrong objects for 16) and must be recreated.

## Verification

All checks ran in the `mimik-ci` container against this worktree, after the fixes above:
`cargo fmt --check`, strict Clippy (`--all-targets --all-features -D warnings`), the full
`cargo test --locked --all-targets --all-features`, `ruff check tools`,
`ruff format --check tools` and the Python unittest discovery. Results are recorded in the
implementer's handoff report.

The Chromium `browser_smoke.py --auth` run passed at both viewports on a container release
build, with `run_admin_ruleset_browser` and the player 403 check. It ran before these
review fixes; the fixes changed only tests, test fixtures and docs.

Initial failing-first evidence: the new tests failed to compile before the store API
existed. The five router tests failed at runtime before the routes were mounted.
