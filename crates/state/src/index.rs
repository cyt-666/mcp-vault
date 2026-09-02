//! Vault-scoped Markdown and knowledge-map projection repositories.
//!
//! SQL for derived note metadata, FTS5, links, and index nodes lives here.
//! The projection is rebuildable and never replaces canonical Vault files.

use serde_json::Value;
use sqlx::{FromRow, QueryBuilder, Sqlite, SqlitePool};

use mcp_vault_domain::{FileId, Revision, VaultContext, VaultId, VaultPath};

use crate::{StateError, now_millis};

const MAX_QUERY_LIMIT: u32 = 100;

/// Parsed note metadata to persist as a derived projection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NoteProjectionInput {
    /// Stable file identity.
    pub file_id: FileId,
    /// Owning Vault.
    pub vault_id: VaultId,
    /// Vault-relative Markdown path.
    pub path: VaultPath,
    /// Canonical file revision analyzed.
    pub revision: Revision,
    /// Note title, when one can be derived.
    pub title: Option<String>,
    /// JSON array of aliases.
    pub aliases_json: String,
    /// Bounded frontmatter JSON object.
    pub frontmatter_json: String,
    /// Plain text projection used by FTS and snippets.
    pub plain_text: String,
    /// First paragraph, when present.
    pub first_paragraph: Option<String>,
    /// Optional language hint.
    pub language: Option<String>,
    /// Approximate Unicode word count.
    pub word_count: u64,
    /// Canonical content hash used to detect stale analysis.
    pub analyzed_content_hash: String,
    /// Analyzer schema/version.
    pub analyzer_version: u32,
    /// FTS aliases as a compact text field.
    pub fts_aliases: String,
    /// FTS tags as a compact text field.
    pub fts_tags: String,
    /// FTS heading titles as a compact text field.
    pub fts_headings: String,
}

/// One heading in a note projection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HeadingProjectionInput {
    /// Stable deterministic row ID.
    pub id: String,
    /// Heading ordinal in source order.
    pub ordinal: u32,
    /// Markdown heading level.
    pub level: u8,
    /// JSON array of ancestor heading titles.
    pub heading_path_json: String,
    /// Heading title.
    pub title: String,
    /// Inclusive source start byte.
    pub start_byte: u64,
    /// Exclusive source end byte, when known.
    pub end_byte: Option<u64>,
}

/// One tag extracted outside code spans or blocks.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TagProjectionInput {
    /// Display-preserving tag text.
    pub tag: String,
    /// Case-folded/search-normalized tag.
    pub normalized_tag: String,
    /// Frontmatter or inline source.
    pub source: String,
}

/// One Markdown or Obsidian link.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LinkProjectionInput {
    /// Stable deterministic row ID.
    pub id: String,
    /// Target text as written by the author.
    pub target_text: String,
    /// Resolved target file identity, when proven.
    pub target_file_id: Option<FileId>,
    /// Optional heading target.
    pub target_heading: Option<String>,
    /// Markdown, wikilink, or embed.
    pub link_type: String,
    /// Link ordinal in source order.
    pub ordinal: u32,
}

/// One deterministic knowledge-map node.
#[derive(Clone, Debug, PartialEq)]
pub struct IndexNodeProjectionInput {
    /// Stable database row ID scoped by Vault.
    pub id: String,
    /// Optional parent database row ID.
    pub parent_id: Option<String>,
    /// Root, folder, manual topic, or tag.
    pub node_type: String,
    /// Stable external node key returned to MCP clients.
    pub stable_key: String,
    /// Display title.
    pub title: String,
    /// Optional deterministic summary.
    pub summary: Option<String>,
    /// Projection source category.
    pub source_type: String,
    /// Optional source path/config reference.
    pub source_ref: Option<String>,
    /// Deterministic relevance/confidence.
    pub confidence: Option<f64>,
    /// Stable sibling ordering key.
    pub sort_key: String,
    /// Input content version.
    pub content_version: String,
    /// Created timestamp for a projection node.
    pub created_at: i64,
    /// Last update timestamp.
    pub updated_at: i64,
}

/// Membership of a note in a knowledge-map node.
#[derive(Clone, Debug, PartialEq)]
pub struct IndexMembershipProjectionInput {
    /// Database node ID.
    pub node_id: String,
    /// Stable file identity.
    pub file_id: FileId,
    /// Relevance within the node.
    pub relevance: f64,
    /// Folder, tag, or taxonomy source.
    pub source_type: String,
}

/// Per-Vault derived-index coverage/status.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IndexStatusRecord {
    /// Owning Vault.
    pub vault_id: VaultId,
    /// Monotonic derived projection revision.
    pub index_revision: Revision,
    /// Active filesystem entries considered.
    pub indexed_entries: u64,
    /// Markdown notes successfully analyzed.
    pub indexed_notes: u64,
    /// Canonical bytes analyzed.
    pub indexed_bytes: u64,
    /// Analyzer version.
    pub analyzer_version: u32,
    /// JSON coverage/degradation details.
    pub coverage: Value,
    /// Last successful full rebuild timestamp.
    pub last_rebuilt_at: Option<i64>,
    /// Safe error code from the last failed operation.
    pub last_error: Option<String>,
}

/// One deterministic knowledge-map node returned by queries.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IndexNodeRecord {
    /// Stable external node key.
    pub stable_key: String,
    /// Optional parent stable key.
    pub parent_key: Option<String>,
    /// Node type.
    pub node_type: String,
    /// Display title.
    pub title: String,
    /// Optional summary.
    pub summary: Option<String>,
    /// Projection source.
    pub source_type: String,
    /// Stable sibling sort key.
    pub sort_key: String,
    /// Number of direct note memberships.
    pub member_count: u64,
}

/// One note summary returned by browse/search queries.
#[derive(Clone, Debug, PartialEq)]
pub struct NoteSearchRecord {
    /// Stable file identity.
    pub file_id: FileId,
    /// Vault-relative path.
    pub path: VaultPath,
    /// Canonical revision analyzed.
    pub revision: Revision,
    /// Optional title.
    pub title: Option<String>,
    /// Modified timestamp from the note projection.
    pub updated_at: i64,
    /// Bounded source snippet.
    pub snippet: String,
    /// FTS rank where available.
    pub score: Option<f64>,
    /// Tags associated with the note.
    pub tags: Vec<String>,
    /// Stable knowledge-map topics associated with the note.
    pub topic_ids: Vec<String>,
    /// Heading title/anchor candidates.
    pub headings: Vec<String>,
    /// Outgoing Markdown/Obsidian links.
    pub outgoing_links: Vec<NoteLinkRecord>,
    /// Number of indexed incoming links.
    pub backlink_count: u64,
}

/// Current rebuildable note text used to derive semantic embedding chunks.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NoteEmbeddingSourceRecord {
    /// Stable canonical file identity.
    pub file_id: FileId,
    /// Current Vault-relative path.
    pub path: VaultPath,
    /// Canonical revision represented by this projection.
    pub revision: Revision,
    /// Optional note title.
    pub title: Option<String>,
    /// Ordered heading titles used as compact semantic context.
    pub headings: Vec<String>,
    /// Plain-text projection derived from canonical Markdown.
    pub plain_text: String,
    /// Canonical content hash analyzed into this projection.
    pub analyzed_content_hash: String,
}

/// One link projection returned with an indexed note summary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NoteLinkRecord {
    /// Stable link row identity.
    pub id: String,
    /// Source file identity.
    pub source_file_id: FileId,
    /// Target text as written in the source.
    pub target_text: String,
    /// Proven target file identity, when available.
    pub target_file_id: Option<FileId>,
    /// Optional target heading.
    pub target_heading: Option<String>,
    /// Markdown, wikilink, or embed.
    pub link_type: String,
    /// Source-order link ordinal.
    pub ordinal: u32,
}

#[derive(Clone, Debug, FromRow)]
struct NoteSearchRow {
    file_id: String,
    path: String,
    revision: i64,
    title: Option<String>,
    updated_at: i64,
    snippet: String,
    score: Option<f64>,
}

#[derive(Clone, Debug, FromRow)]
struct NoteEmbeddingSourceRow {
    file_id: String,
    path: String,
    revision: i64,
    title: Option<String>,
    plain_text: String,
    analyzed_content_hash: String,
}

/// Repository for all Markdown/index projections.
#[derive(Clone)]
pub struct IndexRepository {
    pool: SqlitePool,
}

impl IndexRepository {
    pub(crate) fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Replace one note and all of its subordinate projections atomically.
    pub async fn replace_note(
        &self,
        context: &VaultContext,
        note: &NoteProjectionInput,
        headings: &[HeadingProjectionInput],
        tags: &[TagProjectionInput],
        links: &[LinkProjectionInput],
    ) -> Result<(), StateError> {
        ensure_note_context(context, note)?;
        let now = now_millis()?;
        let mut transaction = self.pool.begin().await?;
        let file_id = note.file_id.to_string();
        let vault_id = context.id().to_string();

        sqlx::query("DELETE FROM note_headings WHERE vault_id = ? AND file_id = ?")
            .bind(&vault_id)
            .bind(&file_id)
            .execute(&mut *transaction)
            .await?;
        sqlx::query("DELETE FROM note_tags WHERE vault_id = ? AND file_id = ?")
            .bind(&vault_id)
            .bind(&file_id)
            .execute(&mut *transaction)
            .await?;
        sqlx::query("DELETE FROM note_links WHERE vault_id = ? AND source_file_id = ?")
            .bind(&vault_id)
            .bind(&file_id)
            .execute(&mut *transaction)
            .await?;
        sqlx::query("DELETE FROM note_fts WHERE vault_id = ? AND file_id = ?")
            .bind(&vault_id)
            .bind(&file_id)
            .execute(&mut *transaction)
            .await?;

        sqlx::query(
            "INSERT INTO notes
             (file_id, vault_id, path, revision, title, aliases_json,
              frontmatter_json, plain_text, first_paragraph, language,
              word_count, analyzed_content_hash, analyzer_version,
              created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(file_id) DO UPDATE SET
               vault_id = excluded.vault_id,
               path = excluded.path,
               revision = excluded.revision,
               title = excluded.title,
               aliases_json = excluded.aliases_json,
               frontmatter_json = excluded.frontmatter_json,
               plain_text = excluded.plain_text,
               first_paragraph = excluded.first_paragraph,
               language = excluded.language,
               word_count = excluded.word_count,
               analyzed_content_hash = excluded.analyzed_content_hash,
               analyzer_version = excluded.analyzer_version,
               updated_at = excluded.updated_at",
        )
        .bind(file_id.clone())
        .bind(vault_id.clone())
        .bind(note.path.as_str())
        .bind(note.revision.as_i64()?)
        .bind(note.title.as_deref())
        .bind(&note.aliases_json)
        .bind(&note.frontmatter_json)
        .bind(&note.plain_text)
        .bind(note.first_paragraph.as_deref())
        .bind(note.language.as_deref())
        .bind(
            i64::try_from(note.word_count)
                .map_err(|_| StateError::InvalidInput("word count overflow"))?,
        )
        .bind(&note.analyzed_content_hash)
        .bind(i64::from(note.analyzer_version))
        .bind(now)
        .bind(now)
        .execute(&mut *transaction)
        .await?;

        for heading in headings {
            sqlx::query(
                "INSERT INTO note_headings
                 (id, vault_id, file_id, ordinal, level, heading_path_json,
                  title, start_byte, end_byte)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(&heading.id)
            .bind(&vault_id)
            .bind(&file_id)
            .bind(i64::from(heading.ordinal))
            .bind(i64::from(heading.level))
            .bind(&heading.heading_path_json)
            .bind(&heading.title)
            .bind(
                i64::try_from(heading.start_byte)
                    .map_err(|_| StateError::InvalidInput("heading byte offset overflow"))?,
            )
            .bind(
                heading
                    .end_byte
                    .map(i64::try_from)
                    .transpose()
                    .map_err(|_| StateError::InvalidInput("heading byte offset overflow"))?,
            )
            .execute(&mut *transaction)
            .await?;
        }
        for tag in tags {
            sqlx::query(
                "INSERT INTO note_tags
                 (vault_id, file_id, tag, normalized_tag, source)
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(&vault_id)
            .bind(&file_id)
            .bind(&tag.tag)
            .bind(&tag.normalized_tag)
            .bind(&tag.source)
            .execute(&mut *transaction)
            .await?;
        }
        for link in links {
            sqlx::query(
                "INSERT INTO note_links
                 (id, vault_id, source_file_id, target_text, target_file_id,
                  target_heading, link_type, ordinal)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(&link.id)
            .bind(&vault_id)
            .bind(&file_id)
            .bind(&link.target_text)
            .bind(link.target_file_id.map(|id| id.to_string()))
            .bind(link.target_heading.as_deref())
            .bind(&link.link_type)
            .bind(i64::from(link.ordinal))
            .execute(&mut *transaction)
            .await?;
        }
        sqlx::query(
            "INSERT INTO note_fts
             (vault_id, file_id, path, title, aliases, tags, headings, plain_text)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&vault_id)
        .bind(&file_id)
        .bind(note.path.as_str())
        .bind(note.title.as_deref().unwrap_or_default())
        .bind(&note.fts_aliases)
        .bind(&note.fts_tags)
        .bind(&note.fts_headings)
        .bind(&note.plain_text)
        .execute(&mut *transaction)
        .await?;

        transaction.commit().await?;
        Ok(())
    }

    /// Remove one file's derived rows without touching canonical state.
    pub async fn remove_note(
        &self,
        context: &VaultContext,
        file_id: FileId,
    ) -> Result<(), StateError> {
        let vault_id = context.id().to_string();
        let file_id = file_id.to_string();
        let mut transaction = self.pool.begin().await?;
        for table in ["note_headings", "note_tags", "note_links", "note_fts"] {
            let sql = format!("DELETE FROM {table} WHERE vault_id = ? AND file_id = ?");
            sqlx::query(&sql)
                .bind(&vault_id)
                .bind(&file_id)
                .execute(&mut *transaction)
                .await?;
        }
        sqlx::query("DELETE FROM index_memberships WHERE vault_id = ? AND file_id = ?")
            .bind(&vault_id)
            .bind(&file_id)
            .execute(&mut *transaction)
            .await?;
        sqlx::query("DELETE FROM notes WHERE vault_id = ? AND file_id = ?")
            .bind(&vault_id)
            .bind(&file_id)
            .execute(&mut *transaction)
            .await?;
        transaction.commit().await?;
        Ok(())
    }

    /// Replace a Vault's deterministic knowledge map and coverage status.
    pub async fn replace_knowledge_map(
        &self,
        context: &VaultContext,
        nodes: &[IndexNodeProjectionInput],
        memberships: &[IndexMembershipProjectionInput],
        status: &IndexStatusRecord,
    ) -> Result<(), StateError> {
        if status.vault_id != context.id() {
            return Err(StateError::InvalidInput(
                "index status Vault does not match context",
            ));
        }
        let vault_id = context.id().to_string();
        let mut transaction = self.pool.begin().await?;
        sqlx::query("DELETE FROM index_memberships WHERE vault_id = ?")
            .bind(&vault_id)
            .execute(&mut *transaction)
            .await?;
        sqlx::query("DELETE FROM index_nodes WHERE vault_id = ? AND parent_id IS NULL")
            .bind(&vault_id)
            .execute(&mut *transaction)
            .await?;

        for node in nodes {
            sqlx::query(
                "INSERT INTO index_nodes
                 (id, vault_id, parent_id, node_type, stable_key, title, summary,
                  source_type, source_ref, confidence, sort_key, content_version,
                  created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(&node.id)
            .bind(&vault_id)
            .bind(node.parent_id.as_deref())
            .bind(&node.node_type)
            .bind(&node.stable_key)
            .bind(&node.title)
            .bind(node.summary.as_deref())
            .bind(&node.source_type)
            .bind(node.source_ref.as_deref())
            .bind(node.confidence)
            .bind(&node.sort_key)
            .bind(&node.content_version)
            .bind(node.created_at)
            .bind(node.updated_at)
            .execute(&mut *transaction)
            .await?;
        }
        for membership in memberships {
            sqlx::query(
                "INSERT INTO index_memberships
                 (vault_id, node_id, file_id, relevance, source_type)
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(&vault_id)
            .bind(&membership.node_id)
            .bind(membership.file_id.to_string())
            .bind(membership.relevance)
            .bind(&membership.source_type)
            .execute(&mut *transaction)
            .await?;
        }
        sqlx::query(
            "INSERT INTO index_status
             (vault_id, index_revision, indexed_entries, indexed_notes,
              indexed_bytes, analyzer_version, coverage_json, last_rebuilt_at,
              last_error)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(vault_id) DO UPDATE SET
               index_revision = excluded.index_revision,
               indexed_entries = excluded.indexed_entries,
               indexed_notes = excluded.indexed_notes,
               indexed_bytes = excluded.indexed_bytes,
               analyzer_version = excluded.analyzer_version,
               coverage_json = excluded.coverage_json,
               last_rebuilt_at = excluded.last_rebuilt_at,
               last_error = excluded.last_error",
        )
        .bind(&vault_id)
        .bind(status.index_revision.as_i64()?)
        .bind(
            i64::try_from(status.indexed_entries)
                .map_err(|_| StateError::InvalidInput("indexed entry count overflow"))?,
        )
        .bind(
            i64::try_from(status.indexed_notes)
                .map_err(|_| StateError::InvalidInput("indexed note count overflow"))?,
        )
        .bind(
            i64::try_from(status.indexed_bytes)
                .map_err(|_| StateError::InvalidInput("indexed byte count overflow"))?,
        )
        .bind(i64::from(status.analyzer_version))
        .bind(serde_json::to_string(&status.coverage)?)
        .bind(status.last_rebuilt_at)
        .bind(status.last_error.as_deref())
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    /// Delete all derived rows for one Vault before a rebuild.
    pub async fn clear_vault(&self, context: &VaultContext) -> Result<(), StateError> {
        let vault_id = context.id().to_string();
        let mut transaction = self.pool.begin().await?;
        for table in [
            "index_memberships",
            "index_nodes",
            "index_status",
            "note_fts",
            "note_links",
            "note_tags",
            "note_headings",
            "notes",
        ] {
            let sql = format!("DELETE FROM {table} WHERE vault_id = ?");
            sqlx::query(&sql)
                .bind(&vault_id)
                .execute(&mut *transaction)
                .await?;
        }
        transaction.commit().await?;
        Ok(())
    }

    /// Return the latest status for a Vault.
    pub async fn status(
        &self,
        context: &VaultContext,
    ) -> Result<Option<IndexStatusRecord>, StateError> {
        let row = sqlx::query_as::<_, IndexStatusRow>(
            "SELECT vault_id, index_revision, indexed_entries, indexed_notes,
                    indexed_bytes, analyzer_version, coverage_json,
                    last_rebuilt_at, last_error
             FROM index_status WHERE vault_id = ?",
        )
        .bind(context.id().to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_status).transpose()
    }

    /// List deterministic map nodes under an optional stable parent key.
    pub async fn list_nodes(
        &self,
        context: &VaultContext,
        parent_key: Option<&str>,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<IndexNodeRecord>, StateError> {
        validate_page(limit, offset)?;
        let vault_id = context.id().to_string();
        let mut query = QueryBuilder::<Sqlite>::new(
            "SELECT node.stable_key, parent.stable_key AS parent_key,
                    node.node_type, node.title, node.summary, node.source_type,
                    node.sort_key, COUNT(membership.file_id) AS member_count
             FROM index_nodes node
             LEFT JOIN index_nodes parent ON parent.id = node.parent_id
                AND parent.vault_id = ",
        );
        query.push_bind(&vault_id);
        query.push(
            " LEFT JOIN index_memberships membership
                ON membership.vault_id = node.vault_id
               AND membership.node_id = node.id",
        );
        query.push(" WHERE node.vault_id = ");
        query.push_bind(&vault_id);
        if let Some(parent_key) = parent_key {
            query.push(" AND parent.stable_key = ");
            query.push_bind(parent_key);
        } else {
            query.push(" AND node.parent_id IS NULL");
        }
        query.push(
            " GROUP BY node.id, parent.stable_key
              ORDER BY node.sort_key ASC, node.id ASC LIMIT ",
        );
        query.push_bind(i64::from(limit));
        query.push(" OFFSET ");
        query.push_bind(i64::from(offset));
        let rows = query
            .build_query_as::<IndexNodeRow>()
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter().map(row_to_node).collect()
    }

    /// List notes directly assigned to one knowledge-map node.
    pub async fn list_node_notes(
        &self,
        context: &VaultContext,
        stable_key: &str,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<NoteSearchRecord>, StateError> {
        validate_page(limit, offset)?;
        let rows = sqlx::query_as::<_, NoteSearchRow>(
            "SELECT n.file_id, file_state.path, n.revision, n.title, n.updated_at,
                    substr(n.plain_text, 1, 280) AS snippet, NULL AS score
             FROM index_nodes node
             JOIN index_memberships membership
               ON membership.vault_id = node.vault_id
              AND membership.node_id = node.id
             JOIN notes n
               ON n.vault_id = membership.vault_id
              AND n.file_id = membership.file_id
             JOIN file_entries file_state
               ON file_state.vault_id = n.vault_id
              AND file_state.id = n.file_id
              AND file_state.deleted_at IS NULL
             WHERE node.vault_id = ? AND node.stable_key = ?
             ORDER BY membership.relevance DESC, file_state.path ASC, n.file_id ASC
             LIMIT ? OFFSET ?",
        )
        .bind(context.id().to_string())
        .bind(stable_key)
        .bind(i64::from(limit))
        .bind(i64::from(offset))
        .fetch_all(&self.pool)
        .await?;
        let mut result = Vec::with_capacity(rows.len());
        for row in rows {
            result.push(row_to_search(row, &self.pool, context.id()).await?);
        }
        Ok(result)
    }

    /// Return lexical/tag/link related notes with deterministic scoring.
    pub async fn related_notes(
        &self,
        context: &VaultContext,
        file_id: FileId,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<NoteSearchRecord>, StateError> {
        validate_page(limit, offset)?;
        let rows = sqlx::query_as::<_, NoteSearchRow>(
            "WITH shared_tags AS (
                 SELECT candidate.file_id, COUNT(*) AS shared_count
                 FROM note_tags source_tag
                 JOIN note_tags candidate_tag
                   ON candidate_tag.vault_id = source_tag.vault_id
                  AND candidate_tag.normalized_tag = source_tag.normalized_tag
                 JOIN notes candidate
                   ON candidate.vault_id = candidate_tag.vault_id
                  AND candidate.file_id = candidate_tag.file_id
                 WHERE source_tag.vault_id = ? AND source_tag.file_id = ?
                   AND candidate.file_id != ?
                 GROUP BY candidate.file_id
             )
             SELECT candidate.file_id, file_state.path, candidate.revision,
                    candidate.title, candidate.updated_at,
                    substr(candidate.plain_text, 1, 280) AS snippet,
                    CAST(
                      COALESCE(shared_tags.shared_count, 0)
                      + CASE WHEN EXISTS (
                          SELECT 1 FROM note_links direct_link
                          WHERE direct_link.vault_id = candidate.vault_id
                            AND (
                              (direct_link.source_file_id = ? AND direct_link.target_file_id = candidate.file_id)
                              OR
                              (direct_link.source_file_id = candidate.file_id AND direct_link.target_file_id = ?)
                            )
                        ) THEN 2 ELSE 0 END
                      AS REAL
                    ) AS score
             FROM notes candidate
             JOIN file_entries file_state
               ON file_state.vault_id = candidate.vault_id
              AND file_state.id = candidate.file_id
              AND file_state.deleted_at IS NULL
             LEFT JOIN shared_tags ON shared_tags.file_id = candidate.file_id
             WHERE candidate.vault_id = ? AND candidate.file_id != ?
               AND (
                 shared_tags.file_id IS NOT NULL
                 OR EXISTS (
                     SELECT 1 FROM note_links direct_link
                     WHERE direct_link.vault_id = candidate.vault_id
                       AND (
                         (direct_link.source_file_id = ? AND direct_link.target_file_id = candidate.file_id)
                         OR
                         (direct_link.source_file_id = candidate.file_id AND direct_link.target_file_id = ?)
                       )
                 )
               )
             ORDER BY score DESC, file_state.path ASC, candidate.file_id ASC
             LIMIT ? OFFSET ?",
        )
        .bind(context.id().to_string())
        .bind(file_id.to_string())
        .bind(file_id.to_string())
        .bind(file_id.to_string())
        .bind(file_id.to_string())
        .bind(context.id().to_string())
        .bind(file_id.to_string())
        .bind(file_id.to_string())
        .bind(file_id.to_string())
        .bind(i64::from(limit))
        .bind(i64::from(offset))
        .fetch_all(&self.pool)
        .await?;
        let mut result = Vec::with_capacity(rows.len());
        for row in rows {
            result.push(row_to_search(row, &self.pool, context.id()).await?);
        }
        Ok(result)
    }

    /// List current note text projections for bounded semantic embedding work.
    pub async fn list_note_embedding_sources(
        &self,
        context: &VaultContext,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<NoteEmbeddingSourceRecord>, StateError> {
        validate_page(limit, offset)?;
        let rows = sqlx::query_as::<_, NoteEmbeddingSourceRow>(
            "SELECT file_id, path, revision, title, plain_text,
                    analyzed_content_hash
             FROM notes WHERE vault_id = ?
             ORDER BY path ASC, file_id ASC LIMIT ? OFFSET ?",
        )
        .bind(context.id().to_string())
        .bind(i64::from(limit))
        .bind(i64::from(offset))
        .fetch_all(&self.pool)
        .await?;
        let mut sources = Vec::with_capacity(rows.len());
        for row in rows {
            sources.push(note_embedding_source(row, &self.pool, context.id()).await?);
        }
        Ok(sources)
    }

    /// Resolve one current note projection by stable file identity.
    pub async fn get_note_embedding_source(
        &self,
        context: &VaultContext,
        file_id: FileId,
    ) -> Result<Option<NoteEmbeddingSourceRecord>, StateError> {
        let row = sqlx::query_as::<_, NoteEmbeddingSourceRow>(
            "SELECT file_id, path, revision, title, plain_text,
                    analyzed_content_hash
             FROM notes WHERE vault_id = ? AND file_id = ?",
        )
        .bind(context.id().to_string())
        .bind(file_id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        match row {
            Some(row) => Ok(Some(
                note_embedding_source(row, &self.pool, context.id()).await?,
            )),
            None => Ok(None),
        }
    }

    /// Return one indexed note as a bounded retrieval cue.
    pub async fn get_note_for_retrieval(
        &self,
        context: &VaultContext,
        file_id: FileId,
    ) -> Result<Option<NoteSearchRecord>, StateError> {
        let row = sqlx::query_as::<_, NoteSearchRow>(
            "SELECT n.file_id, file_state.path, n.revision, n.title, n.updated_at,
                    substr(n.plain_text, 1, 280) AS snippet, NULL AS score
             FROM notes n
             JOIN file_entries file_state
               ON file_state.vault_id = n.vault_id
              AND file_state.id = n.file_id
              AND file_state.deleted_at IS NULL
             WHERE n.vault_id = ? AND n.file_id = ?",
        )
        .bind(context.id().to_string())
        .bind(file_id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        match row {
            Some(row) => Ok(Some(row_to_search(row, &self.pool, context.id()).await?)),
            None => Ok(None),
        }
    }

    /// Search indexed Markdown using a caller-sanitized FTS5 query.
    #[allow(clippy::too_many_arguments)]
    pub async fn search_notes(
        &self,
        context: &VaultContext,
        fts_query: &str,
        path_prefix: Option<&str>,
        tags: &[String],
        topic_keys: &[String],
        modified_after: Option<i64>,
        modified_before: Option<i64>,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<NoteSearchRecord>, StateError> {
        validate_page(limit, offset)?;
        if fts_query.is_empty() || fts_query.len() > 4096 {
            return Err(StateError::InvalidInput("FTS query is invalid"));
        }
        let vault_id = context.id().to_string();
        let mut query = QueryBuilder::<Sqlite>::new(
            "SELECT n.file_id, file_state.path, n.revision, n.title, n.updated_at,
                    snippet(note_fts, 7, '', '', ' … ', 32) AS snippet,
                    bm25(note_fts) AS score
             FROM note_fts
             JOIN notes n ON n.vault_id = note_fts.vault_id
                          AND n.file_id = note_fts.file_id
             JOIN file_entries file_state
               ON file_state.vault_id = n.vault_id
              AND file_state.id = n.file_id
              AND file_state.deleted_at IS NULL
             WHERE note_fts.vault_id = ",
        );
        query.push_bind(&vault_id);
        query.push(" AND note_fts MATCH ");
        query.push_bind(fts_query);
        if let Some(path_prefix) = path_prefix {
            query.push(" AND file_state.path LIKE ");
            query.push_bind(format!("{path_prefix}%"));
        }
        for tag in tags {
            query.push(
                " AND EXISTS (
                    SELECT 1 FROM note_tags nt
                    WHERE nt.vault_id = n.vault_id
                      AND nt.file_id = n.file_id
                      AND nt.normalized_tag = ",
            );
            query.push_bind(tag);
            query.push(")");
        }
        for topic_key in topic_keys {
            query.push(
                " AND EXISTS (
                    SELECT 1 FROM index_memberships topic_membership
                    JOIN index_nodes topic_node
                      ON topic_node.vault_id = topic_membership.vault_id
                     AND topic_node.id = topic_membership.node_id
                   WHERE topic_membership.vault_id = n.vault_id
                     AND topic_membership.file_id = n.file_id
                     AND topic_node.stable_key = ",
            );
            query.push_bind(topic_key);
            query.push(")");
        }
        if let Some(modified_after) = modified_after {
            query.push(" AND n.updated_at >= ");
            query.push_bind(modified_after);
        }
        if let Some(modified_before) = modified_before {
            query.push(" AND n.updated_at <= ");
            query.push_bind(modified_before);
        }
        query.push(" ORDER BY score ASC, file_state.path ASC, n.file_id ASC LIMIT ");
        query.push_bind(i64::from(limit));
        query.push(" OFFSET ");
        query.push_bind(i64::from(offset));
        let rows = query
            .build_query_as::<NoteSearchRow>()
            .fetch_all(&self.pool)
            .await?;
        let mut result = Vec::with_capacity(rows.len());
        for row in rows {
            result.push(row_to_search(row, &self.pool, context.id()).await?);
        }
        Ok(result)
    }
}

#[derive(Debug, FromRow)]
struct IndexStatusRow {
    vault_id: String,
    index_revision: i64,
    indexed_entries: i64,
    indexed_notes: i64,
    indexed_bytes: i64,
    analyzer_version: i64,
    coverage_json: String,
    last_rebuilt_at: Option<i64>,
    last_error: Option<String>,
}

#[derive(Debug, FromRow)]
struct IndexNodeRow {
    stable_key: String,
    parent_key: Option<String>,
    node_type: String,
    title: String,
    summary: Option<String>,
    source_type: String,
    sort_key: String,
    member_count: i64,
}

fn ensure_note_context(
    context: &VaultContext,
    note: &NoteProjectionInput,
) -> Result<(), StateError> {
    if note.vault_id != context.id() {
        return Err(StateError::InvalidInput(
            "note projection Vault does not match context",
        ));
    }
    Ok(())
}

fn validate_page(limit: u32, offset: u32) -> Result<(), StateError> {
    if limit == 0 || limit > MAX_QUERY_LIMIT || offset > 1_000_000 {
        return Err(StateError::InvalidInput("index query page is invalid"));
    }
    Ok(())
}

fn row_to_status(row: IndexStatusRow) -> Result<IndexStatusRecord, StateError> {
    Ok(IndexStatusRecord {
        vault_id: row.vault_id.parse().map_err(StateError::InvalidDomain)?,
        index_revision: Revision::try_from(row.index_revision)?,
        indexed_entries: u64::try_from(row.indexed_entries)
            .map_err(|_| StateError::InvalidInput("indexed entry count is invalid"))?,
        indexed_notes: u64::try_from(row.indexed_notes)
            .map_err(|_| StateError::InvalidInput("indexed note count is invalid"))?,
        indexed_bytes: u64::try_from(row.indexed_bytes)
            .map_err(|_| StateError::InvalidInput("indexed byte count is invalid"))?,
        analyzer_version: u32::try_from(row.analyzer_version)
            .map_err(|_| StateError::InvalidInput("analyzer version is invalid"))?,
        coverage: serde_json::from_str(&row.coverage_json)?,
        last_rebuilt_at: row.last_rebuilt_at,
        last_error: row.last_error,
    })
}

fn row_to_node(row: IndexNodeRow) -> Result<IndexNodeRecord, StateError> {
    Ok(IndexNodeRecord {
        stable_key: row.stable_key,
        parent_key: row.parent_key,
        node_type: row.node_type,
        title: row.title,
        summary: row.summary,
        source_type: row.source_type,
        sort_key: row.sort_key,
        member_count: u64::try_from(row.member_count)
            .map_err(|_| StateError::InvalidInput("index member count is invalid"))?,
    })
}

async fn row_to_search(
    row: NoteSearchRow,
    pool: &SqlitePool,
    vault_id: VaultId,
) -> Result<NoteSearchRecord, StateError> {
    let file_id = row
        .file_id
        .parse::<FileId>()
        .map_err(StateError::InvalidDomain)?;
    let path = VaultPath::parse(&row.path)?;
    let tags = sqlx::query_scalar::<_, String>(
        "SELECT tag FROM note_tags
         WHERE vault_id = ? AND file_id = ?
         ORDER BY normalized_tag ASC",
    )
    .bind(vault_id.to_string())
    .bind(file_id.to_string())
    .fetch_all(pool)
    .await?;
    let topic_ids = sqlx::query_scalar::<_, String>(
        "SELECT node.stable_key
         FROM index_memberships membership
         JOIN index_nodes node
           ON node.vault_id = membership.vault_id
          AND node.id = membership.node_id
         WHERE membership.vault_id = ? AND membership.file_id = ?
           AND node.node_type IN ('topic', 'manual_topic', 'tag')
         ORDER BY node.stable_key ASC",
    )
    .bind(vault_id.to_string())
    .bind(file_id.to_string())
    .fetch_all(pool)
    .await?;
    let headings = sqlx::query_scalar::<_, String>(
        "SELECT title FROM note_headings
         WHERE vault_id = ? AND file_id = ?
         ORDER BY ordinal ASC",
    )
    .bind(vault_id.to_string())
    .bind(file_id.to_string())
    .fetch_all(pool)
    .await?;
    let link_rows = sqlx::query_as::<_, NoteLinkRow>(
        "SELECT id, source_file_id, target_text, target_file_id,
                target_heading, link_type, ordinal
         FROM note_links
         WHERE vault_id = ? AND source_file_id = ?
         ORDER BY ordinal ASC",
    )
    .bind(vault_id.to_string())
    .bind(file_id.to_string())
    .fetch_all(pool)
    .await?;
    let outgoing_links = link_rows
        .into_iter()
        .map(row_to_link)
        .collect::<Result<Vec<_>, _>>()?;
    let backlink_count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM note_links
         WHERE vault_id = ? AND target_file_id = ?",
    )
    .bind(vault_id.to_string())
    .bind(file_id.to_string())
    .fetch_one(pool)
    .await?;
    Ok(NoteSearchRecord {
        file_id,
        path,
        revision: Revision::try_from(row.revision)?,
        title: row.title,
        updated_at: row.updated_at,
        snippet: row.snippet,
        score: row.score,
        tags,
        topic_ids,
        headings,
        outgoing_links,
        backlink_count: u64::try_from(backlink_count)
            .map_err(|_| StateError::InvalidInput("backlink count is invalid"))?,
    })
}

async fn note_embedding_source(
    row: NoteEmbeddingSourceRow,
    pool: &SqlitePool,
    vault_id: VaultId,
) -> Result<NoteEmbeddingSourceRecord, StateError> {
    let file_id = FileId::parse(&row.file_id)?;
    let headings = sqlx::query_scalar::<_, String>(
        "SELECT title FROM note_headings
         WHERE vault_id = ? AND file_id = ? ORDER BY ordinal ASC",
    )
    .bind(vault_id.to_string())
    .bind(file_id.to_string())
    .fetch_all(pool)
    .await?;
    Ok(NoteEmbeddingSourceRecord {
        file_id,
        path: VaultPath::parse(&row.path)?,
        revision: Revision::try_from(row.revision)?,
        title: row.title,
        headings,
        plain_text: row.plain_text,
        analyzed_content_hash: row.analyzed_content_hash,
    })
}

#[derive(Debug, FromRow)]
struct NoteLinkRow {
    id: String,
    source_file_id: String,
    target_text: String,
    target_file_id: Option<String>,
    target_heading: Option<String>,
    link_type: String,
    ordinal: i64,
}

fn row_to_link(row: NoteLinkRow) -> Result<NoteLinkRecord, StateError> {
    Ok(NoteLinkRecord {
        id: row.id,
        source_file_id: row
            .source_file_id
            .parse()
            .map_err(StateError::InvalidDomain)?,
        target_text: row.target_text,
        target_file_id: row
            .target_file_id
            .map(|value| value.parse())
            .transpose()
            .map_err(StateError::InvalidDomain)?,
        target_heading: row.target_heading,
        link_type: row.link_type,
        ordinal: u32::try_from(row.ordinal)
            .map_err(|_| StateError::InvalidInput("link ordinal is invalid"))?,
    })
}
