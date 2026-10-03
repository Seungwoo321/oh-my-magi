CREATE TABLE execution_clock_phase_events (
 sequence INTEGER PRIMARY KEY AUTOINCREMENT,
 run_id TEXT NOT NULL REFERENCES runs(run_id),
 store_generation INTEGER NOT NULL CHECK(store_generation >= 0),
 claim_generation INTEGER NOT NULL CHECK(claim_generation >= 0),
 phase_token TEXT NOT NULL CHECK(length(phase_token)=32),
 kind TEXT NOT NULL CHECK(kind IN ('authentication_wait_started','authenticated','authentication_failed')),
 created_at TEXT NOT NULL CHECK(length(trim(created_at)) > 0),
 UNIQUE(run_id,phase_token,kind)
);
CREATE INDEX execution_clock_phase_run ON execution_clock_phase_events(run_id,sequence);
CREATE TRIGGER execution_clock_phase_no_update BEFORE UPDATE ON execution_clock_phase_events
BEGIN SELECT RAISE(ABORT,'execution clock evidence is immutable'); END;
CREATE TRIGGER execution_clock_phase_no_delete BEFORE DELETE ON execution_clock_phase_events
WHEN NOT EXISTS(SELECT 1 FROM run_tombstones WHERE run_id=OLD.run_id)
BEGIN SELECT RAISE(ABORT,'execution clock evidence is append-only'); END;
