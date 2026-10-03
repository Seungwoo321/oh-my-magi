CREATE TABLE pause_migration_sequences AS SELECT name, seq FROM sqlite_sequence WHERE name IN ('live_run_outbox', 'live_run_events');

CREATE TABLE pause_migration_live_run_outbox AS SELECT * FROM live_run_outbox;

CREATE TABLE pause_migration_live_run_events AS SELECT * FROM live_run_events;

CREATE TABLE pause_migration_live_run_receipts AS SELECT * FROM live_run_receipts;

CREATE TABLE pause_migration_live_run_cancellation_receipts AS SELECT * FROM live_run_cancellation_receipts;

DROP TABLE live_run_cancellation_receipts;

DROP TABLE live_run_receipts;

DROP TABLE live_run_events;

DROP TABLE live_run_outbox;

CREATE TABLE "live_run_outbox" (
    admission_sequence INTEGER PRIMARY KEY AUTOINCREMENT CHECK (admission_sequence > 0),
    run_id TEXT NOT NULL UNIQUE,
    state TEXT NOT NULL
        CHECK (state IN ('queued', 'claimed', 'session_creation_intent', 'running', 'paused', 'cancelling', 'unknown', 'completed', 'cancelled', 'failed')),
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

INSERT INTO live_run_outbox SELECT * FROM pause_migration_live_run_outbox;

DROP TABLE pause_migration_live_run_outbox;

CREATE TABLE live_run_events (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT CHECK (sequence > 0),
    store_generation INTEGER NOT NULL CHECK (store_generation >= 0),
    run_id TEXT NOT NULL,
    run_revision INTEGER NOT NULL CHECK (run_revision >= 0),
    claim_generation INTEGER NOT NULL CHECK (claim_generation >= 0),
    kind TEXT NOT NULL CHECK (kind IN ('status_changed', 'text_delta', 'security_violation')),
    status TEXT CHECK (status IS NULL OR status IN ('queued', 'claimed', 'session_creation_intent', 'running', 'paused', 'cancelling', 'unknown', 'completed', 'cancelled', 'failed')),
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

INSERT INTO live_run_events SELECT * FROM pause_migration_live_run_events;

DROP TABLE pause_migration_live_run_events;

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

INSERT INTO live_run_receipts SELECT * FROM pause_migration_live_run_receipts;

DROP TABLE pause_migration_live_run_receipts;

CREATE TABLE live_run_cancellation_receipts (
    idempotency_key TEXT PRIMARY KEY CHECK (length(idempotency_key) BETWEEN 1 AND 256),
    command_id TEXT NOT NULL UNIQUE CHECK (length(command_id) BETWEEN 1 AND 128),
    run_id TEXT NOT NULL,
    payload_digest TEXT NOT NULL
        CHECK (length(payload_digest) = 64 AND payload_digest = lower(payload_digest)),
    origin_status TEXT NOT NULL
        CHECK (origin_status IN ('queued', 'claimed', 'session_creation_intent', 'running', 'paused', 'cancelling', 'unknown', 'completed', 'cancelled', 'failed')),
    accepted_status TEXT NOT NULL
        CHECK (accepted_status IN ('queued', 'claimed', 'session_creation_intent', 'running', 'paused', 'cancelling', 'unknown', 'completed', 'cancelled', 'failed')),
    accepted_revision INTEGER NOT NULL CHECK (accepted_revision >= 0),
    claim_generation INTEGER NOT NULL CHECK (claim_generation >= 0),
    provider_outcome TEXT NOT NULL
        CHECK (provider_outcome IN ('not_started', 'pending', 'confirmed', 'unknown')),
    outcome_revision INTEGER NOT NULL CHECK (outcome_revision >= 0),
    event_sequence INTEGER NOT NULL CHECK (event_sequence > 0),
    accepted_at TEXT NOT NULL,
    CHECK (outcome_revision >= accepted_revision),
    CHECK (
        (origin_status IN ('queued', 'paused') AND accepted_status = 'cancelled' AND provider_outcome = 'not_started')
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

INSERT INTO live_run_cancellation_receipts SELECT * FROM pause_migration_live_run_cancellation_receipts;

DROP TABLE pause_migration_live_run_cancellation_receipts;

CREATE INDEX live_run_outbox_fifo
    ON live_run_outbox(state, admission_sequence);

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
    OR (OLD.state = 'paused' AND NEW.state IN ('cancelled', 'failed'))
    OR (OLD.state = 'claimed' AND NEW.state IN ('queued', 'session_creation_intent', 'cancelling', 'failed'))
    OR (OLD.state = 'session_creation_intent' AND NEW.state IN ('running', 'cancelling', 'unknown', 'failed'))
    OR (OLD.state = 'running' AND NEW.state IN ('cancelling', 'unknown', 'completed', 'failed', 'paused'))
    OR (OLD.state = 'cancelling' AND NEW.state IN ('cancelled', 'unknown'))
)
BEGIN
    SELECT RAISE(ABORT, 'invalid live run outbox state transition');
END;

CREATE TRIGGER live_run_receipts_no_update
BEFORE UPDATE ON live_run_receipts
BEGIN
    SELECT RAISE(ABORT, 'live run command receipts are immutable');
END;

CREATE INDEX live_run_events_by_run_sequence
    ON live_run_events(run_id, sequence);

CREATE TRIGGER live_run_events_no_update
BEFORE UPDATE ON live_run_events
BEGIN
    SELECT RAISE(ABORT, 'live run events are append-only');
END;

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

CREATE TRIGGER live_run_outbox_capacity
BEFORE INSERT ON live_run_outbox
WHEN NEW.state IN ('queued', 'claimed', 'session_creation_intent', 'running', 'cancelling', 'unknown')
    AND NOT EXISTS (
        SELECT 1 FROM live_run_dispatch_reservations
        WHERE run_id = NEW.run_id AND state IN ('reserved', 'active', 'unknown')
    )
    AND (
        (SELECT count(*) FROM live_run_dispatch_reservations WHERE state IN ('reserved', 'active', 'unknown'))
        +
        (SELECT count(*) FROM live_run_outbox o
         WHERE o.state IN ('queued', 'claimed', 'session_creation_intent', 'running', 'cancelling', 'unknown')
           AND NOT EXISTS (
               SELECT 1 FROM live_run_dispatch_reservations r
               WHERE r.run_id = o.run_id AND r.state IN ('reserved', 'active', 'unknown')
           ))
    ) >= 10
BEGIN
    SELECT RAISE(ABORT, 'live run queue is full');
END;

CREATE TRIGGER live_run_dispatch_reservations_release_terminal
AFTER UPDATE OF state ON live_run_outbox
WHEN NEW.state IN ('completed', 'failed', 'cancelled', 'unknown') AND OLD.state <> NEW.state
BEGIN
    UPDATE live_run_dispatch_reservations
    SET state = CASE WHEN NEW.state = 'unknown' THEN 'unknown' ELSE 'released' END,
        updated_at = NEW.updated_at
    WHERE run_id = NEW.run_id AND state IN ('reserved', 'active');
END;

CREATE TRIGGER live_run_receipts_no_delete BEFORE DELETE ON live_run_receipts
WHEN NOT EXISTS (
 SELECT 1 FROM run_tombstones t JOIN runs r ON r.run_id=t.run_id
 WHERE t.run_id=OLD.run_id AND t.payload_digest=r.input_digest AND r.status IN ('completed','cancelled','failed')
 AND NOT EXISTS(SELECT 1 FROM dispatch_outbox d WHERE d.run_id=t.run_id AND d.state IN ('dispatched','unknown'))
 AND (NOT EXISTS(SELECT 1 FROM live_runs l WHERE l.run_id=t.run_id) OR (SELECT count(*) FROM live_run_dispatch_reservations d WHERE d.run_id=t.run_id)=10)
 AND NOT EXISTS(SELECT 1 FROM live_run_outbox o WHERE o.run_id=t.run_id AND o.state NOT IN ('completed','cancelled','failed'))
 AND NOT EXISTS(SELECT 1 FROM live_run_dispatch_reservations d WHERE d.run_id=t.run_id AND d.state NOT IN ('settled','released'))
)
BEGIN SELECT RAISE(ABORT,'live audit record requires lawful run deletion'); END;

CREATE TRIGGER live_run_events_no_delete BEFORE DELETE ON live_run_events
WHEN NOT EXISTS (
 SELECT 1 FROM run_tombstones t JOIN runs r ON r.run_id=t.run_id
 WHERE t.run_id=OLD.run_id AND t.payload_digest=r.input_digest AND r.status IN ('completed','cancelled','failed')
 AND NOT EXISTS(SELECT 1 FROM dispatch_outbox d WHERE d.run_id=t.run_id AND d.state IN ('dispatched','unknown'))
 AND (NOT EXISTS(SELECT 1 FROM live_runs l WHERE l.run_id=t.run_id) OR (SELECT count(*) FROM live_run_dispatch_reservations d WHERE d.run_id=t.run_id)=10)
 AND NOT EXISTS(SELECT 1 FROM live_run_outbox o WHERE o.run_id=t.run_id AND o.state NOT IN ('completed','cancelled','failed'))
 AND NOT EXISTS(SELECT 1 FROM live_run_dispatch_reservations d WHERE d.run_id=t.run_id AND d.state NOT IN ('settled','released'))
)
BEGIN SELECT RAISE(ABORT,'live audit record requires lawful run deletion'); END;

CREATE TRIGGER live_run_cancellation_receipts_no_delete BEFORE DELETE ON live_run_cancellation_receipts
WHEN NOT EXISTS (
 SELECT 1 FROM run_tombstones t JOIN runs r ON r.run_id=t.run_id
 WHERE t.run_id=OLD.run_id AND t.payload_digest=r.input_digest AND r.status IN ('completed','cancelled','failed')
 AND NOT EXISTS(SELECT 1 FROM dispatch_outbox d WHERE d.run_id=t.run_id AND d.state IN ('dispatched','unknown'))
 AND (NOT EXISTS(SELECT 1 FROM live_runs l WHERE l.run_id=t.run_id) OR (SELECT count(*) FROM live_run_dispatch_reservations d WHERE d.run_id=t.run_id)=10)
 AND NOT EXISTS(SELECT 1 FROM live_run_outbox o WHERE o.run_id=t.run_id AND o.state NOT IN ('completed','cancelled','failed'))
 AND NOT EXISTS(SELECT 1 FROM live_run_dispatch_reservations d WHERE d.run_id=t.run_id AND d.state NOT IN ('settled','released'))
)
BEGIN SELECT RAISE(ABORT,'live audit record requires lawful run deletion'); END;

INSERT INTO sqlite_sequence(name,seq) SELECT old.name,old.seq FROM pause_migration_sequences old WHERE NOT EXISTS (SELECT 1 FROM sqlite_sequence current WHERE current.name=old.name);
UPDATE sqlite_sequence SET seq = max(seq, coalesce((SELECT old.seq FROM pause_migration_sequences old WHERE old.name = sqlite_sequence.name), seq)) WHERE name IN ('live_run_outbox', 'live_run_events');
DROP TABLE pause_migration_sequences;
