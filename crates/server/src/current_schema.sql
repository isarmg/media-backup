CREATE TABLE product_metadata (
    singleton INTEGER PRIMARY KEY NOT NULL CHECK (singleton = 1),
    application TEXT NOT NULL,
    application_version TEXT NOT NULL,
    schema_revision INTEGER NOT NULL,
    schema_sha256 TEXT NOT NULL
);

CREATE TABLE accounts (
    id TEXT PRIMARY KEY,
    created_at TEXT NOT NULL,
    display_name TEXT NOT NULL DEFAULT '默认用户',
    storage_path TEXT NOT NULL,
    quota_bytes INTEGER NOT NULL DEFAULT 107374182400,
    enabled INTEGER NOT NULL DEFAULT 1,
    username TEXT NOT NULL,
    password_hash TEXT
);

CREATE UNIQUE INDEX accounts_storage_path_unique_idx ON accounts(storage_path);
CREATE UNIQUE INDEX accounts_username_unique_idx ON accounts(lower(username));

CREATE TABLE devices (
    id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    platform TEXT NOT NULL,
    token_hash BLOB NOT NULL UNIQUE,
    created_at TEXT NOT NULL,
    last_seen_at TEXT NOT NULL
);

CREATE TABLE assets (
    id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    device_id TEXT NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    source_asset_id TEXT NOT NULL,
    media_kind TEXT NOT NULL,
    source_created_at_ms INTEGER NOT NULL,
    favorite INTEGER NOT NULL DEFAULT 0,
    archived INTEGER NOT NULL DEFAULT 0,
    deleted_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(account_id, device_id, source_asset_id)
);

CREATE TABLE blobs (
    id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    plaintext_size INTEGER NOT NULL,
    stored_size INTEGER NOT NULL,
    storage_path TEXT NOT NULL,
    part_manifest TEXT NOT NULL,
    content_blake3 TEXT NOT NULL,
    storage_encoding TEXT NOT NULL CHECK (storage_encoding = 'plain-v1'),
    created_at TEXT NOT NULL
);

CREATE UNIQUE INDEX blobs_content_unique_idx ON blobs(account_id, content_blake3);

CREATE TABLE resources (
    id TEXT PRIMARY KEY,
    asset_id TEXT NOT NULL REFERENCES assets(id) ON DELETE CASCADE,
    blob_id TEXT NOT NULL REFERENCES blobs(id) ON DELETE RESTRICT,
    source_resource_id TEXT NOT NULL,
    role TEXT NOT NULL,
    filename TEXT NOT NULL,
    mime_type TEXT NOT NULL,
    metadata TEXT,
    created_at TEXT NOT NULL,
    UNIQUE(asset_id, source_resource_id)
);

CREATE TABLE uploads (
    id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    device_id TEXT NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    asset_id TEXT NOT NULL REFERENCES assets(id) ON DELETE CASCADE,
    source_resource_id TEXT NOT NULL,
    content_blake3 TEXT NOT NULL,
    request TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'uploading',
    error TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    completed_at TEXT,
    commit_state TEXT NOT NULL DEFAULT 'receiving'
        CHECK (commit_state IN (
            'receiving',
            'commit_started',
            'finalizing',
            'committed',
            'unknown',
            'failed'
        )),
    commit_staged_key TEXT,
    commit_final_key TEXT,
    commit_account_path TEXT,
    commit_expected_size INTEGER,
    commit_expected_blake3 TEXT,
    commit_blob_id TEXT,
    commit_resource_id TEXT,
    commit_deduplicated INTEGER NOT NULL DEFAULT 0
        CHECK (commit_deduplicated IN (0, 1)),
    commit_error TEXT,
    commit_started_at TEXT,
    finalized_at TEXT
);

CREATE INDEX uploads_lookup_idx
    ON uploads(account_id, device_id, source_resource_id, content_blake3, state);
CREATE INDEX uploads_commit_reconcile_idx
    ON uploads(commit_state, updated_at);

CREATE TABLE upload_parts (
    upload_id TEXT NOT NULL REFERENCES uploads(id) ON DELETE CASCADE,
    part_index INTEGER NOT NULL,
    expected_size INTEGER NOT NULL,
    expected_blake3 TEXT NOT NULL,
    received_size INTEGER,
    received_at TEXT,
    PRIMARY KEY(upload_id, part_index)
);

CREATE INDEX resources_asset_idx ON resources(asset_id);
CREATE INDEX assets_account_idx ON assets(account_id, created_at DESC);
CREATE INDEX assets_timeline_idx ON assets(account_id, source_created_at_ms DESC, id DESC) WHERE deleted_at IS NULL;
CREATE INDEX assets_library_filter_idx ON assets(account_id, favorite, archived, source_created_at_ms DESC);

CREATE TABLE albums (
    id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    device_id TEXT NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    source_album_id TEXT NOT NULL,
    name TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(account_id, device_id, source_album_id)
);

CREATE TABLE album_assets (
    album_id TEXT NOT NULL REFERENCES albums(id) ON DELETE CASCADE,
    asset_id TEXT NOT NULL REFERENCES assets(id) ON DELETE CASCADE,
    added_at TEXT NOT NULL,
    PRIMARY KEY(album_id, asset_id)
);

CREATE INDEX album_assets_asset_idx ON album_assets(asset_id);

CREATE TABLE tags (
    id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(account_id, name)
);

CREATE TABLE tag_assets (
    tag_id TEXT NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
    asset_id TEXT NOT NULL REFERENCES assets(id) ON DELETE CASCADE,
    added_at TEXT NOT NULL,
    PRIMARY KEY(tag_id, asset_id)
);

CREATE INDEX tag_assets_asset_idx ON tag_assets(asset_id);

CREATE TABLE account_changes (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    entity_kind TEXT NOT NULL,
    entity_id TEXT NOT NULL,
    operation TEXT NOT NULL,
    changed_at TEXT NOT NULL
);

CREATE INDEX account_changes_cursor_idx ON account_changes(account_id, sequence);

CREATE TABLE api_keys (
    id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    device_id TEXT NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    prefix TEXT NOT NULL,
    token_hash BLOB NOT NULL UNIQUE,
    created_at TEXT NOT NULL,
    last_used_at TEXT,
    revoked_at TEXT
);

CREATE INDEX api_keys_account_idx ON api_keys(account_id, created_at DESC);

CREATE TABLE audit_events (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    actor_kind TEXT NOT NULL,
    actor_id TEXT,
    action TEXT NOT NULL,
    entity_kind TEXT,
    entity_id TEXT,
    occurred_at TEXT NOT NULL
);

CREATE INDEX audit_events_account_idx ON audit_events(account_id, sequence DESC);

CREATE TABLE auth_users (
    id              TEXT PRIMARY KEY,
    username        TEXT NOT NULL CHECK (
        length(username) BETWEEN 3 AND 64
        AND username = lower(username)
        AND username NOT GLOB '*[^a-z0-9._-]*'
        AND substr(username, 1, 1) GLOB '[a-z0-9]'
        AND substr(username, -1, 1) GLOB '[a-z0-9]'
        AND instr(username, '@') = 0
    ),
    password_hash   TEXT NOT NULL,
    active          INTEGER NOT NULL DEFAULT 1 CHECK (active IN (0, 1)),
    session_version INTEGER NOT NULL DEFAULT 1 CHECK (session_version > 0),
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL
);

CREATE UNIQUE INDEX auth_users_username_idx
    ON auth_users(username);

CREATE TABLE auth_sessions (
    id                   TEXT PRIMARY KEY,
    user_id              TEXT NOT NULL REFERENCES auth_users(id) ON DELETE CASCADE,
    token_hash           BLOB NOT NULL UNIQUE CHECK (length(token_hash) = 32),
    user_session_version INTEGER NOT NULL CHECK (user_session_version > 0),
    created_at           INTEGER NOT NULL,
    last_seen_at         INTEGER NOT NULL,
    idle_expires_at      INTEGER NOT NULL,
    absolute_expires_at  INTEGER NOT NULL,
    revoked_at           INTEGER,
    user_agent           TEXT,
    created_ip           TEXT,
    CHECK (last_seen_at >= created_at),
    CHECK (idle_expires_at > created_at),
    CHECK (absolute_expires_at >= idle_expires_at)
);

CREATE INDEX auth_sessions_user_idx
    ON auth_sessions(user_id, revoked_at);

CREATE INDEX auth_sessions_expiry_idx
    ON auth_sessions(idle_expires_at, absolute_expires_at)
    WHERE revoked_at IS NULL;

CREATE TABLE admin_session_csrf_tokens (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL REFERENCES auth_sessions(id) ON DELETE CASCADE,
    token_hash BLOB NOT NULL CHECK (length(token_hash) = 32),
    created_at INTEGER NOT NULL,
    UNIQUE(session_id, token_hash)
);

CREATE INDEX admin_session_csrf_tokens_session_idx
    ON admin_session_csrf_tokens(session_id, created_at DESC, id DESC);

CREATE TRIGGER auth_users_security_version
AFTER UPDATE OF password_hash, active ON auth_users
FOR EACH ROW
WHEN OLD.password_hash IS NOT NEW.password_hash
  OR OLD.active IS NOT NEW.active
BEGIN
    UPDATE auth_users
       SET session_version = OLD.session_version + 1,
           updated_at = unixepoch()
     WHERE id = OLD.id;

    UPDATE auth_sessions
       SET revoked_at = COALESCE(revoked_at, unixepoch())
     WHERE user_id = OLD.id;
END;
