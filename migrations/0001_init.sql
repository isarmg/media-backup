CREATE EXTENSION IF NOT EXISTS pgcrypto;

CREATE TABLE accounts (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE devices (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    account_id UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    platform TEXT NOT NULL,
    token_hash BYTEA NOT NULL UNIQUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE assets (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    account_id UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    device_id UUID NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    source_asset_id TEXT NOT NULL,
    media_kind TEXT NOT NULL,
    source_created_at_ms BIGINT NOT NULL,
    deleted_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE(account_id, device_id, source_asset_id)
);

CREATE TABLE blobs (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    account_id UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    dedup_token TEXT NOT NULL,
    plaintext_size BIGINT NOT NULL,
    ciphertext_size BIGINT NOT NULL,
    storage_path TEXT NOT NULL,
    wrapped_key TEXT NOT NULL,
    key_nonce TEXT NOT NULL,
    nonce_prefix TEXT NOT NULL,
    part_manifest JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE(account_id, dedup_token)
);

CREATE TABLE resources (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    asset_id UUID NOT NULL REFERENCES assets(id) ON DELETE CASCADE,
    blob_id UUID NOT NULL REFERENCES blobs(id) ON DELETE RESTRICT,
    source_resource_id TEXT NOT NULL,
    role TEXT NOT NULL,
    filename TEXT NOT NULL,
    mime_type TEXT NOT NULL,
    metadata_nonce TEXT,
    metadata_ciphertext TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE(asset_id, source_resource_id)
);

CREATE TABLE uploads (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    account_id UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    device_id UUID NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    asset_id UUID NOT NULL REFERENCES assets(id) ON DELETE CASCADE,
    source_resource_id TEXT NOT NULL,
    dedup_token TEXT NOT NULL,
    request JSONB NOT NULL,
    state TEXT NOT NULL DEFAULT 'uploading',
    error TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    completed_at TIMESTAMPTZ
);

CREATE INDEX uploads_lookup_idx
    ON uploads(account_id, device_id, source_resource_id, dedup_token, state);

CREATE TABLE upload_parts (
    upload_id UUID NOT NULL REFERENCES uploads(id) ON DELETE CASCADE,
    part_index INTEGER NOT NULL,
    expected_size BIGINT NOT NULL,
    expected_blake3 TEXT NOT NULL,
    received_size BIGINT,
    received_at TIMESTAMPTZ,
    PRIMARY KEY(upload_id, part_index)
);

CREATE INDEX resources_asset_idx ON resources(asset_id);
CREATE INDEX assets_account_idx ON assets(account_id, created_at DESC);
