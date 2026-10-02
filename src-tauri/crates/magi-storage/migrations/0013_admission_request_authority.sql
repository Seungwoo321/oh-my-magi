CREATE TABLE admission_request_bindings (
 command_id TEXT PRIMARY KEY CHECK(length(command_id) BETWEEN 1 AND 128),
 idempotency_key TEXT NOT NULL CHECK(length(idempotency_key) BETWEEN 1 AND 256),
 intent_digest TEXT NOT NULL CHECK(length(intent_digest)=64 AND intent_digest NOT GLOB '*[^0-9a-f]*'),
 accepted_at TEXT NOT NULL CHECK(length(accepted_at) BETWEEN 1 AND 64)
);
CREATE INDEX admission_request_bindings_by_key ON admission_request_bindings(idempotency_key);
CREATE TRIGGER admission_request_bindings_consistent_insert BEFORE INSERT ON admission_request_bindings
WHEN EXISTS(SELECT 1 FROM admission_request_bindings b WHERE b.idempotency_key=NEW.idempotency_key AND b.intent_digest<>NEW.intent_digest)
BEGIN SELECT RAISE(ABORT,'admission request intent conflict'); END;
CREATE TRIGGER admission_request_bindings_no_update BEFORE UPDATE ON admission_request_bindings
BEGIN SELECT RAISE(ABORT,'admission request authority is immutable'); END;
CREATE TRIGGER admission_request_bindings_no_delete BEFORE DELETE ON admission_request_bindings
BEGIN SELECT RAISE(ABORT,'admission request authority is immutable'); END;
CREATE TABLE admission_request_cancellations (
 command_id TEXT PRIMARY KEY CHECK(length(command_id) BETWEEN 1 AND 128),
 idempotency_key TEXT NOT NULL UNIQUE CHECK(length(idempotency_key) BETWEEN 1 AND 256),
 request_command_id TEXT NOT NULL REFERENCES admission_request_bindings(command_id),
 request_idempotency_key TEXT NOT NULL,
 intent_digest TEXT NOT NULL CHECK(length(intent_digest)=64 AND intent_digest NOT GLOB '*[^0-9a-f]*'),
 payload_digest TEXT NOT NULL CHECK(length(payload_digest)=64 AND payload_digest NOT GLOB '*[^0-9a-f]*'),
 accepted_at TEXT NOT NULL CHECK(length(accepted_at) BETWEEN 1 AND 64),
 admitted_run_id TEXT,
 receipt_json TEXT NOT NULL CHECK(json_valid(receipt_json))
);
CREATE INDEX admission_request_cancellations_by_request ON admission_request_cancellations(request_idempotency_key,intent_digest);
CREATE TRIGGER admission_request_cancellations_consistent_insert BEFORE INSERT ON admission_request_cancellations
WHEN NOT EXISTS(SELECT 1 FROM admission_request_bindings b WHERE b.command_id=NEW.request_command_id AND b.idempotency_key=NEW.request_idempotency_key AND b.intent_digest=NEW.intent_digest)
BEGIN SELECT RAISE(ABORT,'admission cancellation intent conflict'); END;
CREATE TRIGGER admission_request_cancellations_no_update BEFORE UPDATE ON admission_request_cancellations
BEGIN SELECT RAISE(ABORT,'admission cancellation authority is immutable'); END;
CREATE TRIGGER admission_request_cancellations_no_delete BEFORE DELETE ON admission_request_cancellations
BEGIN SELECT RAISE(ABORT,'admission cancellation authority is immutable'); END;
