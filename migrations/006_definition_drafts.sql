CREATE TABLE definition_drafts (
    id INTEGER PRIMARY KEY AUTOINCREMENT CHECK (id > 0),
    game_id TEXT NOT NULL CHECK (instr(game_id,char(0))=0 AND length(CAST(game_id AS BLOB)) BETWEEN 1 AND 64 AND game_id GLOB '[a-z]*' AND game_id NOT GLOB '*[^a-z0-9-]*'),
    edition_id TEXT NOT NULL CHECK (instr(edition_id,char(0))=0 AND length(CAST(edition_id AS BLOB)) BETWEEN 1 AND 64 AND edition_id GLOB '[a-z]*' AND edition_id NOT GLOB '*[^a-z0-9-]*'),
    definition_id TEXT NOT NULL CHECK (instr(definition_id,char(0))=0 AND length(CAST(definition_id AS BLOB)) BETWEEN 1 AND 64 AND definition_id GLOB '[a-z]*' AND definition_id NOT GLOB '*[^a-z0-9-]*'),
    revision INTEGER NOT NULL CHECK (revision > 0),
    content TEXT NOT NULL CHECK (length(CAST(content AS BLOB)) <= 262144 AND json_valid(content) AND json_type(content)='object'),
    content_hash TEXT NOT NULL CHECK (instr(content_hash,char(0))=0 AND length(CAST(content_hash AS BLOB))=64 AND content_hash NOT GLOB '*[^0-9a-f]*'),
    CHECK (json_type(content,'$.contract_version')='integer' AND json_extract(content,'$.contract_version') IS 1 AND json_extract(content,'$.game_id') IS game_id AND json_extract(content,'$.edition_id') IS edition_id AND json_extract(content,'$.definition_id') IS definition_id),
    UNIQUE (game_id,edition_id,definition_id),
    FOREIGN KEY (game_id,edition_id) REFERENCES registry_editions(game_id,edition_id) ON DELETE RESTRICT
) STRICT;
CREATE TRIGGER definition_draft_identity_immutable BEFORE UPDATE ON definition_drafts
WHEN NEW.id<>OLD.id OR NEW.game_id<>OLD.game_id OR NEW.edition_id<>OLD.edition_id OR NEW.definition_id<>OLD.definition_id
BEGIN SELECT RAISE(ABORT,'Definition draft identity is immutable'); END;
PRAGMA user_version=6;
