-- The owner's reminder setting (docs/assisted-entry.md, "Reminders"; ASSIST-60): at most one row,
-- `enabled` 1 (on) or 0 (off). No row means the owner has not chosen: reminders are on for the
-- GPU path and off for the CPU path. Stored like the reply length (migration 032): by the
-- assistant page, with who changed it and when. IF NOT EXISTS so tests that rewind the version
-- can run it again.
CREATE TABLE IF NOT EXISTS assistant_reminders (
 id INTEGER PRIMARY KEY CHECK(id=1),
 enabled INTEGER NOT NULL CHECK(enabled IN (0,1)),
 changed_by INTEGER NOT NULL CHECK(changed_by>0),
 changed_at INTEGER NOT NULL CHECK(changed_at>0)
) STRICT;
PRAGMA user_version=35;
