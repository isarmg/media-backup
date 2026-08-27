ALTER TABLE accounts
    ADD COLUMN display_name TEXT NOT NULL DEFAULT '默认用户',
    ADD COLUMN storage_path TEXT,
    ADD COLUMN quota_bytes BIGINT NOT NULL DEFAULT 107374182400 CHECK (quota_bytes >= 0),
    ADD COLUMN enabled BOOLEAN NOT NULL DEFAULT TRUE,
    ADD COLUMN setup_token_hash BYTEA;

UPDATE accounts
SET storage_path = 'blobs/' || id::text
WHERE storage_path IS NULL;

ALTER TABLE accounts ALTER COLUMN storage_path SET NOT NULL;

CREATE UNIQUE INDEX accounts_storage_path_unique_idx ON accounts(storage_path);
CREATE UNIQUE INDEX accounts_setup_token_hash_unique_idx
    ON accounts(setup_token_hash)
    WHERE setup_token_hash IS NOT NULL;
