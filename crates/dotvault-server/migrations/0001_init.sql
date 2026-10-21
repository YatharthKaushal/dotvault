-- dotvault server schema.
--
-- Everything here is either ciphertext (`*_ct`, `wrapped_*`), a hash of a
-- derived secret (`auth_hash`, `recovery_hash`, `token_hash`) or metadata.
-- There is deliberately no column that could hold a password, a vault key, a
-- share key or any plaintext env value.

-- Server-local settings (e.g. the persisted fake-salt secret).
CREATE TABLE meta (
    key   TEXT PRIMARY KEY NOT NULL,
    value BLOB NOT NULL
);

CREATE TABLE users (
    id                         INTEGER PRIMARY KEY AUTOINCREMENT,
    -- stored lowercased; NOCASE makes uniqueness case-insensitive regardless
    username                   TEXT NOT NULL UNIQUE COLLATE NOCASE,
    auth_hash                  BLOB NOT NULL,   -- SHA-256(domain || auth_key)
    kdf_salt                   BLOB NOT NULL,
    kdf_params                 TEXT NOT NULL,   -- JSON {"m_kib":..,"t":..,"p":..}
    wrapped_vault_key          BLOB NOT NULL,
    recovery_hash              BLOB NOT NULL,   -- SHA-256(domain || recovery_auth_key)
    wrapped_vault_key_recovery BLOB NOT NULL,
    created_at                 INTEGER NOT NULL
);

CREATE TABLE sessions (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id      INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    token_hash   BLOB NOT NULL UNIQUE,          -- SHA-256(token); the token itself is never stored
    device_name  TEXT,
    created_at   INTEGER NOT NULL,
    last_used_at INTEGER NOT NULL,
    expires_at   INTEGER NOT NULL
);
CREATE INDEX sessions_user_id ON sessions(user_id);
CREATE INDEX sessions_expires_at ON sessions(expires_at);

-- Item ids are chosen by the client (they are part of the AEAD context), so
-- they are unique per user rather than globally: one user can neither probe
-- nor squat another user's ids.
CREATE TABLE vault_items (
    user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    id         TEXT NOT NULL,
    name_ct    BLOB NOT NULL,
    version    INTEGER NOT NULL,                -- current version number
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    deleted_at INTEGER,
    PRIMARY KEY (user_id, id)
) WITHOUT ROWID;

CREATE TABLE vault_item_versions (
    user_id         INTEGER NOT NULL,
    item_id         TEXT NOT NULL,
    version         INTEGER NOT NULL,
    payload_ct      BLOB NOT NULL,
    source_share_id TEXT,
    created_at      INTEGER NOT NULL,
    PRIMARY KEY (user_id, item_id, version),
    FOREIGN KEY (user_id, item_id) REFERENCES vault_items(user_id, id) ON DELETE CASCADE
) WITHOUT ROWID;

CREATE TABLE shares (
    id         TEXT PRIMARY KEY NOT NULL,
    owner_id   INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    payload_ct BLOB,                            -- NULL once revoked or purged
    expires_at INTEGER NOT NULL,
    max_views  INTEGER,
    view_count INTEGER NOT NULL DEFAULT 0,
    revoked_at INTEGER,
    created_at INTEGER NOT NULL
);
CREATE INDEX shares_owner ON shares(owner_id, created_at);
CREATE INDEX shares_purge ON shares(expires_at) WHERE payload_ct IS NOT NULL;

CREATE TABLE share_events (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    share_id   TEXT NOT NULL REFERENCES shares(id) ON DELETE CASCADE,
    user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    action     TEXT NOT NULL CHECK (action IN ('view', 'save')),
    client     TEXT,
    created_at INTEGER NOT NULL
);
CREATE INDEX share_events_share ON share_events(share_id, id);
