CREATE TABLE accounts (
    id INTEGER PRIMARY KEY CHECK(id > 0),
    login TEXT NOT NULL UNIQUE CHECK(typeof(login)='text' AND length(CAST(login AS BLOB)) BETWEEN 3 AND 64 AND length(login)=length(CAST(login AS BLOB)) AND login NOT GLOB '*[^a-z0-9_-]*' AND substr(login,1,1) GLOB '[a-z0-9]'),
    display_name TEXT NOT NULL CHECK(length(display_name) BETWEEN 1 AND 100 AND instr(display_name,char(0))=0),
    state TEXT NOT NULL CHECK(state IN ('pending','enabled','disabled')),
    epoch INTEGER NOT NULL CHECK(typeof(epoch)='integer' AND epoch > 0),
    verifier TEXT,
    CHECK((state='pending' AND verifier IS NULL) OR (state IN ('enabled','disabled') AND typeof(verifier)='text' AND length(CAST(verifier AS BLOB))=97 AND length(verifier)=97 AND substr(verifier,1,31)='$argon2id$v=19$m=19456,t=2,p=1$'))
) STRICT;
CREATE TRIGGER accounts_insert BEFORE INSERT ON accounts WHEN NEW.epoch != 1 OR NEW.state='disabled' OR EXISTS(SELECT 1 FROM accounts WHERE id=NEW.id OR login=NEW.login) BEGIN SELECT RAISE(ABORT,'invalid initial account'); END;
CREATE TRIGGER accounts_update BEFORE UPDATE ON accounts WHEN
 NEW.id IS NOT OLD.id OR NEW.login IS NOT OLD.login OR NEW.display_name IS NOT OLD.display_name OR
 OLD.epoch=9223372036854775807 OR typeof(NEW.epoch)!='integer' OR NEW.epoch != OLD.epoch+1 OR
 OLD.state='disabled' OR (OLD.state='pending' AND NEW.state NOT IN ('pending','enabled')) OR
 (OLD.state='enabled' AND NEW.state NOT IN ('enabled','disabled')) OR
 (NEW.state='disabled' AND NEW.verifier IS NOT OLD.verifier)
 BEGIN SELECT RAISE(ABORT,'invalid account transition'); END;
CREATE TRIGGER accounts_delete BEFORE DELETE ON accounts BEGIN SELECT RAISE(ABORT,'accounts are permanent'); END;
CREATE TABLE account_owner (
 singleton INTEGER PRIMARY KEY CHECK(singleton=1),
 account_id INTEGER NOT NULL UNIQUE REFERENCES accounts(id)
) STRICT;
CREATE TRIGGER account_owner_insert BEFORE INSERT ON account_owner WHEN EXISTS(SELECT 1 FROM account_owner) OR NOT EXISTS(SELECT 1 FROM accounts WHERE id=NEW.account_id AND state='enabled') BEGIN SELECT RAISE(ABORT,'owner must be enabled and binding absent'); END;
CREATE TRIGGER account_owner_update BEFORE UPDATE ON account_owner BEGIN SELECT RAISE(ABORT,'owner binding is permanent'); END;
CREATE TRIGGER account_owner_delete BEFORE DELETE ON account_owner BEGIN SELECT RAISE(ABORT,'owner binding is permanent'); END;
CREATE TABLE account_credentials (
 digest BLOB PRIMARY KEY CHECK(length(digest)=32),
 account_id INTEGER NOT NULL REFERENCES accounts(id),
 purpose TEXT NOT NULL CHECK(purpose IN ('enroll','reset')),
 epoch INTEGER NOT NULL CHECK(typeof(epoch)='integer' AND epoch>0),
 issued_at INTEGER NOT NULL CHECK(issued_at>0 AND issued_at<=253402300799),
 expires_at INTEGER NOT NULL CHECK(expires_at>issued_at AND expires_at<=253402300799 AND expires_at-issued_at=CASE purpose WHEN 'enroll' THEN 86400 ELSE 3600 END),
 state TEXT NOT NULL CHECK(state IN ('active','consumed','revoked'))
) STRICT;
CREATE TRIGGER account_credentials_insert BEFORE INSERT ON account_credentials WHEN EXISTS(SELECT 1 FROM account_credentials WHERE digest=NEW.digest) OR NEW.state!='active' OR NOT EXISTS(SELECT 1 FROM accounts WHERE id=NEW.account_id AND epoch=NEW.epoch AND state=CASE NEW.purpose WHEN 'enroll' THEN 'pending' ELSE 'enabled' END) BEGIN SELECT RAISE(ABORT,'invalid credential binding'); END;
CREATE TRIGGER account_credentials_update BEFORE UPDATE ON account_credentials WHEN OLD.state!='active' OR NEW.state NOT IN ('consumed','revoked') OR NEW.digest IS NOT OLD.digest OR NEW.account_id IS NOT OLD.account_id OR NEW.purpose IS NOT OLD.purpose OR NEW.epoch IS NOT OLD.epoch OR NEW.issued_at IS NOT OLD.issued_at OR NEW.expires_at IS NOT OLD.expires_at BEGIN SELECT RAISE(ABORT,'invalid credential transition'); END;
CREATE TRIGGER account_credentials_delete BEFORE DELETE ON account_credentials BEGIN SELECT RAISE(ABORT,'credentials are permanent'); END;
CREATE TABLE account_events (
 id INTEGER PRIMARY KEY CHECK(id>0),
 account_id INTEGER NOT NULL REFERENCES accounts(id),
 epoch INTEGER NOT NULL CHECK(epoch>0),
 occurred_at INTEGER NOT NULL CHECK(occurred_at>0 AND occurred_at<=253402300799),
 actor TEXT NOT NULL CHECK(actor IN ('local_operator','credential_redemption')),
 action TEXT NOT NULL CHECK(action IN ('bootstrap','invite','reissue','reset_issue','enroll','reset_complete','disable')),
 CHECK(actor=CASE WHEN action IN ('enroll','reset_complete') THEN 'credential_redemption' ELSE 'local_operator' END)
) STRICT;
CREATE TRIGGER account_events_insert BEFORE INSERT ON account_events WHEN EXISTS(SELECT 1 FROM account_events WHERE id=NEW.id) OR NOT EXISTS(SELECT 1 FROM accounts WHERE id=NEW.account_id AND epoch=NEW.epoch) BEGIN SELECT RAISE(ABORT,'event collision or epoch mismatch'); END;
CREATE TRIGGER account_events_update BEFORE UPDATE ON account_events BEGIN SELECT RAISE(ABORT,'events are append only'); END;
CREATE TRIGGER account_events_delete BEFORE DELETE ON account_events BEGIN SELECT RAISE(ABORT,'events are append only'); END;
PRAGMA user_version=10;
