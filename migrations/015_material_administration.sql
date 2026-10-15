CREATE INDEX source_review_page_history ON source_review_events(page_id,id);
CREATE TABLE source_web_events (
 event_id INTEGER PRIMARY KEY REFERENCES source_review_events(id),
 actor_id INTEGER NOT NULL REFERENCES accounts(id) CHECK(actor_id>0),
 actor_epoch INTEGER NOT NULL CHECK(actor_epoch>0),
 session_digest TEXT NOT NULL CHECK(length(CAST(session_digest AS BLOB))=64 AND instr(session_digest,char(0))=0 AND session_digest NOT GLOB '*[^0-9a-f]*')
) STRICT;
CREATE TRIGGER source_web_events_insert BEFORE INSERT ON source_web_events WHEN
 EXISTS(SELECT 1 FROM source_web_events WHERE event_id=NEW.event_id) OR
 NOT EXISTS(SELECT 1 FROM account_web_request r JOIN source_review_events e ON e.id=NEW.event_id
 WHERE r.actor_id=NEW.actor_id AND r.actor_epoch=NEW.actor_epoch AND r.session_digest=NEW.session_digest
 AND e.actor='account:'||r.actor_id AND e.id=(SELECT max(id) FROM source_review_events)
 AND ((e.unit_id IS NOT NULL AND e.state IN ('accepted','corrected','rejected') AND EXISTS(
 SELECT 1 FROM source_units u WHERE u.id=e.unit_id AND u.page_id=e.page_id AND u.revision=e.previous_revision+1 AND u.state=e.state AND u.correction=e.correction))
 OR (e.unit_id IS NULL AND e.state='page-complete' AND EXISTS(
 SELECT 1 FROM source_pages p WHERE p.id=e.page_id AND p.complete=1 AND p.revision=e.previous_revision+1 AND p.printed_label=e.correction))))
 BEGIN SELECT RAISE(ABORT,'invalid authenticated source attribution'); END;
CREATE TRIGGER source_web_events_update BEFORE UPDATE ON source_web_events BEGIN SELECT RAISE(ABORT,'source attribution is append only'); END;
CREATE TRIGGER source_web_events_delete BEFORE DELETE ON source_web_events BEGIN SELECT RAISE(ABORT,'source attribution is append only'); END;
PRAGMA user_version=15;
