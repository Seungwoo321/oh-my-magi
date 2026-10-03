CREATE TABLE disclosure_grants (
    run_id TEXT PRIMARY KEY REFERENCES runs(run_id),
    payload_json TEXT NOT NULL CHECK (json_valid(payload_json)),
    revoked_at_epoch_ms INTEGER CHECK (revoked_at_epoch_ms IS NULL OR revoked_at_epoch_ms >= 0)
);
CREATE TABLE external_replays (
    external_replay_id TEXT PRIMARY KEY CHECK (length(trim(external_replay_id)) BETWEEN 1 AND 128),
    file_digest TEXT NOT NULL CHECK (length(file_digest) = 64 AND file_digest NOT GLOB '*[^0-9a-f]*'),
    payload_json TEXT NOT NULL CHECK (json_valid(payload_json)),
    imported_at_epoch_ms INTEGER NOT NULL CHECK (imported_at_epoch_ms >= 0)
);
CREATE TABLE unavailable_objects (
    digest TEXT PRIMARY KEY REFERENCES content_objects(digest),
    removed_at TEXT NOT NULL CHECK (length(trim(removed_at)) > 0),
    reason TEXT NOT NULL CHECK (reason IN ('user_deleted', 'unreferenced'))
);
CREATE TABLE evidence_tombstones (
    run_id TEXT NOT NULL CHECK (length(trim(run_id)) BETWEEN 1 AND 128),
    digest TEXT NOT NULL CHECK (length(digest) = 64 AND digest NOT GLOB '*[^0-9a-f]*'),
    removed_at TEXT NOT NULL CHECK (length(trim(removed_at)) > 0),
    PRIMARY KEY (run_id, digest)
);
DROP TRIGGER dispatch_batches_no_delete;
CREATE TRIGGER dispatch_batches_no_delete BEFORE DELETE ON dispatch_batches
WHEN NOT EXISTS (SELECT 1 FROM run_tombstones WHERE run_id = OLD.run_id)
BEGIN SELECT RAISE(ABORT, 'dispatch batch receipt is immutable'); END;
DROP TRIGGER dispatch_transition_events_no_delete;
CREATE TRIGGER dispatch_transition_events_no_delete BEFORE DELETE ON dispatch_transition_events
WHEN NOT EXISTS (SELECT 1 FROM run_tombstones WHERE run_id = OLD.run_id)
BEGIN SELECT RAISE(ABORT, 'dispatch transition events are append-only'); END;
DROP TRIGGER source_freshness_no_delete;
CREATE TRIGGER source_freshness_no_delete BEFORE DELETE ON source_freshness_observations
WHEN NOT EXISTS (SELECT 1 FROM run_tombstones WHERE run_id = OLD.run_id)
BEGIN SELECT RAISE(ABORT, 'source freshness observations are append-only'); END;
CREATE TRIGGER run_parent_immutable BEFORE UPDATE OF parent_run_id, conversation_id ON runs
WHEN NEW.parent_run_id IS NOT OLD.parent_run_id OR NEW.conversation_id <> OLD.conversation_id
BEGIN SELECT RAISE(ABORT, 'run ancestry is immutable'); END;
