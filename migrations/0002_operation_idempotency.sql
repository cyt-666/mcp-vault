-- MCP Vault WP-04 operation idempotency.
--
-- WP-02's migration remains immutable. Journal rows now retain the client
-- idempotency key so a crash before metadata commit can be retried or
-- reconciled without guessing from a serialized payload.

ALTER TABLE operation_journal ADD COLUMN idempotency_key TEXT;

CREATE UNIQUE INDEX operation_journal_vault_idempotency_idx
    ON operation_journal(vault_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL;
