CREATE TABLE characters (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    system_id TEXT NOT NULL,
    edition TEXT NOT NULL,
    notes TEXT NOT NULL DEFAULT '',
    schema_version INTEGER NOT NULL DEFAULT 1,
    revision INTEGER NOT NULL DEFAULT 1 CHECK (revision > 0)
);
PRAGMA user_version = 1;
