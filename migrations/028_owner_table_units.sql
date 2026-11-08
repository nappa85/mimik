-- Owner units may be Text or Table (a table the owner builds from selected units). Every other
-- identity, state, revision and finalization guard of 024 stays exactly as it was.
DROP TRIGGER source_unit_origin_insert;
CREATE TRIGGER source_unit_origin_insert BEFORE INSERT ON source_units WHEN
 (NEW.origin='extracted' AND NEW.block_id GLOB 'owner:*') OR
 (NEW.origin='owner' AND (instr(NEW.block_id,char(0))<>0 OR NEW.block_id NOT GLOB 'owner:[1-9]*' OR substr(NEW.block_id,7) GLOB '*[^0-9]*'
 OR CAST(CAST(substr(NEW.block_id,7) AS INTEGER) AS TEXT) IS NOT substr(NEW.block_id,7)
 OR length(CAST(NEW.block_id AS BLOB))>25 OR NEW.state<>'unreviewed' OR NEW.correction<>'' OR NEW.revision<>1 OR NEW.kind NOT IN ('Text','Table')
 OR NOT EXISTS(SELECT 1 FROM source_pages p JOIN source_finalizations f ON f.source_id=p.source_id WHERE p.id=NEW.page_id)))
BEGIN SELECT RAISE(ABORT,'Invalid owner unit identity'); END;
-- An owner table has no review event of its own (it stays unreviewed), so this append-only link
-- records which batch created it: the batch's first rejection event names the units it replaced.
CREATE TABLE source_owner_tables (
 unit_id INTEGER PRIMARY KEY REFERENCES source_units(id),
 operation_id TEXT NOT NULL CHECK(length(operation_id) BETWEEN 1 AND 100 AND instr(operation_id,char(0))=0),
 first_event_id INTEGER NOT NULL UNIQUE REFERENCES source_review_events(id)
) STRICT;
CREATE TRIGGER source_owner_tables_insert BEFORE INSERT ON source_owner_tables
 WHEN NOT EXISTS(SELECT 1 FROM source_units u WHERE u.id=NEW.unit_id AND u.origin='owner' AND u.kind='Table')
 OR NOT EXISTS(SELECT 1 FROM source_review_events e WHERE e.id=NEW.first_event_id AND e.operation_id=NEW.operation_id AND e.state='rejected')
BEGIN SELECT RAISE(ABORT,'Owner table link must name an owner table and its batch'); END;
CREATE TRIGGER source_owner_tables_update BEFORE UPDATE ON source_owner_tables BEGIN SELECT RAISE(ABORT,'Owner table links are append only'); END;
CREATE TRIGGER source_owner_tables_delete BEFORE DELETE ON source_owner_tables BEGIN SELECT RAISE(ABORT,'Owner table links are retained'); END;
PRAGMA user_version=28;
