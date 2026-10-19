# Review: catalogue route status pages after user decisions (2026-10-19)

**Scope.** Commit `3ef98b4` on branch `catalogue-decisions` (parent master `eece89b`), recorded in
`docs/catalogue-plan.md` §19. A missing entry key or unknown type inside a linked, readable,
sealed catalogue answers 404 with a meaningful page; every other refusal answers one uniform 403
"Catalogue unavailable" page with an explanation; malformed addresses answer 400 with an
explanation before any lookup. Bodies pass the auth layer through the `CatalogueNotice` marker.

**Reviewers.**

| Reviewer | Verdict |
|---|---|
| Sol (`--chain main`) | MERGE-READY |
| Sonnet (fresh subagent) | MERGE-READY |
| Third chain | unavailable: Space Bunny and DeepSeek timed out at preflight, Muse returned server errors. Proceeded with the two main reviews, as AGENTS.md allows |

Both reviews were static. Neither found an existence oracle: the 404 variants are built only after
both transactions' checks (session, grant, pin, link, sealed version), the final session recheck
still runs, and refusal bodies carry only the requester's own sheet id.

## Findings and disposition

| # | Source | Severity | Finding | Disposition |
|---|---|---|---|---|
| 1 | Sol | Minor | An invalid search expression (`q=**`) wins over an unknown `type`, so that request answers the 422 search problem, not the unknown-type 404 | Accepted as is: reveals nothing (all checks passed) and the browse form lists the types. Recorded here; revisit with the assisted-sheet work |
| 2 | Sol | Minor | The revocation-gap test drives `private_catalogue` directly; the new 404 paths are not exercised through the router during a revocation | Follow-up test (TODO) |
| 3 | Sonnet | Minor | The 400 check runs before authentication, so a logged-out browser following a malformed link sees the 400 page, not the login redirect | Accepted: existing order, static body |
| 4 | Sonnet | Minor | A session that ends mid-request gets the 403 page; an already-ended session gets 401 | Accepted; §19 and the test comment state it |
| 5 | Sonnet | Note | `normalize` keeps `Set-Cookie` on these pages like any plain error page | No change |

**Verification.** Author gate in plan §19 (Monday 2026-10-19 before 09:00): fmt, pedantic Clippy,
568 Rust tests passed, ruff, tools unittest 751 OK, release build, `browser_smoke.py --auth` and
`smoke_release.py --auth` passed. Not run: audit, about, packaging, LAN smoke.
