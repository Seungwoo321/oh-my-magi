CREATE TEMP TABLE magi_migration_0007_sequence (seq INTEGER NOT NULL);

INSERT INTO magi_migration_0007_sequence (seq)
SELECT COALESCE(
    (SELECT seq FROM sqlite_sequence WHERE name = 'source_freshness_observations'),
    (SELECT MAX(observation_id) FROM source_freshness_observations),
    0
);

CREATE TABLE source_freshness_observations_v7 (
    observation_id INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id TEXT NOT NULL,
    source_id TEXT NOT NULL,
    captured_digest TEXT NOT NULL CHECK (length(captured_digest) = 64 AND captured_digest = lower(captured_digest)),
    observed_digest TEXT CHECK (observed_digest IS NULL OR (length(observed_digest) = 64 AND observed_digest = lower(observed_digest))),
    status TEXT NOT NULL CHECK (status IN ('unchanged', 'changed', 'missing', 'unreadable', 'unchecked')),
    observed_at_epoch_ms INTEGER NOT NULL CHECK (observed_at_epoch_ms >= 0),
    FOREIGN KEY (run_id) REFERENCES runs(run_id),
    CHECK (
        (status = 'unchanged' AND observed_digest IS NOT NULL AND observed_digest = captured_digest) OR
        (status = 'changed' AND observed_digest IS NOT NULL AND observed_digest <> captured_digest) OR
        (status IN ('missing', 'unreadable', 'unchecked') AND observed_digest IS NULL)
    )
);

INSERT INTO source_freshness_observations_v7 (
    observation_id,
    run_id,
    source_id,
    captured_digest,
    observed_digest,
    status,
    observed_at_epoch_ms
)
SELECT
    observation_id,
    run_id,
    source_id,
    captured_digest,
    observed_digest,
    status,
    observed_at_epoch_ms
FROM source_freshness_observations;

DROP TABLE source_freshness_observations;
ALTER TABLE source_freshness_observations_v7 RENAME TO source_freshness_observations;

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

UPDATE sqlite_sequence
SET seq = MAX(seq, (SELECT seq FROM magi_migration_0007_sequence))
WHERE name = 'source_freshness_observations';

INSERT INTO sqlite_sequence (name, seq)
SELECT 'source_freshness_observations', seq
FROM magi_migration_0007_sequence
WHERE seq > 0
  AND NOT EXISTS (
      SELECT 1 FROM sqlite_sequence WHERE name = 'source_freshness_observations'
  );

DROP TABLE magi_migration_0007_sequence;
