-- Existing directory/topic membership changes invalidate derived navigation.
-- This adds no user knowledge and leaves historical migration checksums intact.
CREATE TRIGGER memory_units_navigation_membership_insert AFTER INSERT ON index_memberships
WHEN EXISTS(SELECT 1 FROM memory_unit_sources s WHERE s.vault_id=NEW.vault_id AND s.note_file_id=NEW.file_id)
BEGIN
    INSERT INTO memory_unit_runtime(vault_id,generation) VALUES(NEW.vault_id,1)
    ON CONFLICT(vault_id) DO UPDATE SET generation=generation+1;
END;
CREATE TRIGGER memory_units_navigation_membership_delete AFTER DELETE ON index_memberships
WHEN EXISTS(SELECT 1 FROM memory_unit_sources s WHERE s.vault_id=OLD.vault_id AND s.note_file_id=OLD.file_id)
BEGIN
    INSERT INTO memory_unit_runtime(vault_id,generation) VALUES(OLD.vault_id,1)
    ON CONFLICT(vault_id) DO UPDATE SET generation=generation+1;
END;
CREATE TRIGGER memory_units_navigation_membership_update AFTER UPDATE ON index_memberships
WHEN EXISTS(SELECT 1 FROM memory_unit_sources s WHERE s.vault_id=NEW.vault_id AND s.note_file_id=NEW.file_id)
BEGIN
    INSERT INTO memory_unit_runtime(vault_id,generation) VALUES(NEW.vault_id,1)
    ON CONFLICT(vault_id) DO UPDATE SET generation=generation+1;
END;
CREATE TRIGGER memory_units_navigation_node_update AFTER UPDATE OF title,stable_key,source_ref,node_type ON index_nodes
WHEN EXISTS(SELECT 1 FROM index_memberships m JOIN memory_unit_sources s ON s.vault_id=m.vault_id AND s.note_file_id=m.file_id WHERE m.vault_id=NEW.vault_id AND m.node_id=NEW.id)
BEGIN
    INSERT INTO memory_unit_runtime(vault_id,generation) VALUES(NEW.vault_id,1)
    ON CONFLICT(vault_id) DO UPDATE SET generation=generation+1;
END;
-- Schedule the new scoped pass once. Matching global page caches remain reusable.
UPDATE memory_unit_runtime SET overview_generation=-1;
