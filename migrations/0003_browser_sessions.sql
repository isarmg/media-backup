CREATE TABLE auth_users (
    id              TEXT PRIMARY KEY,
    username        TEXT NOT NULL,
    password_hash   TEXT NOT NULL,
    role            TEXT NOT NULL DEFAULT 'admin',
    active          INTEGER NOT NULL DEFAULT 1 CHECK (active IN (0, 1)),
    session_version INTEGER NOT NULL DEFAULT 1 CHECK (session_version > 0),
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL
);

CREATE UNIQUE INDEX auth_users_username_idx
    ON auth_users(lower(username));

CREATE TABLE auth_sessions (
    id                   TEXT PRIMARY KEY,
    user_id              TEXT NOT NULL REFERENCES auth_users(id) ON DELETE CASCADE,
    token_hash           BLOB NOT NULL UNIQUE CHECK (length(token_hash) = 32),
    csrf_hash            BLOB NOT NULL CHECK (length(csrf_hash) = 32),
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

CREATE TRIGGER auth_users_security_version
AFTER UPDATE OF password_hash, role, active ON auth_users
FOR EACH ROW
WHEN OLD.password_hash IS NOT NEW.password_hash
  OR OLD.role IS NOT NEW.role
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
