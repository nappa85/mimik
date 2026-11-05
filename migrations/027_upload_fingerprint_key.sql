-- Upload-local secret: never copied to the exclusion history or staged contract.
-- Pre-existing staging rows acquire a getrandom key on their first proposal.
ALTER TABLE source_uploads ADD COLUMN fingerprint_key BLOB
 CHECK(fingerprint_key IS NULL OR length(fingerprint_key)=32);
-- Legacy unkeyed text hashes must not survive as offline guessing oracles.
-- Preserve the confirmed geometry, rationale and revision history.
UPDATE source_upload_exclusions SET proposal_fingerprint=NULL;
PRAGMA user_version=27;
