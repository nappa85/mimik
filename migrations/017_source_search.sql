-- Derived lexical search over reviewed source units. Raw evidence rows are not changed.
-- Every object is named source_search_* so fixtures can find them in sqlite_master.
CREATE TABLE source_search_text (
    unit_id INTEGER PRIMARY KEY REFERENCES source_units(id) ON DELETE CASCADE,
    normaliser INTEGER NOT NULL CHECK (normaliser > 0),
    search_text TEXT NOT NULL CHECK (instr(search_text, char(0)) = 0 AND length(CAST(search_text AS BLOB)) <= 262144)
) STRICT;
-- Units whose raw HTML could not be normalised by the recorded normaliser version. They
-- stay out of the index (and cannot be accepted) until corrected, rejected or re-derived.
CREATE TABLE source_search_failures (
    unit_id INTEGER PRIMARY KEY REFERENCES source_units(id) ON DELETE CASCADE,
    normaliser INTEGER NOT NULL CHECK (normaliser > 0),
    reason TEXT NOT NULL CHECK (length(CAST(reason AS BLOB)) BETWEEN 1 AND 300)
) STRICT;
-- rowid = source_units.id; body = effective reviewed text only (accepted: search text,
-- corrected: correction). Unreviewed/rejected text is never indexed.
CREATE VIRTUAL TABLE source_search_fts USING fts5(
    body, tokenize = 'unicode61 remove_diacritics 2', prefix = '3'
);
CREATE TRIGGER source_search_unit_insert_guard BEFORE INSERT ON source_units
WHEN NEW.state IN ('accepted','corrected')
BEGIN SELECT RAISE(ABORT,'Reviewed units must be inserted unreviewed and reviewed after search text exists'); END;
CREATE TRIGGER source_search_unit_accept_guard BEFORE UPDATE ON source_units
WHEN NEW.state='accepted' AND NOT EXISTS(SELECT 1 FROM source_search_text WHERE unit_id=NEW.id)
BEGIN SELECT RAISE(ABORT,'Unit text has no search projection (normalisation failed, see source_search_failures); record a correction or rejection instead'); END;
CREATE TRIGGER source_search_unit_raw_immutable BEFORE UPDATE ON source_units
WHEN NEW.raw_html IS NOT OLD.raw_html
BEGIN SELECT RAISE(ABORT,'Raw unit HTML is immutable; import a new source bundle instead'); END;
CREATE TRIGGER source_search_unit_review_fts AFTER UPDATE ON source_units
WHEN NEW.state IS NOT OLD.state OR NEW.correction IS NOT OLD.correction
BEGIN
    DELETE FROM source_search_fts WHERE rowid=OLD.id;
    INSERT INTO source_search_fts(rowid,body) SELECT NEW.id, NEW.correction WHERE NEW.state='corrected';
    INSERT INTO source_search_fts(rowid,body)
      SELECT NEW.id, t.search_text FROM source_search_text t WHERE t.unit_id=NEW.id AND NEW.state='accepted';
END;
CREATE TRIGGER source_search_unit_delete_fts AFTER DELETE ON source_units
BEGIN DELETE FROM source_search_fts WHERE rowid=OLD.id; END;
CREATE TRIGGER source_search_text_insert_fts AFTER INSERT ON source_search_text
WHEN EXISTS(SELECT 1 FROM source_units WHERE id=NEW.unit_id AND state='accepted')
BEGIN
    DELETE FROM source_search_fts WHERE rowid=NEW.unit_id;
    INSERT INTO source_search_fts(rowid,body) VALUES (NEW.unit_id, NEW.search_text);
END;
CREATE TRIGGER source_search_text_update_fts AFTER UPDATE ON source_search_text
WHEN EXISTS(SELECT 1 FROM source_units WHERE id=NEW.unit_id AND state='accepted')
BEGIN
    DELETE FROM source_search_fts WHERE rowid=OLD.unit_id;
    INSERT INTO source_search_fts(rowid,body) VALUES (NEW.unit_id, NEW.search_text);
END;
-- Retained for accepted units unless a newer normaliser version recorded a failure for
-- the unit (its stale projection must then leave the index).
CREATE TRIGGER source_search_text_delete_guard BEFORE DELETE ON source_search_text
WHEN EXISTS(SELECT 1 FROM source_units WHERE id=OLD.unit_id AND state='accepted')
 AND NOT EXISTS(SELECT 1 FROM source_search_failures f WHERE f.unit_id=OLD.unit_id AND f.normaliser<>OLD.normaliser)
BEGIN SELECT RAISE(ABORT,'Search text of an accepted unit is retained'); END;
CREATE TRIGGER source_search_text_delete_fts AFTER DELETE ON source_search_text
WHEN EXISTS(SELECT 1 FROM source_units WHERE id=OLD.unit_id AND state='accepted')
BEGIN DELETE FROM source_search_fts WHERE rowid=OLD.unit_id; END;
PRAGMA user_version=17;
