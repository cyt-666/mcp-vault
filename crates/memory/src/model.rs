//! Public memory application DTOs independent of HTTP/RMCP.

use std::collections::BTreeMap;

use mcp_vault_domain::{FileId, MemoryId, MemorySetId, Revision, VaultPath};
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

/// Origin of a durable memory.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryOrigin {
    /// Explicit Agent assertion.
    ExplicitAgent,
    /// Explicit Admin assertion.
    ExplicitAdmin,
    /// Imported memory record.
    Import,
}

/// Current-memory ownership. Ownership controls replacement and deletion; it
/// is not inferred from the presence of a source reference.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryOwnership {
    /// Direct memory with its own canonical Markdown record.
    Explicit,
    /// Item in the one current set owned by a source note.
    NoteDerived,
}

impl MemoryOrigin {
    /// Stable database label.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ExplicitAgent => "explicit_agent",
            Self::ExplicitAdmin => "explicit_admin",
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

/// Vault-scoped current-set extraction policy.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct ExtractionPolicy {
    /// Admit future Markdown changes and explicit backfill runs for extraction.
    pub enabled: bool,
    /// Serialized source-admission mode; legacy marker modes migrate to automatic.
    pub source_mode: ExtractionSourceMode,
    /// Legacy compatibility field retained for prerelease Admin payloads.
    /// Current extraction derives whole-source provenance locally and ignores it.
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
    /// Whether a successful call atomically published an empty current set.
    pub empty_set_published: bool,
    /// True when an unchanged note/profile was skipped before a Provider call.
    pub already_evaluated: bool,
    /// Number of current items atomically published for the source.
    pub items_published: u32,
    /// Whether a prepared exact snapshot was reused after interruption.
    pub reused_prepared_snapshot: bool,
}

/// Per-call behavior for automatic note extraction.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct NoteExtractionOptions {
    /// Re-evaluate a current successful note/profile at explicit operator cost.
    pub include_evaluated: bool,
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
    /// Exact provider/model/input profile that current vectors must match.
    pub profile_hash: Option<String>,
    /// Current memories eligible for vector projection.
    pub eligible: u64,
    /// Current-model vectors matching exact current memory content.
    pub current: u64,
    /// Selected-model vector rows that no longer match a current object/input.
    pub stale: u64,
    /// Stable redacted readiness blockers.
    pub blockers: Vec<String>,
}

/// Imported result of an explicitly authorized real-model retrieval
/// calibration. The setting is Vault-scoped and tied to one exact embedding
/// profile; changing model/input/chunk configuration invalidates it.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct MemorySemanticCalibration {
    /// Exact embedding profile evaluated by the report.
    pub embedding_profile_hash: String,
    /// Minimum raw cosine that admitted a semantic-only candidate.
    pub min_cosine: f64,
    /// Labeled queries with at least one acceptable answer.
    pub answered_queries: u32,
    /// Labeled no-answer queries, including hard lexical negatives.
    pub unanswered_queries: u32,
    /// Observed Recall@5 on the unchanged evaluation split.
    pub recall_at_5: f64,
    /// Observed false-return rate for no-answer queries.
    pub no_answer_false_return_rate: f64,
    /// Hash of the external, separately retained real-model report.
    pub report_hash: String,
    /// Evaluation completion time as Unix milliseconds.
    pub evaluated_at: i64,
}

/// Current semantic-admission configuration and why it is or is not active.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct MemorySemanticCalibrationView {
    /// Effective embedding profile, when a model binding is ready.
    pub effective_profile_hash: Option<String>,
    /// Persisted calibration, even when it became stale after reconfiguration.
    pub calibration: Option<MemorySemanticCalibration>,
    /// Optimistic settings revision.
    pub revision: Option<Revision>,
    /// Whether semantic-only admission is currently enabled.
    pub active: bool,
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

/// Content-free outcome of the explicitly authorized v2.1 legacy migration.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct MemoryV2MigrationResult {
    /// Legacy rows inspected by the immediately preceding preflight.
    pub legacy_total: u64,
    /// Historical lifecycle rows kept outside the current model.
    pub historical: u64,
    /// Reliable active explicit rows selected for preservation.
    pub safe_explicit: u64,
    /// Explicit rows newly materialized with their original IDs.
    pub migrated_explicit: u64,
    /// Explicit rows already present in the v2.1 current model.
    pub already_current: u64,
    /// Active note-derived rows that must be regenerated from current notes.
    pub note_derived: u64,
    /// Ambiguous or unsupported IDs left for operator resolution.
    pub unresolved_ids: Vec<String>,
    /// True only when no active legacy row still needs an ownership decision.
    pub completed: bool,
    /// Legacy rows were retained for backup/audit and never deleted automatically.
    pub legacy_rows_deleted: bool,
}

/// Exact outcomes of reconciling one current source-owned memory set.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct CurrentSourceReconcileReport {
    /// Current source-owned sets checked for this stable File ID.
    pub sources_checked: u64,
    /// Sets already aligned with the current path, revision, and content hash.
    pub current: u64,
    /// Sets whose navigation metadata moved with the same stable File ID and hash.
    pub moved: u64,
    /// Sets hidden immediately because their source content hash changed.
    pub changed: u64,
    /// Sets removed because their source file was deleted.
    pub deleted: u64,
    /// Items hidden by a source content change until the replacement set publishes.
    pub memories_hidden: u64,
    /// Items removed together with a deleted source-owned set.
    pub memories_removed: u64,
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
    pub memory_type: Option<MemoryType>,
    /// Optional importance in [0, 1]. Omission stays omission.
    pub importance: Option<f64>,
    /// Optional confidence in [0, 1]. Omission stays omission.
    pub confidence: Option<f64>,
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
            memory_type: None,
            importance: None,
            confidence: None,
            valid_from: None,
            valid_to: None,
            tags: Vec::new(),
            entities: Vec::new(),
            sources: Vec::new(),
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
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RememberResult {
    /// Stored or an idempotent prior storage result.
    pub outcome: String,
    /// Immediately current explicit memory.
    pub memory: Option<MemoryView>,
}

/// Revision-aware durable memory update.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MemoryUpdateInput {
    /// Replacement body, when supplied.
    pub content: Option<String>,
    /// Replacement type. Outer omission preserves; explicit null clears.
    pub memory_type: Option<Option<MemoryType>>,
    /// Replacement importance.
    pub importance: Option<Option<f64>>,
    /// Replacement confidence.
    pub confidence: Option<Option<f64>>,
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
    pub memory_type: Option<MemoryType>,
    /// Direct or note-derived ownership.
    pub ownership: MemoryOwnership,
    /// Owning source set for note-derived memories.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note_set_id: Option<MemorySetId>,
    /// Optimistic memory metadata revision.
    pub revision: Revision,
    /// Atomic body.
    pub content: String,
    /// Importance.
    pub importance: Option<f64>,
    /// Confidence.
    pub confidence: Option<f64>,
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
    /// Score when returned from recall.
    pub score: Option<f64>,
    /// Optional score components.
    pub score_breakdown: Option<BTreeMap<String, f64>>,
}

/// Privacy-preserving result of deleting the only current copy of a memory.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ForgetResult {
    /// Stable identity that was removed.
    pub id: MemoryId,
    /// Always true on success.
    pub deleted: bool,
    /// Ownership that determined deletion behavior.
    pub ownership: MemoryOwnership,
    /// Derived-item deletion pauses automatic extraction for its source.
    pub source_extraction_paused: bool,
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
}

/// Recall result with degradation and budget state.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RecallResult {
    /// Selected memories.
    pub memories: Vec<MemoryView>,
    /// Selected rebuildable ordinary-note cues.
    pub related_notes: Vec<RelatedNoteView>,
    /// Unique durable-memory candidates considered before relevance gating.
    pub candidate_memory_count: u32,
    /// Durable-memory candidates admitted by the relevance gate.
    pub relevant_memory_count: u32,
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
    /// Versioned hash of lexical, chunking, and semantic-admission policy.
    pub retrieval_profile_hash: String,
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
