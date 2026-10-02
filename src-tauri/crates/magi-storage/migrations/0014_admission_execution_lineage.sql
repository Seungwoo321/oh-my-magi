CREATE TABLE admission_execution_lineages (
    lineage_id TEXT PRIMARY KEY CHECK(length(lineage_id)=32),
    restored INTEGER NOT NULL CHECK(restored IN (0,1))
);
CREATE TABLE admission_binding_lineages (
    command_id TEXT PRIMARY KEY REFERENCES admission_request_bindings(command_id),
    lineage_id TEXT NOT NULL REFERENCES admission_execution_lineages(lineage_id)
);
CREATE TABLE live_execution_lineages (
    run_id TEXT PRIMARY KEY,
    lineage_id TEXT NOT NULL REFERENCES admission_execution_lineages(lineage_id)
);
INSERT INTO admission_execution_lineages VALUES(lower(hex(randomblob(16))),0);
INSERT INTO store_meta(key,value) SELECT 'admission_lineage_id',lineage_id FROM admission_execution_lineages;
INSERT INTO store_meta(key,value) VALUES('admission_activation_state','active');
INSERT INTO admission_binding_lineages SELECT command_id,(SELECT value FROM store_meta WHERE key='admission_lineage_id') FROM admission_request_bindings;
INSERT INTO live_execution_lineages SELECT run_id,(SELECT value FROM store_meta WHERE key='admission_lineage_id') FROM live_runs;
CREATE TRIGGER admission_binding_lineage_insert AFTER INSERT ON admission_request_bindings BEGIN
    INSERT INTO admission_binding_lineages VALUES(NEW.command_id,(SELECT value FROM store_meta WHERE key='admission_lineage_id'));
END;
CREATE TRIGGER live_execution_lineage_insert AFTER INSERT ON live_runs BEGIN
    INSERT INTO live_execution_lineages VALUES(NEW.run_id,(SELECT value FROM store_meta WHERE key='admission_lineage_id'));
END;
CREATE TRIGGER live_execution_requires_active BEFORE INSERT ON live_runs WHEN (SELECT value FROM store_meta WHERE key='admission_activation_state') <> 'active' BEGIN SELECT RAISE(ABORT,'inactive execution lineage'); END;
CREATE TRIGGER admission_binding_requires_active BEFORE INSERT ON admission_request_bindings WHEN (SELECT value FROM store_meta WHERE key='admission_activation_state') <> 'active' BEGIN SELECT RAISE(ABORT,'inactive execution lineage'); END;
CREATE TRIGGER admission_lineages_no_update BEFORE UPDATE ON admission_execution_lineages BEGIN SELECT RAISE(ABORT,'immutable execution lineage'); END;
CREATE TRIGGER admission_lineages_no_delete BEFORE DELETE ON admission_execution_lineages BEGIN SELECT RAISE(ABORT,'immutable execution lineage'); END;
CREATE TRIGGER admission_binding_lineages_no_update BEFORE UPDATE ON admission_binding_lineages BEGIN SELECT RAISE(ABORT,'immutable binding lineage'); END;
CREATE TRIGGER admission_binding_lineages_no_delete BEFORE DELETE ON admission_binding_lineages BEGIN SELECT RAISE(ABORT,'immutable binding lineage'); END;
CREATE TRIGGER live_execution_lineages_no_update BEFORE UPDATE ON live_execution_lineages BEGIN SELECT RAISE(ABORT,'immutable run lineage'); END;
CREATE TRIGGER live_execution_lineages_no_delete BEFORE DELETE ON live_execution_lineages BEGIN SELECT RAISE(ABORT,'immutable run lineage'); END;
