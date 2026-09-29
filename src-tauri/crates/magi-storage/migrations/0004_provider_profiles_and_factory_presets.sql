ALTER TABLE role_preset_heads
    ADD COLUMN source TEXT NOT NULL DEFAULT 'user'
    CHECK (source IN ('factory', 'user'));

CREATE TRIGGER role_preset_source_immutable
BEFORE UPDATE OF source ON role_preset_heads
WHEN NEW.source <> OLD.source
BEGIN
    SELECT RAISE(ABORT, 'role preset source cannot be changed');
END;

CREATE TRIGGER factory_role_preset_head_no_update
BEFORE UPDATE ON role_preset_heads
WHEN OLD.source = 'factory'
BEGIN
    SELECT RAISE(ABORT, 'factory role preset is immutable');
END;

CREATE TRIGGER factory_role_preset_head_no_delete
BEFORE DELETE ON role_preset_heads
WHEN OLD.source = 'factory'
BEGIN
    SELECT RAISE(ABORT, 'factory role preset is immutable');
END;

CREATE TABLE active_role_preset_selection (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    preset_id TEXT NOT NULL,
    selection_revision INTEGER NOT NULL CHECK (selection_revision >= 0),
    updated_at TEXT NOT NULL,
    FOREIGN KEY (preset_id) REFERENCES role_preset_heads(preset_id)
);

CREATE TABLE provider_profile_home_roots (
    runtime_home_id TEXT PRIMARY KEY
        CHECK (length(runtime_home_id) BETWEEN 1 AND 128)
        CHECK (runtime_home_id NOT GLOB '*[^A-Za-z0-9_-]*'),
    provider_profile_id TEXT NOT NULL,
    UNIQUE (runtime_home_id, provider_profile_id)
);

CREATE TRIGGER provider_profile_home_roots_no_update
BEFORE UPDATE ON provider_profile_home_roots
BEGIN
    SELECT RAISE(ABORT, 'provider runtime home ownership is immutable');
END;

CREATE TRIGGER provider_profile_home_roots_no_delete
BEFORE DELETE ON provider_profile_home_roots
BEGIN
    SELECT RAISE(ABORT, 'provider runtime home ownership is immutable');
END;

CREATE TABLE provider_profile_revisions (
    provider_profile_id TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK (revision >= 0),
    digest TEXT NOT NULL CHECK (length(digest) = 64 AND digest = lower(digest)),
    runtime_home_id TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (provider_profile_id, revision),
    UNIQUE (provider_profile_id, digest),
    FOREIGN KEY (runtime_home_id, provider_profile_id)
        REFERENCES provider_profile_home_roots(runtime_home_id, provider_profile_id)
);

CREATE TRIGGER provider_profile_revisions_no_update
BEFORE UPDATE ON provider_profile_revisions
BEGIN
    SELECT RAISE(ABORT, 'provider profile revisions are immutable');
END;

CREATE TRIGGER provider_profile_revisions_no_delete
BEFORE DELETE ON provider_profile_revisions
BEGIN
    SELECT RAISE(ABORT, 'provider profile revisions are immutable');
END;

CREATE TABLE provider_profile_heads (
    provider_profile_id TEXT PRIMARY KEY,
    provider_id TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK (revision >= 0),
    updated_at TEXT NOT NULL,
    UNIQUE (provider_id, provider_profile_id),
    FOREIGN KEY (provider_profile_id, revision)
        REFERENCES provider_profile_revisions(provider_profile_id, revision)
);

CREATE TRIGGER provider_profile_head_provider_immutable
BEFORE UPDATE OF provider_id ON provider_profile_heads
WHEN NEW.provider_id <> OLD.provider_id
BEGIN
    SELECT RAISE(ABORT, 'provider profile provider is immutable');
END;

CREATE INDEX provider_profile_heads_updated
    ON provider_profile_heads(updated_at DESC, provider_profile_id);

CREATE TABLE active_provider_profile_selections (
    provider_id TEXT PRIMARY KEY,
    provider_profile_id TEXT NOT NULL,
    selection_revision INTEGER NOT NULL CHECK (selection_revision >= 0),
    updated_at TEXT NOT NULL,
    FOREIGN KEY (provider_id, provider_profile_id)
        REFERENCES provider_profile_heads(provider_id, provider_profile_id)
);
