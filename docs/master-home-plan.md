# Plan: latest edited sheets per campaign on the Master's home (ROAD-8, last item)


## Owner decisions and review changes (2026-11-26; these override the sections below)

Owner decisions:

1. **Names, not numbers.** A Master must tell sheets apart at a glance. Each row shows the
   sheet name, read exactly as the workspace reads it (`src/campaigns/master.rs`: size and
   length guards, "(name unavailable)" fallback), only for the rows shown (at most 10), through
   one shared helper rather than a copy of the expression. The home rule "the snapshot is not
   read" changes to "only a current Master's block reads the guarded sheet name"; say so in
   `auth-and-sessions.md`. Row label: the name; note: campaign title, custodian, badge (and
   `NPC`, `frozen`).
2. **Every recently edited sheet with its badge** (Changed, Not opened or Opened), not only
   unacknowledged ones: the latest edited sheets per campaign, as ROAD-8 says. The Master's own
   edits appear too, normally as Opened. Rows still ignore the home's time mark ("Mark as
   seen" changes nothing here). Block title: "Latest edited sheets in campaigns I run".

Review (fresh Opus, approve with changes) changes adopted:

- **Query plan (Major):** the `EXPLAIN QUERY PLAN` test runs on a populated database and
  requires `private_sheets_classes` by name and `USING COVERING INDEX
  private_sheet_history_time`, and forbids `SCAN s` and `SCAN h`; if the planner picks
  `private_sheets_custody`, add `s.is_npc IN (0,1)` (or `INDEXED BY`). The name read runs
  only after the rows are chosen.
- **Bounded statement:** `row_number()` and `count(*)` over `PARTITION BY s.campaign_id`,
  keep `n<=3`, `ORDER BY occurred_at DESC, id DESC LIMIT 11`; "more in this campaign" when
  `total>3`.
- **Badges:** state CAMP-16 exactly (only full-form and list-form saves always acknowledge;
  other own saves only when the live revision was acknowledged); test an own non-form save on
  an unacknowledged sheet (Changed).
- **Noise:** before release no backfill (`master_sheet_seen` starts empty: existing sheets
  read Not opened); say so. A removal freezing many sheets fills the block for a while; accepted.
- **Migration cost:** `CREATE INDEX` reads every history row once; note it.
- **Tests that must be able to fail:** the fixture has an NPC sheet in the player's campaign;
  acknowledge before the freeze case; a player-only home reads neither `master_sheet_seen`
  nor `private_sheet_history`; a Master of campaign 1 who plays in campaign 2 sees nothing of
  campaign 2; exact note strings (a campaign or display name containing "NPC" must not fool
  the check).
- **Links:** one link per row (the sheet); one "workspace" link per campaign (label "workspace
  (more)" when the campaign has more than 3); no duplicate URLs.
- `mark_home_seen` re-runs the read in its write transaction: bounded, accepted, documented.
- Wording: rows clear by acknowledgement (`POST /private-sheets/{id}/seen` or the module), not
  by opening; fix the `master.rs` module comment (it will no longer be the only reader of
  `is_npc`); early columns may touch one overflow page at most.
- Browser check: add the home block to `tools/master_browser.py` (both viewports, JS on and
  off: a remote edit makes the row Changed, acknowledging makes it Opened, B's badge stays).

## Summary

- **Migration:** one is needed, 040. It adds a covering index on `private_sheet_history(sheet_id,revision,occurred_at)`.
  - Why: the edit time is `occurred_at`, which sits after `snapshot` in the history row. Reading it from the table would walk the overflow pages of snapshots up to 1 MiB.
- **Where the rows go:** a new Master-only block in **Pending**. Not in "new": the per-sheet seen marks decide what shows, not the home's time mark.
- **Owner decisions needed:**
  - sheets by number plus custodian (my recommendation), or by name;
  - only unacknowledged sheets (my recommendation), or all recent ones.

## What a Master sees

A new Pending block, "Sheets to look at in campaigns I run". It is placed after "Campaigns I run" and appears only when that block has at least one campaign.

- **Scope:** sheets of the campaigns where the account is an active Master (`campaign_members.role='master' AND state='active'`). This is read in the same transaction as everything else.
- **Which sheets:** only those whose badge for this Master is **Changed** (`seen_revision < revision`) or **Not opened** (no `master_sheet_seen` row).
  - "Opened" sheets are left out. The block is a to-do list, and a row goes away when the Master opens the sheet.
  - The Master's own saves acknowledge their revision (CAMP-16), so their own edits normally do not appear. This matches "own actions are not news".
- **Order:** newest first across all campaigns, by the `occurred_at` of the sheet's live revision, then `id DESC`.
- **How many:**
  - at most 3 sheets per campaign (the existing home rule);
  - at most 10 rows in the block, with 11 fetched, so the usual "Only the first 10 are shown" note and an "All campaigns" link appear when there are more.
- **One row per sheet:**
  - **Label:** "Sheet #N", linked to `/private-sheets/N`.
  - **Note:** `<campaign title> · <custodian display name> · Changed | Not opened`. It adds `· NPC` when `is_npc` is set and `· frozen` for a departure freeze. A freeze is a new revision, so it shows as Changed, and that is intended.
  - **Time:** the live revision's `occurred_at`. It comes from the same auth clock as the home mark.
  - **Links:** "workspace" (`/campaigns/{c}/master`). On a campaign's third row, when that campaign has a fourth unacknowledged sheet, also "more in this campaign" (`/campaigns/{c}/master`).
    - This is a small, documented change to the rule "then a link to the campaign page". The workspace is the full list with badges, and it has the same current-Master admission.
- **Template:** no change needed. `home_block.html` already shows the label, href, note, time and links.

## Whether to show the sheet name

- **Recommendation: no name.** Show "Sheet #N" plus the custodian's display name (from `accounts`). The snapshot stays unread, so the home rule "a sheet is a link by number, the snapshot is not read" holds without exception.
- **Trade-off:**
  - Showing the name (the workspace reads `json_extract(snapshot,'$.name')` with length guards) is friendlier, and it leaks nothing: a current Master may read every sheet of the campaign.
  - But it makes the home read player-written snapshot content for the first time, and it touches up to 1 MiB snapshot rows. That would only be 10 rows if done after selection, but it is still a rule change in `auth-and-sessions.md`.
  - The custodian name already says who the sheet belongs to in a typical table.
  - If the owner chooses names, read them only for the 10 rows shown, in a second statement with the workspace's guards, and change the doc rule explicitly.
- **NPC label:**
  - It appears only in this Master-only block.
  - `own_sheets` (the "Campaigns I run / play in" links) stays as it is and never reads `is_npc`. A demoted Master who still holds an NPC sheet sees it there as a plain "Sheet #N".
- **Overlap with the existing blocks:**
  - "Campaigns I run" links the Master's own sheets as custodian. The new block lists any unacknowledged sheet. A Master's own NPC sheet edited by a co-Master can appear in both. That is acceptable: one block means "yours", the other "to look at".
  - The "new" panel (`player_events`) shows membership events only, from `campaign_events`. Sheet edits are not campaign events, so there is no overlap.
  - Nothing changes for players.

## Data

- **Edit time:**
  - `private_sheet_history.occurred_at`, the history row whose `(sheet_id, revision)` equals `private_sheets.(id, revision)`. It is written with the auth clock in `snapshots.rs`.
  - `private_sheets` has no time column, and `campaign_events` has no sheet edits.
- **Cost problem:**
  - `occurred_at` is the last column of `private_sheet_history`, after `snapshot` (up to 1 MiB). Reading it through the primary key makes SQLite follow the snapshot's overflow chain, up to about 256 pages per sheet.
  - The same applies to `private_sheets.is_npc`, which migration 039 appended after `snapshot`.
- **Migration 040 (`migrations/040_home_sheet_times.sql`):**
  - Contents: `CREATE INDEX IF NOT EXISTS private_sheet_history_time ON private_sheet_history(sheet_id,revision,occurred_at); PRAGMA user_version=40;`
  - Register it in `src/store.rs` (schema version and `include_str!`).
  - Index only: no trigger or data change, and idempotent like 039.
  - `is_npc` is read through the existing `private_sheets_classes(campaign_id,is_npc,id)` index, so no change is needed for it.
- **Query:** one statement.
  - `mine` = this account's active Master campaigns.
  - Join `private_sheets s` on `campaign_id`, then `private_sheet_history h` on `(s.id, s.revision)`, then accounts (custodian) and campaigns (title).
  - `LEFT JOIN master_sheet_seen m ON (master_id=?1, sheet_id=s.id)`, keeping rows where `m.seen_revision IS NULL OR m.seen_revision < s.revision`.
  - `row_number() OVER (PARTITION BY s.campaign_id ORDER BY h.occurred_at DESC, s.id DESC)`.
  - Keep `n <= 4`: the fourth row only signals "more in this campaign".
  - `ORDER BY occurred_at DESC, id DESC`, then trim in Rust to 3 per campaign and 11 rows.
  - Alternative: a loop of one query per campaign, like `player_events`. Same cost, more statements.
- **Bound:**
  - The scan covers the sheets of the Master's campaigns. These are at most 2048 rows across all 128 campaigns together, because each sheet belongs to one campaign and the `private_sheets_insert` trigger enforces the 2048 per-instance limit.
  - Each row costs index probes only: the covering history index, the `master_sheet_seen` primary key, and the `private_sheets` row's leading columns (`revision`, `state`, `custodian_id`, which come before `snapshot`).
  - Plus an in-memory sort of at most 2048 rows. No new cap is introduced.
  - Pin the plan with an `EXPLAIN QUERY PLAN` test (the pattern is in `src/campaigns/notes_value_tests.rs`). It should show `USING COVERING INDEX private_sheet_history_time` and the `private_sheets_classes` / custody index, with no full scan of the history table.

## Fit with the existing home rules

- **Transaction:** `player_read` inside the existing `player_home`. That is one read transaction through `run_campaign`, with `auth.recheck` before and after. No new admission is needed.
- **Who counts as a current Master:** decided inside the query itself, in the same snapshot as the rest of the page.
  - A demoted Master's `master_sheet_seen` rows survive demotion but are never reached.
  - The Owner never reaches `player_read` (AUTH-3).
  - Players never get the block, because it is built only when `run` is not empty. The query is also scoped by role, so it is correct either way.
- **Bounds:** one bounded query of 11 rows or fewer, 3 per campaign, with an explicit window (the 2048-sheet limit, the 128 campaigns).
- **Placement and marks:** in **Pending**, not "new".
  - `Home::new` computes `newest` from the news blocks only. These rows never move or depend on the home mark, and "Mark as seen" (`POST /home/seen`) leaves them unchanged.
  - Opening a sheet (`POST /private-sheets/{id}/seen`) is what clears a row. This follows the 039 rule "no timestamps decide unread state".
  - A home GET never acknowledges anything.

## Tests

All in `src/campaigns/home_tests.rs` unless noted, in the style of `player_and_master_home_sections`.

1. **Exclusion.** Fixture: Master A, co-Master B, player P, outsider O, and a sheet edited by P.
   - A's home lists `Sheet #N` with "Not opened".
   - P's and O's homes have no such block, and neither contains "NPC".
   - Promote P: P sees the block. Demote P again: the block and the rows are gone, although the seen rows survive.
   - The Owner's home is unchanged.
2. **Badges.**
   - Not opened becomes Opened after A's `POST /private-sheets/{id}/seen`, and the row disappears.
   - A later edit by P brings the row back as "Changed".
   - A's own full-form save does not list the sheet.
   - B's marks are independent: A's open does not clear B's row.
   - A departure freeze shows "Changed · frozen".
3. **Ordering.**
   - Sheets edited at clock 110, 130 and 120 across two campaigns appear newest first.
   - An equal-time tie is broken by id descending.
4. **Bounds.**
   - 5 unacknowledged sheets in one campaign show 3 rows plus "more in this campaign" pointing to `/campaigns/{c}/master`.
   - 12 campaigns with one changed sheet each show 10 rows plus the "Only the first 10" note.
5. **NPC and snapshot privacy.**
   - The Master's block shows "NPC" for an NPC sheet.
   - A demoted Master who holds an NPC sheet sees "Sheet #N" in "Campaigns I play in" without "NPC".
   - The sheet names ("Sheet of three") never appear on any home, if names stay out.
   - Extend `tables_read` to record `column_name`, and assert that no home reads `snapshot` from `private_sheets` or `private_sheet_history`.
6. **Tables audit.**
   - Add `private_sheet_history` and `master_sheet_seen` to `CAMPAIGN_TABLES`, so the Owner's home must not read them.
   - Assert the Master's home reads them.
7. **Migration test** (`src/store_master_tests.rs`, or a new `store_home_tests.rs`):
   - a fresh database has the 040 index;
   - an upgrade from 39 keeps the data;
   - a database rewound below 040 migrates without error.
8. **Query plan:** the `EXPLAIN QUERY PLAN` test described under Data.

**Browser check:** helpful, but not strictly needed. The block is plain server-rendered HTML with no script, and the Rust tests cover its logic. Still, `docs/release-scope.md` says the Master flows are browser-checked. A short addition to `tools/master_browser.py` would do it:
- after the player's edit, Master A's home lists the sheet as Changed;
- after A opens it, the row is gone on reload, while B's row remains;
- check at both viewports (a long note wraps at 390 px).

`tools/browser_smoke.py` needs nothing.

## Docs to update when it lands

- **`docs/auth-and-sessions.md`, "Home page":**
  - add `private_sheet_history (sheet_id, revision, occurred_at)` and `master_sheet_seen` to the index list;
  - add the 2048-sheet bound;
  - add a "Master, pending" bullet for the new block;
  - note that the rows ignore the home mark;
  - add that the "more" link goes to the workspace;
  - keep "the snapshot is not read" (or amend it if names are chosen).
- **`docs/campaigns.md`, "Master workspace and seen marks":** the home lists this Master's Changed and Not opened sheets, 3 per campaign, and the marks are read only for current Masters. Add the home to the places that read `is_npc`.
- **`docs/limitations.md`, ROAD-8:** delete the sentence "Still to do: the latest edited sheets ... campaign page only)". The rest stays as residual limits.
- **`docs/release-scope.md`, "Master workspace" bullet:** one sentence about the home block, and the browser check if it is added.

## Open questions for the owner

1. **Names:** "Sheet #N · custodian" (recommended, keeps the snapshot unread) or the sheet name (friendlier, a change to the home rule)?
2. **Filter:** only Changed and Not opened sheets (recommended, a to-do list that empties) or the latest edited sheets regardless of badge, including Opened?

### Critical Files for Implementation
- /home/marco/Progetti/mimik/src/auth/home.rs
- /home/marco/Progetti/mimik/src/campaigns/home_tests.rs
- /home/marco/Progetti/mimik/migrations/040_home_sheet_times.sql (new; registered in /home/marco/Progetti/mimik/src/store.rs)
- /home/marco/Progetti/mimik/src/campaigns/master.rs (badge logic to reuse: `MasterSheet::badge`)
- /home/marco/Progetti/mimik/docs/auth-and-sessions.md
