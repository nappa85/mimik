-- Assisted sheet step 3b (docs/assisted-entry.md, "Bundles"): the note op `reminded` (a reminder's
-- applied change, told apart from an interpretation's `confirmed`) and the index of a sheet's notes
-- by bundle number, so a bundle's members and `max(bundle)` need no scan of the sheet's notes.
-- SQLite cannot alter a CHECK, so the table is rebuilt as migration 021 did: same columns, rows,
-- index and triggers, with the op CHECK that also admits `reminded`. The triggers are created after
-- the copy, so existing notes (whose revisions are no longer live) are kept as they are.
CREATE TABLE private_sheet_notes_v34 (
 sheet_id INTEGER NOT NULL CHECK(sheet_id>0),
 revision INTEGER NOT NULL CHECK(revision>0),
 address TEXT NOT NULL CHECK(length(CAST(address AS BLOB)) BETWEEN 6 AND 256 AND instr(address,char(0))=0 AND (address GLOB 'field:?*' OR address GLOB 'cell:?*/?*/?*' OR address GLOB 'meta:?*' OR address GLOB 'row:?*/?*')),
 op TEXT NOT NULL CHECK(op IN ('literal','sum','sign_literal','switched','unresolved','erased','confirmed','reverted','reminded')),
 raw TEXT NOT NULL CHECK(length(raw)<=200 AND length(CAST(raw AS BLOB))<=800 AND instr(raw,char(0))=0),
 before TEXT CHECK(before IS NULL OR (length(CAST(before AS BLOB))<=80 AND json_valid(before) AND json_type(before)='object')),
 after TEXT NOT NULL CHECK(length(CAST(after AS BLOB))<=80 AND json_valid(after) AND json_type(after)='object'),
 bundle INTEGER CHECK(bundle IS NULL OR bundle>0),
 target INTEGER CHECK(target IS NULL OR target>0),
 PRIMARY KEY(sheet_id,revision,address),
 FOREIGN KEY(sheet_id,revision) REFERENCES private_sheet_history(sheet_id,revision)
) STRICT;
INSERT INTO private_sheet_notes_v34(sheet_id,revision,address,op,raw,before,after,bundle,target) SELECT sheet_id,revision,address,op,raw,before,after,bundle,target FROM private_sheet_notes;
-- Dropping the table drops its index and triggers first, so no delete trigger fires.
DROP TABLE private_sheet_notes;
ALTER TABLE private_sheet_notes_v34 RENAME TO private_sheet_notes;
-- The current note of an address is its latest one.
CREATE INDEX private_sheet_notes_current ON private_sheet_notes(sheet_id,address,revision);
-- A bundle's members, and the latest bundle number of a sheet (`max(bundle)`, its last entry).
CREATE INDEX private_sheet_notes_bundle ON private_sheet_notes(sheet_id,bundle);
CREATE TRIGGER private_sheet_notes_insert BEFORE INSERT ON private_sheet_notes BEGIN
 -- Replacement guard: INSERT OR REPLACE deletes without firing delete triggers.
 SELECT CASE WHEN EXISTS(SELECT 1 FROM private_sheet_notes WHERE sheet_id=NEW.sheet_id AND revision=NEW.revision AND address=NEW.address) THEN RAISE(ABORT,'notes are insert-only') END;
 SELECT CASE WHEN NOT EXISTS(SELECT 1 FROM private_sheets s JOIN private_sheet_history h ON h.sheet_id=s.id AND h.revision=s.revision AND h.hash=s.hash WHERE s.id=NEW.sheet_id AND s.revision=NEW.revision AND s.state='active' AND h.operation IN ('create','edit')) THEN RAISE(ABORT,'a note must name the live revision') END;
 SELECT CASE WHEN NOT EXISTS(SELECT 1 FROM private_sheet_history h JOIN campaign_request_auth a ON a.account_id=h.actor_id AND a.epoch=h.actor_epoch WHERE h.sheet_id=NEW.sheet_id AND h.revision=NEW.revision) THEN RAISE(ABORT,'note auth required') END;
 SELECT CASE WHEN (SELECT count(*) FROM private_sheet_notes WHERE sheet_id=NEW.sheet_id AND revision=NEW.revision)>=256 THEN RAISE(ABORT,'note limit per revision') END;
END;
CREATE TRIGGER private_sheet_notes_update BEFORE UPDATE ON private_sheet_notes BEGIN SELECT RAISE(ABORT,'notes are insert-only'); END;
CREATE TRIGGER private_sheet_notes_delete BEFORE DELETE ON private_sheet_notes BEGIN SELECT RAISE(ABORT,'notes are permanent'); END;
PRAGMA user_version=34;
