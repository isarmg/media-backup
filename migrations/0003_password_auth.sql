ALTER TABLE accounts
    ADD COLUMN username TEXT,
    ADD COLUMN password_hash TEXT;

UPDATE accounts
SET username = 'user_' || substring(replace(id::text, '-', '') FROM 1 FOR 8)
WHERE username IS NULL;

ALTER TABLE accounts ALTER COLUMN username SET NOT NULL;

CREATE UNIQUE INDEX accounts_username_unique_idx ON accounts(lower(username));

DROP INDEX IF EXISTS accounts_setup_token_hash_unique_idx;
ALTER TABLE accounts DROP COLUMN setup_token_hash;
