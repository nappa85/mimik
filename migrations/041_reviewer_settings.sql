-- The owner's API-reviewer settings (docs/api-reviewer-plan.md step 3, page /admin/reviewer):
-- one row with the provider, the base URL and the model name. The API key is not stored here:
-- it is a 0600 file (`reviewer.key`) in the data folder's `secrets/` directory, outside the
-- database and outside backups. The base URL is checked in the application (HTTPS, a domain,
-- no userinfo, the provider's path rule); the column keeps only a length and shape bound. The
-- model name is at most 100 bytes minus the `reviewer:` prefix it gets as a report's origin.
-- IF NOT EXISTS so tests that rewind the version can run it again.
CREATE TABLE IF NOT EXISTS reviewer_settings (
 id INTEGER PRIMARY KEY CHECK(id=1),
 provider TEXT NOT NULL CHECK(provider IN ('mistral','openai','anthropic')),
 base_url TEXT NOT NULL CHECK(length(CAST(base_url AS BLOB)) BETWEEN 1 AND 2048 AND base_url GLOB 'https://*' AND instr(base_url,char(0))=0),
 model TEXT NOT NULL CHECK(length(CAST(model AS BLOB)) BETWEEN 1 AND 91 AND instr(model,char(0))=0),
 changed_by INTEGER NOT NULL CHECK(changed_by>0),
 changed_at INTEGER NOT NULL CHECK(changed_at>0)
) STRICT;
PRAGMA user_version=41;
