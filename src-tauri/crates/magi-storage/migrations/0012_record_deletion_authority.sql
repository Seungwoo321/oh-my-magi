CREATE TABLE run_command_tombstones (
    command_kind TEXT NOT NULL,
    command_id TEXT NOT NULL,
    target_id TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    payload_digest TEXT NOT NULL CHECK (length(payload_digest)=64 AND payload_digest NOT GLOB '*[^0-9a-f]*'),
    run_id TEXT NOT NULL REFERENCES run_tombstones(run_id),
    deleted_at TEXT NOT NULL,
    PRIMARY KEY (command_kind,command_id),
    UNIQUE (command_kind,target_id,idempotency_key)
);
CREATE TRIGGER run_command_tombstones_no_update BEFORE UPDATE ON run_command_tombstones
BEGIN SELECT RAISE(ABORT,'deleted command authority is immutable'); END;
CREATE TRIGGER run_command_tombstones_no_delete BEFORE DELETE ON run_command_tombstones
BEGIN SELECT RAISE(ABORT,'deleted command authority is immutable'); END;
DROP TRIGGER live_run_receipts_no_delete;
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
DROP TRIGGER live_run_events_no_delete;
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
DROP TRIGGER live_run_cancellation_receipts_no_delete;
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
CREATE TRIGGER run_tombstones_no_update BEFORE UPDATE ON run_tombstones
BEGIN SELECT RAISE(ABORT,'deleted run authority is immutable'); END;
CREATE TRIGGER run_tombstones_no_delete BEFORE DELETE ON run_tombstones
BEGIN SELECT RAISE(ABORT,'deleted run authority is immutable'); END;

CREATE TABLE run_capture_manifest_tombstones (
 run_id TEXT NOT NULL REFERENCES run_tombstones(run_id),
 manifest_id TEXT NOT NULL,
 manifest_digest TEXT NOT NULL CHECK(length(manifest_digest)=64 AND manifest_digest NOT GLOB '*[^0-9a-f]*'),
 PRIMARY KEY(run_id,manifest_id)
);
CREATE TRIGGER run_capture_manifest_tombstones_no_update BEFORE UPDATE ON run_capture_manifest_tombstones
BEGIN SELECT RAISE(ABORT,'deleted capture authority is immutable'); END;
CREATE TRIGGER run_capture_manifest_tombstones_no_delete BEFORE DELETE ON run_capture_manifest_tombstones
BEGIN SELECT RAISE(ABORT,'deleted capture authority is immutable'); END;
DROP TRIGGER source_capture_manifests_no_delete;
CREATE TRIGGER source_capture_manifests_no_delete BEFORE DELETE ON source_capture_manifests
WHEN NOT EXISTS (
 SELECT 1 FROM run_capture_manifest_tombstones c JOIN run_tombstones t USING(run_id) JOIN runs r USING(run_id)
 WHERE c.manifest_id=OLD.manifest_id AND c.manifest_digest=OLD.manifest_digest
 AND r.deleted_at=t.deleted_at AND r.input_digest=t.payload_digest AND r.status IN ('completed','cancelled','failed')
 AND NOT EXISTS(SELECT 1 FROM live_runs l WHERE l.run_id=r.run_id)
 AND r.run_json='{}' AND r.input_json='{}' AND r.question_text='' AND r.tally_json IS NULL
 AND NOT EXISTS(SELECT 1 FROM context_drafts d WHERE d.manifest_id=OLD.manifest_id)
 AND NOT EXISTS(SELECT 1 FROM immutable_snapshots i WHERE i.object_kind='run_context' AND i.object_id=OLD.manifest_id)
 AND NOT EXISTS(SELECT 1 FROM runs a WHERE a.deleted_at IS NULL AND json_extract(a.input_json,'$.context_manifest.manifest_id')=OLD.manifest_id)
)
BEGIN SELECT RAISE(ABORT,'capture metadata requires lawful unreferenced run deletion'); END;
