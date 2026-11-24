ALTER TABLE private_sheets ADD COLUMN is_npc INTEGER NOT NULL DEFAULT 0 CHECK(is_npc IN (0,1));
-- Master view (CAMP-14, ROAD-8). The first line is the only statement that is not idempotent:
-- `migrate()` skips it when a database rewound below 039 already has the column.
-- `is_npc` is the Master's classification of a sheet, set only at creation and never changed.
-- It is kept apart from `kind`, from the canonical snapshot (history bytes and hashes stay
-- exact) and from every player-facing projection. Existing sheets stay player sheets.
CREATE INDEX IF NOT EXISTS private_sheets_classes ON private_sheets(campaign_id,is_npc,id);
-- The two triggers below add to `private_sheets_insert` (038) and `private_sheets_update` (012),
-- which stay as they are, so every restriction there still holds.
CREATE TRIGGER IF NOT EXISTS private_sheets_npc_insert BEFORE INSERT ON private_sheets WHEN NEW.is_npc=1 BEGIN
 SELECT CASE WHEN NOT EXISTS(SELECT 1 FROM campaign_request_auth a JOIN campaign_members m ON m.account_id=a.account_id AND m.campaign_id=NEW.campaign_id AND m.role='master' AND m.state='active' WHERE a.account_id=NEW.custodian_id) THEN RAISE(ABORT,'npc creation requires a master') END;
END;
CREATE TRIGGER IF NOT EXISTS private_sheets_npc_immutable BEFORE UPDATE ON private_sheets WHEN NEW.is_npc IS NOT OLD.is_npc BEGIN
 SELECT RAISE(ABORT,'npc classification is permanent');
END;
-- The newest revision each Master has opened, per sheet. No timestamps decide unread state: a
-- sheet is "Changed" when its revision is above this row, "Not opened" when there is no row.
-- The composite foreign key keeps the row on a revision that exists (the history row is
-- written before any acknowledgement in the same transaction). Rows survive demotion; reads
-- and writes need current Master status. A future purge must remove them explicitly.
CREATE TABLE IF NOT EXISTS master_sheet_seen (
 master_id INTEGER NOT NULL REFERENCES accounts(id),
 sheet_id INTEGER NOT NULL REFERENCES private_sheets(id),
 seen_revision INTEGER NOT NULL CHECK(seen_revision>0),
 PRIMARY KEY(master_id,sheet_id),
 FOREIGN KEY(sheet_id,seen_revision) REFERENCES private_sheet_history(sheet_id,revision)
) STRICT;
CREATE TRIGGER IF NOT EXISTS master_sheet_seen_insert BEFORE INSERT ON master_sheet_seen BEGIN
 SELECT CASE WHEN NOT EXISTS(SELECT 1 FROM campaign_request_auth a JOIN private_sheets s ON s.id=NEW.sheet_id JOIN campaign_members m ON m.campaign_id=s.campaign_id AND m.account_id=a.account_id AND m.role='master' AND m.state='active' WHERE a.account_id=NEW.master_id) THEN RAISE(ABORT,'seen auth required') END;
END;
CREATE TRIGGER IF NOT EXISTS master_sheet_seen_update BEFORE UPDATE ON master_sheet_seen BEGIN
 SELECT CASE WHEN NEW.master_id IS NOT OLD.master_id OR NEW.sheet_id IS NOT OLD.sheet_id OR NEW.seen_revision<OLD.seen_revision OR NOT EXISTS(SELECT 1 FROM campaign_request_auth a JOIN private_sheets s ON s.id=NEW.sheet_id JOIN campaign_members m ON m.campaign_id=s.campaign_id AND m.account_id=a.account_id AND m.role='master' AND m.state='active' WHERE a.account_id=NEW.master_id) THEN RAISE(ABORT,'seen auth required') END;
END;
CREATE TRIGGER IF NOT EXISTS master_sheet_seen_delete BEFORE DELETE ON master_sheet_seen BEGIN
 SELECT RAISE(ABORT,'seen marks are permanent');
END;
PRAGMA user_version=39;
