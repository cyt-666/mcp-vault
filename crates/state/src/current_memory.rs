//! Current-only, source-owned durable-memory operational state.
//!
//! The legacy memory repository remains available only for explicit migration
//! and prerelease compatibility workers.  Every query in this module treats a
//! memory as readable only while its canonical Markdown and (for note-derived
//! items) exact source content are still current.

use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::{FromRow, QueryBuilder, Sqlite, SqlitePool, Transaction};

use mcp_vault_domain::{
    FileId, MemoryId, MemorySetId, MemorySetSnapshotId, MemorySourceId, ModelId, ProviderId,
    Revision, VaultContext, VaultId, VaultPath,
};

use crate::{StateError, memory_search_terms, now_millis};

const MAX_MEMORY_LIMIT: u32 = 200;

/// Ownership determines which object controls replacement and deletion.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CurrentMemoryOwnership {
    /// A direct user/Agent/Admin assertion with its own canonical Markdown.
    Explicit,
    /// One item in the current set owned by a source note.
    NoteDerived,
}

impl CurrentMemoryOwnership {
    /// Stable storage/API label.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Explicit => "explicit",
            Self::NoteDerived => "note_derived",
        }
    }

    fn parse(value: &str) -> Result<Self, StateError> {
        match value {
            "explicit" => Ok(Self::Explicit),
            "note_derived" => Ok(Self::NoteDerived),
            _ => Err(StateError::InvalidInput(
                "stored current-memory ownership is invalid",
            )),
        }
    }
}

/// One current memory item. There is deliberately no lifecycle status.
#[derive(Clone, Debug, PartialEq)]
pub struct CurrentMemoryRecord {
    /// Stable current-item identity.
    pub id: MemoryId,
    /// Vault isolation boundary.
    pub vault_id: VaultId,
    /// Direct or note-set ownership.
    pub ownership: CurrentMemoryOwnership,
    /// Owning note set for derived items.
    pub note_set_id: Option<MemorySetId>,
    /// Stable display order inside a note-owned set.
    pub ordinal: Option<u32>,
    /// Optional caller/model supplied semantic kind.
    pub kind: Option<String>,
    /// Sourced durable content.
    pub content: String,
    /// Deterministic lexical normalization.
    pub normalized_content: String,
    /// Exact hash of normalized content.
    pub content_hash: String,
    /// Optional caller-supplied importance; never synthesized by storage.
    pub importance: Option<f64>,
    /// Optional caller/model confidence; never synthesized by storage.
    pub confidence: Option<f64>,
    /// Explicit-agent, explicit-admin, import, or note-extracted.
    pub origin: String,
    /// Optimistic item revision.
    pub revision: Revision,
    /// Canonical file for explicit items. Note-derived items use their set file.
    pub canonical_file_id: Option<FileId>,
    /// Canonical path for explicit items. Note-derived items use their set path.
    pub canonical_path: Option<VaultPath>,
    /// Canonical revision for explicit items.
    pub canonical_revision: Option<Revision>,
    /// Optional temporal validity start.
    pub valid_from: Option<i64>,
    /// Optional temporal validity end.
    pub valid_to: Option<i64>,
    /// Display/search tags.
    pub tags: Vec<String>,
    /// Search entities.
    pub entities: Vec<String>,
    /// Redaction-safe structured metadata.
    pub metadata: Value,
    /// Creation time.
    pub created_at: i64,
    /// Last item update.
    pub updated_at: i64,
    /// Last successful recall.
    pub last_recalled_at: Option<i64>,
    /// Successful recall count.
    pub recall_count: u64,
}

/// Provenance for one current item.
#[derive(Clone, Debug, PartialEq)]
pub struct CurrentMemorySourceRecord {
    /// Stable source-row identity.
    pub id: MemorySourceId,
    /// Vault isolation boundary.
    pub vault_id: VaultId,
    /// Owning current item.
    pub memory_id: MemoryId,
    /// note, explicit_agent, explicit_admin, or import.
    pub source_type: String,
    /// Stable source-note identity.
    pub note_file_id: Option<FileId>,
    /// Source path at capture time.
    pub note_path: Option<VaultPath>,
    /// Source revision at capture time.
    pub note_revision: Option<Revision>,
    /// Exact full-source content hash at capture time.
    pub source_content_hash: Option<String>,
    /// Optional heading anchor.
    pub heading_path: Vec<String>,
    /// Inclusive source start line.
    pub start_line: Option<u32>,
    /// Inclusive source end line.
    pub end_line: Option<u32>,
    /// Optional bounded evidence hash.
    pub excerpt_hash: Option<String>,
    /// Redacted actor identity.
    pub actor_id: Option<String>,
    /// Creation time.
    pub created_at: i64,
}

/// Current item plus provenance and owning-set metadata.
#[derive(Clone, Debug, PartialEq)]
pub struct CurrentMemoryBundle {
    /// Current item.
    pub memory: CurrentMemoryRecord,
    /// Current provenance.
    pub sources: Vec<CurrentMemorySourceRecord>,
    /// Owning set for note-derived items.
    pub note_set: Option<MemoryNoteSetRecord>,
}

/// One source note's single current memory set.
#[derive(Clone, Debug, PartialEq)]
pub struct MemoryNoteSetRecord {
    /// Stable set identity.
    pub id: MemorySetId,
    /// Vault isolation boundary.
    pub vault_id: VaultId,
    /// Stable source File ID.
    pub source_file_id: FileId,
    /// Latest navigable source path.
    pub source_path: VaultPath,
    /// Exact source bytes/content hash represented by this set.
    pub source_content_hash: String,
    /// Source revision at extraction time.
    pub source_revision: Revision,
    /// Optimistic set revision.
    pub set_revision: Revision,
    /// Manual derived-item deletion pauses future automatic extraction.
    pub extraction_paused: bool,
    /// Canonical set Markdown identity.
    pub canonical_file_id: FileId,
    /// Canonical set Markdown path.
    pub canonical_path: VaultPath,
    /// Canonical set Markdown revision.
    pub canonical_revision: Revision,
    /// Exact extraction-input profile.
    pub profile_hash: String,
    /// Structured extraction contract version.
    pub prompt_version: String,
    /// Provider used for this set.
    pub provider_id: Option<ProviderId>,
    /// Model used for this set.
    pub model_id: Option<ModelId>,
    /// Creation time.
    pub created_at: i64,
    /// Last replacement time.
    pub updated_at: i64,
}

/// Crash-safe model output prepared before canonical publication.
#[derive(Clone, Debug, PartialEq)]
pub struct MemoryNoteSetSnapshotRecord {
    /// Stable snapshot identity.
    pub id: MemorySetSnapshotId,
    /// Vault isolation boundary.
    pub vault_id: VaultId,
    /// Target set identity, allocated before the first publication.
    pub note_set_id: MemorySetId,
    /// Stable source identity.
    pub source_file_id: FileId,
    /// Source path represented by the proposal.
    pub source_path: VaultPath,
    /// Exact source content represented by the model call.
    pub source_content_hash: String,
    /// Exact source revision represented by the model call.
    pub source_revision: Revision,
    /// Set revision observed before the model call.
    pub expected_set_revision: Option<Revision>,
    /// Set revision to publish.
    pub proposed_set_revision: Revision,
    /// Pause state atomically published with the complete source set.
    pub extraction_paused: bool,
    /// Validated, server-owned item representation.
    pub items: Value,
    /// Hash of deterministic canonical bytes to adopt during recovery.
    pub canonical_bytes_hash: String,
    /// Deterministic canonical target.
    pub canonical_path: VaultPath,
    /// Extraction-input profile.
    pub profile_hash: String,
    /// Structured extraction contract version.
    pub prompt_version: String,
    /// Provider used for the one model call.
    pub provider_id: ProviderId,
    /// Model used for the one model call.
    pub model_id: ModelId,
    /// prepared, applied, or rejected.
    pub status: String,
    /// Creation time.
    pub created_at: i64,
    /// Terminal application time.
    pub applied_at: Option<i64>,
}

/// Current-only list/search filters.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CurrentMemoryFilter {
    /// Optional kind labels.
    pub kinds: Vec<String>,
    /// Optional ownership labels.
    pub ownership: Vec<CurrentMemoryOwnership>,
    /// Optional exact tag, case-insensitive.
    pub tag: Option<String>,
    /// Optional exact entity, case-insensitive.
    pub entity: Option<String>,
    /// Optional current source path.
    pub source_path: Option<String>,
    /// Optional temporal eligibility point.
    pub valid_at: Option<i64>,
    /// Minimum importance; missing importance behaves as zero for this filter.
    pub min_importance: Option<f64>,
}

/// FTS result for one current item.
#[derive(Clone, Debug, PartialEq)]
pub struct CurrentMemorySearchHit {
    /// Current item.
    pub memory: CurrentMemoryRecord,
    /// Raw FTS5 rank; lower is better.
    pub rank: f64,
}

/// Current-only dashboard counts without legacy lifecycle categories.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
pub struct CurrentMemoryCounts {
    /// All readable current items.
    pub total: u64,
    /// Independently owned explicit items.
    pub explicit: u64,
    /// Items owned by current source-note sets.
    pub note_derived: u64,
    /// Source sets paused after a manual derived-item deletion.
    pub paused_sources: u64,
}

/// Durable identity reservation for a retryable explicit-memory command.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CurrentExplicitReservation {
    /// Request hash bound to the caller's idempotency key.
    pub request_hash: String,
    /// Stable identity allocated before the canonical write.
    pub memory_id: MemoryId,
    /// Stable creation time reused by retries.
    pub created_at: i64,
}

/// Non-destructive classification of legacy rows before explicit migration.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct MemoryV2MigrationPreflight {
    /// Content-free digest of every legacy field consumed by migration. This
    /// changes even when row counts and ownership classes stay the same.
    pub classified_state_hash: String,
    /// All legacy rows.
    pub legacy_total: u64,
    /// Non-active legacy lifecycle rows retained only in backup/report scope.
    pub historical: u64,
    /// Rows with only explicit/import provenance and no ambiguity.
    pub safe_explicit: u64,
    /// Rows with only note provenance.
    pub note_derived: u64,
    /// Rows combining note and explicit/import provenance.
    pub mixed_source: u64,
    /// Rows with no recognized source shape.
    pub unsupported: u64,
    /// Content-free IDs requiring an operator ownership decision.
    pub mixed_source_ids: Vec<String>,
    /// Content-free IDs with unsupported provenance.
    pub unsupported_ids: Vec<String>,
}

impl MemoryV2MigrationPreflight {
    /// Stable content-free fingerprint that binds an operator confirmation to
    /// the exact preflight classification they reviewed.
    pub fn fingerprint(&self) -> Result<String, StateError> {
        let encoded = serde_json::to_vec(self)?;
        let mut hasher = Sha256::new();
        hasher.update(b"mcp-vault-memory-v2.1-preflight\0");
        hasher.update(encoded);
        Ok(format!("sha256:{:x}", hasher.finalize()))
    }
}

/// SQL boundary for current-only memory state.
#[derive(Clone)]
pub struct CurrentMemoryRepository {
    pool: SqlitePool,
}

impl CurrentMemoryRepository {
    pub(crate) fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Return one readable current item. Stale canonical/source state is
    /// indistinguishable from a missing item at this boundary.
    pub async fn get(
        &self,
        context: &VaultContext,
        memory_id: MemoryId,
    ) -> Result<Option<CurrentMemoryBundle>, StateError> {
        self.ensure_vault_context(context).await?;
        let sql = format!(
            "{} WHERE i.vault_id = ? AND i.id = ? AND {}",
            item_select(),
            current_eligibility_sql()
        );
        let row = sqlx::query_as::<_, CurrentMemoryRow>(&sql)
            .bind(context.id().to_string())
            .bind(memory_id.to_string())
            .fetch_optional(&self.pool)
            .await?;
        self.bundle_from_optional_row(context, row).await
    }

    /// Return one item without freshness filtering for reconciliation and
    /// revision-aware mutation. Protocol reads must use [`Self::get`].
    pub async fn get_unchecked(
        &self,
        context: &VaultContext,
        memory_id: MemoryId,
    ) -> Result<Option<CurrentMemoryBundle>, StateError> {
        self.ensure_vault_context(context).await?;
        let sql = format!("{} WHERE i.vault_id = ? AND i.id = ?", item_select());
        let row = sqlx::query_as::<_, CurrentMemoryRow>(&sql)
            .bind(context.id().to_string())
            .bind(memory_id.to_string())
            .fetch_optional(&self.pool)
            .await?;
        self.bundle_from_optional_row(context, row).await
    }

    /// List only readable current items in deterministic order.
    pub async fn list(
        &self,
        context: &VaultContext,
        filter: &CurrentMemoryFilter,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<CurrentMemoryRecord>, StateError> {
        validate_page(limit, offset)?;
        self.ensure_vault_context(context).await?;
        let mut query = QueryBuilder::<Sqlite>::new(item_select());
        query.push(" WHERE i.vault_id = ");
        query.push_bind(context.id().to_string());
        query.push(" AND ");
        query.push(current_eligibility_sql());
        append_filter(&mut query, filter);
        query.push(" ORDER BY i.updated_at DESC, i.id ASC LIMIT ");
        query.push_bind(i64::from(limit));
        query.push(" OFFSET ");
        query.push_bind(i64::from(offset));
        let rows = query
            .build_query_as::<CurrentMemoryRow>()
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter().map(row_to_item).collect()
    }

    /// Return current-only ownership and pause counts for the control plane.
    pub async fn counts(&self, context: &VaultContext) -> Result<CurrentMemoryCounts, StateError> {
        self.ensure_vault_context(context).await?;
        let sql = format!(
            "SELECT i.ownership, COUNT(*) FROM memory_current_items i\n\
             WHERE i.vault_id = ? AND {} GROUP BY i.ownership",
            current_eligibility_sql()
        );
        let rows = sqlx::query_as::<_, (String, i64)>(&sql)
            .bind(context.id().to_string())
            .fetch_all(&self.pool)
            .await?;
        let mut counts = CurrentMemoryCounts::default();
        for (ownership, count) in rows {
            let count = u64::try_from(count)
                .map_err(|_| StateError::InvalidInput("current-memory count is invalid"))?;
            counts.total = counts.total.saturating_add(count);
            match CurrentMemoryOwnership::parse(&ownership)? {
                CurrentMemoryOwnership::Explicit => counts.explicit = count,
                CurrentMemoryOwnership::NoteDerived => counts.note_derived = count,
            }
        }
        let paused: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM memory_note_sets\n\
             WHERE vault_id = ? AND extraction_paused = 1",
        )
        .bind(context.id().to_string())
        .fetch_one(&self.pool)
        .await?;
        counts.paused_sources = u64::try_from(paused)
            .map_err(|_| StateError::InvalidInput("current-memory count is invalid"))?;
        Ok(counts)
    }

    /// Search the isolated current-only FTS projection.
    pub async fn search_fts(
        &self,
        context: &VaultContext,
        fts_query: &str,
        filter: &CurrentMemoryFilter,
        limit: u32,
    ) -> Result<Vec<CurrentMemorySearchHit>, StateError> {
        validate_page(limit, 0)?;
        if fts_query.is_empty() || fts_query.len() > 16 * 1024 {
            return Err(StateError::InvalidInput(
                "current-memory FTS query is invalid",
            ));
        }
        self.ensure_vault_context(context).await?;
        let mut query = QueryBuilder::<Sqlite>::new(
            "SELECT i.id, i.vault_id, i.ownership, i.note_set_id, i.ordinal, i.kind,\n\
                    i.content, i.normalized_content, i.content_hash, i.importance,\n\
                    i.confidence, i.origin, i.revision, i.canonical_file_id,\n\
                    i.canonical_path, i.canonical_revision, i.valid_from, i.valid_to,\n\
                    i.tags_json, i.entities_json, i.metadata_json, i.created_at,\n\
                    i.updated_at, i.last_recalled_at, i.recall_count,\n\
                    bm25(memory_current_fts) AS memory_rank\n\
             FROM memory_current_fts\n\
             JOIN memory_current_items i\n\
               ON i.vault_id = memory_current_fts.vault_id\n\
              AND i.id = memory_current_fts.memory_id\n\
             WHERE memory_current_fts.vault_id = ",
        );
        query.push_bind(context.id().to_string());
        query.push(" AND memory_current_fts MATCH ");
        query.push_bind(fts_query);
        query.push(" AND ");
        query.push(current_eligibility_sql());
        append_filter(&mut query, filter);
        query.push(" ORDER BY memory_rank ASC, i.id ASC LIMIT ");
        query.push_bind(i64::from(limit));
        let rows = query
            .build_query_as::<CurrentMemorySearchRow>()
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter()
            .map(|row| {
                Ok(CurrentMemorySearchHit {
                    memory: row_to_item(row.item)?,
                    rank: row.memory_rank,
                })
            })
            .collect()
    }

    /// Match explicit context entities/tags without admitting unrelated recent
    /// memories merely because they exist.
    pub async fn search_terms(
        &self,
        context: &VaultContext,
        entities: &[String],
        tags: &[String],
        filter: &CurrentMemoryFilter,
        limit: u32,
    ) -> Result<Vec<CurrentMemoryRecord>, StateError> {
        validate_page(limit, 0)?;
        let entities = normalized_terms(entities);
        let tags = normalized_terms(tags);
        if entities.is_empty() && tags.is_empty() {
            return Ok(Vec::new());
        }
        self.ensure_vault_context(context).await?;
        let mut query = QueryBuilder::<Sqlite>::new(item_select());
        query.push(" WHERE i.vault_id = ");
        query.push_bind(context.id().to_string());
        query.push(" AND ");
        query.push(current_eligibility_sql());
        query.push(" AND (");
        let mut first = true;
        for entity in &entities {
            if !first {
                query.push(" OR ");
            }
            first = false;
            query.push(
                "EXISTS (SELECT 1 FROM json_each(i.entities_json) value\n\
                         WHERE lower(trim(CAST(value.value AS TEXT))) = ",
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
                "EXISTS (SELECT 1 FROM json_each(i.tags_json) value\n\
                         WHERE lower(trim(CAST(value.value AS TEXT))) = ",
            );
            query.push_bind(tag);
            query.push(")");
        }
        query.push(")");
        append_filter(&mut query, filter);
        query.push(" ORDER BY i.updated_at DESC, i.id ASC LIMIT ");
        query.push_bind(i64::from(limit));
        let rows = query
            .build_query_as::<CurrentMemoryRow>()
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter().map(row_to_item).collect()
    }

    /// Insert or replace an explicit item after Vault Core has durably written
    /// the exact canonical revision. This is the only direct-memory publish
    /// operation and uses optimistic revision comparison.
    pub async fn publish_explicit(
        &self,
        context: &VaultContext,
        bundle: &CurrentMemoryBundle,
        expected_revision: Option<Revision>,
        idempotency: Option<(&str, &str)>,
    ) -> Result<CurrentMemoryBundle, StateError> {
        self.ensure_vault_context(context).await?;
        validate_current_bundle(context, bundle)?;
        if bundle.memory.ownership != CurrentMemoryOwnership::Explicit || bundle.note_set.is_some()
        {
            return Err(StateError::InvalidInput(
                "explicit publication ownership is invalid",
            ));
        }
        let current = self.get_unchecked(context, bundle.memory.id).await?;
        match (current.as_ref(), expected_revision) {
            (None, None) => {
                if bundle.memory.revision != Revision::new(1) {
                    return Err(StateError::Conflict);
                }
            }
            (Some(current), Some(expected))
                if current.memory.ownership == CurrentMemoryOwnership::Explicit
                    && current.memory.revision == expected
                    && bundle.memory.revision == expected.next()? => {}
            _ => return Err(StateError::Conflict),
        }
        self.ensure_explicit_canonical_current(context, &bundle.memory)
            .await?;
        let mut transaction = self.pool.begin().await?;
        delete_item_fts(&mut transaction, context.id(), bundle.memory.id).await?;
        upsert_item(&mut transaction, &bundle.memory).await?;
        replace_sources(
            &mut transaction,
            context.id(),
            bundle.memory.id,
            &bundle.sources,
        )
        .await?;
        insert_item_fts(&mut transaction, &bundle.memory).await?;
        if let Some((key, request_hash)) = idempotency {
            if key.trim().is_empty() || key.len() > 256 || request_hash.trim().is_empty() {
                return Err(StateError::InvalidInput(
                    "current-memory idempotency input is invalid",
                ));
            }
            sqlx::query(
                "INSERT INTO memory_current_idempotency\n\
                 (vault_id, idempotency_key, request_hash, memory_id, created_at)\n\
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(context.id().to_string())
            .bind(key)
            .bind(request_hash)
            .bind(bundle.memory.id.to_string())
            .bind(bundle.memory.created_at)
            .execute(&mut *transaction)
            .await?;
            sqlx::query(
                "DELETE FROM memory_current_explicit_reservations\n\
                 WHERE vault_id = ? AND idempotency_key = ?\n\
                   AND request_hash = ? AND memory_id = ?",
            )
            .bind(context.id().to_string())
            .bind(key)
            .bind(request_hash)
            .bind(bundle.memory.id.to_string())
            .execute(&mut *transaction)
            .await?;
        }
        transaction.commit().await?;
        self.get(context, bundle.memory.id)
            .await?
            .ok_or(StateError::Conflict)
    }

    /// Restore an explicit current projection from its already-current
    /// canonical Markdown. This recovery-only path does not require a command
    /// revision transition, but it refuses to change ownership or bypass the
    /// canonical file identity check.
    pub async fn restore_explicit_projection(
        &self,
        context: &VaultContext,
        bundle: &CurrentMemoryBundle,
    ) -> Result<CurrentMemoryBundle, StateError> {
        self.ensure_vault_context(context).await?;
        validate_current_bundle(context, bundle)?;
        if bundle.memory.ownership != CurrentMemoryOwnership::Explicit || bundle.note_set.is_some()
        {
            return Err(StateError::InvalidInput(
                "explicit recovery ownership is invalid",
            ));
        }
        if self
            .get_unchecked(context, bundle.memory.id)
            .await?
            .is_some_and(|current| current.memory.ownership != CurrentMemoryOwnership::Explicit)
        {
            return Err(StateError::Conflict);
        }
        self.ensure_explicit_canonical_current(context, &bundle.memory)
            .await?;
        let mut transaction = self.pool.begin().await?;
        delete_item_fts(&mut transaction, context.id(), bundle.memory.id).await?;
        upsert_item(&mut transaction, &bundle.memory).await?;
        replace_sources(
            &mut transaction,
            context.id(),
            bundle.memory.id,
            &bundle.sources,
        )
        .await?;
        insert_item_fts(&mut transaction, &bundle.memory).await?;
        transaction.commit().await?;
        self.get(context, bundle.memory.id)
            .await?
            .ok_or(StateError::Conflict)
    }

    /// Resolve an explicit-command idempotency key without exposing item
    /// content. The caller compares the request hash before returning the item.
    pub async fn explicit_idempotency(
        &self,
        context: &VaultContext,
        key: &str,
    ) -> Result<Option<(String, MemoryId)>, StateError> {
        if key.trim().is_empty() || key.len() > 256 {
            return Err(StateError::InvalidInput(
                "current-memory idempotency key is invalid",
            ));
        }
        self.ensure_vault_context(context).await?;
        let row = sqlx::query_as::<_, (String, String)>(
            "SELECT request_hash, memory_id FROM memory_current_idempotency\n\
             WHERE vault_id = ? AND idempotency_key = ?",
        )
        .bind(context.id().to_string())
        .bind(key)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|(request_hash, memory_id)| Ok((request_hash, MemoryId::parse(&memory_id)?)))
            .transpose()
    }

    /// Allocate or recover an explicit-memory identity before the canonical
    /// file write. Reusing a key for different input fails closed.
    pub async fn reserve_explicit(
        &self,
        context: &VaultContext,
        key: &str,
        request_hash: &str,
    ) -> Result<CurrentExplicitReservation, StateError> {
        if key.trim().is_empty()
            || key.len() > 256
            || request_hash.trim().is_empty()
            || request_hash.len() > 256
        {
            return Err(StateError::InvalidInput(
                "current-memory reservation input is invalid",
            ));
        }
        self.ensure_vault_context(context).await?;
        let proposed = CurrentExplicitReservation {
            request_hash: request_hash.to_owned(),
            memory_id: MemoryId::new(),
            created_at: now_millis()?,
        };
        sqlx::query(
            "INSERT INTO memory_current_explicit_reservations\n\
             (vault_id, idempotency_key, request_hash, memory_id, created_at)\n\
             VALUES (?, ?, ?, ?, ?)\n\
             ON CONFLICT(vault_id, idempotency_key) DO NOTHING",
        )
        .bind(context.id().to_string())
        .bind(key)
        .bind(request_hash)
        .bind(proposed.memory_id.to_string())
        .bind(proposed.created_at)
        .execute(&self.pool)
        .await?;
        let (stored_hash, memory_id, created_at) = sqlx::query_as::<_, (String, String, i64)>(
            "SELECT request_hash, memory_id, created_at\n\
             FROM memory_current_explicit_reservations\n\
             WHERE vault_id = ? AND idempotency_key = ?",
        )
        .bind(context.id().to_string())
        .bind(key)
        .fetch_one(&self.pool)
        .await?;
        if stored_hash != request_hash {
            return Err(StateError::Conflict);
        }
        Ok(CurrentExplicitReservation {
            request_hash: stored_hash,
            memory_id: MemoryId::parse(&memory_id)?,
            created_at,
        })
    }

    /// Delete one explicit current projection after its canonical file is no
    /// longer current. The item body is never returned by this operation.
    pub async fn delete_explicit_projection(
        &self,
        context: &VaultContext,
        memory_id: MemoryId,
        expected_revision: Revision,
    ) -> Result<bool, StateError> {
        let current = self
            .get_unchecked(context, memory_id)
            .await?
            .ok_or(StateError::Conflict)?;
        if current.memory.ownership != CurrentMemoryOwnership::Explicit
            || current.memory.revision != expected_revision
        {
            return Err(StateError::Conflict);
        }
        let mut transaction = self.pool.begin().await?;
        delete_item_fts(&mut transaction, context.id(), memory_id).await?;
        let result = sqlx::query(
            "DELETE FROM memory_current_items\n\
             WHERE vault_id = ? AND id = ? AND ownership = 'explicit' AND revision = ?",
        )
        .bind(context.id().to_string())
        .bind(memory_id.to_string())
        .bind(expected_revision.as_i64()?)
        .execute(&mut *transaction)
        .await?;
        if result.rows_affected() != 1 {
            return Err(StateError::Conflict);
        }
        transaction.commit().await?;
        Ok(true)
    }

    /// Return the one set owned by a stable source File ID, including an
    /// invalidated set so reconciliation can replace or remove it.
    pub async fn get_note_set_by_source(
        &self,
        context: &VaultContext,
        source_file_id: FileId,
    ) -> Result<Option<MemoryNoteSetRecord>, StateError> {
        self.ensure_vault_context(context).await?;
        let row = sqlx::query_as::<_, MemoryNoteSetRow>(&format!(
            "{} WHERE s.vault_id = ? AND s.source_file_id = ?",
            set_select()
        ))
        .bind(context.id().to_string())
        .bind(source_file_id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_set).transpose()
    }

    /// Return one set by its Vault-scoped identity.
    pub async fn get_note_set(
        &self,
        context: &VaultContext,
        set_id: MemorySetId,
    ) -> Result<Option<MemoryNoteSetRecord>, StateError> {
        self.ensure_vault_context(context).await?;
        let row = sqlx::query_as::<_, MemoryNoteSetRow>(&format!(
            "{} WHERE s.vault_id = ? AND s.id = ?",
            set_select()
        ))
        .bind(context.id().to_string())
        .bind(set_id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_set).transpose()
    }

    /// Return every item in a set for deterministic rewrites and recovery.
    pub async fn list_note_set_items(
        &self,
        context: &VaultContext,
        set_id: MemorySetId,
    ) -> Result<Vec<CurrentMemoryBundle>, StateError> {
        self.ensure_vault_context(context).await?;
        let sql = format!(
            "{} WHERE i.vault_id = ? AND i.note_set_id = ? ORDER BY i.ordinal, i.id",
            item_select()
        );
        let rows = sqlx::query_as::<_, CurrentMemoryRow>(&sql)
            .bind(context.id().to_string())
            .bind(set_id.to_string())
            .fetch_all(&self.pool)
            .await?;
        let note_set = self
            .get_note_set(context, set_id)
            .await?
            .ok_or(StateError::Conflict)?;
        let mut bundles = Vec::with_capacity(rows.len());
        for row in rows {
            let memory = row_to_item(row)?;
            let sources = self.list_sources(context, memory.id).await?;
            bundles.push(CurrentMemoryBundle {
                memory,
                sources,
                note_set: Some(note_set.clone()),
            });
        }
        Ok(bundles)
    }

    /// Persist one validated prepared snapshot. A partial unique index permits
    /// at most one recoverable proposal per source.
    pub async fn prepare_note_set_snapshot(
        &self,
        context: &VaultContext,
        snapshot: &MemoryNoteSetSnapshotRecord,
    ) -> Result<(), StateError> {
        self.ensure_vault_context(context).await?;
        validate_snapshot(context, snapshot)?;
        sqlx::query(
            "INSERT INTO memory_note_set_snapshots\n\
             (id, vault_id, note_set_id, source_file_id, source_path,\n\
              source_content_hash, source_revision, expected_set_revision,\n\
              proposed_set_revision, extraction_paused, items_json, canonical_bytes_hash,\n\
              canonical_path, profile_hash, prompt_version, provider_id, model_id,\n\
              status, created_at, applied_at)\n\
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'prepared', ?, NULL)",
        )
        .bind(snapshot.id.to_string())
        .bind(context.id().to_string())
        .bind(snapshot.note_set_id.to_string())
        .bind(snapshot.source_file_id.to_string())
        .bind(snapshot.source_path.as_str())
        .bind(&snapshot.source_content_hash)
        .bind(snapshot.source_revision.as_i64()?)
        .bind(
            snapshot
                .expected_set_revision
                .map(Revision::as_i64)
                .transpose()?,
        )
        .bind(snapshot.proposed_set_revision.as_i64()?)
        .bind(i64::from(snapshot.extraction_paused))
        .bind(serde_json::to_string(&snapshot.items)?)
        .bind(&snapshot.canonical_bytes_hash)
        .bind(snapshot.canonical_path.as_str())
        .bind(&snapshot.profile_hash)
        .bind(&snapshot.prompt_version)
        .bind(snapshot.provider_id.to_string())
        .bind(snapshot.model_id.to_string())
        .bind(snapshot.created_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Return the prepared snapshot for one exact source, if any.
    pub async fn prepared_note_set_snapshot(
        &self,
        context: &VaultContext,
        source_file_id: FileId,
    ) -> Result<Option<MemoryNoteSetSnapshotRecord>, StateError> {
        self.ensure_vault_context(context).await?;
        let row = sqlx::query_as::<_, MemoryNoteSetSnapshotRow>(&format!(
            "{} WHERE p.vault_id = ? AND p.source_file_id = ? AND p.status = 'prepared'\n\
             ORDER BY p.created_at, p.id LIMIT 1",
            snapshot_select()
        ))
        .bind(context.id().to_string())
        .bind(source_file_id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_snapshot).transpose()
    }

    /// Reject an obsolete prepared proposal without deleting audit evidence.
    pub async fn reject_note_set_snapshot(
        &self,
        context: &VaultContext,
        snapshot_id: MemorySetSnapshotId,
    ) -> Result<bool, StateError> {
        let result = sqlx::query(
            "UPDATE memory_note_set_snapshots SET status = 'rejected'\n\
             WHERE vault_id = ? AND id = ? AND status = 'prepared'",
        )
        .bind(context.id().to_string())
        .bind(snapshot_id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    /// Atomically publish a canonicalized source-owned set and replace every
    /// previously current item for that source.
    pub async fn publish_note_set(
        &self,
        context: &VaultContext,
        snapshot_id: MemorySetSnapshotId,
        note_set: &MemoryNoteSetRecord,
        items: &[CurrentMemoryBundle],
    ) -> Result<Vec<CurrentMemoryBundle>, StateError> {
        self.ensure_vault_context(context).await?;
        validate_note_set(context, note_set)?;
        for item in items {
            validate_current_bundle(context, item)?;
            if item.memory.ownership != CurrentMemoryOwnership::NoteDerived
                || item.memory.note_set_id != Some(note_set.id)
            {
                return Err(StateError::InvalidInput(
                    "note-set item ownership is invalid",
                ));
            }
        }
        self.ensure_note_set_files_current(context, note_set)
            .await?;
        let snapshot = self
            .prepared_note_set_snapshot(context, note_set.source_file_id)
            .await?
            .filter(|snapshot| snapshot.id == snapshot_id)
            .ok_or(StateError::Conflict)?;
        if snapshot.note_set_id != note_set.id
            || snapshot.source_content_hash != note_set.source_content_hash
            || snapshot.source_revision != note_set.source_revision
            || snapshot.proposed_set_revision != note_set.set_revision
            || snapshot.canonical_path != note_set.canonical_path
        {
            return Err(StateError::Conflict);
        }
        let current = self
            .get_note_set_by_source(context, note_set.source_file_id)
            .await?;
        if current.as_ref().map(|set| set.set_revision) != snapshot.expected_set_revision {
            return Err(StateError::Conflict);
        }

        let mut transaction = self.pool.begin().await?;
        delete_set_fts(&mut transaction, context.id(), note_set.id).await?;
        if current.is_some() {
            sqlx::query("DELETE FROM memory_current_items WHERE vault_id = ? AND note_set_id = ?")
                .bind(context.id().to_string())
                .bind(note_set.id.to_string())
                .execute(&mut *transaction)
                .await?;
            update_note_set(&mut transaction, note_set, true).await?;
        } else {
            insert_note_set(&mut transaction, note_set).await?;
        }
        for bundle in items {
            upsert_item(&mut transaction, &bundle.memory).await?;
            replace_sources(
                &mut transaction,
                context.id(),
                bundle.memory.id,
                &bundle.sources,
            )
            .await?;
            insert_item_fts(&mut transaction, &bundle.memory).await?;
        }
        let applied = sqlx::query(
            "UPDATE memory_note_set_snapshots\n\
             SET status = 'applied', applied_at = ?\n\
             WHERE vault_id = ? AND id = ? AND status = 'prepared'",
        )
        .bind(now_millis()?)
        .bind(context.id().to_string())
        .bind(snapshot_id.to_string())
        .execute(&mut *transaction)
        .await?;
        if applied.rows_affected() != 1 {
            return Err(StateError::Conflict);
        }
        transaction.commit().await?;
        self.list_note_set_items(context, note_set.id).await
    }

    /// Restore one complete source-owned projection from already-current
    /// canonical Markdown. It is deliberately separate from normal snapshot
    /// publication and validates the source File ID/hash plus canonical file
    /// identity before atomically replacing the whole set.
    pub async fn restore_note_set_projection(
        &self,
        context: &VaultContext,
        note_set: &MemoryNoteSetRecord,
        items: &[CurrentMemoryBundle],
    ) -> Result<Vec<CurrentMemoryBundle>, StateError> {
        self.ensure_vault_context(context).await?;
        validate_note_set(context, note_set)?;
        for item in items {
            validate_current_bundle(context, item)?;
            if item.memory.ownership != CurrentMemoryOwnership::NoteDerived
                || item.memory.note_set_id != Some(note_set.id)
                || item
                    .note_set
                    .as_ref()
                    .is_none_or(|set| set.id != note_set.id)
            {
                return Err(StateError::InvalidInput(
                    "note-set recovery ownership is invalid",
                ));
            }
        }
        self.ensure_note_set_files_current(context, note_set)
            .await?;
        let current = self
            .get_note_set_by_source(context, note_set.source_file_id)
            .await?;
        if self
            .get_note_set(context, note_set.id)
            .await?
            .is_some_and(|stored| stored.source_file_id != note_set.source_file_id)
        {
            return Err(StateError::Conflict);
        }
        for item in items {
            if self
                .get_unchecked(context, item.memory.id)
                .await?
                .is_some_and(|stored| {
                    stored.memory.ownership != CurrentMemoryOwnership::NoteDerived
                        || !matches!(
                            stored.memory.note_set_id,
                            Some(set_id)
                                if set_id == note_set.id
                                    || current.as_ref().is_some_and(|set| set.id == set_id)
                        )
                })
            {
                return Err(StateError::Conflict);
            }
        }

        let mut transaction = self.pool.begin().await?;
        if let Some(current) = current.as_ref() {
            delete_set_fts(&mut transaction, context.id(), current.id).await?;
            sqlx::query("DELETE FROM memory_current_items WHERE vault_id = ? AND note_set_id = ?")
                .bind(context.id().to_string())
                .bind(current.id.to_string())
                .execute(&mut *transaction)
                .await?;
            if current.id == note_set.id {
                update_note_set(&mut transaction, note_set, false).await?;
            } else {
                sqlx::query("DELETE FROM memory_note_sets WHERE vault_id = ? AND id = ?")
                    .bind(context.id().to_string())
                    .bind(current.id.to_string())
                    .execute(&mut *transaction)
                    .await?;
                insert_note_set(&mut transaction, note_set).await?;
            }
        } else {
            insert_note_set(&mut transaction, note_set).await?;
        }
        for bundle in items {
            upsert_item(&mut transaction, &bundle.memory).await?;
            replace_sources(
                &mut transaction,
                context.id(),
                bundle.memory.id,
                &bundle.sources,
            )
            .await?;
            insert_item_fts(&mut transaction, &bundle.memory).await?;
        }
        sqlx::query(
            "UPDATE memory_note_set_snapshots SET status = 'rejected', applied_at = ?
             WHERE vault_id = ? AND source_file_id = ? AND status = 'prepared'",
        )
        .bind(now_millis()?)
        .bind(context.id().to_string())
        .bind(note_set.source_file_id.to_string())
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        self.list_note_set_items(context, note_set.id).await
    }

    /// Atomically rewrite a note-owned set after a manual item deletion and
    /// pause automatic extraction for the source. Canonical bytes must already
    /// have been committed through Vault Core.
    pub async fn delete_note_item_and_pause(
        &self,
        context: &VaultContext,
        memory_id: MemoryId,
        expected_memory_revision: Revision,
        updated_set: &MemoryNoteSetRecord,
    ) -> Result<(), StateError> {
        self.ensure_vault_context(context).await?;
        validate_note_set(context, updated_set)?;
        if !updated_set.extraction_paused {
            return Err(StateError::InvalidInput(
                "manual note-memory deletion must pause extraction",
            ));
        }
        self.ensure_note_set_files_current(context, updated_set)
            .await?;
        let item = self
            .get_unchecked(context, memory_id)
            .await?
            .ok_or(StateError::Conflict)?;
        let old_set = item.note_set.ok_or(StateError::Conflict)?;
        if item.memory.ownership != CurrentMemoryOwnership::NoteDerived
            || item.memory.revision != expected_memory_revision
            || old_set.id != updated_set.id
            || updated_set.set_revision != old_set.set_revision.next()?
        {
            return Err(StateError::Conflict);
        }
        let mut transaction = self.pool.begin().await?;
        delete_item_fts(&mut transaction, context.id(), memory_id).await?;
        let deleted = sqlx::query(
            "DELETE FROM memory_current_items\n\
             WHERE vault_id = ? AND id = ? AND note_set_id = ? AND revision = ?",
        )
        .bind(context.id().to_string())
        .bind(memory_id.to_string())
        .bind(updated_set.id.to_string())
        .bind(expected_memory_revision.as_i64()?)
        .execute(&mut *transaction)
        .await?;
        if deleted.rows_affected() != 1 {
            return Err(StateError::Conflict);
        }
        update_note_set(&mut transaction, updated_set, true).await?;
        transaction.commit().await?;
        Ok(())
    }

    /// Resume automatic extraction explicitly. The set revision is compared so
    /// an operator cannot accidentally unpause a newer rewrite.
    pub async fn resume_note_extraction(
        &self,
        context: &VaultContext,
        updated_set: &MemoryNoteSetRecord,
        expected_set_revision: Revision,
    ) -> Result<bool, StateError> {
        validate_note_set(context, updated_set)?;
        if updated_set.extraction_paused
            || updated_set.set_revision != expected_set_revision.next()?
        {
            return Err(StateError::Conflict);
        }
        self.ensure_note_set_canonical_current(context, updated_set)
            .await?;
        let current = self
            .get_note_set(context, updated_set.id)
            .await?
            .filter(|set| {
                set.source_file_id == updated_set.source_file_id
                    && set.set_revision == expected_set_revision
                    && set.extraction_paused
            })
            .ok_or(StateError::Conflict)?;
        let _ = current;
        let mut transaction = self.pool.begin().await?;
        update_note_set(&mut transaction, updated_set, true).await?;
        transaction.commit().await?;
        Ok(true)
    }

    /// Update only the navigable source path after a same-File-ID, same-hash
    /// move. This never changes extraction coverage or calls a model.
    pub async fn move_note_set_source(
        &self,
        context: &VaultContext,
        updated_set: &MemoryNoteSetRecord,
        expected_set_revision: Revision,
    ) -> Result<bool, StateError> {
        validate_note_set(context, updated_set)?;
        if updated_set.set_revision != expected_set_revision.next()? {
            return Err(StateError::Conflict);
        }
        self.ensure_note_set_files_current(context, updated_set)
            .await?;
        self.get_note_set(context, updated_set.id)
            .await?
            .filter(|set| {
                set.source_file_id == updated_set.source_file_id
                    && set.source_content_hash == updated_set.source_content_hash
                    && set.set_revision == expected_set_revision
                    && set.extraction_paused == updated_set.extraction_paused
            })
            .ok_or(StateError::Conflict)?;
        let mut transaction = self.pool.begin().await?;
        update_note_set(&mut transaction, updated_set, true).await?;
        transaction.commit().await?;
        Ok(true)
    }

    /// Delete one source-owned projection after Vault Core has deleted its
    /// canonical set file. A retry may call this after the file is already
    /// absent; the optimistic set revision still prevents deleting a newer
    /// replacement.
    pub async fn delete_note_set_projection(
        &self,
        context: &VaultContext,
        source_file_id: FileId,
        expected_set_revision: Revision,
    ) -> Result<bool, StateError> {
        self.ensure_vault_context(context).await?;
        let set = self
            .get_note_set_by_source(context, source_file_id)
            .await?
            .filter(|set| set.set_revision == expected_set_revision)
            .ok_or(StateError::Conflict)?;
        let mut transaction = self.pool.begin().await?;
        delete_set_fts(&mut transaction, context.id(), set.id).await?;
        let result = sqlx::query(
            "DELETE FROM memory_note_sets\n\
             WHERE vault_id = ? AND id = ? AND set_revision = ?",
        )
        .bind(context.id().to_string())
        .bind(set.id.to_string())
        .bind(expected_set_revision.as_i64()?)
        .execute(&mut *transaction)
        .await?;
        if result.rows_affected() != 1 {
            return Err(StateError::Conflict);
        }
        transaction.commit().await?;
        Ok(true)
    }

    /// Record successful recalls without affecting retrieval eligibility.
    pub async fn mark_recalled(
        &self,
        context: &VaultContext,
        memory_ids: &[MemoryId],
    ) -> Result<(), StateError> {
        if memory_ids.is_empty() {
            return Ok(());
        }
        let now = now_millis()?;
        let mut transaction = self.pool.begin().await?;
        for memory_id in memory_ids {
            sqlx::query(
                "UPDATE memory_current_items\n\
                 SET last_recalled_at = ?, recall_count = recall_count + 1\n\
                 WHERE vault_id = ? AND id = ?",
            )
            .bind(now)
            .bind(context.id().to_string())
            .bind(memory_id.to_string())
            .execute(&mut *transaction)
            .await?;
        }
        transaction.commit().await?;
        Ok(())
    }

    /// Classify legacy records and persist a non-sensitive operator report.
    /// This method never changes either legacy or current memory content.
    pub async fn migration_preflight(
        &self,
        context: &VaultContext,
    ) -> Result<MemoryV2MigrationPreflight, StateError> {
        self.ensure_vault_context(context).await?;
        let digest_rows = sqlx::query_as::<_, LegacyMigrationDigestRow>(
            "SELECT record_kind, record_key, record_json FROM (\n\
               SELECT 'memory' AS record_kind, id AS record_key,\n\
                      json_array(id, memory_type, status, status_reason, status_changed_at,\n\
                                 content, normalized_content, content_hash, importance, confidence,\n\
                                 origin, revision, canonical_file_id, canonical_path,\n\
                                 canonical_revision, valid_from, valid_to, extraction_json,\n\
                                 created_at, updated_at, last_recalled_at, recall_count) AS record_json\n\
               FROM memories WHERE vault_id = ?\n\
               UNION ALL\n\
               SELECT 'source', id,\n\
                      json_array(id, memory_id, source_type, note_file_id, note_path, note_revision,\n\
                                 heading_path_json, start_line, end_line, excerpt_hash, actor_id,\n\
                                 created_at)\n\
               FROM memory_sources WHERE vault_id = ?\n\
               UNION ALL\n\
               SELECT 'entity', memory_id || ':' || normalized_entity,\n\
                      json_array(memory_id, entity, normalized_entity)\n\
               FROM memory_entities WHERE vault_id = ?\n\
               UNION ALL\n\
               SELECT 'tag', memory_id || ':' || normalized_tag,\n\
                      json_array(memory_id, tag, normalized_tag)\n\
               FROM memory_tags WHERE vault_id = ?\n\
             ) ORDER BY record_kind, record_key, record_json",
        )
        .bind(context.id().to_string())
        .bind(context.id().to_string())
        .bind(context.id().to_string())
        .bind(context.id().to_string())
        .fetch_all(&self.pool)
        .await?;
        let mut state_hasher = Sha256::new();
        state_hasher.update(b"mcp-vault-memory-v2.1-classified-state\0");
        for row in digest_rows {
            state_hasher.update(serde_json::to_vec(&(
                row.record_kind,
                row.record_key,
                row.record_json,
            ))?);
            state_hasher.update(b"\n");
        }
        let rows = sqlx::query_as::<_, LegacySourceShapeRow>(
            "SELECT m.id, m.status,\n\
                    EXISTS(SELECT 1 FROM memory_sources s\n\
                           WHERE s.vault_id = m.vault_id AND s.memory_id = m.id\n\
                             AND (s.source_type = 'note' OR s.note_file_id IS NOT NULL\n\
                                  OR s.note_path IS NOT NULL OR s.note_revision IS NOT NULL))\n\
                        AS has_note,\n\
                    EXISTS(SELECT 1 FROM memory_sources s\n\
                           WHERE s.vault_id = m.vault_id AND s.memory_id = m.id\n\
                             AND s.source_type IN ('explicit_agent', 'explicit_admin', 'import'))\n\
                        AS has_explicit,\n\
                    EXISTS(SELECT 1 FROM memory_sources s\n\
                           WHERE s.vault_id = m.vault_id AND s.memory_id = m.id\n\
                             AND s.source_type NOT IN\n\
                                 ('note', 'explicit_agent', 'explicit_admin', 'import'))\n\
                        AS has_other\n\
             FROM memories m WHERE m.vault_id = ? ORDER BY m.id",
        )
        .bind(context.id().to_string())
        .fetch_all(&self.pool)
        .await?;
        let mut report = MemoryV2MigrationPreflight {
            classified_state_hash: format!("sha256:{:x}", state_hasher.finalize()),
            ..MemoryV2MigrationPreflight::default()
        };
        for row in rows {
            report.legacy_total += 1;
            if row.status != "active" {
                report.historical += 1;
                continue;
            }
            match (row.has_note != 0, row.has_explicit != 0, row.has_other != 0) {
                (false, true, false) => report.safe_explicit += 1,
                (true, false, false) => report.note_derived += 1,
                (true, true, _) => {
                    report.mixed_source += 1;
                    report.mixed_source_ids.push(row.id);
                }
                _ => {
                    report.unsupported += 1;
                    report.unsupported_ids.push(row.id);
                }
            }
        }
        report.mixed_source_ids.sort();
        report.unsupported_ids.sort();
        let now = now_millis()?;
        let report_json = serde_json::json!({
            "classified_state_hash": &report.classified_state_hash,
            "legacy_total": report.legacy_total,
            "historical": report.historical,
            "safe_explicit": report.safe_explicit,
            "note_derived": report.note_derived,
            "mixed_source": report.mixed_source,
            "unsupported": report.unsupported,
            "mixed_source_ids": &report.mixed_source_ids,
            "unsupported_ids": &report.unsupported_ids,
            "content_included": false,
        });
        sqlx::query(
            "INSERT INTO memory_v2_migration_state\n\
             (vault_id, status, legacy_total, historical, safe_explicit, note_derived, mixed_source,\n\
              unsupported, report_json, preflighted_at, completed_at, updated_at)\n\
             VALUES (?, 'preflighted', ?, ?, ?, ?, ?, ?, ?, ?, NULL, ?)\n\
             ON CONFLICT(vault_id) DO UPDATE SET\n\
              status = 'preflighted', legacy_total = excluded.legacy_total,\n\
              historical = excluded.historical,\n\
              safe_explicit = excluded.safe_explicit, note_derived = excluded.note_derived,\n\
              mixed_source = excluded.mixed_source, unsupported = excluded.unsupported,\n\
              report_json = excluded.report_json, preflighted_at = excluded.preflighted_at,\n\
              completed_at = NULL, updated_at = excluded.updated_at",
        )
        .bind(context.id().to_string())
        .bind(
            i64::try_from(report.legacy_total)
                .map_err(|_| StateError::InvalidInput("legacy memory count is invalid"))?,
        )
        .bind(
            i64::try_from(report.historical)
                .map_err(|_| StateError::InvalidInput("legacy memory count is invalid"))?,
        )
        .bind(
            i64::try_from(report.safe_explicit)
                .map_err(|_| StateError::InvalidInput("legacy memory count is invalid"))?,
        )
        .bind(
            i64::try_from(report.note_derived)
                .map_err(|_| StateError::InvalidInput("legacy memory count is invalid"))?,
        )
        .bind(
            i64::try_from(report.mixed_source)
                .map_err(|_| StateError::InvalidInput("legacy memory count is invalid"))?,
        )
        .bind(
            i64::try_from(report.unsupported)
                .map_err(|_| StateError::InvalidInput("legacy memory count is invalid"))?,
        )
        .bind(serde_json::to_string(&report_json)?)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(report)
    }

    /// Mark an explicitly authorized one-time migration outcome. This only
    /// updates the content-free operator report; it never deletes legacy rows.
    pub async fn finish_migration(
        &self,
        context: &VaultContext,
        completed: bool,
        report: &Value,
    ) -> Result<(), StateError> {
        self.ensure_vault_context(context).await?;
        let now = now_millis()?;
        let result = sqlx::query(
            "UPDATE memory_v2_migration_state\n\
             SET status = ?, report_json = ?, completed_at = ?, updated_at = ?\n\
             WHERE vault_id = ? AND preflighted_at IS NOT NULL",
        )
        .bind(if completed { "completed" } else { "blocked" })
        .bind(serde_json::to_string(report)?)
        .bind(completed.then_some(now))
        .bind(now)
        .bind(context.id().to_string())
        .execute(&self.pool)
        .await?;
        if result.rows_affected() != 1 {
            return Err(StateError::Conflict);
        }
        Ok(())
    }

    async fn bundle_from_optional_row(
        &self,
        context: &VaultContext,
        row: Option<CurrentMemoryRow>,
    ) -> Result<Option<CurrentMemoryBundle>, StateError> {
        let Some(row) = row else {
            return Ok(None);
        };
        let memory = row_to_item(row)?;
        let sources = self.list_sources(context, memory.id).await?;
        let note_set = match memory.note_set_id {
            Some(set_id) => self.get_note_set(context, set_id).await?,
            None => None,
        };
        Ok(Some(CurrentMemoryBundle {
            memory,
            sources,
            note_set,
        }))
    }

    async fn list_sources(
        &self,
        context: &VaultContext,
        memory_id: MemoryId,
    ) -> Result<Vec<CurrentMemorySourceRecord>, StateError> {
        let rows = sqlx::query_as::<_, CurrentMemorySourceRow>(
            "SELECT s.id, s.vault_id, s.memory_id, s.source_type, s.note_file_id,\n\
                    COALESCE(f.path, s.note_path) AS note_path, s.note_revision,\n\
                    s.source_content_hash, s.heading_path_json, s.start_line, s.end_line,\n\
                    s.excerpt_hash, s.actor_id, s.created_at\n\
             FROM memory_current_sources s\n\
             LEFT JOIN file_entries f ON f.vault_id = s.vault_id\n\
                                     AND f.id = s.note_file_id\n\
                                     AND f.deleted_at IS NULL\n\
             WHERE s.vault_id = ? AND s.memory_id = ?\n\
             ORDER BY s.created_at, s.id",
        )
        .bind(context.id().to_string())
        .bind(memory_id.to_string())
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_source).collect()
    }

    async fn ensure_explicit_canonical_current(
        &self,
        context: &VaultContext,
        memory: &CurrentMemoryRecord,
    ) -> Result<(), StateError> {
        let (Some(file_id), Some(path), Some(revision)) = (
            memory.canonical_file_id,
            memory.canonical_path.as_ref(),
            memory.canonical_revision,
        ) else {
            return Err(StateError::InvalidInput(
                "explicit canonical metadata is missing",
            ));
        };
        let exists: i64 = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM file_entries\n\
             WHERE vault_id = ? AND id = ? AND path = ? AND current_revision = ?\n\
               AND deleted_at IS NULL)",
        )
        .bind(context.id().to_string())
        .bind(file_id.to_string())
        .bind(path.as_str())
        .bind(revision.as_i64()?)
        .fetch_one(&self.pool)
        .await?;
        if exists != 1 {
            return Err(StateError::Conflict);
        }
        Ok(())
    }

    async fn ensure_note_set_files_current(
        &self,
        context: &VaultContext,
        note_set: &MemoryNoteSetRecord,
    ) -> Result<(), StateError> {
        let exists: i64 = sqlx::query_scalar(
            "SELECT EXISTS(\n\
                SELECT 1 FROM file_entries source\n\
                JOIN file_entries canonical ON canonical.vault_id = source.vault_id\n\
                WHERE source.vault_id = ? AND source.id = ?\n\
                  AND source.content_hash = ? AND source.deleted_at IS NULL\n\
                  AND canonical.id = ? AND canonical.path = ?\n\
                  AND canonical.current_revision = ? AND canonical.deleted_at IS NULL\n\
             )",
        )
        .bind(context.id().to_string())
        .bind(note_set.source_file_id.to_string())
        .bind(&note_set.source_content_hash)
        .bind(note_set.canonical_file_id.to_string())
        .bind(note_set.canonical_path.as_str())
        .bind(note_set.canonical_revision.as_i64()?)
        .fetch_one(&self.pool)
        .await?;
        if exists != 1 {
            return Err(StateError::Conflict);
        }
        Ok(())
    }

    async fn ensure_note_set_canonical_current(
        &self,
        context: &VaultContext,
        note_set: &MemoryNoteSetRecord,
    ) -> Result<(), StateError> {
        let exists: i64 = sqlx::query_scalar(
            "SELECT EXISTS(\n\
                SELECT 1 FROM file_entries source\n\
                JOIN file_entries canonical ON canonical.vault_id = source.vault_id\n\
                WHERE source.vault_id = ? AND source.id = ?\n\
                  AND source.deleted_at IS NULL\n\
                  AND canonical.id = ? AND canonical.path = ?\n\
                  AND canonical.current_revision = ? AND canonical.deleted_at IS NULL\n\
             )",
        )
        .bind(context.id().to_string())
        .bind(note_set.source_file_id.to_string())
        .bind(note_set.canonical_file_id.to_string())
        .bind(note_set.canonical_path.as_str())
        .bind(note_set.canonical_revision.as_i64()?)
        .fetch_one(&self.pool)
        .await?;
        if exists != 1 {
            return Err(StateError::Conflict);
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

fn item_select() -> &'static str {
    "SELECT i.id, i.vault_id, i.ownership, i.note_set_id, i.ordinal, i.kind,\n\
            i.content, i.normalized_content, i.content_hash, i.importance,\n\
            i.confidence, i.origin, i.revision, i.canonical_file_id,\n\
            i.canonical_path, i.canonical_revision, i.valid_from, i.valid_to,\n\
            i.tags_json, i.entities_json, i.metadata_json, i.created_at, i.updated_at,\n\
            i.last_recalled_at, i.recall_count\n\
     FROM memory_current_items i"
}

fn set_select() -> &'static str {
    "SELECT s.id, s.vault_id, s.source_file_id, s.source_path,\n\
            s.source_content_hash, s.source_revision, s.set_revision,\n\
            s.extraction_paused, s.canonical_file_id, s.canonical_path,\n\
            s.canonical_revision, s.profile_hash, s.prompt_version,\n\
            s.provider_id, s.model_id, s.created_at, s.updated_at\n\
     FROM memory_note_sets s"
}

fn snapshot_select() -> &'static str {
    "SELECT p.id, p.vault_id, p.note_set_id, p.source_file_id, p.source_path,\n\
            p.source_content_hash, p.source_revision, p.expected_set_revision,\n\
            p.proposed_set_revision, p.extraction_paused, p.items_json,\n\
            p.canonical_bytes_hash,\n\
            p.canonical_path, p.profile_hash, p.prompt_version, p.provider_id,\n\
            p.model_id, p.status, p.created_at, p.applied_at\n\
     FROM memory_note_set_snapshots p"
}

/// SQL eligibility is intentionally embedded in every public current read.
/// A note move keeps the same File ID/hash and remains eligible; a content
/// change, deletion, missing canonical file, or half-published rewrite fails
/// closed immediately.
fn current_eligibility_sql() -> &'static str {
    "(\n\
        (i.ownership = 'explicit' AND EXISTS (\n\
            SELECT 1 FROM file_entries canonical\n\
            WHERE canonical.vault_id = i.vault_id\n\
              AND canonical.id = i.canonical_file_id\n\
              AND canonical.path = i.canonical_path\n\
              AND canonical.current_revision = i.canonical_revision\n\
              AND canonical.deleted_at IS NULL\n\
        ))\n\
        OR\n\
        (i.ownership = 'note_derived' AND EXISTS (\n\
            SELECT 1 FROM memory_note_sets s\n\
            JOIN file_entries source\n\
              ON source.vault_id = s.vault_id AND source.id = s.source_file_id\n\
            JOIN file_entries canonical\n\
              ON canonical.vault_id = s.vault_id AND canonical.id = s.canonical_file_id\n\
            WHERE s.vault_id = i.vault_id AND s.id = i.note_set_id\n\
              AND source.deleted_at IS NULL\n\
              AND source.content_hash = s.source_content_hash\n\
              AND canonical.deleted_at IS NULL\n\
              AND canonical.path = s.canonical_path\n\
              AND canonical.current_revision = s.canonical_revision\n\
        ))\n\
     )"
}

fn append_filter<'a>(query: &mut QueryBuilder<'a, Sqlite>, filter: &'a CurrentMemoryFilter) {
    if !filter.kinds.is_empty() {
        query.push(" AND i.kind IN (");
        for (index, kind) in filter.kinds.iter().enumerate() {
            if index != 0 {
                query.push(", ");
            }
            query.push_bind(kind);
        }
        query.push(")");
    }
    if !filter.ownership.is_empty() {
        query.push(" AND i.ownership IN (");
        for (index, ownership) in filter.ownership.iter().enumerate() {
            if index != 0 {
                query.push(", ");
            }
            query.push_bind(ownership.as_str());
        }
        query.push(")");
    }
    if let Some(tag) = filter.tag.as_deref() {
        query.push(
            " AND EXISTS (SELECT 1 FROM json_each(i.tags_json) value\n\
             WHERE lower(trim(CAST(value.value AS TEXT))) = lower(trim(",
        );
        query.push_bind(tag);
        query.push(")))");
    }
    if let Some(entity) = filter.entity.as_deref() {
        query.push(
            " AND EXISTS (SELECT 1 FROM json_each(i.entities_json) value\n\
             WHERE lower(trim(CAST(value.value AS TEXT))) = lower(trim(",
        );
        query.push_bind(entity);
        query.push(")))");
    }
    if let Some(source_path) = filter.source_path.as_deref() {
        query.push(
            " AND EXISTS (SELECT 1 FROM memory_current_sources source\n\
             LEFT JOIN file_entries current_file\n\
               ON current_file.vault_id = source.vault_id\n\
              AND current_file.id = source.note_file_id\n\
              AND current_file.deleted_at IS NULL\n\
             WHERE source.vault_id = i.vault_id AND source.memory_id = i.id\n\
               AND COALESCE(current_file.path, source.note_path) = ",
        );
        query.push_bind(source_path);
        query.push(")");
    }
    if let Some(valid_at) = filter.valid_at {
        query.push(" AND (i.valid_from IS NULL OR i.valid_from <= ");
        query.push_bind(valid_at);
        query.push(") AND (i.valid_to IS NULL OR i.valid_to > ");
        query.push_bind(valid_at);
        query.push(")");
    }
    if let Some(min_importance) = filter.min_importance {
        query.push(" AND COALESCE(i.importance, 0.0) >= ");
        query.push_bind(min_importance);
    }
}

async fn upsert_item(
    transaction: &mut Transaction<'_, Sqlite>,
    item: &CurrentMemoryRecord,
) -> Result<(), StateError> {
    sqlx::query(
        "INSERT INTO memory_current_items\n\
         (id, vault_id, ownership, note_set_id, ordinal, kind, content,\n\
          normalized_content, content_hash, importance, confidence, origin, revision,\n\
          canonical_file_id, canonical_path, canonical_revision, valid_from, valid_to,\n\
          tags_json, entities_json, metadata_json, created_at, updated_at,\n\
          last_recalled_at, recall_count)\n\
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)\n\
         ON CONFLICT(vault_id, id) DO UPDATE SET\n\
          ownership = excluded.ownership, note_set_id = excluded.note_set_id,\n\
          ordinal = excluded.ordinal, kind = excluded.kind, content = excluded.content,\n\
          normalized_content = excluded.normalized_content, content_hash = excluded.content_hash,\n\
          importance = excluded.importance, confidence = excluded.confidence,\n\
          origin = excluded.origin, revision = excluded.revision,\n\
          canonical_file_id = excluded.canonical_file_id,\n\
          canonical_path = excluded.canonical_path,\n\
          canonical_revision = excluded.canonical_revision,\n\
          valid_from = excluded.valid_from, valid_to = excluded.valid_to,\n\
          tags_json = excluded.tags_json, entities_json = excluded.entities_json,\n\
          metadata_json = excluded.metadata_json, updated_at = excluded.updated_at,\n\
          last_recalled_at = excluded.last_recalled_at, recall_count = excluded.recall_count",
    )
    .bind(item.id.to_string())
    .bind(item.vault_id.to_string())
    .bind(item.ownership.as_str())
    .bind(item.note_set_id.map(|id| id.to_string()))
    .bind(item.ordinal.map(i64::from))
    .bind(item.kind.as_deref())
    .bind(&item.content)
    .bind(&item.normalized_content)
    .bind(&item.content_hash)
    .bind(item.importance)
    .bind(item.confidence)
    .bind(&item.origin)
    .bind(item.revision.as_i64()?)
    .bind(item.canonical_file_id.map(|id| id.to_string()))
    .bind(item.canonical_path.as_ref().map(VaultPath::as_str))
    .bind(item.canonical_revision.map(Revision::as_i64).transpose()?)
    .bind(item.valid_from)
    .bind(item.valid_to)
    .bind(serde_json::to_string(&item.tags)?)
    .bind(serde_json::to_string(&item.entities)?)
    .bind(serde_json::to_string(&item.metadata)?)
    .bind(item.created_at)
    .bind(item.updated_at)
    .bind(item.last_recalled_at)
    .bind(
        i64::try_from(item.recall_count)
            .map_err(|_| StateError::InvalidInput("current-memory recall count is invalid"))?,
    )
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn replace_sources(
    transaction: &mut Transaction<'_, Sqlite>,
    vault_id: VaultId,
    memory_id: MemoryId,
    sources: &[CurrentMemorySourceRecord],
) -> Result<(), StateError> {
    sqlx::query("DELETE FROM memory_current_sources WHERE vault_id = ? AND memory_id = ?")
        .bind(vault_id.to_string())
        .bind(memory_id.to_string())
        .execute(&mut **transaction)
        .await?;
    for source in sources {
        if source.vault_id != vault_id || source.memory_id != memory_id {
            return Err(StateError::InvalidInput(
                "current-memory source scope is invalid",
            ));
        }
        sqlx::query(
            "INSERT INTO memory_current_sources\n\
             (id, vault_id, memory_id, source_type, note_file_id, note_path,\n\
              note_revision, source_content_hash, heading_path_json, start_line,\n\
              end_line, excerpt_hash, actor_id, created_at)\n\
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(source.id.to_string())
        .bind(vault_id.to_string())
        .bind(memory_id.to_string())
        .bind(&source.source_type)
        .bind(source.note_file_id.map(|id| id.to_string()))
        .bind(source.note_path.as_ref().map(VaultPath::as_str))
        .bind(source.note_revision.map(Revision::as_i64).transpose()?)
        .bind(source.source_content_hash.as_deref())
        .bind(serde_json::to_string(&source.heading_path)?)
        .bind(source.start_line.map(i64::from))
        .bind(source.end_line.map(i64::from))
        .bind(source.excerpt_hash.as_deref())
        .bind(source.actor_id.as_deref())
        .bind(source.created_at)
        .execute(&mut **transaction)
        .await?;
    }
    Ok(())
}

async fn insert_item_fts(
    transaction: &mut Transaction<'_, Sqlite>,
    item: &CurrentMemoryRecord,
) -> Result<(), StateError> {
    sqlx::query(
        "INSERT INTO memory_current_fts\n\
         (vault_id, memory_id, content, normalized_content, entities, tags, search_terms)\n\
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(item.vault_id.to_string())
    .bind(item.id.to_string())
    .bind(&item.content)
    .bind(&item.normalized_content)
    .bind(item.entities.join(" "))
    .bind(item.tags.join(" "))
    .bind(memory_search_terms(
        std::iter::once(item.content.as_str())
            .chain(item.tags.iter().map(String::as_str))
            .chain(item.entities.iter().map(String::as_str)),
        4_096,
    ))
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn delete_item_fts(
    transaction: &mut Transaction<'_, Sqlite>,
    vault_id: VaultId,
    memory_id: MemoryId,
) -> Result<(), StateError> {
    sqlx::query("DELETE FROM memory_current_fts WHERE vault_id = ? AND memory_id = ?")
        .bind(vault_id.to_string())
        .bind(memory_id.to_string())
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

async fn delete_set_fts(
    transaction: &mut Transaction<'_, Sqlite>,
    vault_id: VaultId,
    set_id: MemorySetId,
) -> Result<(), StateError> {
    sqlx::query(
        "DELETE FROM memory_current_fts\n\
         WHERE vault_id = ? AND memory_id IN (\n\
             SELECT id FROM memory_current_items\n\
             WHERE vault_id = ? AND note_set_id = ?\n\
         )",
    )
    .bind(vault_id.to_string())
    .bind(vault_id.to_string())
    .bind(set_id.to_string())
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn insert_note_set(
    transaction: &mut Transaction<'_, Sqlite>,
    set: &MemoryNoteSetRecord,
) -> Result<(), StateError> {
    sqlx::query(
        "INSERT INTO memory_note_sets\n\
         (id, vault_id, source_file_id, source_path, source_content_hash, source_revision,\n\
          set_revision, extraction_paused, canonical_file_id, canonical_path,\n\
          canonical_revision, profile_hash, prompt_version, provider_id, model_id,\n\
          created_at, updated_at)\n\
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(set.id.to_string())
    .bind(set.vault_id.to_string())
    .bind(set.source_file_id.to_string())
    .bind(set.source_path.as_str())
    .bind(&set.source_content_hash)
    .bind(set.source_revision.as_i64()?)
    .bind(set.set_revision.as_i64()?)
    .bind(i64::from(set.extraction_paused))
    .bind(set.canonical_file_id.to_string())
    .bind(set.canonical_path.as_str())
    .bind(set.canonical_revision.as_i64()?)
    .bind(&set.profile_hash)
    .bind(&set.prompt_version)
    .bind(set.provider_id.map(|id| id.to_string()))
    .bind(set.model_id.map(|id| id.to_string()))
    .bind(set.created_at)
    .bind(set.updated_at)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn update_note_set(
    transaction: &mut Transaction<'_, Sqlite>,
    set: &MemoryNoteSetRecord,
    compare_previous_revision: bool,
) -> Result<(), StateError> {
    let mut query = QueryBuilder::<Sqlite>::new("UPDATE memory_note_sets SET source_path = ");
    query.push_bind(set.source_path.as_str());
    query.push(", source_content_hash = ");
    query.push_bind(&set.source_content_hash);
    query.push(", source_revision = ");
    query.push_bind(set.source_revision.as_i64()?);
    query.push(", set_revision = ");
    query.push_bind(set.set_revision.as_i64()?);
    query.push(", extraction_paused = ");
    query.push_bind(i64::from(set.extraction_paused));
    query.push(", canonical_file_id = ");
    query.push_bind(set.canonical_file_id.to_string());
    query.push(", canonical_path = ");
    query.push_bind(set.canonical_path.as_str());
    query.push(", canonical_revision = ");
    query.push_bind(set.canonical_revision.as_i64()?);
    query.push(", profile_hash = ");
    query.push_bind(&set.profile_hash);
    query.push(", prompt_version = ");
    query.push_bind(&set.prompt_version);
    query.push(", provider_id = ");
    query.push_bind(set.provider_id.map(|id| id.to_string()));
    query.push(", model_id = ");
    query.push_bind(set.model_id.map(|id| id.to_string()));
    query.push(", updated_at = ");
    query.push_bind(set.updated_at);
    query.push(" WHERE vault_id = ");
    query.push_bind(set.vault_id.to_string());
    query.push(" AND id = ");
    query.push_bind(set.id.to_string());
    if compare_previous_revision {
        query.push(" AND set_revision = ");
        let previous_revision = set
            .set_revision
            .value()
            .checked_sub(1)
            .map(Revision::new)
            .ok_or(StateError::Conflict)?;
        query.push_bind(previous_revision.as_i64()?);
    }
    let result = query.build().execute(&mut **transaction).await?;
    if result.rows_affected() != 1 {
        return Err(StateError::Conflict);
    }
    Ok(())
}

fn validate_current_bundle(
    context: &VaultContext,
    bundle: &CurrentMemoryBundle,
) -> Result<(), StateError> {
    let item = &bundle.memory;
    if item.vault_id != context.id()
        || item.content.trim().is_empty()
        || item.normalized_content.trim().is_empty()
        || item.content_hash.trim().is_empty()
        || item.tags.len() > 128
        || item.entities.len() > 128
    {
        return Err(StateError::InvalidInput("current-memory bundle is invalid"));
    }
    if item
        .importance
        .is_some_and(|value| !(0.0..=1.0).contains(&value))
        || item
            .confidence
            .is_some_and(|value| !(0.0..=1.0).contains(&value))
        || matches!((item.valid_from, item.valid_to), (Some(from), Some(to)) if from >= to)
    {
        return Err(StateError::InvalidInput(
            "current-memory metadata is invalid",
        ));
    }
    if bundle.sources.iter().any(|source| {
        source.vault_id != context.id()
            || source.memory_id != item.id
            || !matches!(
                source.source_type.as_str(),
                "note" | "explicit_agent" | "explicit_admin" | "import"
            )
    }) {
        return Err(StateError::InvalidInput(
            "current-memory provenance is invalid",
        ));
    }
    Ok(())
}

fn validate_note_set(context: &VaultContext, set: &MemoryNoteSetRecord) -> Result<(), StateError> {
    if set.vault_id != context.id()
        || set.source_content_hash.trim().is_empty()
        || set.profile_hash.trim().is_empty()
        || set.prompt_version.trim().is_empty()
    {
        return Err(StateError::InvalidInput("memory note set is invalid"));
    }
    Ok(())
}

fn validate_snapshot(
    context: &VaultContext,
    snapshot: &MemoryNoteSetSnapshotRecord,
) -> Result<(), StateError> {
    if snapshot.vault_id != context.id()
        || snapshot.status != "prepared"
        || snapshot.source_content_hash.trim().is_empty()
        || snapshot.canonical_bytes_hash.trim().is_empty()
        || snapshot.profile_hash.trim().is_empty()
        || snapshot.prompt_version.trim().is_empty()
        || snapshot
            .expected_set_revision
            .is_some_and(|revision| revision.next().ok() != Some(snapshot.proposed_set_revision))
        || snapshot.expected_set_revision.is_none()
            && snapshot.proposed_set_revision != Revision::new(1)
    {
        return Err(StateError::InvalidInput(
            "memory note-set snapshot is invalid",
        ));
    }
    Ok(())
}

fn validate_page(limit: u32, offset: u32) -> Result<(), StateError> {
    if limit == 0 || limit > MAX_MEMORY_LIMIT || offset > 100_000 {
        return Err(StateError::InvalidInput(
            "current-memory page bounds are invalid",
        ));
    }
    Ok(())
}

fn normalized_terms(values: &[String]) -> Vec<String> {
    let mut values = values
        .iter()
        .map(|value| value.trim().to_lowercase())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    values.sort();
    values.dedup();
    values
}

#[derive(Debug, FromRow)]
struct CurrentMemoryRow {
    id: String,
    vault_id: String,
    ownership: String,
    note_set_id: Option<String>,
    ordinal: Option<i64>,
    kind: Option<String>,
    content: String,
    normalized_content: String,
    content_hash: String,
    importance: Option<f64>,
    confidence: Option<f64>,
    origin: String,
    revision: i64,
    canonical_file_id: Option<String>,
    canonical_path: Option<String>,
    canonical_revision: Option<i64>,
    valid_from: Option<i64>,
    valid_to: Option<i64>,
    tags_json: String,
    entities_json: String,
    metadata_json: String,
    created_at: i64,
    updated_at: i64,
    last_recalled_at: Option<i64>,
    recall_count: i64,
}

#[derive(Debug, FromRow)]
struct CurrentMemorySearchRow {
    #[sqlx(flatten)]
    item: CurrentMemoryRow,
    memory_rank: f64,
}

#[derive(Debug, FromRow)]
struct CurrentMemorySourceRow {
    id: String,
    vault_id: String,
    memory_id: String,
    source_type: String,
    note_file_id: Option<String>,
    note_path: Option<String>,
    note_revision: Option<i64>,
    source_content_hash: Option<String>,
    heading_path_json: String,
    start_line: Option<i64>,
    end_line: Option<i64>,
    excerpt_hash: Option<String>,
    actor_id: Option<String>,
    created_at: i64,
}

#[derive(Debug, FromRow)]
struct MemoryNoteSetRow {
    id: String,
    vault_id: String,
    source_file_id: String,
    source_path: String,
    source_content_hash: String,
    source_revision: i64,
    set_revision: i64,
    extraction_paused: i64,
    canonical_file_id: String,
    canonical_path: String,
    canonical_revision: i64,
    profile_hash: String,
    prompt_version: String,
    provider_id: Option<String>,
    model_id: Option<String>,
    created_at: i64,
    updated_at: i64,
}

#[derive(Debug, FromRow)]
struct MemoryNoteSetSnapshotRow {
    id: String,
    vault_id: String,
    note_set_id: String,
    source_file_id: String,
    source_path: String,
    source_content_hash: String,
    source_revision: i64,
    expected_set_revision: Option<i64>,
    proposed_set_revision: i64,
    extraction_paused: i64,
    items_json: String,
    canonical_bytes_hash: String,
    canonical_path: String,
    profile_hash: String,
    prompt_version: String,
    provider_id: String,
    model_id: String,
    status: String,
    created_at: i64,
    applied_at: Option<i64>,
}

#[derive(Debug, FromRow)]
struct LegacySourceShapeRow {
    id: String,
    status: String,
    has_note: i64,
    has_explicit: i64,
    has_other: i64,
}

#[derive(Debug, FromRow)]
struct LegacyMigrationDigestRow {
    record_kind: String,
    record_key: String,
    record_json: String,
}

fn row_to_item(row: CurrentMemoryRow) -> Result<CurrentMemoryRecord, StateError> {
    Ok(CurrentMemoryRecord {
        id: MemoryId::parse(&row.id)?,
        vault_id: VaultId::parse(&row.vault_id)?,
        ownership: CurrentMemoryOwnership::parse(&row.ownership)?,
        note_set_id: row
            .note_set_id
            .as_deref()
            .map(MemorySetId::parse)
            .transpose()?,
        ordinal: row
            .ordinal
            .map(|value| {
                u32::try_from(value)
                    .map_err(|_| StateError::InvalidInput("stored memory ordinal is invalid"))
            })
            .transpose()?,
        kind: row.kind,
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
        tags: serde_json::from_str(&row.tags_json)?,
        entities: serde_json::from_str(&row.entities_json)?,
        metadata: serde_json::from_str(&row.metadata_json)?,
        created_at: row.created_at,
        updated_at: row.updated_at,
        last_recalled_at: row.last_recalled_at,
        recall_count: u64::try_from(row.recall_count)
            .map_err(|_| StateError::InvalidInput("stored memory recall count is invalid"))?,
    })
}

fn row_to_source(row: CurrentMemorySourceRow) -> Result<CurrentMemorySourceRecord, StateError> {
    Ok(CurrentMemorySourceRecord {
        id: MemorySourceId::parse(&row.id)?,
        vault_id: VaultId::parse(&row.vault_id)?,
        memory_id: MemoryId::parse(&row.memory_id)?,
        source_type: row.source_type,
        note_file_id: row.note_file_id.as_deref().map(FileId::parse).transpose()?,
        note_path: row.note_path.as_deref().map(VaultPath::parse).transpose()?,
        note_revision: row.note_revision.map(Revision::try_from).transpose()?,
        source_content_hash: row.source_content_hash,
        heading_path: serde_json::from_str(&row.heading_path_json)?,
        start_line: row
            .start_line
            .map(|value| {
                u32::try_from(value)
                    .map_err(|_| StateError::InvalidInput("stored source line is invalid"))
            })
            .transpose()?,
        end_line: row
            .end_line
            .map(|value| {
                u32::try_from(value)
                    .map_err(|_| StateError::InvalidInput("stored source line is invalid"))
            })
            .transpose()?,
        excerpt_hash: row.excerpt_hash,
        actor_id: row.actor_id,
        created_at: row.created_at,
    })
}

fn row_to_set(row: MemoryNoteSetRow) -> Result<MemoryNoteSetRecord, StateError> {
    Ok(MemoryNoteSetRecord {
        id: MemorySetId::parse(&row.id)?,
        vault_id: VaultId::parse(&row.vault_id)?,
        source_file_id: FileId::parse(&row.source_file_id)?,
        source_path: VaultPath::parse(&row.source_path)?,
        source_content_hash: row.source_content_hash,
        source_revision: Revision::try_from(row.source_revision)?,
        set_revision: Revision::try_from(row.set_revision)?,
        extraction_paused: row.extraction_paused != 0,
        canonical_file_id: FileId::parse(&row.canonical_file_id)?,
        canonical_path: VaultPath::parse(&row.canonical_path)?,
        canonical_revision: Revision::try_from(row.canonical_revision)?,
        profile_hash: row.profile_hash,
        prompt_version: row.prompt_version,
        provider_id: row
            .provider_id
            .as_deref()
            .map(ProviderId::parse)
            .transpose()?,
        model_id: row.model_id.as_deref().map(ModelId::parse).transpose()?,
        created_at: row.created_at,
        updated_at: row.updated_at,
    })
}

fn row_to_snapshot(
    row: MemoryNoteSetSnapshotRow,
) -> Result<MemoryNoteSetSnapshotRecord, StateError> {
    Ok(MemoryNoteSetSnapshotRecord {
        id: MemorySetSnapshotId::parse(&row.id)?,
        vault_id: VaultId::parse(&row.vault_id)?,
        note_set_id: MemorySetId::parse(&row.note_set_id)?,
        source_file_id: FileId::parse(&row.source_file_id)?,
        source_path: VaultPath::parse(&row.source_path)?,
        source_content_hash: row.source_content_hash,
        source_revision: Revision::try_from(row.source_revision)?,
        expected_set_revision: row
            .expected_set_revision
            .map(Revision::try_from)
            .transpose()?,
        proposed_set_revision: Revision::try_from(row.proposed_set_revision)?,
        extraction_paused: row.extraction_paused != 0,
        items: serde_json::from_str(&row.items_json)?,
        canonical_bytes_hash: row.canonical_bytes_hash,
        canonical_path: VaultPath::parse(&row.canonical_path)?,
        profile_hash: row.profile_hash,
        prompt_version: row.prompt_version,
        provider_id: ProviderId::parse(&row.provider_id)?,
        model_id: ModelId::parse(&row.model_id)?,
        status: row.status,
        created_at: row.created_at,
        applied_at: row.applied_at,
    })
}
