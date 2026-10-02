ALTER TABLE runs ADD COLUMN question_text TEXT NOT NULL DEFAULT '';

CREATE INDEX runs_history_keyset
    ON runs(created_at DESC, run_id DESC)
    WHERE deleted_at IS NULL;

INSERT INTO store_meta (key, value)
VALUES ('history_membership_generation', '0')
ON CONFLICT(key) DO NOTHING;

CREATE TABLE role_preset_revisions (
    preset_id TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK (revision >= 0),
    digest TEXT NOT NULL CHECK (length(digest) = 64 AND digest = lower(digest)),
    payload_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (preset_id, revision),
    UNIQUE (preset_id, digest)
);

CREATE TABLE role_preset_heads (
    preset_id TEXT PRIMARY KEY,
    revision INTEGER NOT NULL CHECK (revision >= 0),
    updated_at TEXT NOT NULL,
    FOREIGN KEY (preset_id, revision)
        REFERENCES role_preset_revisions(preset_id, revision)
);

CREATE TRIGGER role_preset_revisions_no_update
BEFORE UPDATE ON role_preset_revisions
BEGIN
    SELECT RAISE(ABORT, 'role preset revisions are immutable');
END;

CREATE TRIGGER role_preset_revisions_no_delete
BEFORE DELETE ON role_preset_revisions
BEGIN
    SELECT RAISE(ABORT, 'role preset revisions are immutable');
END;

CREATE TABLE source_freshness_observations (
    observation_id INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id TEXT NOT NULL,
    source_id TEXT NOT NULL,
    captured_digest TEXT NOT NULL CHECK (length(captured_digest) = 64 AND captured_digest = lower(captured_digest)),
    observed_digest TEXT CHECK (observed_digest IS NULL OR (length(observed_digest) = 64 AND observed_digest = lower(observed_digest))),
    status TEXT NOT NULL CHECK (status IN ('unchanged', 'changed', 'missing', 'unreadable', 'unchecked')),
    observed_at_epoch_ms INTEGER NOT NULL CHECK (observed_at_epoch_ms >= 0),
    FOREIGN KEY (run_id) REFERENCES runs(run_id),
    CHECK (
        (status = 'unchanged' AND observed_digest = captured_digest) OR
        (status = 'changed' AND observed_digest IS NOT NULL AND observed_digest <> captured_digest) OR
        (status IN ('missing', 'unreadable', 'unchecked') AND observed_digest IS NULL)
    )
);

CREATE INDEX source_freshness_by_run_source
    ON source_freshness_observations(run_id, source_id, observed_at_epoch_ms DESC, observation_id DESC);

CREATE TRIGGER source_freshness_no_update
BEFORE UPDATE ON source_freshness_observations
BEGIN
    SELECT RAISE(ABORT, 'source freshness observations are append-only');
END;

CREATE TRIGGER source_freshness_no_delete
BEFORE DELETE ON source_freshness_observations
BEGIN
    SELECT RAISE(ABORT, 'source freshness observations are append-only');
END;

CREATE TRIGGER source_capture_manifests_no_update
BEFORE UPDATE ON source_capture_manifests
BEGIN
    SELECT RAISE(ABORT, 'source capture manifests are immutable');
END;

CREATE TRIGGER source_capture_manifests_no_delete
BEFORE DELETE ON source_capture_manifests
BEGIN
    SELECT RAISE(ABORT, 'source capture manifests are immutable');
END;
