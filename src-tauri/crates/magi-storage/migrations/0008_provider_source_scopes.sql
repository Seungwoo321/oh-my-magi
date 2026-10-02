CREATE TABLE provider_source_scopes (
    grant_id TEXT PRIMARY KEY CHECK (length(grant_id) BETWEEN 1 AND 128),
    provider_profile_id TEXT NOT NULL,
    canonical_path TEXT NOT NULL
        CHECK (length(canonical_path) BETWEEN 1 AND 4096)
        CHECK (substr(canonical_path, 1, 1) = '/'),
    root_device INTEGER NOT NULL CHECK (root_device >= 0),
    root_inode INTEGER NOT NULL CHECK (root_inode >= 0),
    created_at TEXT NOT NULL,
    revoked_at TEXT CHECK (revoked_at IS NULL OR length(revoked_at) BETWEEN 1 AND 64),
    FOREIGN KEY (provider_profile_id) REFERENCES provider_profile_heads(provider_profile_id)
);

CREATE UNIQUE INDEX provider_source_scopes_active_path
    ON provider_source_scopes(provider_profile_id, canonical_path)
    WHERE revoked_at IS NULL;

CREATE TRIGGER provider_source_scopes_identity_immutable
BEFORE UPDATE ON provider_source_scopes
WHEN NEW.grant_id <> OLD.grant_id
    OR NEW.provider_profile_id <> OLD.provider_profile_id
    OR NEW.canonical_path <> OLD.canonical_path
    OR NEW.root_device <> OLD.root_device
    OR NEW.root_inode <> OLD.root_inode
    OR NEW.created_at <> OLD.created_at
BEGIN
    SELECT RAISE(ABORT, 'provider source scope identity is immutable');
END;

CREATE TRIGGER provider_source_scopes_revoke_once
BEFORE UPDATE OF revoked_at ON provider_source_scopes
WHEN OLD.revoked_at IS NOT NULL OR NEW.revoked_at IS NULL
BEGIN
    SELECT RAISE(ABORT, 'provider source scopes can only be revoked once');
END;

CREATE TRIGGER provider_source_scopes_no_delete
BEFORE DELETE ON provider_source_scopes
BEGIN
    SELECT RAISE(ABORT, 'provider source scopes are retained after revocation');
END;
