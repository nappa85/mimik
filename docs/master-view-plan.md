# Master view: remaining work (CAMP-14, ROAD-8)

Slice (a) is built (migration 039, NPC classification, seen marks, own-save acknowledgement,
the plain-HTML workspace, classified creation and the click-only seen POST); the reference is
`docs/campaigns.md` ("Master workspace and seen marks") and CAMP-15/CAMP-16 in
`docs/decisions.md`. Slices (b) and (c) remain. Section numbers 3 to 6 keep their original
numbers (sections 1 and 2 were slice (a)).

## 1. What the built slices leave for the next ones

- **Built:** slice (a) (workspace, seen marks, NPC classification) and slice (b1): the
  notification hub (`src/hub.rs`), hints published after every commit, read-only admission
  (`auth::read_only_request_auth`) and the JSON snapshot probe; the reference is
  `docs/campaigns.md` ("Master workspace and seen marks") and `docs/auth-and-sessions.md`.
- **Acknowledgement.** Slice (c) auto-submits the seen form with JS (the page's `#seen-form`
  carries the exact rendered revision). Without JS a Master who edits by units without
  clicking Seen keeps the badge (owner decision 6); slice (c)'s auto-submit removes this in
  practice.
- **Opening a sheet** stays one ordinary document per tab: no multi-sheet panels, no push that
  replaces values, form bases or drafts, no stream on sheet pages, no live newer-revision notice.
- **Pages.** The workspace uses the default page policy today; slice (c) gives it the strict
  `SheetPage` policy with its script.

## 3. Consume the hints, with bounded recovery (what b2 adds)

- Subscribe **before** the initial authorized snapshot (`Store::feed(campaign, account)`);
  then drain/coalesce hints, with at most one projection read per stream at once and one
  refresh per 250 ms. Hints are invalidations, not an ordered log: callbacks can race after the
  mutex is released and arrive out of revision order, so always read authoritative revisions and
  never apply an older hint over newer state. `Feed` filters by campaign and seen recipient only
  and is not an authorization: admit a current Master of the campaign (and recheck it) before
  creating one, because sheet hints reveal sheet IDs and revisions, NPC sheets included.
- On `RecvError::Lagged`, discard deltas and send a fresh authorized snapshot of the
  selected list pages. Do the same on every connection/reconnection. Use no SSE event IDs
  or replay promise in v1; add no `Last-Event-ID` handling. A full snapshot replaces list
  state, including pagination metadata; off-page changes are found on navigating to their page.
- Reconcile from SQL every 30 s even without hints, including seen state. This repairs
  missed publication and out-of-process account changes; restart loses only hints, not
  revisions or seen state. Keep the supported serving model one process per database.
  Multi-server fan-out would need a separately reviewed durable/shared mechanism.

## 4. Serve SSE without borrowing ordinary-request capacity

- Add read-only `GET /campaigns/{id}/master/events` with the same two validated page
  cursors as the workspace. Use Axum `response::sse::{Sse,Event,KeepAlive}` and existing
  Tokio/futures-util stream primitives; do not hand-write SSE framing or add a broker.
  Return `text/event-stream` and `X-Accel-Buffering: no`; existing `normalize` supplies
  `Cache-Control: no-store` and `nosniff`. Keep comments every 15 s and advertise `retry: 3000`.
- Connect via same-origin cookie authentication, existing peer/Host admission and current
  campaign Master admission. If GET supplies Origin, require the exact expected origin;
  reject cross-site fetch metadata and expose no CORS permissions. Normal EventSource GETs
  without Origin must work. No CSRF token in URLs/headers and no session creation/rotation,
  lifetime extension, last-seen write or other mutation on events or snapshot GETs.
  Admit through `auth::read_only_request_auth` (built in b1), never `request_auth()`. Keep all
  POST Origin/CSRF rules (REL-5) unchanged.
- Capture the existing `RequestAuth` session binding in the finite handshake. Its
  `deadline` is the absolute session expiry, **not** the outer 5 s request budget; reuse
  `recheck` rather than minting or extending it. Recheck active session/tombstone, enabled
  account, exact epoch/expiry, Owner exclusion and current active Master role in a single
  read transaction before each data projection, every 5 s while idle, and at session
  expiry. Recheck after potentially blocking work, immediately before admitting output.
  On revocation/expiry/demotion or definitive revalidation failure, close fail-closed and
  discard queued data. On transient busy/operation timeout, hold output and retry within
  the fixed grace below; nothing is emitted without a fresh successful recheck.
  Already admitted network bytes cannot be retracted; document this
  boundary. A demoted custodian still cannot keep a Master stream.
- Preserve `auth::boundary`: it times `next.run(request)` and drops `REQUESTS`' permit
  when the response is constructed, before streaming the body. Complete initial auth,
  subscription and first bounded projection within its existing 5 s timeout; return
  a lazy body promptly. Do not await the stream inside the handler, move the request
  permit into it, exempt a URI prefix from the boundary, or collect the SSE body in
  middleware. Keep `DefaultBodyLimit` and network/session/error normalization for the
  handshake. Mount outside the sheet router's `runtime` layer: no definition permit.
- Independent RAII connection slots: **1 per Master account across campaigns (owner
  decision), 16 per campaign, 128 per instance**, subject to contention measurement before
  adoption. Only a Master's most recent list tab is live; other tabs can reload.
  At the per-Master cap, atomically cancel/evict that Master's oldest stream and admit
  the newest (prefer replacing the same session's oldest stream where practical). The evicted
  stream receives a fixed `replaced` status event before closing; the client enters a terminal
  “Live updates moved to another tab” state with an explicit “Resume live updates” action and the
  Reload link, and never retries automatically (if the notice is lost, the client's snapshot
  probe reports `replaced` for that tab's stream token, so it still does not reclaim a slot).
  Count replacement atomically without overshooting campaign/instance caps. Release on
  every disconnect, handshake timeout/error, producer failure and shutdown. Refuse with
  429 + `Retry-After: 6` for campaign saturation, 503 for global saturation; verify the
  normalized response through the full router (`normalize` forces 6 on every 429).
  The stream owns only these slots and a bounded producer task, never an auth-worker,
  database mutex, transaction or ordinary-request permit while waiting for events.
- Give stream projections/rechecks their own **2-permit semaphore**, separate from the
  non-queuing 16-permit `AUTH_WORKERS` pool. Never draw stream work from that pool. Streams
  acquire it with a fair (FIFO) bounded wait counted inside each operation's 1 s deadline.
  The finite `GET .../master/snapshot` probe (built in b1) is an ordinary request on `AUTH_WORKERS`.
  Each short operation has a 1 s deadline. On busy/timeout hold output, coalesce work and
  retry with bounded delay; close if no recheck succeeds for **10 s** (initial admission
  still obeys the 5 s handshake budget). No output without a fresh successful recheck.
  A timed-out blocking worker retains its permit until it exits; do not replace it while
  running. Check cancellation/deadline after mutex acquisition before SQL. Keep at most
  one outstanding operation per stream, with no unbounded spawn or retry queue.
  Stream tasks run outside `boundary`'s task-locals: detect busy explicitly and wrap SQL
  guards needing request context in `with_domain_request`. Before adopting any numerical
  caps, measure SQLite contention, recheck latency/grace expiry and ordinary read/edit/login
  availability at those caps; reduce caps or tune the 2–4 worker budget from that evidence.
- Emit named `snapshot` JSON events containing only the authorized selected pages:
  IDs, bounded display labels (sheet/custodian names), revisions, state, NPC classification,
  this Master's seen revision and pagination cursors. No sheet content/values, history,
  notes, account security data or other campaign metadata. Encode i64 IDs/revisions as
  decimal strings to avoid JavaScript precision loss. Limit each row to 2 KiB and each
  event to 256 KiB, at most 100 visible rows; reject projection corruption/overflow visibly,
  never truncate silently. Status/shutdown events carry only fixed public codes.
- Give the producer a capacity-one output queue, coalescing invalidations into a dirty
  flag. Run revalidation independently of body polling; a non-reading socket must not
  suspend revocation checks indefinitely. Cancel on drop; close on an output queue blocked
  for 1 s, definitive rebuild failure or a maximum connection age of 15 min; transient busy
  rebuilds use the same output hold/grace policy. Ensure idle tasks and
  counters cannot leak when cancellation races with a worker. On server termination/cleanup
  failure, signal all streams, best-effort fixed shutdown notice, then close; bounded drain
  must not keep the process alive. Reconnect obtains a complete fresh snapshot.
- Test direct LAN HTTP, loopback and the configured root-origin HTTPS proxy. Preserve
  Host and Secure-cookie selection, never trust forwarded headers. Document proxy buffering
  off, compression off for SSE and read timeout above the heartbeat interval (e.g. 60 s).
  Check actual incremental delivery, not just headers; Wi-Fi sleep/proxy idle closure must
  recover by full snapshot. No public-internet or physical-device certification implied.

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
HTML (built); (b) commit hints (b1, built) and SSE resource/auth lifecycle (b2); (c) external enhancement and
browser/LAN gate integration. Each slice retains meaningful regression tests; establish a failing test
before fixing any reproducible defect encountered.

- **Data/domain (slice a, built):** `campaigns/master_tests.rs` and `store_master_tests.rs`.
  Still to add with slices (b) and (c): stale open racing N+1 and overlapping opens at the
  publication level, seen updates across same-Master tabs, no seen changes on SSE, and the
  browser-level label-absence checks in player and outsider network responses.
- **Publication/concurrency:** b1 tests the write paths, rollback, cancellation, lag and the
  subscribe/snapshot order at the hub level (`campaigns/master_hub_tests.rs`). Still to add with
  b2: out-of-order callbacks and disconnected/slow readers through real streams; restart and
  missing hints recovering through authoritative SQL snapshots.
- **Real HTTP streams:** assert bytes arrive after >5 s, including heartbeat; hold multiple
  streams and concurrently read/edit/login without exhausting the eight `REQUESTS` slots
  or holding definition/SQL permits. Saturate each connection limit concurrently, verify
  no overshoot and slot release after drop, timeout and error; at the per-Master cap,
  a reconnect evicts the oldest half-open stream and the newest receives a snapshot; with
  two healthy list tabs, the displaced tab shows the terminal replaced state and does not
  reclaim the slot (the newest tab stays live). A JS unit save after a refused or rebased
  answer does not acknowledge another Master's revision.
  Assert read-only events authentication leaves session storage unchanged even
  for expired/revoked sessions (the snapshot probe's test is built); verify normalized 429 `Retry-After: 6` through the router.
  Test separate stream-worker saturation, SQL contention, output hold/retry/grace expiry
  and recovery without ordinary-load/login 503s from `AUTH_WORKERS` exhaustion.
  Test role loss, logout, CLI reset/disable/epoch changes and absolute expiry mid-stream,
  both idle and backpressured;
  no subsequently admitted sheet metadata. Include SQL busy/failure, no receiver, shutdown,
  reconnect after restart and full-page replacement.
  Use paused clocks/barriers for timing tests; do not collect an infinite response body.
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
- Extend `tools/lan_client.py`/`lan_smoke.py` and their helper tests with separate-client
  push/reconnect, denied subnet/wrong Host/foreign Origin, HTTPS Secure cookies and proxy
  flushing/idle behavior. Wire the focused suite into `.github/workflows/ci.yml` and
  allowlisted container/source inputs; run against the unpacked artifact, not a dev binary.
  Preserve the fail-closed gate: formatting, strict Clippy, locked Rust tests, helper
  lint/tests, audit/notices, release packaging/startup/edit/persistence and browser/LAN jobs.
- Update `docs/campaigns.md` (the stream route and limits),
  `docs/auth-and-sessions.md` (stream lifecycle, revalidation,
  capacities and proxy example),
  `docs/assisted-entry.md` (open acknowledgements/editor interaction),
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

Load from per-stream revalidation may compete with ordinary writes on the single SQLite
mutex; tune caps/coalescing from measured bounded-worker tests, not channel capacity alone.
Slow-reader cancellation must not depend on Hyper polling the body. Lost post-commit hints
and reordered callbacks must not produce persistent stale lists. Proxy buffering, background
tab suspension and HTTP/1 browser connection limits can delay updates; fallback status/reload
must remain truthful. Preserve the distinction between a sheet opened at N and currently at
N+1; never let convenience updates discard drafts or acknowledge unseen data. New metadata
must not leak via shared templates/JSON after a role change. Only checks actually run can
establish release scope.
