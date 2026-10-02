CREATE TABLE provider_catalog_snapshots (
    catalog_snapshot_id TEXT PRIMARY KEY
        CHECK (length(catalog_snapshot_id) BETWEEN 1 AND 128),
    catalog_digest TEXT NOT NULL
        CHECK (length(catalog_digest) = 64 AND catalog_digest = lower(catalog_digest)),
    provider_id TEXT NOT NULL,
    acp_mode TEXT NOT NULL CHECK (acp_mode = 'acp'),
    provider_profile_id TEXT NOT NULL,
    profile_revision INTEGER NOT NULL CHECK (profile_revision >= 0),
    adapter_id TEXT NOT NULL,
    adapter_version TEXT NOT NULL,
    adapter_digest TEXT NOT NULL
        CHECK (length(adapter_digest) = 64 AND adapter_digest = lower(adapter_digest)),
    fetched_at TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    UNIQUE (provider_profile_id, profile_revision, catalog_snapshot_id),
    FOREIGN KEY (provider_profile_id, profile_revision)
        REFERENCES provider_profile_revisions(provider_profile_id, revision)
);

CREATE INDEX provider_catalog_snapshots_profile
    ON provider_catalog_snapshots(provider_profile_id, profile_revision, fetched_at DESC);

CREATE TRIGGER provider_catalog_snapshots_no_update
BEFORE UPDATE ON provider_catalog_snapshots
BEGIN
    SELECT RAISE(ABORT, 'provider catalog snapshots are immutable');
END;

CREATE TRIGGER provider_catalog_snapshots_no_delete
BEFORE DELETE ON provider_catalog_snapshots
BEGIN
    SELECT RAISE(ABORT, 'provider catalog snapshots are immutable');
END;

CREATE TABLE live_runs (
    run_id TEXT PRIMARY KEY CHECK (length(run_id) BETWEEN 1 AND 128),
    schema_version INTEGER NOT NULL CHECK (schema_version = 1),
    question_text TEXT NOT NULL CHECK (length(question_text) BETWEEN 1 AND 32000),
    revision INTEGER NOT NULL CHECK (revision >= 0),
    model_binding_json TEXT NOT NULL,
    catalog_snapshot_id TEXT NOT NULL,
    model_binding_digest TEXT NOT NULL
        CHECK (length(model_binding_digest) = 64 AND model_binding_digest = lower(model_binding_digest)),
    result_digest TEXT
        CHECK (result_digest IS NULL OR (length(result_digest) = 64 AND result_digest = lower(result_digest))),
    result_byte_length INTEGER CHECK (result_byte_length IS NULL OR result_byte_length >= 0),
    failure_json TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    FOREIGN KEY (catalog_snapshot_id) REFERENCES provider_catalog_snapshots(catalog_snapshot_id),
    FOREIGN KEY (result_digest) REFERENCES content_objects(digest),
    CHECK ((result_digest IS NULL) = (result_byte_length IS NULL))
);

CREATE TRIGGER live_runs_revision_monotonic
BEFORE UPDATE OF revision ON live_runs
WHEN NEW.revision <> OLD.revision + 1
BEGIN
    SELECT RAISE(ABORT, 'live run revisions must advance by one');
END;

CREATE TRIGGER live_runs_model_binding_immutable
BEFORE UPDATE OF model_binding_json, catalog_snapshot_id, model_binding_digest ON live_runs
BEGIN
    SELECT RAISE(ABORT, 'live run model binding is immutable');
END;

CREATE TABLE live_run_outbox (
    admission_sequence INTEGER PRIMARY KEY AUTOINCREMENT CHECK (admission_sequence > 0),
    run_id TEXT NOT NULL UNIQUE,
    state TEXT NOT NULL
        CHECK (state IN ('queued', 'claimed', 'session_creation_intent', 'running', 'unknown', 'completed', 'failed')),
    claim_generation INTEGER NOT NULL DEFAULT 0 CHECK (claim_generation >= 0),
    claim_owner TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    FOREIGN KEY (run_id) REFERENCES live_runs(run_id),
    CHECK (
        (state IN ('claimed', 'session_creation_intent', 'running') AND claim_owner IS NOT NULL)
        OR (state NOT IN ('claimed', 'session_creation_intent', 'running') AND claim_owner IS NULL)
    )
);

CREATE INDEX live_run_outbox_fifo
    ON live_run_outbox(state, admission_sequence);

CREATE TRIGGER live_run_outbox_capacity
BEFORE INSERT ON live_run_outbox
WHEN NEW.state IN ('queued', 'claimed', 'session_creation_intent', 'running', 'unknown')
    AND (
        SELECT count(*) FROM live_run_outbox
        WHERE state IN ('queued', 'claimed', 'session_creation_intent', 'running', 'unknown')
    ) >= 10
BEGIN
    SELECT RAISE(ABORT, 'live run queue is full');
END;

CREATE TRIGGER live_run_outbox_generation_monotonic
BEFORE UPDATE OF claim_generation ON live_run_outbox
WHEN NEW.claim_generation < OLD.claim_generation
BEGIN
    SELECT RAISE(ABORT, 'live run claim generation cannot decrease');
END;

CREATE TRIGGER live_run_outbox_state_guard
BEFORE UPDATE OF state ON live_run_outbox
WHEN OLD.state <> NEW.state AND NOT (
    (OLD.state = 'queued' AND NEW.state IN ('claimed', 'failed'))
    OR (OLD.state = 'claimed' AND NEW.state IN ('queued', 'session_creation_intent', 'failed'))
    OR (OLD.state = 'session_creation_intent' AND NEW.state IN ('running', 'unknown', 'failed'))
    OR (OLD.state = 'running' AND NEW.state IN ('unknown', 'completed', 'failed'))
)
BEGIN
    SELECT RAISE(ABORT, 'invalid live run outbox state transition');
END;

CREATE TABLE live_run_receipts (
    idempotency_key TEXT PRIMARY KEY CHECK (length(idempotency_key) BETWEEN 1 AND 256),
    command_id TEXT NOT NULL UNIQUE CHECK (length(command_id) BETWEEN 1 AND 128),
    run_id TEXT NOT NULL UNIQUE,
    payload_digest TEXT NOT NULL
        CHECK (length(payload_digest) = 64 AND payload_digest = lower(payload_digest)),
    accepted_revision INTEGER NOT NULL CHECK (accepted_revision >= 0),
    admission_sequence INTEGER NOT NULL CHECK (admission_sequence > 0),
    accepted_at TEXT NOT NULL,
    receipt_json TEXT NOT NULL,
    FOREIGN KEY (run_id) REFERENCES live_runs(run_id),
    FOREIGN KEY (admission_sequence) REFERENCES live_run_outbox(admission_sequence)
);

CREATE TRIGGER live_run_receipts_no_update
BEFORE UPDATE ON live_run_receipts
BEGIN
    SELECT RAISE(ABORT, 'live run command receipts are immutable');
END;

CREATE TRIGGER live_run_receipts_no_delete
BEFORE DELETE ON live_run_receipts
BEGIN
    SELECT RAISE(ABORT, 'live run command receipts are immutable');
END;

CREATE TABLE live_run_events (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT CHECK (sequence > 0),
    store_generation INTEGER NOT NULL CHECK (store_generation >= 0),
    run_id TEXT NOT NULL,
    run_revision INTEGER NOT NULL CHECK (run_revision >= 0),
    claim_generation INTEGER NOT NULL CHECK (claim_generation >= 0),
    kind TEXT NOT NULL CHECK (kind IN ('status_changed', 'text_delta', 'security_violation')),
    status TEXT CHECK (status IS NULL OR status IN ('queued', 'claimed', 'session_creation_intent', 'running', 'unknown', 'completed', 'failed')),
    text_delta TEXT,
    failure_json TEXT,
    created_at TEXT NOT NULL,
    FOREIGN KEY (run_id) REFERENCES live_runs(run_id),
    CHECK (
        (kind = 'status_changed' AND status IS NOT NULL AND text_delta IS NULL)
        OR (kind = 'text_delta' AND status IS NULL AND text_delta IS NOT NULL AND failure_json IS NULL)
        OR (kind = 'security_violation' AND status IS NULL AND text_delta IS NULL AND failure_json IS NULL)
    )
);

CREATE INDEX live_run_events_by_run_sequence
    ON live_run_events(run_id, sequence);

CREATE TRIGGER live_run_events_no_update
BEFORE UPDATE ON live_run_events
BEGIN
    SELECT RAISE(ABORT, 'live run events are append-only');
END;

CREATE TRIGGER live_run_events_no_delete
BEFORE DELETE ON live_run_events
BEGIN
    SELECT RAISE(ABORT, 'live run events are append-only');
END;
