CREATE TABLE store_meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS schema_migrations (
    version INTEGER PRIMARY KEY CHECK (version > 0),
    checksum TEXT NOT NULL,
    applied_at TEXT NOT NULL
);

CREATE TABLE conversations (
    conversation_id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK (revision >= 0),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    deleted_at TEXT
);

CREATE TABLE runs (
    run_id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL,
    parent_run_id TEXT,
    status TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK (revision >= 0),
    generation INTEGER NOT NULL CHECK (generation >= 0),
    input_digest TEXT NOT NULL,
    run_json TEXT NOT NULL,
    input_json TEXT NOT NULL,
    tally_json TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    deleted_at TEXT,
    FOREIGN KEY (conversation_id) REFERENCES conversations(conversation_id),
    FOREIGN KEY (parent_run_id) REFERENCES runs(run_id)
);

CREATE UNIQUE INDEX one_live_run_per_conversation
    ON runs(conversation_id)
    WHERE deleted_at IS NULL AND status NOT IN ('completed', 'cancelled', 'failed');

CREATE INDEX runs_by_conversation_created
    ON runs(conversation_id, created_at DESC);

CREATE TABLE immutable_snapshots (
    object_kind TEXT NOT NULL,
    object_id TEXT NOT NULL,
    digest TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (object_kind, object_id)
);

CREATE TRIGGER immutable_snapshots_no_update
BEFORE UPDATE ON immutable_snapshots
BEGIN
    SELECT RAISE(ABORT, 'immutable snapshot cannot be updated');
END;

CREATE TABLE content_objects (
    digest TEXT PRIMARY KEY CHECK (length(digest) = 64 AND digest = lower(digest)),
    byte_length INTEGER NOT NULL CHECK (byte_length >= 0),
    created_at TEXT NOT NULL
);

CREATE TABLE source_capture_manifests (
    manifest_id TEXT PRIMARY KEY,
    manifest_digest TEXT NOT NULL UNIQUE CHECK (length(manifest_digest) = 64 AND manifest_digest = lower(manifest_digest)),
    disclosure_state TEXT NOT NULL CHECK (disclosure_state IN ('draft', 'approved')),
    payload_json TEXT NOT NULL,
    created_at_epoch_ms INTEGER NOT NULL CHECK (created_at_epoch_ms >= 0)
);

CREATE TABLE source_capture_manifest_objects (
    manifest_id TEXT NOT NULL,
    source_id TEXT NOT NULL,
    object_digest TEXT NOT NULL,
    PRIMARY KEY (manifest_id, source_id),
    FOREIGN KEY (manifest_id) REFERENCES source_capture_manifests(manifest_id),
    FOREIGN KEY (object_digest) REFERENCES content_objects(digest)
);

CREATE TABLE context_drafts (
    draft_id TEXT PRIMARY KEY,
    revision INTEGER NOT NULL CHECK (revision >= 0),
    manifest_id TEXT NOT NULL,
    updated_at_epoch_ms INTEGER NOT NULL CHECK (updated_at_epoch_ms >= 0),
    FOREIGN KEY (manifest_id) REFERENCES source_capture_manifests(manifest_id)
);

CREATE TABLE role_assessments (
    run_id TEXT NOT NULL,
    stage TEXT NOT NULL CHECK (stage IN ('independent_review', 'cross_review')),
    core_id TEXT NOT NULL CHECK (core_id IN ('MELCHIOR-1', 'BALTHASAR-2', 'CASPER-3')),
    attempt_id TEXT NOT NULL,
    attempt_generation INTEGER NOT NULL CHECK (attempt_generation >= 0),
    payload_digest TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (run_id, stage, core_id),
    UNIQUE (run_id, attempt_id),
    FOREIGN KEY (run_id) REFERENCES runs(run_id)
);

CREATE TABLE proposals (
    run_id TEXT PRIMARY KEY,
    proposal_id TEXT NOT NULL UNIQUE,
    proposal_digest TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    UNIQUE (run_id, proposal_id),
    FOREIGN KEY (run_id) REFERENCES runs(run_id)
);

CREATE TABLE ballots (
    run_id TEXT NOT NULL,
    core_id TEXT NOT NULL CHECK (core_id IN ('MELCHIOR-1', 'BALTHASAR-2', 'CASPER-3')),
    attempt_id TEXT NOT NULL,
    attempt_generation INTEGER NOT NULL CHECK (attempt_generation >= 0),
    proposal_id TEXT NOT NULL,
    proposal_digest TEXT NOT NULL,
    vote TEXT NOT NULL CHECK (vote IN ('support', 'oppose', 'abstain')),
    payload_digest TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    revealed_at TEXT,
    PRIMARY KEY (run_id, core_id),
    UNIQUE (run_id, attempt_id),
    FOREIGN KEY (run_id) REFERENCES runs(run_id),
    FOREIGN KEY (run_id, proposal_id) REFERENCES proposals(run_id, proposal_id)
);

CREATE TABLE decision_dossiers (
    run_id TEXT PRIMARY KEY,
    payload_digest TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    FOREIGN KEY (run_id) REFERENCES runs(run_id)
);

CREATE TABLE run_checkpoints (
    run_id TEXT PRIMARY KEY,
    generation INTEGER NOT NULL CHECK (generation >= 0),
    revision INTEGER NOT NULL CHECK (revision >= 0),
    latest_event_sequence INTEGER NOT NULL CHECK (latest_event_sequence >= 0),
    checkpoint_json TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    FOREIGN KEY (run_id) REFERENCES runs(run_id)
);

CREATE TABLE run_events (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    store_generation INTEGER NOT NULL CHECK (store_generation >= 0),
    run_id TEXT NOT NULL,
    run_revision INTEGER NOT NULL CHECK (run_revision >= 0),
    attempt_generation INTEGER NOT NULL CHECK (attempt_generation >= 0),
    event_type TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    FOREIGN KEY (run_id) REFERENCES runs(run_id)
);

CREATE INDEX run_events_by_run_sequence ON run_events(run_id, sequence);

CREATE TABLE commands (
    command_id TEXT PRIMARY KEY,
    command_kind TEXT NOT NULL,
    target_id TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    payload_digest TEXT NOT NULL,
    receipt_json TEXT NOT NULL,
    accepted_at TEXT NOT NULL,
    UNIQUE (command_kind, target_id, idempotency_key)
);

CREATE TABLE dispatch_outbox (
    dispatch_id TEXT PRIMARY KEY,
    run_id TEXT NOT NULL,
    slot_id TEXT NOT NULL,
    stage TEXT NOT NULL,
    core_id TEXT,
    attempt_generation INTEGER NOT NULL CHECK (attempt_generation >= 0),
    input_digest TEXT NOT NULL,
    binding_digest TEXT NOT NULL,
    payload_digest TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('prepared', 'dispatched', 'settled', 'unknown', 'aborted_before_dispatch')),
    provider_request_id TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    FOREIGN KEY (run_id) REFERENCES runs(run_id)
);

CREATE INDEX dispatch_outbox_pending ON dispatch_outbox(state, created_at)
    WHERE state IN ('prepared', 'dispatched', 'unknown');

CREATE TABLE run_tombstones (
    run_id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL,
    deleted_at TEXT NOT NULL,
    payload_digest TEXT NOT NULL
);
