-- Owner-unit replacements: which unit an owner unit takes the place of in reading order. A
-- `table`, `merge` or `split` unit sorts at its anchor (the first replaced unit in reading order,
-- or the split unit); `order_key` is the anchor's sort key plus '.' and the zero-padded part, so
-- it is never NULL (an anchor that is an owner unit with no other position sorts at
-- '~'||id). The sort key expression is the one in `unit_sort_key!` (src/sources/mod.rs); a test
-- compares the two texts.
-- The trigger does not check that the anchor is the first rejected unit in reading order, nor
-- stop a merge-suggestion unit from also getting a replacement row; the writers guarantee both.
CREATE TABLE source_owner_replacements (
 unit_id INTEGER PRIMARY KEY REFERENCES source_units(id),
 operation_id TEXT NOT NULL CHECK(length(operation_id) BETWEEN 1 AND 100 AND instr(operation_id,char(0))=0),
 kind TEXT NOT NULL CHECK(kind IN ('table','merge','split')),
 anchor_unit_id INTEGER NOT NULL REFERENCES source_units(id),
 part INTEGER NOT NULL CHECK(part BETWEEN 1 AND 9999),
 order_key TEXT NOT NULL,
 UNIQUE(operation_id,part)
) STRICT;
CREATE TRIGGER source_owner_replacements_insert BEFORE INSERT ON source_owner_replacements
 WHEN NOT EXISTS(SELECT 1 FROM source_units u JOIN source_units anc ON anc.id=NEW.anchor_unit_id
  WHERE u.id=NEW.unit_id AND u.origin='owner' AND u.page_id=anc.page_id AND u.id<>anc.id
  AND ((NEW.kind='table' AND u.kind='Table' AND NEW.part=1 AND EXISTS(SELECT 1 FROM source_owner_tables t WHERE t.unit_id=u.id AND t.operation_id=NEW.operation_id)) OR (NEW.kind='merge' AND u.kind='Text' AND NEW.part=1) OR (NEW.kind='split' AND u.kind='Text'))
  AND EXISTS(SELECT 1 FROM source_review_events e WHERE e.unit_id=anc.id AND e.operation_id=NEW.operation_id AND e.state='rejected')
  AND (NEW.kind='table' OR EXISTS(SELECT 1 FROM source_review_events c WHERE c.unit_id=u.id AND c.operation_id=NEW.operation_id AND c.state='corrected' AND c.previous_revision=1))
  AND NEW.order_key IS (SELECT CASE WHEN anc.origin='extracted' THEN printf('%020d',anc.id) ELSE coalesce((SELECT ok_rr.order_key FROM source_owner_replacements ok_rr WHERE ok_rr.unit_id=anc.id),(SELECT printf('%020d',ok_m) FROM (SELECT min(json_extract(ok_jt.value,'$.unit')) ok_m FROM source_suggestion_applications ok_sa JOIN source_suggestions ok_ss ON ok_ss.id=ok_sa.suggestion_id,json_each(ok_ss.targets,'$.targets') ok_jt WHERE ok_sa.owner_unit_id=anc.id AND ok_ss.kind='merge') WHERE ok_m IS NOT NULL),'~'||printf('%020d',anc.id)) END||'.'||printf('%04d',NEW.part)))
BEGIN SELECT RAISE(ABORT,'Owner replacement must name an owner unit, its anchor and its operation'); END;
CREATE TRIGGER source_owner_replacements_update BEFORE UPDATE ON source_owner_replacements BEGIN SELECT RAISE(ABORT,'Owner replacements are append only'); END;
CREATE TRIGGER source_owner_replacements_delete BEFORE DELETE ON source_owner_replacements BEGIN SELECT RAISE(ABORT,'Owner replacements are retained'); END;
-- Backfill: existing owner tables (each has its source_owner_tables link, which the trigger requires), in unit_id order, anchored at the batch's first rejection (a
-- table built from an earlier table has that table's row). The recursive walk follows anchors
-- that are themselves tables, adding one '.0001' per level, so the keys do not depend on rows
-- this statement is inserting; the insert trigger re-checks every key.
WITH RECURSIVE walk(unit_id,cur,suffix) AS (
 SELECT t.unit_id,e.unit_id,'.0001' FROM source_owner_tables t JOIN source_review_events e ON e.id=t.first_event_id
 UNION ALL
 SELECT w.unit_id,e.unit_id,w.suffix||'.0001' FROM walk w JOIN source_owner_tables t ON t.unit_id=w.cur JOIN source_review_events e ON e.id=t.first_event_id
)
INSERT INTO source_owner_replacements(unit_id,operation_id,kind,anchor_unit_id,part,order_key)
SELECT t.unit_id,t.operation_id,'table',e.unit_id,1,
 (SELECT CASE WHEN b.origin='extracted' THEN printf('%020d',b.id) ELSE coalesce((SELECT ok_rr.order_key FROM source_owner_replacements ok_rr WHERE ok_rr.unit_id=b.id),(SELECT printf('%020d',ok_m) FROM (SELECT min(json_extract(ok_jt.value,'$.unit')) ok_m FROM source_suggestion_applications ok_sa JOIN source_suggestions ok_ss ON ok_ss.id=ok_sa.suggestion_id,json_each(ok_ss.targets,'$.targets') ok_jt WHERE ok_sa.owner_unit_id=b.id AND ok_ss.kind='merge') WHERE ok_m IS NOT NULL),'~'||printf('%020d',b.id)) END||w.suffix FROM source_units b WHERE b.id=w.cur)
FROM source_owner_tables t JOIN source_review_events e ON e.id=t.first_event_id
JOIN walk w ON w.unit_id=t.unit_id AND NOT EXISTS(SELECT 1 FROM source_owner_tables x WHERE x.unit_id=w.cur)
ORDER BY t.unit_id;
PRAGMA user_version=30;
