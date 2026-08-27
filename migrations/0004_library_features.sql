ALTER TABLE assets
    ADD COLUMN favorite BOOLEAN NOT NULL DEFAULT FALSE,
    ADD COLUMN archived BOOLEAN NOT NULL DEFAULT FALSE;

ALTER TABLE blobs
    ADD COLUMN content_blake3 TEXT,
    ADD COLUMN storage_encoding TEXT NOT NULL DEFAULT 'legacy-e2ee-v1',
    ADD COLUMN stored_size BIGINT;

UPDATE blobs SET stored_size = ciphertext_size;
ALTER TABLE blobs ALTER COLUMN stored_size SET NOT NULL;
ALTER TABLE blobs ALTER COLUMN ciphertext_size DROP NOT NULL;

ALTER TABLE blobs ALTER COLUMN wrapped_key DROP NOT NULL;
ALTER TABLE blobs ALTER COLUMN key_nonce DROP NOT NULL;
ALTER TABLE blobs ALTER COLUMN nonce_prefix DROP NOT NULL;

CREATE UNIQUE INDEX blobs_plain_content_unique_idx
    ON blobs(account_id, content_blake3)
    WHERE storage_encoding = 'plain-v1';

ALTER TABLE resources ADD COLUMN metadata JSONB;

UPDATE uploads
SET state = 'failed', error = 'legacy encrypted upload must be prepared again', updated_at = now()
WHERE state = 'uploading';

CREATE INDEX assets_timeline_idx
    ON assets(account_id, source_created_at_ms DESC, id DESC)
    WHERE deleted_at IS NULL;

CREATE INDEX assets_library_filter_idx
    ON assets(account_id, favorite, archived, source_created_at_ms DESC);

CREATE TABLE albums (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    account_id UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    device_id UUID NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    source_album_id TEXT NOT NULL,
    name TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE(account_id, device_id, source_album_id)
);

CREATE TABLE album_assets (
    album_id UUID NOT NULL REFERENCES albums(id) ON DELETE CASCADE,
    asset_id UUID NOT NULL REFERENCES assets(id) ON DELETE CASCADE,
    added_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY(album_id, asset_id)
);

CREATE INDEX album_assets_asset_idx ON album_assets(asset_id);

CREATE TABLE tags (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    account_id UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE(account_id, name)
);

CREATE TABLE tag_assets (
    tag_id UUID NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
    asset_id UUID NOT NULL REFERENCES assets(id) ON DELETE CASCADE,
    added_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY(tag_id, asset_id)
);

CREATE INDEX tag_assets_asset_idx ON tag_assets(asset_id);

CREATE TABLE account_changes (
    sequence BIGSERIAL PRIMARY KEY,
    account_id UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    entity_kind TEXT NOT NULL,
    entity_id UUID NOT NULL,
    operation TEXT NOT NULL,
    changed_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX account_changes_cursor_idx ON account_changes(account_id, sequence);

INSERT INTO account_changes(account_id, entity_kind, entity_id, operation, changed_at)
SELECT account_id, 'asset', id, 'upsert', updated_at FROM assets;

CREATE TABLE api_keys (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    account_id UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    device_id UUID NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    prefix TEXT NOT NULL,
    token_hash BYTEA NOT NULL UNIQUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_used_at TIMESTAMPTZ,
    revoked_at TIMESTAMPTZ
);

CREATE INDEX api_keys_account_idx ON api_keys(account_id, created_at DESC);

CREATE TABLE audit_events (
    sequence BIGSERIAL PRIMARY KEY,
    account_id UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    actor_kind TEXT NOT NULL,
    actor_id UUID,
    action TEXT NOT NULL,
    entity_kind TEXT,
    entity_id UUID,
    occurred_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX audit_events_account_idx ON audit_events(account_id, sequence DESC);
