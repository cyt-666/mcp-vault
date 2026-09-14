-- Safe, Vault-scoped diagnostics for an interrupted Admin initialization.
ALTER TABLE memory_initialization_tasks ADD COLUMN error_stage TEXT;
ALTER TABLE memory_initialization_tasks ADD COLUMN error_path TEXT;
ALTER TABLE memory_initialization_tasks ADD COLUMN error_source_code TEXT;
