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
PRAGMA user_version=28;
