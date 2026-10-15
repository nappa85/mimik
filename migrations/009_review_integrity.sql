-- Retrofit interrupted-session schema7/8 databases without changing retained data.
CREATE TRIGGER page_completion_integrity BEFORE UPDATE ON source_pages
WHEN NEW.complete=1 AND EXISTS(SELECT 1 FROM source_units WHERE page_id=OLD.id AND state='unreviewed')
BEGIN SELECT RAISE(ABORT,'Page completion requires every unit to be reviewed'); END;
CREATE TRIGGER page_completion_event_integrity BEFORE INSERT ON source_review_events
WHEN NEW.unit_id IS NULL AND NEW.state='page-complete'
 AND EXISTS(SELECT 1 FROM source_units WHERE page_id=NEW.page_id AND state='unreviewed')
BEGIN SELECT RAISE(ABORT,'Invalid source review event or replacement'); END;
DROP TRIGGER IF EXISTS definition_approvals_provenance;
-- Rust verifies canonical bytes/hashes; SQL enforces relational/envelope integrity.
CREATE TRIGGER definition_approvals_provenance BEFORE INSERT ON definition_approvals
WHEN NOT EXISTS (
 SELECT 1 FROM definition_versions v WHERE v.id=NEW.version_id
 AND (
  (NEW.target_kind='version' AND NEW.target_id=v.id AND NEW.target_revision=v.version)
  OR (NEW.target_kind='draft' AND EXISTS (
   SELECT 1 FROM definition_drafts d WHERE d.id=NEW.target_id AND d.revision=NEW.target_revision
   AND d.game_id=v.game_id AND d.edition_id=v.edition_id AND d.definition_id=v.definition_id
   AND d.content_hash=v.content_hash AND d.content=v.content
  ))
 )
 AND json_extract(NEW.snapshot,'$.encoding') IS NEW.encoding
 AND json_type(NEW.snapshot,'$.encoding') IS 'integer'
 AND json_type(NEW.snapshot,'$.ruleset') IS 'object'
 AND json_extract(NEW.snapshot,'$.ruleset.game_id') IS v.game_id
 AND json_extract(NEW.snapshot,'$.ruleset.edition_id') IS v.edition_id
 AND json_extract(NEW.snapshot,'$.definition_id') IS v.definition_id
 AND json_extract(NEW.snapshot,'$.content_hash') IS v.content_hash
 AND json_type(NEW.snapshot,'$.sources') IS 'array' AND json_array_length(NEW.snapshot,'$.sources') BETWEEN 1 AND 32
 AND json_type(NEW.snapshot,'$.pages') IS 'array' AND json_array_length(NEW.snapshot,'$.pages') BETWEEN 1 AND 1024
 AND json_type(NEW.snapshot,'$.units') IS 'array' AND json_array_length(NEW.snapshot,'$.units') BETWEEN 1 AND 1024
 AND json_type(NEW.snapshot,'$.unresolved') IS 'array' AND json_array_length(NEW.snapshot,'$.unresolved') BETWEEN 0 AND 256
 AND (SELECT count(*) FROM json_each(NEW.snapshot))=8
 AND (SELECT count(*) FROM json_each(NEW.snapshot,'$.ruleset'))=2
 AND NOT EXISTS(SELECT 1 FROM json_each(NEW.snapshot) WHERE key NOT IN ('encoding','ruleset','definition_id','content_hash','sources','pages','units','unresolved'))
) BEGIN SELECT RAISE(ABORT,'Invalid approval provenance or snapshot envelope'); END;
PRAGMA user_version=9;
