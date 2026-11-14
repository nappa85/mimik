# Navigation and home page: remaining work (owner request 2026-11-13)

Goal: fewer clicks and page changes. Every signed-in page shares one top menu with drop-down
submenus, and `/` becomes a home page with what is pending, what changed since the account last
marked it as seen, and quick links for the signed-in user. Scope: the authenticated app (`auth::app`). The
unauthenticated local mode (`web::app`: `/characters`, `/sources`) is not changed.

## Owner decisions (2026-11-13)

- One menu for everyone; its entries depend on the role.
- Menus by logical concept: few menus, one sub-level at most.
- Home: quick links, what changed since the home mark, and what is pending. The Owner sees
  materials waiting for review; a player sees the campaigns they play in; a Master also sees
  their campaigns with the latest edited sheets.
- Opening a menu never loads a page.
- Swapping page content without a full load (htmx boost) is not built: the most used pages
  (materials review, upload) run under the strict sheet CSP, which forbids htmx's swaps, so a
  boost would only cover the light list pages, at the cost of private pages in htmx's history
  cache and a second error and login path. Revisit if full loads are reported as slow.

## Order

Slice 1 (shared layout and menu) and slice 2 (home page) are built and described in
`docs/auth-and-sessions.md`; the owner browser-checks after slice 2. Still pending: the Master's
latest edited sheets per campaign on the home, which come with the Master view (CAMP-14,
`docs/master-view-plan.md`, which defines what a Master may see). Until then the home links to
the campaign page only. Points about the review screen itself are collected from the owner
after slice 1.
