# Master view: remaining work (CAMP-14, ROAD-8)

Slices (a) and (b) are built (migration 039, NPC classification, seen marks, own-save
acknowledgement, the plain-HTML workspace, classified creation, the click-only seen POST, the
notification hub, read-only admission, the JSON snapshot probe and the SSE event stream with its
connection slots, worker pool and revalidation); the reference is `docs/campaigns.md` ("Master
workspace and seen marks") and `docs/auth-and-sessions.md` ("Event stream lifecycle"), and
CAMP-15/CAMP-16 in `docs/decisions.md`. Slice (c) remains. Section numbers 5 and 6 keep their
original numbers.

## 1. What the built slices leave for the next ones

- **Built:** slice (a) (workspace, seen marks, NPC classification) and slice (b): the
  notification hub (`src/hub.rs`), hints published after every commit, read-only admission
  (`auth::read_only_request_auth`), the JSON snapshot probe and the event stream
  (`GET /campaigns/{id}/master/events`, `src/streams.rs`, `src/campaigns/master_stream.rs`); the
  reference is `docs/campaigns.md` ("Master workspace and seen marks") and
  `docs/auth-and-sessions.md`. The client contract the browser module (section 5) follows: a
  `status` event `live` carries the tab's stream token; `replaced` is terminal (no automatic
  retry, explicit resume only); after any stream error the module probes
  `GET .../master/snapshot?stream=<token>` and, when the `X-Master-Stream` header says
  `replaced`, does not reconnect.
- **Acknowledgement.** Slice (c) auto-submits the seen form with JS (the page's `#seen-form`
  carries the exact rendered revision). Without JS a Master who edits by units without
  clicking Seen keeps the badge (owner decision 6); slice (c)'s auto-submit removes this in
  practice.
- **Opening a sheet** stays one ordinary document per tab: no multi-sheet panels, no push that
  replaces values, form bases or drafts, no stream on sheet pages, no live newer-revision notice.
- **Pages.** The workspace uses the default page policy today; slice (c) gives it the strict
  `SheetPage` policy with its script.

## 5. Add progressive enhancement under the existing strict CSP

- First try to make the htmx SSE extension work (owner decision): bundled like DEP-5, loaded
  only on the Master page, with `SHEET_CSP` unchanged (no Trusted Types policy, no
  `unsafe-*`). Its HTML swaps go through `innerHTML`-type sinks that `trusted-types 'none'`
  blocks, so it likely fails; demonstrate that in a browser check and record it. Never weaken
  the policy for it without asking the owner. Only if it cannot work: a small external module
  around the browser's maintained native `EventSource`: application-specific JSON-to-DOM
  projection using `createElement`/`textContent`, fixed attributes and locally constructed
  links/forms only. No custom SSE parser, HTML sinks, eval, inline script/style or remote
  asset. Record the demonstrated gap and selection in a new DEP decision before coding;
  check any new Rust feature/dependency's maintenance, licence and runtime cost too.
- Give Master documents, assets and JSON the existing strict `SheetPage` policy; same-origin
  EventSource fits `default-src 'self'`. Keep the editor module separate; share small
  acknowledgement helpers only where useful. Maintain focus, scroll and stable row identity
  when updating lists; use a polite status region for connection state, not an announcement
  for every field change. Do not replace a focused Open link.
- Show “Connecting / Live / Reconnecting; list may be stale” and an always-working Reload
  link. Native EventSource retries transport failures; on `error`, close it and schedule
  a bounded, jittered retry (3–30 s) after an authorized finite snapshot probe at
  `GET /campaigns/{id}/master/snapshot` (same cursors, projection and read-only admission).
  That probe
  distinguishes 401 (sign-in link), 403 (access ended; clear Master list), 429/503 (back off)
  and offline failure, since EventSource does not expose HTTP status. Stop automatic retries
  on lost access; explicit retry after sign-in is allowed. Close on page exit and rebuild
  after back/forward cache restoration, without duplicating streams.
- Without JS (or when the module fails), use server-rendered pages, the plain Reload link,
  pagination, GET Open links and the one-click seen form; explain that updates require
  reload. Prefer this over meta refresh, which can discard forms or disturb focus. GET list reloads never clear
  badges. JavaScript blocked by CSP must degrade to the same usable page.

## 6. Verification, integration and reference updates

Implement in reviewed slices: (a) schema/classification and seen/open contracts with plain
HTML (built); (b) commit hints and SSE resource/auth lifecycle (built); (c) external enhancement and
browser/LAN gate integration. Each slice retains meaningful regression tests; establish a failing test
before fixing any reproducible defect encountered.

- **Data/domain (slices a and b, built):** `campaigns/master_tests.rs`, `store_master_tests.rs`,
  `master_hub_tests.rs` and `master_stream_tests.rs`. Still to add with slice (c): stale open
  racing N+1 and overlapping opens at the publication level, seen updates across same-Master
  tabs, and the browser-level label-absence checks in player and outsider network responses. A
  JS unit save after a refused or rebased answer must not acknowledge another Master's
  revision. Two healthy list tabs: the displaced tab shows the terminal replaced state and does
  not reclaim the slot (the newest tab stays live), against the real server in the browser.
- **Browser:** add a focused synthetic Master suite to the existing artifact/browser gate,
  reusing `tools/browser_smoke.py`'s `no_overflow`, `CSP_PROBE`/`csp_clean` and account fixtures
  (as the review/catalogue suites do). Run all four combinations **1280×720 / 390×844 ×
  JS enabled / disabled**, with two independent Master contexts, a player and an outsider.
  Create runtime NPCs through the normal Master-only UI, including refused
  creation retaining the choice only for a still-authorized Master. Create/edit/freeze
  from another context and observe list push without navigation; badges
  clear only for the opening Master and reappear for a later revision. Exercise runtime and
  unsupported-pin opens, both NPC sections/pages, keyboard focus, long names/bidi/escaping, empty
  state and no document overflow. Keep an unsaved editor draft during a remote edit;
  existing 409/422 retention still works. Test disconnect/reconnect/server restart, access
  revocation, duplicate-stream prevention, JS failure and the plain reload/open flow.
  Assert strict CSP headers/no violations, no unexpected external traffic, and no NPC label
  or forbidden payload in player/outsider network responses, not merely invisible DOM.
- Test direct LAN HTTP, loopback and the configured root-origin HTTPS proxy: Host and
  Secure-cookie selection, forwarded headers never trusted, actual incremental delivery (not just
  headers), Wi-Fi sleep or proxy idle closure recovering by full snapshot. No public-internet or
  physical-device certification implied.
- Extend `tools/lan_client.py`/`lan_smoke.py` and their helper tests with separate-client
  push/reconnect, denied subnet/wrong Host/foreign Origin, HTTPS Secure cookies and proxy
  flushing/idle behavior. Wire the focused suite into `.github/workflows/ci.yml` and
  allowlisted container/source inputs; run against the unpacked artifact, not a dev binary.
  Preserve the fail-closed gate: formatting, strict Clippy, locked Rust tests, helper
  lint/tests, audit/notices, release packaging/startup/edit/persistence and browser/LAN jobs.
- Update `docs/assisted-entry.md` (open acknowledgements/editor interaction),
  `docs/decisions.md` (owner choices/DEP selection), assets/notices if applicable, and
  `docs/release-scope.md` (actual verified Master scope and gates). Update ROAD-8 and any
  demonstrated residual limits; remove finished sections of this plan as slices land.

## Owner decisions (2026-11-05)

1. **Opened:** GET + `/seen` with the rendered revision, auto-submitted with JS, one click
   without.
2. **Existing sheets:** all start as player sheets; NPC classification only at creation.
3. **NPC grouping:** one shared NPC section for all campaign Masters, custodian shown.
4. **Client:** try the htmx SSE extension first under the unchanged strict CSP; native
   EventSource plus the small module only if htmx cannot work.
5. **Budget:** 1 live stream per Master (newest tab wins, the replaced tab stops automatic
   retries), 16 per campaign, 128 per instance, subject to measurement.
6. **Own edits** (refined 2026-11-23, owner): create and full/list-form saves acknowledge their
   new revision; every other own save moves seen from L to L+1 only when this Master had already
   acknowledged the live revision L (unit bases run ahead of the page, so base = L is no witness).
7. **Departure:** today's freeze rule stays for NPCs in v1.
8. **NPC allowance:** NPCs share the 32-per-custodian and 2048-instance sheet limits.

## Risks to resolve before release

Proxy buffering, background
tab suspension and HTTP/1 browser connection limits can delay updates; fallback status/reload
must remain truthful. Preserve the distinction between a sheet opened at N and currently at
N+1; never let convenience updates discard drafts or acknowledge unseen data. New metadata
must not leak via shared templates/JSON after a role change. Only checks actually run can
establish release scope.
