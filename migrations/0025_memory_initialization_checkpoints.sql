-- Forward-only addition for installations already running migration 0024.
-- Preserve existing jobs, initialized state and prepared canonical operations.
ALTER TABLE memory_organization_state ADD COLUMN reset_cursor TEXT;
ALTER TABLE memory_organization_state ADD COLUMN reset_complete INTEGER NOT NULL DEFAULT 0 CHECK(reset_complete IN (0,1));
ALTER TABLE memory_organization_state ADD COLUMN adoption_cursor TEXT;
ALTER TABLE memory_organization_state ADD COLUMN reset_checked INTEGER NOT NULL DEFAULT 0;
ALTER TABLE memory_organization_state ADD COLUMN adoption_checked INTEGER NOT NULL DEFAULT 0;
