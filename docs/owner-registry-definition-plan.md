# Owner browser registry and definition publication — plan

2026-10-15. Status: **plan draft, awaiting independent plan review.** Produced by a
read-only planning subagent; not implemented. Needs no new user policy decision.

## Current state (read-only findings)

- `src/registry.rs` transitions (`Store::register_ruleset`, `register_ruleset_alias`,
  `rulesets`) open their own `self.run` transactions, with no Owner recheck or actor.
  `rulesets()` is unbounded.
- Definition operations in `src/definitions/publication.rs` (`compare_definition`,
  `approve_definition`, `definition_version`, `withdraw_definition`) and
  `src/definitions/store.rs` (`definition_draft`, `definition_drafts`, max 100) own
  their transactions; actor/rationale are free text checked by
  `src/sources/mod.rs:review_identity` (100/5,000 chars).
- `comparison()` embeds CLI URLs (`/sources/pages/{id}/image`) in `comparison_hash`;
  those routes are not mounted in the auth router.
- Pattern to copy: `src/sources/materials.rs` `material_review_unit`/`material_complete`
  (owner_auth → csrf_admission → run_bounded_auth → with_domain_request → immediate tx
  → recheck_owner + csrf.recheck → shared tx helper → audit sidecar → recheck → commit).
  Origin is enforced globally by `post_origin` in `src/auth/mod.rs`.

## Scope

**Phase A (no `src/definitions/` edits):** Owner-only `/admin/rulesets` (paginated
list, register game+edition, exact legacy alias); read-only `/admin/definitions`
(drafts per ruleset, draft show, published version and wiring status).

**Phase B:** browser compare → approve (publish/reapprove) → withdraw with
authenticated attribution.

**Non-goals:** browser draft import/edit, manual upload/source import/finalization,
blank-sheet/model wiring, formula fields, any delete/rename/reassign/recovery/
transfer/reactivation/purge, CLI behaviour/output changes, Owner campaign access.

## Routes and templates

`src/auth/rulesets.rs`: `GET /admin/rulesets?after_game=&after_edition=` (composite
keyset cursor, 50+1; editions have no stable integer id), `POST /admin/rulesets`,
`POST /admin/rulesets/aliases`.

`src/auth/definitions.rs`: `GET /admin/definitions?game=&edition=&after=`,
`GET /admin/definitions/drafts/{id}`, `GET …/drafts/{id}/compare`,
`POST …/drafts/{id}/approve`, `GET /admin/definitions/versions/{id}`,
`GET …/versions/{id}/compare`, `POST …/versions/{id}/approve`,
`POST …/versions/{id}/withdraw`.

Merged in `router_with_sessions`. Templates `admin_rulesets.html`,
`admin_definitions.html`, `admin_definition.html`, `admin_comparison.html`;
`admin.html` "local CLI only" text replaced with links. Comparison pages link
originals through existing `/admin/materials/pages/{id}/image` and
`…/units/{unit}/image` via a new `Comparison::original_targets()` accessor; hashed
CLI URL strings stay unchanged so CLI and browser tickets are identical. Errors use
the existing mapping (NotFound→403, Conflict→409, Invalid→422, busy→503).

## Domain reuse (refactor, no copy)

1. `src/registry.rs`: extract `pub(crate) fn register_in(tx, &RegisterRuleset)` and
   `alias_in(...)`, add `rulesets_page(db, cursor) -> Listing`; CLI wrappers become
   `self.run(|db| register_in(...))`, unchanged behaviour.
2. Owner wrappers in a new `src/registry_admin.rs` (or `src/auth/registry_store.rs`),
   materials pattern, plus `registry_web_events` row.
3. Phase B: split `approve_definition` into `candidate()` (unchanged) and
   `pub(super) fn approve_in(tx, target, id, &Candidate, &ApprovalRequest)`; likewise
   `withdraw_in(tx, id, expected, actor, rationale)`. Owner wrappers in new
   `src/definitions/owner.rs`. CLI path byte-for-byte unchanged.
4. Reads get Owner variants with `recheck_owner` before/after a coherent read;
   draft listing bounded to 50+1 via `Listing`.

## Authorization, stale writes, audit

- Owner-only; player/Master/anonymous get 403. Owner gains nothing in campaigns.
- POSTs: strict Origin, durable CSRF rechecked in-tx and pre-commit,
  `deny_unknown_fields` forms; default 8 KiB body (32 KiB only if worst-case
  encoding requires); 413 tested.
- Stale: registration idempotent/immutable (conflict → 422 retaining input);
  approve requires hidden `comparison_hash` (binds revision/evidence/latest version/
  approval/withdrawal); withdraw requires hidden `expected_approval`; conflicts →
  409 TrustedDraft retaining rationale with "comparison changed, compare again";
  `originals_compared` checkbox never pre-ticked.
- Audit: browser actor is always `account:<id>`, never from input. New schema adds
  `registry_web_events` and `definition_web_events` (approval/withdrawal id, actor id,
  epoch, session digest) with insert-only triggers checking the Owner request view
  (as in migration 015); audit insert failure rolls back; no invented attribution for
  CLI rows.

## Failing-first tests

Rust: route matrix (anonymous/player/Owner); missing/wrong/expired/revoked CSRF and
foreign Origin → no mutation; revoked-while-queued; pagination with 51+ editions
and drafts; hostile `actor` field rejected; idempotent re-registration; alias
collision/native-alias rejection; stale comparison hash → 409 no write; double
withdraw → 409; CLI ticket accepted by browser and vice versa; audit trigger
failure rollback; populated schema-15 upgrade preserves approvals/reviews; real
router 409/422 escaping and retention. Existing CLI tests unchanged and green.

Browser: new `tools/admin_definition_browser.py` (pattern `tools/material_browser.py`)
in `browser_smoke.py` auth scenario: register synthetic ruleset, view CLI-prepared
draft, open originals, approve, current status, stale via unit review, reapprove,
withdraw, stale tab 409, player 403, both viewports; through the LAN/proxy gate.

## Docs

`docs/runtime-registry.md`, `docs/definition-publication.md`,
`docs/definition-drafts.md`, `docs/accounts.md`, `docs/release-scope.md`, a review
record and `TODO.md`.

## Risks and sequencing

- Hash coupling: comparison URLs stay out of scope.
- The `admit()` permit must stay held across the auth wrapper including cancellation.
- Registry editions lack a stable integer id → composite cursor.
- **Schema numbers:** concurrent retrieval work also needs a migration. Assignment
  (main session): retrieval takes 016, this slice takes 017, formulas (if they need
  a migration) take the next free number; whoever lands out of order renumbers.
- Phase A touches no `src/definitions/` files. Phase B touches only `publication.rs`
  (extraction) plus new `owner.rs`; land after the formula merge or as a pure refactor.
  Leave `admission.rs`, `decode.rs`, `mod.rs` and `schemas/definition-v1.json` to the
  formula work. Render definition fields generically.
- Likely conflict files: `src/store.rs`, `src/auth/mod.rs`, `templates/admin.html`,
  `TODO.md`, `docs/release-scope.md`.

## Lifecycle policy questions (answered 2026-10-15)

Answered by the user; see `collaboration-and-history.md` "Confirmed lifecycle decisions". Notably: re-enable keeps memberships, rejoin needs Master approval of an updated sheet, Owner restores soft-deleted sheets, purge removes the audit trail too, unreferenced rulesets/definitions may be deleted. Original options below.

Recommended default first.

1. Owner lockout/succession: (a) local CLI bootstrap of a new Owner reset token;
   (b) second designated Owner; (c) unsupported.
2. Disabled player reactivation: (a) not allowed, issue a fresh invite; (b) Owner
   re-enables with new epoch and forced reset; (c) re-enable keeps memberships.
3. Character transfer: (a) player initiates, recipient accepts, same campaign,
   history keeps prior custodian; (b) Master may initiate; (c) Owner may force.
4. Frozen sheet after rejoin: (a) player explicitly unfreezes; (b) automatic; (c) Master approves.
5. Soft delete/restore: (a) player deletes own sheet, player or current Master
   restores, hidden from lists; (b) player only; (c) Master/Owner too.
6. Physical purge: (a) Owner only, removes sheet/history/attachments/advice, audit
   keeps a tombstone, backups documented as not purged; (b) also audit; (c) no purge.
7. Purge vs "always recoverable": (a) the one explicit exception with typed scope
   confirmation; (b) only after a retention delay.
8. Registry/definition retirement: (a) no delete, later "retired" flag hides from new
   campaigns; (b) delete when unreferenced.

## Independent plan review and amendments (2026-10-15)

Fresh read-only reviewer: **GO-with-amendments**; current-state claims verified
(registry.rs:126,157,213; publication.rs:243-268; sources/web.rs:63-70;
sources/materials.rs:126-174; auth/mod.rs:447). Amendments, binding for execution:

1. (Blocking → resolved) Add an append-only `registry_events` table (kind, game,
   edition, label/alias, created_at) written by the shared registry transitions; the
   browser sidecar `registry_web_events` references it with the 015-style
   `id=max(id)` trigger. Idempotent no-op re-registration writes **no** event and
   returns the existing state; failing-first tests for both.
2. (Blocking → resolved) Definitions views move to **Phase B** (after the formula
   merge), where `*_in(tx, …)` read helpers are extracted in `definitions/store.rs`
   and `publication.rs`. Phase A = rulesets only. The CLI 100-row draft listing stays
   unchanged; the Owner gets a separate 50+1 query.
3. (Blocking → resolved by user decision 2026-10-15) The user confirmed that the Owner
   may register rulesets and publish/withdraw definitions from the browser in
   addition to the CLI, and that admin work proceeds in parallel with the
   model-harness work. The "needs no new policy decision" line is withdrawn.
   `docs/runtime-registry.md` must be updated to say so.
4. Approve/withdraw body limit ≈64 KiB (5,000 four-byte characters percent-encoded);
   boundary and 413 tests.
5. Explicit `DefinitionError` → HTTP mapping (Busy→503, NotFound→403, escaped bounded
   ValidationReport → 422, Conflict→409, Worker/Configuration→500), tested per branch.
6. Acquire `admit()` first, then every Owner step through `run_bounded_auth`; permit
   owned by the blocking closure; test that cancellation releases both semaphores;
   document CLI/browser contention (browser may 503).
7. Two definition sidecar tables (approval/withdrawal), each trigger requiring
   `actor='account:'||actor_id`, own-table `id=max(id)` and an `account_web_request` join.
8. Update downgrade helpers (administration_tests.rs:393, campaigns/tests.rs:2444) for
   new tables; add a test that migration numbers are contiguous; renumber before merge.
9. `Comparison::original_targets()` is not serialized; a test pins `comparison_hash`
   and CLI JSON output.
10. Composite cursor: both parts or neither, each `valid_id`, row-value comparison on the PK.
11. No new `|safe`; `<script>` regression tests on 409/422 retention and comparison pages.

Schema numbers: retrieval 016, this slice 017.
