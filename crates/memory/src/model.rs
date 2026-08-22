//! Public memory application DTOs independent of HTTP/RMCP.

use std::collections::BTreeMap;

use mcp_vault_domain::{FileId, MemoryId, Revision, VaultPath};
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
    /// Derived proposal awaiting review.
    Candidate,
    /// Eligible for normal recall and canonical Markdown.
    Active,
    /// Replaced by a newer memory.
    Superseded,
    /// No current source supports the proposition.
    Stale,
    /// Intentionally inactive but retained.
    Archived,
    /// Rejected by review/policy.
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
    /// Validated provider candidate.
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
    /// Maximum memories.
    pub max_results: u32,
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
            include_sources: true,
            include_score_breakdown: false,
            max_results: 12,
            max_tokens: 1800,
        }
    }
}

/// Result of explicit remember.
#[derive(Clone, Debug, PartialEq)]
pub struct RememberResult {
    /// created, reinforced_existing, merged_into_existing, or conflict_requires_review.
    pub outcome: String,
    /// Resulting memory bundle.
    pub memory: MemoryView,
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
    /// Number of eligible candidates before result truncation.
    pub available_result_count: u32,
    /// Whether the token/result budget truncated output.
    pub truncated: bool,
    /// Stable degradation codes.
    pub degraded: Vec<String>,
}

/// Candidate extraction proposal accepted by deterministic validation.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ExtractedCandidate {
    /// Memory type.
    pub memory_type: MemoryType,
    /// Atomic proposition.
    pub content: String,
    /// Importance.
    pub importance: f64,
    /// Confidence.
    pub confidence: f64,
    /// Optional validity start.
    pub valid_from: Option<i64>,
    /// Entity values.
    pub entities: Vec<String>,
    /// Tag values.
    pub tags: Vec<String>,
    /// Source anchor heading.
    pub heading_path: Vec<String>,
    /// Source line range.
    pub start_line: Option<u32>,
    /// Source line range end.
    pub end_line: Option<u32>,
}
