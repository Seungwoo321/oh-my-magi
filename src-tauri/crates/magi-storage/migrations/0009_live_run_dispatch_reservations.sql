DROP TRIGGER live_run_outbox_capacity;

CREATE TABLE live_run_dispatch_reservations (
    run_id TEXT NOT NULL,
    slot_ordinal INTEGER NOT NULL CHECK (slot_ordinal BETWEEN 0 AND 9),
    state TEXT NOT NULL CHECK (state IN ('reserved', 'active', 'settled', 'released', 'unknown')),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (run_id, slot_ordinal),
    FOREIGN KEY (run_id) REFERENCES live_runs(run_id)
);

WITH RECURSIVE slots(slot_ordinal) AS (
    SELECT 0
    UNION ALL
    SELECT slot_ordinal + 1 FROM slots WHERE slot_ordinal < 9
)
INSERT INTO live_run_dispatch_reservations (run_id, slot_ordinal, state, created_at, updated_at)
SELECT q.run_id,
       slots.slot_ordinal,
       CASE q.state
           WHEN 'completed' THEN 'settled'
           WHEN 'failed' THEN 'released'
           WHEN 'cancelled' THEN 'released'
           WHEN 'unknown' THEN 'unknown'
           WHEN 'cancelling' THEN 'unknown'
           WHEN 'session_creation_intent' THEN CASE WHEN slots.slot_ordinal = 0 THEN 'unknown' ELSE 'reserved' END
           WHEN 'running' THEN CASE WHEN slots.slot_ordinal = 0 THEN 'unknown' ELSE 'reserved' END
           ELSE 'reserved'
       END,
       q.created_at,
       q.updated_at
FROM live_run_outbox q
JOIN runs r ON r.run_id = q.run_id AND r.deleted_at IS NULL
CROSS JOIN slots;

CREATE INDEX live_run_dispatch_reservations_state
    ON live_run_dispatch_reservations(state, run_id, slot_ordinal);

CREATE TRIGGER live_run_dispatch_reservation_capacity
BEFORE INSERT ON live_run_dispatch_reservations
WHEN NEW.state IN ('reserved', 'active', 'unknown')
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
    SELECT RAISE(ABORT, 'live run dispatch capacity is full');
END;

CREATE TRIGGER live_run_dispatch_reservation_state_guard
BEFORE UPDATE OF state ON live_run_dispatch_reservations
WHEN OLD.state <> NEW.state AND NOT (
    (OLD.state = 'reserved' AND NEW.state IN ('active', 'released', 'unknown'))
    OR (OLD.state = 'active' AND NEW.state IN ('settled', 'released', 'unknown'))
    OR (OLD.state = 'unknown' AND NEW.state IN ('settled', 'released'))
)
BEGIN
    SELECT RAISE(ABORT, 'invalid live run dispatch reservation transition');
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
