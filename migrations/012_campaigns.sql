CREATE VIEW campaign_request_auth AS SELECT a.id AS account_id,a.epoch AS epoch,s.id AS session_id,s.expires_at AS deadline FROM auth_sessions s JOIN accounts a ON a.id=s.account_id WHERE s.state='active' AND a.state='enabled' AND a.epoch=s.epoch AND s.id=json_extract(mimik_request_auth(),'$.id') AND s.account_id=json_extract(mimik_request_auth(),'$.account') AND s.epoch=json_extract(mimik_request_auth(),'$.epoch') AND s.expires_at=json_extract(mimik_request_auth(),'$.deadline') AND s.expires_at>mimik_auth_now();
CREATE TABLE campaigns (
 id INTEGER PRIMARY KEY CHECK(id>0),
 creator_id INTEGER NOT NULL REFERENCES accounts(id),
 game_id TEXT NOT NULL,
 edition_id TEXT NOT NULL,
 title TEXT NOT NULL CHECK(length(title) BETWEEN 1 AND 100 AND length(trim(title))>0 AND instr(title,char(0))=0),
 revision INTEGER NOT NULL CHECK(revision=1),
 FOREIGN KEY(game_id,edition_id) REFERENCES registry_editions(game_id,edition_id),
 FOREIGN KEY(id,creator_id) REFERENCES campaign_members(campaign_id,account_id) DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE INDEX campaigns_creator ON campaigns(creator_id,id);
CREATE TRIGGER campaigns_insert BEFORE INSERT ON campaigns BEGIN
 SELECT CASE WHEN NOT EXISTS(SELECT 1 FROM campaign_request_auth WHERE account_id=NEW.creator_id) THEN RAISE(ABORT,'campaign auth required') END;
 SELECT CASE WHEN EXISTS(SELECT 1 FROM campaigns WHERE id=NEW.id) OR (SELECT count(*) FROM campaigns)>=128 OR (SELECT count(*) FROM campaigns WHERE creator_id=NEW.creator_id)>=32 OR NOT EXISTS(SELECT 1 FROM accounts WHERE id=NEW.creator_id AND state='enabled') THEN RAISE(ABORT,'invalid campaign creation') END;
END;
CREATE TRIGGER campaigns_update BEFORE UPDATE ON campaigns BEGIN SELECT RAISE(ABORT,'campaign immutable'); END;
CREATE TRIGGER campaigns_delete BEFORE DELETE ON campaigns BEGIN SELECT RAISE(ABORT,'campaign permanent'); END;
CREATE TABLE campaign_members (
 campaign_id INTEGER NOT NULL REFERENCES campaigns(id),
 account_id INTEGER NOT NULL REFERENCES accounts(id),
 revision INTEGER NOT NULL CHECK(revision>0),
 state TEXT NOT NULL CHECK(state IN ('active','inactive')),
 role TEXT NOT NULL CHECK(role IN ('player','master')),
 PRIMARY KEY(campaign_id,account_id)
) STRICT;
CREATE INDEX campaign_members_account ON campaign_members(account_id,campaign_id);
CREATE UNIQUE INDEX campaign_members_state_key ON campaign_members(campaign_id,account_id,state);
CREATE TABLE campaign_requests (
 campaign_id INTEGER NOT NULL REFERENCES campaigns(id),
 account_id INTEGER NOT NULL REFERENCES accounts(id),
 revision INTEGER NOT NULL CHECK(revision>0),
 state TEXT NOT NULL CHECK(state IN ('pending','accepted','declined')),
 PRIMARY KEY(campaign_id,account_id)
) STRICT;
CREATE TRIGGER campaign_members_insert BEFORE INSERT ON campaign_members BEGIN
 SELECT CASE WHEN NOT EXISTS(SELECT 1 FROM campaign_request_auth a JOIN campaigns c ON c.id=NEW.campaign_id WHERE (c.creator_id=a.account_id AND NEW.account_id=a.account_id) OR EXISTS(SELECT 1 FROM campaign_members m WHERE m.campaign_id=c.id AND m.account_id=a.account_id AND m.state='active' AND m.role='master')) THEN RAISE(ABORT,'membership auth required') END;
 SELECT CASE WHEN NEW.revision!=1 OR NEW.state!='active' OR EXISTS(SELECT 1 FROM campaign_members WHERE campaign_id=NEW.campaign_id AND account_id=NEW.account_id) OR NOT EXISTS(SELECT 1 FROM accounts WHERE id=NEW.account_id AND state='enabled') OR NEW.role!=CASE WHEN (SELECT creator_id FROM campaigns WHERE id=NEW.campaign_id)=NEW.account_id THEN 'master' ELSE 'player' END THEN RAISE(ABORT,'invalid initial membership') END;
 SELECT CASE WHEN NOT EXISTS(SELECT 1 FROM campaign_requests WHERE campaign_id=NEW.campaign_id AND account_id=NEW.account_id) AND (SELECT count(*) FROM (SELECT account_id FROM campaign_members WHERE campaign_id=NEW.campaign_id UNION SELECT account_id FROM campaign_requests WHERE campaign_id=NEW.campaign_id))>=64 THEN RAISE(ABORT,'campaign reservation limit') END;
END;
CREATE TRIGGER campaign_members_update BEFORE UPDATE ON campaign_members BEGIN
 SELECT CASE WHEN NOT EXISTS(SELECT 1 FROM campaign_request_auth a JOIN campaigns c ON c.id=NEW.campaign_id WHERE (OLD.state='active' AND NEW.state='inactive' AND (a.account_id=NEW.account_id OR (OLD.role='player' AND EXISTS(SELECT 1 FROM campaign_members m WHERE m.campaign_id=c.id AND m.account_id=a.account_id AND m.state='active' AND m.role='master')))) OR (OLD.state='inactive' AND NEW.state='active' AND EXISTS(SELECT 1 FROM campaign_members m WHERE m.campaign_id=c.id AND m.account_id=a.account_id AND m.state='active' AND m.role='master')) OR (OLD.state='active' AND NEW.state='active' AND OLD.role!=NEW.role AND c.creator_id=a.account_id)) THEN RAISE(ABORT,'membership auth required') END;
 SELECT CASE WHEN NEW.campaign_id IS NOT OLD.campaign_id OR NEW.account_id IS NOT OLD.account_id OR OLD.revision=9223372036854775807 OR NEW.revision!=OLD.revision+1 OR (SELECT creator_id FROM campaigns WHERE id=OLD.campaign_id)=OLD.account_id OR NOT ((OLD.state='active' AND NEW.state='inactive' AND NEW.role=OLD.role) OR (OLD.state='active' AND NEW.state='active' AND NEW.role!=OLD.role) OR (OLD.state='inactive' AND NEW.state='active' AND NEW.role='player')) OR (NEW.state='active' AND (OLD.state='inactive' OR (OLD.role='player' AND NEW.role='master')) AND NOT EXISTS(SELECT 1 FROM accounts WHERE id=NEW.account_id AND state='enabled')) THEN RAISE(ABORT,'invalid membership transition') END;
END;
CREATE TRIGGER campaign_members_delete BEFORE DELETE ON campaign_members BEGIN SELECT RAISE(ABORT,'membership permanent'); END;
CREATE TRIGGER campaign_requests_insert BEFORE INSERT ON campaign_requests BEGIN
 SELECT CASE WHEN NOT EXISTS(SELECT 1 FROM campaign_request_auth WHERE account_id=NEW.account_id) THEN RAISE(ABORT,'request auth required') END;
 SELECT CASE WHEN NEW.revision!=1 OR NEW.state!='pending' OR EXISTS(SELECT 1 FROM campaign_requests WHERE campaign_id=NEW.campaign_id AND account_id=NEW.account_id) OR EXISTS(SELECT 1 FROM campaign_members WHERE campaign_id=NEW.campaign_id AND account_id=NEW.account_id AND state='active') OR NOT EXISTS(SELECT 1 FROM accounts WHERE id=NEW.account_id AND state='enabled') THEN RAISE(ABORT,'invalid join request') END;
 SELECT CASE WHEN NOT EXISTS(SELECT 1 FROM campaign_members WHERE campaign_id=NEW.campaign_id AND account_id=NEW.account_id) AND (SELECT count(*) FROM (SELECT account_id FROM campaign_members WHERE campaign_id=NEW.campaign_id UNION SELECT account_id FROM campaign_requests WHERE campaign_id=NEW.campaign_id))>=64 THEN RAISE(ABORT,'campaign reservation limit') END;
END;
CREATE TRIGGER campaign_requests_update BEFORE UPDATE ON campaign_requests BEGIN
 SELECT CASE WHEN NOT EXISTS(SELECT 1 FROM campaign_request_auth a WHERE (NEW.state='pending' AND a.account_id=NEW.account_id) OR (OLD.state='pending' AND EXISTS(SELECT 1 FROM campaign_members m WHERE m.campaign_id=NEW.campaign_id AND m.account_id=a.account_id AND m.state='active' AND m.role='master'))) THEN RAISE(ABORT,'request auth required') END;
 SELECT CASE WHEN NEW.campaign_id IS NOT OLD.campaign_id OR NEW.account_id IS NOT OLD.account_id OR OLD.revision=9223372036854775807 OR NEW.revision!=OLD.revision+1 OR NOT ((OLD.state='pending' AND NEW.state IN ('accepted','declined')) OR (OLD.state IN ('accepted','declined') AND NEW.state='pending' AND NOT EXISTS(SELECT 1 FROM campaign_members WHERE campaign_id=NEW.campaign_id AND account_id=NEW.account_id AND state='active'))) THEN RAISE(ABORT,'invalid request transition') END;
END;
CREATE TRIGGER campaign_requests_delete BEFORE DELETE ON campaign_requests BEGIN SELECT RAISE(ABORT,'request permanent'); END;
CREATE TABLE private_sheets (
 id INTEGER PRIMARY KEY CHECK(id>0),
 campaign_id INTEGER NOT NULL REFERENCES campaigns(id),
 custodian_id INTEGER NOT NULL REFERENCES accounts(id),
 game_id TEXT NOT NULL,
 edition_id TEXT NOT NULL,
 kind TEXT NOT NULL CHECK(kind IN ('native','runtime')),
 pin TEXT NOT NULL CHECK(json_valid(pin) AND length(CAST(pin AS BLOB))<=4096),
 revision INTEGER NOT NULL CHECK(revision>0),
 state TEXT NOT NULL CHECK(state IN ('active','frozen') AND (state='frozen' OR revision<9223372036854775807)),
 snapshot TEXT NOT NULL CHECK(json_valid(snapshot) AND length(CAST(snapshot AS BLOB))<=1048576),
 hash TEXT NOT NULL CHECK(length(hash)=64 AND length(CAST(hash AS BLOB))=64 AND hash NOT GLOB '*[^0-9a-f]*'),
 custodian_active TEXT GENERATED ALWAYS AS (CASE WHEN state='active' THEN 'active' END) STORED,
 definition_version_id INTEGER GENERATED ALWAYS AS (CASE WHEN kind='runtime' THEN json_extract(pin,'$.version_id') END) STORED REFERENCES definition_versions(id),
 FOREIGN KEY(game_id,edition_id) REFERENCES registry_editions(game_id,edition_id),
 FOREIGN KEY(campaign_id,custodian_id,custodian_active) REFERENCES campaign_members(campaign_id,account_id,state) DEFERRABLE INITIALLY DEFERRED,
 FOREIGN KEY(id,revision,hash) REFERENCES private_sheet_history(sheet_id,revision,hash) DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE INDEX private_sheets_custody ON private_sheets(campaign_id,custodian_id,id);
CREATE INDEX private_sheets_personal ON private_sheets(custodian_id,id);
CREATE TABLE private_sheet_history (
 sheet_id INTEGER NOT NULL REFERENCES private_sheets(id),
 revision INTEGER NOT NULL CHECK(revision>0),
 hash TEXT NOT NULL,
 snapshot TEXT NOT NULL,
 operation TEXT NOT NULL CHECK(operation IN ('create','edit','freeze')),
 previous_revision INTEGER NOT NULL CHECK(previous_revision>=0),
 actor_id INTEGER NOT NULL REFERENCES accounts(id),
 actor_epoch INTEGER NOT NULL CHECK(actor_epoch>0),
 session_digest TEXT NOT NULL CHECK(length(session_digest)=64 AND session_digest NOT GLOB '*[^0-9a-f]*'),
 occurred_at INTEGER NOT NULL CHECK(occurred_at BETWEEN 1 AND 253402300799),
 PRIMARY KEY(sheet_id,revision),
 UNIQUE(sheet_id,revision,hash)
) STRICT;
CREATE TRIGGER private_sheets_insert BEFORE INSERT ON private_sheets BEGIN
 SELECT CASE WHEN NOT EXISTS(SELECT 1 FROM campaign_request_auth a JOIN campaign_members m ON m.account_id=a.account_id AND m.campaign_id=NEW.campaign_id AND m.state='active' WHERE a.account_id=NEW.custodian_id) OR NOT EXISTS(SELECT 1 FROM campaign_members m JOIN accounts a ON a.id=m.account_id WHERE m.campaign_id=NEW.campaign_id AND m.role='master' AND m.state='active' AND a.state='enabled') THEN RAISE(ABORT,'private auth required') END;
 SELECT CASE WHEN (NEW.kind='native' AND (NEW.game_id!='mork-borg' OR NEW.edition_id!='it-2025' OR json_extract(NEW.pin,'$.type') IS NOT 'native' OR json_extract(NEW.pin,'$.adapter') IS NOT 'mork-borg-it-2025' OR json_extract(NEW.pin,'$.schema') IS NOT 1 OR json_extract(NEW.pin,'$.calculation_version') IS NOT 1)) OR (NEW.kind='runtime' AND (json_extract(NEW.pin,'$.type') IS NOT 'runtime' OR json_extract(NEW.pin,'$.schema') IS NOT 1 OR NOT EXISTS(SELECT 1 FROM definition_versions v WHERE v.id=json_extract(NEW.pin,'$.version_id') AND v.version=json_extract(NEW.pin,'$.version') AND v.content_hash=json_extract(NEW.pin,'$.content_hash') AND v.game_id=NEW.game_id AND v.edition_id=NEW.edition_id))) THEN RAISE(ABORT,'private pin relation') END;
 SELECT CASE WHEN NEW.revision!=1 OR NEW.state!='active' OR EXISTS(SELECT 1 FROM private_sheets WHERE id=NEW.id) OR NOT EXISTS(SELECT 1 FROM campaigns WHERE id=NEW.campaign_id AND game_id=NEW.game_id AND edition_id=NEW.edition_id) OR (SELECT count(*) FROM private_sheets)>=2048 OR (SELECT count(*) FROM private_sheets WHERE campaign_id=NEW.campaign_id AND custodian_id=NEW.custodian_id)>=32 THEN RAISE(ABORT,'invalid private creation') END;
END;
CREATE TRIGGER private_sheets_update BEFORE UPDATE ON private_sheets BEGIN
 SELECT CASE WHEN NOT EXISTS(SELECT 1 FROM campaign_request_auth a WHERE (NEW.state='active' AND EXISTS(SELECT 1 FROM campaign_members m WHERE m.campaign_id=NEW.campaign_id AND m.account_id=a.account_id AND m.state='active' AND (m.role='master' OR a.account_id=NEW.custodian_id)) AND EXISTS(SELECT 1 FROM campaign_members m JOIN accounts enabled ON enabled.id=m.account_id WHERE m.campaign_id=NEW.campaign_id AND m.state='active' AND m.role='master' AND enabled.state='enabled')) OR (NEW.state='frozen' AND EXISTS(SELECT 1 FROM campaign_members m WHERE m.campaign_id=NEW.campaign_id AND m.account_id=NEW.custodian_id AND m.state='inactive') AND (a.account_id=NEW.custodian_id OR EXISTS(SELECT 1 FROM campaign_members m WHERE m.campaign_id=NEW.campaign_id AND m.account_id=a.account_id AND m.state='active' AND m.role='master')))) THEN RAISE(ABORT,'private auth required') END;
 SELECT CASE WHEN NEW.id IS NOT OLD.id OR NEW.campaign_id IS NOT OLD.campaign_id OR NEW.custodian_id IS NOT OLD.custodian_id OR NEW.game_id IS NOT OLD.game_id OR NEW.edition_id IS NOT OLD.edition_id OR NEW.kind IS NOT OLD.kind OR NEW.pin IS NOT OLD.pin OR OLD.state!='active' OR OLD.revision=9223372036854775807 OR NEW.revision!=OLD.revision+1 THEN RAISE(ABORT,'invalid private transition') END;
END;
CREATE TRIGGER private_sheets_delete BEFORE DELETE ON private_sheets BEGIN SELECT RAISE(ABORT,'private sheets permanent'); END;
CREATE TRIGGER private_sheet_history_insert BEFORE INSERT ON private_sheet_history BEGIN
 SELECT CASE WHEN NOT EXISTS(SELECT 1 FROM campaign_request_auth WHERE account_id=NEW.actor_id AND epoch=NEW.actor_epoch) THEN RAISE(ABORT,'history auth required') END;
 SELECT CASE WHEN EXISTS(SELECT 1 FROM private_sheet_history WHERE sheet_id=NEW.sheet_id AND revision=NEW.revision) OR NOT EXISTS(SELECT 1 FROM private_sheets s WHERE s.id=NEW.sheet_id AND s.revision=NEW.revision AND s.hash=NEW.hash AND s.snapshot=NEW.snapshot AND json_extract(NEW.snapshot,'$.sheet_id') IS s.id AND json_extract(NEW.snapshot,'$.campaign_id') IS s.campaign_id AND json_extract(NEW.snapshot,'$.custodian_id') IS s.custodian_id AND json_extract(NEW.snapshot,'$.game_id') IS s.game_id AND json_extract(NEW.snapshot,'$.edition_id') IS s.edition_id AND json_extract(NEW.snapshot,'$.kind') IS s.kind AND json_extract(NEW.snapshot,'$.pin') IS json(s.pin) AND json_extract(NEW.snapshot,'$.revision') IS s.revision AND json_extract(NEW.snapshot,'$.state') IS s.state AND s.state=CASE NEW.operation WHEN 'freeze' THEN 'frozen' ELSE 'active' END) OR NOT ((NEW.operation='create' AND NEW.previous_revision=0 AND NEW.revision=1) OR (NEW.operation IN ('edit','freeze') AND NEW.previous_revision>0 AND NEW.previous_revision<9223372036854775807 AND NEW.revision=NEW.previous_revision+1 AND EXISTS(SELECT 1 FROM private_sheet_history WHERE sheet_id=NEW.sheet_id AND revision=NEW.previous_revision AND json_extract(snapshot,'$.state')='active'))) OR NOT EXISTS(SELECT 1 FROM accounts WHERE id=NEW.actor_id AND epoch=NEW.actor_epoch AND state='enabled') THEN RAISE(ABORT,'invalid private history') END;
END;
CREATE TRIGGER private_sheet_history_update BEFORE UPDATE ON private_sheet_history BEGIN SELECT RAISE(ABORT,'history append only'); END;
CREATE TRIGGER private_sheet_history_delete BEFORE DELETE ON private_sheet_history BEGIN SELECT RAISE(ABORT,'history permanent'); END;
CREATE VIEW private_sheet_events_valid AS
 SELECT h.* FROM private_sheet_history h JOIN private_sheets s ON s.id=h.sheet_id
 WHERE CASE WHEN length(CAST(h.snapshot AS BLOB))<=1048576 THEN CASE WHEN json_valid(h.snapshot) THEN
  h.actor_id>0 AND h.actor_epoch>0 AND h.occurred_at BETWEEN 1 AND 253402300799
  AND EXISTS(SELECT 1 FROM accounts a WHERE a.id=h.actor_id AND a.epoch>=h.actor_epoch)
  AND length(CAST(h.hash AS BLOB))=64 AND h.hash NOT GLOB '*[^0-9a-f]*'
  AND length(CAST(h.session_digest AS BLOB))=64 AND h.session_digest NOT GLOB '*[^0-9a-f]*'
  AND json_type(h.snapshot,'$.encoding')='integer' AND json_extract(h.snapshot,'$.encoding')=1
  AND json_type(h.snapshot,'$.sheet_id')='integer' AND json_extract(h.snapshot,'$.sheet_id')=s.id
  AND json_type(h.snapshot,'$.campaign_id')='integer' AND json_extract(h.snapshot,'$.campaign_id')=s.campaign_id
  AND json_type(h.snapshot,'$.custodian_id')='integer' AND json_extract(h.snapshot,'$.custodian_id')=s.custodian_id
  AND json_type(h.snapshot,'$.revision')='integer' AND json_extract(h.snapshot,'$.revision')=h.revision
  AND json_extract(h.snapshot,'$.game_id')=s.game_id AND json_extract(h.snapshot,'$.edition_id')=s.edition_id
  AND json_extract(h.snapshot,'$.kind')=s.kind AND json_extract(h.snapshot,'$.pin')=json(s.pin)
  AND ((h.revision=1 AND h.previous_revision=0 AND h.operation='create' AND json_extract(h.snapshot,'$.state')='active')
    OR (h.revision>1 AND h.previous_revision=h.revision-1 AND h.operation IN ('edit','freeze')
      AND json_extract(h.snapshot,'$.state')=CASE h.operation WHEN 'freeze' THEN 'frozen' ELSE 'active' END
       AND EXISTS(SELECT 1 FROM private_sheet_history p WHERE p.sheet_id=h.sheet_id AND p.revision=h.previous_revision
         AND CASE WHEN length(CAST(p.snapshot AS BLOB))<=1048576 THEN CASE WHEN json_valid(p.snapshot) THEN
           json_extract(p.snapshot,'$.state')='active' AND json_extract(p.snapshot,'$.pin')=json_extract(h.snapshot,'$.pin')
         ELSE 0 END ELSE 0 END)))
 ELSE 0 END ELSE 0 END;
CREATE TABLE campaign_events (
 id INTEGER PRIMARY KEY CHECK(id>0),
 campaign_id INTEGER NOT NULL REFERENCES campaigns(id),
 actor_id INTEGER NOT NULL REFERENCES accounts(id),
 actor_epoch INTEGER NOT NULL CHECK(actor_epoch>0),
 target_id INTEGER NOT NULL REFERENCES accounts(id),
 action TEXT NOT NULL CHECK(action IN ('create','request','accept','decline','promote','demote','leave','remove')),
 occurred_at INTEGER NOT NULL CHECK(occurred_at BETWEEN 1 AND 253402300799)
) STRICT;
CREATE TRIGGER campaign_events_insert BEFORE INSERT ON campaign_events WHEN EXISTS(SELECT 1 FROM campaign_events WHERE id=NEW.id) OR NOT EXISTS(SELECT 1 FROM campaign_request_auth WHERE account_id=NEW.actor_id AND epoch=NEW.actor_epoch) BEGIN SELECT RAISE(ABORT,'invalid campaign event'); END;
CREATE TRIGGER campaign_events_update BEFORE UPDATE ON campaign_events BEGIN SELECT RAISE(ABORT,'events append only'); END;
CREATE TRIGGER campaign_events_delete BEFORE DELETE ON campaign_events BEGIN SELECT RAISE(ABORT,'events permanent'); END;
PRAGMA user_version=12;
