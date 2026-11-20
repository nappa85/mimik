-- SRC-15 slice 2: upload staging bounds follow the inspection frame (20,000 pages), the PDF has no
-- size bound of its own, `document_changed` joins the job reasons, and an exclusion holds up to
-- eight boxes per page and a list of proposal fingerprints.
-- Owner decision 2026-11-11: no release has shipped and staged uploads are disposable, so the four
-- upload tables are dropped and recreated empty, not copied. Staged files of the dropped rows are
-- orphan directories that the startup reconciliation removes (descriptor-relative, no-follow).
-- Children first; the views go before the tables they read. Triggers and the index go with them.
DROP VIEW IF EXISTS source_upload_invalid_exclusion;
DROP VIEW IF EXISTS source_upload_invalid_inspection;
DROP TABLE IF EXISTS source_upload_exclusions;
DROP TABLE IF EXISTS source_upload_pages;
DROP TABLE IF EXISTS source_jobs;
DROP TABLE IF EXISTS source_uploads;
-- Mutable staging, deliberately outside the permanent-source immutability triggers.
CREATE TABLE source_uploads (
 id TEXT PRIMARY KEY CHECK(length(id)=32 AND id NOT GLOB '*[^0-9a-f]*'),
 pdf_sha256 TEXT NOT NULL CHECK(length(pdf_sha256)=64 AND pdf_sha256 NOT GLOB '*[^0-9a-f]*'),
 pdf_bytes INTEGER NOT NULL CHECK(pdf_bytes>=1),
 actor TEXT NOT NULL CHECK(length(actor) BETWEEN 1 AND 100 AND instr(actor,char(0))=0),
 actor_id INTEGER NOT NULL REFERENCES accounts(id) CHECK(actor_id>0),
 actor_epoch INTEGER NOT NULL CHECK(actor_epoch>0),
 session_digest TEXT NOT NULL CHECK(length(session_digest)=64 AND session_digest NOT GLOB '*[^0-9a-f]*'),
 created_at INTEGER NOT NULL CHECK(created_at>0),
 updated_at INTEGER NOT NULL CHECK(updated_at>=created_at),
 revision INTEGER NOT NULL DEFAULT 1 CHECK(revision>0),
 state TEXT NOT NULL CHECK(state IN ('uploaded','inspected','rendering','rendered','failed')),
 inspection TEXT CHECK(inspection IS NULL OR (length(CAST(inspection AS BLOB))<=9437184 AND json_valid(inspection) AND json_type(inspection)='object')),
 selected_pages TEXT NOT NULL DEFAULT '[]' CHECK(length(CAST(selected_pages AS BLOB))<=140000 AND json_valid(selected_pages) AND json_type(selected_pages)='array' AND json_array_length(selected_pages)<=20000),
 system_id TEXT NOT NULL CHECK(length(system_id) BETWEEN 1 AND 64 AND substr(system_id,1,1) GLOB '[a-z]' AND system_id NOT GLOB '*[^a-z0-9-]*'),
 edition TEXT NOT NULL CHECK(length(edition) BETWEEN 1 AND 100 AND instr(edition,char(0))=0),
 language TEXT NOT NULL CHECK(length(CAST(language AS BLOB)) BETWEEN 1 AND 30 AND instr(language,char(0))=0),
 -- Upload-local secret: never copied to the exclusion history or staged contract.
 fingerprint_key BLOB CHECK(fingerprint_key IS NULL OR length(fingerprint_key)=32),
 -- Set when the staged file stopped matching the hash it was admitted with (slice 3).
 document_changed INTEGER NOT NULL DEFAULT 0 CHECK(document_changed IN (0,1)),
 CHECK(actor='account:'||actor_id),
 CHECK(inspection IS NULL OR (
  json_type(inspection,'$.document') IS 'object' AND
  json_type(inspection,'$.document.pages') IS 'integer' AND json_extract(inspection,'$.document.pages') BETWEEN 1 AND 20000 AND
  (json_type(inspection,'$.document.encrypted') IS 'true' OR json_type(inspection,'$.document.encrypted') IS 'false') AND
  json_type(inspection,'$.document.mupdf') IS 'text' AND length(CAST(json_extract(inspection,'$.document.mupdf') AS BLOB)) BETWEEN 1 AND 32 AND
  json_type(inspection,'$.pages') IS 'array' AND json_array_length(inspection,'$.pages')=json_extract(inspection,'$.document.pages')
 ))
) STRICT;
CREATE UNIQUE INDEX source_upload_single_slot ON source_uploads((1));
CREATE TRIGGER source_upload_selection_insert BEFORE INSERT ON source_uploads WHEN EXISTS(
 SELECT 1 FROM json_each(NEW.selected_pages) WHERE type!='integer' OR value NOT BETWEEN 1 AND 20000
) OR (SELECT count(DISTINCT value) FROM json_each(NEW.selected_pages))!=json_array_length(NEW.selected_pages)
BEGIN SELECT RAISE(ABORT,'invalid upload selection'); END;
-- One shared predicate validates bounded inspection facts for both insert and update.
CREATE VIEW source_upload_invalid_inspection AS
 SELECT u.id FROM source_uploads u,json_each(u.inspection,'$.pages') p WHERE
 p.type IS NOT 'object' OR
 json_type(p.value,'$.n') IS NOT 'integer' OR json_extract(p.value,'$.n')!=CAST(p.key AS INTEGER)+1 OR
 NOT coalesce(json_type(p.value,'$.width') IN ('integer','real') AND json_extract(p.value,'$.width') BETWEEN 1 AND 4000,0) OR
 NOT coalesce(json_type(p.value,'$.height') IN ('integer','real') AND json_extract(p.value,'$.height') BETWEEN 1 AND 4000,0) OR
 NOT coalesce(json_type(p.value,'$.rotate')='integer' AND json_extract(p.value,'$.rotate') IN (0,90,180,270),0) OR
 NOT coalesce(json_type(p.value,'$.user_unit') IN ('integer','real') AND json_extract(p.value,'$.user_unit') BETWEEN 0.1 AND 10,0) OR
 NOT coalesce(json_type(p.value,'$.tagged') IN ('true','false'),0) OR
 json_type(p.value,'$.transform') IS NOT 'array' OR json_array_length(p.value,'$.transform')!=6 OR
 EXISTS(SELECT 1 FROM json_each(p.value,'$.transform') t WHERE t.type NOT IN ('integer','real') OR t.value NOT BETWEEN -100000 AND 100000) OR
 NOT coalesce(json_extract(p.value,'$.transform[0]')*json_extract(p.value,'$.transform[3]')-json_extract(p.value,'$.transform[1]')*json_extract(p.value,'$.transform[2]')!=0,0) OR
 json_type(p.value,'$.visible') IS NOT 'array' OR json_array_length(p.value,'$.visible')!=4 OR
 EXISTS(SELECT 1 FROM json_each(p.value,'$.visible') v WHERE v.type NOT IN ('integer','real') OR v.value NOT BETWEEN -1e307 AND 1e307);
CREATE TRIGGER source_upload_inspection_insert AFTER INSERT ON source_uploads WHEN
 EXISTS(SELECT 1 FROM source_upload_invalid_inspection WHERE id=NEW.id)
 BEGIN SELECT RAISE(ABORT,'invalid upload inspection'); END;
CREATE TRIGGER source_upload_inspection_update AFTER UPDATE OF inspection ON source_uploads WHEN
 EXISTS(SELECT 1 FROM source_upload_invalid_inspection WHERE id=NEW.id)
 BEGIN SELECT RAISE(ABORT,'invalid upload inspection'); END;
CREATE TRIGGER source_upload_selection_update BEFORE UPDATE OF selected_pages ON source_uploads WHEN EXISTS(
 SELECT 1 FROM json_each(NEW.selected_pages) WHERE type!='integer' OR value NOT BETWEEN 1 AND 20000
) OR (SELECT count(DISTINCT value) FROM json_each(NEW.selected_pages))!=json_array_length(NEW.selected_pages)
BEGIN SELECT RAISE(ABORT,'invalid upload selection'); END;
CREATE TABLE source_upload_pages (
 upload_id TEXT NOT NULL REFERENCES source_uploads(id) ON DELETE CASCADE,
 pdf_page INTEGER NOT NULL CHECK(pdf_page BETWEEN 1 AND 20000),
 rendered_revision INTEGER NOT NULL CHECK(rendered_revision>0),
 png_sha256 TEXT NOT NULL CHECK(length(png_sha256)=64 AND png_sha256 NOT GLOB '*[^0-9a-f]*'),
 png_bytes INTEGER NOT NULL CHECK(png_bytes BETWEEN 1 AND 12582912),
 width_px INTEGER NOT NULL CHECK(width_px BETWEEN 1 AND 4096),
 height_px INTEGER NOT NULL CHECK(height_px BETWEEN 1 AND 8192),
 dpi INTEGER NOT NULL CHECK(dpi BETWEEN 1 AND 150),
 width_points REAL NOT NULL CHECK(width_points BETWEEN 1 AND 4000),
 height_points REAL NOT NULL CHECK(height_points BETWEEN 1 AND 4000),
 transform TEXT NOT NULL CHECK(length(CAST(transform AS BLOB))<=256 AND json_valid(transform) AND json_type(transform)='array' AND json_array_length(transform)=6),
 stats TEXT NOT NULL CHECK(length(CAST(stats AS BLOB))<=4096 AND json_valid(stats) AND json_type(stats)='object'),
 settings TEXT NOT NULL CHECK(length(CAST(settings AS BLOB))<=256 AND json_valid(settings) AND json_type(settings)='object'),
 text_sha256 TEXT NOT NULL CHECK(length(text_sha256)=64 AND text_sha256 NOT GLOB '*[^0-9a-f]*'),
 text_bytes INTEGER NOT NULL CHECK(text_bytes BETWEEN 0 AND 8388608),
 renderer_crate_version TEXT NOT NULL CHECK(length(CAST(renderer_crate_version AS BLOB)) BETWEEN 1 AND 32),
 mupdf_version TEXT NOT NULL CHECK(length(CAST(mupdf_version AS BLOB)) BETWEEN 1 AND 32),
 PRIMARY KEY(upload_id,pdf_page),
 CHECK(width_px*height_px<=12000000),
 CHECK(json_type(stats,'$.n') IS 'integer' AND json_extract(stats,'$.n')=pdf_page),
 CHECK(json_type(stats,'$.chars') IS 'integer' AND json_extract(stats,'$.chars') BETWEEN 0 AND 1000000),
 CHECK(json_type(stats,'$.unmapped') IS 'integer' AND json_extract(stats,'$.unmapped') BETWEEN 0 AND json_extract(stats,'$.chars')),
 CHECK((json_type(stats,'$.image_area') IS 'integer' OR json_type(stats,'$.image_area') IS 'real') AND json_extract(stats,'$.image_area') BETWEEN 0 AND 1),
 CHECK(json_type(stats,'$.fonts_substituted') IS 'array' AND json_array_length(stats,'$.fonts_substituted')<=8),
 CHECK(json_type(stats,'$.fonts_skipped') IS 'integer' AND json_extract(stats,'$.fonts_skipped') BETWEEN 0 AND 10000),
 CHECK(json_type(stats,'$.incomplete') IS 'true' OR json_type(stats,'$.incomplete') IS 'false'),
 CHECK(json_type(stats,'$.dropped_lines') IS 'integer' AND json_extract(stats,'$.dropped_lines') BETWEEN 0 AND 1000000),
 CHECK(json_type(settings,'$.dpi') IS 'integer' AND json_extract(settings,'$.dpi') BETWEEN 1 AND 150),
 CHECK(json_type(settings,'$.colourspace') IS 'text' AND json_extract(settings,'$.colourspace')='rgb'),
 CHECK(json_type(settings,'$.anti_aliasing') IS 'integer' AND json_extract(settings,'$.anti_aliasing') BETWEEN 0 AND 8),
 CHECK(json_extract(transform,'$[0]')*json_extract(transform,'$[3]')-json_extract(transform,'$[1]')*json_extract(transform,'$[2]') != 0)
) STRICT;
CREATE TRIGGER source_upload_page_facts_insert BEFORE INSERT ON source_upload_pages WHEN
 EXISTS(SELECT 1 FROM json_each(NEW.transform) WHERE type NOT IN ('integer','real') OR value NOT BETWEEN -100000 AND 100000) OR
 EXISTS(SELECT 1 FROM json_each(NEW.stats,'$.fonts_substituted') WHERE type!='text' OR length(CAST(value AS BLOB)) NOT BETWEEN 1 AND 64)
 BEGIN SELECT RAISE(ABORT,'invalid staged page facts'); END;
CREATE TRIGGER source_upload_page_facts_update BEFORE UPDATE ON source_upload_pages WHEN
 EXISTS(SELECT 1 FROM json_each(NEW.transform) WHERE type NOT IN ('integer','real') OR value NOT BETWEEN -100000 AND 100000) OR
 EXISTS(SELECT 1 FROM json_each(NEW.stats,'$.fonts_substituted') WHERE type!='text' OR length(CAST(value AS BLOB)) NOT BETWEEN 1 AND 64)
 BEGIN SELECT RAISE(ABORT,'invalid staged page facts'); END;
-- `boxes` maps each excluded page to 1-8 boxes. The view bounds one page at 384 bytes (eight boxes
-- of four numbers rounded outward to 0.01 pt need at most 320) and a revision at 20,000 pages of
-- it: 6.4 MB by that worst case, bounded at 6,500,000 bytes. The box keys, sorted numerically, must equal the
-- `pages` list in document order (one ordered group_concat each, SQLite >= 3.44; never a join, so the view
-- stays linear at 20,000 pages); that also forces `pages` strictly ascending and distinct.
CREATE TABLE source_upload_exclusions (
 upload_id TEXT NOT NULL REFERENCES source_uploads(id) ON DELETE CASCADE,
 revision INTEGER NOT NULL CHECK(revision>0),
 actor TEXT NOT NULL CHECK(length(actor) BETWEEN 1 AND 100 AND instr(actor,char(0))=0),
 actor_id INTEGER NOT NULL REFERENCES accounts(id) CHECK(actor_id>0),
 actor_epoch INTEGER NOT NULL CHECK(actor_epoch>0),
 session_digest TEXT NOT NULL CHECK(length(session_digest)=64 AND session_digest NOT GLOB '*[^0-9a-f]*'),
 rationale TEXT NOT NULL CHECK(length(rationale) BETWEEN 1 AND 5000 AND instr(rationale,char(0))=0),
 pages TEXT NOT NULL CHECK(length(CAST(pages AS BLOB))<=140000 AND json_valid(pages) AND json_type(pages)='array' AND json_array_length(pages) BETWEEN 1 AND 20000),
 boxes TEXT NOT NULL CHECK(length(CAST(boxes AS BLOB))<=6500000 AND json_valid(boxes) AND json_type(boxes)='object'),
 -- Fingerprints of the confirmed proposals (none for manual boxes); entries checked by the view.
 proposal_fingerprints TEXT NOT NULL DEFAULT '[]' CHECK(length(CAST(proposal_fingerprints AS BLOB))<=600 AND json_valid(proposal_fingerprints) AND json_type(proposal_fingerprints)='array' AND json_array_length(proposal_fingerprints)<=8),
 removed INTEGER NOT NULL DEFAULT 0 CHECK(removed IN (0,1)),
 created_at INTEGER NOT NULL CHECK(created_at>0),
 PRIMARY KEY(upload_id,revision),
 CHECK(actor='account:'||actor_id)
) STRICT;
CREATE TABLE source_jobs (
 upload_id TEXT PRIMARY KEY REFERENCES source_uploads(id) ON DELETE CASCADE,
 upload_revision INTEGER NOT NULL CHECK(upload_revision>0),
 kind TEXT NOT NULL CHECK(kind IN ('inspect','render')),
 state TEXT NOT NULL CHECK(state IN ('queued','running','succeeded','failed','cancelled')),
 pages_done INTEGER NOT NULL DEFAULT 0 CHECK(pages_done BETWEEN 0 AND 20000),
 pages_total INTEGER NOT NULL DEFAULT 0 CHECK(pages_total BETWEEN 0 AND 20000 AND pages_done<=pages_total),
 reason TEXT CHECK(reason IN ('unreadable','timeout','too_large','too_many_pages','password_protected','unsupported','unavailable','space','source_png_limit','interrupted','cancelled','document_changed')),
 created_at INTEGER NOT NULL CHECK(created_at>0),
 started_at INTEGER CHECK(started_at>=created_at),
 finished_at INTEGER CHECK(finished_at>=created_at)
) STRICT;
CREATE VIEW source_upload_invalid_exclusion AS
 SELECT e.upload_id,e.revision FROM source_upload_exclusions e WHERE
 EXISTS(SELECT 1 FROM json_each(e.pages) WHERE type!='integer' OR value NOT BETWEEN 1 AND 20000) OR
 (SELECT count(DISTINCT value) FROM json_each(e.pages))!=json_array_length(e.pages) OR
 (SELECT count(*) FROM json_each(e.boxes))!=json_array_length(e.pages) OR
 (SELECT count(DISTINCT key) FROM json_each(e.boxes))!=json_array_length(e.pages) OR
 (SELECT group_concat(key,',' ORDER BY CAST(key AS INTEGER)) FROM json_each(e.boxes)) IS NOT (SELECT group_concat(CAST(value AS TEXT),',' ORDER BY key) FROM json_each(e.pages)) OR
 EXISTS(SELECT 1 FROM json_each(e.boxes) b WHERE b.type IS NOT 'array' OR json_array_length(b.value) NOT BETWEEN 1 AND 8 OR length(CAST(b.value AS BLOB))>384 OR (SELECT count(DISTINCT value) FROM json_each(b.value))!=json_array_length(b.value)) OR
 EXISTS(SELECT 1 FROM json_each(e.boxes) b,json_each(b.value) x WHERE x.type IS NOT 'array' OR json_array_length(x.value)!=4) OR
 EXISTS(SELECT 1 FROM json_each(e.boxes) b,json_each(b.value) x,json_each(x.value) c WHERE c.type NOT IN ('integer','real') OR c.value NOT BETWEEN 0 AND 4000) OR
 EXISTS(SELECT 1 FROM json_each(e.boxes) b,json_each(b.value) x WHERE NOT coalesce(json_extract(x.value,'$[0]')<json_extract(x.value,'$[2]') AND json_extract(x.value,'$[1]')<json_extract(x.value,'$[3]'),0)) OR
 EXISTS(SELECT 1 FROM json_each(e.proposal_fingerprints) f WHERE f.type IS NOT 'text' OR length(f.value)!=64 OR f.value GLOB '*[^0-9a-f]*') OR
 (SELECT count(DISTINCT value) FROM json_each(e.proposal_fingerprints))!=json_array_length(e.proposal_fingerprints);
CREATE TRIGGER source_upload_exclusion_insert AFTER INSERT ON source_upload_exclusions WHEN
 EXISTS(SELECT 1 FROM source_upload_invalid_exclusion WHERE upload_id=NEW.upload_id AND revision=NEW.revision)
 BEGIN SELECT RAISE(ABORT,'invalid staged exclusion'); END;
CREATE TRIGGER source_upload_exclusion_update AFTER UPDATE ON source_upload_exclusions WHEN
 EXISTS(SELECT 1 FROM source_upload_invalid_exclusion WHERE upload_id=NEW.upload_id AND revision=NEW.revision)
 BEGIN SELECT RAISE(ABORT,'invalid staged exclusion'); END;
PRAGMA user_version=36;
