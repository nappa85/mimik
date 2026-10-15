-- No data conversion: production Owner authority is administrative only.
DROP VIEW campaign_request_auth;
CREATE VIEW campaign_request_auth AS
 SELECT a.id AS account_id,a.epoch AS epoch,s.id AS session_id,s.expires_at AS deadline
 FROM auth_sessions s JOIN accounts a ON a.id=s.account_id
 WHERE s.state='active' AND a.state='enabled' AND a.epoch=s.epoch
 AND s.id=json_extract(mimik_request_auth(),'$.id')
 AND s.account_id=json_extract(mimik_request_auth(),'$.account')
 AND s.epoch=json_extract(mimik_request_auth(),'$.epoch')
 AND s.expires_at=json_extract(mimik_request_auth(),'$.deadline')
 AND s.expires_at>mimik_auth_now()
 AND (NOT EXISTS(SELECT 1 FROM account_owner o WHERE o.account_id=a.id)
   OR json_extract(mimik_request_auth(),'$.development_owner_play') IS 1);
CREATE TRIGGER private_owner_supervision_insert BEFORE INSERT ON private_sheets
WHEN NOT EXISTS(SELECT 1 FROM campaign_members m JOIN accounts a ON a.id=m.account_id
 WHERE m.campaign_id=NEW.campaign_id AND m.state='active' AND m.role='master'
 AND a.state='enabled' AND (NOT EXISTS(SELECT 1 FROM account_owner o WHERE o.account_id=a.id)
   OR json_extract(mimik_request_auth(),'$.development_owner_play') IS 1))
BEGIN SELECT RAISE(ABORT,'enabled gameplay Master required'); END;
CREATE TRIGGER private_owner_supervision_update BEFORE UPDATE ON private_sheets
WHEN NEW.state='active' AND NOT EXISTS(SELECT 1 FROM campaign_members m JOIN accounts a ON a.id=m.account_id
 WHERE m.campaign_id=NEW.campaign_id AND m.state='active' AND m.role='master'
 AND a.state='enabled' AND (NOT EXISTS(SELECT 1 FROM account_owner o WHERE o.account_id=a.id)
   OR json_extract(mimik_request_auth(),'$.development_owner_play') IS 1))
BEGIN SELECT RAISE(ABORT,'enabled gameplay Master required'); END;
CREATE TRIGGER campaign_owner_membership_insert BEFORE INSERT ON campaign_members
WHEN EXISTS(SELECT 1 FROM account_owner o WHERE o.account_id=NEW.account_id)
 AND json_extract(mimik_request_auth(),'$.development_owner_play') IS NOT 1
BEGIN SELECT RAISE(ABORT,'Owner is administration only'); END;
CREATE TRIGGER campaign_owner_membership_update BEFORE UPDATE ON campaign_members
WHEN EXISTS(SELECT 1 FROM account_owner o WHERE o.account_id=NEW.account_id)
 AND NEW.state='active' AND (OLD.state='inactive' OR (OLD.role='player' AND NEW.role='master'))
 AND json_extract(mimik_request_auth(),'$.development_owner_play') IS NOT 1
BEGIN SELECT RAISE(ABORT,'Owner is administration only'); END;
PRAGMA user_version=13;
