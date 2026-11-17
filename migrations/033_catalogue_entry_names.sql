-- Assisted sheet step 3a (docs/catalogues.md, "Name index"): the normalised entry names the
-- sheet's linking on view compares, for the latest sealed version of each catalogue only. A
-- derived table like catalogue_names_fts, so it has no immutability triggers: the seal
-- transaction replaces a catalogue's rows, `rebuild-catalogue-index` and `verify-catalogues`
-- recompute them, and store open rebuilds them when the marker below differs from the build's
-- text_fold::name_fold_version() (src/text_fold.rs). Type and retirement are joined from catalogue_entries.
-- Like 032 it can run again over a current schema (tests that rewind the version do).
CREATE TABLE IF NOT EXISTS catalogue_entry_names (
    version_id INTEGER NOT NULL REFERENCES catalogue_versions(id) ON DELETE RESTRICT,
    entry_key TEXT NOT NULL,
    norm TEXT NOT NULL,
    PRIMARY KEY (version_id,entry_key)
) STRICT, WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS catalogue_entry_names_norm ON catalogue_entry_names(version_id,norm);
-- One row, written after the last catalogue is rebuilt.
CREATE TABLE IF NOT EXISTS catalogue_name_fold (
    id INTEGER PRIMARY KEY CHECK (id=1),
    version TEXT NOT NULL CHECK (length(version) BETWEEN 1 AND 100)
) STRICT;
PRAGMA user_version=33;
