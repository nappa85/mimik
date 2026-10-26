-- The owner's model setting for the interpretation assistant (docs/assisted-sheet-plan.md
-- §1.4b, step 2d-1; ASSIST-28, ASSIST-36). `assistant_models` keeps the models the owner
-- switched to (and the one they switched away from): the spec, the file's SHA-256 recorded
-- when it was tried, its size, the server alias, the `max_tokens` the trial derived and the
-- trial's measurements. `assistant_model_choice` is the single row naming the active and the
-- previous model ("Switch back" swaps them). Without a row the start flags decide.
CREATE TABLE assistant_models (
 id INTEGER PRIMARY KEY,
 spec TEXT NOT NULL CHECK(length(CAST(spec AS BLOB)) BETWEEN 7 AND 512 AND spec GLOB 'local:?*' AND instr(spec,char(0))=0),
 file_name TEXT NOT NULL CHECK(length(file_name) BETWEEN 6 AND 128 AND file_name GLOB '*.gguf' AND file_name NOT GLOB '.*' AND file_name NOT GLOB '*[^A-Za-z0-9._-]*'),
 sha256 TEXT NOT NULL CHECK(length(sha256)=64 AND sha256 NOT GLOB '*[^0-9a-f]*'),
 size INTEGER CHECK(size IS NULL OR size BETWEEN 1 AND 8589934592),
 alias TEXT NOT NULL CHECK(length(CAST(alias AS BLOB)) BETWEEN 1 AND 128 AND alias NOT GLOB '*[^A-Za-z0-9._-]*'),
 max_tokens INTEGER NOT NULL CHECK(max_tokens BETWEEN 256 AND 384),
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
PRAGMA user_version=22;
