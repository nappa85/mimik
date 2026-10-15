CREATE VIEW account_web_request AS SELECT
 a.id AS actor_id,a.epoch AS actor_epoch,json_extract(mimik_request_auth(),'$.digest') AS session_digest
 FROM auth_sessions s JOIN accounts a ON a.id=s.account_id JOIN account_owner o ON o.account_id=a.id
 WHERE s.id=json_extract(mimik_request_auth(),'$.id') AND s.state='active'
 AND s.expires_at=json_extract(mimik_request_auth(),'$.deadline') AND mimik_auth_now()<s.expires_at
 AND a.id=json_extract(mimik_request_auth(),'$.account') AND a.epoch=json_extract(mimik_request_auth(),'$.epoch')
 AND s.epoch=a.epoch AND a.state='enabled'
 AND json_extract(s.payload,'$.principal.id')=a.id AND json_extract(s.payload,'$.principal.epoch')=a.epoch;
CREATE TABLE account_web_events (
 event_id INTEGER PRIMARY KEY REFERENCES account_events(id),
 actor_id INTEGER NOT NULL REFERENCES accounts(id) CHECK(actor_id>0),
 actor_epoch INTEGER NOT NULL CHECK(actor_epoch>0),
 session_digest TEXT NOT NULL CHECK(length(session_digest)=64 AND length(CAST(session_digest AS BLOB))=64 AND session_digest NOT GLOB '*[^0-9a-f]*')
) STRICT;
CREATE TRIGGER account_web_events_insert BEFORE INSERT ON account_web_events WHEN
 EXISTS(SELECT 1 FROM account_web_events WHERE event_id=NEW.event_id) OR
 NOT EXISTS(SELECT 1 FROM account_web_request r JOIN account_events e ON e.id=NEW.event_id
 WHERE r.actor_id=NEW.actor_id AND r.actor_epoch=NEW.actor_epoch AND r.session_digest=NEW.session_digest
 AND e.actor='local_operator' AND e.action IN ('invite','reissue','reset_issue','disable')
 AND e.id=(SELECT max(id) FROM account_events)
 AND EXISTS(SELECT 1 FROM accounts a WHERE a.id=e.account_id AND a.epoch=e.epoch))
 BEGIN SELECT RAISE(ABORT,'invalid authenticated administration attribution'); END;
CREATE TRIGGER account_web_events_update BEFORE UPDATE ON account_web_events BEGIN SELECT RAISE(ABORT,'web attribution is append only'); END;
CREATE TRIGGER account_web_events_delete BEFORE DELETE ON account_web_events BEGIN SELECT RAISE(ABORT,'web attribution is append only'); END;
PRAGMA user_version=14;
