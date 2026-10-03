ALTER TABLE admission_request_bindings ADD COLUMN common_context_budget_json TEXT
CHECK(common_context_budget_json IS NULL OR (
 typeof(common_context_budget_json)='text'
 AND length(common_context_budget_json) BETWEEN 1 AND 4096
 AND json_valid(common_context_budget_json)
));
