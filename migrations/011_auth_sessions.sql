CREATE TABLE auth_sessions (
 id TEXT PRIMARY KEY NOT NULL CHECK(length(id)=22 AND length(CAST(id AS BLOB))=22 AND id NOT GLOB '*[^A-Za-z0-9_-]*'),
 issued_at INTEGER NOT NULL CHECK(issued_at BETWEEN 1 AND 253402300799),
 expires_at INTEGER NOT NULL CHECK(expires_at>issued_at AND expires_at<=253402300799),
 state TEXT NOT NULL CHECK(state IN ('active','revoked')),
 payload TEXT,
 account_id INTEGER REFERENCES accounts(id),
 epoch INTEGER,
 CHECK((account_id IS NULL AND epoch IS NULL) OR (account_id IS NOT NULL AND epoch IS NOT NULL AND account_id>0 AND epoch>0)),
 CHECK(expires_at-issued_at <= CASE WHEN account_id IS NULL THEN 600 ELSE 28800 END),
 CHECK((state='revoked' AND payload IS NULL) OR (state='active' AND payload IS NOT NULL AND length(CAST(payload AS BLOB))<=4096 AND json_valid(payload)))
) STRICT;
CREATE INDEX auth_sessions_expiry ON auth_sessions(expires_at,id);
CREATE TRIGGER auth_sessions_insert BEFORE INSERT ON auth_sessions BEGIN
 SELECT CASE WHEN EXISTS(SELECT 1 FROM auth_sessions WHERE id=NEW.id) OR NEW.state!='active' OR NEW.issued_at>mimik_auth_now() OR NEW.expires_at<=mimik_auth_now() OR (SELECT count(*) FROM auth_sessions)>=10000 THEN RAISE(ABORT,'invalid session insertion') END;
 SELECT CASE WHEN json_type(NEW.payload) IS NOT 'object' OR (SELECT count(*) FROM json_each(NEW.payload)) != CASE WHEN NEW.account_id IS NULL THEN 1 ELSE 2 END OR EXISTS(SELECT 1 FROM json_each(NEW.payload) WHERE key NOT IN ('csrf','principal')) OR json_type(NEW.payload,'$.csrf') IS NOT 'text' OR length(json_extract(NEW.payload,'$.csrf'))!=64 OR length(CAST(json_extract(NEW.payload,'$.csrf') AS BLOB))!=64 OR json_extract(NEW.payload,'$.csrf') GLOB '*[^0-9a-f]*' THEN RAISE(ABORT,'invalid session payload') END;
 SELECT CASE WHEN (NEW.account_id IS NULL AND json_type(NEW.payload,'$.principal') IS NOT NULL) OR (NEW.account_id IS NOT NULL AND (json_type(NEW.payload,'$.principal') IS NOT 'object' OR (SELECT count(*) FROM json_each(NEW.payload,'$.principal'))!=2 OR EXISTS(SELECT 1 FROM json_each(NEW.payload,'$.principal') WHERE key NOT IN ('id','epoch')) OR json_type(NEW.payload,'$.principal.id') IS NOT 'integer' OR json_type(NEW.payload,'$.principal.epoch') IS NOT 'integer' OR json_extract(NEW.payload,'$.principal.id') IS NOT NEW.account_id OR json_extract(NEW.payload,'$.principal.epoch') IS NOT NEW.epoch OR NOT EXISTS(SELECT 1 FROM accounts WHERE id=NEW.account_id AND epoch=NEW.epoch AND state='enabled'))) THEN RAISE(ABORT,'invalid session principal') END;
END;
CREATE TRIGGER auth_sessions_update BEFORE UPDATE ON auth_sessions BEGIN
 SELECT mimik_auth_now();
 SELECT CASE WHEN NEW.id IS NOT OLD.id OR NEW.issued_at IS NOT OLD.issued_at OR NEW.expires_at IS NOT OLD.expires_at OR NEW.account_id IS NOT OLD.account_id OR NEW.epoch IS NOT OLD.epoch OR (OLD.state='revoked' AND (NEW.state!='revoked' OR NEW.payload IS NOT NULL)) OR (NEW.state='active' AND (NEW.payload IS NOT OLD.payload OR NEW.expires_at<=mimik_auth_now() OR (NEW.account_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM accounts WHERE id=NEW.account_id AND epoch=NEW.epoch AND state='enabled')))) OR (NEW.state='revoked' AND NEW.payload IS NOT NULL) THEN RAISE(ABORT,'invalid session transition') END;
END;
CREATE TRIGGER auth_sessions_delete BEFORE DELETE ON auth_sessions BEGIN
 SELECT CASE WHEN OLD.expires_at>mimik_auth_now() THEN RAISE(ABORT,'session not expired') END;
END;
PRAGMA user_version=11;
