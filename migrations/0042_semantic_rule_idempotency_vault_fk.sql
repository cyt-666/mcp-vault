-- M3-B: idempotency records must not outlive or cross their Vault.
-- SQLite cannot add a foreign key in place, so rebuild this small operational
-- table while preserving existing rows and its composite primary key.
ALTER TABLE semantic_rule_idempotency RENAME TO semantic_rule_idempotency_0042;
CREATE TABLE semantic_rule_idempotency (
    vault_id TEXT NOT NULL,
    rule_kind TEXT NOT NULL CHECK (rule_kind IN ('correction', 'suppression')),
    idempotency_key TEXT NOT NULL,
    request_hash TEXT NOT NULL,
    result_id TEXT NOT NULL,
    rules_revision INTEGER NOT NULL CHECK (rules_revision >= 0),
    created_at INTEGER NOT NULL,
    PRIMARY KEY (vault_id, rule_kind, idempotency_key),
    FOREIGN KEY (vault_id) REFERENCES vaults(id) ON DELETE CASCADE
);
INSERT INTO semantic_rule_idempotency
    (vault_id,rule_kind,idempotency_key,request_hash,result_id,rules_revision,created_at)
SELECT vault_id,rule_kind,idempotency_key,request_hash,result_id,rules_revision,created_at
FROM semantic_rule_idempotency_0042;
DROP TABLE semantic_rule_idempotency_0042;
