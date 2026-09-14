-- An omitted expected dimension is valid configuration. Older organization
-- skipped every vector in that case, even when coverage was complete.
-- Revisit affected independent contributions once, retaining decisions, pause
-- state, formal publications, canonical content and already verified groups.
UPDATE memory_organization_items AS work
SET done = 0,
    outcome = 'pending',
    priority = 1,
    attempt_sequence = 0,
    generation = generation + 1
WHERE work.done = 1
  AND EXISTS (
    SELECT 1
    FROM memory_formal_supports support
    JOIN memory_formal_items formal
      ON formal.vault_id = support.vault_id AND formal.id = support.formal_id
    JOIN embedding_records embedding
      ON embedding.vault_id = formal.vault_id
     AND embedding.object_type = 'memory'
     AND embedding.object_id = formal.id
     AND embedding.content_hash = formal.content_hash
    JOIN embedding_vectors vector
      ON vector.vault_id = embedding.vault_id AND vector.embedding_id = embedding.id
     AND vector.dimension = embedding.dimension AND vector.norm > 0
    JOIN models model ON model.id = embedding.model_id
    WHERE support.vault_id = work.vault_id
      AND support.contribution_id = work.contribution_id
      AND json_extract(model.capability_json, '$.dimension') IS NULL
      AND embedding.model_id = COALESCE(
        (SELECT binding.model_id FROM model_bindings binding
         WHERE binding.vault_id = work.vault_id AND binding.role = 'embedding_memory'),
        (SELECT binding.model_id FROM model_bindings binding
         WHERE binding.vault_id IS NULL AND binding.role = 'embedding_memory')
      )
      AND NOT EXISTS (
        SELECT 1 FROM memory_formal_supports other_support
        WHERE other_support.vault_id = support.vault_id
          AND other_support.formal_id = support.formal_id
          AND other_support.contribution_id <> support.contribution_id
      )
  );
