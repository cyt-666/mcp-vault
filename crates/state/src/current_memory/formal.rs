//! Formal-memory projection and durable local publication boundary.
use super::*;

/// Exact current contribution supporting the complete formal proposition.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FormalMemorySupport {
    /// Source-owned contribution identity.
    pub contribution_id: MemoryId,
    /// Stable source identity; paths never establish ownership.
    pub source_file_id: FileId,
    /// Exact represented source bytes.
    pub source_hash: String,
    /// Exact semantic contribution identity, including optional metadata.
    pub semantic_hash: String,
}

/// Portable formal object; canonical physical metadata is assigned by Vault Core.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FormalMemoryDocument {
    /// Representative complete proposition and preserved metadata.
    pub memory: CurrentMemoryRecord,
    /// Independently validated supports, never a similarity component.
    pub supports: Vec<FormalMemorySupport>,
}

/// One local source rewrite performed with formal forgetting/compaction.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FormalSourceRewrite {
    /// Set revision observed before preparing the operation.
    pub expected_revision: Revision,
    /// Next source set, including explicit pause state.
    pub set: MemoryNoteSetRecord,
    /// Complete surviving contributions.
    pub items: Vec<CurrentMemoryBundle>,
}

/// One bounded crash-recoverable publication. Only one can be active per Vault.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FormalMemoryOperation {
    /// Stable operation identity allocated before canonical I/O.
    pub id: String,
    /// Previous formal objects used for revision checks and eventual file cleanup.
    pub before: Vec<FormalMemoryDocument>,
    /// Complete new objects, with stable IDs and deterministic canonical bytes.
    pub after: Vec<FormalMemoryDocument>,
    /// Source contributions to replace atomically with formal publication.
    pub source_rewrites: Vec<FormalSourceRewrite>,
    /// User-forgotten formal IDs, hidden immediately during recovery.
    pub forgotten: Vec<MemoryId>,
}

/// Exact proposition key. Retrieval normalization is deliberately excluded.
pub fn contribution_semantic_hash(item: &CurrentMemoryRecord) -> Result<String, StateError> {
    let bytes = serde_json::to_vec(&serde_json::json!([
        item.content,
        item.kind,
        item.importance,
        item.confidence,
        item.valid_from,
        item.valid_to,
        item.tags,
        item.entities,
        item.metadata
    ]))?;
    Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
}

impl CurrentMemoryRepository {
    /// Read whether all public note-derived reads use the formal projection.
    pub async fn formal_enabled(&self, context: &VaultContext) -> Result<bool, StateError> {
        Ok(sqlx::query_scalar::<_, i64>(
            "SELECT coalesce((SELECT enabled FROM memory_formal_mode WHERE vault_id=?),0)",
        )
        .bind(context.id().to_string())
        .fetch_one(&self.pool)
        .await?
            == 1)
    }

    /// Page eligible contributions independently of public formal reads.
    pub async fn contributions(
        &self,
        context: &VaultContext,
        after: Option<MemoryId>,
        limit: u32,
    ) -> Result<Vec<CurrentMemoryBundle>, StateError> {
        validate_page(limit, 0)?;
        let sql = format!(
            "{} WHERE i.vault_id=? AND i.ownership='note_derived' AND (? IS NULL OR i.id>?) AND {} ORDER BY i.id LIMIT ?",
            contribution_select(),
            contribution_eligibility_sql()
        );
        let rows = sqlx::query_as::<_, CurrentMemoryRow>(&sql)
            .bind(context.id().to_string())
            .bind(after.map(|id| id.to_string()))
            .bind(after.map(|id| id.to_string()))
            .bind(i64::from(limit))
            .fetch_all(&self.pool)
            .await?;
        let mut result = Vec::new();
        for row in rows {
            let item = row_to_item(row)?;
            // Populate the additive fingerprint for pre-0021 rows without rewriting
            // canonical bytes or touching the old embedding content/input hashes.
            sqlx::query("UPDATE memory_current_items SET semantic_hash=? WHERE vault_id=? AND id=? AND semantic_hash=''")
                .bind(contribution_semantic_hash(&item)?).bind(context.id().to_string()).bind(item.id.to_string()).execute(&self.pool).await?;
            if let Some(bundle) = self.get_unchecked(context, item.id).await? {
                result.push(bundle);
            }
        }
        Ok(result)
    }

    /// Current formal document including invalid supports for local reconciliation.
    pub async fn formal_document(
        &self,
        context: &VaultContext,
        id: MemoryId,
    ) -> Result<Option<FormalMemoryDocument>, StateError> {
        let sql = contribution_select().replace("memory_current_items i", "memory_formal_items i")
            + " WHERE i.vault_id=? AND i.id=?";
        let Some(row) = sqlx::query_as::<_, CurrentMemoryRow>(&sql)
            .bind(context.id().to_string())
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await?
        else {
            return Ok(None);
        };
        let rows:Vec<(String,String,String,String)>=sqlx::query_as("SELECT contribution_id,source_file_id,source_hash,semantic_hash FROM memory_formal_supports WHERE vault_id=? AND formal_id=? ORDER BY contribution_id")
            .bind(context.id().to_string()).bind(id.to_string()).fetch_all(&self.pool).await?;
        let supports = rows
            .into_iter()
            .map(|(contribution, source, source_hash, semantic_hash)| {
                Ok(FormalMemorySupport {
                    contribution_id: MemoryId::parse(&contribution)?,
                    source_file_id: FileId::parse(&source)?,
                    source_hash,
                    semantic_hash,
                })
            })
            .collect::<Result<Vec<_>, StateError>>()?;
        Ok(Some(FormalMemoryDocument {
            memory: row_to_item(row)?,
            supports,
        }))
    }

    /// Deterministic keyset page, including orphaned objects needing cleanup.
    pub async fn formal_documents(
        &self,
        context: &VaultContext,
        after: Option<MemoryId>,
        limit: u32,
    ) -> Result<Vec<FormalMemoryDocument>, StateError> {
        validate_page(limit, 0)?;
        let ids:Vec<String>=sqlx::query_scalar("SELECT id FROM memory_formal_items WHERE vault_id=? AND (? IS NULL OR id>?) ORDER BY id LIMIT ?")
            .bind(context.id().to_string()).bind(after.map(|id|id.to_string())).bind(after.map(|id|id.to_string())).bind(i64::from(limit)).fetch_all(&self.pool).await?;
        let mut docs = Vec::new();
        for id in ids {
            if let Some(doc) = self.formal_document(context, MemoryId::parse(&id)?).await? {
                docs.push(doc);
            }
        }
        Ok(docs)
    }

    /// Resolve an internal contribution's formal owner without exposing an alias.
    pub async fn contribution_owner(
        &self,
        context: &VaultContext,
        id: MemoryId,
    ) -> Result<Option<MemoryId>, StateError> {
        let id: Option<String> = sqlx::query_scalar(
            "SELECT formal_id FROM memory_formal_supports WHERE vault_id=? AND contribution_id=?",
        )
        .bind(context.id().to_string())
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        id.map(|id| MemoryId::parse(&id).map_err(Into::into))
            .transpose()
    }

    pub(super) async fn formal_sources(
        &self,
        context: &VaultContext,
        id: MemoryId,
    ) -> Result<Vec<CurrentMemorySourceRecord>, StateError> {
        let ids:Vec<String>=sqlx::query_scalar("SELECT min(contribution_id) FROM memory_valid_formal_supports WHERE vault_id=? AND formal_id=? GROUP BY source_file_id ORDER BY source_file_id")
            .bind(context.id().to_string()).bind(id.to_string()).fetch_all(&self.pool).await?;
        let mut sources = Vec::new();
        for contribution in ids {
            let mut rows = self
                .list_sources(context, MemoryId::parse(&contribution)?)
                .await?;
            for row in &mut rows {
                row.memory_id = id;
            }
            sources.extend(rows);
        }
        Ok(sources)
    }

    /// Verify a canonical support against current source and semantic identity.
    pub async fn support_current(
        &self,
        context: &VaultContext,
        support: &FormalMemorySupport,
    ) -> Result<bool, StateError> {
        let Some(bundle) = self.get_unchecked(context, support.contribution_id).await? else {
            return Ok(false);
        };
        let Some(set) = bundle.note_set else {
            return Ok(false);
        };
        if set.source_file_id != support.source_file_id
            || set.source_content_hash != support.source_hash
            || contribution_semantic_hash(&bundle.memory)? != support.semantic_hash
        {
            return Ok(false);
        }
        Ok(self
            .ensure_note_set_files_current(context, &set)
            .await
            .is_ok())
    }

    /// Switch a Vault only after every currently eligible contribution is adopted.
    pub async fn enable_formal_if_covered(
        &self,
        context: &VaultContext,
    ) -> Result<bool, StateError> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let sql = format!(
            "SELECT count(*) FROM memory_current_items i WHERE i.vault_id=? AND i.ownership='note_derived' AND {} AND NOT EXISTS(SELECT 1 FROM memory_valid_formal_supports p WHERE p.vault_id=i.vault_id AND p.contribution_id=i.id)",
            contribution_eligibility_sql()
        );
        let missing: i64 = sqlx::query_scalar(&sql)
            .bind(context.id().to_string())
            .fetch_one(&mut *tx)
            .await?;
        if missing == 0 {
            sqlx::query("INSERT INTO memory_formal_mode(vault_id,enabled) VALUES(?,1) ON CONFLICT(vault_id) DO UPDATE SET enabled=1").bind(context.id().to_string()).execute(&mut *tx).await?;
            // Hidden raw contributions must not change the FTS corpus or BM25
            // ranking after adoption (including a cold Markdown rebuild).
            sqlx::query("DELETE FROM memory_current_fts WHERE vault_id=? AND NOT EXISTS(SELECT 1 FROM memory_public_items i WHERE i.vault_id=memory_current_fts.vault_id AND i.id=memory_current_fts.memory_id)")
                .bind(context.id().to_string())
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(missing == 0)
    }

    /// Load the sole in-flight local publication after restart.
    pub async fn formal_operation(
        &self,
        context: &VaultContext,
    ) -> Result<Option<FormalMemoryOperation>, StateError> {
        let value: Option<String> = sqlx::query_scalar(
            "SELECT payload_json FROM memory_formal_operations WHERE vault_id=?",
        )
        .bind(context.id().to_string())
        .fetch_optional(&self.pool)
        .await?;
        value
            .map(|s| serde_json::from_str(&s).map_err(Into::into))
            .transpose()
    }

    /// Reserve publication before any file change. Immutable operation bytes
    /// serialize competing workers/processes without keeping a SQL lock over I/O.
    pub async fn prepare_formal_operation(
        &self,
        context: &VaultContext,
        op: &FormalMemoryOperation,
    ) -> Result<(), StateError> {
        if op.after.len() > 256 || op.before.len() > 256 || op.source_rewrites.len() > 256 {
            return Err(StateError::InvalidInput("formal operation exceeds bound"));
        }
        for doc in op.after.iter().chain(op.before.iter()) {
            if doc.memory.vault_id != context.id()
                || doc.memory.ownership != CurrentMemoryOwnership::NoteDerived
            {
                return Err(StateError::InvalidInput(
                    "formal ownership scope is invalid",
                ));
            }
        }
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        for doc in &op.before {
            let rev: Option<i64> = sqlx::query_scalar(
                "SELECT revision FROM memory_formal_items WHERE vault_id=? AND id=?",
            )
            .bind(context.id().to_string())
            .bind(doc.memory.id.to_string())
            .fetch_optional(&mut *tx)
            .await?;
            if rev != Some(doc.memory.revision.as_i64()?) {
                return Err(StateError::Conflict);
            }
        }
        for rewrite in &op.source_rewrites {
            let rev: Option<i64> = sqlx::query_scalar(
                "SELECT set_revision FROM memory_note_sets WHERE vault_id=? AND id=?",
            )
            .bind(context.id().to_string())
            .bind(rewrite.set.id.to_string())
            .fetch_optional(&mut *tx)
            .await?;
            if rewrite.set.vault_id != context.id()
                || rev != Some(rewrite.expected_revision.as_i64()?)
            {
                return Err(StateError::Conflict);
            }
        }
        for doc in &op.after {
            if !op.before.iter().any(|old| old.memory.id == doc.memory.id) {
                sqlx::query("INSERT INTO memory_formal_identity_reservations(vault_id,memory_id) VALUES(?,?)")
                    .bind(context.id().to_string()).bind(doc.memory.id.to_string()).execute(&mut *tx).await?;
            }
        }
        // Every transferred contribution must still belong to one of this operation's
        // old formal objects; a late competing merge cannot steal another support.
        for doc in &op.after {
            for p in &doc.supports {
                let owner:Option<String>=sqlx::query_scalar("SELECT formal_id FROM memory_formal_supports WHERE vault_id=? AND contribution_id=?").bind(context.id().to_string()).bind(p.contribution_id.to_string()).fetch_optional(&mut *tx).await?;
                if owner.is_some_and(|id| !op.before.iter().any(|d| d.memory.id.to_string() == id))
                {
                    return Err(StateError::Conflict);
                }
            }
        }
        sqlx::query("INSERT INTO memory_formal_operations(vault_id,operation_id,payload_json,created_at) VALUES(?,?,?,?)")
            .bind(context.id().to_string()).bind(&op.id).bind(serde_json::to_string(op)?).bind(now_millis()?).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Atomically replace the formal view after deterministic canonical writes.
    /// Current support qualification is also enforced at every public read.
    pub async fn commit_formal_operation(
        &self,
        context: &VaultContext,
        op: &FormalMemoryOperation,
    ) -> Result<(), StateError> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let active: Option<String> = sqlx::query_scalar(
            "SELECT operation_id FROM memory_formal_operations WHERE vault_id=?",
        )
        .bind(context.id().to_string())
        .fetch_optional(&mut *tx)
        .await?;
        if active.as_deref() != Some(op.id.as_str()) {
            return Err(StateError::Conflict);
        }
        for rewrite in &op.source_rewrites {
            let rev: Option<i64> = sqlx::query_scalar(
                "SELECT set_revision FROM memory_note_sets WHERE vault_id=? AND id=?",
            )
            .bind(context.id().to_string())
            .bind(rewrite.set.id.to_string())
            .fetch_optional(&mut *tx)
            .await?;
            if rev != Some(rewrite.expected_revision.as_i64()?) {
                return Err(StateError::Conflict);
            }
            delete_set_fts(&mut tx, context.id(), rewrite.set.id).await?;
            sqlx::query("DELETE FROM memory_current_items WHERE vault_id=? AND note_set_id=?")
                .bind(context.id().to_string())
                .bind(rewrite.set.id.to_string())
                .execute(&mut *tx)
                .await?;
            update_note_set(&mut tx, &rewrite.set, true).await?;
            for bundle in &rewrite.items {
                upsert_item(&mut tx, &bundle.memory).await?;
                replace_sources(&mut tx, context.id(), bundle.memory.id, &bundle.sources).await?;
                insert_item_fts(&mut tx, &bundle.memory).await?;
            }
            sqlx::query("UPDATE memory_note_set_snapshots SET status='rejected',applied_at=? WHERE vault_id=? AND source_file_id=? AND status='prepared'").bind(now_millis()?).bind(context.id().to_string()).bind(rewrite.set.source_file_id.to_string()).execute(&mut *tx).await?;
        }
        for doc in &op.before {
            delete_item_fts_raw(&mut tx, context.id(), doc.memory.id).await?;
            sqlx::query("DELETE FROM memory_formal_items WHERE vault_id=? AND id=?")
                .bind(context.id().to_string())
                .bind(doc.memory.id.to_string())
                .execute(&mut *tx)
                .await?;
        }
        for doc in &op.after {
            upsert_item_table(&mut tx, &doc.memory, true).await?;
            delete_item_fts_raw(&mut tx, context.id(), doc.memory.id).await?;
            insert_item_fts_raw(&mut tx, &doc.memory).await?;
            for p in &doc.supports {
                sqlx::query("INSERT INTO memory_formal_supports(vault_id,formal_id,contribution_id,source_file_id,source_hash,semantic_hash) VALUES(?,?,?,?,?,?)")
                    .bind(context.id().to_string()).bind(doc.memory.id.to_string()).bind(p.contribution_id.to_string()).bind(p.source_file_id.to_string()).bind(&p.source_hash).bind(&p.semantic_hash).execute(&mut *tx).await?;
            }
        }
        sqlx::query(
            "UPDATE memory_formal_operations SET committed=1 WHERE vault_id=? AND operation_id=?",
        )
        .bind(context.id().to_string())
        .bind(&op.id)
        .execute(&mut *tx)
        .await?;
        // Keep the operation until obsolete canonical files have been removed.
        // Recovery recognizes committed metadata by each after object's revision.
        tx.commit().await?;
        Ok(())
    }

    /// Whether projection publication already committed before a crash.
    pub async fn formal_operation_committed(
        &self,
        context: &VaultContext,
    ) -> Result<bool, StateError> {
        Ok(sqlx::query_scalar::<_, i64>(
            "SELECT coalesce((SELECT committed FROM memory_formal_operations WHERE vault_id=?),0)",
        )
        .bind(context.id().to_string())
        .fetch_one(&self.pool)
        .await?
            == 1)
    }

    /// Remove a completed operation only after obsolete canonical paths are gone.
    pub async fn finish_formal_operation(
        &self,
        context: &VaultContext,
        id: &str,
    ) -> Result<(), StateError> {
        sqlx::query("DELETE FROM memory_formal_operations WHERE vault_id=? AND operation_id=?")
            .bind(context.id().to_string())
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Rebuild one canonical formal document without model work or source rebinding.
    pub async fn restore_formal_document(
        &self,
        context: &VaultContext,
        doc: &FormalMemoryDocument,
    ) -> Result<(), StateError> {
        if doc.memory.vault_id != context.id()
            || doc.memory.ownership != CurrentMemoryOwnership::NoteDerived
        {
            return Err(StateError::InvalidInput("formal scope is invalid"));
        }
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        for support in &doc.supports {
            let owner: Option<String> = sqlx::query_scalar("SELECT formal_id FROM memory_formal_supports WHERE vault_id=? AND contribution_id=?")
                .bind(context.id().to_string()).bind(support.contribution_id.to_string()).fetch_optional(&mut *tx).await?;
            if owner.is_some_and(|id| id != doc.memory.id.to_string()) {
                return Err(StateError::Conflict);
            }
        }
        sqlx::query("INSERT INTO memory_formal_identity_reservations(vault_id,memory_id) VALUES(?,?) ON CONFLICT DO NOTHING")
            .bind(context.id().to_string()).bind(doc.memory.id.to_string()).execute(&mut *tx).await?;
        for support in &doc.supports {
            sqlx::query("INSERT INTO memory_formal_identity_reservations(vault_id,memory_id) VALUES(?,?) ON CONFLICT DO NOTHING")
                .bind(context.id().to_string()).bind(support.contribution_id.to_string()).execute(&mut *tx).await?;
        }
        upsert_item_table(&mut tx, &doc.memory, true).await?;
        sqlx::query("DELETE FROM memory_formal_supports WHERE vault_id=? AND formal_id=?")
            .bind(context.id().to_string())
            .bind(doc.memory.id.to_string())
            .execute(&mut *tx)
            .await?;
        for p in &doc.supports {
            sqlx::query("INSERT INTO memory_formal_supports(vault_id,formal_id,contribution_id,source_file_id,source_hash,semantic_hash) VALUES(?,?,?,?,?,?)")
            .bind(context.id().to_string()).bind(doc.memory.id.to_string()).bind(p.contribution_id.to_string()).bind(p.source_file_id.to_string()).bind(&p.source_hash).bind(&p.semantic_hash).execute(&mut *tx).await?;
        }
        delete_item_fts_raw(&mut tx, context.id(), doc.memory.id).await?;
        insert_item_fts_raw(&mut tx, &doc.memory).await?;
        tx.commit().await?;
        Ok(())
    }
}

/// Content-free, current background-maintenance progress for Admin.
#[derive(Clone, Debug, Serialize, Default)]
pub struct FormalMaintenanceStatus {
    /// Actual current maintenance stage, independent of the last error.
    pub phase: String,
    /// Successfully examined singleton bodies, including unchanged bodies.
    pub sentence_checked: u64,
    /// Local compatibility read-view cutover completed.
    pub adopted: bool,
    /// Pending bounded semantic candidate pairs.
    pub pending_pairs: u64,
    /// Persistent terminal judgments for these inputs (including uncertainty).
    pub checked_pairs: u64,
    /// Current state or redacted blocking reason.
    pub status: String,
    /// Earliest automatic retry time.
    pub retry_at: i64,
}

impl CurrentMemoryRepository {
    /// Explicit-only or empty Vaults have no automatic semantic work.
    pub async fn has_dedup_work(&self, context: &VaultContext) -> Result<bool, StateError> {
        Ok(sqlx::query_scalar::<_, i64>("SELECT EXISTS(SELECT 1 FROM memory_current_items WHERE vault_id=?1 AND ownership='note_derived') OR EXISTS(SELECT 1 FROM memory_formal_items WHERE vault_id=?1) OR EXISTS(SELECT 1 FROM memory_formal_operations WHERE vault_id=?1) OR EXISTS(SELECT 1 FROM memory_formal_pairs WHERE vault_id=?1 AND done=0)")
            .bind(context.id().to_string()).fetch_one(&self.pool).await? != 0)
    }
    /// Metadata-only admission fingerprint; never reads note bodies or invokes a model.
    pub async fn dedup_input_fingerprint(
        &self,
        context: &VaultContext,
        profile: &str,
    ) -> Result<String, StateError> {
        let mut digest = Sha256::new();
        digest.update(profile.as_bytes());
        let mut cursor = String::new();
        loop {
            let rows: Vec<String> = sqlx::query_scalar("SELECT entry FROM (
                SELECT json_array('source',s.id,s.source_content_hash,s.set_revision,s.extraction_paused,f.content_hash,f.deleted_at,c.content_hash) AS entry FROM memory_note_sets s LEFT JOIN file_entries f ON f.vault_id=s.vault_id AND f.id=s.source_file_id LEFT JOIN file_entries c ON c.vault_id=s.vault_id AND c.id=s.canonical_file_id WHERE s.vault_id=?1
                UNION ALL SELECT json_array('item',id,semantic_hash) FROM memory_current_items WHERE vault_id=?1 AND ownership='note_derived'
                UNION ALL SELECT json_array('formal',id,revision,content_hash) FROM memory_formal_items WHERE vault_id=?1
                UNION ALL SELECT json_array('vector',id,model_id,content_hash,updated_at) FROM embedding_records WHERE vault_id=?1 AND object_type='memory'
            ) WHERE entry>?2 ORDER BY entry LIMIT 128")
                .bind(context.id().to_string()).bind(&cursor).fetch_all(&self.pool).await?;
            if rows.is_empty() {
                break;
            }
            for row in &rows {
                digest.update(row.as_bytes());
                digest.update([0]);
            }
            cursor = rows.last().unwrap().clone();
        }
        Ok(format!("sha256:{:x}", digest.finalize()))
    }
    /// Stable covered inputs need no new semantic job; unfinished work always wins.
    pub async fn dedup_inputs_covered(
        &self,
        context: &VaultContext,
        fingerprint: &str,
    ) -> Result<bool, StateError> {
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM memory_formal_maintenance m WHERE m.vault_id=? AND m.status='covered_candidates' AND m.fingerprint=? AND NOT EXISTS(SELECT 1 FROM memory_formal_pairs p WHERE p.vault_id=m.vault_id AND p.done=0) AND NOT EXISTS(SELECT 1 FROM memory_dedup_new_contributions n WHERE n.vault_id=m.vault_id) AND NOT EXISTS(SELECT 1 FROM memory_formal_operations o WHERE o.vault_id=m.vault_id)")
            .bind(context.id().to_string()).bind(fingerprint).fetch_one(&self.pool).await?;
        Ok(count == 1)
    }
    /// Save the inputs seen before a successful pass; concurrent changes remain dirty.
    pub async fn checkpoint_dedup_inputs(
        &self,
        context: &VaultContext,
        fingerprint: &str,
    ) -> Result<(), StateError> {
        sqlx::query("UPDATE memory_formal_maintenance SET fingerprint=? WHERE vault_id=?")
            .bind(fingerprint)
            .bind(context.id().to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }
    /// Exact input/profile examination checkpoint, independent of public IDs.
    pub async fn formal_examined(
        &self,
        context: &VaultContext,
        id: MemoryId,
        fingerprint: &str,
    ) -> Result<bool, StateError> {
        Ok(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM memory_formal_examined WHERE vault_id=? AND contribution_id=? AND fingerprint=?").bind(context.id().to_string()).bind(id.to_string()).bind(fingerprint).fetch_one(&self.pool).await?==1)
    }
    /// Newly published contributions bypass the historical discovery cursor.
    pub async fn new_dedup_contributions(
        &self,
        context: &VaultContext,
        limit: u32,
    ) -> Result<Vec<MemoryId>, StateError> {
        validate_page(limit, 0)?;
        let ids: Vec<String> = sqlx::query_scalar("SELECT contribution_id FROM memory_dedup_new_contributions WHERE vault_id=? ORDER BY contribution_id LIMIT ?")
            .bind(context.id().to_string()).bind(i64::from(limit)).fetch_all(&self.pool).await?;
        ids.iter()
            .map(|id| MemoryId::parse(id).map_err(StateError::from))
            .collect()
    }
    /// Retire obsolete or already examined incremental inputs.
    pub async fn finish_new_dedup_contribution(
        &self,
        context: &VaultContext,
        id: MemoryId,
    ) -> Result<(), StateError> {
        sqlx::query(
            "DELETE FROM memory_dedup_new_contributions WHERE vault_id=? AND contribution_id=?",
        )
        .bind(context.id().to_string())
        .bind(id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }
    /// Persist candidates before marking an input examined; crashes cannot lose work.
    pub async fn queue_formal_pairs(
        &self,
        context: &VaultContext,
        id: MemoryId,
        fingerprint: &str,
        pairs: &[(String, MemoryId, MemoryId)],
    ) -> Result<(), StateError> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let priority: i64 = sqlx::query_scalar("SELECT count(*) FROM memory_dedup_new_contributions WHERE vault_id=? AND contribution_id=?")
            .bind(context.id().to_string()).bind(id.to_string()).fetch_one(&mut *tx).await?;
        for (key, left, right) in pairs {
            sqlx::query("INSERT INTO memory_formal_pairs(vault_id,pair_key,left_id,right_id,priority) VALUES(?,?,?,?,?) ON CONFLICT(vault_id,pair_key) DO UPDATE SET priority=max(priority,excluded.priority)").bind(context.id().to_string()).bind(key).bind(left.to_string()).bind(right.to_string()).bind(priority).execute(&mut *tx).await?;
        }
        sqlx::query(
            "DELETE FROM memory_dedup_new_contributions WHERE vault_id=? AND contribution_id=?",
        )
        .bind(context.id().to_string())
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
        sqlx::query("INSERT INTO memory_formal_examined(vault_id,contribution_id,fingerprint) VALUES(?,?,?) ON CONFLICT(vault_id,contribution_id) DO UPDATE SET fingerprint=excluded.fingerprint").bind(context.id().to_string()).bind(id.to_string()).bind(fingerprint).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }
    /// Bounded persistent candidate page. Successful/uncertain decisions are not retried.
    pub async fn pending_formal_pairs(
        &self,
        context: &VaultContext,
        limit: u32,
    ) -> Result<Vec<(String, MemoryId, MemoryId)>, StateError> {
        validate_page(limit, 0)?;
        let rows:Vec<(String,String,String)>=sqlx::query_as("SELECT pair_key,left_id,right_id FROM memory_formal_pairs WHERE vault_id=? AND done=0 ORDER BY attempt_sequence,priority DESC,pair_key LIMIT ?").bind(context.id().to_string()).bind(i64::from(limit)).fetch_all(&self.pool).await?;
        rows.into_iter()
            .map(|(k, l, r)| Ok((k, MemoryId::parse(&l)?, MemoryId::parse(&r)?)))
            .collect()
    }
    /// Persist a candidate's turn before I/O, without treating interruption as completion.
    pub async fn start_formal_pair(
        &self,
        context: &VaultContext,
        key: &str,
    ) -> Result<bool, StateError> {
        let result = sqlx::query("UPDATE memory_formal_pairs SET attempt_sequence=(SELECT coalesce(max(attempt_sequence),0)+1 FROM memory_formal_pairs WHERE vault_id=? AND done=0) WHERE vault_id=? AND pair_key=? AND done=0")
            .bind(context.id().to_string())
            .bind(context.id().to_string())
            .bind(key)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() == 1)
    }
    /// Checkpoint one completed candidate without exposing text or model output.
    pub async fn finish_formal_pair(
        &self,
        context: &VaultContext,
        key: &str,
    ) -> Result<(), StateError> {
        sqlx::query("UPDATE memory_formal_pairs SET done=1 WHERE vault_id=? AND pair_key=?")
            .bind(context.id().to_string())
            .bind(key)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
    /// Store only a project-owned status code and next automatic retry time.
    pub async fn set_formal_status(
        &self,
        context: &VaultContext,
        status: &str,
        retry_at: i64,
    ) -> Result<(), StateError> {
        sqlx::query("INSERT INTO memory_formal_maintenance(vault_id,status,retry_at) VALUES(?,?,?) ON CONFLICT(vault_id) DO UPDATE SET status=excluded.status,retry_at=excluded.retry_at").bind(context.id().to_string()).bind(status).bind(retry_at).execute(&self.pool).await?;
        Ok(())
    }
    /// Read-only background status, independent of source extraction actions.
    pub async fn formal_status(
        &self,
        context: &VaultContext,
    ) -> Result<FormalMaintenanceStatus, StateError> {
        let row: Option<(String, i64)> = sqlx::query_as(
            "SELECT status,retry_at FROM memory_formal_maintenance WHERE vault_id=?",
        )
        .bind(context.id().to_string())
        .fetch_optional(&self.pool)
        .await?;
        let (pending,done):(i64,i64)=sqlx::query_as("SELECT coalesce(sum(done=0),0),coalesce(sum(done=1),0) FROM memory_formal_pairs WHERE vault_id=?").bind(context.id().to_string()).fetch_one(&self.pool).await?;
        let (status, retry_at) = row.unwrap_or(("pending".into(), 0));
        let (phase, sentence_checked): (String, i64) = sqlx::query_as(
            "SELECT phase,sentence_checked FROM memory_dedup_progress WHERE vault_id=?",
        )
        .bind(context.id().to_string())
        .fetch_optional(&self.pool)
        .await?
        .unwrap_or(("pending".into(), 0));
        Ok(FormalMaintenanceStatus {
            phase,
            sentence_checked: sentence_checked as u64,
            adopted: self.formal_enabled(context).await?,
            pending_pairs: pending as u64,
            checked_pairs: done as u64,
            status,
            retry_at,
        })
    }
}

impl CurrentMemoryRepository {
    /// Count current contributions without loading bodies.
    pub async fn contribution_count(&self, context: &VaultContext) -> Result<u64, StateError> {
        let sql = format!(
            "SELECT count(*) FROM memory_current_items i WHERE i.vault_id=? AND i.ownership='note_derived' AND {}",
            contribution_eligibility_sql()
        );
        let n: i64 = sqlx::query_scalar(&sql)
            .bind(context.id().to_string())
            .fetch_one(&self.pool)
            .await?;
        Ok(n as u64)
    }
    /// Per-object vector arrival stamp; another object's vector does not invalidate this input.
    pub async fn formal_vector_stamp(
        &self,
        context: &VaultContext,
        id: MemoryId,
    ) -> Result<String, StateError> {
        let (n,t):(i64,i64)=sqlx::query_as("SELECT count(*),coalesce(max(updated_at),0) FROM embedding_records WHERE vault_id=? AND object_type='memory' AND object_id=?").bind(context.id().to_string()).bind(id.to_string()).fetch_one(&self.pool).await?;
        Ok(format!("{n}:{t}"))
    }
    /// Keep source writers out of a prepared multi-source forget transaction.
    pub(super) async fn ensure_no_formal_source_rewrite(
        &self,
        context: &VaultContext,
        file_id: FileId,
    ) -> Result<(), StateError> {
        let n:i64=sqlx::query_scalar("SELECT count(*) FROM memory_formal_operations op,json_each(op.payload_json,'$.source_rewrites') r WHERE op.vault_id=? AND json_extract(r.value,'$.set.source_file_id')=?").bind(context.id().to_string()).bind(file_id.to_string()).fetch_one(&self.pool).await?;
        if n > 0 {
            Err(StateError::Conflict)
        } else {
            Ok(())
        }
    }
}

impl CurrentMemoryRepository {
    /// Reuse a bounded, validated textual proposal without repeating generation.
    pub async fn equivalence_rewrite(
        &self,
        context: &VaultContext,
        key: &str,
    ) -> Result<Option<String>, StateError> {
        Ok(sqlx::query_scalar(
            "SELECT content FROM memory_equivalence_rewrites WHERE vault_id=? AND input_hash=?",
        )
        .bind(context.id().to_string())
        .bind(key)
        .fetch_optional(&self.pool)
        .await?)
    }
    /// Store only parsed bounded proposals. Empty content means keep the original.
    pub async fn save_equivalence_rewrite(
        &self,
        context: &VaultContext,
        key: &str,
        content: &str,
    ) -> Result<(), StateError> {
        if content.len() > 65536 {
            return Err(StateError::InvalidInput("rewrite exceeds bound"));
        }
        sqlx::query("INSERT INTO memory_equivalence_rewrites(vault_id,input_hash,content) VALUES(?,?,?) ON CONFLICT(vault_id,input_hash) DO NOTHING").bind(context.id().to_string()).bind(key).bind(content).execute(&self.pool).await?;
        Ok(())
    }
}

impl CurrentMemoryRepository {
    /// Stable internal source cursor remains valid while completed sets disappear.
    pub async fn source_ids_after(
        &self,
        context: &VaultContext,
        after: Option<FileId>,
        limit: u32,
    ) -> Result<Vec<FileId>, StateError> {
        validate_page(limit, 0)?;
        let ids:Vec<String> = sqlx::query_scalar("SELECT source_file_id FROM memory_note_sets WHERE vault_id=? AND (? IS NULL OR source_file_id>?) ORDER BY source_file_id LIMIT ?")
            .bind(context.id().to_string()).bind(after.map(|id|id.to_string())).bind(after.map(|id|id.to_string())).bind(i64::from(limit)).fetch_all(&self.pool).await?;
        ids.iter()
            .map(|id| FileId::parse(id).map_err(StateError::from))
            .collect()
    }
}

impl CurrentMemoryRepository {
    /// A retired formal identity remains reserved without retaining its body.
    pub async fn formal_identity_reserved(
        &self,
        context: &VaultContext,
        id: MemoryId,
    ) -> Result<bool, StateError> {
        Ok(sqlx::query_scalar::<_,i64>("SELECT EXISTS(SELECT 1 FROM memory_formal_identity_reservations WHERE vault_id=? AND memory_id=?)").bind(context.id().to_string()).bind(id.to_string()).fetch_one(&self.pool).await?!=0)
    }
}

impl CurrentMemoryRepository {
    /// Persisted candidate-scan continuation, separate from completed pair work.
    pub async fn formal_scan_cursor(
        &self,
        context: &VaultContext,
    ) -> Result<Option<MemoryId>, StateError> {
        let cursor: Option<String> =
            sqlx::query_scalar("SELECT cursor FROM memory_formal_maintenance WHERE vault_id=?")
                .bind(context.id().to_string())
                .fetch_optional(&self.pool)
                .await?
                .flatten();
        cursor
            .filter(|id| !id.is_empty())
            .map(|id| MemoryId::parse(&id).map_err(StateError::from))
            .transpose()
    }
    /// Checkpoint before yielding; a completed sweep restarts at the next periodic admission.
    pub async fn save_formal_scan_cursor(
        &self,
        context: &VaultContext,
        cursor: Option<MemoryId>,
    ) -> Result<(), StateError> {
        sqlx::query("INSERT INTO memory_formal_maintenance(vault_id,cursor) VALUES(?,?) ON CONFLICT(vault_id) DO UPDATE SET cursor=excluded.cursor")
            .bind(context.id().to_string()).bind(cursor.map(|id|id.to_string()).unwrap_or_default()).execute(&self.pool).await?;
        Ok(())
    }
}

impl CurrentMemoryRepository {
    /// Record a content-free stage without overwriting a waiting/error reason.
    pub async fn set_dedup_phase(
        &self,
        context: &VaultContext,
        phase: &str,
    ) -> Result<(), StateError> {
        sqlx::query("INSERT INTO memory_dedup_progress(vault_id,phase) VALUES(?,?) ON CONFLICT(vault_id) DO UPDATE SET phase=excluded.phase")
            .bind(context.id().to_string()).bind(phase).execute(&self.pool).await?;
        Ok(())
    }
    /// Continue a singleton sweep after restart or cancellation.
    pub async fn sentence_scan_cursor(
        &self,
        context: &VaultContext,
    ) -> Result<Option<MemoryId>, StateError> {
        let cursor: Option<String> = sqlx::query_scalar(
            "SELECT sentence_cursor FROM memory_dedup_progress WHERE vault_id=?",
        )
        .bind(context.id().to_string())
        .fetch_optional(&self.pool)
        .await?
        .flatten();
        cursor
            .map(|id| MemoryId::parse(&id).map_err(StateError::from))
            .transpose()
    }
    /// Advance before an external call, so one repeatedly slow body cannot block
    /// all later bodies. It remains eligible on the next sweep if interrupted.
    pub async fn checkpoint_sentence_scan(
        &self,
        context: &VaultContext,
        cursor: Option<MemoryId>,
    ) -> Result<(), StateError> {
        sqlx::query("INSERT INTO memory_dedup_progress(vault_id,sentence_cursor) VALUES(?,?) ON CONFLICT(vault_id) DO UPDATE SET sentence_cursor=excluded.sentence_cursor")
            .bind(context.id().to_string()).bind(cursor.map(|id| id.to_string())).execute(&self.pool).await?;
        Ok(())
    }
    /// Count a completed body check even when it requires no rewrite.
    pub async fn record_sentence_checked(&self, context: &VaultContext) -> Result<(), StateError> {
        sqlx::query(
            "UPDATE memory_dedup_progress SET sentence_checked=sentence_checked+1 WHERE vault_id=?",
        )
        .bind(context.id().to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod budget_tests {
    use super::*;
    #[tokio::test]
    async fn dispatch_accounting_does_not_cap_daily_work_and_expires_without_losing_decisions() {
        let state = crate::StateStore::connect_and_migrate("sqlite::memory:")
            .await
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let context = VaultContext::new(
            VaultId::new(),
            mcp_vault_domain::VaultSlug::new("budget-expiry").unwrap(),
            dir.path().join("vault"),
            Revision::ZERO,
        )
        .unwrap();
        state
            .vaults()
            .insert(&context, "budget", crate::VaultStatus::Active)
            .await
            .unwrap();
        let repository = state.current_memory();
        let key = format!("sha256:{}", "a".repeat(64));
        repository
            .save_equivalence_decision(&context, &key, "uncertain")
            .await
            .unwrap();
        // Exceed both former daily limits; real attempts remain accounted for.
        repository
            .record_equivalence_dispatch(&context, 4 * 1024 * 1024 + 1)
            .await
            .unwrap();
        for _ in 0..300 {
            repository
                .record_equivalence_dispatch(&context, 1)
                .await
                .unwrap();
        }
        let (count, bytes): (i64, i64) = sqlx::query_as(
            "SELECT count(*),sum(input_bytes) FROM memory_equivalence_dispatches WHERE vault_id=?",
        )
        .bind(context.id().to_string())
        .fetch_one(&repository.pool)
        .await
        .unwrap();
        assert_eq!(count, 301);
        assert_eq!(bytes, 4 * 1024 * 1024 + 301);
        // Simulate wall-clock passage in the synthetic repository fixture;
        // production reserve logic uses exactly the same rolling predicate.
        sqlx::query("UPDATE memory_equivalence_dispatches SET dispatched_at=? WHERE vault_id=?")
            .bind(now_millis().unwrap() - 86_400_001)
            .bind(context.id().to_string())
            .execute(&repository.pool)
            .await
            .unwrap();
        repository
            .record_equivalence_dispatch(&context, 1)
            .await
            .unwrap();
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM memory_equivalence_dispatches WHERE vault_id=?",
        )
        .bind(context.id().to_string())
        .fetch_one(&repository.pool)
        .await
        .unwrap();
        assert_eq!(count, 1);
        assert_eq!(
            repository
                .equivalence_decision(&context, &key)
                .await
                .unwrap()
                .as_deref(),
            Some("uncertain")
        );
    }
}
