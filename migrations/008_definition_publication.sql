CREATE TABLE definition_versions (
 id INTEGER PRIMARY KEY AUTOINCREMENT CHECK(id>0),
 game_id TEXT NOT NULL, edition_id TEXT NOT NULL, definition_id TEXT NOT NULL,
 version INTEGER NOT NULL CHECK(version>0),
 content TEXT NOT NULL CHECK(json_valid(content) AND json_type(content)='object' AND length(CAST(content AS BLOB))<=262144),
 content_hash TEXT NOT NULL CHECK(instr(content_hash,char(0))=0 AND length(CAST(content_hash AS BLOB))=64 AND content_hash NOT GLOB '*[^0-9a-f]*'),
 CHECK(json_extract(content,'$.game_id') IS game_id AND json_extract(content,'$.edition_id') IS edition_id AND json_extract(content,'$.definition_id') IS definition_id AND json_extract(content,'$.contract_version') IS 1),
 UNIQUE(game_id,edition_id,definition_id,version),
 FOREIGN KEY(game_id,edition_id) REFERENCES registry_editions(game_id,edition_id) ON DELETE RESTRICT
) STRICT;
CREATE TABLE definition_approvals (
 id INTEGER PRIMARY KEY AUTOINCREMENT CHECK(id>0),
 version_id INTEGER NOT NULL REFERENCES definition_versions(id) ON DELETE RESTRICT,
 encoding INTEGER NOT NULL CHECK(encoding=1),
 snapshot TEXT NOT NULL CHECK(json_valid(snapshot) AND json_type(snapshot)='object' AND length(CAST(snapshot AS BLOB))<=1048576),
 snapshot_hash TEXT NOT NULL CHECK(instr(snapshot_hash,char(0))=0 AND length(CAST(snapshot_hash AS BLOB))=64 AND snapshot_hash NOT GLOB '*[^0-9a-f]*'),
 actor TEXT NOT NULL CHECK(length(actor) BETWEEN 1 AND 100), rationale TEXT NOT NULL CHECK(length(rationale) BETWEEN 1 AND 5000),
 originals_compared INTEGER NOT NULL CHECK(originals_compared=1),
 target_kind TEXT NOT NULL CHECK(target_kind IN ('draft','version')),
 target_id INTEGER NOT NULL CHECK(target_id>0),
 target_revision INTEGER NOT NULL CHECK(target_revision>0),
 comparison_encoding INTEGER NOT NULL CHECK(comparison_encoding=1),
 comparison_hash TEXT NOT NULL CHECK(instr(comparison_hash,char(0))=0 AND length(CAST(comparison_hash AS BLOB))=64 AND comparison_hash NOT GLOB '*[^0-9a-f]*'),
 created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
) STRICT;
CREATE TABLE definition_withdrawals (
 id INTEGER PRIMARY KEY AUTOINCREMENT CHECK(id>0),
 approval_id INTEGER NOT NULL UNIQUE REFERENCES definition_approvals(id) ON DELETE RESTRICT,
 version_id INTEGER NOT NULL REFERENCES definition_versions(id) ON DELETE RESTRICT,
 actor TEXT NOT NULL CHECK(length(actor) BETWEEN 1 AND 100), rationale TEXT NOT NULL CHECK(length(rationale) BETWEEN 1 AND 5000),
 created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
) STRICT;
CREATE TRIGGER definition_versions_insert BEFORE INSERT ON definition_versions WHEN EXISTS(SELECT 1 FROM definition_versions WHERE id=NEW.id OR (game_id=NEW.game_id AND edition_id=NEW.edition_id AND definition_id=NEW.definition_id AND version=NEW.version)) BEGIN SELECT RAISE(ABORT,'Published version replacement rejected'); END;
CREATE TRIGGER definition_versions_update BEFORE UPDATE ON definition_versions BEGIN SELECT RAISE(ABORT,'Published versions are immutable'); END;
CREATE TRIGGER definition_versions_delete BEFORE DELETE ON definition_versions BEGIN SELECT RAISE(ABORT,'Published versions are retained'); END;
CREATE TRIGGER definition_approvals_insert BEFORE INSERT ON definition_approvals WHEN EXISTS(SELECT 1 FROM definition_approvals WHERE id=NEW.id) BEGIN SELECT RAISE(ABORT,'Approval replacement rejected'); END;
CREATE TRIGGER definition_approvals_update BEFORE UPDATE ON definition_approvals BEGIN SELECT RAISE(ABORT,'Approvals are immutable'); END;
CREATE TRIGGER definition_approvals_delete BEFORE DELETE ON definition_approvals BEGIN SELECT RAISE(ABORT,'Approvals are retained'); END;
CREATE TRIGGER definition_withdrawals_insert BEFORE INSERT ON definition_withdrawals WHEN EXISTS(SELECT 1 FROM definition_withdrawals WHERE id=NEW.id OR approval_id=NEW.approval_id) OR NOT EXISTS(SELECT 1 FROM definition_approvals WHERE id=NEW.approval_id AND version_id=NEW.version_id) BEGIN SELECT RAISE(ABORT,'Invalid withdrawal or replacement'); END;
CREATE TRIGGER definition_withdrawals_update BEFORE UPDATE ON definition_withdrawals BEGIN SELECT RAISE(ABORT,'Withdrawals are immutable'); END;
CREATE TRIGGER definition_withdrawals_delete BEFORE DELETE ON definition_withdrawals BEGIN SELECT RAISE(ABORT,'Withdrawals are retained'); END;
PRAGMA user_version=8;
