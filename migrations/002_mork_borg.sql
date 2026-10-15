CREATE TABLE mork_borg_sheets (
    character_id INTEGER PRIMARY KEY REFERENCES characters(id),
    schema_version INTEGER NOT NULL DEFAULT 1 CHECK (schema_version = 1),
    data TEXT NOT NULL
);
PRAGMA user_version = 2;
