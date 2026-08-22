//! Vault-scoped, redacted audit-log queries for control-plane diagnostics.

use mcp_vault_domain::{Actor, ActorType, EventId, SourcePlane, VaultContext, VaultId};
use serde_json::Value;
use sqlx::{FromRow, QueryBuilder, Sqlite, SqlitePool};

use crate::StateError;

/// One append-only audit fact with redacted metadata.
#[derive(Clone, Debug, PartialEq)]
pub struct AuditRecord {
    /// Stable audit identity.
    pub id: EventId,
    /// UTC Unix millisecond timestamp.
    pub occurred_at: i64,
    /// Request correlation identifier, when supplied.
    pub request_id: Option<String>,
    /// Owning Vault; global configuration actions may be null.
    pub vault_id: Option<VaultId>,
    /// Security plane label.
    pub plane: String,
    /// Non-secret actor category.
    pub actor_type: String,
    /// Non-secret actor identity.
    pub actor_id: Option<String>,
    /// Stable action label.
    pub action: String,
    /// Target category and identity.
    pub target_type: Option<String>,
    pub target_id: Option<String>,
    /// Path hash only; never a raw path.
    pub target_path_hash: Option<String>,
    /// Success/failure result.
    pub result: String,
    /// Redacted structured metadata.
    pub metadata: Value,
}

#[derive(Debug, FromRow)]
struct AuditRow {
    id: String,
    occurred_at: i64,
    request_id: Option<String>,
    vault_id: Option<String>,
    plane: String,
    actor_type: String,
    actor_id: Option<String>,
    action: String,
    target_type: Option<String>,
    target_id: Option<String>,
    target_path_hash: Option<String>,
    result: String,
    metadata_json: String,
}

/// SQL boundary for read-only audit diagnostics.
#[derive(Clone)]
pub struct AuditRepository {
    pool: SqlitePool,
}

impl AuditRepository {
    pub(crate) fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Append one bounded, already-redacted audit fact.
    #[allow(clippy::too_many_arguments)]
    pub async fn append(
        &self,
        context: Option<&VaultContext>,
        request_id: Option<&str>,
        source_plane: SourcePlane,
        actor: &Actor,
        action: &str,
        target_type: Option<&str>,
        target_id: Option<&str>,
        result: &str,
        metadata: &Value,
    ) -> Result<EventId, StateError> {
        if action.is_empty()
            || action.len() > 128
            || result.is_empty()
            || result.len() > 64
            || serde_json::to_vec(metadata)?.len() > 16 * 1024
        {
            return Err(StateError::InvalidInput("audit fact is invalid"));
        }
        if let Some(context) = context {
            let root = context
                .content_root()
                .to_str()
                .ok_or(StateError::InvalidInput("Vault root must be valid UTF-8"))?;
            let exists: i64 = sqlx::query_scalar(
                "SELECT EXISTS(
                     SELECT 1 FROM vaults WHERE id = ? AND slug = ? AND content_root = ?
                 )",
            )
            .bind(context.id().to_string())
            .bind(context.slug().as_str())
            .bind(root)
            .fetch_one(&self.pool)
            .await?;
            if exists == 0 {
                return Err(StateError::InvalidInput("Vault context is not registered"));
            }
        }
        let id = EventId::new();
        sqlx::query(
            "INSERT INTO audit_log
             (id, occurred_at, request_id, vault_id, plane, actor_type, actor_id,
              action, target_type, target_id, target_path_hash, result, metadata_json)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, ?, ?)",
        )
        .bind(id.to_string())
        .bind(crate::now_millis()?)
        .bind(request_id)
        .bind(context.map(|context| context.id().to_string()))
        .bind(source_plane.as_str())
        .bind(actor_type_label(actor.actor_type()))
        .bind(actor.actor_id().map(|id| id.as_str()))
        .bind(action)
        .bind(target_type)
        .bind(target_id)
        .bind(result)
        .bind(serde_json::to_string(metadata)?)
        .execute(&self.pool)
        .await?;
        Ok(id)
    }

    /// List current-Vault and global audit facts with bounded filters.
    pub async fn list_for_vault(
        &self,
        context: &VaultContext,
        action: Option<&str>,
        result: Option<&str>,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<AuditRecord>, StateError> {
        let root = context
            .content_root()
            .to_str()
            .ok_or(StateError::InvalidInput("Vault root must be valid UTF-8"))?;
        let exists: i64 = sqlx::query_scalar(
            "SELECT EXISTS(
                 SELECT 1 FROM vaults WHERE id = ? AND slug = ? AND content_root = ?
             )",
        )
        .bind(context.id().to_string())
        .bind(context.slug().as_str())
        .bind(root)
        .fetch_one(&self.pool)
        .await?;
        if exists == 0 {
            return Err(StateError::InvalidInput("Vault context is not registered"));
        }
        if limit == 0 || limit > 200 || offset > 1_000_000 {
            return Err(StateError::InvalidInput("audit page is invalid"));
        }
        let mut query = QueryBuilder::<Sqlite>::new(
            "SELECT id, occurred_at, request_id, vault_id, plane, actor_type,
                    actor_id, action, target_type, target_id, target_path_hash,
                    result, metadata_json
             FROM audit_log
             WHERE (vault_id = ",
        );
        query.push_bind(context.id().to_string());
        query.push(" OR vault_id IS NULL)");
        if let Some(action) = action {
            query.push(" AND action = ");
            query.push_bind(action);
        }
        if let Some(result) = result {
            query.push(" AND result = ");
            query.push_bind(result);
        }
        query.push(" ORDER BY occurred_at DESC, id DESC LIMIT ");
        query.push_bind(i64::from(limit));
        query.push(" OFFSET ");
        query.push_bind(i64::from(offset));
        query
            .build_query_as::<AuditRow>()
            .fetch_all(&self.pool)
            .await?
            .into_iter()
            .map(row_to_audit)
            .collect()
    }
}

fn actor_type_label(actor_type: ActorType) -> &'static str {
    match actor_type {
        ActorType::Admin => "admin",
        ActorType::WebDavCredential => "webdav_credential",
        ActorType::McpPat => "mcp_pat",
        ActorType::McpOAuthSubject => "mcp_oauth_subject",
        ActorType::Reconciler => "reconciler",
        ActorType::MemoryWorker => "memory_worker",
        ActorType::System => "system",
    }
}

fn row_to_audit(row: AuditRow) -> Result<AuditRecord, StateError> {
    Ok(AuditRecord {
        id: EventId::parse(&row.id)?,
        occurred_at: row.occurred_at,
        request_id: row.request_id,
        vault_id: row.vault_id.as_deref().map(VaultId::parse).transpose()?,
        plane: row.plane,
        actor_type: row.actor_type,
        actor_id: row.actor_id,
        action: row.action,
        target_type: row.target_type,
        target_id: row.target_id,
        target_path_hash: row.target_path_hash,
        result: row.result,
        metadata: serde_json::from_str(&row.metadata_json)?,
    })
}
