-- Test-only: keep the legacy rows, then the caller recreates migration 024's suggestion
-- table and triggers and copies these rows back. New v25-only data is not rewindable.
CREATE TEMP TABLE legacy_suggestions AS
 SELECT s.id,s.source_id,s.bundle_hash,s.origin,s.kind,s.targets,s.suggested_text,s.rationale,
 d.actor dismissed_by,d.created_at dismissed_at FROM source_suggestions s
 LEFT JOIN source_suggestion_dismissals d ON d.suggestion_id=s.id;
DROP TABLE source_suggestion_web_events;
DROP TABLE source_suggestion_applications;
DROP TABLE source_suggestion_dismissals;
DROP TABLE source_suggestions;
DROP TABLE source_suggestion_reports;
DROP TRIGGER source_suggestion_page_bounds_populate;
DROP TRIGGER source_suggestion_page_bounds_populate_update;
DROP TRIGGER source_suggestion_bounds_source_update;
DROP TABLE source_suggestion_page_bounds;
PRAGMA user_version=24;
