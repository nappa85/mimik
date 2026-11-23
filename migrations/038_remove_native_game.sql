-- Mimik reproduces the sheet, not the table: the hardcoded MÖRK BORG sheet is gone, and sheets
-- come only from runtime definitions the Owner registers (PROD-25). `migrate()` runs a Rust
-- guard before this file and refuses a database that still holds native sheet data, so the
-- tables and rows dropped here are empty.
DROP TABLE mork_borg_sheets;

-- A private sheet is created only on a runtime pin. The column CHECK still lists 'native':
-- narrowing it would need a table rebuild, which the foreign keys, triggers and generated
-- columns of the private-sheet tables rule out.
DROP TRIGGER private_sheets_insert;
CREATE TRIGGER private_sheets_insert BEFORE INSERT ON private_sheets BEGIN
 SELECT CASE WHEN NOT EXISTS(SELECT 1 FROM campaign_request_auth a JOIN campaign_members m ON m.account_id=a.account_id AND m.campaign_id=NEW.campaign_id AND m.state='active' WHERE a.account_id=NEW.custodian_id) OR NOT EXISTS(SELECT 1 FROM campaign_members m JOIN accounts a ON a.id=m.account_id WHERE m.campaign_id=NEW.campaign_id AND m.role='master' AND m.state='active' AND a.state='enabled') THEN RAISE(ABORT,'private auth required') END;
 SELECT CASE WHEN NEW.kind IS NOT 'runtime' OR json_extract(NEW.pin,'$.type') IS NOT 'runtime' OR json_extract(NEW.pin,'$.schema') IS NOT 1 OR NOT EXISTS(SELECT 1 FROM definition_versions v WHERE v.id=json_extract(NEW.pin,'$.version_id') AND v.version=json_extract(NEW.pin,'$.version') AND v.content_hash=json_extract(NEW.pin,'$.content_hash') AND v.game_id=NEW.game_id AND v.edition_id=NEW.edition_id) THEN RAISE(ABORT,'private pin relation') END;
 SELECT CASE WHEN NEW.revision!=1 OR NEW.state!='active' OR EXISTS(SELECT 1 FROM private_sheets WHERE id=NEW.id) OR NOT EXISTS(SELECT 1 FROM campaigns WHERE id=NEW.campaign_id AND game_id=NEW.game_id AND edition_id=NEW.edition_id) OR (SELECT count(*) FROM private_sheets)>=2048 OR (SELECT count(*) FROM private_sheets WHERE campaign_id=NEW.campaign_id AND custodian_id=NEW.custodian_id)>=32 THEN RAISE(ABORT,'invalid private creation') END;
END;

-- The seeded MÖRK BORG pair (005) stays as an ordinary registered game when anything refers
-- to it, and is deleted otherwise, so a fresh database ends with no game. The check is taken
-- once, before any deletion.
CREATE TEMP TABLE native_seed_unused AS SELECT
 NOT EXISTS(SELECT 1 FROM campaigns WHERE game_id='mork-borg' AND edition_id='it-2025')
 AND NOT EXISTS(SELECT 1 FROM private_sheets WHERE game_id='mork-borg' AND edition_id='it-2025')
 AND NOT EXISTS(SELECT 1 FROM character_rulesets WHERE game_id='mork-borg' AND edition_id='it-2025')
 AND NOT EXISTS(SELECT 1 FROM characters WHERE system_id='mork-borg' AND edition='MÖRK BORG — IT 2025')
 AND NOT EXISTS(SELECT 1 FROM source_rulesets WHERE game_id='mork-borg' AND edition_id='it-2025')
 AND NOT EXISTS(SELECT 1 FROM sources WHERE system_id='mork-borg' AND edition='MÖRK BORG — IT 2025')
 AND NOT EXISTS(SELECT 1 FROM source_uploads WHERE system_id='mork-borg' AND edition='MÖRK BORG — IT 2025')
 AND NOT EXISTS(SELECT 1 FROM definition_drafts WHERE game_id='mork-borg' AND edition_id='it-2025')
 AND NOT EXISTS(SELECT 1 FROM definition_versions WHERE game_id='mork-borg' AND edition_id='it-2025')
 AND NOT EXISTS(SELECT 1 FROM catalogues WHERE game_id='mork-borg' AND edition_id='it-2025')
 AND NOT EXISTS(SELECT 1 FROM catalogue_drafts WHERE game_id='mork-borg' AND edition_id='it-2025')
 AND NOT EXISTS(SELECT 1 FROM registry_events WHERE game_id='mork-borg' AND edition_id='it-2025')
 AND NOT EXISTS(SELECT 1 FROM legacy_ruleset_aliases WHERE game_id='mork-borg' AND edition_id='it-2025' AND edition IS NOT 'MÖRK BORG — IT 2025')
 AS unused;
DROP TRIGGER ruleset_aliases_immutable_delete;
DROP TRIGGER registry_editions_immutable_delete;
DROP TRIGGER registry_games_immutable_delete;
DELETE FROM legacy_ruleset_aliases WHERE system_id='mork-borg' AND edition='MÖRK BORG — IT 2025' AND game_id='mork-borg' AND edition_id='it-2025' AND (SELECT unused FROM native_seed_unused);
DELETE FROM registry_editions WHERE game_id='mork-borg' AND edition_id='it-2025' AND (SELECT unused FROM native_seed_unused);
DELETE FROM registry_games WHERE game_id='mork-borg' AND label='MÖRK BORG' AND (SELECT unused FROM native_seed_unused) AND NOT EXISTS(SELECT 1 FROM registry_editions WHERE game_id='mork-borg');
CREATE TRIGGER registry_games_immutable_delete BEFORE DELETE ON registry_games BEGIN SELECT RAISE(ABORT,'Registry identities are retained'); END;
CREATE TRIGGER registry_editions_immutable_delete BEFORE DELETE ON registry_editions BEGIN SELECT RAISE(ABORT,'Registry editions are retained'); END;
CREATE TRIGGER ruleset_aliases_immutable_delete BEFORE DELETE ON legacy_ruleset_aliases BEGIN SELECT RAISE(ABORT,'Ruleset aliases are retained'); END;
DROP TABLE native_seed_unused;

-- Last: nothing above names the column (no index, trigger or view does).
ALTER TABLE registry_editions DROP COLUMN native;
PRAGMA user_version=38;
