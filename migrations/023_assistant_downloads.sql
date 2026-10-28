-- Model downloads (docs/assisted-sheet-plan.md §1.4b, step 2d-2). One row per download the
-- owner confirmed: the spec (`hf:<org>/<repo>/<path>@<revision>`), the file's name in the
-- models folder, its SHA-256 and size from the review, and the licence snapshot the owner
-- confirmed (identifier, repository path, SHA-256 of the licence file at that revision; NULL
-- for none). `confirmed` is a download that is waiting, running or stopped (the partial file's
-- length is the progress; nothing else is stored); `complete` is a verified file in the models
-- folder. The models of migration 022 are untouched: a downloaded file is tried and recorded
-- there as `local:<file_name>`.
CREATE TABLE assistant_downloads (
 id INTEGER PRIMARY KEY,
 spec TEXT NOT NULL UNIQUE CHECK(length(CAST(spec AS BLOB)) BETWEEN 10 AND 512 AND spec GLOB 'hf:?*/?*/?*@????????????????????????????????????????' AND spec GLOB '*.gguf@*' AND substr(spec,length(spec)-39) NOT GLOB '*[^0-9a-f]*' AND spec NOT GLOB '*[^A-Za-z0-9._@/:-]*'),
 file_name TEXT NOT NULL CHECK(length(file_name) BETWEEN 6 AND 128 AND file_name GLOB '*.gguf' AND file_name NOT GLOB '.*' AND file_name NOT GLOB '*[^A-Za-z0-9._-]*'),
 sha256 TEXT NOT NULL CHECK(length(sha256)=64 AND sha256 NOT GLOB '*[^0-9a-f]*'),
 size INTEGER NOT NULL CHECK(size BETWEEN 1 AND 8589934592),
 licence_id TEXT CHECK(licence_id IS NULL OR (length(licence_id) BETWEEN 1 AND 64 AND licence_id NOT GLOB '*[^A-Za-z0-9._+-]*')),
 licence_path TEXT CHECK(licence_path IS NULL OR (length(licence_path) BETWEEN 1 AND 512 AND licence_path NOT GLOB '*[^A-Za-z0-9._/-]*' AND licence_path NOT GLOB '*..*' AND licence_path NOT GLOB '/*' AND licence_path NOT GLOB '*/' AND licence_path NOT GLOB '*//*' AND licence_path NOT GLOB './*' AND licence_path NOT GLOB '*/./*' AND licence_path NOT GLOB '*/.')),
 licence_sha256 TEXT CHECK(licence_sha256 IS NULL OR (length(licence_sha256)=64 AND licence_sha256 NOT GLOB '*[^0-9a-f]*')),
 confirmed_by INTEGER NOT NULL CHECK(confirmed_by>0),
 confirmed_at INTEGER NOT NULL CHECK(confirmed_at>0),
 state TEXT NOT NULL CHECK(state IN ('confirmed','complete')),
 completed_at INTEGER CHECK((state='complete')=(completed_at IS NOT NULL) AND (completed_at IS NULL OR completed_at>0))
) STRICT;
CREATE INDEX assistant_downloads_sha256 ON assistant_downloads(sha256);
CREATE INDEX assistant_downloads_file_name ON assistant_downloads(file_name);
PRAGMA user_version=23;
