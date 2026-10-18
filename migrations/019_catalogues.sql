-- Read-only catalogues (docs/catalogue-plan.md). Step 1: revisioned drafts only; later steps of
-- the same unmerged branch add versions, projections, seals and the name index here.
CREATE TABLE catalogue_drafts (
    id INTEGER PRIMARY KEY AUTOINCREMENT CHECK (id > 0),
    game_id TEXT NOT NULL CHECK (instr(game_id,char(0))=0 AND length(CAST(game_id AS BLOB)) BETWEEN 1 AND 64 AND game_id GLOB '[a-z]*' AND game_id NOT GLOB '*[^a-z0-9-]*'),
    edition_id TEXT NOT NULL CHECK (instr(edition_id,char(0))=0 AND length(CAST(edition_id AS BLOB)) BETWEEN 1 AND 64 AND edition_id GLOB '[a-z]*' AND edition_id NOT GLOB '*[^a-z0-9-]*'),
    catalogue_id TEXT NOT NULL CHECK (instr(catalogue_id,char(0))=0 AND length(CAST(catalogue_id AS BLOB)) BETWEEN 1 AND 64 AND catalogue_id GLOB '[a-z]*' AND catalogue_id NOT GLOB '*[^a-z0-9-]*'),
    revision INTEGER NOT NULL CHECK (revision > 0),
    content TEXT NOT NULL CHECK (length(CAST(content AS BLOB)) <= 4194304 AND json_valid(content) AND json_type(content)='object'),
    content_hash TEXT NOT NULL CHECK (instr(content_hash,char(0))=0 AND length(CAST(content_hash AS BLOB))=64 AND content_hash NOT GLOB '*[^0-9a-f]*'),
    CHECK (json_type(content,'$.contract_version')='integer' AND json_extract(content,'$.contract_version') IS 1 AND json_extract(content,'$.game_id') IS game_id AND json_extract(content,'$.edition_id') IS edition_id AND json_extract(content,'$.catalogue_id') IS catalogue_id),
    UNIQUE (game_id,edition_id,catalogue_id),
    FOREIGN KEY (game_id,edition_id) REFERENCES registry_editions(game_id,edition_id) ON DELETE RESTRICT
) STRICT;
CREATE TRIGGER catalogue_draft_identity_immutable BEFORE UPDATE ON catalogue_drafts
WHEN NEW.id<>OLD.id OR NEW.game_id<>OLD.game_id OR NEW.edition_id<>OLD.edition_id OR NEW.catalogue_id<>OLD.catalogue_id
BEGIN SELECT RAISE(ABORT,'Catalogue draft identity is immutable'); END;
PRAGMA user_version=19;
