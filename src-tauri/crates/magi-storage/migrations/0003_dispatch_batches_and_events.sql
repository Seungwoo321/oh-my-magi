CREATE TABLE dispatch_batches (
    batch_key TEXT PRIMARY KEY,
    command_id TEXT UNIQUE,
    run_id TEXT NOT NULL,
    dispatch_count INTEGER NOT NULL CHECK (dispatch_count >= 0),
    dispatches_digest TEXT CHECK (dispatches_digest IS NULL OR (length(dispatches_digest) = 64 AND dispatches_digest = lower(dispatches_digest))),
    created_at TEXT NOT NULL,
    is_legacy INTEGER NOT NULL DEFAULT 0 CHECK (is_legacy IN (0, 1)),
    FOREIGN KEY (command_id) REFERENCES commands(command_id),
    FOREIGN KEY (run_id) REFERENCES runs(run_id),
    CHECK ((is_legacy = 1) OR (command_id IS NOT NULL AND dispatches_digest IS NOT NULL))
);

INSERT INTO dispatch_batches (batch_key, command_id, run_id, dispatch_count, dispatches_digest, created_at, is_legacy)
SELECT 'legacy-command:' || command_id, command_id, target_id, 0,
       '4f53cda18c2baa0c0354bb5f9a3ecbe5ed12ab4d8e11ba873c2f11161202b945', accepted_at, 1
FROM commands;

INSERT INTO dispatch_batches (batch_key, command_id, run_id, dispatch_count, dispatches_digest, created_at, is_legacy)
SELECT 'legacy-dispatch:' || dispatch_id, NULL, run_id, 1, NULL, created_at, 1
FROM dispatch_outbox;

ALTER TABLE dispatch_outbox RENAME TO dispatch_outbox_v2;

CREATE TABLE dispatch_outbox (
    dispatch_id TEXT PRIMARY KEY,
    batch_key TEXT NOT NULL,
    command_id TEXT,
    batch_ordinal INTEGER NOT NULL CHECK (batch_ordinal >= 0),
    run_id TEXT NOT NULL,
    slot_id TEXT NOT NULL,
    stage TEXT NOT NULL CHECK (stage IN ('independent_review', 'cross_review', 'synthesis', 'balloting')),
    core_id TEXT CHECK (core_id IS NULL OR core_id IN ('MELCHIOR-1', 'BALTHASAR-2', 'CASPER-3')),
    attempt_generation INTEGER NOT NULL CHECK (attempt_generation >= 0),
    store_generation INTEGER NOT NULL CHECK (store_generation >= 0),
    input_digest TEXT NOT NULL CHECK (length(input_digest) = 64 AND input_digest = lower(input_digest)),
    binding_digest TEXT NOT NULL CHECK (length(binding_digest) = 64 AND binding_digest = lower(binding_digest)),
    payload_digest TEXT NOT NULL CHECK (length(payload_digest) = 64 AND payload_digest = lower(payload_digest)),
    state TEXT NOT NULL CHECK (state IN ('prepared', 'dispatched', 'settled', 'unknown', 'aborted_before_dispatch', 'reconciled_no_effect')),
    provider_request_id TEXT,
    result_digest TEXT CHECK (result_digest IS NULL OR (length(result_digest) = 64 AND result_digest = lower(result_digest))),
    result_byte_length INTEGER CHECK (result_byte_length IS NULL OR result_byte_length >= 0),
    reconciliation_reference TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE (batch_key, batch_ordinal),
    UNIQUE (run_id, attempt_generation, stage, slot_id),
    FOREIGN KEY (batch_key) REFERENCES dispatch_batches(batch_key),
    FOREIGN KEY (command_id) REFERENCES commands(command_id),
    FOREIGN KEY (run_id) REFERENCES runs(run_id),
    CHECK ((state = 'settled' AND result_digest IS NOT NULL AND result_byte_length IS NOT NULL) OR (state <> 'settled' AND result_digest IS NULL AND result_byte_length IS NULL)),
    CHECK ((state = 'reconciled_no_effect' AND reconciliation_reference IS NOT NULL) OR (state <> 'reconciled_no_effect' AND reconciliation_reference IS NULL))
);

INSERT INTO dispatch_outbox (
    dispatch_id, batch_key, command_id, batch_ordinal, run_id, slot_id, stage, core_id,
    attempt_generation, store_generation, input_digest, binding_digest, payload_digest,
    state, provider_request_id, result_digest, result_byte_length, reconciliation_reference,
    created_at, updated_at
)
SELECT dispatch_id, 'legacy-dispatch:' || dispatch_id, NULL, 0, run_id, slot_id, stage, core_id,
       attempt_generation, 0, input_digest, binding_digest, payload_digest,
       CASE WHEN state = 'settled' THEN 'unknown' ELSE state END, provider_request_id,
       NULL, NULL, NULL, created_at, updated_at
FROM dispatch_outbox_v2;

DROP TABLE dispatch_outbox_v2;

CREATE INDEX dispatch_outbox_pending ON dispatch_outbox(state, store_generation, created_at, batch_key, batch_ordinal)
    WHERE state = 'prepared';
CREATE INDEX dispatch_outbox_by_run ON dispatch_outbox(run_id, created_at, batch_key, batch_ordinal);

CREATE TABLE dispatch_transition_events (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    store_generation INTEGER NOT NULL CHECK (store_generation >= 0),
    dispatch_id TEXT NOT NULL,
    batch_key TEXT NOT NULL,
    run_id TEXT NOT NULL,
    run_generation INTEGER NOT NULL CHECK (run_generation >= 0),
    event_kind TEXT NOT NULL CHECK (event_kind IN ('prepared', 'dispatched', 'provider_request_identified', 'settled', 'unknown', 'aborted_before_dispatch', 'reconciled_no_effect', 'late_callback_quarantined')),
    previous_state TEXT CHECK (previous_state IS NULL OR previous_state IN ('prepared', 'dispatched', 'settled', 'unknown', 'aborted_before_dispatch', 'reconciled_no_effect')),
    state TEXT NOT NULL CHECK (state IN ('prepared', 'dispatched', 'settled', 'unknown', 'aborted_before_dispatch', 'reconciled_no_effect')),
    disposition TEXT NOT NULL CHECK (disposition IN ('applied', 'reconciled', 'quarantined')),
    provider_request_id TEXT,
    result_digest TEXT CHECK (result_digest IS NULL OR (length(result_digest) = 64 AND result_digest = lower(result_digest))),
    result_byte_length INTEGER CHECK (result_byte_length IS NULL OR result_byte_length >= 0),
    reconciliation_reference TEXT,
    created_at TEXT NOT NULL,
    FOREIGN KEY (dispatch_id) REFERENCES dispatch_outbox(dispatch_id),
    FOREIGN KEY (batch_key) REFERENCES dispatch_batches(batch_key),
    FOREIGN KEY (run_id) REFERENCES runs(run_id),
    CHECK ((result_digest IS NULL AND result_byte_length IS NULL) OR (result_digest IS NOT NULL AND result_byte_length IS NOT NULL))
);

CREATE INDEX dispatch_events_by_run_sequence
    ON dispatch_transition_events(run_id, sequence);

CREATE TRIGGER dispatch_batches_no_update
BEFORE UPDATE ON dispatch_batches
BEGIN
    SELECT RAISE(ABORT, 'dispatch batch receipt is immutable');
END;

CREATE TRIGGER dispatch_batches_no_delete
BEFORE DELETE ON dispatch_batches
BEGIN
    SELECT RAISE(ABORT, 'dispatch batch receipt is immutable');
END;

CREATE TRIGGER dispatch_transition_events_no_update
BEFORE UPDATE ON dispatch_transition_events
BEGIN
    SELECT RAISE(ABORT, 'dispatch transition events are append-only');
END;

CREATE TRIGGER dispatch_transition_events_no_delete
BEFORE DELETE ON dispatch_transition_events
BEGIN
    SELECT RAISE(ABORT, 'dispatch transition events are append-only');
END;
