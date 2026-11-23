# Master view: remaining work (CAMP-14, ROAD-8)

The schema change is migration 039 (the next free number on 2026-11-23; recheck at integration).
Slice (a) is next.

## 1. Persist classification and opened revisions

- **Migration 039.** Bump `SCHEMA_VERSION` and `MIGRATIONS` (`store.rs`). Several tests rewind
  a migrated database and reopen it (`store_native_tests.rs`, `store_pdf_tests.rs`,
  `home_tests.rs`, `assistant/records_tests.rs`, `assistant/download_rows_tests.rs`), so add a
  `number == 39` guard in `Store::migrate` like 037/038: when `pragma_table_info('private_sheets')`
  already lists `is_npc`, only set `user_version=39`. Create the new table, index and triggers
  with `IF NOT EXISTS`. Add a rewound-below-039 regression test.
- **NPC classification.** Add `private_sheets.is_npc INTEGER NOT NULL DEFAULT 0 CHECK(is_npc IN
  (0,1))`, set only at creation. It is separate from `kind` (only `runtime` is created since
  038), canonical game values and snapshot encoding 1; it never enters `PrivateSnapshot`. Keep
  old snapshots, hashes and history byte-exact; rewrite no history and manufacture no revision.
  Existing sheets stay player sheets (`0`); never infer NPC status from custody or role.
  - Leave the long `private_sheets_insert` (038) and `private_sheets_update` (012) triggers
    untouched and add separate ones, so every existing restriction holds by construction:
    `private_sheets_npc_insert` (BEFORE INSERT `WHEN NEW.is_npc=1`: refuse unless the custodian
    is the request's authenticated account and an active Master of the campaign, via
    `campaign_request_auth` and `campaign_members`) and `private_sheets_npc_immutable` (BEFORE
    UPDATE `WHEN NEW.is_npc IS NOT OLD.is_npc`: refuse).
  - Index `(campaign_id,is_npc,id)` for the two lists.
- **Seen revisions.** Add STRICT `master_sheet_seen(master_id REFERENCES accounts(id), sheet_id
  REFERENCES private_sheets(id), seen_revision INTEGER NOT NULL CHECK(seen_revision>0), PRIMARY
  KEY (master_id,sheet_id), FOREIGN KEY(sheet_id,seen_revision) REFERENCES
  private_sheet_history(sheet_id,revision))` (that pair is the history table's primary key; the
  history row is written before any acknowledgement in the same transaction). No timestamps
  decide unread state.
  - Triggers: INSERT and UPDATE require `campaign_request_auth.account_id=NEW.master_id` and an
    active Master membership in the sheet's campaign; UPDATE also requires unchanged
    `master_id`/`sheet_id` and `NEW.seen_revision>=OLD.seen_revision`; DELETE is refused.
    Writes run under `with_domain_request` (`Store::run_campaign`).
  - Rows survive demotion and re-promotion, but reads and writes need current Master status.
    Storage is bounded by sheet/Master pairs under the existing caps; a future purge must remove
    these rows explicitly.
- **Unread state.** A sheet is "Changed" when its current revision is above that Master's seen
  revision, and "Not opened" (also shown as text) when there is no row. Creation, edits,
  note-only revisions and freezes count; no-op saves, list views and stream delivery do not.
  Each Master's marks are independent and never shown to another Master.
- **Own saves (owner decision 6).** Give `private_save_keeping` an explicit acknowledgement
  input. The shared save cannot use a page base: unit, switch, Undo and accepted-proposal saves
  are rebased onto the live revision L before they reach it, and per-unit bases move ahead of the
  page (`sheet-assist.js`, refused JSON answers). In the save transaction, after
  `snapshots::write` and only when the role check there says current Master:
  - **Create:** acknowledge revision 1.
  - **Full-form save:** acknowledge the new revision (the form's `revision` equals L, already
    enforced): the only page-wide witness.
  - **Unit, scalar, meta, cell, row, quick-add, switch, Undo and accepted-proposal saves:**
    `UPDATE master_sheet_seen SET seen_revision=L+1 WHERE master_id=? AND sheet_id=? AND
    seen_revision=L`, so a save acknowledges only when this Master had already seen L; otherwise
    seen is unchanged and another Master's change stays "Changed".
  - **Departure freezes** never acknowledge.
- **Projections.** `is_npc` is selected only by the Master workspace projection and the create
  path. Every existing query names its columns, so nothing leaks today; player lists, player
  campaign pages, sheet and history HTML, JSON answers and error echoes must keep omitting it
  (hidden inputs and data attributes included). The label grants no access: custody,
  current-Master reads, departure freezes and immutable pins are unchanged. Demotion neither
  transfers nor erases an NPC; its custodian keeps the existing sheet rights without seeing the
  label in player views.

## 2. Render a usable Master workspace and acknowledge opens

Slice (a) is plain HTML: acknowledgement is one click, and there is no hub or stream (the JS
auto-submit is slice (c), the post-commit seen invalidation slice (b)).

- **Workspace page.** `GET /campaigns/{id}/master`, linked from the campaign page only for
  current Masters. Authorize in the store transaction, including a final boundary after
  render preparation; Owner, player, former Master and other-campaign requests get the uniform
  refusal (`AppError::NotFound`, rendered as 403) with no sheet metadata.
  - One Store projection with its own DTO (name, custodian, revision, active/frozen state, NPC
    flag, this Master's seen revision), reusable by slice (b)'s snapshot and stream.
  - "Player sheets" and "NPC sheets" are separate sections with textual "Changed"/"Not opened"
    badges (not colour alone). The NPC section lists every classified NPC with its custodian;
    unclassified Master-held sheets stay in the player section.
  - Pagination follows `DetailPage` (`domain.rs`, `serde(deny_unknown_fields)`, nonnegative):
    cursors `players_after` and `npcs_after`, `Listing::new` (`pagination.rs`), `LIMIT 51` per
    section, stable ID order, empty states, next/first-page links. Never truncate a campaign
    to its first 50 sheets.
- **Opening a sheet.** One ordinary sheet document at a time through `GET /private-sheets/{id}`
  (`private_sheet.html`: unit forms and full form, the unsupported-pin read-only panel, history,
  read-only warnings, conflicts). No multi-sheet panels or iframes; separate browser tabs are
  allowed. No push replaces sheet values, form bases, drafts or unsaved input; sheet pages open
  no stream and show no live newer-revision notice. A return link to the Master list keeps the
  validated cursors only on the GET opened from the list; after a save (which redirects to a
  bare `/private-sheets/{id}`) it falls back to the first page.
- **Classified creation.** The choice travels on the create URL, never as a form key:
  `/campaigns/{id}/sheets/new?version=V&npc=1`, with `npc` added to the `Version` query
  (`deny_unknown_fields`). The campaign page shows "New NPC sheet" links only to current
  Masters, and the create form's action is rebuilt with the flag, so it survives draft
  operations, oversize 422, invalid drafts, 409 and the busy page without entering the closed
  key set. Validate "current active Master, self as custodian" in `private_create_context` and
  `private_badge` and again in the save transaction; without current Master status refuse with
  the uniform 403 rather than create a player sheet. Pass the flag to `snapshots::write` as an
  explicit create-only argument of its INSERT. NPCs count toward the 32-per-custodian and
  2048-instance limits (owner decision 8).
- **Acknowledging an open.** `POST /private-sheets/{id}/seen`, mounted in the top-level router
  (8 KiB body limit) outside the sheet router's `runtime` layer. Global `post_origin` already
  enforces the exact Origin and urlencoded content type; the handler runs `check_csrf`, then
  `request_auth`, then `Pairs::admit({"csrf","revision"})`, with transactional
  session/principal/role rechecks.
  - Only `read_private` with no history and status 200, for a viewer who is a current Master at
    `private_badge`'s final boundary, renders the seen form, carrying **that exact rendered
    revision**. 409/422 drafts, `rejected_editor`, the busy page, history, catalogue reads and
    failed renders never offer or perform an acknowledgement. When the Master's seen revision
    already reaches the rendered one (read in the same final boundary), the page shows
    "Opened (revision N)" instead of the form.
  - The POST validates that the revision exists for this authorized sheet, never substitutes
    the latest one, raises seen with `max(old, submitted)` (an older submission never lowers
    it; a concurrent N+1 stays changed when N is acknowledged), creates no sheet revision or
    history, and answers 303 to `/private-sheets/{id}`, never a sheet document.
  - A failed acknowledgement is visible and never reported as seen. This records the Master's
    reading preference, not proof that every value was read; it needs no signed receipts.

## 3. Publish committed changes, with bounded recovery

- Publish a Master-targeted seen invalidation after an acknowledgement commits, so that
  Master's other list tab clears its badge (never broadcast one Master's seen state to another).
- Use existing Tokio `broadcast` for a Store-owned, in-process notification hub (shared
  across Store clones, isolated between stores). Begin with one 256-entry ring carrying
  small typed hints: campaign ID, sheet ID/revision or role invalidation, and a recipient
  ID for seen changes. No snapshots, notes, sessions, credentials or page HTML in the hub.
  Filter hints before projecting data; never forward the global ring to a browser.
- Collect hints at transaction-owning write paths, publish only after successful commit
  outside the SQLite mutex using the cancellation-safe `Store::run_after` pattern. The
  existing `notes_committed` hook ignores empty note lists and is not a general sheet
  notification mechanism: extend the commit result alongside it, preserving its behavior.
  Every sheet write goes through `snapshots::write` at two sites: the shared save in
  `sheets.rs` (create/full save, scalar/cell/list/quick-add, switch, Undo and accepted
  assistant proposals) and the departure freeze in `domain.rs`; departure/removal must publish all freeze changes
  only after the entire membership-plus-freezes transaction commits. Role changes need
  post-commit invalidation too. No notifications for no-ops, stale/invalid saves or rollback.
- Treat hints as invalidations, not a durable ordered log. Callbacks can race after the
  mutex is released; always read authoritative revisions, never apply an older hint over
  newer state. Subscribe **before** the initial authorized snapshot; then drain/coalesce
  hints, with at most one projection read per stream at once and one refresh per 250 ms.
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
  Do not call `request_auth()` → `principal()` on these paths: it can flush a session
  and write a tombstone. Provide read-only admission that refuses invalid sessions without
  flushing/saving; assert session storage is unchanged on successful and refused requests.
  Keep all POST Origin/CSRF rules (REL-5) unchanged.
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
  The finite `GET .../master/snapshot` probe is an ordinary request on `AUTH_WORKERS`.
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
HTML; (b) commit hints and SSE resource/auth lifecycle; (c) external enhancement and browser/LAN
gate integration. Each slice retains meaningful regression tests; establish a failing test
before fixing any reproducible defect encountered.

- **Data/domain:** migrate empty and populated databases; history bytes,
  hashes and foreign keys unchanged; NPC immutable/default/forged-player creation guards;
  independent Masters; absent/equal/newer seen states; stale open racing N+1 and overlapping
  opens; future/nonexistent revision refusal, session binding, CSRF and revoked-role writes;
  frozen/unsupported sheets; no seen changes on list/SSE/history/failed render or no-op.
  Preserve regression coverage for every transition previously rejected by the recreated
  insert/update triggers. Test own-save acknowledgement independently for each Master.
  Assert label absence in player responses, including a demoted NPC custodian and rejected
  drafts. Check fresh/reopened persistence and 50/51-row pagination in both sections.
- **Publication/concurrency:** every production write path, note-only and empty-note edits,
  multi-sheet freezes, final-freeze failure rollback, caller cancellation after commit,
  out-of-order callbacks, subscribe/snapshot race, seen updates across same-Master tabs,
  ring overflow/lag, disconnected/slow readers, bounded memory and no producer backpressure
  on commits. Restart and missing hints recover through authoritative SQL snapshots.
- **Real HTTP streams:** assert bytes arrive after >5 s, including heartbeat; hold multiple
  streams and concurrently read/edit/login without exhausting the eight `REQUESTS` slots
  or holding definition/SQL permits. Saturate each connection limit concurrently, verify
  no overshoot and slot release after drop, timeout and error; at the per-Master cap,
  a reconnect evicts the oldest half-open stream and the newest receives a snapshot; with
  two healthy list tabs, the displaced tab shows the terminal replaced state and does not
  reclaim the slot (the newest tab stays live). A JS unit save after a refused or rebased
  answer does not acknowledge another Master's revision.
  Assert read-only events/snapshot authentication leaves session storage unchanged even
  for expired/revoked sessions; verify normalized 429 `Retry-After: 6` through the router.
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
- Update `docs/campaigns.md` (routes, metadata/seen semantics, pagination and limits),
  `docs/auth-and-sessions.md` (finite handshake versus stream lifecycle, revalidation,
  capacities and proxy example), `docs/architecture.md` (hub/module),
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
6. **Own edits:** a Master's own committed save acknowledges its revision only when based on the
   live revision.
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
