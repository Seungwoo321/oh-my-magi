CREATE TABLE provider_model_selections (
    provider_profile_id TEXT PRIMARY KEY REFERENCES provider_profile_heads(provider_profile_id),
    profile_revision INTEGER NOT NULL CHECK (profile_revision >= 0),
    catalog_snapshot_id TEXT NOT NULL REFERENCES provider_catalog_snapshots(catalog_snapshot_id),
    selection_revision INTEGER NOT NULL CHECK (selection_revision >= 0),
    payload_json TEXT NOT NULL
);
CREATE TABLE core_model_selections (
    core_id TEXT PRIMARY KEY CHECK (core_id IN ('MELCHIOR-1', 'BALTHASAR-2', 'CASPER-3')),
    provider_profile_id TEXT NOT NULL REFERENCES provider_model_selections(provider_profile_id),
    profile_revision INTEGER NOT NULL CHECK (profile_revision >= 0),
    model_selection_revision INTEGER NOT NULL CHECK (model_selection_revision >= 0),
    selection_revision INTEGER NOT NULL CHECK (selection_revision >= 0),
    payload_json TEXT NOT NULL
);
