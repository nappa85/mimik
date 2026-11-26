# Campaigns and private sheets

Players create and join campaigns and keep personal sheets in them. Every sheet
belongs to one campaign; sheet values and history are visible only to their custodian
and the campaign's current Masters. Accounts and sessions are in `auth-and-sessions.md`;
sheet editing aids (notes, Undo, per-field save) and definitions have their own docs.

Code: `src/campaigns/mod.rs` (roles, events), `domain.rs` (campaign transitions,
departure), `sheets.rs` (create, edit, history), `snapshots.rs` (read/edit
authorization, the single sheet/history writer), `web.rs` (routes). Schema:
`migrations/012_campaigns.sql`, `013_owner_administration.sql`. Templates
`campaigns.html`, `campaign.html`, `private_*.html`.

## Roles

- **Owner:** technical administrator of the instance. Admin-only: no campaign
  membership, request, sheet or history access, even with grants left from development.
  SQL triggers reject Owner membership. The human
  running the instance enrolls a separate player account to play.
- **Player:** any enabled non-Owner account. Can create campaigns, request to join, and
  hold sheets.
- **Master:** a per-campaign role. The creator is the campaign's original Master.
  Being Master in one campaign gives nothing in another.

## Campaigns and joining

Routes: `/campaigns` (directory, create), `/campaigns/{id}` (detail),
`/campaigns/{id}/join`, `/campaigns/{id}/requests/{target}`,
`/campaigns/{id}/members/{target}`.

- Creating a campaign binds an exact registered `(game_id, edition_id)`; title 1–100
  characters. Campaigns cannot be renamed, rebound or deleted.
- The directory shows only ID, title, exact ruleset and the original Master's display
  name: no roster, sheet names, counts or history for non-members.
- Join is a request. Any current Master accepts or declines; acceptance makes the
  requester a player. A declined or departed account can request again.
- Only the original creator promotes a player to Master or demotes a Master.
  Additional Masters cannot promote or demote.
- Leaving: a player or an additional Master can leave. A Master can remove a player. An
  additional Master must be demoted by the creator before being removed. The creator
  cannot leave or be removed, and there is no succession.
- Every transition writes an append-only `campaign_events` row (create, request,
  accept, decline, promote, demote, leave, remove) with actor ID and epoch.
- Memberships, requests and events are never deleted; transitions use expected
  revisions (stale → 409).

## Sheets and custody

Routes: `/campaigns/{id}/sheets/new`, `/private-sheets` (personal list),
`/private-sheets/{id}`, `/private-sheets/{id}/history[/{revision}]`, plus the
assisted-editing routes (`switch`, `undo`, `meta/{which}`, `fields/{field}`), the catalogue routes
and, for a catalogue-backed name, `suggest/{f}/{p}` and `cells/{f}/{r}/{p}/which`
([catalogues.md](catalogues.md#reading-through-a-sheet)).

- A sheet's custodian is its creator; custody never changes. There is no transfer,
  delete, restore, rollback or purge.
- One kind: **runtime**, from a published definition version of the campaign's exact
  ruleset with current wiring approval. Creating a sheet needs current approval and an
  admitted immutable pin (a create without one is refused: `?version=` names no version
  otherwise); existing sheets stay readable and editable with a warning when the approval is
  stale or withdrawn. A sheet is not an official sheet or a legality engine.
- **Read** (`authorize_read`): the custodian (also after leaving) or a current active
  Master of the campaign.
- **Edit** (`authorize_edit`): the sheet is active, its pin is supported, the campaign
  has an enabled non-Owner Master to supervise, and the editor is a current Master or the
  custodian with active membership.
- An unknown or future interpretation pin (a runtime pin of another schema) makes the sheet
  read-only: its page shows the retained name, notes and data without any form, and no save
  route admits it. Corrupt immutable content is an error.
- Account disablement blocks access but does not transfer custody or freeze sheets.

## Master workspace and seen marks

Routes: `GET /campaigns/{id}/master`, `GET /campaigns/{id}/master/snapshot`,
`GET /campaigns/{id}/master/events` and `POST /private-sheets/{id}/seen`.

- **Workspace** (plain HTML plus the live-list module): linked from the campaign page to current Masters only. It lists
  every sheet of the campaign in two sections, "Player sheets" and "NPC sheets" (custodian
  shown), each 50 per page on its own cursor (`players_after`, `npcs_after`, nonnegative,
  nothing else accepted). Each row shows a textual badge: "Changed" (revision above this
  Master's mark), "Not opened" (no mark) or "Opened". The Owner, players, former Masters and
  other campaigns' Masters get the uniform 403, authorized in the page's own transaction and
  again at its end. The page carries the strict sheet policy and loads `assets/master-live.js`, which keeps the lists current from the event stream (`assisted-entry.md`, "Master workspace script"); without it the page says updates need a reload and the Reload link, the paging links, the Open links and the one-click seen form work. A GET (reload, open) never clears a badge: the acknowledgement is the POST, sent by the sheet page's module or by one click. A sheet opened from the list shows a
  link back to it, with the list's cursors only on that first GET.
- **Snapshot** (`GET /campaigns/{id}/master/snapshot`, same two cursors and the same uniform 403
  as the page, an ordinary request inside `boundary`): JSON of the selected pages from the
  same projection, authorised read-only (no session write, see `auth-and-sessions.md`) and
  rechecked, including the current Master role, in its own read transaction. Body:
  `{"players":{"rows":[...],"next":"id"|null},"npcs":{...}}`, a row being `id`, `name`,
  `custodian`, `revision`, `state`, `npc` (bool) and `seen` (this Master's mark or null),
  all IDs and revisions as decimal strings. 401 without a valid session, 403 for everyone but
  a current Master of the campaign, 400 for bad cursors, `no-store`, no CORS. A row over
  2 KiB, a body over 256 KiB or more than 100 rows is a 500, never truncated (the limits are defence in depth: validated names and display names of at most 100
  characters keep real rows far below them, and two pages of 50 rows cannot exceed 100).
  With `?stream=<token>` (1 to 64 characters, else 400) the answer also carries
  `X-Master-Stream: live | replaced | unknown` for that token among the requesting Master's own
  streams (see the stream below); without the parameter there is no such header.
- **Event stream** (`GET /campaigns/{id}/master/events`, `src/campaigns/master_stream.rs` and
  `src/streams.rs`): the same two cursors, read-only admission and uniform refusals as the probe
  (401, 403, 400), then `text/event-stream` with `X-Accel-Buffering: no`, `no-store` and no CORS.
  Events, in the order sent: a `status` event `{"code":"live","stream":"<32 hex>"}` carrying
  `retry: 3000` and the tab's stream token (random, public, meaningful only to the same
  account's probe); a `snapshot` event with exactly the probe's JSON; then `snapshot` events
  whenever the projection changed, `:` comments every 15 s of idleness, and at the end a fixed
  `status` event, `{"code":"replaced"}` (a newer tab of the same Master took the slot: the
  client stops automatic retries and offers an explicit resume), `{"code":"closing"}` (maximum
  age, shutdown, output blocked) or `{"code":"error"}`
  (a projection over its limits or failing), after which the body ends. Revocation, expiry,
  demotion and a transient failure beyond its grace (10 s) end the body with no notice and discard anything queued. No event IDs, no replay and
  no `Last-Event-ID`: a reconnect always starts with a full snapshot. Snapshots replace the list
  (pagination metadata included); an unchanged projection is not resent.
  - **Handshake:** read-only admission, then a current-Master check in one read transaction
    (before anything is subscribed: hints reveal sheet IDs), the connection slot, the hint
    subscription, the first projection and only then the connection slot (a handshake that fails
    leaves the Master's older stream alone), all inside the request's 5 s boundary on ordinary
    request capacity; the lazy body is returned and the request permit is released as for any
    response.
  - **Slots** (RAII, released on every exit): 1 per Master account across campaigns (the newest
    stream evicts the oldest, which flushes `replaced` and closes), 16 per campaign (429, which
    `normalize` gives `Retry-After: 6`), 128 per instance (503). An eviction frees the slot in the
    same atomic step, so the caps never overshoot; the evicted stream does no further work.
  - **Producer** (one task per stream, never waiting on the body): hints only mark the stream
    dirty (`Lagged` too, and a SQL reconcile every 30 s without hints); at most one projection
    per stream at a time and one per 250 ms; a capacity-one output queue (the next snapshot is
    projected only once the body took the previous one; events that stay untaken for 1 s, with
    or without a newer snapshot waiting, close the stream with `closing`; the handshake's two
    first events count as queued until taken); maximum age 15 min; a shutdown signal makes every stream send `closing` and end.
  - **Revalidation** (`auth-and-sessions.md`): session binding, epoch, expiry, Owner exclusion
    and the active Master role are rechecked before every projection (at its start, at its end
    and once more after the commit), every 5 s while idle and at the session's absolute expiry,
    on a separate 2-permit FIFO worker pool with a 1 s deadline per operation. Busy or timed-out
    operations hold the output and retry every 250 ms; no successful recheck for 10 s closes the
    stream.
  - **The replaced probe:** the tab remembers its token. After a stream error it probes
    `GET .../master/snapshot?stream=<token>`; `replaced` means it must not reclaim a slot, so a
    displaced tab does not fight the newest (only the last 8 replaced tokens of an account are
    remembered, in memory: `unknown` after that or after a restart, and the tab may reconnect).
- **Hints** (`src/hub.rs`): after a commit, outside the SQLite mutex and by the committing
  task (so also when the caller is cancelled), every write publishes small typed hints on
  the store's broadcast ring: `Sheet{campaign,sheet,revision}` for each committed sheet
  revision (every shared save, creations, each departure freeze), `Role{campaign,account}`
  for a join, promotion, demotion, leave or removal, and `Seen{campaign,recipient,sheet}`
  when a Master's mark actually changed (the seen POST and the own-save acknowledgement).
  A departure publishes its role and all freeze hints only after the whole transaction. Nothing
  is published for no-ops, refused, stale or rolled-back writes. Hints are invalidations, not
  an ordered or durable log: a consumer reads authoritative state from SQL, `Feed` shows a
  consumer only its campaign's hints (and its own account's seen hints), and a subscriber that
  falls more than 256 hints behind gets `Lagged` and must take a fresh snapshot. The event
  stream consumes them as invalidations (above). Account disable, credential reset and epoch
  changes (in-process administration and the out-of-process CLI) publish no hint: consumers rely
  on the per-stream recheck and the periodic SQL reconcile for them.
- **NPC classification:** `private_sheets.is_npc` (migration 039), set only at creation by a
  current Master for a sheet they hold. The choice is the create URL's `npc=1`
  (`/campaigns/{id}/sheets/new?version=V&npc=1`, never a form key; only 0 or 1), offered as
  "New NPC sheet" links on the campaign page to current Masters only. It is checked again at
  the create page, the final read boundary and the save transaction (without current Master
  status: 403, never a player sheet), and by the `private_sheets_npc_insert` trigger. The
  `private_sheets_npc_immutable` trigger refuses any change. The flag grants no access:
  custody, current-Master reads and departure freezes are as for any sheet, and a demoted
  Master's NPC stays in their custody. It is read only by two Master-only readers, the workspace
  projection and the home's block of latest edited sheets, and never appears in the
  snapshot, history, player lists, sheet pages or JSON answers.
- **Seen marks** (`master_sheet_seen`, one row per Master and sheet, the newest acknowledged
  revision; the composite foreign key keeps it on an existing history revision; triggers
  require the request's own account to be a current Master of the sheet's campaign, refuse
  lowering, moving and deleting). Rows survive demotion; reads and writes need current Master
  status. Marks are independent per Master and never shown to another. The Master's home
  (`auth-and-sessions.md`, Home page) lists the latest edited sheets of the campaigns the account
  currently runs, 3 per campaign and 10 in all, each with this Master's badge (every recent
  edit, not only unacknowledged ones); the marks are read only for current Masters.
- **Acknowledging an open:** a plain current read (status 200, no history) of a current Master
  shows a one-click form carrying the rendered revision, or "Opened (revision N)" when the mark
  already reaches it. The POST (CSRF, session, exactly `csrf` and `revision`) refuses
  non-Masters (403), malformed revisions (400) and revisions the sheet's history lacks (422),
  raises the mark to `max(old, submitted)`, writes no sheet revision and answers 303 to the
  sheet. 409/422 re-renders, history, the busy page and listings never offer or write it.
- **Own saves:** after the sheet write, in the same transaction and only for a current Master:
  a create acknowledges revision 1; a full-form or list-form save (its base is the rendered
  revision) acknowledges its new revision; every other save (unit, scalar, metadata, cell, row,
  quick-add, switch, Undo, accepted proposal; they are rebased onto the live revision L)
  moves the mark from L to L+1 only if it was L. Departure freezes never acknowledge.

## Departure and freeze

Leaving or being removed sets the membership inactive and, in the same transaction,
freezes every active sheet of that custodian in that campaign: one `freeze` revision per
sheet, values, campaign and custody unchanged. Afterwards the custodian and current
Masters have read-only access to the sheet and its history; nobody can edit it.
Rejoining does not unfreeze. Any storage failure rolls back the departure and all its
freezes.

## Revisions, history and conflicts

- Each sheet has a revision; every successful create, edit and freeze appends an immutable
  `private_sheet_history` row. A no-op save (nothing changed) writes no revision (D-U); an
  edit whose data is unchanged but that records a new change note still writes one. Each row
  holds the full resulting snapshot, hash, operation, actor ID and epoch, one-way session digest and time. Live
  row and history row are linked by deferred foreign keys; history cannot be updated or
  deleted.
- Snapshots are host-canonical JSON with a versioned, domain-separated hash used to
  detect corruption (not protection against a database owner).
- The runtime definition ID/version/hash/schema are immutable per sheet.
- Every write rechecks session, deadline, account epoch, grants and expected revision in
  one IMMEDIATE transaction. A stale full-form save returns 409, an invalid one 422;
  both re-render the escaped draft with its submitted revision.
- A timeout can follow a committed write: reload the sheet or history before retrying.
- History pages list 50 revisions per page; each revision shows what changed against the
  previous one. History is never evicted.

## Legacy records

Legacy records keep their identity and notes at `/characters/{id}`; no new ones are created and
none carries a sheet. A pre-release database that still holds native sheet data is refused at upgrade, not converted.

The `--no-auth` prototype's character tables are separate from `private_sheets`. They
are never assigned to campaigns or shown in authenticated mode, and the prototype cannot
read private tables. Moving them in needs an explicit migration policy.

## Limits

Enforced in both Rust and SQL triggers (migration 012), as of now:

| Resource | Limit |
| --- | --- |
| Campaigns per instance | 128 |
| Campaigns created per account | 32 |
| Distinct members plus requesters per campaign | 64 |
| Retained sheets per custodian per campaign (frozen included) | 32 |
| Private sheets per instance | 2,048 |
| Stored snapshot | 1 MiB (1,048,576 bytes), canonical |
| Sheet name / notes | 100 / 20,000 characters |
| Directory, roster, requests, versions, sheet lists, Master workspace sections, history | 50 per page, independent cursors, Next only when another row exists |
| Ruleset selector | first 50 registered pairs |
| Body: campaign forms | 8 KiB |
| Body: sheet create/full save | 5 MiB (`SHEET_POST_BYTES`); draft echo up to 1 MiB |
| Body: per-field and per-cell save | 256 KiB; new row 5 MiB; row action, Undo/switch 8 KiB; other sheet routes 1 MiB |
| Request time | 5 s (whole request, including departure freezes) |

Definition-side bounds (256 fields, 32 groups, text 8,000 and multiline 20,000
characters) are in the definitions docs. A 32-sheet near-1 MiB departure measured about
3 s, within the 5 s timeout (see `results.md`).

The caps limit counts, not storage: history grows without a retention limit. They are
due to become safety-only limits (see limitations).
