//! Vault-scoped durable memory projections and candidate state.
//!
//! Canonical memory Markdown is written by Vault Core. This module owns only
//! the authoritative operational projection and rebuildable search metadata.

use serde_json::Value;
use sqlx::{FromRow, QueryBuilder, Sqlite, SqlitePool};

use mcp_vault_domain::{
    FileId, MemoryCandidateId, MemoryId, MemoryRelationId, MemorySourceId, Revision, VaultContext,
    VaultId, VaultPath,
};

use crate::{StateError, now_millis};

const MAX_MEMORY_LIMIT: u32 = 200;
const MAX_CANDIDATE_LIMIT: u32 = 200;

/// Durable memory projection metadata.
#[derive(Clone, Debug, PartialEq)]
pub struct MemoryRecord {
    /// Stable memory identity.
    pub id: MemoryId,
    /// Isolation boundary.
    pub vault_id: VaultId,
    /// Initial memory type.
    pub memory_type: String,
    /// Candidate/active/superseded/stale/archived/rejected/quarantined.
    pub status: String,
    /// Atomic proposition body.
    pub content: String,
    /// Deterministically normalized proposition.
    pub normalized_content: String,
    /// SHA-256 identity of normalized content.
    pub content_hash: String,
    /// Owner-supplied importance.
    pub importance: f64,
    /// Evidence/model confidence.
    pub confidence: f64,
    /// Explicit, extracted, imported, or direct Markdown origin.
    pub origin: String,
    /// Optimistic memory metadata revision.
    pub revision: Revision,
    /// Canonical managed file identity.
    pub canonical_file_id: Option<FileId>,
    /// Canonical managed Markdown path.
    pub canonical_path: Option<VaultPath>,
    /// Current canonical file revision.
    pub canonical_revision: Option<Revision>,
    /// Temporal start.
    pub valid_from: Option<i64>,
    /// Temporal end.
    pub valid_to: Option<i64>,
    /// Provider/prompt/pipeline metadata, never logged by default.
    pub extraction: Value,
    /// Creation time.
    pub created_at: i64,
    /// Last metadata update.
    pub updated_at: i64,
    /// Last successful recall.
    pub last_recalled_at: Option<i64>,
    /// Number of successful recalls.
    pub recall_count: u64,
}

/// Provenance for one durable memory.
#[derive(Clone, Debug, PartialEq)]
pub struct MemorySourceRecord {
    /// Stable source identity.
    pub id: MemorySourceId,
    /// Isolation boundary.
    pub vault_id: VaultId,
    /// Owning memory.
    pub memory_id: MemoryId,
    /// Note, explicit agent, admin, Markdown, or import.
    pub source_type: String,
    /// Source note identity when known.
    pub note_file_id: Option<FileId>,
    /// Source note path when known.
    pub note_path: Option<VaultPath>,
    /// Source revision when known.
    pub note_revision: Option<Revision>,
    /// Heading path anchor.
    pub heading_path: Vec<String>,
    /// Inclusive source line.
    pub start_line: Option<u32>,
    /// Inclusive source end line.
    pub end_line: Option<u32>,
    /// Hash of a bounded source excerpt.
    pub excerpt_hash: Option<String>,
    /// Redacted actor identity.
    pub actor_id: Option<String>,
    /// Creation time.
    pub created_at: i64,
}

/// One relation between two memories in one Vault.
#[derive(Clone, Debug, PartialEq)]
pub struct MemoryRelationRecord {
    /// Stable relation identity.
    pub id: MemoryRelationId,
    /// Isolation boundary.
    pub vault_id: VaultId,
    /// Source memory.
    pub source_memory_id: MemoryId,
    /// Target memory.
    pub target_memory_id: MemoryId,
    /// Supersedes/supports/contradicts/refines/related_to/derived_from.
    pub relation_type: String,
    /// Relation confidence.
    pub confidence: f64,
    /// Creation time.
    pub created_at: i64,
}

/// A validated automatic extraction candidate.
#[derive(Clone, Debug, PartialEq)]
pub struct MemoryCandidateRecord {
    /// Stable candidate identity.
    pub id: MemoryCandidateId,
    /// Isolation boundary.
    pub vault_id: VaultId,
    /// Source note identity.
    pub source_file_id: FileId,
    /// Source note path at extraction time.
    pub source_path: VaultPath,
    /// Source revision at extraction time.
    pub source_revision: Revision,
    /// Structured candidate proposal without raw note body.
    pub candidate: Value,
    /// Candidate proposition hash.
    pub content_hash: String,
    /// Deterministic source/pipeline/candidate fingerprint.
    pub extraction_fingerprint: String,
    /// Candidate confidence.
    pub confidence: f64,
    /// Candidate importance.
    pub importance: f64,
    /// Review decision.
    pub decision: Option<String>,
    /// Redacted review reason.
    pub decision_reason: Option<String>,
    /// Creation time.
    pub created_at: i64,
    /// Review time.
    pub reviewed_at: Option<i64>,
}

/// A memory plus its sourced projections.
#[derive(Clone, Debug, PartialEq)]
pub struct MemoryBundle {
    /// Memory row.
    pub memory: MemoryRecord,
    /// Provenance sources.
    pub sources: Vec<MemorySourceRecord>,
    /// Normalized entities.
    pub entities: Vec<String>,
    /// Display tags.
    pub tags: Vec<String>,
    /// Memory relations.
    pub relations: Vec<MemoryRelationRecord>,
}

/// Bounded filters shared by list, lexical, and metadata recall pools.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MemoryFilter {
    /// Allowed lifecycle statuses.
    pub statuses: Vec<String>,
    /// Allowed memory types.
    pub memory_types: Vec<String>,
    /// Optional normalized tag.
    pub tag: Option<String>,
    /// Optional normalized entity.
    pub entity: Option<String>,
    /// Optional source path prefix.
    pub source_path: Option<String>,
    /// Optional point-in-time validity check.
    pub valid_at: Option<i64>,
    /// Minimum importance.
    pub min_importance: Option<f64>,
}

/// One lexical memory candidate with a lower-is-better FTS rank.
#[derive(Clone, Debug, PartialEq)]
pub struct MemorySearchHit {
    /// Memory projection.
    pub memory: MemoryRecord,
    /// SQLite FTS rank, normalized by the caller.
    pub rank: f64,
}

/// Durable explicit-command idempotency mapping.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MemoryIdempotencyRecord {
    /// Vault isolation boundary.
    pub vault_id: VaultId,
    /// Client-supplied idempotency key.
    pub idempotency_key: String,
    /// Hash of the normalized command input.
    pub request_hash: String,
    /// Resulting memory identity.
    pub memory_id: MemoryId,
    /// Created/reinforced/merged outcome.
    pub outcome: String,
    /// Creation time.
    pub created_at: i64,
}

/// Redacted diagnosis for one invalid managed memory file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MemoryDiagnosticRecord {
    /// Stable diagnostic row identity.
    pub id: String,
    /// Vault isolation boundary.
    pub vault_id: VaultId,
    /// Managed relative path.
    pub path: VaultPath,
    /// Stable validation code.
    pub code: String,
    /// Last observation time.
    pub updated_at: i64,
}

/// Bounded lifecycle counts used by the Admin dashboard.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MemoryCounts {
    /// All projected memory rows.
    pub total: u64,
    /// Active rows eligible for normal recall.
    pub active: u64,
    /// Reviewable extraction candidates represented as rows.
    pub candidate: u64,
    /// Rows whose current provenance is no longer supported.
    pub stale: u64,
    /// Explicitly superseded rows.
    pub superseded: u64,
    /// Intentionally archived rows.
    pub archived: u64,
    /// Invalid managed records excluded from recall.
    pub quarantined: u64,
}

#[derive(Debug, FromRow)]
struct MemoryRow {
    id: String,
    vault_id: String,
    memory_type: String,
    status: String,
    content: String,
    normalized_content: String,
    content_hash: String,
    importance: f64,
    confidence: f64,
    origin: String,
    revision: i64,
    canonical_file_id: Option<String>,
    canonical_path: Option<String>,
    canonical_revision: Option<i64>,
    valid_from: Option<i64>,
    valid_to: Option<i64>,
    extraction_json: String,
    created_at: i64,
    updated_at: i64,
    last_recalled_at: Option<i64>,
    recall_count: i64,
}

#[derive(Debug, FromRow)]
struct SourceRow {
    id: String,
    vault_id: String,
    memory_id: String,
    source_type: String,
    note_file_id: Option<String>,
    note_path: Option<String>,
    note_revision: Option<i64>,
    heading_path_json: String,
    start_line: Option<i64>,
    end_line: Option<i64>,
    excerpt_hash: Option<String>,
    actor_id: Option<String>,
    created_at: i64,
}

#[derive(Debug, FromRow)]
struct RelationRow {
    id: String,
    vault_id: String,
    source_memory_id: String,
    target_memory_id: String,
    relation_type: String,
    confidence: f64,
    created_at: i64,
}

#[derive(Debug, FromRow)]
struct CandidateRow {
    id: String,
    vault_id: String,
    source_file_id: String,
    source_path: String,
    source_revision: i64,
    candidate_json: String,
    content_hash: String,
    extraction_fingerprint: String,
    confidence: f64,
    importance: f64,
    decision: Option<String>,
    decision_reason: Option<String>,
    created_at: i64,
    reviewed_at: Option<i64>,
}

/// SQL boundary for memory projection and candidate state.
#[derive(Clone)]
pub struct MemoryRepository {
    pool: SqlitePool,
}

impl MemoryRepository {
    pub(crate) fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Insert or update one complete memory projection transactionally.
    pub async fn replace_bundle(
        &self,
        context: &VaultContext,
        bundle: &MemoryBundle,
        expected_revision: Option<Revision>,
    ) -> Result<MemoryBundle, StateError> {
        validate_bundle(context, bundle)?;
        self.ensure_vault_context(context).await?;
        let current = self.get_memory(context, bundle.memory.id).await?;
        if let Some(expected) = expected_revision
            && current.as_ref().map(|memory| memory.revision) != Some(expected)
        {
            return Err(StateError::InvalidInput("memory revision conflict"));
        }
        let now = now_millis()?;
        let revision = current
            .as_ref()
            .map(|memory| memory.revision.next())
            .transpose()?
            .unwrap_or(bundle.memory.revision);
        if revision < Revision::new(1) {
            return Err(StateError::InvalidInput("memory revision is invalid"));
        }
        let mut transaction = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO memories\n             (id, vault_id, memory_type, status, content,\n              normalized_content, content_hash, importance, confidence, origin,\n              revision, canonical_file_id, canonical_path, canonical_revision,\n              valid_from, valid_to, extraction_json, created_at, updated_at,\n              last_recalled_at, recall_count)\n             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)\n             ON CONFLICT(vault_id, id) DO UPDATE SET\n              memory_type = excluded.memory_type,\n              status = excluded.status,\n              content = excluded.content,\n              normalized_content = excluded.normalized_content,\n              content_hash = excluded.content_hash,\n              importance = excluded.importance,\n              confidence = excluded.confidence,\n              origin = excluded.origin,\n              revision = excluded.revision,\n              canonical_file_id = excluded.canonical_file_id,\n              canonical_path = excluded.canonical_path,\n              canonical_revision = excluded.canonical_revision,\n              valid_from = excluded.valid_from,\n              valid_to = excluded.valid_to,\n              extraction_json = excluded.extraction_json,\n              updated_at = excluded.updated_at,\n              last_recalled_at = excluded.last_recalled_at,\n              recall_count = excluded.recall_count",
        )
        .bind(bundle.memory.id.to_string())
        .bind(context.id().to_string())
        .bind(&bundle.memory.memory_type)
        .bind(&bundle.memory.status)
        .bind(&bundle.memory.content)
        .bind(&bundle.memory.normalized_content)
        .bind(&bundle.memory.content_hash)
        .bind(bundle.memory.importance)
        .bind(bundle.memory.confidence)
        .bind(&bundle.memory.origin)
        .bind(revision.as_i64()?)
        .bind(bundle.memory.canonical_file_id.map(|id| id.to_string()))
        .bind(bundle.memory.canonical_path.as_ref().map(VaultPath::as_str))
        .bind(
            bundle
                .memory
                .canonical_revision
                .map(Revision::as_i64)
                .transpose()?,
        )
        .bind(bundle.memory.valid_from)
        .bind(bundle.memory.valid_to)
        .bind(serde_json::to_string(&bundle.memory.extraction)?)
        .bind(bundle.memory.created_at)
        .bind(now)
        .bind(bundle.memory.last_recalled_at)
        .bind(i64::try_from(bundle.memory.recall_count).map_err(|_| {
            StateError::InvalidInput("memory recall count exceeds SQLite range")
        })?)
        .execute(&mut *transaction)
        .await?;

        let vault_id = context.id().to_string();
        let memory_id = bundle.memory.id.to_string();
        for table in ["memory_sources", "memory_entities", "memory_tags"] {
            let sql = format!("DELETE FROM {table} WHERE vault_id = ? AND memory_id = ?");
            sqlx::query(&sql)
                .bind(&vault_id)
                .bind(&memory_id)
                .execute(&mut *transaction)
                .await?;
        }
        sqlx::query(
            "DELETE FROM memory_relations\n             WHERE vault_id = ? AND source_memory_id = ?",
        )
        .bind(&vault_id)
        .bind(&memory_id)
        .execute(&mut *transaction)
        .await?;
        sqlx::query("DELETE FROM memory_fts WHERE vault_id = ? AND memory_id = ?")
            .bind(&vault_id)
            .bind(&memory_id)
            .execute(&mut *transaction)
            .await?;

        for source in &bundle.sources {
            validate_source(context, source)?;
            sqlx::query(
                "INSERT INTO memory_sources\n                 (id, vault_id, memory_id, source_type, note_file_id,\n                  note_path, note_revision, heading_path_json, start_line,\n                  end_line, excerpt_hash, actor_id, created_at)\n                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(source.id.to_string())
            .bind(&vault_id)
            .bind(&memory_id)
            .bind(&source.source_type)
            .bind(source.note_file_id.map(|id| id.to_string()))
            .bind(source.note_path.as_ref().map(VaultPath::as_str))
            .bind(source.note_revision.map(Revision::as_i64).transpose()?)
            .bind(serde_json::to_string(&source.heading_path)?)
            .bind(source.start_line.map(i64::from))
            .bind(source.end_line.map(i64::from))
            .bind(source.excerpt_hash.as_deref())
            .bind(source.actor_id.as_deref())
            .bind(source.created_at)
            .execute(&mut *transaction)
            .await?;
        }
        for entity in &bundle.entities {
            let normalized = normalize_term(entity);
            if normalized.is_empty() {
                continue;
            }
            sqlx::query(
                "INSERT INTO memory_entities\n                 (vault_id, memory_id, entity, normalized_entity)\n                 VALUES (?, ?, ?, ?)",
            )
            .bind(&vault_id)
            .bind(&memory_id)
            .bind(entity)
            .bind(normalized)
            .execute(&mut *transaction)
            .await?;
        }
        for tag in &bundle.tags {
            let normalized = normalize_term(tag);
            if normalized.is_empty() {
                continue;
            }
            sqlx::query(
                "INSERT INTO memory_tags\n                 (vault_id, memory_id, tag, normalized_tag)\n                 VALUES (?, ?, ?, ?)",
            )
            .bind(&vault_id)
            .bind(&memory_id)
            .bind(tag)
            .bind(normalized)
            .execute(&mut *transaction)
            .await?;
        }
        for relation in &bundle.relations {
            if relation.vault_id != context.id() {
                return Err(StateError::InvalidInput(
                    "memory relation Vault does not match context",
                ));
            }
            sqlx::query(
                "INSERT INTO memory_relations\n                 (id, vault_id, source_memory_id, target_memory_id,\n                  relation_type, confidence, created_at)\n                 VALUES (?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(relation.id.to_string())
            .bind(&vault_id)
            .bind(relation.source_memory_id.to_string())
            .bind(relation.target_memory_id.to_string())
            .bind(&relation.relation_type)
            .bind(relation.confidence)
            .bind(relation.created_at)
            .execute(&mut *transaction)
            .await?;
        }
        let entities = bundle
            .entities
            .iter()
            .map(|value| normalize_term(value))
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        let tags = bundle
            .tags
            .iter()
            .map(|value| normalize_term(value))
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        sqlx::query(
            "INSERT INTO memory_fts\n             (vault_id, memory_id, content, normalized_content, entities, tags)\n             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&vault_id)
        .bind(&memory_id)
        .bind(&bundle.memory.content)
        .bind(&bundle.memory.normalized_content)
        .bind(entities)
        .bind(tags)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        self.get_bundle(context, bundle.memory.id)
            .await?
            .ok_or(StateError::InvalidInput("memory was not saved"))
    }

    /// Fetch one memory projection without its child rows.
    pub async fn get_memory(
        &self,
        context: &VaultContext,
        memory_id: MemoryId,
    ) -> Result<Option<MemoryRecord>, StateError> {
        let row = sqlx::query_as::<_, MemoryRow>(
            "SELECT id, vault_id, memory_type, status, content,\n                    normalized_content, content_hash, importance, confidence,\n                    origin, revision, canonical_file_id, canonical_path,\n                    canonical_revision, valid_from, valid_to, extraction_json,\n                    created_at, updated_at, last_recalled_at, recall_count\n             FROM memories WHERE vault_id = ? AND id = ?",
        )
        .bind(context.id().to_string())
        .bind(memory_id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_memory).transpose()
    }

    /// Fetch one memory by its canonical managed path.
    pub async fn get_by_canonical_path(
        &self,
        context: &VaultContext,
        path: &VaultPath,
    ) -> Result<Option<MemoryRecord>, StateError> {
        let row = sqlx::query_as::<_, MemoryRow>(
            "SELECT id, vault_id, memory_type, status, content,
                    normalized_content, content_hash, importance, confidence,
                    origin, revision, canonical_file_id, canonical_path,
                    canonical_revision, valid_from, valid_to, extraction_json,
                    created_at, updated_at, last_recalled_at, recall_count
             FROM memories WHERE vault_id = ? AND canonical_path = ?",
        )
        .bind(context.id().to_string())
        .bind(path.as_str())
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_memory).transpose()
    }

    /// Find memories with one exact content hash in selected statuses.
    pub async fn find_by_content_hash(
        &self,
        context: &VaultContext,
        content_hash: &str,
        statuses: &[String],
    ) -> Result<Vec<MemoryRecord>, StateError> {
        let mut query = QueryBuilder::<Sqlite>::new(
            "SELECT m.id, m.vault_id, m.memory_type, m.status, m.content,
                    m.normalized_content, m.content_hash, m.importance,
                    m.confidence, m.origin, m.revision, m.canonical_file_id,
                    m.canonical_path, m.canonical_revision, m.valid_from,
                    m.valid_to, m.extraction_json, m.created_at, m.updated_at,
                    m.last_recalled_at, m.recall_count
             FROM memories m WHERE m.vault_id = ",
        );
        query.push_bind(context.id().to_string());
        query.push(" AND m.content_hash = ");
        query.push_bind(content_hash);
        if !statuses.is_empty() {
            query.push(" AND m.status IN (");
            for (index, status) in statuses.iter().enumerate() {
                if index != 0 {
                    query.push(", ");
                }
                query.push_bind(status);
            }
            query.push(")");
        }
        let rows = query
            .build_query_as::<MemoryRow>()
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter().map(row_to_memory).collect()
    }

    /// Fetch one memory and all sourced projections.
    pub async fn get_bundle(
        &self,
        context: &VaultContext,
        memory_id: MemoryId,
    ) -> Result<Option<MemoryBundle>, StateError> {
        let Some(memory) = self.get_memory(context, memory_id).await? else {
            return Ok(None);
        };
        let sources = self.list_sources(context, memory_id).await?;
        let entities = sqlx::query_scalar::<_, String>(
            "SELECT entity FROM memory_entities\n             WHERE vault_id = ? AND memory_id = ?\n             ORDER BY normalized_entity ASC",
        )
        .bind(context.id().to_string())
        .bind(memory_id.to_string())
        .fetch_all(&self.pool)
        .await?;
        let tags = sqlx::query_scalar::<_, String>(
            "SELECT tag FROM memory_tags\n             WHERE vault_id = ? AND memory_id = ?\n             ORDER BY normalized_tag ASC",
        )
        .bind(context.id().to_string())
        .bind(memory_id.to_string())
        .fetch_all(&self.pool)
        .await?;
        let relations = self.list_relations(context, memory_id).await?;
        Ok(Some(MemoryBundle {
            memory,
            sources,
            entities,
            tags,
            relations,
        }))
    }

    /// List memories with bounded status/type/temporal/metadata filters.
    pub async fn list_memories(
        &self,
        context: &VaultContext,
        filter: &MemoryFilter,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<MemoryRecord>, StateError> {
        validate_page(limit, offset, MAX_MEMORY_LIMIT)?;
        let mut query = QueryBuilder::<Sqlite>::new(
            "SELECT m.id, m.vault_id, m.memory_type, m.status, m.content,\n                    m.normalized_content, m.content_hash, m.importance,\n                    m.confidence, m.origin, m.revision, m.canonical_file_id,\n                    m.canonical_path, m.canonical_revision, m.valid_from,\n                    m.valid_to, m.extraction_json, m.created_at, m.updated_at,\n                    m.last_recalled_at, m.recall_count\n             FROM memories m WHERE m.vault_id = ",
        );
        query.push_bind(context.id().to_string());
        append_memory_filter(&mut query, filter);
        query.push(" ORDER BY m.updated_at DESC, m.id ASC LIMIT ");
        query.push_bind(i64::from(limit));
        query.push(" OFFSET ");
        query.push_bind(i64::from(offset));
        let rows = query
            .build_query_as::<MemoryRow>()
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter().map(row_to_memory).collect()
    }

    /// Count lifecycle states for one Vault without loading memory bodies.
    pub async fn counts(&self, context: &VaultContext) -> Result<MemoryCounts, StateError> {
        let rows = sqlx::query_as::<_, (String, i64)>(
            "SELECT status, COUNT(*)
             FROM memories
             WHERE vault_id = ?
             GROUP BY status",
        )
        .bind(context.id().to_string())
        .fetch_all(&self.pool)
        .await?;
        let mut counts = MemoryCounts::default();
        for (status, value) in rows {
            let value = u64::try_from(value)
                .map_err(|_| StateError::InvalidInput("memory count is invalid"))?;
            counts.total = counts.total.saturating_add(value);
            match status.as_str() {
                "active" => counts.active = value,
                "candidate" => counts.candidate = value,
                "stale" => counts.stale = value,
                "superseded" => counts.superseded = value,
                "archived" => counts.archived = value,
                "quarantined" => counts.quarantined = value,
                _ => {}
            }
        }
        let candidate_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM memory_candidates
             WHERE vault_id = ? AND (decision IS NULL OR decision = 'review')",
        )
        .bind(context.id().to_string())
        .fetch_one(&self.pool)
        .await?;
        counts.candidate = u64::try_from(candidate_count)
            .map_err(|_| StateError::InvalidInput("candidate count is invalid"))?;
        Ok(counts)
    }

    /// Search the memory FTS projection for one Vault.
    pub async fn search_fts(
        &self,
        context: &VaultContext,
        fts_query: &str,
        filter: &MemoryFilter,
        limit: u32,
    ) -> Result<Vec<MemorySearchHit>, StateError> {
        validate_page(limit, 0, MAX_MEMORY_LIMIT)?;
        if fts_query.is_empty() || fts_query.len() > 4096 {
            return Err(StateError::InvalidInput("memory FTS query is invalid"));
        }
        let mut query = QueryBuilder::<Sqlite>::new(
            "SELECT m.id, m.vault_id, m.memory_type, m.status, m.content,\n                    m.normalized_content, m.content_hash, m.importance,\n                    m.confidence, m.origin, m.revision, m.canonical_file_id,\n                    m.canonical_path, m.canonical_revision, m.valid_from,\n                    m.valid_to, m.extraction_json, m.created_at, m.updated_at,\n                    m.last_recalled_at, m.recall_count,\n                    bm25(memory_fts) AS memory_rank\n             FROM memory_fts\n             JOIN memories m ON m.vault_id = memory_fts.vault_id\n                            AND m.id = memory_fts.memory_id\n             WHERE memory_fts.vault_id = ",
        );
        query.push_bind(context.id().to_string());
        query.push(" AND memory_fts MATCH ");
        query.push_bind(fts_query);
        append_memory_filter(&mut query, filter);
        query.push(" ORDER BY memory_rank ASC, m.id ASC LIMIT ");
        query.push_bind(i64::from(limit));
        let rows = query
            .build_query_as::<MemorySearchRow>()
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter()
            .map(|row| {
                Ok(MemorySearchHit {
                    memory: row_to_memory(row.memory)?,
                    rank: row.memory_rank,
                })
            })
            .collect()
    }

    /// Search entity/tag projections for one Vault.
    pub async fn search_terms(
        &self,
        context: &VaultContext,
        entities: &[String],
        tags: &[String],
        filter: &MemoryFilter,
        limit: u32,
    ) -> Result<Vec<MemoryRecord>, StateError> {
        validate_page(limit, 0, MAX_MEMORY_LIMIT)?;
        let entities = entities
            .iter()
            .map(|value| normalize_term(value))
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>();
        let tags = tags
            .iter()
            .map(|value| normalize_term(value))
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>();
        if entities.is_empty() && tags.is_empty() {
            return Ok(Vec::new());
        }
        let mut query = QueryBuilder::<Sqlite>::new(
            "SELECT m.id, m.vault_id, m.memory_type, m.status, m.content,\n                    m.normalized_content, m.content_hash, m.importance,\n                    m.confidence, m.origin, m.revision, m.canonical_file_id,\n                    m.canonical_path, m.canonical_revision, m.valid_from,\n                    m.valid_to, m.extraction_json, m.created_at, m.updated_at,\n                    m.last_recalled_at, m.recall_count\n             FROM memories m WHERE m.vault_id = ",
        );
        query.push_bind(context.id().to_string());
        query.push(" AND (");
        let mut first = true;
        for entity in &entities {
            if !first {
                query.push(" OR ");
            }
            first = false;
            query.push(
                "EXISTS (SELECT 1 FROM memory_entities e\n                         WHERE e.vault_id = m.vault_id\n                           AND e.memory_id = m.id\n                           AND e.normalized_entity = ",
            );
            query.push_bind(entity);
            query.push(")");
        }
        for tag in &tags {
            if !first {
                query.push(" OR ");
            }
            first = false;
            query.push(
                "EXISTS (SELECT 1 FROM memory_tags t\n                         WHERE t.vault_id = m.vault_id\n                           AND t.memory_id = m.id\n                           AND t.normalized_tag = ",
            );
            query.push_bind(tag);
            query.push(")");
        }
        query.push(")");
        append_memory_filter(&mut query, filter);
        query.push(" ORDER BY m.importance DESC, m.updated_at DESC, m.id ASC LIMIT ");
        query.push_bind(i64::from(limit));
        let rows = query
            .build_query_as::<MemoryRow>()
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter().map(row_to_memory).collect()
    }

    /// Return recent memories for continuity boosts.
    pub async fn recent_memories(
        &self,
        context: &VaultContext,
        filter: &MemoryFilter,
        limit: u32,
    ) -> Result<Vec<MemoryRecord>, StateError> {
        self.list_memories(context, filter, limit, 0).await
    }

    /// Add or return one candidate by its deterministic fingerprint.
    pub async fn insert_candidate(
        &self,
        context: &VaultContext,
        candidate: &MemoryCandidateRecord,
    ) -> Result<MemoryCandidateRecord, StateError> {
        if candidate.vault_id != context.id() {
            return Err(StateError::InvalidInput(
                "memory candidate Vault does not match context",
            ));
        }
        validate_score(candidate.confidence)?;
        validate_score(candidate.importance)?;
        sqlx::query(
            "INSERT INTO memory_candidates\n             (id, vault_id, source_file_id, source_path, source_revision,\n              candidate_json, content_hash, extraction_fingerprint, confidence,\n              importance, decision, decision_reason, created_at, reviewed_at)\n             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)\n             ON CONFLICT(vault_id, extraction_fingerprint) DO NOTHING",
        )
        .bind(candidate.id.to_string())
        .bind(context.id().to_string())
        .bind(candidate.source_file_id.to_string())
        .bind(candidate.source_path.as_str())
        .bind(candidate.source_revision.as_i64()?)
        .bind(serde_json::to_string(&candidate.candidate)?)
        .bind(&candidate.content_hash)
        .bind(&candidate.extraction_fingerprint)
        .bind(candidate.confidence)
        .bind(candidate.importance)
        .bind(candidate.decision.as_deref())
        .bind(candidate.decision_reason.as_deref())
        .bind(candidate.created_at)
        .bind(candidate.reviewed_at)
        .execute(&self.pool)
        .await?;
        self.get_candidate_by_fingerprint(context, &candidate.extraction_fingerprint)
            .await?
            .ok_or(StateError::InvalidInput("memory candidate was not saved"))
    }

    /// Fetch a candidate by identity.
    pub async fn get_candidate(
        &self,
        context: &VaultContext,
        candidate_id: MemoryCandidateId,
    ) -> Result<Option<MemoryCandidateRecord>, StateError> {
        let row = sqlx::query_as::<_, CandidateRow>(
            "SELECT id, vault_id, source_file_id, source_path, source_revision,\n                    candidate_json, content_hash, extraction_fingerprint,\n                    confidence, importance, decision, decision_reason,\n                    created_at, reviewed_at\n             FROM memory_candidates WHERE vault_id = ? AND id = ?",
        )
        .bind(context.id().to_string())
        .bind(candidate_id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_candidate).transpose()
    }

    /// Fetch a candidate by its idempotent extraction fingerprint.
    pub async fn get_candidate_by_fingerprint(
        &self,
        context: &VaultContext,
        fingerprint: &str,
    ) -> Result<Option<MemoryCandidateRecord>, StateError> {
        let row = sqlx::query_as::<_, CandidateRow>(
            "SELECT id, vault_id, source_file_id, source_path, source_revision,\n                    candidate_json, content_hash, extraction_fingerprint,\n                    confidence, importance, decision, decision_reason,\n                    created_at, reviewed_at\n             FROM memory_candidates WHERE vault_id = ? AND extraction_fingerprint = ?",
        )
        .bind(context.id().to_string())
        .bind(fingerprint)
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_candidate).transpose()
    }

    /// List reviewable candidates.
    pub async fn list_candidates(
        &self,
        context: &VaultContext,
        decision: Option<&str>,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<MemoryCandidateRecord>, StateError> {
        validate_page(limit, offset, MAX_CANDIDATE_LIMIT)?;
        let row = if let Some(decision) = decision {
            sqlx::query_as::<_, CandidateRow>(
                "SELECT id, vault_id, source_file_id, source_path, source_revision,\n                        candidate_json, content_hash, extraction_fingerprint,\n                        confidence, importance, decision, decision_reason,\n                        created_at, reviewed_at\n                 FROM memory_candidates\n                 WHERE vault_id = ? AND decision = ?\n                 ORDER BY created_at DESC, id ASC LIMIT ? OFFSET ?",
            )
            .bind(context.id().to_string())
            .bind(decision)
            .bind(i64::from(limit))
            .bind(i64::from(offset))
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query_as::<_, CandidateRow>(
                "SELECT id, vault_id, source_file_id, source_path, source_revision,\n                        candidate_json, content_hash, extraction_fingerprint,\n                        confidence, importance, decision, decision_reason,\n                        created_at, reviewed_at\n                 FROM memory_candidates\n                 WHERE vault_id = ?\n                 ORDER BY created_at DESC, id ASC LIMIT ? OFFSET ?",
            )
            .bind(context.id().to_string())
            .bind(i64::from(limit))
            .bind(i64::from(offset))
            .fetch_all(&self.pool)
            .await?
        };
        row.into_iter().map(row_to_candidate).collect()
    }

    /// Record a candidate review decision.
    pub async fn decide_candidate(
        &self,
        context: &VaultContext,
        candidate_id: MemoryCandidateId,
        decision: &str,
        reason: Option<&str>,
    ) -> Result<MemoryCandidateRecord, StateError> {
        if !matches!(decision, "promoted" | "rejected" | "review") {
            return Err(StateError::InvalidInput("candidate decision is invalid"));
        }
        let result = sqlx::query(
            "UPDATE memory_candidates\n             SET decision = ?, decision_reason = ?, reviewed_at = ?\n             WHERE vault_id = ? AND id = ?",
        )
        .bind(decision)
        .bind(reason)
        .bind(now_millis()?)
        .bind(context.id().to_string())
        .bind(candidate_id.to_string())
        .execute(&self.pool)
        .await?;
        if result.rows_affected() != 1 {
            return Err(StateError::InvalidInput("memory candidate does not exist"));
        }
        self.get_candidate(context, candidate_id)
            .await?
            .ok_or(StateError::InvalidInput("memory candidate disappeared"))
    }

    /// Fetch an explicit remember idempotency result.
    pub async fn get_idempotency(
        &self,
        context: &VaultContext,
        key: &str,
    ) -> Result<Option<MemoryIdempotencyRecord>, StateError> {
        let row = sqlx::query_as::<_, (String, String, String, String, String, i64)>(
            "SELECT vault_id, idempotency_key, request_hash, memory_id,
                    outcome, created_at
             FROM memory_idempotency WHERE vault_id = ? AND idempotency_key = ?",
        )
        .bind(context.id().to_string())
        .bind(key)
        .fetch_optional(&self.pool)
        .await?;
        row.map(
            |(vault_id, idempotency_key, request_hash, memory_id, outcome, created_at)| {
                Ok(MemoryIdempotencyRecord {
                    vault_id: VaultId::parse(&vault_id)?,
                    idempotency_key,
                    request_hash,
                    memory_id: MemoryId::parse(&memory_id)?,
                    outcome,
                    created_at,
                })
            },
        )
        .transpose()
    }

    /// Insert an explicit remember idempotency result.
    pub async fn put_idempotency(
        &self,
        context: &VaultContext,
        key: &str,
        request_hash: &str,
        memory_id: MemoryId,
        outcome: &str,
    ) -> Result<(), StateError> {
        sqlx::query(
            "INSERT INTO memory_idempotency
             (vault_id, idempotency_key, request_hash, memory_id, outcome, created_at)
             VALUES (?, ?, ?, ?, ?, ?)
             ON CONFLICT(vault_id, idempotency_key) DO NOTHING",
        )
        .bind(context.id().to_string())
        .bind(key)
        .bind(request_hash)
        .bind(memory_id.to_string())
        .bind(outcome)
        .bind(now_millis()?)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Upsert one redacted invalid-managed-file diagnostic.
    pub async fn upsert_diagnostic(
        &self,
        context: &VaultContext,
        path: &VaultPath,
        code: &str,
    ) -> Result<(), StateError> {
        sqlx::query(
            "INSERT INTO memory_diagnostics (id, vault_id, path, code, updated_at)
             VALUES (?, ?, ?, ?, ?)
             ON CONFLICT(vault_id, path) DO UPDATE SET
               code = excluded.code, updated_at = excluded.updated_at",
        )
        .bind(format!("{}:{path}", context.id()))
        .bind(context.id().to_string())
        .bind(path.as_str())
        .bind(code)
        .bind(now_millis()?)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Clear a diagnosis after a managed memory file is valid again.
    pub async fn clear_diagnostic(
        &self,
        context: &VaultContext,
        path: &VaultPath,
    ) -> Result<(), StateError> {
        sqlx::query("DELETE FROM memory_diagnostics WHERE vault_id = ? AND path = ?")
            .bind(context.id().to_string())
            .bind(path.as_str())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Return all memories sourced from one note file.
    pub async fn memory_ids_for_source(
        &self,
        context: &VaultContext,
        file_id: FileId,
    ) -> Result<Vec<MemoryId>, StateError> {
        let rows = sqlx::query_scalar::<_, String>(
            "SELECT DISTINCT memory_id FROM memory_sources\n             WHERE vault_id = ? AND note_file_id = ?\n             ORDER BY memory_id ASC",
        )
        .bind(context.id().to_string())
        .bind(file_id.to_string())
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|value| MemoryId::parse(&value).map_err(StateError::InvalidDomain))
            .collect()
    }

    /// Update one memory lifecycle/status without replacing content.
    pub async fn set_status(
        &self,
        context: &VaultContext,
        memory_id: MemoryId,
        status: &str,
        expected_revision: Option<Revision>,
    ) -> Result<MemoryRecord, StateError> {
        if !matches!(
            status,
            "candidate"
                | "active"
                | "superseded"
                | "stale"
                | "archived"
                | "rejected"
                | "quarantined"
        ) {
            return Err(StateError::InvalidInput("memory status is invalid"));
        }
        let current = self
            .get_memory(context, memory_id)
            .await?
            .ok_or(StateError::InvalidInput("memory does not exist"))?;
        if let Some(expected) = expected_revision
            && current.revision != expected
        {
            return Err(StateError::InvalidInput("memory revision conflict"));
        }
        let revision = current.revision.next()?;
        sqlx::query(
            "UPDATE memories SET status = ?, revision = ?, updated_at = ?\n             WHERE vault_id = ? AND id = ? AND revision = ?",
        )
        .bind(status)
        .bind(revision.as_i64()?)
        .bind(now_millis()?)
        .bind(context.id().to_string())
        .bind(memory_id.to_string())
        .bind(current.revision.as_i64()?)
        .execute(&self.pool)
        .await?;
        self.get_memory(context, memory_id)
            .await?
            .ok_or(StateError::InvalidInput("memory update disappeared"))
    }

    /// Mark recall statistics without touching canonical Markdown.
    pub async fn mark_recalled(
        &self,
        context: &VaultContext,
        memory_ids: &[MemoryId],
    ) -> Result<(), StateError> {
        if memory_ids.len() > MAX_MEMORY_LIMIT as usize {
            return Err(StateError::InvalidInput("too many recall statistics"));
        }
        let now = now_millis()?;
        for memory_id in memory_ids {
            sqlx::query(
                "UPDATE memories SET last_recalled_at = ?, recall_count = recall_count + 1\n                 WHERE vault_id = ? AND id = ?",
            )
            .bind(now)
            .bind(context.id().to_string())
            .bind(memory_id.to_string())
            .execute(&self.pool)
            .await?;
        }
        Ok(())
    }

    /// Rebuild the memory FTS projection for one Vault.
    pub async fn rebuild_fts(&self, context: &VaultContext) -> Result<(), StateError> {
        self.ensure_vault_context(context).await?;
        let vault_id = context.id().to_string();
        let mut transaction = self.pool.begin().await?;
        sqlx::query("DELETE FROM memory_fts WHERE vault_id = ?")
            .bind(&vault_id)
            .execute(&mut *transaction)
            .await?;
        sqlx::query(
            "INSERT INTO memory_fts\n             (vault_id, memory_id, content, normalized_content, entities, tags)\n             SELECT m.vault_id, m.id, m.content, m.normalized_content,\n                    COALESCE((SELECT group_concat(normalized_entity, ' ')\n                              FROM memory_entities e\n                              WHERE e.vault_id = m.vault_id AND e.memory_id = m.id), ''),\n                    COALESCE((SELECT group_concat(normalized_tag, ' ')\n                              FROM memory_tags t\n                              WHERE t.vault_id = m.vault_id AND t.memory_id = m.id), '')\n             FROM memories m WHERE m.vault_id = ?",
        )
        .bind(&vault_id)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    /// List provenance rows for one memory.
    pub async fn list_sources(
        &self,
        context: &VaultContext,
        memory_id: MemoryId,
    ) -> Result<Vec<MemorySourceRecord>, StateError> {
        let rows = sqlx::query_as::<_, SourceRow>(
            "SELECT id, vault_id, memory_id, source_type, note_file_id,\n                    note_path, note_revision, heading_path_json, start_line,\n                    end_line, excerpt_hash, actor_id, created_at\n             FROM memory_sources WHERE vault_id = ? AND memory_id = ?\n             ORDER BY created_at ASC, id ASC",
        )
        .bind(context.id().to_string())
        .bind(memory_id.to_string())
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_source).collect()
    }

    /// List outgoing relations for one memory.
    pub async fn list_relations(
        &self,
        context: &VaultContext,
        memory_id: MemoryId,
    ) -> Result<Vec<MemoryRelationRecord>, StateError> {
        let rows = sqlx::query_as::<_, RelationRow>(
            "SELECT id, vault_id, source_memory_id, target_memory_id,\n                    relation_type, confidence, created_at\n             FROM memory_relations\n             WHERE vault_id = ? AND source_memory_id = ?\n             ORDER BY relation_type ASC, target_memory_id ASC",
        )
        .bind(context.id().to_string())
        .bind(memory_id.to_string())
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_relation).collect()
    }

    /// Replace only the rebuildable outgoing relation projection.
    ///
    /// Relation targets may be projected after their source in a filesystem
    /// walk, so rebuilds use this second-pass operation after all memory rows
    /// have been admitted. It intentionally does not change the memory
    /// metadata revision or canonical Markdown.
    pub async fn replace_relations(
        &self,
        context: &VaultContext,
        source_memory_id: MemoryId,
        relations: &[MemoryRelationRecord],
    ) -> Result<(), StateError> {
        if self.get_memory(context, source_memory_id).await?.is_none() {
            return Err(StateError::InvalidInput(
                "memory relation source is missing",
            ));
        }
        for relation in relations {
            if relation.vault_id != context.id() || relation.source_memory_id != source_memory_id {
                return Err(StateError::InvalidInput("memory relation is invalid"));
            }
            validate_score(relation.confidence)?;
        }
        let vault_id = context.id().to_string();
        let source_id = source_memory_id.to_string();
        let mut transaction = self.pool.begin().await?;
        sqlx::query(
            "DELETE FROM memory_relations
             WHERE vault_id = ? AND source_memory_id = ?",
        )
        .bind(&vault_id)
        .bind(&source_id)
        .execute(&mut *transaction)
        .await?;
        for relation in relations {
            sqlx::query(
                "INSERT INTO memory_relations
                 (id, vault_id, source_memory_id, target_memory_id,
                  relation_type, confidence, created_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(relation.id.to_string())
            .bind(&vault_id)
            .bind(&source_id)
            .bind(relation.target_memory_id.to_string())
            .bind(&relation.relation_type)
            .bind(relation.confidence)
            .bind(relation.created_at)
            .execute(&mut *transaction)
            .await?;
        }
        transaction.commit().await?;
        Ok(())
    }

    /// Delete one memory projection; canonical file deletion is owned by Core.
    pub async fn delete_memory_projection(
        &self,
        context: &VaultContext,
        memory_id: MemoryId,
    ) -> Result<(), StateError> {
        let result = sqlx::query("DELETE FROM memories WHERE vault_id = ? AND id = ?")
            .bind(context.id().to_string())
            .bind(memory_id.to_string())
            .execute(&self.pool)
            .await?;
        if result.rows_affected() != 1 {
            return Err(StateError::InvalidInput("memory does not exist"));
        }
        Ok(())
    }

    async fn ensure_vault_context(&self, context: &VaultContext) -> Result<(), StateError> {
        let exists: i64 = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM vaults WHERE id = ?)")
            .bind(context.id().to_string())
            .fetch_one(&self.pool)
            .await?;
        if exists != 1 {
            return Err(StateError::InvalidInput("Vault is not registered"));
        }
        Ok(())
    }
}

#[derive(Debug, FromRow)]
struct MemorySearchRow {
    #[sqlx(flatten)]
    memory: MemoryRow,
    memory_rank: f64,
}

fn append_memory_filter<'a>(query: &mut QueryBuilder<'a, Sqlite>, filter: &'a MemoryFilter) {
    if !filter.statuses.is_empty() {
        query.push(" AND m.status IN (");
        for (index, status) in filter.statuses.iter().enumerate() {
            if index != 0 {
                query.push(", ");
            }
            query.push_bind(status);
        }
        query.push(")");
    }
    if !filter.memory_types.is_empty() {
        query.push(" AND m.memory_type IN (");
        for (index, memory_type) in filter.memory_types.iter().enumerate() {
            if index != 0 {
                query.push(", ");
            }
            query.push_bind(memory_type);
        }
        query.push(")");
    }
    if let Some(tag) = filter.tag.as_deref() {
        query.push(
            " AND EXISTS (SELECT 1 FROM memory_tags filter_tag\n                         WHERE filter_tag.vault_id = m.vault_id\n                           AND filter_tag.memory_id = m.id\n                           AND filter_tag.normalized_tag = ",
        );
        query.push_bind(normalize_term(tag));
        query.push(")");
    }
    if let Some(entity) = filter.entity.as_deref() {
        query.push(
            " AND EXISTS (SELECT 1 FROM memory_entities filter_entity\n                         WHERE filter_entity.vault_id = m.vault_id\n                           AND filter_entity.memory_id = m.id\n                           AND filter_entity.normalized_entity = ",
        );
        query.push_bind(normalize_term(entity));
        query.push(")");
    }
    if let Some(source_path) = filter.source_path.as_deref() {
        query.push(
            " AND EXISTS (SELECT 1 FROM memory_sources filter_source\n                         WHERE filter_source.vault_id = m.vault_id\n                           AND filter_source.memory_id = m.id\n                           AND filter_source.note_path LIKE ",
        );
        query.push_bind(format!("{source_path}%"));
        query.push(")");
    }
    if let Some(valid_at) = filter.valid_at {
        query.push(" AND (m.valid_from IS NULL OR m.valid_from <= ");
        query.push_bind(valid_at);
        query.push(") AND (m.valid_to IS NULL OR m.valid_to > ");
        query.push_bind(valid_at);
        query.push(")");
    }
    if let Some(min_importance) = filter.min_importance {
        query.push(" AND m.importance >= ");
        query.push_bind(min_importance);
    }
}

fn validate_bundle(context: &VaultContext, bundle: &MemoryBundle) -> Result<(), StateError> {
    if bundle.memory.vault_id != context.id()
        || bundle.memory.content.trim().is_empty()
        || bundle.memory.content.len() > 64 * 1024
        || bundle.memory.normalized_content.len() > 64 * 1024
        || bundle
            .memory
            .canonical_path
            .as_ref()
            .is_some_and(|path| path.is_root())
    {
        return Err(StateError::InvalidInput("memory projection is invalid"));
    }
    validate_score(bundle.memory.importance)?;
    validate_score(bundle.memory.confidence)?;
    Ok(())
}

fn validate_source(context: &VaultContext, source: &MemorySourceRecord) -> Result<(), StateError> {
    if source.vault_id != context.id()
        || source.heading_path.len() > 32
        || source.heading_path.iter().any(|value| {
            value.is_empty() || value.len() > 512 || value.chars().any(char::is_control)
        })
    {
        return Err(StateError::InvalidInput("memory source is invalid"));
    }
    Ok(())
}

fn validate_score(value: f64) -> Result<(), StateError> {
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        return Err(StateError::InvalidInput("memory score is invalid"));
    }
    Ok(())
}

fn validate_page(limit: u32, offset: u32, max: u32) -> Result<(), StateError> {
    if limit == 0 || limit > max || offset > 1_000_000 {
        return Err(StateError::InvalidInput("memory query page is invalid"));
    }
    Ok(())
}

fn normalize_term(value: &str) -> String {
    value.trim().chars().flat_map(char::to_lowercase).collect()
}

fn row_to_memory(row: MemoryRow) -> Result<MemoryRecord, StateError> {
    Ok(MemoryRecord {
        id: MemoryId::parse(&row.id)?,
        vault_id: VaultId::parse(&row.vault_id)?,
        memory_type: row.memory_type,
        status: row.status,
        content: row.content,
        normalized_content: row.normalized_content,
        content_hash: row.content_hash,
        importance: row.importance,
        confidence: row.confidence,
        origin: row.origin,
        revision: Revision::try_from(row.revision)?,
        canonical_file_id: row
            .canonical_file_id
            .as_deref()
            .map(FileId::parse)
            .transpose()?,
        canonical_path: row
            .canonical_path
            .as_deref()
            .map(VaultPath::parse)
            .transpose()?,
        canonical_revision: row.canonical_revision.map(Revision::try_from).transpose()?,
        valid_from: row.valid_from,
        valid_to: row.valid_to,
        extraction: serde_json::from_str(&row.extraction_json)?,
        created_at: row.created_at,
        updated_at: row.updated_at,
        last_recalled_at: row.last_recalled_at,
        recall_count: u64::try_from(row.recall_count)
            .map_err(|_| StateError::InvalidInput("memory recall count is invalid"))?,
    })
}

fn row_to_source(row: SourceRow) -> Result<MemorySourceRecord, StateError> {
    Ok(MemorySourceRecord {
        id: MemorySourceId::parse(&row.id)?,
        vault_id: VaultId::parse(&row.vault_id)?,
        memory_id: MemoryId::parse(&row.memory_id)?,
        source_type: row.source_type,
        note_file_id: row.note_file_id.as_deref().map(FileId::parse).transpose()?,
        note_path: row.note_path.as_deref().map(VaultPath::parse).transpose()?,
        note_revision: row.note_revision.map(Revision::try_from).transpose()?,
        heading_path: serde_json::from_str(&row.heading_path_json)?,
        start_line: row
            .start_line
            .map(|value| {
                u32::try_from(value).map_err(|_| StateError::InvalidInput("source line is invalid"))
            })
            .transpose()?,
        end_line: row
            .end_line
            .map(|value| {
                u32::try_from(value).map_err(|_| StateError::InvalidInput("source line is invalid"))
            })
            .transpose()?,
        excerpt_hash: row.excerpt_hash,
        actor_id: row.actor_id,
        created_at: row.created_at,
    })
}

fn row_to_relation(row: RelationRow) -> Result<MemoryRelationRecord, StateError> {
    Ok(MemoryRelationRecord {
        id: MemoryRelationId::parse(&row.id)?,
        vault_id: VaultId::parse(&row.vault_id)?,
        source_memory_id: MemoryId::parse(&row.source_memory_id)?,
        target_memory_id: MemoryId::parse(&row.target_memory_id)?,
        relation_type: row.relation_type,
        confidence: row.confidence,
        created_at: row.created_at,
    })
}

fn row_to_candidate(row: CandidateRow) -> Result<MemoryCandidateRecord, StateError> {
    Ok(MemoryCandidateRecord {
        id: MemoryCandidateId::parse(&row.id)?,
        vault_id: VaultId::parse(&row.vault_id)?,
        source_file_id: FileId::parse(&row.source_file_id)?,
        source_path: VaultPath::parse(&row.source_path)?,
        source_revision: Revision::try_from(row.source_revision)?,
        candidate: serde_json::from_str(&row.candidate_json)?,
        content_hash: row.content_hash,
        extraction_fingerprint: row.extraction_fingerprint,
        confidence: row.confidence,
        importance: row.importance,
        decision: row.decision,
        decision_reason: row.decision_reason,
        created_at: row.created_at,
        reviewed_at: row.reviewed_at,
    })
}
