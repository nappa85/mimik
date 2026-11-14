-- The home page's per-account "seen" mark (docs/auth-and-sessions.md, Home page): the time of the
-- newest item the account marked as seen. It lives in its own table because `accounts` only changes
-- with an epoch bump, which ends the account's other sessions. The mark never moves backwards
-- and is never in the future; it is kept (never deleted).
CREATE TABLE account_home_marks (
 account_id INTEGER PRIMARY KEY REFERENCES accounts(id) CHECK(account_id>0),
 seen_at INTEGER NOT NULL CHECK(seen_at BETWEEN 1 AND 253402300799)
) STRICT;
CREATE TRIGGER account_home_marks_insert BEFORE INSERT ON account_home_marks WHEN NEW.seen_at>mimik_auth_now() BEGIN SELECT RAISE(ABORT,'home mark is in the future'); END;
CREATE TRIGGER account_home_marks_update BEFORE UPDATE ON account_home_marks WHEN NEW.account_id IS NOT OLD.account_id OR NEW.seen_at<OLD.seen_at OR NEW.seen_at>mimik_auth_now() BEGIN SELECT RAISE(ABORT,'home mark never moves backwards or into the future'); END;
CREATE TRIGGER account_home_marks_delete BEFORE DELETE ON account_home_marks BEGIN SELECT RAISE(ABORT,'home marks are retained'); END;
-- Indexes of the home queries: each is bounded by its index, not by a table scan.
CREATE INDEX campaign_events_target ON campaign_events(target_id,occurred_at);
CREATE INDEX campaign_events_campaign ON campaign_events(campaign_id,occurred_at);
CREATE INDEX campaign_requests_account ON campaign_requests(account_id,state);
CREATE INDEX account_events_action ON account_events(action,occurred_at);
CREATE INDEX account_credentials_open ON account_credentials(purpose,expires_at) WHERE state='active';
CREATE INDEX source_suggestion_reports_imported ON source_suggestion_reports(imported_at);
CREATE INDEX source_pages_complete ON source_pages(source_id,complete,pdf_page);
PRAGMA user_version=31;
