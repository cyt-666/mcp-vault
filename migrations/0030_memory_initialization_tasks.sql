-- Durable Admin scheduling state for source-preserving memory initialization.
CREATE TABLE memory_initialization_tasks (
    vault_id TEXT PRIMARY KEY REFERENCES vaults(id) ON DELETE CASCADE,
    task_id TEXT NOT NULL UNIQUE,
    state TEXT NOT NULL CHECK (state IN ('queued', 'running', 'failed', 'ready')),
    requested_at INTEGER NOT NULL,
    started_at INTEGER,
    finished_at INTEGER,
    completed_files INTEGER NOT NULL DEFAULT 0 CHECK (completed_files >= 0),
    total_files INTEGER NOT NULL DEFAULT 0 CHECK (total_files >= 0),
    error_code TEXT,
    resumable INTEGER NOT NULL DEFAULT 1 CHECK (resumable IN (0, 1)),
    maintenance_previous_mode TEXT NOT NULL DEFAULT 'normal'
);

CREATE INDEX memory_initialization_tasks_state
    ON memory_initialization_tasks(state, requested_at);
