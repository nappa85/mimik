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
- `--dev-owner-play` is removed in slice 1, as AUTH-3 already requires: the Owner never plays.
- Swapping page content without a full load (htmx boost) is not built: the most used pages
  (materials review, upload) run under the strict sheet CSP, which forbids htmx's swaps, so a
  boost would only cover the light list pages, at the cost of private pages in htmx's history
  cache and a second error and login path. Revisit if full loads are reported as slow.

## 1. Shared layout and menu

- One base template (`templates/layout.html`) for every page of the authenticated app: header
  with the logo (link to `/`), the menu, and the account menu on the right. Every template
  that today writes its own document or extends `material_base.html` extends it;
  `material_base.html`'s link row goes. Template blocks: title, head (page scripts), content.
- Menu (entries shown per role; a menu with no entries is not shown):
  - **Play** (player accounts): Campaigns, My sheets.
  - **Materials** (Owner): Sources and review, Upload PDF, Rulesets.
  - **Administration** (Owner): Accounts, Assistant.
  - **Account** (everyone, right side, labelled with the display name): Switch account,
    Sign out (POST form with the CSRF token).
- Drop-downs are `<details>`/`<summary>`: they open without a page load and without
  JavaScript, under every page's CSP including `SHEET_CSP` (no inline script or style). A small
  same-origin script closes an open menu on an outside click or Escape; without it the menu
  still works. Keyboard: summary is focusable, entries are links. The layout takes a `strict`
  flag: on `SheetPage` responses (sheet pages, owner material and upload pages) the nav script
  is left out, so the sheet script invariants of `src/campaigns/script_tests.rs` (no script on
  the create page and on `history/{revision}`, exactly one module on a saved sheet) hold
  unchanged; on the material pages the existing same-origin scripts may include it.
- The current section is marked (`aria-current="page"` on the entry).
- One Rust struct (`Nav`: display name, role flags, CSRF token, current section) built by one
  helper from the account summary and token the handler's authentication already loaded (no
  second session lookup), passed to every page template; no per-handler duplication.
- Pages without a session keep a logo-only header and are listed as exclusions in the route
  test: rebuilt error pages (`errors::document`), the login, enrolment and reset pages,
  catalogue notices, `private_sheet_busy`, the non-Linux upload stub.
- Sheet pages keep `SHEET_CSP` unchanged; the layout adds no inline code anywhere.
- After login, `/` is the destination (players and Owner alike). `/auth/session` redirects to
  `/`; its "Signed in" page and template go, and the links to it (`admin.html`,
  `campaigns.html`, `script_tests.rs`, the browser navigation test) are updated.
- `--dev-owner-play` and everything behind it go (`src/main.rs`, `src/auth/mod.rs`,
  `src/campaigns/domain.rs`, `snapshots.rs`, their tests, `tools/smoke_release.py`, the docs);
  AUTH-3 loses its "to be removed" clause.
- Tests: every authenticated page renders the menu with the role's entries (table test over
  the routes, Owner / player); sheet pages keep their CSP header and
  carry no inline script or style; Sign out still requires CSRF; no entry leaks a page the
  role cannot open (each entry's route answers 200 for that role); `/auth/session` redirects
  to `/`.

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

Slices 1 and 2, each reviewed and committed on a branch; the owner browser-checks after each. The Master's latest edited sheets land with the Master view. Points about the review
screen itself are collected from the owner after slice 1.
