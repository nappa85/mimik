DROP TABLE source_primary_web_events;
DROP TABLE source_primary_events;
DROP TABLE source_primary_pages;
DROP TABLE source_document_members;
DROP TABLE source_documents;
DROP TABLE source_suggestions;
DROP TRIGGER source_unit_origin_update;
DROP TRIGGER source_unit_origin_insert;
DROP TRIGGER source_owner_geometry;
DROP TRIGGER source_owner_page_reset;
DROP TRIGGER units_finalized_insert;
DROP TRIGGER sources_registered_insert;
DROP TRIGGER sources_registered_update;
DROP TRIGGER sources_registered_delete;
DROP TRIGGER source_review_operation_insert;
DROP INDEX source_review_operation;
ALTER TABLE source_units DROP COLUMN origin;
ALTER TABLE source_review_events DROP COLUMN operation_id;
CREATE TRIGGER units_finalized_insert BEFORE INSERT ON source_units
WHEN EXISTS(SELECT 1 FROM source_pages p JOIN source_finalizations f ON f.source_id=p.source_id WHERE p.id=NEW.page_id)
 OR EXISTS(SELECT 1 FROM source_units u JOIN source_pages p ON p.id=u.page_id JOIN source_finalizations f ON f.source_id=p.source_id WHERE u.id=NEW.id)
BEGIN SELECT RAISE(ABORT,'Finalized unit membership cannot change'); END;
