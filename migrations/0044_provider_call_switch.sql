-- ADR-0045: remove LocalOnly without silently enabling outbound requests.
-- Preserve legacy settings JSON readers, but canonicalize installed values once.
-- Other Vault privacy/path settings and Provider configuration are untouched.
UPDATE vault_settings
SET value_json = CASE json_extract(value_json, '$')
        WHEN 'local_only' THEN '"disabled"'
        WHEN 'remote_allowed' THEN '"enabled"'
    END,
    revision = revision + 1,
    updated_at = CAST(strftime('%s', 'now') AS INTEGER) * 1000,
    updated_by = NULL
WHERE key = 'provider.mode'
  AND json_extract(value_json, '$') IN ('local_only', 'remote_allowed');

UPDATE system_settings
SET value_json = CASE json_extract(value_json, '$')
        WHEN 'local_only' THEN '"disabled"'
        WHEN 'remote_allowed' THEN '"enabled"'
    END,
    revision = revision + 1,
    updated_at = CAST(strftime('%s', 'now') AS INTEGER) * 1000,
    updated_by = NULL
WHERE key = 'provider.mode'
  AND json_extract(value_json, '$') IN ('local_only', 'remote_allowed');
