DROP INDEX one_live_run_per_conversation;
CREATE UNIQUE INDEX one_live_run_per_conversation ON runs(conversation_id)
WHERE deleted_at IS NULL AND status NOT IN ('completed','cancelled','failed')
AND NOT (status='paused'
 AND coalesce(json_extract(run_json,'$.status.status'),'')='paused'
 AND coalesce(json_extract(run_json,'$.status.reason'),'')='needs_input');

CREATE TABLE clarification_drafts (
    draft_id TEXT PRIMARY KEY REFERENCES context_drafts(draft_id),
    parent_run_id TEXT NOT NULL REFERENCES runs(run_id),
    parent_revision INTEGER NOT NULL CHECK(parent_revision >= 0),
    parent_generation INTEGER NOT NULL CHECK(parent_generation >= 0),
    parent_input_digest TEXT NOT NULL CHECK(length(parent_input_digest)=64 AND parent_input_digest=lower(parent_input_digest)),
    lineage_id TEXT NOT NULL REFERENCES admission_execution_lineages(lineage_id),
    store_generation INTEGER NOT NULL CHECK(store_generation > 0),
    question_text TEXT NOT NULL CHECK(length(CAST(question_text AS BLOB)) BETWEEN 1 AND 32000)
);
CREATE TRIGGER clarification_draft_parent_immutable BEFORE UPDATE OF draft_id,parent_run_id,parent_revision,parent_generation,parent_input_digest,lineage_id,store_generation ON clarification_drafts BEGIN SELECT RAISE(ABORT,'immutable clarification parent authority'); END;
CREATE TABLE clarification_admission_intents (
    command_id TEXT PRIMARY KEY REFERENCES admission_request_bindings(command_id),
    parent_run_id TEXT NOT NULL,
    parent_revision INTEGER NOT NULL CHECK(parent_revision >= 0),
    parent_generation INTEGER NOT NULL CHECK(parent_generation >= 0),
    parent_input_digest TEXT NOT NULL CHECK(length(parent_input_digest)=64 AND parent_input_digest=lower(parent_input_digest)),
    draft_id TEXT NOT NULL,
    draft_revision INTEGER NOT NULL CHECK(draft_revision >= 0),
    manifest_digest TEXT NOT NULL CHECK(length(manifest_digest)=64 AND manifest_digest=lower(manifest_digest)),
    base_intent_digest TEXT NOT NULL CHECK(length(base_intent_digest)=64 AND base_intent_digest=lower(base_intent_digest))
);
CREATE TRIGGER clarification_intent_no_update BEFORE UPDATE ON clarification_admission_intents BEGIN SELECT RAISE(ABORT,'immutable clarification admission intent'); END;
CREATE TRIGGER clarification_intent_no_delete BEFORE DELETE ON clarification_admission_intents BEGIN SELECT RAISE(ABORT,'clarification cancellation authority is retained'); END;

CREATE TABLE clarification_capture_owners (
 manifest_id TEXT NOT NULL,
 draft_id TEXT NOT NULL,
 parent_run_id TEXT NOT NULL,
 manifest_digest TEXT NOT NULL CHECK(length(manifest_digest)=64 AND manifest_digest NOT GLOB '*[^0-9a-f]*'),
 PRIMARY KEY(manifest_id,draft_id)
);
CREATE TRIGGER clarification_capture_owner_no_update BEFORE UPDATE ON clarification_capture_owners BEGIN SELECT RAISE(ABORT,'immutable clarification capture owner'); END;
CREATE TRIGGER clarification_capture_owner_no_delete BEFORE DELETE ON clarification_capture_owners BEGIN SELECT RAISE(ABORT,'clarification capture retirement authority is retained'); END;
CREATE TABLE clarification_capture_retirements (
 manifest_id TEXT NOT NULL,
 draft_id TEXT NOT NULL,
 manifest_digest TEXT NOT NULL CHECK(length(manifest_digest)=64 AND manifest_digest NOT GLOB '*[^0-9a-f]*'),
 retired_at TEXT NOT NULL,
 PRIMARY KEY(manifest_id,draft_id),
 FOREIGN KEY(manifest_id,draft_id) REFERENCES clarification_capture_owners(manifest_id,draft_id)
);
CREATE TRIGGER clarification_capture_retirement_no_update BEFORE UPDATE ON clarification_capture_retirements BEGIN SELECT RAISE(ABORT,'immutable clarification capture retirement'); END;
CREATE TRIGGER clarification_capture_retirement_no_delete BEFORE DELETE ON clarification_capture_retirements BEGIN SELECT RAISE(ABORT,'clarification capture retirement authority is retained'); END;

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
AND NOT EXISTS (
 SELECT 1 FROM clarification_capture_retirements t JOIN clarification_capture_owners o USING(manifest_id,draft_id)
 WHERE t.manifest_id=OLD.manifest_id AND t.manifest_digest=OLD.manifest_digest AND o.manifest_digest=t.manifest_digest
 AND NOT EXISTS(SELECT 1 FROM clarification_drafts c WHERE c.draft_id=t.draft_id)
 AND NOT EXISTS(SELECT 1 FROM context_drafts d WHERE d.manifest_id=OLD.manifest_id)
 AND NOT EXISTS(SELECT 1 FROM immutable_snapshots i WHERE i.object_kind='run_context' AND i.object_id=OLD.manifest_id)
 AND NOT EXISTS(SELECT 1 FROM runs r WHERE r.deleted_at IS NULL AND json_extract(r.input_json,'$.context_manifest.manifest_id')=OLD.manifest_id)
)
BEGIN SELECT RAISE(ABORT,'capture metadata requires lawful unreferenced owner retirement'); END;
