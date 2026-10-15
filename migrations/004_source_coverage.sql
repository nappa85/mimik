ALTER TABLE sources ADD COLUMN total_pages INTEGER NOT NULL DEFAULT 0 CHECK (total_pages >= 0);
PRAGMA user_version = 4;
