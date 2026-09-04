//! Public memory application DTOs independent of HTTP/RMCP.

use std::collections::BTreeMap;

use mcp_vault_domain::{FileId, JobId, MemoryId, MemoryRawId, Revision, VaultPath};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Supported initial memory types.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryType {
    /// Stable user/project identity.
    Identity,
    /// Durable working preference.
    Preference,
    /// Explicit design or process decision.
    Decision,
    /// Constraint that limits future work.
    Constraint,
    /// Durable fact.
    Fact,
    /// Project definition or ownership.
    Project,
    /// Current project/task progress.
    Progress,
    /// Important dated event.
    Event,
    /// Meaningful relation between entities.
    Relationship,
    /// Reusable procedure.
    Procedure,
}

impl MemoryType {
    /// Stable database label.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Identity => "identity",
            Self::Preference => "preference",
            Self::Decision => "decision",
            Self::Constraint => "constraint",
            Self::Fact => "fact",
            Self::Project => "project",
            Self::Progress => "progress",
            Self::Event => "event",
            Self::Relationship => "relationship",
            Self::Procedure => "procedure",
        }
    }
}

impl TryFrom<&str> for MemoryType {
    type Error = ();

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "identity" => Ok(Self::Identity),
            "preference" => Ok(Self::Preference),
            "decision" => Ok(Self::Decision),
            "constraint" => Ok(Self::Constraint),
            "fact" => Ok(Self::Fact),
            "project" => Ok(Self::Project),
            "progress" => Ok(Self::Progress),
            "event" => Ok(Self::Event),
            "relationship" => Ok(Self::Relationship),
            "procedure" => Ok(Self::Procedure),
            _ => Err(()),
        }
    }
}

/// Lifecycle state visible to callers.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryStatus {
    /// Legacy prerelease candidate projection retained for migration parsing.
    Candidate,
    /// Eligible for normal recall and canonical Markdown.
    Active,
    /// Replaced by a newer memory.
    Superseded,
    /// No current source supports the proposition.
    Stale,
    /// Intentionally inactive but retained.
    Archived,
    /// Legacy/manual rejected lifecycle value retained for compatibility.
    Rejected,
    /// Invalid managed Markdown excluded from recall.
    Quarantined,
}

impl MemoryStatus {
    /// Stable database label.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Candidate => "candidate",
            Self::Active => "active",
            Self::Superseded => "superseded",
            Self::Stale => "stale",
            Self::Archived => "archived",
            Self::Rejected => "rejected",
            Self::Quarantined => "quarantined",
        }
    }
}

impl TryFrom<&str> for MemoryStatus {
    type Error = ();

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "candidate" => Ok(Self::Candidate),
            "active" => Ok(Self::Active),
            "superseded" => Ok(Self::Superseded),
            "stale" => Ok(Self::Stale),
            "archived" => Ok(Self::Archived),
            "rejected" => Ok(Self::Rejected),
            "quarantined" => Ok(Self::Quarantined),
            _ => Err(()),
        }
    }
}

/// Origin of a durable memory.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryOrigin {
    /// Consolidated from model-distilled note sources.
    Extracted,
    /// Explicit Agent assertion.
    ExplicitAgent,
    /// Explicit Admin assertion.
    ExplicitAdmin,
    /// Reconciled direct managed Markdown edit.
    DirectMarkdown,
    /// Imported memory record.
    Import,
}

impl MemoryOrigin {
    /// Stable database label.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Extracted => "extracted",
            Self::ExplicitAgent => "explicit_agent",
            Self::ExplicitAdmin => "explicit_admin",
            Self::DirectMarkdown => "direct_markdown",
            Self::Import => "import",
        }
    }
}

/// Which ordinary Markdown revisions may be evaluated for automatic memory.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtractionSourceMode {
    /// Evaluate ordinary notes after the Vault-level feature is enabled.
    #[serde(alias = "explicit_only", alias = "all_notes")]
    #[default]
    Automatic,
}

/// Vault-scoped automatic Phase 1 extraction policy.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct ExtractionPolicy {
    /// Admit future Markdown changes and explicit backfill runs for extraction.
    pub enabled: bool,
    /// Serialized source-admission mode; legacy marker modes migrate to automatic.
    pub source_mode: ExtractionSourceMode,
    /// Legacy compatibility field retained for prerelease Admin payloads.
    /// Phase 1 v3 derives whole-source provenance locally and ignores it.
    #[serde(alias = "max_candidates_per_note")]
    pub max_evidence_per_note: u32,
    /// Total deadline for one structured note-extraction request.
    pub request_timeout_seconds: u64,
}

impl Default for ExtractionPolicy {
    fn default() -> Self {
        Self {
            enabled: false,
            source_mode: ExtractionSourceMode::Automatic,
            max_evidence_per_note: 3,
            request_timeout_seconds: 300,
        }
    }
}

/// Extraction policy together with its optimistic settings revision.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ExtractionPolicyState {
    /// Effective typed policy; absent storage resolves to the safe default.
    pub policy: ExtractionPolicy,
    /// Persisted settings revision, absent for the implicit default.
    pub revision: Option<Revision>,
}

/// Redacted readiness projection for Admin and durable job admission.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct ExtractionReadiness {
    /// True only when policy, provider mode, binding, model, and provider permit extraction.
    pub ready: bool,
    /// Stable non-secret blocker codes.
    pub blockers: Vec<String>,
    /// Selected provider identity when resolvable.
    pub provider_id: Option<String>,
    /// Selected internal model identity when resolvable.
    pub model_id: Option<String>,
    /// Provider-specific model identifier for display.
    pub external_model_id: Option<String>,
}

/// Bounded outcome of processing one current Markdown note.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct NoteExtractionResult {
    /// Whether the current note passed local bounds and reached the provider.
    pub source_admitted: bool,
    /// Whether Phase 1 stored a non-empty consolidation-ready raw memory.
    pub raw_memory_staged: bool,
    /// Whether Phase 1 successfully decided that the source has no memory input.
    pub no_output: bool,
    /// True when an unchanged note/profile was skipped before a Provider call.
    pub already_evaluated: bool,
}

/// Per-call behavior for automatic note extraction.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct NoteExtractionOptions {
    /// Re-evaluate a current successful note/profile at explicit operator cost.
    pub include_evaluated: bool,
}

/// Outcome of reconciling one required fresh pipeline regeneration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PipelineRegenerationAdmission {
    /// The Vault does not require a fresh regeneration.
    NotPending,
    /// One or both memory model phases are not ready yet.
    AwaitingConfiguration,
    /// Another extraction currently owns the singleton slot.
    AwaitingOtherExtraction,
    /// A current-generation fresh extraction exists and pending state cleared.
    Admitted,
}

/// Outcome of one committed Phase 2 global-memory consolidation.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct MemoryConsolidationReport {
    /// Dirty Phase 1 inputs consumed by this generation.
    pub raw_inputs: u32,
    /// New semantic final memories created.
    pub created: u32,
    /// Existing semantic memories updated.
    pub updated: u32,
    /// Existing memories archived or superseded.
    pub retired: u32,
    /// Raw inputs intentionally discarded as low-signal/temporary.
    pub discarded: u32,
    /// Monotonic committed global-memory generation.
    pub generation: u64,
    /// Whether a crash-safe prepared proposal was reused without another model call.
    pub reused_proposal: bool,
}

/// Current multilingual retrieval coverage for one Vault.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct MemoryRetrievalCoverageView {
    /// Stable retrieval metadata contract version.
    pub prompt_version: String,
    /// Hash of the alias targets, prompt contract, and lexical profile.
    pub profile_hash: String,
    /// Languages generated for each memory after source-language deduplication.
    pub target_languages: Vec<String>,
    /// Active, stale, and superseded memories eligible for backfill.
    pub eligible: u64,
    /// Current metadata matching exact memory content.
    pub current: u64,
    /// Missing or explicitly pending metadata.
    pub pending: u64,
    /// Current content whose latest enrichment failed.
    pub failed: u64,
    /// Maximum eight-item Provider batches required for uncovered memory.
    pub estimated_batches: u64,
}

/// Current durable-memory vector projection status for one Vault/model binding.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct MemoryEmbeddingStatusView {
    /// Whether an effective `embedding_memory` model is bound.
    pub configured: bool,
    /// Whether current Vault Provider policy permits embedding calls.
    pub provider_mode_enabled: bool,
    /// Selected internal model identity.
    pub model_id: Option<String>,
    /// Provider-visible model identifier.
    pub external_model_id: Option<String>,
    /// Active, stale, and superseded memories eligible for vector projection.
    pub eligible: u64,
    /// Current-model vectors matching exact current memory content.
    pub current: u64,
    /// Current-model vector rows that do not match the current projection.
    pub stale: u64,
    /// Stable redacted readiness blockers.
    pub blockers: Vec<String>,
}

/// Outcome of admitting missing current-memory vectors to durable jobs.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct MemoryEmbeddingScheduleReport {
    /// Eligible current memory records considered.
    pub eligible: u64,
    /// Already-current vectors for the selected model.
    pub current: u64,
    /// Missing current vectors admitted or deduplicated.
    pub queued: u64,
    /// Obsolete selected-model vectors removed before admission.
    pub pruned: u64,
    /// Durable `embedding.rebuild` jobs admitted or deduplicated.
    pub jobs: u64,
    /// Selected internal model identity.
    pub model_id: Option<String>,
    /// Provider-visible model identifier.
    pub external_model_id: Option<String>,
}

/// Outcome of one bounded multilingual retrieval-enrichment batch.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct MemoryRetrievalEnrichmentReport {
    /// Memories present in the prepared batch.
    pub processed: u32,
    /// Metadata rows made current.
    pub enriched: u32,
    /// Canonical bodies equivalently rewritten into source language.
    pub rewritten: u32,
    /// Items kept in their existing language because safe rewrite was unavailable.
    pub rewrite_skipped: u32,
    /// Items re-admitted because their revision/content snapshot changed.
    pub snapshot_conflicts: u32,
    /// Remaining explicitly admitted rows after this batch.
    pub remaining: u64,
    /// Whether a persisted proposal avoided a second Provider call.
    pub reused_proposal: bool,
}

/// Outcome of one idempotent historical memory-source repair pass.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct MemorySourceRepairReport {
    /// Canonical memory records rewritten with current source metadata.
    pub memories_rewritten: u64,
    /// Phase 1 source rows rebound to their current paths/revisions.
    pub stage1_sources_rebound: u64,
    /// Note sources whose current readable file identity cannot be proven.
    pub unresolved_note_sources: u64,
    /// Active extracted memories made stale because no support remains.
    pub memories_marked_stale: u64,
}

/// Exact outcomes of one event-driven source reconciliation pass.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct MemorySourceReconcileReport {
    /// Final-memory note sources checked.
    pub final_sources_checked: u64,
    /// Sources proven current without changing File ID.
    pub current: u64,
    /// Sources rebound to one uniquely proven current File ID.
    pub rebound: u64,
    /// Sources whose stable file no longer contains the evidence.
    pub changed: u64,
    /// Sources whose stable file is tombstoned.
    pub deleted: u64,
    /// Sources for which no current identity can be proven.
    pub missing: u64,
    /// Sources for which exact identity is not unique.
    pub ambiguous: u64,
    /// Memories newly made stale by unavailable note evidence.
    pub memories_staled: u64,
    /// Source-unavailable memories reactivated by exact proof.
    pub memories_reactivated: u64,
    /// Stage 1 rows rebound to current source metadata.
    pub stage1_rebound: u64,
    /// Stage 1 rows withdrawn after source loss.
    pub stage1_withdrawn: u64,
    /// Individual sources/memories that could not be processed safely.
    pub errors: u64,
}

/// One durable page of the repeatable source audit.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct MemorySourceAuditPage {
    /// Per-page exact reconciliation outcomes.
    pub report: MemorySourceReconcileReport,
    /// Last committed final source identity.
    pub cursor: Option<String>,
    /// Whether every final and Stage 1 source has been examined.
    pub complete: bool,
}

/// Result of one destructive prerelease memory-pipeline cutover.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MemoryPipelineResetReport {
    /// Whether the current pipeline generation already completed cleanup.
    pub already_completed: bool,
    /// Managed memory Markdown files removed through Vault Core.
    pub removed_managed_files: u64,
    /// Final memory projections removed.
    pub cleared_memories: u64,
    /// Phase 1 source outputs removed.
    pub cleared_stage1_outputs: u64,
    /// Candidate projections removed.
    pub cleared_candidates: u64,
    /// Consolidation proposals removed.
    pub cleared_proposals: u64,
    /// Managed-file diagnostics removed.
    pub cleared_diagnostics: u64,
    /// Derived memory embedding records removed.
    pub cleared_embeddings: u64,
}

/// Provenance input supplied by an explicit command or extraction validator.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct MemorySourceInput {
    /// Note, explicit_agent, explicit_admin, direct_markdown, or import.
    pub source_type: String,
    /// Source note identity when known.
    pub note_file_id: Option<FileId>,
    /// Source note path when known.
    pub note_path: Option<VaultPath>,
    /// Source note revision when known.
    pub note_revision: Option<Revision>,
    /// Heading path anchor.
    pub heading_path: Vec<String>,
    /// Inclusive source start line.
    pub start_line: Option<u32>,
    /// Inclusive source end line.
    pub end_line: Option<u32>,
    /// Bounded source excerpt hash.
    pub excerpt_hash: Option<String>,
    /// Redacted actor identity.
    pub actor_id: Option<String>,
}

/// Explicit durable memory command.
#[derive(Clone, Debug, PartialEq)]
pub struct RememberInput {
    /// Atomic proposition body.
    pub content: String,
    /// Memory type.
    pub memory_type: MemoryType,
    /// Importance in [0, 1].
    pub importance: f64,
    /// Confidence in [0, 1].
    pub confidence: f64,
    /// Optional validity start timestamp.
    pub valid_from: Option<i64>,
    /// Optional validity end timestamp.
    pub valid_to: Option<i64>,
    /// Display/search tags.
    pub tags: Vec<String>,
    /// Search entities.
    pub entities: Vec<String>,
    /// Provenance sources.
    pub sources: Vec<MemorySourceInput>,
    /// Optional explicit supersession target.
    pub supersedes: Option<MemoryId>,
    /// Explicit command idempotency key.
    pub idempotency_key: Option<String>,
    /// Origin, normally explicit_agent for this command.
    pub origin: MemoryOrigin,
    /// Provider/prompt/pipeline metadata for extracted memories.
    pub extraction: Value,
}

impl Default for RememberInput {
    fn default() -> Self {
        Self {
            content: String::new(),
            memory_type: MemoryType::Fact,
            importance: 0.5,
            confidence: 0.5,
            valid_from: None,
            valid_to: None,
            tags: Vec::new(),
            entities: Vec::new(),
            sources: Vec::new(),
            supersedes: None,
            idempotency_key: None,
            origin: MemoryOrigin::ExplicitAgent,
            extraction: Value::Object(Default::default()),
        }
    }
}

/// Optional continuity signals supplied by the MCP client.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RecallContext {
    /// Current project name.
    pub active_project: Option<String>,
    /// Current entities.
    pub entities: Vec<String>,
    /// Recent topics.
    pub recent_topics: Vec<String>,
}

/// Recall request with bounded hybrid ranking controls.
#[derive(Clone, Debug, PartialEq)]
pub struct RecallRequest {
    /// Current task/question.
    pub query: String,
    /// Optional continuity context.
    pub context: RecallContext,
    /// Type filters.
    pub types: Vec<MemoryType>,
    /// Optional validity point/range start.
    pub valid_at: Option<i64>,
    /// Minimum importance.
    pub min_importance: f64,
    /// Include stale/superseded/archived/rejected history.
    pub include_historical: bool,
    /// Return provenance sources.
    pub include_sources: bool,
    /// Return score breakdown.
    pub include_score_breakdown: bool,
    /// Include ordinary-note cues only when the caller can also read Vault notes.
    pub include_related_notes: bool,
    /// Maximum memories.
    pub max_results: u32,
    /// Maximum ordinary-note cues.
    pub max_related_notes: u32,
    /// Approximate output token budget.
    pub max_tokens: u32,
}

impl Default for RecallRequest {
    fn default() -> Self {
        Self {
            query: String::new(),
            context: RecallContext::default(),
            types: Vec::new(),
            valid_at: None,
            min_importance: 0.0,
            include_historical: false,
            include_sources: false,
            include_score_breakdown: false,
            include_related_notes: true,
            max_results: 12,
            max_related_notes: 8,
            max_tokens: 1800,
        }
    }
}

/// Result of explicit remember.
#[derive(Clone, Debug, PartialEq)]
pub struct RememberResult {
    /// staged for consolidation or an idempotent prior staging result.
    pub outcome: String,
    /// Final memory when returned by a legacy read path; staging returns none.
    pub memory: Option<MemoryView>,
    /// Phase 1 raw-memory input identity.
    pub raw_memory_id: Option<MemoryRawId>,
    /// Admitted Phase 2 job identity.
    pub consolidation_job_id: Option<JobId>,
}

/// Revision-aware durable memory update.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MemoryUpdateInput {
    /// Replacement body, when supplied.
    pub content: Option<String>,
    /// Replacement type, when supplied.
    pub memory_type: Option<MemoryType>,
    /// Replacement importance.
    pub importance: Option<f64>,
    /// Replacement confidence.
    pub confidence: Option<f64>,
    /// Replacement validity start.
    pub valid_from: Option<Option<i64>>,
    /// Replacement validity end.
    pub valid_to: Option<Option<i64>>,
    /// Replacement tags.
    pub tags: Option<Vec<String>>,
    /// Replacement entities.
    pub entities: Option<Vec<String>>,
}

/// Compact memory output used by recall and get/list operations.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MemoryView {
    /// Stable identity.
    pub id: MemoryId,
    /// Type.
    pub memory_type: MemoryType,
    /// Lifecycle status.
    pub status: MemoryStatus,
    /// Machine-readable lifecycle reason, when one applies.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_reason: Option<String>,
    /// Optimistic memory metadata revision.
    pub revision: Revision,
    /// Atomic body.
    pub content: String,
    /// Importance.
    pub importance: f64,
    /// Confidence.
    pub confidence: f64,
    /// Temporal validity.
    pub valid_from: Option<i64>,
    /// Temporal end.
    pub valid_to: Option<i64>,
    /// Canonical managed path.
    pub canonical_path: Option<VaultPath>,
    /// Canonical file revision.
    pub canonical_revision: Option<Revision>,
    /// Search tags.
    pub tags: Vec<String>,
    /// Entities.
    pub entities: Vec<String>,
    /// Provenance.
    pub sources: Vec<MemorySourceView>,
    /// Relations.
    pub relations: Vec<MemoryRelationView>,
    /// Score when returned from recall.
    pub score: Option<f64>,
    /// Optional score components.
    pub score_breakdown: Option<BTreeMap<String, f64>>,
}

/// Compact provenance output.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MemorySourceView {
    /// Source kind.
    pub source_type: String,
    /// Source path.
    pub path: Option<VaultPath>,
    /// Source file identity.
    pub file_id: Option<FileId>,
    /// Source revision.
    pub revision: Option<Revision>,
    /// Heading anchor.
    pub heading: Vec<String>,
    /// Start line.
    pub start_line: Option<u32>,
    /// End line.
    pub end_line: Option<u32>,
    /// Derived source-health state for note sources.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub health: Option<String>,
    /// Bounded source-health diagnostic reason.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub health_reason: Option<String>,
    /// Last exact evidence-check timestamp.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checked_at: Option<i64>,
}

/// Compact relation output.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MemoryRelationView {
    /// Relation type.
    pub relation_type: String,
    /// Related memory identity.
    pub memory_id: MemoryId,
    /// Relation confidence.
    pub confidence: f64,
}

/// Recall result with degradation and budget state.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RecallResult {
    /// Selected memories.
    pub memories: Vec<MemoryView>,
    /// Selected rebuildable ordinary-note cues.
    pub related_notes: Vec<RelatedNoteView>,
    /// Total eligible memory and note candidates before truncation.
    pub available_result_count: u32,
    /// Eligible durable memories before truncation.
    pub available_memory_count: u32,
    /// Eligible ordinary-note cues before truncation.
    pub available_related_note_count: u32,
    /// Whether the token/result budget truncated output.
    pub truncated: bool,
    /// Stable degradation codes.
    pub degraded: Vec<String>,
    /// Current offline multilingual retrieval coverage.
    pub retrieval_coverage: MemoryRetrievalCoverageView,
}

/// A rebuildable cue that points an Agent back to canonical note source.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RelatedNoteView {
    /// Stable canonical file identity.
    pub file_id: FileId,
    /// Current Vault-relative path.
    pub path: VaultPath,
    /// Canonical revision represented by the cue.
    pub revision: Revision,
    /// Optional note title.
    pub title: Option<String>,
    /// Bounded matching snippet from the derived projection.
    pub snippet: String,
    /// Search tags.
    pub tags: Vec<String>,
    /// Stable knowledge-map topic keys.
    pub topic_ids: Vec<String>,
    /// Ordered note headings for source selection.
    pub headings: Vec<String>,
    /// Fused relevance score.
    pub score: f64,
    /// Optional stable score diagnostics.
    pub score_breakdown: Option<BTreeMap<String, f64>>,
}
