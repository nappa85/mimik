-- The reply length the owner sets per model file (docs/model-runtime.md, "Reply length";
-- ASSIST-59): `assistant_reply_lengths`, one row per file hash, independent of the model rows
-- and of which model is active. A file without a row uses the default. The bounds below are
-- `model::reply_length` (a test keeps them equal).
-- A model row no longer carries `max_tokens` (it was what a trial derived; the length is the
-- file's now). SQLite cannot drop a column that a CHECK uses in place, and the choice row
-- references the models, so both tables are rebuilt in this one transaction with every row
-- kept under its id (pre-release data, no backfill rule). The migration can run again over
-- a current schema (tests that rewind the version do).
CREATE TABLE IF NOT EXISTS assistant_reply_lengths (
 sha256 TEXT PRIMARY KEY CHECK(length(sha256)=64 AND sha256 NOT GLOB '*[^0-9a-f]*'),
 max_tokens INTEGER NOT NULL CHECK(max_tokens BETWEEN 256 AND 2560 AND max_tokens % 128 = 0),
 changed_by INTEGER NOT NULL CHECK(changed_by>0),
 changed_at INTEGER NOT NULL CHECK(changed_at>0)
) STRICT;
CREATE TEMP TABLE assistant_models_old AS SELECT * FROM assistant_models;
CREATE TEMP TABLE assistant_model_choice_old AS SELECT * FROM assistant_model_choice;
DROP TABLE assistant_model_choice;
DROP TABLE assistant_models;
CREATE TABLE assistant_models (
 id INTEGER PRIMARY KEY,
 spec TEXT NOT NULL CHECK(length(CAST(spec AS BLOB)) BETWEEN 7 AND 512 AND spec GLOB 'local:?*' AND instr(spec,char(0))=0),
 file_name TEXT NOT NULL CHECK(length(file_name) BETWEEN 6 AND 128 AND file_name GLOB '*.gguf' AND file_name NOT GLOB '.*' AND file_name NOT GLOB '*[^A-Za-z0-9._-]*'),
 sha256 TEXT NOT NULL CHECK(length(sha256)=64 AND sha256 NOT GLOB '*[^0-9a-f]*'),
 size INTEGER CHECK(size IS NULL OR size BETWEEN 1 AND 8589934592),
 alias TEXT NOT NULL CHECK(length(CAST(alias AS BLOB)) BETWEEN 1 AND 128 AND alias NOT GLOB '*[^A-Za-z0-9._-]*'),
 trial TEXT CHECK(trial IS NULL OR (json_valid(trial) AND json_type(trial)='object' AND length(CAST(trial AS BLOB))<=8192)),
 recorded_at INTEGER NOT NULL CHECK(recorded_at>0)
) STRICT;
CREATE TABLE assistant_model_choice (
 id INTEGER PRIMARY KEY CHECK(id=1),
 active_model INTEGER NOT NULL REFERENCES assistant_models(id),
 previous_model INTEGER REFERENCES assistant_models(id) CHECK(previous_model IS NULL OR previous_model<>active_model),
 changed_by INTEGER NOT NULL CHECK(changed_by>0),
 changed_at INTEGER NOT NULL CHECK(changed_at>0)
) STRICT;
INSERT INTO assistant_models(id,spec,file_name,sha256,size,alias,trial,recorded_at)
 SELECT id,spec,file_name,sha256,size,alias,trial,recorded_at FROM assistant_models_old;
INSERT INTO assistant_model_choice(id,active_model,previous_model,changed_by,changed_at)
 SELECT id,active_model,previous_model,changed_by,changed_at FROM assistant_model_choice_old;
DROP TABLE assistant_model_choice_old;
DROP TABLE assistant_models_old;
PRAGMA user_version=32;
