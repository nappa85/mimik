# Navigation and home page: remaining work (owner request 2026-11-13)

Goal: fewer clicks and page changes. Every signed-in page shares one top menu with drop-down
submenus, and `/` becomes a home page with what is pending, what changed since the previous
login, and quick links for the signed-in user. Scope: the authenticated app (`auth::app`). The
unauthenticated local mode (`web::app`: `/characters`, `/sources`) is not changed.

## Owner decisions (2026-11-13)

- One menu for everyone; its entries depend on the role.
- Menus by logical concept: few menus, one sub-level at most.
- Home: quick links, what changed since the previous login, and what is pending. The Owner sees
  materials waiting for review; a player sees the campaigns they play in; a Master also sees
  their campaigns with the latest edited sheets.
- Opening a menu never loads a page.
- Swapping page content without a full load (htmx boost) is not built: the most used pages
  (materials review, upload) run under the strict sheet CSP, which forbids htmx's swaps, so a
  boost would only cover the light list pages, at the cost of private pages in htmx's history
  cache and a second error and login path. Revisit if full loads are reported as slow.

## 2. Home page

- `GET /` for a signed-in account renders the home (signed-out still goes to the login).
- **What changed:** "since your login" is unreliable across devices (a phone login minutes
  before the laptop's hides everything), and `accounts` cannot carry it (its update trigger
  requires an epoch bump, which ends the account's other sessions). Instead a new STRICT table
  `account_home_marks(account_id PRIMARY KEY REFERENCES accounts, seen_at)` holds a per-account
  mark: "New since you last marked them as seen" lists items after the mark, each with its
  time, and a "Mark as seen" button (POST, CSRF) moves the mark to the newest item shown
  (never past items not shown, never backwards: a trigger refuses a smaller value). No mark
  yet: the section lists the latest items. The migration takes the next free number when it
  merges (031/032 are reserved by `docs/source-limits-plan.md`) and adds an index on
  `campaign_events (target_id, occurred_at)`.
- Sections, each bounded (at most 10 rows, a link to the full list), each its own query:
  - Owner, pending: sources with pages not yet complete (page-level counts, not a scan of
    every unit; link to the first page needing attention), uploads running or failed, open suggestions, pending invitations,
    assistant model download or trial waiting for confirmation.
  - Owner, since the previous login: uploads finished or failed, invitations redeemed
    (`account_events`), suggestions added.
  - Player: campaigns they play in, each with a link to their sheet; join requests waiting
    (theirs, and for a Master the requests to answer); since the previous login: campaign
    events whose target is the account itself (`campaign_events.target_id`), and for a
    Master the events of the campaigns they currently run.
  - Master: campaigns they run. The latest edited sheets per campaign come with the Master
    view (CAMP-14, `docs/master-view-plan.md`), which defines what a Master may see; until then
    the home links to the campaign page only.
- Nothing on the home shows sheet contents or another player's private data; counts and
  links only, checked per role like the pages they link to.
- Every home query is bounded and indexed and runs within the request deadline.
- Tests: each section for each role on a seeded database, including empty states, no mark
  yet, the 10-row bound, the mark never moving backwards or past what was shown, a player
  never seeing another player's sheet or an Owner-only count, and the Owner's home never
  running a campaign query.

## Order

Slice 1 (shared layout and menu, `docs/auth-and-sessions.md`) is done. Slice 2 is next, reviewed
and committed on the branch; the owner browser-checks after it. The Master's latest edited sheets land with the Master view. Points about the review
screen itself are collected from the owner after slice 1.
