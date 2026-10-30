CREATE TABLE source_documents (
 id INTEGER PRIMARY KEY,
 pdf_sha256 TEXT NOT NULL UNIQUE
) STRICT;
CREATE TABLE source_document_members (
 source_id INTEGER PRIMARY KEY REFERENCES sources(id),
 document_id INTEGER NOT NULL REFERENCES source_documents(id)
) STRICT;
CREATE INDEX source_document_members_document ON source_document_members(document_id,source_id);
INSERT INTO source_documents(pdf_sha256) SELECT DISTINCT pdf_hash FROM sources ORDER BY pdf_hash;
INSERT INTO source_document_members SELECT s.id,d.id FROM sources s JOIN source_documents d ON d.pdf_sha256=s.pdf_hash;
CREATE TRIGGER source_documents_insert BEFORE INSERT ON source_documents WHEN EXISTS(SELECT 1 FROM source_documents WHERE id=NEW.id OR pdf_sha256=NEW.pdf_sha256)
BEGIN SELECT RAISE(ABORT,'Document identity cannot be replaced'); END;
CREATE TRIGGER source_documents_update BEFORE UPDATE ON source_documents BEGIN SELECT RAISE(ABORT,'Document identity is immutable'); END;
CREATE TRIGGER source_documents_delete BEFORE DELETE ON source_documents BEGIN SELECT RAISE(ABORT,'Documents are retained'); END;
CREATE TRIGGER source_document_members_insert BEFORE INSERT ON source_document_members WHEN
 EXISTS(SELECT 1 FROM source_document_members WHERE source_id=NEW.source_id) OR NOT EXISTS(
 SELECT 1 FROM sources s JOIN source_documents d ON d.pdf_sha256=s.pdf_hash WHERE s.id=NEW.source_id AND d.id=NEW.document_id)
BEGIN SELECT RAISE(ABORT,'Invalid document membership'); END;
CREATE TRIGGER source_document_members_update BEFORE UPDATE ON source_document_members BEGIN SELECT RAISE(ABORT,'Document membership is immutable'); END;
CREATE TRIGGER source_document_members_delete BEFORE DELETE ON source_document_members BEGIN SELECT RAISE(ABORT,'Document membership is retained'); END;

CREATE TABLE source_primary_pages (
 document_id INTEGER NOT NULL REFERENCES source_documents(id),
 pdf_page INTEGER NOT NULL CHECK(pdf_page>0),
 source_id INTEGER NOT NULL REFERENCES sources(id),
 PRIMARY KEY(document_id,pdf_page)
) STRICT;
CREATE INDEX source_primary_pages_source ON source_primary_pages(source_id,pdf_page);
CREATE TABLE source_primary_events (
 id INTEGER PRIMARY KEY,
 document_id INTEGER NOT NULL REFERENCES source_documents(id),
 pdf_page INTEGER NOT NULL CHECK(pdf_page>0),
 source_id INTEGER NOT NULL REFERENCES sources(id),
 previous_source_id INTEGER REFERENCES sources(id),
 actor TEXT NOT NULL CHECK(length(actor) BETWEEN 1 AND 100 AND instr(actor,char(0))=0),
 rationale TEXT NOT NULL CHECK(length(rationale) BETWEEN 1 AND 5000 AND instr(rationale,char(0))=0),
 created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
) STRICT;
CREATE INDEX source_primary_history ON source_primary_events(document_id,pdf_page,id);
CREATE TRIGGER source_primary_events_insert BEFORE INSERT ON source_primary_events
BEGIN
 SELECT CASE WHEN EXISTS(SELECT 1 FROM source_primary_events WHERE id=NEW.id)
 OR NEW.previous_source_id IS NOT (SELECT source_id FROM source_primary_pages WHERE document_id=NEW.document_id AND pdf_page=NEW.pdf_page)
 OR NOT EXISTS(SELECT 1 FROM source_document_members m JOIN source_pages p ON p.source_id=m.source_id
 WHERE m.document_id=NEW.document_id AND m.source_id=NEW.source_id AND p.pdf_page=NEW.pdf_page)
 THEN RAISE(ABORT,'Invalid primary decision or previous state') END;
END;
CREATE TRIGGER source_primary_events_apply AFTER INSERT ON source_primary_events
BEGIN
 INSERT INTO source_primary_pages VALUES(NEW.document_id,NEW.pdf_page,NEW.source_id)
 ON CONFLICT(document_id,pdf_page) DO UPDATE SET source_id=excluded.source_id;
END;
CREATE TRIGGER source_primary_pages_insert BEFORE INSERT ON source_primary_pages WHEN NOT EXISTS(
 SELECT 1 FROM source_primary_events e WHERE e.id=(SELECT max(id) FROM source_primary_events)
 AND e.document_id=NEW.document_id AND e.pdf_page=NEW.pdf_page AND e.source_id=NEW.source_id)
BEGIN SELECT RAISE(ABORT,'Primary selection requires an event'); END;
CREATE TRIGGER source_primary_pages_update BEFORE UPDATE ON source_primary_pages WHEN
 NEW.document_id IS NOT OLD.document_id OR NEW.pdf_page IS NOT OLD.pdf_page OR NOT EXISTS(
 SELECT 1 FROM source_primary_events e WHERE e.id=(SELECT max(id) FROM source_primary_events)
 AND e.document_id=NEW.document_id AND e.pdf_page=NEW.pdf_page AND e.source_id=NEW.source_id AND e.previous_source_id=OLD.source_id)
BEGIN SELECT RAISE(ABORT,'Primary selection requires an event'); END;
CREATE TRIGGER source_primary_pages_delete BEFORE DELETE ON source_primary_pages BEGIN SELECT RAISE(ABORT,'Primary selections are retained'); END;
CREATE TRIGGER source_primary_events_update BEFORE UPDATE ON source_primary_events BEGIN SELECT RAISE(ABORT,'Primary history is append only'); END;
CREATE TRIGGER source_primary_events_delete BEFORE DELETE ON source_primary_events BEGIN SELECT RAISE(ABORT,'Primary history is retained'); END;
INSERT INTO source_primary_events(document_id,pdf_page,source_id,actor,rationale)
 SELECT m.document_id,p.pdf_page,min(p.source_id),'import','Unique legacy reading'
 FROM source_pages p JOIN source_document_members m ON m.source_id=p.source_id
 GROUP BY m.document_id,p.pdf_page HAVING count(*)=1;
CREATE TABLE source_primary_web_events (
 event_id INTEGER PRIMARY KEY REFERENCES source_primary_events(id),
 actor_id INTEGER NOT NULL REFERENCES accounts(id) CHECK(actor_id>0),
 actor_epoch INTEGER NOT NULL CHECK(actor_epoch>0),
 session_digest TEXT NOT NULL CHECK(length(CAST(session_digest AS BLOB))=64 AND instr(session_digest,char(0))=0 AND session_digest NOT GLOB '*[^0-9a-f]*')
) STRICT;
CREATE TRIGGER source_primary_web_events_insert BEFORE INSERT ON source_primary_web_events WHEN
 EXISTS(SELECT 1 FROM source_primary_web_events WHERE event_id=NEW.event_id) OR NOT EXISTS(
 SELECT 1 FROM account_web_request r JOIN source_primary_events e ON e.id=NEW.event_id
 JOIN source_primary_pages p ON p.document_id=e.document_id AND p.pdf_page=e.pdf_page AND p.source_id=e.source_id
 WHERE r.actor_id=NEW.actor_id AND r.actor_epoch=NEW.actor_epoch AND r.session_digest=NEW.session_digest
 AND e.actor='account:'||r.actor_id AND e.id=(SELECT max(id) FROM source_primary_events))
BEGIN SELECT RAISE(ABORT,'Invalid authenticated primary attribution'); END;
CREATE TRIGGER source_primary_web_events_update BEFORE UPDATE ON source_primary_web_events BEGIN SELECT RAISE(ABORT,'Primary attribution is append only'); END;
CREATE TRIGGER source_primary_web_events_delete BEFORE DELETE ON source_primary_web_events BEGIN SELECT RAISE(ABORT,'Primary attribution is retained'); END;

ALTER TABLE source_units ADD COLUMN origin TEXT NOT NULL DEFAULT 'extracted' CHECK(origin IN ('extracted','owner'));
ALTER TABLE source_review_events ADD COLUMN operation_id TEXT CHECK(operation_id IS NULL OR (length(operation_id) BETWEEN 1 AND 100 AND instr(operation_id,char(0))=0));
CREATE INDEX source_review_operation ON source_review_events(operation_id) WHERE operation_id IS NOT NULL;
CREATE TRIGGER source_unit_origin_update BEFORE UPDATE ON source_units WHEN NEW.origin IS NOT OLD.origin
BEGIN SELECT RAISE(ABORT,'Unit origin is immutable'); END;
CREATE TRIGGER source_unit_origin_insert BEFORE INSERT ON source_units WHEN
 (NEW.origin='extracted' AND NEW.block_id GLOB 'owner:*') OR
 (NEW.origin='owner' AND (NEW.block_id NOT GLOB 'owner:[1-9]*' OR substr(NEW.block_id,7) GLOB '*[^0-9]*'
 OR CAST(CAST(substr(NEW.block_id,7) AS INTEGER) AS TEXT) IS NOT substr(NEW.block_id,7)
 OR length(NEW.block_id)>25 OR NEW.state<>'unreviewed' OR NEW.correction<>'' OR NEW.revision<>1 OR NEW.kind<>'Text'
 OR NOT EXISTS(SELECT 1 FROM source_pages p JOIN source_finalizations f ON f.source_id=p.source_id WHERE p.id=NEW.page_id)))
BEGIN SELECT RAISE(ABORT,'Invalid owner unit identity'); END;
DROP TRIGGER units_finalized_insert;
CREATE TRIGGER units_finalized_insert BEFORE INSERT ON source_units WHEN
 (EXISTS(SELECT 1 FROM source_pages p JOIN source_finalizations f ON f.source_id=p.source_id WHERE p.id=NEW.page_id) AND NEW.origin<>'owner')
 OR EXISTS(SELECT 1 FROM source_units u WHERE u.id=NEW.id OR (u.page_id=NEW.page_id AND u.block_id=NEW.block_id))
BEGIN SELECT RAISE(ABORT,'Finalized unit membership cannot change'); END;
-- Geometry is checked in SQL as well as at the store boundary. Four strictly convex points
-- inside the immutable manifest rectangle; malformed/degenerate/crossed polygons fail closed.
CREATE TRIGGER source_owner_geometry BEFORE INSERT ON source_units WHEN NEW.origin='owner'
BEGIN
 SELECT CASE WHEN length(CAST(NEW.polygon AS BLOB))>1024 OR NOT json_valid(NEW.polygon)
 THEN RAISE(ABORT,'Invalid owner polygon') END;
 SELECT CASE WHEN json_type(NEW.polygon)<>'array' OR json_array_length(NEW.polygon)<>4 OR NOT EXISTS(
 SELECT 1 FROM source_pages p JOIN sources s ON s.id=p.source_id,
 json_each(CASE WHEN length(s.prepared_json)<=8388608 AND json_valid(s.prepared_json) THEN s.prepared_json ELSE '{}' END,'$.pages') g
 WHERE p.id=NEW.page_id AND json_extract(g.value,'$.pdf_page')=p.pdf_page
 AND json_extract(g.value,'$.width_points') BETWEEN 1 AND 4000 AND json_extract(g.value,'$.height_points') BETWEEN 1 AND 4000
 AND NOT EXISTS(SELECT 1 FROM json_each(NEW.polygon) v WHERE json_type(v.value)<>'array' OR json_array_length(v.value)<>2
 OR coalesce(json_type(v.value,'$[0]'),'null') NOT IN ('integer','real') OR coalesce(json_type(v.value,'$[1]'),'null') NOT IN ('integer','real')
 OR json_extract(v.value,'$[0]') NOT BETWEEN 0 AND json_extract(g.value,'$.width_points')
 OR json_extract(v.value,'$[1]') NOT BETWEEN 0 AND json_extract(g.value,'$.height_points')))
 THEN RAISE(ABORT,'Owner polygon must be inside its page') END;
 SELECT CASE WHEN NOT (
 WITH points AS (SELECT CAST(key AS INTEGER) i,json_extract(value,'$[0]') x,json_extract(value,'$[1]') y FROM json_each(NEW.polygon)),
 turns AS (SELECT (b.x-a.x)*(c.y-b.y)-(b.y-a.y)*(c.x-b.x) turn FROM points a JOIN points b ON b.i=(a.i+1)%4 JOIN points c ON c.i=(a.i+2)%4)
 SELECT min(turn)>0 OR max(turn)<0 FROM turns)
 THEN RAISE(ABORT,'Owner polygon must be nondegenerate and convex') END;
END;
CREATE TRIGGER source_owner_page_reset AFTER INSERT ON source_units WHEN NEW.origin='owner'
BEGIN UPDATE source_pages SET complete=0,revision=revision+1 WHERE id=NEW.page_id; END;

CREATE TABLE source_suggestions (
 id INTEGER PRIMARY KEY,
 source_id INTEGER NOT NULL REFERENCES sources(id),
 bundle_hash TEXT NOT NULL,
 origin TEXT NOT NULL CHECK(length(origin) BETWEEN 1 AND 100 AND instr(origin,char(0))=0),
 kind TEXT NOT NULL CHECK(length(kind) BETWEEN 1 AND 100 AND instr(kind,char(0))=0),
 targets TEXT NOT NULL CHECK(length(CAST(targets AS BLOB))<=65536 AND json_valid(targets)),
 suggested_text TEXT CHECK(suggested_text IS NULL OR (length(suggested_text)<=20000 AND instr(suggested_text,char(0))=0)),
 rationale TEXT NOT NULL CHECK(length(rationale) BETWEEN 1 AND 5000 AND instr(rationale,char(0))=0),
 dismissed_by TEXT CHECK(dismissed_by IS NULL OR (length(dismissed_by) BETWEEN 1 AND 100 AND instr(dismissed_by,char(0))=0)),
 dismissed_at TEXT,
 CHECK((dismissed_by IS NULL)=(dismissed_at IS NULL))
) STRICT;
CREATE INDEX source_suggestions_source ON source_suggestions(source_id,id);
CREATE TRIGGER source_suggestions_insert BEFORE INSERT ON source_suggestions WHEN
 EXISTS(SELECT 1 FROM source_suggestions WHERE id=NEW.id) OR NEW.dismissed_by IS NOT NULL OR NEW.dismissed_at IS NOT NULL
 OR NOT EXISTS(SELECT 1 FROM sources WHERE id=NEW.source_id AND bundle_hash=NEW.bundle_hash)
BEGIN SELECT RAISE(ABORT,'Suggestion bundle does not match source'); END;
CREATE TRIGGER source_suggestions_update BEFORE UPDATE ON source_suggestions WHEN
 NEW.id IS NOT OLD.id OR NEW.source_id IS NOT OLD.source_id OR NEW.bundle_hash IS NOT OLD.bundle_hash
 OR NEW.origin IS NOT OLD.origin OR NEW.kind IS NOT OLD.kind OR NEW.targets IS NOT OLD.targets
 OR NEW.suggested_text IS NOT OLD.suggested_text OR NEW.rationale IS NOT OLD.rationale
 OR OLD.dismissed_by IS NOT NULL OR NEW.dismissed_by IS NULL OR NEW.dismissed_at IS NULL
BEGIN SELECT RAISE(ABORT,'Only first dismissal may change a suggestion'); END;
CREATE TRIGGER source_suggestions_delete BEFORE DELETE ON source_suggestions BEGIN SELECT RAISE(ABORT,'Suggestions are retained'); END;
PRAGMA user_version=24;
