-- Read-only catalogues (docs/catalogue-plan.md). Revisioned drafts; immutable sealed versions
-- whose relational projections the seal trigger binds to the hashed content; and the derived
-- latest-only name index. One unmerged migration for plan steps 1-2.
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
-- Every draft rewrite advances the revision by exactly one, so content never changes under an
-- unchanged revision (versions keep their own content copy either way).
CREATE TRIGGER catalogue_draft_revision_advances BEFORE UPDATE ON catalogue_drafts
WHEN NEW.revision IS NOT OLD.revision+1
BEGIN SELECT RAISE(ABORT,'Catalogue draft revision must advance by one'); END;

-- Integer surrogate per (game, edition, catalogue) (R20): the name-index rowid is
-- id * 2^20 + entry ordinal, so the id stays below 2^43.
CREATE TABLE catalogues (
    id INTEGER PRIMARY KEY AUTOINCREMENT CHECK (id BETWEEN 1 AND 8796093022207),
    game_id TEXT NOT NULL CHECK (instr(game_id,char(0))=0 AND length(CAST(game_id AS BLOB)) BETWEEN 1 AND 64 AND game_id GLOB '[a-z]*' AND game_id NOT GLOB '*[^a-z0-9-]*'),
    edition_id TEXT NOT NULL CHECK (instr(edition_id,char(0))=0 AND length(CAST(edition_id AS BLOB)) BETWEEN 1 AND 64 AND edition_id GLOB '[a-z]*' AND edition_id NOT GLOB '*[^a-z0-9-]*'),
    catalogue_id TEXT NOT NULL CHECK (instr(catalogue_id,char(0))=0 AND length(CAST(catalogue_id AS BLOB)) BETWEEN 1 AND 64 AND catalogue_id GLOB '[a-z]*' AND catalogue_id NOT GLOB '*[^a-z0-9-]*'),
    UNIQUE (game_id,edition_id,catalogue_id),
    UNIQUE (id,game_id,edition_id,catalogue_id),
    FOREIGN KEY (game_id,edition_id) REFERENCES registry_editions(game_id,edition_id) ON DELETE RESTRICT
) STRICT;

-- Immutable published versions (every column, the admission profile included). Identity and
-- title columns must equal the content's; provenance (draft id and revision with identical
-- content and hash) is checked on insert.
CREATE TABLE catalogue_versions (
    id INTEGER PRIMARY KEY AUTOINCREMENT CHECK (id > 0),
    catalogue INTEGER NOT NULL,
    game_id TEXT NOT NULL,
    edition_id TEXT NOT NULL,
    catalogue_id TEXT NOT NULL,
    version INTEGER NOT NULL CHECK (version > 0),
    title TEXT NOT NULL CHECK (instr(title,char(0))=0 AND length(title) BETWEEN 1 AND 100),
    content TEXT NOT NULL CHECK (length(CAST(content AS BLOB)) <= 4194304 AND json_valid(content) AND json_type(content)='object'),
    content_hash TEXT NOT NULL CHECK (instr(content_hash,char(0))=0 AND length(CAST(content_hash AS BLOB))=64 AND content_hash NOT GLOB '*[^0-9a-f]*'),
    draft_id INTEGER NOT NULL REFERENCES catalogue_drafts(id) ON DELETE RESTRICT,
    draft_revision INTEGER NOT NULL CHECK (draft_revision > 0),
    comparison_hash TEXT NOT NULL CHECK (instr(comparison_hash,char(0))=0 AND length(CAST(comparison_hash AS BLOB))=64 AND comparison_hash NOT GLOB '*[^0-9a-f]*'),
    actor TEXT NOT NULL CHECK (instr(actor,char(0))=0 AND length(actor) BETWEEN 1 AND 100),
    rationale TEXT NOT NULL CHECK (instr(rationale,char(0))=0 AND length(rationale) BETWEEN 1 AND 5000),
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    -- The admission rules the content was published under; verification re-admits with them.
    -- A tightened rule set is a new profile id added here, never a change to an old one.
    admission_profile TEXT NOT NULL CHECK (admission_profile IN ('catalogue-admission-1')),
    CHECK (json_type(content,'$.contract_version')='integer' AND json_extract(content,'$.contract_version') IS 1 AND json_extract(content,'$.game_id') IS game_id AND json_extract(content,'$.edition_id') IS edition_id AND json_extract(content,'$.catalogue_id') IS catalogue_id AND json_extract(content,'$.title') IS title),
    UNIQUE (catalogue,version),
    FOREIGN KEY (catalogue,game_id,edition_id,catalogue_id) REFERENCES catalogues(id,game_id,edition_id,catalogue_id) ON DELETE RESTRICT
) STRICT;

-- Projections (R1/R17): everything readers use, bound to the content by the seal.
CREATE TABLE catalogue_entry_types (
    version_id INTEGER NOT NULL REFERENCES catalogue_versions(id) ON DELETE RESTRICT,
    type_id TEXT NOT NULL CHECK (instr(type_id,char(0))=0 AND length(CAST(type_id AS BLOB)) BETWEEN 1 AND 64 AND type_id GLOB '[a-z]*' AND type_id NOT GLOB '*[^a-z0-9-]*'),
    ordinal INTEGER NOT NULL CHECK (ordinal BETWEEN 0 AND 31),
    label TEXT NOT NULL CHECK (instr(label,char(0))=0 AND length(label) BETWEEN 1 AND 100),
    properties TEXT NOT NULL CHECK (length(CAST(properties AS BLOB)) <= 4194304 AND json_valid(properties) AND json_type(properties)='array'),
    PRIMARY KEY (version_id,type_id),
    UNIQUE (version_id,ordinal)
) STRICT, WITHOUT ROWID;
CREATE TABLE catalogue_entries (
    version_id INTEGER NOT NULL REFERENCES catalogue_versions(id) ON DELETE RESTRICT,
    key TEXT NOT NULL CHECK (instr(key,char(0))=0 AND length(CAST(key AS BLOB)) BETWEEN 1 AND 64 AND key GLOB '[a-z]*' AND key NOT GLOB '*[^a-z0-9-]*'),
    ordinal INTEGER NOT NULL CHECK (ordinal BETWEEN 0 AND 8191),
    type_id TEXT NOT NULL,
    name TEXT NOT NULL CHECK (instr(name,char(0))=0 AND length(name) BETWEEN 1 AND 100),
    retired INTEGER NOT NULL CHECK (retired IN (0,1)),
    desc_kind TEXT NOT NULL CHECK (desc_kind IN ('units','owner_text')),
    owner_text TEXT CHECK (owner_text IS NULL OR (instr(owner_text,char(0))=0 AND length(owner_text) BETWEEN 1 AND 20000)),
    properties TEXT NOT NULL CHECK (length(CAST(properties AS BLOB)) <= 262144 AND json_valid(properties) AND json_type(properties)='object'),
    CHECK ((desc_kind='owner_text') = (owner_text IS NOT NULL)),
    PRIMARY KEY (version_id,key),
    UNIQUE (version_id,ordinal),
    FOREIGN KEY (version_id,type_id) REFERENCES catalogue_entry_types(version_id,type_id) ON DELETE RESTRICT
) STRICT, WITHOUT ROWID;
CREATE INDEX catalogue_entries_browse ON catalogue_entries(version_id,type_id,name,key);
-- Only unit_id has a foreign key (R2); page and source are checked by trigger on insert and
-- derived through joins at read time.
CREATE TABLE catalogue_entry_units (
    version_id INTEGER NOT NULL,
    key TEXT NOT NULL,
    ordinal INTEGER NOT NULL CHECK (ordinal BETWEEN 0 AND 15),
    unit_id INTEGER NOT NULL REFERENCES source_units(id) ON DELETE RESTRICT,
    page_id INTEGER NOT NULL CHECK (page_id > 0),
    source_id INTEGER NOT NULL CHECK (source_id > 0),
    PRIMARY KEY (version_id,key,ordinal),
    FOREIGN KEY (version_id,key) REFERENCES catalogue_entries(version_id,key) ON DELETE RESTRICT
) STRICT, WITHOUT ROWID;
CREATE INDEX catalogue_entry_units_unit ON catalogue_entry_units(unit_id);
CREATE TABLE catalogue_seals (
    version_id INTEGER PRIMARY KEY REFERENCES catalogue_versions(id) ON DELETE RESTRICT,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
) STRICT;

-- Derived latest-only name index (R2/R20), exempt from the immutability triggers and repaired
-- by `rebuild-catalogue-index`. rowid = catalogues.id * 2^20 + entry ordinal. The statement
-- is repeated verbatim in src/catalogues/publication.rs (NAME_INDEX), checked by a test.
CREATE VIRTUAL TABLE catalogue_names_fts USING fts5(name, catalogue_row_id UNINDEXED, key UNINDEXED, tokenize = 'unicode61 remove_diacritics 2', prefix = '3');

-- Pre-insert replacement guards on every primary and unique key (R19, the 008 pattern):
-- INSERT OR REPLACE deletes conflicting rows without firing delete triggers. One EXISTS per
-- key, so each probe uses that key's index (an OR scans the version: quadratic publication).
CREATE TRIGGER catalogues_insert BEFORE INSERT ON catalogues
WHEN EXISTS(SELECT 1 FROM catalogues WHERE id=NEW.id)
  OR EXISTS(SELECT 1 FROM catalogues WHERE game_id=NEW.game_id AND edition_id=NEW.edition_id AND catalogue_id=NEW.catalogue_id)
BEGIN SELECT RAISE(ABORT,'Catalogue identity replacement rejected'); END;
CREATE TRIGGER catalogues_update BEFORE UPDATE ON catalogues BEGIN SELECT RAISE(ABORT,'Catalogue identities are immutable'); END;
CREATE TRIGGER catalogues_delete BEFORE DELETE ON catalogues BEGIN SELECT RAISE(ABORT,'Catalogue identities are retained'); END;

CREATE TRIGGER catalogue_versions_insert BEFORE INSERT ON catalogue_versions
WHEN EXISTS(SELECT 1 FROM catalogue_versions WHERE id=NEW.id)
  OR EXISTS(SELECT 1 FROM catalogue_versions WHERE catalogue=NEW.catalogue AND version=NEW.version)
BEGIN SELECT RAISE(ABORT,'Catalogue version replacement rejected'); END;
-- Versions are numbered 1, 2, ...; a new one starts only once every earlier one is sealed,
-- and its content is exactly the named draft revision's.
CREATE TRIGGER catalogue_versions_order BEFORE INSERT ON catalogue_versions
WHEN NEW.version IS NOT (SELECT coalesce(max(version),0)+1 FROM catalogue_versions WHERE catalogue=NEW.catalogue)
  OR EXISTS(SELECT 1 FROM catalogue_versions v WHERE v.catalogue=NEW.catalogue AND NOT EXISTS(SELECT 1 FROM catalogue_seals s WHERE s.version_id=v.id))
  OR NOT EXISTS(SELECT 1 FROM catalogue_drafts d WHERE d.id=NEW.draft_id AND d.revision=NEW.draft_revision AND d.game_id=NEW.game_id AND d.edition_id=NEW.edition_id AND d.catalogue_id=NEW.catalogue_id AND d.content_hash=NEW.content_hash AND d.content=NEW.content)
BEGIN SELECT RAISE(ABORT,'Catalogue version order or provenance rejected'); END;
CREATE TRIGGER catalogue_versions_update BEFORE UPDATE ON catalogue_versions BEGIN SELECT RAISE(ABORT,'Catalogue versions are immutable'); END;
CREATE TRIGGER catalogue_versions_delete BEFORE DELETE ON catalogue_versions BEGIN SELECT RAISE(ABORT,'Catalogue versions are retained'); END;

CREATE TRIGGER catalogue_entry_types_insert BEFORE INSERT ON catalogue_entry_types
WHEN EXISTS(SELECT 1 FROM catalogue_entry_types WHERE version_id=NEW.version_id AND type_id=NEW.type_id)
  OR EXISTS(SELECT 1 FROM catalogue_entry_types WHERE version_id=NEW.version_id AND ordinal=NEW.ordinal)
BEGIN SELECT RAISE(ABORT,'Catalogue entry type replacement rejected'); END;
CREATE TRIGGER catalogue_entry_types_sealed BEFORE INSERT ON catalogue_entry_types
WHEN EXISTS(SELECT 1 FROM catalogue_seals WHERE version_id=NEW.version_id)
BEGIN SELECT RAISE(ABORT,'Catalogue version is sealed'); END;
CREATE TRIGGER catalogue_entry_types_update BEFORE UPDATE ON catalogue_entry_types BEGIN SELECT RAISE(ABORT,'Catalogue entry types are immutable'); END;
CREATE TRIGGER catalogue_entry_types_delete BEFORE DELETE ON catalogue_entry_types BEGIN SELECT RAISE(ABORT,'Catalogue entry types are retained'); END;

CREATE TRIGGER catalogue_entries_insert BEFORE INSERT ON catalogue_entries
WHEN EXISTS(SELECT 1 FROM catalogue_entries WHERE version_id=NEW.version_id AND key=NEW.key)
  OR EXISTS(SELECT 1 FROM catalogue_entries WHERE version_id=NEW.version_id AND ordinal=NEW.ordinal)
BEGIN SELECT RAISE(ABORT,'Catalogue entry replacement rejected'); END;
CREATE TRIGGER catalogue_entries_sealed BEFORE INSERT ON catalogue_entries
WHEN EXISTS(SELECT 1 FROM catalogue_seals WHERE version_id=NEW.version_id)
BEGIN SELECT RAISE(ABORT,'Catalogue version is sealed'); END;
CREATE TRIGGER catalogue_entries_update BEFORE UPDATE ON catalogue_entries BEGIN SELECT RAISE(ABORT,'Catalogue entries are immutable'); END;
CREATE TRIGGER catalogue_entries_delete BEFORE DELETE ON catalogue_entries BEGIN SELECT RAISE(ABORT,'Catalogue entries are retained'); END;

CREATE TRIGGER catalogue_entry_units_insert BEFORE INSERT ON catalogue_entry_units
WHEN EXISTS(SELECT 1 FROM catalogue_entry_units WHERE version_id=NEW.version_id AND key=NEW.key AND ordinal=NEW.ordinal)
BEGIN SELECT RAISE(ABORT,'Catalogue entry unit replacement rejected'); END;
CREATE TRIGGER catalogue_entry_units_sealed BEFORE INSERT ON catalogue_entry_units
WHEN EXISTS(SELECT 1 FROM catalogue_seals WHERE version_id=NEW.version_id)
BEGIN SELECT RAISE(ABORT,'Catalogue version is sealed'); END;
-- R17: the unit is on that page, the page is in that source, and the source is finalized and
-- of the catalogue's pair.
CREATE TRIGGER catalogue_entry_units_source BEFORE INSERT ON catalogue_entry_units
WHEN NOT EXISTS(SELECT 1 FROM source_units u JOIN source_pages p ON p.id=u.page_id
  JOIN source_finalizations f ON f.source_id=p.source_id
  JOIN source_rulesets r ON r.source_id=p.source_id
  JOIN catalogue_versions v ON v.id=NEW.version_id
  WHERE u.id=NEW.unit_id AND u.page_id=NEW.page_id AND p.source_id=NEW.source_id
    AND r.game_id=v.game_id AND r.edition_id=v.edition_id)
BEGIN SELECT RAISE(ABORT,'Catalogue unit, page, source or pair mismatch'); END;
CREATE TRIGGER catalogue_entry_units_update BEFORE UPDATE ON catalogue_entry_units BEGIN SELECT RAISE(ABORT,'Catalogue entry units are immutable'); END;
CREATE TRIGGER catalogue_entry_units_delete BEFORE DELETE ON catalogue_entry_units BEGIN SELECT RAISE(ABORT,'Catalogue entry units are retained'); END;

CREATE TRIGGER catalogue_seals_insert BEFORE INSERT ON catalogue_seals
WHEN EXISTS(SELECT 1 FROM catalogue_seals WHERE version_id=NEW.version_id)
BEGIN SELECT RAISE(ABORT,'Catalogue seal replacement rejected'); END;
-- The seal (R1/R17/R18/R19). Equality is linear: two EXCEPT queries per projection over
-- json_each of the content, plus exact row counts. Only then the append-only comparison with
-- the previous sealed version, over projections now proven equal to their content.
CREATE TRIGGER catalogue_seals_check BEFORE INSERT ON catalogue_seals
BEGIN
  SELECT RAISE(ABORT,'Catalogue seal: version order') WHERE
    (SELECT version FROM catalogue_versions WHERE id=NEW.version_id) IS NOT
      coalesce((SELECT max(v.version) FROM catalogue_seals s JOIN catalogue_versions v ON v.id=s.version_id
        WHERE v.catalogue=(SELECT catalogue FROM catalogue_versions WHERE id=NEW.version_id)),0)+1
    OR EXISTS(SELECT 1 FROM catalogue_versions v WHERE v.catalogue=(SELECT catalogue FROM catalogue_versions WHERE id=NEW.version_id)
      AND v.version<(SELECT version FROM catalogue_versions WHERE id=NEW.version_id)
      AND NOT EXISTS(SELECT 1 FROM catalogue_seals s WHERE s.version_id=v.id));
  SELECT RAISE(ABORT,'Catalogue seal: entry types differ from content') WHERE EXISTS(
      SELECT t.value->>'$.id', t.key, t.value->>'$.label', t.value->'$.properties'
      FROM catalogue_versions v, json_each(v.content,'$.entry_types') t WHERE v.id=NEW.version_id
      EXCEPT SELECT type_id, ordinal, label, properties FROM catalogue_entry_types WHERE version_id=NEW.version_id)
    OR EXISTS(SELECT type_id, ordinal, label, properties FROM catalogue_entry_types WHERE version_id=NEW.version_id
      EXCEPT SELECT t.value->>'$.id', t.key, t.value->>'$.label', t.value->'$.properties'
      FROM catalogue_versions v, json_each(v.content,'$.entry_types') t WHERE v.id=NEW.version_id)
    OR (SELECT json_array_length(content,'$.entry_types') FROM catalogue_versions WHERE id=NEW.version_id)
      IS NOT (SELECT count(*) FROM catalogue_entry_types WHERE version_id=NEW.version_id);
  SELECT RAISE(ABORT,'Catalogue seal: entries differ from content') WHERE EXISTS(
      SELECT e.value->>'$.key', e.key, e.value->>'$.type', e.value->>'$.name', coalesce(e.value->>'$.retired',0),
        CASE WHEN e.value->'$.description.owner_text' IS NULL THEN 'units' ELSE 'owner_text' END,
        e.value->>'$.description.owner_text', e.value->'$.properties'
      FROM catalogue_versions v, json_each(v.content,'$.entries') e WHERE v.id=NEW.version_id
      EXCEPT SELECT key, ordinal, type_id, name, retired, desc_kind, owner_text, properties
      FROM catalogue_entries WHERE version_id=NEW.version_id)
    OR EXISTS(SELECT key, ordinal, type_id, name, retired, desc_kind, owner_text, properties
      FROM catalogue_entries WHERE version_id=NEW.version_id
      EXCEPT SELECT e.value->>'$.key', e.key, e.value->>'$.type', e.value->>'$.name', coalesce(e.value->>'$.retired',0),
        CASE WHEN e.value->'$.description.owner_text' IS NULL THEN 'units' ELSE 'owner_text' END,
        e.value->>'$.description.owner_text', e.value->'$.properties'
      FROM catalogue_versions v, json_each(v.content,'$.entries') e WHERE v.id=NEW.version_id)
    OR (SELECT json_array_length(content,'$.entries') FROM catalogue_versions WHERE id=NEW.version_id)
      IS NOT (SELECT count(*) FROM catalogue_entries WHERE version_id=NEW.version_id);
  SELECT RAISE(ABORT,'Catalogue seal: entry units differ from content') WHERE EXISTS(
      SELECT e.value->>'$.key', u.key, u.value->>'$.unit_id', u.value->>'$.page_id', u.value->>'$.source_id'
      FROM catalogue_versions v, json_each(v.content,'$.entries') e, json_each(e.value,'$.description.units') u
      WHERE v.id=NEW.version_id
      EXCEPT SELECT key, ordinal, unit_id, page_id, source_id FROM catalogue_entry_units WHERE version_id=NEW.version_id)
    OR EXISTS(SELECT key, ordinal, unit_id, page_id, source_id FROM catalogue_entry_units WHERE version_id=NEW.version_id
      EXCEPT SELECT e.value->>'$.key', u.key, u.value->>'$.unit_id', u.value->>'$.page_id', u.value->>'$.source_id'
      FROM catalogue_versions v, json_each(v.content,'$.entries') e, json_each(e.value,'$.description.units') u
      WHERE v.id=NEW.version_id)
    OR (SELECT count(*) FROM catalogue_versions v, json_each(v.content,'$.entries') e,
          json_each(e.value,'$.description.units') u WHERE v.id=NEW.version_id)
      IS NOT (SELECT count(*) FROM catalogue_entry_units WHERE version_id=NEW.version_id);
  SELECT RAISE(ABORT,'Catalogue seal: append-only violated') WHERE EXISTS(
      SELECT type_id FROM catalogue_entry_types WHERE version_id=(
        SELECT s.version_id FROM catalogue_seals s JOIN catalogue_versions v ON v.id=s.version_id
        WHERE v.catalogue=(SELECT catalogue FROM catalogue_versions WHERE id=NEW.version_id)
        ORDER BY v.version DESC LIMIT 1)
      EXCEPT SELECT type_id FROM catalogue_entry_types WHERE version_id=NEW.version_id)
    OR EXISTS(SELECT key, type_id FROM catalogue_entries WHERE version_id=(
        SELECT s.version_id FROM catalogue_seals s JOIN catalogue_versions v ON v.id=s.version_id
        WHERE v.catalogue=(SELECT catalogue FROM catalogue_versions WHERE id=NEW.version_id)
        ORDER BY v.version DESC LIMIT 1)
      EXCEPT SELECT key, type_id FROM catalogue_entries WHERE version_id=NEW.version_id);
END;
CREATE TRIGGER catalogue_seals_update BEFORE UPDATE ON catalogue_seals BEGIN SELECT RAISE(ABORT,'Catalogue seals are immutable'); END;
CREATE TRIGGER catalogue_seals_delete BEFORE DELETE ON catalogue_seals BEGIN SELECT RAISE(ABORT,'Catalogue seals are retained'); END;
PRAGMA user_version=19;
