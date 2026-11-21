-- SRC-15 slice 7: source PDFs live in the content-addressed store (`sources/<pdf_hash>.pdf` beside
-- the database), not in the row. Owner decision 2026-11-11: no release has shipped and local data
-- is disposable, so the legacy blobs are NOT copied out; a source imported before this migration
-- has no file in the store and must be imported again to have its PDF (using it gives
-- `pdf_corrupt`; startup reports how many sources lack a file). Nothing else references the
-- column. Run VACUUM afterwards to reclaim the space.
-- The store id ties this database to its `sources/.store-id` marker, so two databases sharing one
-- folder are noticed and neither sweeps the other's files.
ALTER TABLE sources DROP COLUMN pdf;
CREATE TABLE pdf_store_identity (
 singleton INTEGER PRIMARY KEY CHECK(singleton=1),
 store_id TEXT NOT NULL CHECK(length(store_id)=32 AND store_id NOT GLOB '*[^0-9a-f]*')
);
INSERT INTO pdf_store_identity VALUES(1,lower(hex(randomblob(16))));
PRAGMA user_version=37;
