CREATE TABLE source_suggestion_reports (
 id INTEGER PRIMARY KEY,
 source_id INTEGER NOT NULL REFERENCES sources(id),
 report_sha256 TEXT NOT NULL CHECK(length(report_sha256)=64 AND report_sha256 NOT GLOB '*[^0-9a-f]*'),
 origin TEXT NOT NULL CHECK(length(CAST(origin AS BLOB)) BETWEEN 1 AND 100 AND instr(origin,char(0))=0),
 schema_version INTEGER NOT NULL CHECK(schema_version IN (1,2)),
 binding TEXT NOT NULL CHECK(binding IN ('bundle','extraction')),
 created_at TEXT NOT NULL,
 imported_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
 correction INTEGER NOT NULL CHECK(correction>=0),
 reject INTEGER NOT NULL CHECK(reject>=0),
 merge INTEGER NOT NULL CHECK(merge>=0),
 missing_text INTEGER NOT NULL CHECK(missing_text>=0),
 note INTEGER NOT NULL CHECK(note>=0),
 failed_pages INTEGER NOT NULL CHECK(failed_pages>=0),
 UNIQUE(source_id,report_sha256)
) STRICT;
CREATE TRIGGER source_suggestion_reports_insert BEFORE INSERT ON source_suggestion_reports WHEN
 EXISTS(SELECT 1 FROM source_suggestion_reports WHERE id=NEW.id OR (source_id=NEW.source_id AND report_sha256=NEW.report_sha256))
 OR NOT EXISTS(SELECT 1 FROM sources WHERE id=NEW.source_id)
BEGIN SELECT RAISE(ABORT,'Invalid or replaced suggestion report'); END;
CREATE TRIGGER source_suggestion_reports_update BEFORE UPDATE ON source_suggestion_reports BEGIN SELECT RAISE(ABORT,'Reports are immutable'); END;
CREATE TRIGGER source_suggestion_reports_delete BEFORE DELETE ON source_suggestion_reports BEGIN SELECT RAISE(ABORT,'Reports are retained'); END;

CREATE TABLE source_suggestions_new (
 id INTEGER PRIMARY KEY,
 source_id INTEGER NOT NULL REFERENCES sources(id),
 bundle_hash TEXT NOT NULL,
 origin TEXT NOT NULL CHECK(length(origin) BETWEEN 1 AND 100 AND instr(origin,char(0))=0),
 kind TEXT NOT NULL CHECK(kind IN ('correction','merge','missing_text','reject','note')),
 targets TEXT NOT NULL CHECK(length(CAST(targets AS BLOB))<=65536 AND json_valid(targets)),
 suggested_text TEXT CHECK(suggested_text IS NULL OR (length(suggested_text)<=20000 AND instr(suggested_text,char(0))=0)),
 rationale TEXT NOT NULL CHECK(length(rationale) BETWEEN 1 AND 5000 AND instr(rationale,char(0))=0),
 report_id INTEGER REFERENCES source_suggestion_reports(id)
) STRICT;
CREATE TEMP TABLE legacy_suggestion_dismissals AS SELECT id,dismissed_by,dismissed_at FROM source_suggestions WHERE dismissed_by IS NOT NULL;
INSERT INTO source_suggestions_new SELECT id,source_id,bundle_hash,origin,kind,targets,suggested_text,rationale,NULL FROM source_suggestions;
DROP TABLE source_suggestions;
ALTER TABLE source_suggestions_new RENAME TO source_suggestions;
CREATE TABLE source_suggestion_dismissals (
 id INTEGER PRIMARY KEY,
 suggestion_id INTEGER NOT NULL UNIQUE REFERENCES source_suggestions(id),
 actor TEXT NOT NULL CHECK(length(actor) BETWEEN 1 AND 100 AND instr(actor,char(0))=0),
 reason TEXT CHECK(reason IS NULL OR (length(reason)<=5000 AND instr(reason,char(0))=0)),
 created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP CHECK(datetime(created_at) IS created_at)
) STRICT;
INSERT INTO source_suggestion_dismissals(suggestion_id,actor,created_at)
 SELECT id,dismissed_by,dismissed_at FROM legacy_suggestion_dismissals;
DROP TABLE legacy_suggestion_dismissals;
CREATE INDEX source_suggestions_source ON source_suggestions(source_id,id);
CREATE TRIGGER source_suggestions_insert BEFORE INSERT ON source_suggestions WHEN
 EXISTS(SELECT 1 FROM source_suggestions WHERE id=NEW.id)
 OR NOT EXISTS(SELECT 1 FROM sources WHERE id=NEW.source_id AND bundle_hash=NEW.bundle_hash)
 OR (NEW.report_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM source_suggestion_reports WHERE id=NEW.report_id AND source_id=NEW.source_id))
BEGIN SELECT RAISE(ABORT,'Suggestion source, bundle or report mismatch'); END;
CREATE TRIGGER source_suggestions_update BEFORE UPDATE ON source_suggestions BEGIN SELECT RAISE(ABORT,'Suggestions are immutable'); END;
CREATE TRIGGER source_suggestions_delete BEFORE DELETE ON source_suggestions BEGIN SELECT RAISE(ABORT,'Suggestions are retained'); END;
CREATE TRIGGER source_suggestions_envelope BEFORE INSERT ON source_suggestions
BEGIN
 SELECT CASE WHEN NOT coalesce(json_type(NEW.targets)='object' AND (
 (NEW.kind='correction' AND json_extract(NEW.targets,'$.type')='unit' AND length(trim(coalesce(NEW.suggested_text,'')))>0) OR
 (NEW.kind='reject' AND json_extract(NEW.targets,'$.type')='unit' AND NEW.suggested_text IS NULL) OR
 (NEW.kind='merge' AND json_extract(NEW.targets,'$.type')='units' AND length(trim(coalesce(NEW.suggested_text,'')))>0) OR
 (NEW.kind='missing_text' AND json_extract(NEW.targets,'$.type')='region' AND length(trim(coalesce(NEW.suggested_text,'')))>0) OR
 (NEW.kind='note' AND json_extract(NEW.targets,'$.type') IN ('unit','page'))),0)
 THEN RAISE(ABORT,'Suggestion kind, target shape and text do not agree') END;
 SELECT CASE WHEN json_extract(NEW.targets,'$.type') IN ('unit','units') AND NOT coalesce((
 WITH u AS (
 SELECT json_type(NEW.targets,'$.target.unit') ut,json_type(NEW.targets,'$.target.revision') rt,json_extract(NEW.targets,'$.target.unit') unit,json_extract(NEW.targets,'$.target.revision') rev WHERE json_extract(NEW.targets,'$.type')='unit'
 UNION ALL
 SELECT json_type(v.value,'$.unit'),json_type(v.value,'$.revision'),json_extract(v.value,'$.unit'),json_extract(v.value,'$.revision') FROM json_each(NEW.targets,'$.targets') v WHERE json_extract(NEW.targets,'$.type')='units' AND json_type(NEW.targets,'$.targets')='array')
 SELECT count(*) BETWEEN CASE json_extract(NEW.targets,'$.type') WHEN 'units' THEN 2 ELSE 1 END AND 1024
 AND (json_extract(NEW.targets,'$.type')='unit' OR json_array_length(NEW.targets,'$.targets')=count(*))
 AND count(*)=sum(coalesce(ut='integer' AND rt='integer' AND rev>0,0)) AND count(DISTINCT unit)=count(*)
 AND (SELECT count(*) FROM u JOIN source_units su ON su.id=u.unit JOIN source_pages sp ON sp.id=su.page_id WHERE sp.source_id=NEW.source_id AND u.rev<=su.revision)=count(*)
 AND (NEW.kind<>'merge' OR (SELECT count(DISTINCT su.page_id) FROM u JOIN source_units su ON su.id=u.unit)=1) FROM u),0)
 THEN RAISE(ABORT,'Suggestion units must be distinct current units of the source') END;
 SELECT CASE WHEN json_extract(NEW.targets,'$.type') IN ('page','region') AND NOT coalesce(
 json_type(NEW.targets,'$.page')='integer' AND EXISTS(SELECT 1 FROM source_pages WHERE id=json_extract(NEW.targets,'$.page') AND source_id=NEW.source_id),0)
 THEN RAISE(ABORT,'Suggestion page must belong to source') END;
 SELECT CASE WHEN json_extract(NEW.targets,'$.type')='page' AND json_type(NEW.targets,'$.block_id') IS NOT NULL AND NOT coalesce(
 json_type(NEW.targets,'$.block_id')='text' AND length(CAST(json_extract(NEW.targets,'$.block_id') AS BLOB))<=512 AND instr(json_extract(NEW.targets,'$.block_id'),char(0))=0,0)
 THEN RAISE(ABORT,'Invalid page block citation') END;
 SELECT CASE WHEN json_extract(NEW.targets,'$.type')='region' AND NOT coalesce(json_type(NEW.targets,'$.polygon')='array' AND json_array_length(NEW.targets,'$.polygon')=4,0)
 THEN RAISE(ABORT,'Suggestion region needs four points') END;
 SELECT CASE WHEN json_extract(NEW.targets,'$.type')='region' AND NOT EXISTS(
 SELECT 1 FROM source_pages p JOIN sources s ON s.id=p.source_id,
 json_each(CASE WHEN length(s.prepared_json)<=8388608 AND json_valid(s.prepared_json) THEN s.prepared_json ELSE '{}' END,'$.pages') g
 WHERE p.id=json_extract(NEW.targets,'$.page') AND json_extract(g.value,'$.pdf_page')=p.pdf_page
 AND json_type(g.value,'$.width_points') IN ('integer','real') AND json_type(g.value,'$.height_points') IN ('integer','real')
 AND json_extract(g.value,'$.width_points') BETWEEN 1 AND 4000 AND json_extract(g.value,'$.height_points') BETWEEN 1 AND 4000
 AND NOT EXISTS(SELECT 1 FROM json_each(NEW.targets,'$.polygon') v WHERE NOT coalesce(json_type(v.value)='array' AND json_array_length(v.value)=2
 AND json_type(v.value,'$[0]') IN ('integer','real') AND json_type(v.value,'$[1]') IN ('integer','real')
 AND json_extract(v.value,'$[0]') BETWEEN 0 AND json_extract(g.value,'$.width_points') AND json_extract(v.value,'$[1]') BETWEEN 0 AND json_extract(g.value,'$.height_points'),0)))
 THEN RAISE(ABORT,'Suggestion region must be inside its page') END;
 SELECT CASE WHEN json_extract(NEW.targets,'$.type')='region' AND NOT coalesce((
 WITH points AS (SELECT CAST(key AS INTEGER) i,json_extract(value,'$[0]') x,json_extract(value,'$[1]') y FROM json_each(NEW.targets,'$.polygon')),
 turns AS (SELECT (b.x-a.x)*(c.y-b.y)-(b.y-a.y)*(c.x-b.x) turn FROM points a JOIN points b ON b.i=(a.i+1)%4 JOIN points c ON c.i=(a.i+2)%4)
 SELECT count(*)=4 AND (min(turn)>0 OR max(turn)<0) FROM turns),0)
 THEN RAISE(ABORT,'Suggestion region must be nondegenerate and convex') END;
END;

CREATE TABLE source_suggestion_applications (
 suggestion_id INTEGER PRIMARY KEY REFERENCES source_suggestions(id),
 event_id INTEGER NOT NULL REFERENCES source_review_events(id),
 operation_id TEXT,
 owner_unit_id INTEGER REFERENCES source_units(id),
 edited INTEGER NOT NULL CHECK(edited IN (0,1))
) STRICT;
CREATE TRIGGER source_suggestion_applications_insert BEFORE INSERT ON source_suggestion_applications
BEGIN
 SELECT CASE WHEN EXISTS(SELECT 1 FROM source_suggestion_applications WHERE suggestion_id=NEW.suggestion_id)
 OR EXISTS(SELECT 1 FROM source_suggestion_dismissals WHERE suggestion_id=NEW.suggestion_id)
 OR NOT EXISTS(
 SELECT 1 FROM source_suggestions s JOIN source_review_events e ON e.id=NEW.event_id
 JOIN source_pages p ON p.id=e.page_id AND p.source_id=s.source_id
 WHERE s.id=NEW.suggestion_id AND e.id=(SELECT max(id) FROM source_review_events) AND (
 (s.kind IN ('correction','reject') AND e.unit_id=json_extract(s.targets,'$.target.unit')
 AND e.previous_revision=json_extract(s.targets,'$.target.revision') AND NEW.operation_id IS NULL AND NEW.owner_unit_id IS NULL
 AND e.state=CASE s.kind WHEN 'correction' THEN 'corrected' ELSE 'rejected' END) OR
 (s.kind='missing_text' AND e.page_id=json_extract(s.targets,'$.page') AND e.unit_id=NEW.owner_unit_id AND e.state='corrected' AND e.previous_revision=1 AND NEW.operation_id IS NULL
 AND EXISTS(SELECT 1 FROM source_units u WHERE u.id=NEW.owner_unit_id AND u.page_id=p.id AND u.origin='owner')) OR
 (s.kind='merge' AND NEW.operation_id IS NOT NULL AND e.operation_id=NEW.operation_id
 AND EXISTS(SELECT 1 FROM source_review_events oe JOIN source_units ou ON ou.id=oe.unit_id WHERE oe.operation_id=NEW.operation_id AND oe.unit_id=NEW.owner_unit_id AND ou.origin='owner' AND ou.page_id=p.id AND oe.state='corrected' AND oe.previous_revision=1)
  AND NOT EXISTS(SELECT 1 FROM json_each(s.targets,'$.targets') t WHERE NOT EXISTS(SELECT 1 FROM source_review_events te WHERE te.operation_id=NEW.operation_id AND te.page_id=p.id AND te.unit_id=json_extract(t.value,'$.unit') AND te.previous_revision=json_extract(t.value,'$.revision') AND te.state='rejected')))))
 THEN RAISE(ABORT,'Invalid suggestion application or dismissed suggestion') END;
END;
CREATE TRIGGER source_suggestion_applications_update BEFORE UPDATE ON source_suggestion_applications BEGIN SELECT RAISE(ABORT,'Applications are immutable'); END;
CREATE TRIGGER source_suggestion_applications_delete BEFORE DELETE ON source_suggestion_applications BEGIN SELECT RAISE(ABORT,'Applications are retained'); END;
CREATE TRIGGER source_suggestion_dismissals_insert BEFORE INSERT ON source_suggestion_dismissals WHEN
 EXISTS(SELECT 1 FROM source_suggestion_dismissals WHERE id=NEW.id OR suggestion_id=NEW.suggestion_id)
 OR EXISTS(SELECT 1 FROM source_suggestion_applications WHERE suggestion_id=NEW.suggestion_id)
 OR NOT EXISTS(SELECT 1 FROM source_suggestions WHERE id=NEW.suggestion_id)
BEGIN SELECT RAISE(ABORT,'Invalid, repeated or applied dismissal'); END;
CREATE TRIGGER source_suggestion_dismissals_update BEFORE UPDATE ON source_suggestion_dismissals BEGIN SELECT RAISE(ABORT,'Dismissals are immutable'); END;
CREATE TRIGGER source_suggestion_dismissals_delete BEFORE DELETE ON source_suggestion_dismissals BEGIN SELECT RAISE(ABORT,'Dismissals are retained'); END;
CREATE TABLE source_suggestion_web_events (
 event_id INTEGER PRIMARY KEY REFERENCES source_suggestion_dismissals(id),
 actor_id INTEGER NOT NULL REFERENCES accounts(id) CHECK(actor_id>0),
 actor_epoch INTEGER NOT NULL CHECK(actor_epoch>0),
 session_digest TEXT NOT NULL CHECK(length(CAST(session_digest AS BLOB))=64 AND instr(session_digest,char(0))=0 AND session_digest NOT GLOB '*[^0-9a-f]*')
) STRICT;
CREATE TRIGGER source_suggestion_web_events_insert BEFORE INSERT ON source_suggestion_web_events WHEN
 EXISTS(SELECT 1 FROM source_suggestion_web_events WHERE event_id=NEW.event_id) OR NOT EXISTS(
 SELECT 1 FROM account_web_request r JOIN source_suggestion_dismissals e ON e.id=NEW.event_id
 WHERE r.actor_id=NEW.actor_id AND r.actor_epoch=NEW.actor_epoch AND r.session_digest=NEW.session_digest
 AND e.actor='account:'||r.actor_id AND e.id=(SELECT max(id) FROM source_suggestion_dismissals))
BEGIN SELECT RAISE(ABORT,'Invalid authenticated dismissal attribution'); END;
CREATE TRIGGER source_suggestion_web_events_update BEFORE UPDATE ON source_suggestion_web_events BEGIN SELECT RAISE(ABORT,'Dismissal attribution is immutable'); END;
CREATE TRIGGER source_suggestion_web_events_delete BEFORE DELETE ON source_suggestion_web_events BEGIN SELECT RAISE(ABORT,'Dismissal attribution is retained'); END;
PRAGMA user_version=25;
