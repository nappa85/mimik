-- The edit time of a sheet's live revision for the Master's home (ROAD-8). `occurred_at` is the
-- last column of `private_sheet_history`, after `snapshot` (up to 1 MiB): reading it from the
-- table would walk the snapshot's overflow pages, so this covering index serves it. Index only,
-- no trigger or data change, idempotent. `CREATE INDEX` reads every history row once.
CREATE INDEX IF NOT EXISTS private_sheet_history_time ON private_sheet_history(sheet_id,revision,occurred_at);
PRAGMA user_version=40;
