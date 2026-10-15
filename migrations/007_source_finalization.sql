CREATE TABLE source_finalizations (
    source_id INTEGER PRIMARY KEY REFERENCES sources(id) ON DELETE RESTRICT,
    prepared_hash TEXT NOT NULL CHECK (instr(prepared_hash,char(0))=0 AND length(CAST(prepared_hash AS BLOB))=64 AND prepared_hash NOT GLOB '*[^0-9a-f]*'),
    extraction_hash TEXT NOT NULL CHECK (instr(extraction_hash,char(0))=0 AND length(CAST(extraction_hash AS BLOB))=64 AND extraction_hash NOT GLOB '*[^0-9a-f]*')
) STRICT;
CREATE TABLE source_page_hashes (
    page_id INTEGER PRIMARY KEY REFERENCES source_pages(id) ON DELETE RESTRICT,
    source_id INTEGER NOT NULL REFERENCES sources(id) ON DELETE RESTRICT,
    image_hash TEXT NOT NULL CHECK (instr(image_hash,char(0))=0 AND length(CAST(image_hash AS BLOB))=64 AND image_hash NOT GLOB '*[^0-9a-f]*')
) STRICT;
CREATE TRIGGER source_page_hash_insert BEFORE INSERT ON source_page_hashes
BEGIN
    SELECT CASE WHEN EXISTS(SELECT 1 FROM source_page_hashes WHERE page_id=NEW.page_id)
      OR EXISTS(SELECT 1 FROM source_finalizations WHERE source_id=NEW.source_id)
      OR NOT EXISTS(SELECT 1 FROM source_pages WHERE id=NEW.page_id AND source_id=NEW.source_id)
      THEN RAISE(ABORT,'Invalid or finalized page hash identity') END;
END;
CREATE TRIGGER source_finalize_insert BEFORE INSERT ON source_finalizations
BEGIN
    SELECT CASE WHEN EXISTS(SELECT 1 FROM source_finalizations WHERE source_id=NEW.source_id)
      OR NOT EXISTS(SELECT 1 FROM sources s WHERE s.id=NEW.source_id AND s.total_pages>0 AND s.bundle_hash=NEW.prepared_hash
        AND json_valid(s.prepared_json) AND json_extract(s.prepared_json,'$.extraction_sha256') IS NEW.extraction_hash
        AND (json_extract(s.prepared_json,'$.contract_version')=1 OR EXISTS(SELECT 1 FROM source_rulesets r WHERE r.source_id=s.id))
        AND (SELECT count(*) FROM source_pages p WHERE p.source_id=s.id)>0
        AND NOT EXISTS(SELECT 1 FROM source_pages p WHERE p.source_id=s.id AND (typeof(p.revision)<>'integer' OR p.revision<=0))
        AND NOT EXISTS(SELECT 1 FROM source_units u JOIN source_pages p ON p.id=u.page_id WHERE p.source_id=s.id AND (typeof(u.revision)<>'integer' OR u.revision<=0))
        AND (SELECT count(*) FROM source_pages p WHERE p.source_id=s.id)=(SELECT count(*) FROM source_page_hashes h WHERE h.source_id=s.id)
        AND NOT EXISTS(SELECT 1 FROM source_pages p WHERE p.source_id=s.id AND NOT EXISTS(SELECT 1 FROM source_page_hashes h WHERE h.page_id=p.id AND h.source_id=s.id)))
      THEN RAISE(ABORT,'Invalid or incomplete source finalization') END;
END;
CREATE TRIGGER source_finalize_update BEFORE UPDATE ON source_finalizations BEGIN SELECT RAISE(ABORT,'Source finalizations are immutable'); END;
CREATE TRIGGER source_finalize_delete BEFORE DELETE ON source_finalizations BEGIN SELECT RAISE(ABORT,'Source finalizations are retained'); END;
CREATE TRIGGER source_page_hash_update BEFORE UPDATE ON source_page_hashes BEGIN SELECT RAISE(ABORT,'Page hashes are immutable'); END;
CREATE TRIGGER source_page_hash_delete BEFORE DELETE ON source_page_hashes BEGIN SELECT RAISE(ABORT,'Page hashes are retained'); END;
CREATE TRIGGER sources_finalized_insert BEFORE INSERT ON sources
WHEN EXISTS(SELECT 1 FROM sources s JOIN source_finalizations f ON f.source_id=s.id WHERE s.id=NEW.id OR s.bundle_hash=NEW.bundle_hash)
BEGIN SELECT RAISE(ABORT,'Finalized source identity cannot be replaced'); END;
CREATE TRIGGER sources_finalized_update BEFORE UPDATE ON sources WHEN EXISTS(SELECT 1 FROM source_finalizations WHERE source_id=OLD.id)
 OR EXISTS(SELECT 1 FROM sources s JOIN source_finalizations f ON f.source_id=s.id WHERE s.id=NEW.id OR s.bundle_hash=NEW.bundle_hash)
BEGIN SELECT RAISE(ABORT,'Finalized source artifacts are immutable'); END;
CREATE TRIGGER sources_finalized_delete BEFORE DELETE ON sources WHEN EXISTS(SELECT 1 FROM source_finalizations WHERE source_id=OLD.id)
BEGIN SELECT RAISE(ABORT,'Finalized source identity is retained'); END;
CREATE TRIGGER pages_finalized_insert BEFORE INSERT ON source_pages
WHEN EXISTS(SELECT 1 FROM source_finalizations WHERE source_id=NEW.source_id)
 OR EXISTS(SELECT 1 FROM source_pages p JOIN source_finalizations f ON f.source_id=p.source_id WHERE p.id=NEW.id)
BEGIN SELECT RAISE(ABORT,'Finalized page membership cannot change'); END;
CREATE TRIGGER pages_finalized_update BEFORE UPDATE ON source_pages
WHEN EXISTS(SELECT 1 FROM source_finalizations WHERE source_id=OLD.source_id)
 AND (NEW.id IS NOT OLD.id OR NEW.source_id IS NOT OLD.source_id OR NEW.pdf_page IS NOT OLD.pdf_page OR NEW.png IS NOT OLD.png)
 OR (NEW.source_id IS NOT OLD.source_id AND EXISTS(SELECT 1 FROM source_finalizations WHERE source_id=NEW.source_id))
 OR (NEW.id IS NOT OLD.id AND EXISTS(SELECT 1 FROM source_pages p JOIN source_finalizations f ON f.source_id=p.source_id WHERE p.id=NEW.id))
BEGIN SELECT RAISE(ABORT,'Finalized page artifacts are immutable'); END;
CREATE TRIGGER pages_finalized_delete BEFORE DELETE ON source_pages WHEN EXISTS(SELECT 1 FROM source_finalizations WHERE source_id=OLD.source_id)
BEGIN SELECT RAISE(ABORT,'Finalized page identity is retained'); END;
CREATE TRIGGER units_finalized_insert BEFORE INSERT ON source_units
WHEN EXISTS(SELECT 1 FROM source_pages p JOIN source_finalizations f ON f.source_id=p.source_id WHERE p.id=NEW.page_id)
 OR EXISTS(SELECT 1 FROM source_units u JOIN source_pages p ON p.id=u.page_id JOIN source_finalizations f ON f.source_id=p.source_id WHERE u.id=NEW.id)
BEGIN SELECT RAISE(ABORT,'Finalized unit membership cannot change'); END;
CREATE TRIGGER units_finalized_update BEFORE UPDATE ON source_units
WHEN EXISTS(SELECT 1 FROM source_pages p JOIN source_finalizations f ON f.source_id=p.source_id WHERE p.id=OLD.page_id)
 AND (NEW.id IS NOT OLD.id OR NEW.page_id IS NOT OLD.page_id OR NEW.block_id IS NOT OLD.block_id OR NEW.kind IS NOT OLD.kind
   OR NEW.polygon IS NOT OLD.polygon OR NEW.raw_html IS NOT OLD.raw_html OR NEW.raw_text IS NOT OLD.raw_text)
 OR (NEW.page_id IS NOT OLD.page_id AND EXISTS(SELECT 1 FROM source_pages p JOIN source_finalizations f ON f.source_id=p.source_id WHERE p.id=NEW.page_id))
 OR (NEW.id IS NOT OLD.id AND EXISTS(SELECT 1 FROM source_units u JOIN source_pages p ON p.id=u.page_id JOIN source_finalizations f ON f.source_id=p.source_id WHERE u.id=NEW.id))
BEGIN SELECT RAISE(ABORT,'Finalized raw units are immutable'); END;
CREATE TRIGGER units_finalized_delete BEFORE DELETE ON source_units
WHEN EXISTS(SELECT 1 FROM source_pages p JOIN source_finalizations f ON f.source_id=p.source_id WHERE p.id=OLD.page_id)
BEGIN SELECT RAISE(ABORT,'Finalized unit identity is retained'); END;
CREATE TRIGGER source_rulesets_finalized_insert BEFORE INSERT ON source_rulesets WHEN EXISTS(SELECT 1 FROM source_finalizations WHERE source_id=NEW.source_id)
BEGIN
    SELECT CASE WHEN EXISTS(SELECT 1 FROM source_rulesets WHERE source_id=NEW.source_id)
      OR NOT EXISTS(SELECT 1 FROM sources s JOIN legacy_ruleset_aliases a ON a.system_id=s.system_id AND a.edition=s.edition
        WHERE s.id=NEW.source_id AND json_extract(s.prepared_json,'$.contract_version')=1 AND a.game_id=NEW.game_id AND a.edition_id=NEW.edition_id)
      THEN RAISE(ABORT,'Finalized association must be its exact first alias') END;
END;
CREATE TRIGGER source_rulesets_finalized_update BEFORE UPDATE ON source_rulesets WHEN EXISTS(SELECT 1 FROM source_finalizations WHERE source_id=OLD.source_id OR source_id=NEW.source_id)
BEGIN SELECT RAISE(ABORT,'Finalized source association is immutable'); END;
CREATE TRIGGER source_rulesets_finalized_delete BEFORE DELETE ON source_rulesets WHEN EXISTS(SELECT 1 FROM source_finalizations WHERE source_id=OLD.source_id)
BEGIN SELECT RAISE(ABORT,'Finalized source association is retained'); END;
CREATE TRIGGER page_review_revision BEFORE UPDATE ON source_pages
WHEN NEW.revision IS NOT OLD.revision OR NEW.complete IS NOT OLD.complete OR NEW.printed_label IS NOT OLD.printed_label
BEGIN
    SELECT CASE WHEN NEW.complete=1 AND EXISTS(SELECT 1 FROM source_units WHERE page_id=OLD.id AND state='unreviewed')
      THEN RAISE(ABORT,'Page completion requires every unit to be reviewed') END;
    SELECT CASE WHEN typeof(NEW.revision)<>'integer' OR NEW.revision<=0 OR OLD.revision>=9223372036854775807 OR NEW.revision<>OLD.revision+1
      THEN RAISE(ABORT,'Page review revisions must advance exactly once') END;
END;
CREATE TRIGGER unit_review_revision BEFORE UPDATE ON source_units
WHEN NEW.revision IS NOT OLD.revision OR NEW.state IS NOT OLD.state OR NEW.correction IS NOT OLD.correction
BEGIN
    SELECT CASE WHEN typeof(NEW.revision)<>'integer' OR NEW.revision<=0 OR OLD.revision>=9223372036854775807 OR NEW.revision<>OLD.revision+1
      THEN RAISE(ABORT,'Unit review revisions must advance exactly once') END;
END;
CREATE TRIGGER unit_review_page_cascade AFTER UPDATE ON source_units
WHEN NEW.revision IS NOT OLD.revision OR NEW.state IS NOT OLD.state OR NEW.correction IS NOT OLD.correction
BEGIN
    UPDATE source_pages SET complete=0,revision=revision+1 WHERE id=NEW.page_id;
END;
CREATE TRIGGER source_review_event_insert BEFORE INSERT ON source_review_events
BEGIN
    SELECT CASE WHEN EXISTS(SELECT 1 FROM source_review_events WHERE id=NEW.id)
      OR typeof(NEW.previous_revision)<>'integer' OR NEW.previous_revision<=0
      OR NOT ((NEW.unit_id IS NOT NULL AND EXISTS(SELECT 1 FROM source_units u WHERE u.id=NEW.unit_id AND u.page_id=NEW.page_id
        AND u.revision=NEW.previous_revision+1 AND u.state=NEW.state AND u.correction=NEW.correction
        AND NEW.state IN ('accepted','corrected','rejected') AND ((NEW.state='corrected' AND length(NEW.correction)>0) OR (NEW.state<>'corrected' AND NEW.correction=''))))
        OR (NEW.unit_id IS NULL AND NEW.state='page-complete' AND EXISTS(SELECT 1 FROM source_pages p WHERE p.id=NEW.page_id
          AND p.complete=1 AND p.revision=NEW.previous_revision+1 AND p.printed_label=NEW.correction
          AND NOT EXISTS(SELECT 1 FROM source_units u WHERE u.page_id=p.id AND u.state='unreviewed'))))
      THEN RAISE(ABORT,'Invalid source review event or replacement') END;
END;
CREATE TRIGGER source_review_event_update BEFORE UPDATE ON source_review_events BEGIN SELECT RAISE(ABORT,'Source review history is append-only'); END;
CREATE TRIGGER source_review_event_delete BEFORE DELETE ON source_review_events BEGIN SELECT RAISE(ABORT,'Source review history is retained'); END;
PRAGMA user_version=7;
