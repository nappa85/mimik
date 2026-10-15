-- Registry history starts here: earlier registrations keep no invented events or attribution.
-- Planned as 017 (016 is reserved for retrieval); renumber at merge if retrieval lands first.
CREATE TABLE registry_events (
 id INTEGER PRIMARY KEY CHECK(id>0),
 kind TEXT NOT NULL CHECK(kind IN ('ruleset','alias')),
 game_id TEXT NOT NULL,
 edition_id TEXT NOT NULL,
 game_label TEXT CHECK((kind='ruleset')=(game_label IS NOT NULL)),
 label TEXT NOT NULL,
 created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
 FOREIGN KEY (game_id,edition_id) REFERENCES registry_editions(game_id,edition_id) ON DELETE RESTRICT
) STRICT;
CREATE UNIQUE INDEX registry_event_identity ON registry_events(kind,game_id,edition_id,label);
CREATE TRIGGER registry_events_insert BEFORE INSERT ON registry_events WHEN
 EXISTS(SELECT 1 FROM registry_events WHERE id=NEW.id)
 OR EXISTS(SELECT 1 FROM registry_events WHERE kind=NEW.kind AND game_id=NEW.game_id AND edition_id=NEW.edition_id AND label=NEW.label)
 OR NOT ((NEW.kind='ruleset' AND EXISTS(SELECT 1 FROM registry_editions e JOIN registry_games g ON g.game_id=e.game_id
   WHERE e.game_id=NEW.game_id AND e.edition_id=NEW.edition_id AND e.label=NEW.label AND g.label=NEW.game_label))
 OR (NEW.kind='alias' AND EXISTS(SELECT 1 FROM legacy_ruleset_aliases a
   WHERE a.system_id=NEW.game_id AND a.game_id=NEW.game_id AND a.edition_id=NEW.edition_id AND a.edition=NEW.label)))
 BEGIN SELECT RAISE(ABORT,'invalid registry event'); END;
CREATE TRIGGER registry_events_update BEFORE UPDATE ON registry_events BEGIN SELECT RAISE(ABORT,'registry history is append only'); END;
CREATE TRIGGER registry_events_delete BEFORE DELETE ON registry_events BEGIN SELECT RAISE(ABORT,'registry history is append only'); END;
CREATE TABLE registry_web_events (
 event_id INTEGER PRIMARY KEY REFERENCES registry_events(id),
 actor_id INTEGER NOT NULL REFERENCES accounts(id) CHECK(actor_id>0),
 actor_epoch INTEGER NOT NULL CHECK(actor_epoch>0),
 session_digest TEXT NOT NULL CHECK(length(CAST(session_digest AS BLOB))=64 AND instr(session_digest,char(0))=0 AND session_digest NOT GLOB '*[^0-9a-f]*')
) STRICT;
CREATE TRIGGER registry_web_events_insert BEFORE INSERT ON registry_web_events WHEN
 EXISTS(SELECT 1 FROM registry_web_events WHERE event_id=NEW.event_id) OR
 NOT EXISTS(SELECT 1 FROM account_web_request r JOIN registry_events e ON e.id=NEW.event_id
 WHERE r.actor_id=NEW.actor_id AND r.actor_epoch=NEW.actor_epoch AND r.session_digest=NEW.session_digest
 AND e.id=(SELECT max(id) FROM registry_events))
 BEGIN SELECT RAISE(ABORT,'invalid authenticated registry attribution'); END;
CREATE TRIGGER registry_web_events_update BEFORE UPDATE ON registry_web_events BEGIN SELECT RAISE(ABORT,'registry attribution is append only'); END;
CREATE TRIGGER registry_web_events_delete BEFORE DELETE ON registry_web_events BEGIN SELECT RAISE(ABORT,'registry attribution is append only'); END;
PRAGMA user_version=16;
