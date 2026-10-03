CREATE TABLE live_run_receipts_migration_backup (
    idempotency_key TEXT NOT NULL,
    command_id TEXT NOT NULL,
    run_id TEXT NOT NULL,
    payload_digest TEXT NOT NULL,
    accepted_revision INTEGER NOT NULL,
    admission_sequence INTEGER NOT NULL,
    accepted_at TEXT NOT NULL,
    receipt_json TEXT NOT NULL
);

INSERT INTO live_run_receipts_migration_backup
    (idempotency_key, command_id, run_id, payload_digest, accepted_revision,
     admission_sequence, accepted_at, receipt_json)
SELECT idempotency_key, command_id, run_id, payload_digest, accepted_revision,
       admission_sequence, accepted_at, receipt_json
FROM live_run_receipts;

CREATE TABLE live_run_events_migration_backup (
    sequence INTEGER NOT NULL,
    store_generation INTEGER NOT NULL,
    run_id TEXT NOT NULL,
    run_revision INTEGER NOT NULL,
    claim_generation INTEGER NOT NULL,
    kind TEXT NOT NULL,
    status TEXT,
    text_delta TEXT,
    failure_json TEXT,
    created_at TEXT NOT NULL
);

INSERT INTO live_run_events_migration_backup
    (sequence, store_generation, run_id, run_revision, claim_generation, kind,
     status, text_delta, failure_json, created_at)
SELECT sequence, store_generation, run_id, run_revision, claim_generation, kind,
       status, text_delta, failure_json, created_at
FROM live_run_events;

CREATE TABLE live_run_outbox_next (
    admission_sequence INTEGER PRIMARY KEY AUTOINCREMENT CHECK (admission_sequence > 0),
    run_id TEXT NOT NULL UNIQUE,
    state TEXT NOT NULL
        CHECK (state IN ('queued', 'claimed', 'session_creation_intent', 'running', 'cancelling', 'unknown', 'completed', 'cancelled', 'failed')),
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

INSERT INTO live_run_outbox_next
    (admission_sequence, run_id, state, claim_generation, claim_owner, created_at, updated_at)
SELECT admission_sequence, run_id, state, claim_generation, claim_owner, created_at, updated_at
FROM live_run_outbox;

DROP TABLE live_run_receipts;
DROP TABLE live_run_outbox;
ALTER TABLE live_run_outbox_next RENAME TO live_run_outbox;

CREATE INDEX live_run_outbox_fifo
    ON live_run_outbox(state, admission_sequence);

CREATE TRIGGER live_run_outbox_capacity
BEFORE INSERT ON live_run_outbox
WHEN NEW.state IN ('queued', 'claimed', 'session_creation_intent', 'running', 'cancelling', 'unknown')
    AND (
        SELECT count(*) FROM live_run_outbox
        WHERE state IN ('queued', 'claimed', 'session_creation_intent', 'running', 'cancelling', 'unknown')
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
    (OLD.state = 'queued' AND NEW.state IN ('claimed', 'cancelled', 'failed'))
    OR (OLD.state = 'claimed' AND NEW.state IN ('queued', 'session_creation_intent', 'cancelling', 'failed'))
    OR (OLD.state = 'session_creation_intent' AND NEW.state IN ('running', 'cancelling', 'unknown', 'failed'))
    OR (OLD.state = 'running' AND NEW.state IN ('cancelling', 'unknown', 'completed', 'failed'))
    OR (OLD.state = 'cancelling' AND NEW.state IN ('cancelled', 'unknown'))
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

INSERT INTO live_run_receipts
    (idempotency_key, command_id, run_id, payload_digest, accepted_revision,
     admission_sequence, accepted_at, receipt_json)
SELECT idempotency_key, command_id, run_id, payload_digest, accepted_revision,
       admission_sequence, accepted_at, receipt_json
FROM live_run_receipts_migration_backup;

DROP TABLE live_run_receipts_migration_backup;

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

DROP TABLE live_run_events;

CREATE TABLE live_run_events (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT CHECK (sequence > 0),
    store_generation INTEGER NOT NULL CHECK (store_generation >= 0),
    run_id TEXT NOT NULL,
    run_revision INTEGER NOT NULL CHECK (run_revision >= 0),
    claim_generation INTEGER NOT NULL CHECK (claim_generation >= 0),
    kind TEXT NOT NULL CHECK (kind IN ('status_changed', 'text_delta', 'security_violation')),
    status TEXT CHECK (status IS NULL OR status IN ('queued', 'claimed', 'session_creation_intent', 'running', 'cancelling', 'unknown', 'completed', 'cancelled', 'failed')),
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

INSERT INTO live_run_events
    (sequence, store_generation, run_id, run_revision, claim_generation, kind,
     status, text_delta, failure_json, created_at)
SELECT sequence, store_generation, run_id, run_revision, claim_generation, kind,
       status, text_delta, failure_json, created_at
FROM live_run_events_migration_backup;

DROP TABLE live_run_events_migration_backup;

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

CREATE TABLE live_run_cancellation_receipts (
    idempotency_key TEXT PRIMARY KEY CHECK (length(idempotency_key) BETWEEN 1 AND 256),
    command_id TEXT NOT NULL UNIQUE CHECK (length(command_id) BETWEEN 1 AND 128),
    run_id TEXT NOT NULL,
    payload_digest TEXT NOT NULL
        CHECK (length(payload_digest) = 64 AND payload_digest = lower(payload_digest)),
    origin_status TEXT NOT NULL
        CHECK (origin_status IN ('queued', 'claimed', 'session_creation_intent', 'running', 'cancelling', 'unknown', 'completed', 'cancelled', 'failed')),
    accepted_status TEXT NOT NULL
        CHECK (accepted_status IN ('queued', 'claimed', 'session_creation_intent', 'running', 'cancelling', 'unknown', 'completed', 'cancelled', 'failed')),
    accepted_revision INTEGER NOT NULL CHECK (accepted_revision >= 0),
    claim_generation INTEGER NOT NULL CHECK (claim_generation >= 0),
    provider_outcome TEXT NOT NULL
        CHECK (provider_outcome IN ('not_started', 'pending', 'confirmed', 'unknown')),
    outcome_revision INTEGER NOT NULL CHECK (outcome_revision >= 0),
    event_sequence INTEGER NOT NULL CHECK (event_sequence > 0),
    accepted_at TEXT NOT NULL,
    CHECK (outcome_revision >= accepted_revision),
    CHECK (
        (origin_status = 'queued' AND accepted_status = 'cancelled' AND provider_outcome = 'not_started')
        OR (origin_status IN ('claimed', 'session_creation_intent')
            AND accepted_status = 'cancelling'
            AND provider_outcome IN ('not_started', 'pending', 'unknown'))
        OR (origin_status IN ('running', 'cancelling')
            AND accepted_status = 'cancelling'
            AND provider_outcome IN ('pending', 'confirmed', 'unknown'))
    ),
    FOREIGN KEY (run_id) REFERENCES live_runs(run_id),
    FOREIGN KEY (event_sequence) REFERENCES live_run_events(sequence)
);

CREATE TRIGGER live_run_cancellation_receipts_event_guard_insert
BEFORE INSERT ON live_run_cancellation_receipts
WHEN (NEW.origin_status IN ('claimed', 'session_creation_intent', 'running', 'cancelling')
        AND NEW.provider_outcome <> 'pending')
    OR NOT EXISTS (
    SELECT 1 FROM live_run_events e
    JOIN live_run_outbox q ON q.run_id = NEW.run_id
    WHERE e.sequence = NEW.event_sequence
        AND e.run_id = NEW.run_id
        AND e.run_revision = NEW.outcome_revision
        AND e.claim_generation >= NEW.claim_generation
        AND e.kind = 'status_changed'
        AND e.status = CASE NEW.provider_outcome
            WHEN 'not_started' THEN 'cancelled'
            WHEN 'pending' THEN 'cancelling'
            WHEN 'confirmed' THEN 'cancelled'
            WHEN 'unknown' THEN 'unknown'
        END
        AND q.state = CASE NEW.provider_outcome
            WHEN 'not_started' THEN 'cancelled'
            WHEN 'pending' THEN 'cancelling'
            WHEN 'confirmed' THEN 'cancelled'
            WHEN 'unknown' THEN 'unknown'
        END
)
BEGIN
    SELECT RAISE(ABORT, 'live run cancellation receipt does not match its durable event');
END;

CREATE TRIGGER live_run_cancellation_receipts_update_guard
BEFORE UPDATE ON live_run_cancellation_receipts
WHEN NEW.idempotency_key <> OLD.idempotency_key
    OR NEW.command_id <> OLD.command_id
    OR NEW.run_id <> OLD.run_id
    OR NEW.payload_digest <> OLD.payload_digest
    OR NEW.origin_status <> OLD.origin_status
    OR NEW.accepted_status <> OLD.accepted_status
    OR NEW.accepted_revision <> OLD.accepted_revision
    OR NEW.claim_generation <> OLD.claim_generation
    OR NEW.accepted_at <> OLD.accepted_at
    OR NOT (
        (OLD.provider_outcome = NEW.provider_outcome
            AND OLD.outcome_revision = NEW.outcome_revision
            AND OLD.event_sequence = NEW.event_sequence)
        OR (OLD.provider_outcome = 'pending'
            AND NEW.provider_outcome = 'confirmed'
            AND OLD.origin_status IN ('session_creation_intent', 'running', 'cancelling')
            AND NEW.outcome_revision > OLD.outcome_revision
            AND NEW.event_sequence > OLD.event_sequence)
        OR (OLD.provider_outcome = 'pending'
            AND NEW.provider_outcome = 'unknown'
            AND NEW.outcome_revision > OLD.outcome_revision
            AND NEW.event_sequence > OLD.event_sequence)
        OR (OLD.provider_outcome = 'pending'
            AND NEW.provider_outcome = 'not_started'
            AND OLD.origin_status IN ('claimed', 'session_creation_intent')
            AND NEW.outcome_revision > OLD.outcome_revision
            AND NEW.event_sequence > OLD.event_sequence)
    )
    OR NOT EXISTS (
        SELECT 1 FROM live_run_events e
        JOIN live_run_outbox q ON q.run_id = NEW.run_id
        WHERE e.sequence = NEW.event_sequence
            AND e.run_id = NEW.run_id
            AND e.run_revision = NEW.outcome_revision
            AND e.claim_generation >= NEW.claim_generation
            AND e.kind = 'status_changed'
            AND e.status = CASE NEW.provider_outcome
                WHEN 'not_started' THEN 'cancelled'
                WHEN 'pending' THEN 'cancelling'
                WHEN 'confirmed' THEN 'cancelled'
                WHEN 'unknown' THEN 'unknown'
            END
            AND q.state = CASE NEW.provider_outcome
                WHEN 'not_started' THEN 'cancelled'
                WHEN 'pending' THEN 'cancelling'
                WHEN 'confirmed' THEN 'cancelled'
                WHEN 'unknown' THEN 'unknown'
            END
    )
BEGIN
    SELECT RAISE(ABORT, 'live run cancellation receipt identity is immutable');
END;

CREATE TRIGGER live_run_cancellation_receipts_no_delete
BEFORE DELETE ON live_run_cancellation_receipts
BEGIN
    SELECT RAISE(ABORT, 'live run cancellation receipts are immutable');
END;
