//! No-Provider manifest, run-config, and artifact boundary for M6.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path},
    sync::atomic::{AtomicU32, Ordering},
};

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

use mcp_vault_domain::VaultPath;
use mcp_vault_memory::{MemoryPack, semantic::a80_batch_count};
pub use mcp_vault_providers::{
    ProviderRuntimeSnapshot, StrictFunctionCallIssue, StructuredJsonDiagnostic,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

mod adapters;
mod diagnostic;
mod live_runtime;
mod private_fs;
mod provider_capability_probe;
mod templates;
pub use adapters::{
    ArmSemanticRuntime, LiveTransportRequestBudget, ProviderServiceAppBoundary,
    ProviderStageTemplate, SemanticMemoryServiceAppBoundary, VaultCoreSourceVerifier,
    validate_external_model_identity,
};
pub use diagnostic::{
    DiagnosticFailure, DiagnosticSelection, LiveSemanticDiagnosticResult,
    run_live_semantic_diagnostic, validate_live_semantic_diagnostic_config,
};
pub use live_runtime::{
    LiveCliConfig, LiveRuntime, build_live_runtime, claim_live_preparation,
    write_live_preparation_seal,
};
pub use provider_capability_probe::*;
pub use templates::{
    M6_A80_PROMPT_ID, M6_A80_SCHEMA_ID, M6_PROMPT_ID, M6_SCHEMA_ID,
    semantic_a80_provider_templates, semantic_live_provider_templates,
};
use thiserror::Error;

const MANIFEST_SCHEMA: &str = "semantic-memory-eval-manifest-v1";
const RUN_SCHEMA: &str = "semantic-memory-eval-run-config-v1";
const ARTIFACT_SCHEMA: &str = "semantic-memory-eval-artifact-v1";

#[derive(Debug, Error, PartialEq, Eq)]
pub enum EvalError {
    #[error("manifest invalid: {0}")]
    Manifest(&'static str),
    #[error("run config invalid: {0}")]
    RunConfig(&'static str),
    #[error("live execution blocked before external request: {0}")]
    LiveBlocked(&'static str),
    #[error("artifact path is unsafe")]
    UnsafeArtifactPath,
    #[error("artifact write failed")]
    ArtifactIo,
    #[error("shadow configuration is invalid: {0}")]
    ShadowConfig(&'static str),
    #[error("shadow source copy failed")]
    ShadowCopy,
    #[error("authoritative source snapshot mismatch")]
    SourceMismatch,
    #[error("live evaluation configuration is invalid: {0}")]
    LiveConfig(&'static str),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EvalMode {
    Mock,
    Replay,
    Live,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskSplit {
    Development,
    Validation,
    Holdout,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ComparisonArm {
    A,
    B,
    C,
}

fn comparison_arm_wire_name(arm: &ComparisonArm) -> &'static str {
    match arm {
        ComparisonArm::A => "a",
        ComparisonArm::B => "b",
        ComparisonArm::C => "c",
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EvalSource {
    pub logical_id: String,
    pub synthetic_placeholder: bool,
    pub vault_id: String,
    pub file_id: String,
    pub path: String,
    pub file_revision: u64,
    pub content_hash: String,
    /// Frozen count of non-empty source lines used as logical blocks.
    /// A80 live preflight uses this to enforce the shared Provider budget.
    #[serde(default)]
    pub logical_block_count: u32,
    pub source_revision_id: String,
    pub authorization_revision: i64,
    pub profile_id: String,
    pub rules_revision: i64,
    pub source_generation: i64,
    pub extraction_commit_sequence: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EvalTask {
    pub id: String,
    pub split: TaskSplit,
    pub query_refs: Vec<String>,
    pub query: String,
    pub source_ids: Vec<String>,
    #[serde(default)]
    pub source_fence: Vec<TaskSourceFence>,
    pub must_preserve: Vec<String>,
    pub must_not_infer: Vec<String>,
    pub expected_usable_information: Vec<String>,
    #[serde(default)]
    pub expected_no_answer: bool,
    pub expected_status: String,
    pub expected_source_relations: Vec<ExpectedRelation>,
    pub severity: String,
}

/// The immutable source witness attached to a frozen evaluation task.
///
/// A task is not allowed to silently follow a newer source revision: the
/// evaluator can compare these fields with the manifest source fence before
/// submitting any model result.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TaskSourceFence {
    pub source_id: String,
    pub file_revision: u64,
    pub source_revision_id: String,
    pub content_hash: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedRelation {
    pub kind: String,
    pub source_ids: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EvaluationManifest {
    pub schema_version: String,
    pub dataset_id: String,
    pub synthetic_only: bool,
    pub sources: Vec<EvalSource>,
    pub tasks: Vec<EvalTask>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthoritativeSourceSnapshot {
    pub logical_id: String,
    pub vault_id: String,
    pub file_id: String,
    pub path: String,
    pub file_revision: u64,
    pub source_revision_id: String,
    pub content_hash: String,
    pub content: Vec<u8>,
    pub authorized: bool,
    pub authorization_revision: i64,
    pub profile_id: String,
    pub rules_revision: i64,
    pub source_generation: i64,
    pub extraction_commit_sequence: i64,
}

#[async_trait::async_trait]
pub trait SourceSnapshotVerifier: Send + Sync {
    fn snapshot(&self, source: &EvalSource) -> Result<AuthoritativeSourceSnapshot, EvalError>;

    /// Re-read the authoritative source at a critical boundary. Implementors
    /// backed by Vault Core/State must override this instead of reusing an
    /// initial run snapshot. The default keeps deterministic local fakes
    /// source-compatible while still providing an explicit boundary.
    async fn snapshot_current(
        &self,
        source: &EvalSource,
    ) -> Result<AuthoritativeSourceSnapshot, EvalError> {
        self.snapshot(source)
    }
}

pub fn verify_source_snapshots<V: SourceSnapshotVerifier>(
    manifest: &EvaluationManifest,
    verifier: &V,
) -> Result<(), EvalError> {
    validate_manifest(manifest)?;
    for source in &manifest.sources {
        let actual = verifier.snapshot(source)?;
        if !actual.authorized
            || actual.logical_id != source.logical_id
            || actual.vault_id != source.vault_id
            || actual.file_id != source.file_id
            || actual.path != source.path
            || actual.file_revision != source.file_revision
            || actual.source_revision_id != source.source_revision_id
            || actual.content_hash != source.content_hash
            || format!("{:x}", Sha256::digest(&actual.content)) != source.content_hash
            || actual.authorization_revision != source.authorization_revision
            || actual.profile_id != source.profile_id
            || actual.rules_revision != source.rules_revision
            || actual.source_generation != source.source_generation
            || actual.extraction_commit_sequence != source.extraction_commit_sequence
        {
            return Err(EvalError::SourceMismatch);
        }
    }
    Ok(())
}

#[derive(Clone, Debug, Deserialize)]
struct FixtureManifest {
    schema_version: String,
    fixture_id: String,
    dataset_class: String,
    contains_real_user_data: bool,
    sources: Vec<FixtureSource>,
    tasks: Vec<FixtureTask>,
    #[serde(flatten)]
    _extra: std::collections::BTreeMap<String, serde_json::Value>,
}

#[derive(Clone, Debug, Deserialize)]
struct FixtureSource {
    id: String,
    path: String,
    sha256: String,
    vault_id: String,
    #[serde(default)]
    file_id: Option<String>,
    #[serde(default)]
    file_revision: Option<u64>,
    #[serde(default)]
    source_revision_id: Option<String>,
    #[serde(default)]
    authorization_revision: Option<i64>,
    #[serde(default)]
    profile_id: Option<String>,
    #[serde(default)]
    rules_revision: Option<i64>,
    #[serde(default)]
    source_generation: Option<i64>,
    #[serde(default)]
    extraction_commit_sequence: Option<i64>,
    #[serde(flatten)]
    _extra: std::collections::BTreeMap<String, serde_json::Value>,
}

#[derive(Clone, Debug, Deserialize)]
struct FixtureTask {
    id: String,
    #[serde(default)]
    split: Option<TaskSplit>,
    q_refs: Vec<String>,
    question: String,
    source_ids: Vec<String>,
    #[serde(default)]
    source_fence: Vec<TaskSourceFence>,
    expected_usable_information: Vec<String>,
    must_preserve_conditions: Vec<String>,
    must_not_infer: Vec<String>,
    #[serde(default)]
    expected_no_answer: Option<bool>,
    expected_status: String,
    expected_source_relations: Vec<ExpectedRelation>,
    severity: String,
    #[serde(flatten)]
    _extra: std::collections::BTreeMap<String, serde_json::Value>,
}

/// Convert the existing M0 `semantic-memory-manifest-v1` fixture without
/// discarding its gold expectations. Missing immutable file metadata is
/// represented as an explicit fixture projection, never guessed as live data.
pub fn convert_m0_fixture_manifest(json: &str) -> Result<EvaluationManifest, EvalError> {
    let fixture: FixtureManifest =
        serde_json::from_str(json).map_err(|_| EvalError::Manifest("fixture schema is invalid"))?;
    if fixture.schema_version != "semantic-memory-manifest-v1"
        || fixture.dataset_class != "synthetic"
        || fixture.contains_real_user_data
    {
        return Err(EvalError::Manifest("M0 fixture is not synthetic v1"));
    }
    let sources = fixture
        .sources
        .into_iter()
        .map(|source| EvalSource {
            logical_id: source.id.clone(),
            synthetic_placeholder: true,
            vault_id: source.vault_id,
            file_id: source
                .file_id
                .unwrap_or_else(|| format!("fixture-file-{}", source.id)),
            path: source.path,
            file_revision: source.file_revision.unwrap_or(1),
            content_hash: source.sha256,
            logical_block_count: 0,
            source_revision_id: source
                .source_revision_id
                .unwrap_or_else(|| format!("fixture-source-revision-{}", source.id)),
            authorization_revision: source.authorization_revision.unwrap_or(1),
            profile_id: source
                .profile_id
                .unwrap_or_else(|| "fixture-profile-v1".into()),
            rules_revision: source.rules_revision.unwrap_or(1),
            source_generation: source.source_generation.unwrap_or(1),
            extraction_commit_sequence: source.extraction_commit_sequence.unwrap_or(1),
        })
        .collect::<Vec<_>>();
    let source_ids = sources
        .iter()
        .map(|source| source.logical_id.as_str())
        .collect::<BTreeSet<_>>();
    let tasks = fixture
        .tasks
        .into_iter()
        .map(|task| {
            if task
                .source_ids
                .iter()
                .any(|id| !source_ids.contains(id.as_str()))
            {
                return Err(EvalError::Manifest("task references an unknown source"));
            }
            Ok(EvalTask {
                id: task.id,
                split: task.split.unwrap_or(TaskSplit::Development),
                query_refs: task.q_refs,
                query: task.question,
                source_ids: task.source_ids,
                source_fence: task.source_fence,
                must_preserve: task.must_preserve_conditions,
                must_not_infer: task.must_not_infer,
                expected_usable_information: task.expected_usable_information,
                expected_no_answer: task.expected_no_answer.unwrap_or({
                    matches!(
                        task.expected_status.as_str(),
                        "unanswered" | "access_denied"
                    )
                }),
                expected_status: task.expected_status,
                expected_source_relations: task.expected_source_relations,
                severity: task.severity,
            })
        })
        .collect::<Result<Vec<_>, EvalError>>()?;
    let manifest = EvaluationManifest {
        schema_version: MANIFEST_SCHEMA.into(),
        dataset_id: fixture.fixture_id,
        synthetic_only: true,
        sources,
        tasks,
    };
    validate_manifest(&manifest)?;
    Ok(manifest)
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Budget {
    pub max_entries: u32,
    pub max_bytes: u32,
    pub max_tokens: u32,
    pub external_request_budget: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ComparisonConfig {
    pub arm: ComparisonArm,
    pub source_ids: Vec<String>,
    pub task_ids: Vec<String>,
    pub budget: Budget,
    pub index_profile_id: String,
    pub prompt_id: String,
    pub schema_id: String,
    pub answer_model_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ArmRecord {
    pub arm: ComparisonArm,
    pub manifest_hash: String,
    pub source_ids: Vec<String>,
    pub task_ids: Vec<String>,
    pub budget: Budget,
    pub index_profile_id: String,
    pub prompt_id: String,
    pub schema_id: String,
    pub answer_model_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EvaluationRunConfig {
    pub schema_version: String,
    pub mode: EvalMode,
    pub allow_live: bool,
    pub explicit_live_authorization: bool,
    pub source_allowlist: Vec<String>,
    pub cost_budget_minor: u64,
    pub artifact_root: Option<String>,
    pub comparisons: Vec<ComparisonConfig>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GateStatus {
    Pass,
    Fail,
    Pending,
    NotRun,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UsageSummary {
    pub request_count: u32,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cost: Option<String>,
    pub status: GateStatus,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EvaluationArtifact {
    pub schema_version: String,
    pub manifest_hash: String,
    pub run_config_hash: String,
    pub mode: EvalMode,
    pub live_ready: bool,
    pub engineering_status: GateStatus,
    pub semantic_status: GateStatus,
    pub task_status: GateStatus,
    pub cost_status: GateStatus,
    pub comparison_consistent: bool,
    pub manual_review_pending: bool,
    pub usage: UsageSummary,
    pub errors: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ShadowBuildConfig {
    pub manifest: EvaluationManifest,
    pub source_root: String,
    pub shadow_root: String,
    pub shadow_state: String,
    pub shadow_history: String,
    pub artifact_root: String,
    pub source_allowlist: Vec<String>,
    pub outbox_cursor: u64,
    pub rules_revision: i64,
    pub shadow_vault_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ShadowCatchupState {
    pub outbox_cursor: u64,
    pub rules_revision: i64,
    pub manifest_hash: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ShadowBuildPlan {
    pub manifest_hash: String,
    pub shadow_vault_id: String,
    pub copied_logical_ids: Vec<String>,
    pub copied_file_count: u32,
    pub source_unchanged: bool,
    pub independent_state_paths: bool,
    pub needs_replay: bool,
    pub needs_rebuild: bool,
    pub drift_codes: Vec<String>,
    pub provider_started: bool,
    pub source_publish_attempted: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReplayEvent {
    ProviderError { code: String },
    ProviderSuccess,
}

pub fn validate_manifest(manifest: &EvaluationManifest) -> Result<String, EvalError> {
    if manifest.schema_version != MANIFEST_SCHEMA || !manifest.synthetic_only {
        return Err(EvalError::Manifest("only synthetic manifest v1 is allowed"));
    }
    if manifest.dataset_id.trim().is_empty()
        || manifest.sources.is_empty()
        || manifest.tasks.is_empty()
    {
        return Err(EvalError::Manifest(
            "dataset, sources, and tasks are required",
        ));
    }
    let mut revisions = BTreeSet::new();
    let mut files = BTreeSet::new();
    let mut logical_ids = BTreeSet::new();
    for source in &manifest.sources {
        if !logical_ids.insert(&source.logical_id)
            || !revisions.insert(&source.source_revision_id)
            || !files.insert(&source.file_id)
        {
            return Err(EvalError::Manifest("source identity is duplicated"));
        }
        VaultPath::parse(&source.path).map_err(|_| EvalError::Manifest("source path is unsafe"))?;
        if source.vault_id.trim().is_empty() || !is_hash(&source.content_hash) {
            return Err(EvalError::Manifest("source identity or hash is invalid"));
        }
        reject_secret_fields(source)?;
    }
    let logical_source_ids = manifest
        .sources
        .iter()
        .map(|source| source.logical_id.as_str())
        .collect::<BTreeSet<_>>();
    if manifest.tasks.iter().any(|task| {
        task.source_ids
            .iter()
            .any(|id| !logical_source_ids.contains(id.as_str()))
    }) {
        return Err(EvalError::Manifest("task references an unknown source"));
    }
    let task_ids = manifest
        .tasks
        .iter()
        .map(|task| task.id.as_str())
        .collect::<BTreeSet<_>>();
    if task_ids.len() != manifest.tasks.len()
        || manifest
            .tasks
            .iter()
            .any(|task| task.query.trim().is_empty())
    {
        return Err(EvalError::Manifest("task identity or query is invalid"));
    }
    validate_task_source_fences(manifest)?;
    canonical_hash(manifest).map_err(|_| EvalError::Manifest("manifest serialization failed"))
}

fn validate_task_source_fences(manifest: &EvaluationManifest) -> Result<(), EvalError> {
    let sources = manifest
        .sources
        .iter()
        .map(|source| (source.logical_id.as_str(), source))
        .collect::<std::collections::BTreeMap<_, _>>();
    for task in &manifest.tasks {
        if task.source_fence.is_empty() {
            continue;
        }
        let ids = task
            .source_fence
            .iter()
            .map(|fence| fence.source_id.as_str())
            .collect::<BTreeSet<_>>();
        let expected_ids = task
            .source_ids
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        if ids != expected_ids || ids.len() != task.source_fence.len() {
            return Err(EvalError::Manifest(
                "task source fence does not match sources",
            ));
        }
        for fence in &task.source_fence {
            let Some(source) = sources.get(fence.source_id.as_str()) else {
                return Err(EvalError::Manifest(
                    "task source fence references unknown source",
                ));
            };
            if fence.file_revision != source.file_revision
                || fence.source_revision_id != source.source_revision_id
                || fence.content_hash != source.content_hash
            {
                return Err(EvalError::Manifest("task source fence is stale"));
            }
        }
    }
    Ok(())
}

/// Validate the frozen M6 synthetic corpus using the same manifest converter
/// and generic manifest verifier as the smaller M0 fixture.
///
/// The additional checks are deliberately finite and corpus-specific: they
/// enforce the minimum evaluation size, complete task fences, and a strict
/// source-disjoint development/holdout split. No new query or gold DSL is
/// introduced.
pub fn convert_m6_synthetic_fixture_manifest(json: &str) -> Result<EvaluationManifest, EvalError> {
    let manifest = convert_m0_fixture_manifest(json)?;
    validate_m6_synthetic_manifest(&manifest)?;
    Ok(manifest)
}

/// Apply the M6 corpus-specific size, fence, and split checks to an already
/// converted manifest. This is the same validator used by the JSON converter
/// and by regression tests that mutate a frozen manifest.
pub fn validate_m6_synthetic_manifest(manifest: &EvaluationManifest) -> Result<(), EvalError> {
    if manifest.dataset_id != "semantic-memory-synthetic-m6-v1"
        || !manifest.synthetic_only
        || manifest.sources.len() < 30
        || manifest.tasks.len() < 60
    {
        return Err(EvalError::Manifest(
            "M6 synthetic fixture requires at least 30 sources and 60 tasks",
        ));
    }
    let mut query_ref_ids = BTreeSet::new();
    for task in &manifest.tasks {
        for query_ref in &task.query_refs {
            if !query_ref_ids.insert(query_ref.as_str()) {
                return Err(EvalError::Manifest("M6 query references are duplicated"));
            }
        }
    }
    if manifest
        .sources
        .iter()
        .any(|source| !source.synthetic_placeholder)
    {
        return Err(EvalError::Manifest(
            "M6 fixture contains non-synthetic source",
        ));
    }
    if manifest
        .tasks
        .iter()
        .any(|task| task.source_fence.is_empty())
    {
        return Err(EvalError::Manifest("M6 task source fence is required"));
    }
    let mut source_splits = std::collections::BTreeMap::<String, TaskSplit>::new();
    for task in &manifest.tasks {
        let mut relation_records = BTreeSet::new();
        if task.query_refs.is_empty()
            || task.query_refs.iter().collect::<BTreeSet<_>>().len() != task.query_refs.len()
            || task.source_ids.is_empty()
            || task.source_ids.iter().collect::<BTreeSet<_>>().len() != task.source_ids.len()
            || task.expected_source_relations.iter().any(|relation| {
                let mut ids = relation.source_ids.clone();
                ids.sort();
                relation.kind.trim().is_empty()
                    || relation.source_ids.is_empty()
                    || relation.source_ids.iter().collect::<BTreeSet<_>>().len()
                        != relation.source_ids.len()
                    || !relation_records.insert(format!("{}:{:?}", relation.kind, ids))
                    || relation.source_ids.iter().any(|source_id| {
                        task.source_ids
                            .iter()
                            .all(|task_source_id| task_source_id != source_id)
                    })
            })
        {
            return Err(EvalError::Manifest(
                "M6 task references are incomplete or outside its source set",
            ));
        }
        for source_id in &task.source_ids {
            if let Some(previous) = source_splits.insert(source_id.clone(), task.split.clone())
                && previous != task.split
            {
                return Err(EvalError::Manifest(
                    "development and holdout tasks share a source",
                ));
            }
        }
        if task.expected_source_relations.iter().any(|relation| {
            relation.source_ids.iter().any(|source_id| {
                source_splits
                    .get(source_id)
                    .is_some_and(|split| split != &task.split)
            })
        }) {
            return Err(EvalError::Manifest(
                "task relation crosses development and holdout source splits",
            ));
        }
    }
    let development_count = manifest
        .tasks
        .iter()
        .filter(|task| task.split == TaskSplit::Development)
        .count();
    let holdout_count = manifest
        .tasks
        .iter()
        .filter(|task| task.split == TaskSplit::Holdout)
        .count();
    let no_answer_count = manifest
        .tasks
        .iter()
        .filter(|task| task.expected_no_answer)
        .count();
    let relation_count = manifest
        .tasks
        .iter()
        .filter(|task| !task.expected_source_relations.is_empty())
        .count();
    if development_count < 36 || holdout_count < 24 || no_answer_count < 10 || relation_count < 60 {
        return Err(EvalError::Manifest(
            "M6 fixture lacks the required explicit development/holdout/no-answer/relation counts",
        ));
    }
    Ok(())
}

/// Verify the checked-in source bytes against the same immutable fence used by
/// the manifest and task records. This is intentionally separate from live
/// `SourceSnapshotVerifier`, which verifies an isolated Vault instead.
pub fn verify_synthetic_fixture_files(
    manifest: &EvaluationManifest,
    fixture_root: &Path,
) -> Result<(), EvalError> {
    for source in &manifest.sources {
        let relative = VaultPath::parse(&source.path)
            .map_err(|_| EvalError::Manifest("source path is unsafe"))?;
        let path = fixture_root.join(relative.as_str());
        if !fixture_root.is_absolute()
            || !path.starts_with(fixture_root)
            || path
                .components()
                .any(|component| matches!(component, Component::Normal(value) if value == "data" || value == "vault" || value == "production"))
        {
            return Err(EvalError::Manifest("fixture source path is unsafe"));
        }
        let mut current = std::path::PathBuf::new();
        for component in path.components() {
            current.push(component.as_os_str());
            if let Ok(metadata) = fs::symlink_metadata(&current)
                && metadata.file_type().is_symlink()
            {
                return Err(EvalError::Manifest("fixture source symlink is unsafe"));
            }
        }
        let metadata = fs::symlink_metadata(&path)
            .map_err(|_| EvalError::Manifest("fixture source is missing"))?;
        if !metadata.is_file() {
            return Err(EvalError::Manifest("fixture source is not a regular file"));
        }
        let bytes = fs::read(path).map_err(|_| EvalError::Manifest("fixture source is missing"))?;
        if format!("{:x}", Sha256::digest(bytes)) != source.content_hash {
            return Err(EvalError::Manifest("fixture source hash mismatch"));
        }
    }
    validate_manifest(manifest)?;
    Ok(())
}

pub fn prepare_shadow_dry_run(
    config: &ShadowBuildConfig,
    current: &ShadowCatchupState,
) -> Result<ShadowBuildPlan, EvalError> {
    let manifest_hash = validate_manifest(&config.manifest)
        .map_err(|_| EvalError::ShadowConfig("manifest is not valid"))?;
    let source_root = Path::new(&config.source_root);
    let shadow_root = Path::new(&config.shadow_root);
    let state_path = Path::new(&config.shadow_state);
    let history_path = Path::new(&config.shadow_history);
    let artifact_root = Path::new(&config.artifact_root);
    validate_shadow_root(source_root)?;
    if !fs::symlink_metadata(source_root)
        .map_err(|_| EvalError::ShadowConfig("source root is unavailable"))?
        .is_dir()
    {
        return Err(EvalError::ShadowConfig("source root must be a directory"));
    }
    let source_real = canonical_for_overlap(source_root)?;
    for path in [shadow_root, state_path, history_path, artifact_root] {
        validate_shadow_root(path)?;
        if paths_overlap(&source_real, &canonical_for_overlap(path)?) {
            return Err(EvalError::ShadowConfig(
                "shadow output overlaps source root",
            ));
        }
    }
    if [shadow_root, state_path, history_path, artifact_root]
        .iter()
        .enumerate()
        .any(|(index, path)| {
            [shadow_root, state_path, history_path, artifact_root]
                .iter()
                .skip(index + 1)
                .any(|other| {
                    canonical_for_overlap(path).is_ok_and(|left| {
                        canonical_for_overlap(other).is_ok_and(|right| paths_overlap(&left, &right))
                    })
                })
        })
    {
        return Err(EvalError::ShadowConfig(
            "shadow state/history/artifact roots overlap",
        ));
    }
    if config.shadow_vault_id.trim().is_empty() || config.shadow_vault_id == "production" {
        return Err(EvalError::ShadowConfig("shadow Vault identity is invalid"));
    }
    let expected_allowlist = config
        .manifest
        .sources
        .iter()
        .flat_map(|source| [source.logical_id.clone(), source.file_id.clone()])
        .collect::<BTreeSet<_>>();
    if config
        .source_allowlist
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>()
        != expected_allowlist
    {
        return Err(EvalError::ShadowConfig(
            "source allowlist does not exactly cover manifest",
        ));
    }
    if current.manifest_hash != manifest_hash {
        return Err(EvalError::ShadowConfig("source manifest drifted"));
    }
    fs::create_dir_all(shadow_root).map_err(|_| EvalError::ShadowCopy)?;
    let mut copied = Vec::new();
    for source in &config.manifest.sources {
        let relative = VaultPath::parse(&source.path)
            .map_err(|_| EvalError::ShadowConfig("source path is unsafe"))?;
        let source_path = source_root.join(relative.as_str());
        let destination = shadow_root.join(relative.as_str());
        if !is_within(source_root, &source_path) || !is_within(shadow_root, &destination) {
            return Err(EvalError::ShadowConfig("source path escapes root"));
        }
        validate_existing_components(&source_path)?;
        let source_metadata =
            fs::symlink_metadata(&source_path).map_err(|_| EvalError::ShadowCopy)?;
        if source_metadata.file_type().is_symlink() || !source_metadata.is_file() {
            return Err(EvalError::ShadowConfig("source must be a regular file"));
        }
        let bytes = fs::read(&source_path).map_err(|_| EvalError::ShadowCopy)?;
        if format!("{:x}", Sha256::digest(&bytes)) != source.content_hash {
            return Err(EvalError::ShadowConfig("source content hash mismatch"));
        }
        let parent = destination.parent().ok_or(EvalError::ShadowCopy)?;
        validate_existing_components(parent)?;
        fs::create_dir_all(parent).map_err(|_| EvalError::ShadowCopy)?;
        validate_existing_components(parent)?;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&destination)
            .map_err(|_| EvalError::ShadowCopy)?;
        use std::io::Write;
        file.write_all(&bytes).map_err(|_| EvalError::ShadowCopy)?;
        copied.push(source.logical_id.clone());
        validate_existing_components(&source_path)?;
        if fs::read(&source_path).map_err(|_| EvalError::ShadowCopy)? != bytes {
            return Err(EvalError::ShadowCopy);
        }
    }
    let mut drift_codes = Vec::new();
    let needs_replay = current.outbox_cursor != config.outbox_cursor;
    let needs_rebuild = current.rules_revision != config.rules_revision;
    if needs_replay {
        drift_codes.push("outbox_cursor_drift".to_owned());
    }
    if needs_rebuild {
        drift_codes.push("rules_revision_drift".to_owned());
    }
    Ok(ShadowBuildPlan {
        manifest_hash,
        shadow_vault_id: config.shadow_vault_id.clone(),
        copied_file_count: copied.len() as u32,
        copied_logical_ids: copied,
        source_unchanged: true,
        independent_state_paths: true,
        needs_replay,
        needs_rebuild,
        drift_codes,
        provider_started: false,
        source_publish_attempted: false,
    })
}

fn validate_shadow_root(path: &Path) -> Result<(), EvalError> {
    if !path.is_absolute() || path.components().any(|component| matches!(component, Component::Normal(value) if value == "vault" || value == "production")) { return Err(EvalError::ShadowConfig("root must be absolute and non-production")); }
    validate_existing_components(path)?;
    Ok(())
}

fn validate_existing_components(path: &Path) -> Result<(), EvalError> {
    let mut current = std::path::PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(metadata)
                if metadata.file_type().is_symlink() && !trusted_system_alias(&current) =>
            {
                return Err(EvalError::ShadowConfig(
                    "symlink path component is forbidden",
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(_) => {
                return Err(EvalError::ShadowConfig(
                    "shadow path component is unavailable",
                ));
            }
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn trusted_system_alias(path: &Path) -> bool {
    path == Path::new("/var") || path == Path::new("/tmp")
}

#[cfg(not(target_os = "macos"))]
fn trusted_system_alias(_path: &Path) -> bool {
    false
}

fn canonical_for_overlap(path: &Path) -> Result<std::path::PathBuf, EvalError> {
    let mut missing = Vec::new();
    let mut existing = path;
    while !existing.exists() {
        let name = existing
            .file_name()
            .ok_or(EvalError::ShadowConfig("root has no existing ancestor"))?;
        missing.push(name.to_owned());
        existing = existing
            .parent()
            .ok_or(EvalError::ShadowConfig("root has no existing ancestor"))?;
    }
    let mut result = fs::canonicalize(existing)
        .map_err(|_| EvalError::ShadowConfig("root cannot be canonicalized"))?;
    for component in missing.iter().rev() {
        result.push(component);
    }
    Ok(result)
}

fn paths_overlap(left: &Path, right: &Path) -> bool {
    left == right || left.starts_with(right) || right.starts_with(left)
}

fn is_within(root: &Path, path: &Path) -> bool {
    path.starts_with(root)
}

pub fn validate_run_config(
    manifest: &EvaluationManifest,
    config: &EvaluationRunConfig,
) -> Result<String, EvalError> {
    validate_manifest(manifest)?;
    if config.schema_version != RUN_SCHEMA || config.comparisons.len() != 3 {
        return Err(EvalError::RunConfig("run config must contain A/B/C arms"));
    }
    if matches!(config.mode, EvalMode::Live) {
        if !config.allow_live {
            return Err(EvalError::LiveBlocked("allow_live is false"));
        }
        if !config.explicit_live_authorization {
            return Err(EvalError::LiveBlocked("explicit authorization is missing"));
        }
        let expected_allowlist = manifest
            .sources
            .iter()
            .flat_map(|source| [source.logical_id.clone(), source.file_id.clone()])
            .collect::<BTreeSet<_>>();
        let actual_allowlist = config
            .source_allowlist
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        if actual_allowlist != expected_allowlist {
            return Err(EvalError::LiveBlocked(
                "source allowlist does not exactly cover manifest",
            ));
        }
        if manifest.synthetic_only
            || manifest
                .sources
                .iter()
                .any(|source| source.synthetic_placeholder)
        {
            return Err(EvalError::LiveBlocked(
                "synthetic manifest is not live-eligible",
            ));
        }
        if config.cost_budget_minor == 0 {
            return Err(EvalError::LiveBlocked("cost budget is zero"));
        }
        if config
            .artifact_root
            .as_deref()
            .is_none_or(|path| path.is_empty())
        {
            return Err(EvalError::LiveBlocked("dedicated artifact root is missing"));
        }
    }
    let expected = [ComparisonArm::A, ComparisonArm::B, ComparisonArm::C];
    let manifest_sources = manifest
        .sources
        .iter()
        .map(|source| source.logical_id.as_str())
        .collect::<BTreeSet<_>>();
    let manifest_tasks = manifest
        .tasks
        .iter()
        .map(|task| task.id.as_str())
        .collect::<BTreeSet<_>>();
    for (index, arm) in config.comparisons.iter().enumerate() {
        if arm.arm != expected[index] {
            return Err(EvalError::RunConfig("comparison arm order is invalid"));
        }
        if arm.source_ids.is_empty()
            || arm.task_ids.is_empty()
            || arm.answer_model_id.trim().is_empty()
            || arm.source_ids.iter().collect::<BTreeSet<_>>().len() != arm.source_ids.len()
            || arm.task_ids.iter().collect::<BTreeSet<_>>().len() != arm.task_ids.len()
        {
            return Err(EvalError::RunConfig("comparison identifiers are required"));
        }
        if arm
            .source_ids
            .iter()
            .any(|id| !manifest_sources.contains(id.as_str()))
            || arm
                .task_ids
                .iter()
                .any(|id| !manifest_tasks.contains(id.as_str()))
        {
            return Err(EvalError::RunConfig(
                "comparison references are outside manifest",
            ));
        }
        reject_secret_fields(arm)?;
    }
    let first = &config.comparisons[0];
    if config.comparisons.iter().skip(1).any(|arm| {
        arm.budget != first.budget
            || arm.index_profile_id != first.index_profile_id
            || arm.prompt_id != first.prompt_id
            || arm.schema_id != first.schema_id
            || arm.answer_model_id != first.answer_model_id
    }) {
        return Err(EvalError::RunConfig(
            "A/B/C inputs or budget are inconsistent",
        ));
    }
    if matches!(config.mode, EvalMode::Live) && first.budget.external_request_budget == 0 {
        return Err(EvalError::LiveBlocked("external request budget is zero"));
    }
    canonical_hash(config).map_err(|_| EvalError::RunConfig("run config serialization failed"))
}

pub fn validate_arm_records(
    manifest: &EvaluationManifest,
    config: &EvaluationRunConfig,
    records: &[ArmRecord],
) -> Result<bool, EvalError> {
    let manifest_hash = validate_manifest(manifest)?;
    validate_run_config(manifest, config)?;
    if records.len() != 3 {
        return Ok(false);
    }
    let expected = [ComparisonArm::A, ComparisonArm::B, ComparisonArm::C];
    Ok(records.iter().enumerate().all(|(index, record)| {
        let expected_config = &config.comparisons[index];
        record.arm == expected[index]
            && record.manifest_hash == manifest_hash
            && record.source_ids == expected_config.source_ids
            && record.task_ids == expected_config.task_ids
            && record.budget == expected_config.budget
            && record.index_profile_id == expected_config.index_profile_id
            && record.prompt_id == expected_config.prompt_id
            && record.schema_id == expected_config.schema_id
            && record.answer_model_id == expected_config.answer_model_id
    }))
}

pub fn run_mock_or_replay(
    manifest: &EvaluationManifest,
    config: &EvaluationRunConfig,
    events: &[ReplayEvent],
) -> Result<EvaluationArtifact, EvalError> {
    let manifest_hash = validate_manifest(manifest)?;
    let run_config_hash = validate_run_config(manifest, config)?;
    if matches!(config.mode, EvalMode::Live) {
        return Err(EvalError::LiveBlocked(
            "live execution is not provided by this crate",
        ));
    }
    let mut errors = Vec::new();
    for event in events {
        if let ReplayEvent::ProviderError { code } = event {
            errors.push(format!("provider_error:{}", safe_code(code)));
            break;
        }
    }
    Ok(EvaluationArtifact {
        schema_version: ARTIFACT_SCHEMA.to_owned(),
        manifest_hash,
        run_config_hash,
        mode: config.mode.clone(),
        live_ready: false,
        engineering_status: if errors.is_empty() {
            GateStatus::NotRun
        } else {
            GateStatus::Fail
        },
        semantic_status: GateStatus::Pending,
        task_status: GateStatus::Pending,
        cost_status: GateStatus::Pending,
        comparison_consistent: false,
        manual_review_pending: true,
        errors,
        usage: UsageSummary {
            request_count: 0,
            input_tokens: None,
            output_tokens: None,
            cost: None,
            status: GateStatus::Pending,
        },
    })
}

pub fn write_artifact(path: &Path, artifact: &EvaluationArtifact) -> Result<(), EvalError> {
    if !path.is_absolute() || path.exists() || path.extension().and_then(|value| value.to_str()) != Some("json") || path.components().any(|component| matches!(component, Component::Normal(value) if value == "vault" || value == "production")) {
        return Err(EvalError::UnsafeArtifactPath);
    }
    fs::write(
        path,
        serde_json::to_vec_pretty(artifact).map_err(|_| EvalError::ArtifactIo)?,
    )
    .map_err(|_| EvalError::ArtifactIo)
}

/// The schema revision used by the isolated live runner.  This is deliberately
/// separate from the M0 synthetic fixture schema: a real run must never be
/// made live merely by flipping `synthetic_only` in an old fixture.
pub const LIVE_RUN_SCHEMA: &str = "semantic-memory-eval-live-run-v1";

/// A request sent to the application-owned Provider boundary.  The runner
/// never knows about HTTP, credentials, retries, or a vendor wire format.
/// Production adapters should implement this by calling
/// `ProviderService::generate_structured`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LiveProviderRequest {
    pub sequence: u32,
    pub stage: String,
    pub arm: ComparisonArm,
    pub source_id: Option<String>,
    pub task_id: Option<String>,
    pub model_id: String,
    pub prompt_id: String,
    pub schema_id: String,
    pub index_profile_id: String,
    /// This is intentionally not written to an artifact. It may contain the
    /// source blocks assembled by SemanticMemoryService.
    pub input: serde_json::Value,
}

/// Redacted result returned by the Provider application boundary.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LiveProviderOutput {
    pub output: serde_json::Value,
    pub usage: Option<serde_json::Value>,
    pub cost_minor: Option<u64>,
}

/// Provider errors are deliberately code-only.  Implementations must not put
/// response bodies, prompts, credentials, or source text in this value.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LiveProviderError {
    pub code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema_issue: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_json_diagnostic: Option<StructuredJsonDiagnostic>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol_issue: Option<StrictFunctionCallIssue>,
}

#[async_trait::async_trait]
pub trait ProviderAppBoundary: Send + Sync {
    async fn generate(
        &self,
        request: LiveProviderRequest,
    ) -> Result<LiveProviderOutput, LiveProviderError>;

    /// Verify the captured Provider configuration at each live-call boundary.
    /// Local fake providers may use the default; the production adapter
    /// compares the exact non-secret runtime fingerprint.
    async fn verify_frozen_configuration(
        &self,
        _expected: &ProviderRuntimeSnapshot,
    ) -> Result<(), LiveProviderError> {
        Ok(())
    }

    /// Cumulative number of request attempts reserved by the shared transport.
    /// `None` identifies a local fake and makes one boundary call count as one.
    fn transport_attempt_count(&self) -> Option<u32> {
        None
    }
}

/// The semantic application boundary used by the runner.  A production
/// implementation should delegate `prepare` and `submit_observation` to
/// `SemanticMemoryService::prepare_source` and
/// `SemanticMemoryService::submit_proposal_json`, respectively.  Keeping this
/// boundary here prevents Provider code from entering the evaluator and makes
/// local fake-provider contract tests deterministic.
#[async_trait::async_trait]
pub trait SemanticMemoryAppBoundary: Send + Sync {
    /// Ordinary note retrieval for arm A. The implementation must resolve
    /// selected sources through Vault Core and may not inspect the manifest's
    /// complete source set on behalf of the runner.
    async fn ordinary_retrieve(
        &self,
        task: &EvalTask,
        sources: &[EvalSource],
        budget: &Budget,
    ) -> Result<serde_json::Value, String> {
        let _ = (task, sources, budget);
        Err("ordinary_retrieval_unimplemented".to_owned())
    }

    /// Profile-aware ordinary retrieval. The default preserves compatibility
    /// with local fakes; production adapters must bind the frozen profile to
    /// the real index/application service.
    async fn ordinary_retrieve_with_profile(
        &self,
        task: &EvalTask,
        sources: &[EvalSource],
        budget: &Budget,
        _index_profile_id: &str,
    ) -> Result<serde_json::Value, String> {
        self.ordinary_retrieve(task, sources, budget).await
    }

    async fn prepare(&self, source: &EvalSource) -> Result<serde_json::Value, String>;

    async fn build_pack(
        &self,
        task: &EvalTask,
        source_paths: &[String],
        budget: &Budget,
    ) -> Result<serde_json::Value, String>;

    /// Arm-aware pack boundary. Production implementations should use the
    /// supplied arm to select an isolated semantic namespace/state. The
    /// default keeps local fakes source-compatible while still allowing the
    /// runner to enforce source-scope filtering in the memory service.
    async fn build_pack_for_arm(
        &self,
        _arm: ComparisonArm,
        task: &EvalTask,
        sources: &[EvalSource],
        budget: &Budget,
    ) -> Result<serde_json::Value, String> {
        self.build_pack(
            task,
            &sources
                .iter()
                .map(|source| source.path.clone())
                .collect::<Vec<_>>(),
            budget,
        )
        .await
    }

    /// Prepare current M2 relation candidates for one isolated task scope.
    /// `None` keeps local fake boundaries source-compatible; production C
    /// adapters return a bounded, temporary-ID candidate projection.
    async fn prepare_relation_input_for_arm(
        &self,
        _arm: ComparisonArm,
        _task: &EvalTask,
        _sources: &[EvalSource],
    ) -> Result<Option<serde_json::Value>, String> {
        Ok(None)
    }

    /// Apply one validated relation proposal through the owning semantic
    /// application service. This is called only when the prepared C scope
    /// contains candidates.
    async fn submit_relation_proposal_for_arm(
        &self,
        _arm: ComparisonArm,
        _task: &EvalTask,
        _sources: &[EvalSource],
        _proposal: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        Err("semantic_relation_submission_unimplemented".to_owned())
    }

    async fn prepare_for_arm(
        &self,
        _arm: ComparisonArm,
        source: &EvalSource,
    ) -> Result<serde_json::Value, String> {
        self.prepare(source).await
    }

    /// A80 returns a source-scoped list of bounded observation inputs. The
    /// default preserves test/legacy adapter compatibility with one batch.
    async fn prepare_a80_for_arm(
        &self,
        arm: ComparisonArm,
        source: &EvalSource,
        _provider_fingerprint: &str,
        _prompt_id: &str,
        _schema_id: &str,
    ) -> Result<serde_json::Value, String> {
        Ok(
            serde_json::json!({"batches":[{"batch_index":0,"input":self.prepare_for_arm(arm, source).await?}]}),
        )
    }

    async fn accept_a80_batch_for_arm(
        &self,
        arm: ComparisonArm,
        source: &EvalSource,
        _batch_index: u32,
        proposal: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        self.accept_observation_for_arm(arm, source, proposal).await
    }

    async fn reserve_a80_batch_for_arm(
        &self,
        _arm: ComparisonArm,
        _source: &EvalSource,
        _batch_index: u32,
    ) -> Result<String, String> {
        Ok("dispatching".to_owned())
    }

    async fn fail_a80_batch_for_arm(
        &self,
        _arm: ComparisonArm,
        _source: &EvalSource,
        _batch_index: u32,
        _safe_error_code: &str,
    ) -> Result<(), String> {
        Err("semantic_observation_batch_failure_unimplemented".to_owned())
    }

    async fn reserve_a80_regen_for_arm(
        &self,
        _arm: ComparisonArm,
        _source: &EvalSource,
        _batch_index: u32,
    ) -> Result<String, String> {
        Err("semantic_observation_batch_regen_unimplemented".to_owned())
    }

    async fn finalize_a80_for_arm(
        &self,
        arm: ComparisonArm,
        source: &EvalSource,
    ) -> Result<serde_json::Value, String> {
        self.submit_composition_for_arm(arm, source, &serde_json::json!({}))
            .await
    }

    async fn prepare_legacy_for_arm(
        &self,
        arm: ComparisonArm,
        source: &EvalSource,
    ) -> Result<serde_json::Value, String> {
        self.prepare_for_arm(arm, source).await
    }

    async fn submit_observation_for_arm(
        &self,
        _arm: ComparisonArm,
        source: &EvalSource,
        proposal: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        self.submit_observation(source, proposal).await
    }

    /// Accept the first-stage observation result and return the bounded
    /// composition input. Legacy boundaries complete the old one-call path.
    async fn accept_observation_for_arm(
        &self,
        arm: ComparisonArm,
        source: &EvalSource,
        proposal: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        self.submit_observation_for_arm(arm, source, proposal).await
    }

    /// Return the number of redundant body/context role references normalized
    /// for the last accepted M6 V7 observation. Implementations without this
    /// indexed wire contract report zero.
    async fn take_observation_context_overlap_removed_for_arm(
        &self,
        _arm: ComparisonArm,
        _source: &EvalSource,
    ) -> u64 {
        0
    }

    async fn submit_composition_for_arm(
        &self,
        _arm: ComparisonArm,
        _source: &EvalSource,
        _proposal: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        Err("semantic_composition_submission_unimplemented".to_owned())
    }

    async fn abort_semantic_for_arm(
        &self,
        _arm: ComparisonArm,
        _source: &EvalSource,
    ) -> Result<(), String> {
        Ok(())
    }

    async fn current_card_projection_for_arm(
        &self,
        _arm: ComparisonArm,
        source: &EvalSource,
    ) -> Result<serde_json::Value, String> {
        self.current_card_projection(source).await
    }

    /// Check the arm's own isolated Markdown copy against the frozen source
    /// fence. Production implementations must override this for B/C.
    async fn verify_arm_task_sources(
        &self,
        _arm: ComparisonArm,
        _sources: &[EvalSource],
        _boundary: &str,
    ) -> Result<(), String> {
        Ok(())
    }

    async fn submit_observation(
        &self,
        source: &EvalSource,
        proposal: &serde_json::Value,
    ) -> Result<serde_json::Value, String>;

    async fn current_card_projection(
        &self,
        source: &EvalSource,
    ) -> Result<serde_json::Value, String>;
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LiveEvaluationConfig {
    pub schema_version: String,
    #[serde(default)]
    pub semantic_protocol: String,
    pub manifest: EvaluationManifest,
    pub run_config: EvaluationRunConfig,
    pub run_root: String,
    pub source_root: String,
    pub state_root: String,
    pub history_root: String,
    pub artifact_root: String,
    pub isolated_vault_id: String,
    /// B/C each operate on distinct, pre-provisioned copies of the explicitly
    /// selected evaluation sources. Their SQLite and filesystem roots must be
    /// disjoint from A and from each other. No copy is made by this runner.
    pub semantic_arm_roots: SemanticArmRoots,
    pub current_schema: String,
    pub task_budget: u32,
    pub unbounded_cost_authorized: bool,
    /// Immutable provider identity/configuration captured in the run hash.
    /// The internal model id is supplied by the CLI adapter when available;
    /// local contract fakes may leave it absent.
    #[serde(default)]
    pub provider_model_id: Option<String>,
    #[serde(default)]
    pub provider_runtime_snapshot: Option<ProviderRuntimeSnapshot>,
    #[serde(default)]
    pub provider_templates: Vec<ProviderStageTemplate>,
    #[serde(default)]
    pub isolated_master_key_path: Option<String>,
}

/// Compute the bounded M1 request count from the selected manifest rather than
/// copying the configured transport budget. Two-stage M1 adds one composition
/// call for each selected B/C observation source; answers and C relations are
/// counted from their selected task sets.
pub fn computed_semantic_request_upper_bound(config: &LiveEvaluationConfig) -> u32 {
    let selected_tasks = config
        .manifest
        .tasks
        .iter()
        .filter(|task| config.run_config.comparisons[0].task_ids.contains(&task.id))
        .collect::<Vec<_>>();
    let source_union = selected_tasks
        .iter()
        .flat_map(|task| task.source_ids.iter().cloned())
        .collect::<BTreeSet<_>>();
    let observation_sources = config.run_config.comparisons[1..]
        .iter()
        .map(|arm| {
            arm.source_ids
                .iter()
                .filter(|id| source_union.contains(*id))
                .count() as u32
        })
        .sum::<u32>();
    let answer_calls = config
        .run_config
        .comparisons
        .iter()
        .map(|arm| arm.task_ids.len() as u32)
        .sum::<u32>();
    let relation_calls = config
        .manifest
        .tasks
        .iter()
        .filter(|task| {
            config.run_config.comparisons[2].task_ids.contains(&task.id)
                && !task.expected_source_relations.is_empty()
        })
        .count() as u32;
    if config.semantic_protocol == "m1-a80-v1" {
        let block_counts = config
            .manifest
            .sources
            .iter()
            .map(|source| (source.logical_id.as_str(), source.logical_block_count))
            .collect::<BTreeMap<_, _>>();
        let selected_chunks = config.run_config.comparisons[1..]
            .iter()
            .flat_map(|arm| arm.source_ids.iter())
            .filter(|source_id| source_union.contains(*source_id))
            .map(|source_id| {
                a80_batch_count(
                    block_counts
                        .get(source_id.as_str())
                        .copied()
                        .unwrap_or_default(),
                )
            })
            .sum::<u32>();
        let one_retry_per_source_arm = observation_sources;
        let one_retry_per_relation = relation_calls;
        let bounded_answer_retries = answer_calls.min(4);
        selected_chunks
            .saturating_add(answer_calls)
            .saturating_add(relation_calls)
            .saturating_add(one_retry_per_source_arm)
            .saturating_add(one_retry_per_relation)
            .saturating_add(bounded_answer_retries)
    } else {
        observation_sources + observation_sources + answer_calls + relation_calls
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticArmRoot {
    pub vault_slug: String,
    pub vault_id: String,
    pub source_root: String,
    pub state_root: String,
    pub history_root: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticArmRoots {
    pub b: SemanticArmRoot,
    pub c: SemanticArmRoot,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveRunStatus {
    Running,
    Completed,
    Failed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LiveEvaluationResult {
    pub status: LiveRunStatus,
    pub manifest_hash: String,
    pub run_config_hash: String,
    pub artifact_root: String,
    pub provider_requests: u32,
    pub completed_tasks: u32,
    pub first_error_code: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct LiveArtifactRecord {
    sequence: u32,
    stage: String,
    arm: ComparisonArm,
    source_id: Option<String>,
    task_id: Option<String>,
    pack_hash: Option<String>,
    output: serde_json::Value,
    usage: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct LiveFailureRecord {
    sequence: u32,
    stage: String,
    code: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct LiveItemFailureRecord {
    sequence: u32,
    arm: ComparisonArm,
    source_id: Option<String>,
    task_id: Option<String>,
    stage: String,
    code: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct LiveCheckpoint {
    schema_version: &'static str,
    manifest_hash: String,
    run_config_hash: String,
    live_config_hash: String,
    last_boundary: String,
    provider_requests: u32,
    completed_tasks: u32,
    status: LiveRunStatus,
    first_error: Option<LiveFailureRecord>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LiveResumeMetadata {
    pub provider_requests_consumed: u32,
    pub last_attempt_sequence: u32,
    pub completed_tasks: u32,
    pub attempts: Vec<LiveAttemptIdentity>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LiveAttemptIdentity {
    pub sequence: u32,
    pub stage: String,
    pub arm: String,
    pub source_id: Option<String>,
    pub task_id: Option<String>,
    pub batch_index: Option<u32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PersistedCheckpoint {
    schema_version: String,
    manifest_hash: String,
    run_config_hash: String,
    live_config_hash: String,
    last_boundary: String,
    provider_requests: u32,
    completed_tasks: u32,
    status: String,
    first_error: Option<serde_json::Value>,
}

/// Inspect only safe run-ledger metadata. A nonempty A80 artifact root is
/// resumable only when every sealed hash matches and its checkpoint is still
/// in progress; each durable started-attempt row consumes budget, including
/// an in-flight call interrupted by process exit.
pub fn inspect_live_resume(
    config: &LiveEvaluationConfig,
    manifest_hash: &str,
    run_config_hash: &str,
    live_config_hash: &str,
) -> Result<Option<LiveResumeMetadata>, EvalError> {
    let root = Path::new(&config.artifact_root);
    if !root.exists() {
        return Ok(None);
    }
    private_fs::validate_private_directory(root)
        .map_err(|_| EvalError::LiveConfig("resume artifact root is not private"))?;
    let mut entries = fs::read_dir(root).map_err(|_| EvalError::ArtifactIo)?;
    if entries.next().is_none() {
        return Ok(None);
    }
    if config.semantic_protocol != "m1-a80-v1" {
        return Err(EvalError::LiveConfig(
            "only A80 runs may resume a nonempty artifact root",
        ));
    }
    for final_name in [
        "manifest.json",
        "run-config.json",
        "live-config.json",
        "report.md",
    ] {
        if root.join(final_name).exists() {
            return Err(EvalError::LiveConfig(
                "finalized live artifacts cannot be resumed",
            ));
        }
    }
    let checkpoint_path = root.join("checkpoint.json");
    private_fs::validate_no_symlink_components(&checkpoint_path)
        .map_err(|_| EvalError::LiveConfig("resume checkpoint path is unsafe"))?;
    private_fs::validate_private_file(&checkpoint_path)
        .map_err(|_| EvalError::LiveConfig("resume checkpoint must be private"))?;
    let bytes = fs::read(&checkpoint_path).map_err(|_| EvalError::ArtifactIo)?;
    let checkpoint: PersistedCheckpoint = serde_json::from_slice(&bytes)
        .map_err(|_| EvalError::LiveConfig("resume checkpoint is invalid"))?;
    if checkpoint.schema_version != LIVE_RUN_SCHEMA
        || checkpoint.manifest_hash != manifest_hash
        || checkpoint.run_config_hash != run_config_hash
        || checkpoint.live_config_hash != live_config_hash
        || checkpoint.status != "running"
        || checkpoint.first_error.is_some()
        || checkpoint.last_boundary == "final"
    {
        return Err(EvalError::LiveConfig(
            "resume checkpoint does not match the in-progress sealed run",
        ));
    }
    let attempts_path = root.join("attempts.jsonl");
    let mut attempt_count = 0_u32;
    let mut last_attempt_sequence = 0_u32;
    let mut attempts = Vec::new();
    if attempts_path.exists() {
        private_fs::validate_no_symlink_components(&attempts_path)
            .map_err(|_| EvalError::LiveConfig("resume attempts path is unsafe"))?;
        private_fs::validate_private_file(&attempts_path)
            .map_err(|_| EvalError::LiveConfig("resume attempts ledger must be private"))?;
        let text = fs::read_to_string(&attempts_path).map_err(|_| EvalError::ArtifactIo)?;
        let source_ids = config
            .manifest
            .sources
            .iter()
            .map(|source| source.logical_id.as_str())
            .collect::<BTreeSet<_>>();
        let task_ids = config
            .manifest
            .tasks
            .iter()
            .map(|task| task.id.as_str())
            .collect::<BTreeSet<_>>();
        for line in text.lines() {
            let value: serde_json::Value = serde_json::from_str(line)
                .map_err(|_| EvalError::LiveConfig("resume attempts ledger is invalid"))?;
            let object = value
                .as_object()
                .ok_or(EvalError::LiveConfig("resume attempt record is invalid"))?;
            if object.len() != 7
                || object.keys().any(|key| {
                    ![
                        "sequence",
                        "stage",
                        "arm",
                        "source_id",
                        "task_id",
                        "batch_index",
                        "status",
                    ]
                    .contains(&key.as_str())
                })
                || !matches!(
                    object.get("status").and_then(serde_json::Value::as_str),
                    Some("reserved" | "started" | "uncertain")
                )
            {
                return Err(EvalError::LiveConfig("resume attempt record is invalid"));
            }
            let sequence = object
                .get("sequence")
                .and_then(serde_json::Value::as_u64)
                .and_then(|value| u32::try_from(value).ok())
                .filter(|value| *value > last_attempt_sequence)
                .ok_or(EvalError::LiveConfig("resume attempt sequence is invalid"))?;
            let stage = object
                .get("stage")
                .and_then(serde_json::Value::as_str)
                .ok_or(EvalError::LiveConfig("resume attempt stage is invalid"))?;
            let arm = object
                .get("arm")
                .and_then(serde_json::Value::as_str)
                .ok_or(EvalError::LiveConfig("resume attempt arm is invalid"))?;
            let source_id = object.get("source_id").and_then(serde_json::Value::as_str);
            let task_id = object.get("task_id").and_then(serde_json::Value::as_str);
            let batch_index = object
                .get("batch_index")
                .and_then(serde_json::Value::as_u64)
                .and_then(|value| u32::try_from(value).ok());
            let belongs_to_run = match stage {
                "observation" => {
                    matches!(arm, "b" | "c")
                        && source_id.is_some_and(|id| source_ids.contains(id))
                        && task_id.is_none()
                        && (config.semantic_protocol != "m1-a80-v1" || batch_index.is_some())
                }
                "relation" | "answer" => {
                    batch_index.is_none()
                        && (arm == "c" || (stage == "answer" && matches!(arm, "a" | "b")))
                }
                _ => false,
            };
            if !belongs_to_run
                || (task_id.is_some_and(|id| !task_ids.contains(id)))
                || (stage != "observation" && (source_id.is_some() || task_id.is_none()))
            {
                return Err(EvalError::LiveConfig("resume attempt identity is invalid"));
            }
            attempt_count = attempt_count
                .checked_add(1)
                .ok_or(EvalError::LiveConfig("resume attempt count overflow"))?;
            last_attempt_sequence = sequence;
            attempts.push(LiveAttemptIdentity {
                sequence,
                stage: stage.to_owned(),
                arm: arm.to_owned(),
                source_id: source_id.map(str::to_owned),
                task_id: task_id.map(str::to_owned),
                batch_index,
            });
        }
    }
    Ok(Some(LiveResumeMetadata {
        provider_requests_consumed: checkpoint.provider_requests.max(attempt_count),
        last_attempt_sequence: last_attempt_sequence.max(checkpoint.provider_requests),
        completed_tasks: checkpoint.completed_tasks,
        attempts,
    }))
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct LiveUsageStats {
    request_count: u32,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cost_minor: Option<u64>,
    cost_unknown: bool,
}

struct LiveProviderCall {
    output: LiveProviderOutput,
    attempts: u32,
}

struct LiveProviderCallError {
    code: String,
    attempts: u32,
    schema_issue: Option<String>,
    schema_path: Option<String>,
    structured_json_diagnostic: Option<StructuredJsonDiagnostic>,
    protocol_issue: Option<StrictFunctionCallIssue>,
}

fn persist_schema_diagnostic(
    root: &Path,
    sequence: u32,
    stage: &str,
    arm: &ComparisonArm,
    source_id: Option<&str>,
    task_id: Option<&str>,
    error: &LiveProviderCallError,
) -> Result<(), EvalError> {
    if error.schema_issue.is_none()
        && error.schema_path.is_none()
        && error.structured_json_diagnostic.is_none()
        && error.protocol_issue.is_none()
    {
        return Ok(());
    }
    let record = serde_json::json!({
        "sequence": sequence,
        "stage": stage,
        "arm": arm,
        "source_id": source_id,
        "task_id": task_id,
        "code": error.code,
        "issue": error.schema_issue,
        "path": error.schema_path,
        "structured_json_diagnostic": error.structured_json_diagnostic,
        "protocol_issue": error.protocol_issue,
    });
    let path = root.join("schema-diagnostics.jsonl");
    let mut bytes = if path.exists() {
        fs::read(&path).map_err(|_| EvalError::ArtifactIo)?
    } else {
        Vec::new()
    };
    bytes.extend(serde_json::to_vec(&record).map_err(|_| EvalError::ArtifactIo)?);
    bytes.push(b'\n');
    atomic_write(&path, &bytes)
}

#[allow(clippy::too_many_arguments)]
fn persist_provider_attempt(
    root: &Path,
    sequence: u32,
    stage: &str,
    arm: &ComparisonArm,
    source_id: Option<&str>,
    task_id: Option<&str>,
    batch_index: Option<u32>,
    status: &str,
) -> Result<(), EvalError> {
    let record = serde_json::json!({
        "sequence": sequence,
        "stage": stage,
        "arm": arm,
        "source_id": source_id,
        "task_id": task_id,
        "batch_index": batch_index,
        "status": status,
    });
    let path = root.join("attempts.jsonl");
    let mut bytes = if path.exists() {
        fs::read(&path).map_err(|_| EvalError::ArtifactIo)?
    } else {
        Vec::new()
    };
    bytes.extend(serde_json::to_vec(&record).map_err(|_| EvalError::ArtifactIo)?);
    bytes.push(b'\n');
    atomic_write(&path, &bytes)
}

fn mark_provider_attempt_started(root: &Path, sequence: u32) -> Result<(), EvalError> {
    let path = root.join("attempts.jsonl");
    private_fs::validate_no_symlink_components(&path)
        .map_err(|_| EvalError::LiveConfig("attempt ledger path is unsafe"))?;
    private_fs::validate_private_file(&path)
        .map_err(|_| EvalError::LiveConfig("attempt ledger must be private"))?;
    let text = fs::read_to_string(&path).map_err(|_| EvalError::ArtifactIo)?;
    let mut rows = text
        .lines()
        .map(|line| {
            serde_json::from_str::<serde_json::Value>(line)
                .map_err(|_| EvalError::LiveConfig("attempt ledger record is invalid"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let row = rows
        .iter_mut()
        .find(|row| {
            row.get("sequence").and_then(serde_json::Value::as_u64) == Some(u64::from(sequence))
        })
        .ok_or(EvalError::LiveConfig(
            "reserved attempt ledger row is missing",
        ))?;
    if row.get("status").and_then(serde_json::Value::as_str) != Some("reserved") {
        return Err(EvalError::LiveConfig(
            "attempt ledger transition is invalid",
        ));
    }
    row["status"] = serde_json::Value::String("started".to_owned());
    let mut bytes = Vec::new();
    for row in rows {
        bytes.extend(serde_json::to_vec(&row).map_err(|_| EvalError::ArtifactIo)?);
        bytes.push(b'\n');
    }
    atomic_write(&path, &bytes)
}

fn validate_live_manifest(manifest: &EvaluationManifest) -> Result<String, EvalError> {
    if manifest.schema_version != MANIFEST_SCHEMA || manifest.synthetic_only {
        return Err(EvalError::LiveConfig(
            "live manifest must use the current non-synthetic schema",
        ));
    }
    if manifest.dataset_id.trim().is_empty()
        || manifest.sources.is_empty()
        || manifest.tasks.is_empty()
    {
        return Err(EvalError::LiveConfig(
            "live manifest requires dataset, sources, and tasks",
        ));
    }
    let mut logical_ids = BTreeSet::new();
    let mut file_ids = BTreeSet::new();
    let mut revision_ids = BTreeSet::new();
    for source in &manifest.sources {
        if source.synthetic_placeholder
            || source.vault_id.trim().is_empty()
            || source.logical_id.trim().is_empty()
            || !logical_ids.insert(source.logical_id.as_str())
            || !file_ids.insert(source.file_id.as_str())
            || !revision_ids.insert(source.source_revision_id.as_str())
            || !is_hash(&source.content_hash)
        {
            return Err(EvalError::LiveConfig("live source identity is invalid"));
        }
        VaultPath::parse(&source.path)
            .map_err(|_| EvalError::LiveConfig("live source path is unsafe"))?;
        reject_secret_fields(source)?;
    }
    let task_ids = manifest
        .tasks
        .iter()
        .map(|task| task.id.as_str())
        .collect::<BTreeSet<_>>();
    if task_ids.len() != manifest.tasks.len()
        || manifest.tasks.iter().any(|task| {
            task.query.trim().is_empty()
                || task.query_refs.is_empty()
                || task.query_refs.iter().collect::<BTreeSet<_>>().len() != task.query_refs.len()
                || task.source_ids.is_empty()
                || task.source_ids.iter().collect::<BTreeSet<_>>().len() != task.source_ids.len()
                || task
                    .source_ids
                    .iter()
                    .any(|id| !logical_ids.contains(id.as_str()))
                || task.source_fence.is_empty()
        })
    {
        return Err(EvalError::LiveConfig(
            "live task identity or source is invalid",
        ));
    }
    let mut query_ref_ids = BTreeSet::new();
    let mut source_splits = std::collections::BTreeMap::<&str, &TaskSplit>::new();
    for task in &manifest.tasks {
        for query_ref in &task.query_refs {
            if !query_ref_ids.insert(query_ref.as_str()) {
                return Err(EvalError::LiveConfig(
                    "live query references must be globally unique",
                ));
            }
        }
        for source_id in &task.source_ids {
            if source_splits
                .insert(source_id.as_str(), &task.split)
                .is_some_and(|split| split != &task.split)
            {
                return Err(EvalError::LiveConfig(
                    "development, validation, and holdout sources must be disjoint",
                ));
            }
        }
    }
    if !manifest
        .tasks
        .iter()
        .any(|task| task.split == TaskSplit::Holdout)
    {
        return Err(EvalError::LiveConfig(
            "live manifest requires a frozen holdout",
        ));
    }
    validate_task_source_fences(manifest)
        .map_err(|_| EvalError::LiveConfig("live task source fence is incomplete or stale"))?;
    for task in &manifest.tasks {
        let mut relation_keys = BTreeSet::new();
        for relation in &task.expected_source_relations {
            if relation.kind.trim().is_empty()
                || relation.source_ids.is_empty()
                || relation.source_ids.iter().collect::<BTreeSet<_>>().len()
                    != relation.source_ids.len()
                || relation
                    .source_ids
                    .iter()
                    .any(|id| !task.source_ids.iter().any(|task_id| task_id == id))
            {
                return Err(EvalError::LiveConfig(
                    "expected relation source IDs must belong to the task source set",
                ));
            }
            let mut ids = relation.source_ids.clone();
            ids.sort();
            if !relation_keys.insert(format!("{}:{:?}", relation.kind, ids)) {
                return Err(EvalError::LiveConfig(
                    "duplicate expected relation records are not allowed",
                ));
            }
        }
    }
    canonical_hash(manifest).map_err(|_| EvalError::LiveConfig("manifest serialization failed"))
}

fn validate_live_roots(config: &LiveEvaluationConfig) -> Result<(), EvalError> {
    #[cfg(not(unix))]
    {
        let _ = config;
        return Err(EvalError::LiveConfig(
            "private live-run filesystem permissions cannot be verified on this platform",
        ));
    }

    #[cfg(unix)]
    {
        let run_root = Path::new(&config.run_root);
        let paths = vec![
            Path::new(&config.source_root),
            Path::new(&config.state_root),
            Path::new(&config.history_root),
            Path::new(&config.artifact_root),
            Path::new(&config.semantic_arm_roots.b.source_root),
            Path::new(&config.semantic_arm_roots.b.state_root),
            Path::new(&config.semantic_arm_roots.b.history_root),
            Path::new(&config.semantic_arm_roots.c.source_root),
            Path::new(&config.semantic_arm_roots.c.state_root),
            Path::new(&config.semantic_arm_roots.c.history_root),
        ];
        if !run_root.is_absolute()
            || run_root.components().any(|component| {
                matches!(
                    component,
                    Component::Normal(value)
                        if value == "data" || value == "vault" || value == "production"
                )
            })
        {
            return Err(EvalError::LiveConfig(
                "run root is not an isolated absolute path",
            ));
        }
        validate_existing_components(run_root)?;
        for path in std::iter::once(run_root).chain(paths.iter().copied()) {
            private_fs::validate_private_directory_if_exists(path).map_err(|_| {
                EvalError::LiveConfig("live roots must be non-symlink directories with mode 0700")
            })?;
        }
        for path in &paths {
            if !path.is_absolute()
                || path.components().any(|component| {
                    matches!(
                        component,
                        Component::Normal(value)
                            if value == "data" || value == "vault" || value == "production"
                    )
                })
            {
                return Err(EvalError::LiveConfig(
                    "live roots must be absolute and outside ./data/production/vault",
                ));
            }
            validate_existing_components(path)?;
        }
        let run_real = canonical_for_overlap(run_root)?;
        for path in &paths {
            if !paths_overlap(&run_real, &canonical_for_overlap(path)?)
                || !canonical_for_overlap(path)?.starts_with(&run_real)
            {
                return Err(EvalError::LiveConfig(
                    "live root is outside isolated run root",
                ));
            }
        }
        for (index, path) in paths.iter().enumerate() {
            let current = canonical_for_overlap(path)?;
            if paths.iter().skip(index + 1).any(|other| {
                canonical_for_overlap(other).is_ok_and(|other| paths_overlap(&current, &other))
            }) {
                return Err(EvalError::LiveConfig("live roots overlap"));
            }
        }
        for state_root in [
            Path::new(&config.state_root),
            Path::new(&config.semantic_arm_roots.b.state_root),
            Path::new(&config.semantic_arm_roots.c.state_root),
        ] {
            validate_isolated_state_database_leaf(&config.run_root, state_root)?;
        }
        if config.isolated_vault_id.trim().is_empty()
            || config.isolated_vault_id == "production"
            || config.current_schema != LIVE_RUN_SCHEMA
        {
            return Err(EvalError::LiveConfig(
                "isolated Vault identity or current schema is invalid",
            ));
        }
        let b = &config.semantic_arm_roots.b;
        let c = &config.semantic_arm_roots.c;
        if b.vault_id.trim().is_empty()
            || c.vault_id.trim().is_empty()
            || b.vault_id == c.vault_id
            || b.vault_id == config.isolated_vault_id
            || c.vault_id == config.isolated_vault_id
            || b.vault_slug.trim().is_empty()
            || c.vault_slug.trim().is_empty()
            || b.vault_slug == c.vault_slug
        {
            return Err(EvalError::LiveConfig(
                "A/B/C semantic Vault identities must be distinct",
            ));
        }
        for source in &config.manifest.sources {
            let path = VaultPath::parse(&source.path)
                .map_err(|_| EvalError::LiveConfig("source path is unsafe"))?;
            for source_root in [
                config.source_root.as_str(),
                b.source_root.as_str(),
                c.source_root.as_str(),
            ] {
                let source_path = Path::new(source_root).join(path.as_str());
                if !source_path.starts_with(source_root) {
                    return Err(EvalError::LiveConfig("source file escapes source root"));
                }
                validate_existing_components(&source_path)?;
                if let Ok(metadata) = fs::symlink_metadata(&source_path)
                    && (metadata.file_type().is_symlink() || !metadata.is_file())
                {
                    return Err(EvalError::LiveConfig("source file is not a regular file"));
                }
            }
        }
        if let Some(master_key_path) = &config.isolated_master_key_path {
            validate_isolated_master_key_path(
                config.run_root.as_str(),
                Path::new(master_key_path),
            )?;
        }
        Ok(())
    }
}

fn validate_live_config(config: &LiveEvaluationConfig) -> Result<(String, String), EvalError> {
    // Root isolation must be established before any other preflight failure
    // can be considered eligible for a persisted error checkpoint.
    validate_live_roots(config)?;
    let manifest_hash = validate_live_manifest(&config.manifest)?;
    let run_config = &config.run_config;
    if config.schema_version != LIVE_RUN_SCHEMA
        || run_config.mode != EvalMode::Live
        || !run_config.allow_live
        || !run_config.explicit_live_authorization
        || (run_config.cost_budget_minor == 0 && !config.unbounded_cost_authorized)
        || run_config.comparisons.len() != 3
        || config.task_budget == 0
        || config.task_budget as usize > config.manifest.tasks.len()
    {
        return Err(EvalError::LiveBlocked(
            "live authorization, positive cost/request/task budgets, and A/B/C are required",
        ));
    }
    validate_provider_templates(config)?;
    if config.provider_model_id.is_some() && config.provider_runtime_snapshot.is_none() {
        return Err(EvalError::LiveBlocked(
            "production live runs require a frozen Provider runtime fingerprint",
        ));
    }
    if let Some(snapshot) = &config.provider_runtime_snapshot
        && (config.provider_model_id.as_deref() != Some(snapshot.model_id.as_str())
            || config.run_config.comparisons[0].answer_model_id != snapshot.external_model_id
            || snapshot.fingerprint.trim().is_empty())
    {
        return Err(EvalError::LiveBlocked(
            "Provider runtime fingerprint does not match internal/external model identity",
        ));
    }
    if run_config.artifact_root.as_deref() != Some(config.artifact_root.as_str()) {
        return Err(EvalError::LiveBlocked(
            "run-config artifact root must match the isolated artifact root",
        ));
    }
    if !config.unbounded_cost_authorized && run_config.cost_budget_minor > 0 {
        return Err(EvalError::LiveBlocked(
            "finite currency budgets require an explicit pre-request estimate; unsupported",
        ));
    }
    let expected_allowlist = config
        .manifest
        .sources
        .iter()
        .flat_map(|source| [source.logical_id.clone(), source.file_id.clone()])
        .collect::<BTreeSet<_>>();
    let actual_allowlist = run_config
        .source_allowlist
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    if actual_allowlist != expected_allowlist
        || actual_allowlist.len() != run_config.source_allowlist.len()
    {
        return Err(EvalError::LiveBlocked(
            "source allowlist must exactly cover logical and file IDs",
        ));
    }
    let expected_arms = [ComparisonArm::A, ComparisonArm::B, ComparisonArm::C];
    let manifest_sources = config
        .manifest
        .sources
        .iter()
        .map(|source| source.logical_id.as_str())
        .collect::<BTreeSet<_>>();
    let manifest_tasks = config
        .manifest
        .tasks
        .iter()
        .map(|task| task.id.as_str())
        .collect::<BTreeSet<_>>();
    for (index, arm) in run_config.comparisons.iter().enumerate() {
        if arm.arm != expected_arms[index]
            || arm.source_ids.is_empty()
            || arm.task_ids.is_empty()
            || arm
                .source_ids
                .iter()
                .any(|id| !manifest_sources.contains(id.as_str()))
            || arm
                .task_ids
                .iter()
                .any(|id| !manifest_tasks.contains(id.as_str()))
            || arm.index_profile_id.trim().is_empty()
            || arm.prompt_id.trim().is_empty()
            || arm.schema_id.trim().is_empty()
            || arm.answer_model_id.trim().is_empty()
            || arm.source_ids.iter().collect::<BTreeSet<_>>().len() != arm.source_ids.len()
            || arm.task_ids.iter().collect::<BTreeSet<_>>().len() != arm.task_ids.len()
            || arm.budget.external_request_budget == 0
            || arm.budget.max_entries == 0
            || arm.budget.max_bytes == 0
            || arm.budget.max_tokens == 0
        {
            return Err(EvalError::LiveBlocked(
                "fixed provider/model/prompt/schema/index and positive budgets are required",
            ));
        }
        reject_secret_fields(arm)?;
    }
    let first = &run_config.comparisons[0];
    let computed_upper_bound = computed_semantic_request_upper_bound(config);
    if matches!(
        config.semantic_protocol.as_str(),
        "m1-two-stage-v2" | "m1-a80-v1"
    ) && first.budget.external_request_budget < computed_upper_bound
    {
        return Err(EvalError::LiveBlocked(
            "two-stage semantic evaluation budget is below computed request upper bound",
        ));
    }
    if config.semantic_protocol == "m1-a80-v1"
        && config
            .manifest
            .sources
            .iter()
            .filter(|source| {
                config.run_config.comparisons[1..]
                    .iter()
                    .any(|arm| arm.source_ids.contains(&source.logical_id))
            })
            .any(|source| source.logical_block_count == 0)
    {
        return Err(EvalError::LiveBlocked(
            "A80 selected sources require frozen non-empty block counts",
        ));
    }
    if run_config.comparisons.iter().skip(1).any(|arm| {
        arm.budget != first.budget
            || arm.index_profile_id != first.index_profile_id
            || arm.prompt_id != first.prompt_id
            || arm.schema_id != first.schema_id
            || arm.answer_model_id != first.answer_model_id
    }) {
        return Err(EvalError::LiveBlocked("A/B/C configuration is not frozen"));
    }
    let task_set = first.task_ids.iter().collect::<BTreeSet<_>>();
    if run_config
        .comparisons
        .iter()
        .skip(1)
        .any(|arm| arm.task_ids.iter().collect::<BTreeSet<_>>() != task_set)
    {
        return Err(EvalError::LiveBlocked(
            "A/B/C must evaluate one identical task set",
        ));
    }
    if config.task_budget as usize != task_set.len() {
        return Err(EvalError::LiveBlocked(
            "task budget must equal the frozen comparison task set",
        ));
    }
    if config
        .manifest
        .tasks
        .iter()
        .any(|task| task_set.contains(&task.id) && task.split != TaskSplit::Holdout)
    {
        return Err(EvalError::LiveBlocked(
            "live task set must be selected from the frozen holdout",
        ));
    }
    let complete_holdout_task_set = config
        .manifest
        .tasks
        .iter()
        .filter(|task| task.split == TaskSplit::Holdout)
        .map(|task| &task.id)
        .collect::<BTreeSet<_>>();
    if task_set != complete_holdout_task_set {
        return Err(EvalError::LiveBlocked(
            "M6 live evaluation must include the complete frozen holdout task set",
        ));
    }
    let selected_task_source_union = config
        .manifest
        .tasks
        .iter()
        .filter(|task| task_set.contains(&task.id))
        .flat_map(|task| task.source_ids.iter().cloned())
        .collect::<BTreeSet<_>>();
    let b_config = &run_config.comparisons[1];
    let c_config = &run_config.comparisons[2];
    let a_config = &run_config.comparisons[0];
    for arm in [b_config, c_config] {
        if arm
            .source_ids
            .iter()
            .any(|source_id| !selected_task_source_union.contains(source_id))
        {
            return Err(EvalError::LiveBlocked(
                "semantic observation allowlist exceeds the selected holdout task source union",
            ));
        }
    }
    for task in &config.manifest.tasks {
        if !task_set.contains(&task.id) {
            continue;
        }
        let b_selected = task
            .source_ids
            .iter()
            .filter(|source_id| b_config.source_ids.contains(source_id))
            .collect::<Vec<_>>();
        let c_selected = task
            .source_ids
            .iter()
            .filter(|source_id| c_config.source_ids.contains(source_id))
            .collect::<Vec<_>>();
        if task.source_ids.iter().any(|source_id| {
            !a_config
                .source_ids
                .iter()
                .any(|allowed| allowed == source_id)
        }) {
            return Err(EvalError::LiveBlocked(
                "A arm must cover every required task source",
            ));
        }
        if b_config.task_ids.contains(&task.id) && b_selected.len() != 1 {
            return Err(EvalError::LiveBlocked(
                "B arm must use exactly one selected source per task",
            ));
        }
        if c_selected.is_empty()
            || task.expected_source_relations.iter().any(|relation| {
                relation
                    .source_ids
                    .iter()
                    .any(|id| !c_selected.contains(&id))
            })
        {
            return Err(EvalError::LiveBlocked(
                "C arm must select every task relation source",
            ));
        }
    }
    let run_config_hash = canonical_hash(run_config)
        .map_err(|_| EvalError::LiveConfig("run config serialization failed"))?;
    Ok((manifest_hash, run_config_hash))
}

fn validate_provider_templates(config: &LiveEvaluationConfig) -> Result<(), EvalError> {
    let required = ["observation", "relation", "answer"];
    let has_composition = config
        .provider_templates
        .iter()
        .any(|template| template.stage == "composition");
    if !config.semantic_protocol.is_empty()
        && !matches!(
            config.semantic_protocol.as_str(),
            "m1-two-stage-v2" | "m1-a80-v1"
        )
    {
        return Err(EvalError::LiveBlocked(
            "unknown semantic evaluation protocol",
        ));
    }
    if has_composition != (config.semantic_protocol == "m1-two-stage-v2") {
        return Err(EvalError::LiveBlocked(
            "two-stage protocol and composition template must be paired",
        ));
    }
    if !(config.provider_templates.len() == 3 || config.provider_templates.len() == 4)
        || required.iter().any(|stage| {
            config
                .provider_templates
                .iter()
                .filter(|template| template.stage == *stage)
                .count()
                != 1
        })
        || config.provider_templates.iter().any(|template| {
            template.stage.trim().is_empty()
                || !required.contains(&template.stage.as_str()) && template.stage != "composition"
        })
    {
        return Err(EvalError::LiveBlocked(
            "observation, relation, and answer templates must be present exactly once",
        ));
    }
    let answer_model = &config.run_config.comparisons[0].answer_model_id;
    if config.provider_templates.iter().any(|template| {
        template.model_id != *answer_model
            || template.prompt_id != config.run_config.comparisons[0].prompt_id
            || template.schema_id != config.run_config.comparisons[0].schema_id
            || template.index_profile_id != config.run_config.comparisons[0].index_profile_id
            || template.schema_name.trim().is_empty()
            || template.system.trim().is_empty()
            || template.max_output_tokens == 0
            || template.max_output_tokens > config.run_config.comparisons[0].budget.max_tokens
            || template.timeout_seconds == 0
    }) {
        return Err(EvalError::LiveBlocked(
            "provider templates do not match frozen run configuration",
        ));
    }
    if let Some(internal) = &config.provider_model_id
        && internal.trim().is_empty()
    {
        return Err(EvalError::LiveBlocked(
            "internal Provider model id is empty",
        ));
    }
    for template in &config.provider_templates {
        reject_secret_fields(template)?;
    }
    Ok(())
}

/// Run the complete configuration/root preflight without opening a database
/// or invoking any application boundary.  The dedicated CLI calls this before
/// loading its isolated current-schema state.
pub fn validate_live_evaluation_config(
    config: &LiveEvaluationConfig,
) -> Result<(String, String), EvalError> {
    validate_live_config(config)
}

/// Validate the isolated master-key reference before opening the state DB.
/// The key itself may be a not-yet-created leaf, but every existing ancestor
/// must be non-symlink and the canonical parent must remain under run_root.
pub fn validate_isolated_master_key_path(
    run_root: &str,
    master_key_path: &Path,
) -> Result<(), EvalError> {
    let root = Path::new(run_root);
    if !master_key_path.is_absolute() {
        return Err(EvalError::LiveConfig("master key path must be absolute"));
    }
    private_fs::validate_no_symlink_components(master_key_path)
        .map_err(|_| EvalError::LiveConfig("master key path contains an unsafe component"))?;
    validate_existing_components(master_key_path)?;
    let key_parent = master_key_path
        .parent()
        .ok_or(EvalError::LiveConfig("master key path has no parent"))?;
    let root_real = canonical_for_overlap(root)?;
    let parent_real = canonical_for_overlap(key_parent)?;
    if !parent_real.starts_with(&root_real) {
        return Err(EvalError::LiveConfig(
            "master key path must remain inside isolated run root",
        ));
    }
    private_fs::validate_private_directory_if_exists(key_parent)
        .map_err(|_| EvalError::LiveConfig("master key parent must be private mode 0700"))?;
    if let Ok(metadata) = fs::symlink_metadata(master_key_path)
        && (!metadata.is_file() || metadata.file_type().is_symlink())
    {
        return Err(EvalError::LiveConfig(
            "master key path must be a regular non-symlink file",
        ));
    }
    if master_key_path.exists() {
        private_fs::validate_private_file(master_key_path)
            .map_err(|_| EvalError::LiveConfig("master key must be a private mode 0600 file"))?;
    }
    Ok(())
}

/// Create the M6 live roots with the same private-directory policy as the
/// Provider capability probe before opening SQLite or Provider services.
pub fn prepare_live_private_directories(
    config: &LiveEvaluationConfig,
    master_key_path: &Path,
) -> Result<(), EvalError> {
    validate_live_config(config)?;
    validate_isolated_master_key_path(&config.run_root, master_key_path)?;
    for directory in live_private_directories(config)
        .into_iter()
        .chain(std::iter::once(master_key_path.parent().ok_or(
            EvalError::LiveConfig("master key path has no parent"),
        )?))
    {
        private_fs::ensure_private_directory(directory)
            .map_err(|_| EvalError::LiveConfig("live root must be private mode 0700"))?;
    }
    validate_live_config(config)?;
    validate_isolated_master_key_path(&config.run_root, master_key_path)
}

fn live_private_directories(config: &LiveEvaluationConfig) -> Vec<&Path> {
    vec![
        Path::new(&config.run_root),
        Path::new(&config.source_root),
        Path::new(&config.state_root),
        Path::new(&config.history_root),
        Path::new(&config.artifact_root),
        Path::new(&config.semantic_arm_roots.b.source_root),
        Path::new(&config.semantic_arm_roots.b.state_root),
        Path::new(&config.semantic_arm_roots.b.history_root),
        Path::new(&config.semantic_arm_roots.c.source_root),
        Path::new(&config.semantic_arm_roots.c.state_root),
        Path::new(&config.semantic_arm_roots.c.history_root),
    ]
}

fn prepare_live_root_directories(config: &LiveEvaluationConfig) -> Result<(), EvalError> {
    for directory in live_private_directories(config) {
        private_fs::ensure_private_directory(directory)
            .map_err(|_| EvalError::LiveConfig("live root must be private mode 0700"))?;
    }
    Ok(())
}

/// Validate the actual SQLite leaf before or after opening/migration. On Unix,
/// a missing leaf is permitted before connect and an existing leaf must be
/// private to one directory entry, regular, non-symlink, and inside run_root.
/// Other platforms fail closed before either a missing or existing leaf can be
/// opened because their standard metadata API does not expose a reliable link
/// count.
#[cfg(unix)]
pub fn validate_isolated_state_database_leaf(
    run_root: &str,
    state_root: &Path,
) -> Result<std::path::PathBuf, EvalError> {
    let root = Path::new(run_root);
    let database = state_root.join("state.sqlite3");
    if !root.is_absolute() || !state_root.is_absolute() || !database.is_absolute() {
        return Err(EvalError::LiveConfig(
            "isolated SQLite paths must be absolute",
        ));
    }
    validate_existing_components(root)?;
    validate_existing_components(state_root)?;
    validate_existing_components(&database)?;
    let root_real = canonical_for_overlap(root)?;
    let parent = database
        .parent()
        .ok_or(EvalError::LiveConfig("isolated SQLite leaf has no parent"))?;
    let parent_real = canonical_for_overlap(parent)?;
    if !parent_real.starts_with(&root_real) {
        return Err(EvalError::LiveConfig(
            "isolated SQLite parent escapes the live run root",
        ));
    }
    match fs::symlink_metadata(&database) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err(EvalError::LiveConfig(
                "isolated SQLite leaf must be a regular non-symlink file",
            ));
        }
        Ok(metadata) => {
            validate_single_link_state_database_leaf(&metadata)?;
            let database_real = fs::canonicalize(&database).map_err(|_| {
                EvalError::LiveConfig("isolated SQLite leaf cannot be canonicalized")
            })?;
            if !database_real.starts_with(&root_real) {
                return Err(EvalError::LiveConfig(
                    "isolated SQLite leaf escapes the live run root",
                ));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(EvalError::LiveConfig("isolated SQLite leaf is unavailable")),
    }
    Ok(database)
}

#[cfg(unix)]
fn validate_single_link_state_database_leaf(metadata: &fs::Metadata) -> Result<(), EvalError> {
    if metadata.nlink() != 1 {
        return Err(EvalError::LiveConfig(
            "isolated SQLite leaf must not have additional hard links",
        ));
    }
    Ok(())
}

#[cfg(not(unix))]
pub fn validate_isolated_state_database_leaf(
    _run_root: &str,
    _state_root: &Path,
) -> Result<std::path::PathBuf, EvalError> {
    Err(EvalError::LiveConfig(
        "isolated SQLite leaf hard-link count cannot be verified on this platform",
    ))
}

fn safe_usage_stats(usage: Option<&serde_json::Value>) -> (Option<u64>, Option<u64>) {
    let Some(usage) = usage else {
        return (None, None);
    };
    (
        usage
            .get("input_tokens")
            .or_else(|| usage.get("prompt_tokens"))
            .and_then(serde_json::Value::as_u64),
        usage
            .get("output_tokens")
            .or_else(|| usage.get("completion_tokens"))
            .and_then(serde_json::Value::as_u64),
    )
}

fn redact_artifact_value(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => serde_json::Value::Object(
            map.iter()
                .filter_map(|(key, value)| {
                    let normalized = key
                        .chars()
                        .filter(|character| character.is_ascii_alphanumeric())
                        .flat_map(char::to_lowercase)
                        .collect::<String>();
                    if is_sensitive_artifact_key(&normalized) {
                        None
                    } else {
                        Some((key.clone(), redact_artifact_value(value)))
                    }
                })
                .collect(),
        ),
        serde_json::Value::Array(values) => {
            serde_json::Value::Array(values.iter().map(redact_artifact_value).collect())
        }
        _ => value.clone(),
    }
}

fn is_sensitive_artifact_key(normalized: &str) -> bool {
    matches!(
        normalized,
        "prompt"
            | "system"
            | "user"
            | "content"
            | "body"
            | "token"
            | "accesstoken"
            | "refreshtoken"
            | "authorization"
            | "password"
            | "apikey"
            | "secret"
            | "sourcetext"
            | "sourcebody"
            | "sourcecontent"
            | "notebody"
            | "notecontent"
            | "requestheaders"
            | "headers"
    ) || normalized.contains("token")
        || normalized.contains("apikey")
        || normalized.contains("authorization")
        || normalized.contains("requestheader")
        || normalized.contains("sourcetext")
        || normalized.contains("notebody")
}

fn project_provider_output(stage: &str, value: &serde_json::Value) -> serde_json::Value {
    let allowed = match stage {
        "observation" => [
            "outcome",
            "observations",
            "cards",
            "statement",
            "kind",
            "scope",
            "assertion_status",
            "conditions",
            "exceptions",
            "ordered_steps",
            "result",
            "uncertainty",
            "admission_reason",
            "value_for_future_work",
            "body_block_ids",
            "context_block_ids",
        ]
        .as_slice(),
        "relation" => [
            "actions",
            "kind",
            "relation",
            "action",
            "status",
            "reason",
            "source_ids",
            "evidence_refs",
            "candidate_ids",
            "candidate_count",
            "decision_count",
            "card_ref",
            "title",
            "content",
            "item_kind",
            "support_operator",
        ]
        .as_slice(),
        "answer" => ["answer", "answerability", "status", "evidence_ids"].as_slice(),
        _ => [].as_slice(),
    };
    let Some(object) = value.as_object() else {
        return serde_json::json!({});
    };
    let projected = object
        .iter()
        .filter(|(key, _)| allowed.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), redact_artifact_value(value)))
        .collect::<serde_json::Map<_, _>>();
    serde_json::Value::Object(projected)
}

fn project_card_projection(value: &serde_json::Value) -> serde_json::Value {
    fn project_object(value: &serde_json::Value, allowed: &[&str]) -> serde_json::Value {
        let Some(object) = value.as_object() else {
            return serde_json::json!({});
        };
        serde_json::Value::Object(
            object
                .iter()
                .filter(|(key, _)| allowed.contains(&key.as_str()))
                .map(|(key, value)| (key.clone(), project_card_value(value)))
                .collect(),
        )
    }
    fn project_card_value(value: &serde_json::Value) -> serde_json::Value {
        match value {
            serde_json::Value::Object(_) => project_object(
                value,
                &[
                    "card_id",
                    "revision_id",
                    "title",
                    "kind",
                    "scope",
                    "scope_ref",
                    "assertion_status",
                    "status",
                    "value",
                    "temporal_scope",
                    "source_id",
                    "source_revision_id",
                    "card_count",
                    "cards",
                    "assertions",
                    "items",
                    "statement",
                    "content",
                    "conditions",
                    "exceptions",
                    "ordered_steps",
                    "result",
                    "uncertainty",
                    "qualifiers",
                    "evidence_refs",
                    "support_refs",
                    "observation_id",
                    "ordinal",
                    "assertion_hash",
                    "card_hash",
                ],
            ),
            serde_json::Value::Array(values) => {
                serde_json::Value::Array(values.iter().map(project_card_value).collect())
            }
            _ => value.clone(),
        }
    }
    let mut projected = project_card_value(value);
    if let Some(object) = projected.as_object_mut() {
        object.remove("card_hash");
    }
    let hash = canonical_hash(&projected).unwrap_or_default();
    if let Some(object) = projected.as_object_mut() {
        object.insert("card_hash".to_owned(), serde_json::json!(hash));
    }
    projected
}

fn card_artifact_projection(value: &serde_json::Value, source: &EvalSource) -> serde_json::Value {
    let mut projected = project_card_projection(value);
    let safe_projection_hash = projected
        .get("card_hash")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_owned();
    if let Some(object) = projected.as_object_mut() {
        object.insert(
            "canonical_markdown_hash".to_owned(),
            serde_json::json!(source.content_hash),
        );
        object.insert(
            "safe_projection_hash".to_owned(),
            serde_json::json!(safe_projection_hash),
        );
        object.insert(
            "hash_semantics".to_owned(),
            serde_json::json!({
                "canonical_markdown_hash": "SHA-256 of the frozen canonical source Markdown bytes",
                "safe_projection_hash": "SHA-256 of the allowlisted source-scoped card collection projection before hash metadata",
            }),
        );
    }
    projected
}

fn project_pack_projection(value: &serde_json::Value) -> Result<serde_json::Value, EvalError> {
    // Deserialize the application boundary response into the production
    // MemoryPack type first. This prevents arbitrary adapter JSON from
    // expanding the persisted Provider material contract.
    let required = [
        "current_context",
        "relevant_experiences",
        "conflicts_or_checks",
        "evidence_gaps",
        "related_sources",
        "diagnostics",
        "estimated_tokens",
    ];
    if !value
        .as_object()
        .is_some_and(|object| required.iter().all(|key| object.contains_key(*key)))
    {
        return Err(EvalError::LiveConfig("memory pack contract is incomplete"));
    }
    let pack = serde_json::from_value::<MemoryPack>(value.clone())
        .map_err(|_| EvalError::LiveConfig("memory pack contract is invalid"))?;
    let mut projected = serde_json::to_value(pack)
        .map_err(|_| EvalError::LiveConfig("memory pack projection failed"))?;
    // MemoryPack itself contains semantic assertions/qualifiers and source
    // bindings, not source note bodies. Its fields are the explicit allowlist.
    let hash =
        canonical_hash(&projected).map_err(|_| EvalError::LiveConfig("memory pack hash failed"))?;
    let object = projected
        .as_object_mut()
        .ok_or(EvalError::LiveConfig("memory pack projection failed"))?;
    object.insert("pack_hash".to_owned(), serde_json::json!(hash));
    Ok(projected)
}

fn project_usage(value: Option<&serde_json::Value>) -> Option<serde_json::Value> {
    let value = value?.as_object()?;
    let mut projected = serde_json::Map::new();
    for key in [
        "input_tokens",
        "output_tokens",
        "prompt_tokens",
        "completion_tokens",
        "cached_tokens",
    ] {
        if let Some(number) = value.get(key).and_then(serde_json::Value::as_u64) {
            projected.insert(key.to_owned(), serde_json::json!(number.min(1_000_000_000)));
        }
    }
    Some(serde_json::Value::Object(projected))
}

fn update_usage_stats(
    stats: &mut LiveUsageStats,
    usage: Option<&serde_json::Value>,
    cost: Option<u64>,
    attempts: u32,
) {
    let (input_tokens, output_tokens) = safe_usage_stats(usage);
    stats.request_count = stats.request_count.saturating_add(attempts);
    if attempts != 1 {
        stats.input_tokens = None;
        stats.output_tokens = None;
        stats.cost_unknown = true;
        stats.cost_minor = None;
    } else {
        stats.input_tokens = match (stats.input_tokens, input_tokens) {
            (Some(total), Some(value)) => Some(total.saturating_add(value)),
            (None, Some(value)) if stats.request_count == attempts => Some(value),
            _ => None,
        };
        stats.output_tokens = match (stats.output_tokens, output_tokens) {
            (Some(total), Some(value)) => Some(total.saturating_add(value)),
            (None, Some(value)) if stats.request_count == attempts => Some(value),
            _ => None,
        };
        match cost {
            Some(cost) if !stats.cost_unknown => {
                stats.cost_minor = Some(stats.cost_minor.unwrap_or_default().saturating_add(cost));
            }
            Some(_) => {}
            None => {
                stats.cost_unknown = true;
                stats.cost_minor = None;
            }
        }
    }
}

fn record_arm_usage(
    usage_by_arm: &mut std::collections::BTreeMap<String, LiveUsageStats>,
    arm: &ComparisonArm,
    usage: Option<&serde_json::Value>,
    cost: Option<u64>,
    attempts: u32,
) {
    if attempts == 0 {
        return;
    }
    let stats = usage_by_arm
        .entry(format!("{arm:?}"))
        .or_insert(LiveUsageStats {
            request_count: 0,
            input_tokens: None,
            output_tokens: None,
            cost_minor: None,
            cost_unknown: false,
        });
    update_usage_stats(stats, usage, cost, attempts);
}

fn write_live_json(path: &Path, value: &impl Serialize) -> Result<(), EvalError> {
    atomic_write(
        path,
        &serde_json::to_vec_pretty(value).map_err(|_| EvalError::ArtifactIo)?,
    )
}

fn write_live_jsonl<T: Serialize>(path: &Path, values: &[T]) -> Result<(), EvalError> {
    let mut bytes = Vec::new();
    for value in values {
        bytes.extend(
            serde_json::to_vec(value)
                .map_err(|_| EvalError::ArtifactIo)?
                .as_slice(),
        );
        bytes.push(b'\n');
    }
    atomic_write(path, &bytes)
}

fn read_live_jsonl<T: serde::de::DeserializeOwned>(
    root: &Path,
    name: &str,
) -> Result<Vec<T>, EvalError> {
    let path = root.join(name);
    if !path.exists() {
        return Ok(Vec::new());
    }
    private_fs::validate_no_symlink_components(&path)
        .map_err(|_| EvalError::LiveConfig("resume artifact path is unsafe"))?;
    private_fs::validate_private_file(&path)
        .map_err(|_| EvalError::LiveConfig("resume artifact file must be private"))?;
    let text = fs::read_to_string(path).map_err(|_| EvalError::ArtifactIo)?;
    text.lines()
        .map(|line| serde_json::from_str(line).map_err(|_| EvalError::ArtifactIo))
        .collect()
}

fn read_live_values_jsonl(root: &Path, name: &str) -> Result<Vec<serde_json::Value>, EvalError> {
    read_live_jsonl(root, name)
}

fn read_live_usage(
    root: &Path,
) -> Result<std::collections::BTreeMap<String, LiveUsageStats>, EvalError> {
    let path = root.join("usage.json");
    if !path.exists() {
        return Ok(std::collections::BTreeMap::new());
    }
    private_fs::validate_no_symlink_components(&path)
        .map_err(|_| EvalError::LiveConfig("resume usage path is unsafe"))?;
    private_fs::validate_private_file(&path)
        .map_err(|_| EvalError::LiveConfig("resume usage file must be private"))?;
    let value: serde_json::Value =
        serde_json::from_slice(&fs::read(path).map_err(|_| EvalError::ArtifactIo)?)
            .map_err(|_| EvalError::ArtifactIo)?;
    serde_json::from_value(
        value
            .get("by_arm")
            .cloned()
            .ok_or(EvalError::LiveConfig("resume usage summary is incomplete"))?,
    )
    .map_err(|_| EvalError::LiveConfig("resume usage summary is invalid"))
}

fn task_answers_are_terminal(
    task_id: &str,
    run_config: &EvaluationRunConfig,
    answers: &[LiveArtifactRecord],
) -> bool {
    run_config
        .comparisons
        .iter()
        .filter(|comparison| comparison.task_ids.iter().any(|id| id == task_id))
        .all(|comparison| {
            answers.iter().any(|answer| {
                answer.arm == comparison.arm
                    && answer.task_id.as_deref() == Some(task_id)
                    && matches!(answer.stage.as_str(), "answer" | "answer_failure")
            })
        })
}

fn write_live_values_jsonl(path: &Path, values: &[serde_json::Value]) -> Result<(), EvalError> {
    let mut bytes = Vec::new();
    for value in values {
        bytes.extend(
            serde_json::to_vec(&redact_artifact_value(value))
                .map_err(|_| EvalError::ArtifactIo)?
                .as_slice(),
        );
        bytes.push(b'\n');
    }
    atomic_write(path, &bytes)
}

#[allow(clippy::too_many_arguments)]
fn persist_live_progress(
    root: &Path,
    observations: &[LiveArtifactRecord],
    relations: &[LiveArtifactRecord],
    cards: &[LiveArtifactRecord],
    packs: &[LiveArtifactRecord],
    answers: &[LiveArtifactRecord],
    usage_by_arm: &std::collections::BTreeMap<String, LiveUsageStats>,
    provider_requests: u32,
    completed_tasks: u32,
) -> Result<(), EvalError> {
    let cost_unknown = usage_by_arm.values().any(|stats| stats.cost_unknown);
    let cost_available = !usage_by_arm.is_empty()
        && usage_by_arm
            .values()
            .all(|stats| stats.cost_minor.is_some());
    let input_tokens_available = !usage_by_arm.is_empty()
        && usage_by_arm
            .values()
            .all(|stats| stats.input_tokens.is_some());
    let output_tokens_available = !usage_by_arm.is_empty()
        && usage_by_arm
            .values()
            .all(|stats| stats.output_tokens.is_some());
    let token_status = if provider_requests == 0 {
        "unavailable"
    } else if input_tokens_available && output_tokens_available {
        "known"
    } else {
        "unknown"
    };
    write_live_jsonl(&root.join("observations.jsonl"), observations)?;
    write_live_jsonl(&root.join("relations.jsonl"), relations)?;
    write_live_jsonl(&root.join("cards.jsonl"), cards)?;
    write_live_jsonl(&root.join("packs.jsonl"), packs)?;
    write_live_jsonl(&root.join("answers.jsonl"), answers)?;
    write_live_json(
        &root.join("usage.json"),
        &serde_json::json!({
            "schema_version": LIVE_RUN_SCHEMA,
            "request_count": provider_requests,
            "input_tokens": input_tokens_available.then(|| usage_by_arm.values().filter_map(|stats| stats.input_tokens).fold(0_u64, u64::saturating_add)),
            "output_tokens": output_tokens_available.then(|| usage_by_arm.values().filter_map(|stats| stats.output_tokens).fold(0_u64, u64::saturating_add)),
            "token_status": token_status,
            "cost_minor": if cost_unknown || !cost_available { serde_json::Value::Null } else { serde_json::json!(usage_by_arm.values().filter_map(|stats| stats.cost_minor).fold(0_u64, u64::saturating_add)) },
            "cost_status": if cost_unknown { "unknown" } else if cost_available { "known" } else { "unavailable" },
            "completed_tasks": completed_tasks,
            "by_arm": usage_by_arm,
            "status": "running",
        }),
    )
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), EvalError> {
    private_fs::atomic_write_private_file(path, bytes).map_err(|_| EvalError::ArtifactIo)
}

#[allow(clippy::too_many_arguments)]
fn persist_incremental_checkpoint(
    root: &Path,
    manifest_hash: &str,
    run_config_hash: &str,
    live_config_hash: &str,
    last_boundary: &str,
    provider_requests: u32,
    completed_tasks: u32,
    first_error: Option<LiveFailureRecord>,
) -> Result<(), EvalError> {
    let status = if first_error.is_some() {
        LiveRunStatus::Failed
    } else {
        LiveRunStatus::Running
    };
    write_live_json(
        &root.join("checkpoint.json"),
        &LiveCheckpoint {
            schema_version: LIVE_RUN_SCHEMA,
            manifest_hash: manifest_hash.to_owned(),
            run_config_hash: run_config_hash.to_owned(),
            live_config_hash: live_config_hash.to_owned(),
            last_boundary: last_boundary.to_owned(),
            provider_requests,
            completed_tasks,
            status,
            first_error,
        },
    )
}

fn persist_item_failures(root: &Path, failures: &[LiveItemFailureRecord]) -> Result<(), EvalError> {
    write_live_jsonl(&root.join("item-failures.jsonl"), failures)
}

#[allow(clippy::too_many_arguments)]
fn append_a80_answer_failure(
    item_failures: &mut Vec<LiveItemFailureRecord>,
    answers: &mut Vec<LiveArtifactRecord>,
    sequence: u32,
    arm: ComparisonArm,
    task_id: &str,
    stage: &str,
    code: &str,
    pack_hash: Option<String>,
) {
    let code = safe_code(code);
    item_failures.push(LiveItemFailureRecord {
        sequence,
        arm: arm.clone(),
        source_id: None,
        task_id: Some(task_id.to_owned()),
        stage: stage.to_owned(),
        code: code.clone(),
    });
    answers.push(LiveArtifactRecord {
        sequence,
        stage: "answer_failure".into(),
        arm,
        source_id: None,
        task_id: Some(task_id.to_owned()),
        pack_hash,
        output: serde_json::json!({"status":"failed","code":code}),
        usage: None,
    });
}

#[allow(clippy::too_many_arguments)]
fn write_live_artifacts(
    root: &Path,
    manifest: &EvaluationManifest,
    run_config: &EvaluationRunConfig,
    manifest_hash: &str,
    run_config_hash: &str,
    observations: &[LiveArtifactRecord],
    relations: &[LiveArtifactRecord],
    cards: &[LiveArtifactRecord],
    packs: &[LiveArtifactRecord],
    answers: &[LiveArtifactRecord],
    review: &[serde_json::Value],
    usage: &serde_json::Value,
    report: &str,
    checkpoint: &LiveCheckpoint,
    live_config: &LiveEvaluationConfig,
    live_config_hash: &str,
) -> Result<(), EvalError> {
    let manifest_value = serde_json::to_value(manifest).map_err(|_| EvalError::ArtifactIo)?;
    let run_config_value = serde_json::to_value(run_config).map_err(|_| EvalError::ArtifactIo)?;
    write_live_json(
        &root.join("manifest.json"),
        &redact_artifact_value(&manifest_value),
    )?;
    write_live_json(
        &root.join("run-config.json"),
        &redact_artifact_value(&run_config_value),
    )?;
    let mut semantic_arm_roots = serde_json::Map::new();
    for (arm, root_config) in [
        (ComparisonArm::B, &live_config.semantic_arm_roots.b),
        (ComparisonArm::C, &live_config.semantic_arm_roots.c),
    ] {
        let comparison = run_config
            .comparisons
            .iter()
            .find(|comparison| comparison.arm == arm)
            .ok_or(EvalError::LiveConfig("semantic observation arm is missing"))?;
        let mut allowlist = comparison.source_ids.clone();
        allowlist.sort();
        let allowlist_hash = canonical_hash(&allowlist).map_err(|_| EvalError::ArtifactIo)?;
        semantic_arm_roots.insert(
            format!("{arm:?}"),
            serde_json::json!({
                "vault_slug": root_config.vault_slug,
                "vault_id": root_config.vault_id,
                "source_root": root_config.source_root,
                "state_root": root_config.state_root,
                "state_database_leaf": "state.sqlite3",
                "history_root": root_config.history_root,
                "source_allowlist": allowlist,
                "source_allowlist_hash": allowlist_hash,
            }),
        );
    }
    let provider_templates = safe_provider_template_artifacts(&live_config.provider_templates)?;
    let live_config_value = serde_json::json!({
        "schema_version": live_config.schema_version,
        "run_root": live_config.run_root,
        "source_root": live_config.source_root,
        "state_root": live_config.state_root,
        "history_root": live_config.history_root,
        "artifact_root": live_config.artifact_root,
        "isolated_vault_id": live_config.isolated_vault_id,
        "current_schema": live_config.current_schema,
        "task_budget": live_config.task_budget,
        "provider_model_id": live_config.provider_model_id,
        "provider_external_model_id": live_config.run_config.comparisons[0].answer_model_id,
        "provider_runtime_fingerprint": live_config.provider_runtime_snapshot.as_ref().map(|snapshot| &snapshot.fingerprint),
        "provider_safe_settings": live_config.provider_runtime_snapshot.as_ref().map(|snapshot| &snapshot.settings),
        "provider_templates": provider_templates,
        "isolated_master_key_path": live_config.isolated_master_key_path,
        "request_budget": live_config.run_config.comparisons[0].budget.external_request_budget,
        "unbounded_cost_authorized": live_config.unbounded_cost_authorized,
        "source_allowlist": live_config.run_config.source_allowlist,
        "source_allowlist_hash": canonical_hash(&live_config.run_config.source_allowlist).map_err(|_| EvalError::ArtifactIo)?,
        "observation_allowlists": observation_allowlist_artifact(run_config)?,
        "semantic_arm_roots": semantic_arm_roots,
        "run_config_hash": run_config_hash,
        "live_config_hash": live_config_hash,
    });
    write_live_json(&root.join("live-config.json"), &live_config_value)?;
    write_live_jsonl(&root.join("observations.jsonl"), observations)?;
    write_live_jsonl(&root.join("relations.jsonl"), relations)?;
    write_live_jsonl(&root.join("cards.jsonl"), cards)?;
    write_live_jsonl(&root.join("packs.jsonl"), packs)?;
    write_live_jsonl(&root.join("answers.jsonl"), answers)?;
    write_live_values_jsonl(&root.join("review.jsonl"), review)?;
    write_live_json(&root.join("usage.json"), usage)?;
    atomic_write(&root.join("report.md"), report.as_bytes())?;
    write_live_json(&root.join("checkpoint.json"), checkpoint)?;
    let _ = manifest_hash;
    Ok(())
}

fn safe_provider_template_artifacts(
    templates: &[ProviderStageTemplate],
) -> Result<Vec<serde_json::Value>, EvalError> {
    let mut artifacts = templates
        .iter()
        .map(|template| {
            Ok(serde_json::json!({
                "stage": template.stage,
                "model_id": template.model_id,
                "prompt_id": template.prompt_id,
                "schema_id": template.schema_id,
                "index_profile_id": template.index_profile_id,
                "max_output_tokens": template.max_output_tokens,
                "temperature": template.temperature,
                "timeout_seconds": template.timeout_seconds,
                // Covers the complete template without persisting prompt/schema text.
                "template_fingerprint": canonical_hash(template)
                    .map_err(|_| EvalError::ArtifactIo)?,
            }))
        })
        .collect::<Result<Vec<_>, EvalError>>()?;
    artifacts.sort_by(|left, right| left["stage"].as_str().cmp(&right["stage"].as_str()));
    Ok(artifacts)
}

fn observation_allowlist_artifact(
    run_config: &EvaluationRunConfig,
) -> Result<serde_json::Value, EvalError> {
    let mut allowlists = serde_json::Map::new();
    for arm in [ComparisonArm::B, ComparisonArm::C] {
        let comparison = run_config
            .comparisons
            .iter()
            .find(|comparison| comparison.arm == arm)
            .ok_or(EvalError::LiveConfig("semantic observation arm is missing"))?;
        let source_ids = comparison
            .source_ids
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let source_ids_hash = canonical_hash(&source_ids).map_err(|_| EvalError::ArtifactIo)?;
        allowlists.insert(
            format!("{arm:?}"),
            serde_json::json!({
                "source_ids": source_ids,
                "source_ids_hash": source_ids_hash,
            }),
        );
    }
    Ok(serde_json::Value::Object(allowlists))
}

#[allow(clippy::too_many_arguments)]
async fn call_live_provider<P: ProviderAppBoundary>(
    provider: &P,
    provider_snapshot: Option<&ProviderRuntimeSnapshot>,
    run_config: &EvaluationRunConfig,
    request_budget: u32,
    budget_reserved: bool,
    request_counter: &AtomicU32,
    sequence: &mut u32,
    provider_requests: &mut u32,
    stage: &str,
    arm: ComparisonArm,
    source_id: Option<String>,
    task_id: Option<String>,
    input: serde_json::Value,
) -> Result<LiveProviderCall, LiveProviderCallError> {
    if let Some(expected) = provider_snapshot {
        provider
            .verify_frozen_configuration(expected)
            .await
            .map_err(|error| LiveProviderCallError {
                code: error.code,
                attempts: 0,
                schema_issue: error.schema_issue,
                schema_path: error.schema_path,
                structured_json_diagnostic: error.structured_json_diagnostic,
                protocol_issue: error.protocol_issue,
            })?;
    }
    if !budget_reserved {
        consume_live_request_budget(request_counter, request_budget).map_err(|_| {
            LiveProviderCallError {
                code: "request_budget_exhausted".into(),
                attempts: 0,
                schema_issue: None,
                schema_path: None,
                structured_json_diagnostic: None,
                protocol_issue: None,
            }
        })?;
    }
    *sequence = sequence.saturating_add(1);
    let arm_config = &run_config.comparisons[match arm {
        ComparisonArm::A => 0,
        ComparisonArm::B => 1,
        ComparisonArm::C => 2,
    }];
    let attempts_before = provider.transport_attempt_count();
    let output = provider
        .generate(LiveProviderRequest {
            sequence: *sequence,
            stage: stage.to_owned(),
            arm,
            source_id,
            task_id,
            model_id: arm_config.answer_model_id.clone(),
            prompt_id: arm_config.prompt_id.clone(),
            schema_id: arm_config.schema_id.clone(),
            index_profile_id: arm_config.index_profile_id.clone(),
            input,
        })
        .await;
    let attempts = match (attempts_before, provider.transport_attempt_count()) {
        (Some(before), Some(after)) => after.saturating_sub(before),
        _ => 1,
    };
    if !budget_reserved {
        *provider_requests = provider_requests.saturating_add(attempts);
    }
    match output {
        Ok(output) => Ok(LiveProviderCall { output, attempts }),
        Err(error) => Err(LiveProviderCallError {
            code: error.code,
            attempts,
            schema_issue: error.schema_issue,
            schema_path: error.schema_path,
            structured_json_diagnostic: error.structured_json_diagnostic,
            protocol_issue: error.protocol_issue,
        }),
    }
}

fn consume_live_request_budget(request_counter: &AtomicU32, request_budget: u32) -> Result<(), ()> {
    request_counter
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |current| {
            current
                .checked_add(1)
                .filter(|next| *next <= request_budget)
        })
        .map(|_| ())
        .map_err(|_| ())
}

fn is_a80_global_provider_failure(code: &str) -> bool {
    matches!(
        code,
        "provider_config_invalid"
            | "provider_not_found"
            | "model_not_found"
            | "model_disabled"
            | "model_capability_mismatch"
            | "provider_disabled"
            | "provider_privacy_denied"
            | "provider_runtime_configuration_drift"
            | "provider_endpoint_denied"
            | "provider_auth_failed"
            | "provider_secret_unavailable"
            | "provider_capability_unavailable"
            | "provider_request_budget_exhausted"
            | "request_budget_exhausted"
            | "provider_state_error"
            | "provider_url_invalid"
    )
}

async fn verify_live_source<V: SourceSnapshotVerifier>(
    source: &EvalSource,
    verifier: &V,
) -> Result<(), EvalError> {
    let actual = verifier.snapshot_current(source).await?;
    let actual_logical_block_count = actual
        .content
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.iter().all(u8::is_ascii_whitespace))
        .count();
    if !actual.authorized
        || actual.logical_id != source.logical_id
        || actual.vault_id != source.vault_id
        || actual.file_id != source.file_id
        || actual.path != source.path
        || actual.file_revision != source.file_revision
        || actual.source_revision_id != source.source_revision_id
        || actual.content_hash != source.content_hash
        || format!("{:x}", Sha256::digest(&actual.content)) != source.content_hash
        || actual.authorization_revision != source.authorization_revision
        || actual.profile_id != source.profile_id
        || actual.rules_revision != source.rules_revision
        || actual.source_generation != source.source_generation
        || actual.extraction_commit_sequence != source.extraction_commit_sequence
        || (source.logical_block_count > 0
            && usize::try_from(source.logical_block_count).ok() != Some(actual_logical_block_count))
    {
        return Err(EvalError::SourceMismatch);
    }
    Ok(())
}

async fn verify_task_sources<V: SourceSnapshotVerifier>(
    task: &EvalTask,
    arm: &ComparisonConfig,
    manifest: &EvaluationManifest,
    verifier: &V,
) -> Result<(), EvalError> {
    for source_id in task.source_ids.iter().filter(|id| {
        // Arm A is the ordinary retrieval baseline and must fence every
        // required source. B/C remain limited to their explicit arm allowlist.
        matches!(arm.arm, ComparisonArm::A) || arm.source_ids.contains(id)
    }) {
        let source = manifest
            .sources
            .iter()
            .find(|source| &source.logical_id == source_id)
            .ok_or(EvalError::SourceMismatch)?;
        verify_live_source(source, verifier).await?;
    }
    Ok(())
}

/// Execute one explicitly authorized live run.  All source fences and output
/// roots are checked before the first Provider call.  Provider or semantic
/// errors are persisted as a redacted checkpoint and stop the run; no retry or
/// fake success is attempted.  The report deliberately remains pending for
/// human semantic/task review.
fn ordinary_retrieval_review_record(
    task: &EvalTask,
    input: &serde_json::Value,
) -> serde_json::Value {
    serde_json::json!({
        "kind": "ordinary_lexical_coverage",
        "arm": "A",
        "task_id": task.id,
        "coverage": input.get("coverage").cloned().unwrap_or(serde_json::Value::Null),
        "degradation_reasons": input
            .get("degradation_reasons")
            .cloned()
            .unwrap_or_else(|| serde_json::json!(["ordinary_retrieval_contract_missing"])),
        "quality_events": input
            .get("quality_events")
            .cloned()
            .unwrap_or_else(|| serde_json::json!([])),
    })
}

fn ordinary_retrieval_failure(
    task: &EvalTask,
    sources: &[EvalSource],
    input: &serde_json::Value,
) -> Option<&'static str> {
    if input
        .get("retrieval_strategy")
        .and_then(|value| value.as_str())
        != Some("ordinary_note_v1")
        || input.get("task_id").and_then(|value| value.as_str()) != Some(task.id.as_str())
    {
        return Some("ordinary_retrieval_contract_invalid");
    }
    let Some(coverage) = input.get("coverage") else {
        return Some("ordinary_retrieval_contract_invalid");
    };
    let expected_source_ids = sources
        .iter()
        .map(|source| source.logical_id.clone())
        .collect::<BTreeSet<_>>();
    let source_id_set = |value: Option<&serde_json::Value>| {
        value.and_then(serde_json::Value::as_array).map(|items| {
            items
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::to_owned)
                .collect::<BTreeSet<_>>()
        })
    };
    let Some(expected_ids) = source_id_set(coverage.get("expected_source_ids")) else {
        return Some("ordinary_retrieval_contract_invalid");
    };
    let Some(indexed_ids) = source_id_set(coverage.get("current_indexed_source_ids")) else {
        return Some("ordinary_retrieval_contract_invalid");
    };
    let expected_count = u64::try_from(expected_source_ids.len()).unwrap_or(u64::MAX);
    if expected_ids != expected_source_ids
        || indexed_ids != expected_source_ids
        || coverage.get("complete").and_then(|value| value.as_bool()) != Some(true)
        || coverage
            .get("expected_source_count")
            .and_then(|value| value.as_u64())
            != Some(expected_count)
        || coverage
            .get("current_indexed_source_count")
            .and_then(|value| value.as_u64())
            != Some(expected_count)
        || coverage
            .get("coverage_ratio")
            .and_then(|value| value.as_f64())
            != Some(1.0)
    {
        return Some("ordinary_index_coverage_incomplete");
    }
    if coverage
        .get("stale_hit_count")
        .and_then(|value| value.as_u64())
        .unwrap_or(0)
        != 0
    {
        return Some("ordinary_index_stale_hit");
    }
    if input
        .get("degradation_reasons")
        .and_then(|value| value.as_array())
        .is_none_or(|reasons| !reasons.is_empty())
    {
        return Some("ordinary_retrieval_degraded");
    }
    None
}

pub async fn run_live_evaluation<V, P, S>(
    config: &LiveEvaluationConfig,
    verifier: &V,
    provider: &P,
    semantic: &S,
) -> Result<LiveEvaluationResult, EvalError>
where
    V: SourceSnapshotVerifier,
    P: ProviderAppBoundary,
    S: SemanticMemoryAppBoundary,
{
    let live_config_hash = canonical_hash(config)
        .map_err(|_| EvalError::LiveConfig("live config serialization failed"))?;
    let (manifest_hash, run_config_hash) = match validate_live_config(config) {
        Ok(hashes) => hashes,
        Err(error) => {
            // Safe preflight rejections (notably unsupported finite currency
            // caps and missing/duplicate templates) are still recorded when
            // their configured artifact root is a fresh isolated directory.
            let root = Path::new(&config.artifact_root);
            if validate_live_roots(config).is_ok()
                && root.is_absolute()
                && (!root.exists()
                    || fs::read_dir(root)
                        .ok()
                        .is_some_and(|mut entries| entries.next().is_none()))
                && private_fs::ensure_private_directory(root).is_ok()
                && validate_live_roots(config).is_ok()
            {
                let manifest_hash = canonical_hash(&config.manifest).unwrap_or_default();
                let run_config_hash = canonical_hash(&config.run_config).unwrap_or_default();
                let _ = persist_incremental_checkpoint(
                    root,
                    &manifest_hash,
                    &run_config_hash,
                    &live_config_hash,
                    "preflight",
                    0,
                    0,
                    Some(LiveFailureRecord {
                        sequence: 0,
                        stage: "preflight".into(),
                        code: safe_code(&error.to_string()),
                    }),
                );
            }
            return Err(error);
        }
    };
    let root = Path::new(&config.artifact_root);
    prepare_live_root_directories(config)?;
    let resume = inspect_live_resume(config, &manifest_hash, &run_config_hash, &live_config_hash)?;
    if root.exists()
        && fs::read_dir(root)
            .map_err(|_| EvalError::ArtifactIo)?
            .next()
            .is_some()
        && resume.is_none()
    {
        return Err(EvalError::LiveConfig("artifact root must be empty"));
    }
    private_fs::validate_private_directory(root)
        .map_err(|_| EvalError::LiveConfig("artifact root must be private mode 0700"))?;
    if resume.is_none() {
        persist_incremental_checkpoint(
            root,
            &manifest_hash,
            &run_config_hash,
            &live_config_hash,
            "preflight",
            0,
            0,
            None,
        )?;
    }
    if verify_live_source_snapshots(&config.manifest, verifier)
        .await
        .is_err()
    {
        persist_incremental_checkpoint(
            root,
            &manifest_hash,
            &run_config_hash,
            &live_config_hash,
            "preflight_source_fence",
            0,
            0,
            Some(LiveFailureRecord {
                sequence: 0,
                stage: "source_fence".into(),
                code: "source_mismatch".into(),
            }),
        )?;
        return Err(EvalError::SourceMismatch);
    }
    let source_ids = config
        .manifest
        .sources
        .iter()
        .map(|source| source.logical_id.as_str())
        .collect::<BTreeSet<_>>();
    let task_ids = config
        .manifest
        .tasks
        .iter()
        .filter(|task| config.run_config.comparisons[0].task_ids.contains(&task.id))
        .map(|task| task.id.clone())
        .collect::<BTreeSet<_>>();
    let observation_source_union = config
        .manifest
        .tasks
        .iter()
        .filter(|task| task_ids.contains(&task.id))
        .flat_map(|task| task.source_ids.iter().cloned())
        .collect::<BTreeSet<_>>();
    let mut observations = read_live_jsonl::<LiveArtifactRecord>(root, "observations.jsonl")?;
    let mut relations = read_live_jsonl::<LiveArtifactRecord>(root, "relations.jsonl")?;
    let mut cards = read_live_jsonl::<LiveArtifactRecord>(root, "cards.jsonl")?;
    let mut packs = read_live_jsonl::<LiveArtifactRecord>(root, "packs.jsonl")?;
    let mut persisted_packs =
        std::collections::BTreeMap::<String, (serde_json::Value, String)>::new();
    let mut pack_failures = std::collections::BTreeMap::<String, String>::new();
    for record in &packs {
        if record.stage == "pack"
            && record.arm == ComparisonArm::C
            && let (Some(task_id), Some(pack_hash)) =
                (record.task_id.as_ref(), record.pack_hash.as_ref())
        {
            persisted_packs.insert(task_id.clone(), (record.output.clone(), pack_hash.clone()));
        }
    }
    let mut answers = read_live_jsonl::<LiveArtifactRecord>(root, "answers.jsonl")?;
    let mut review = read_live_values_jsonl(root, "review.jsonl")?;
    let mut usage_by_arm = read_live_usage(root)?;
    let recovered_provider_requests = resume
        .as_ref()
        .map_or(0, |resume| resume.provider_requests_consumed)
        .max(provider.transport_attempt_count().unwrap_or_default());
    let mut provider_requests = recovered_provider_requests;
    let mut completed_tasks = u32::try_from(
        task_ids
            .iter()
            .filter(|task_id| task_answers_are_terminal(task_id, &config.run_config, &answers))
            .count(),
    )
    .map_err(|_| EvalError::LiveConfig("completed task count overflow"))?;
    let mut first_error: Option<LiveFailureRecord> = None;
    let mut sequence = resume
        .as_ref()
        .map_or(0, |resume| resume.last_attempt_sequence)
        .max(recovered_provider_requests);
    let request_counter = AtomicU32::new(recovered_provider_requests);
    let request_budget = config.run_config.comparisons[0]
        .budget
        .external_request_budget;
    let mut item_failures = read_live_jsonl::<LiveItemFailureRecord>(root, "item-failures.jsonl")?;
    let resume_attempts = resume
        .as_ref()
        .map(|resume| resume.attempts.clone())
        .unwrap_or_default();
    let observation_sources = config
        .manifest
        .sources
        .iter()
        .filter(|source| observation_source_union.contains(&source.logical_id))
        .cloned()
        .collect::<Vec<_>>();
    'run: for source in &observation_sources {
        for arm in [ComparisonArm::B, ComparisonArm::C] {
            let arm_index = match arm {
                ComparisonArm::A => 0,
                ComparisonArm::B => 1,
                ComparisonArm::C => 2,
            };
            if !config.run_config.comparisons[arm_index]
                .source_ids
                .contains(&source.logical_id)
                || !observation_source_union.contains(&source.logical_id)
            {
                continue;
            }
            if config.semantic_protocol == "m1-a80-v1"
                && cards.iter().any(|record| {
                    record.stage == "card"
                        && record.arm == arm
                        && record.source_id.as_deref() == Some(source.logical_id.as_str())
                })
            {
                continue;
            }
            if verify_live_source(source, verifier).await.is_err() {
                let _ = semantic.abort_semantic_for_arm(arm.clone(), source).await;
                first_error = Some(LiveFailureRecord {
                    sequence,
                    stage: "source_fence".into(),
                    code: "source_mismatch".into(),
                });
                break 'run;
            }
            if semantic
                .verify_arm_task_sources(
                    arm.clone(),
                    std::slice::from_ref(source),
                    "before_prepare",
                )
                .await
                .is_err()
            {
                let _ = semantic.abort_semantic_for_arm(arm.clone(), source).await;
                first_error = Some(LiveFailureRecord {
                    sequence,
                    stage: "arm_source_fence_before_prepare".into(),
                    code: "source_mismatch".into(),
                });
                break 'run;
            }
            if config.semantic_protocol == "m1-a80-v1" {
                let prepared = match semantic
                    .prepare_a80_for_arm(
                        arm.clone(),
                        source,
                        &live_config_hash,
                        M6_A80_PROMPT_ID,
                        M6_A80_SCHEMA_ID,
                    )
                    .await
                {
                    Ok(value) => value,
                    Err(code) => {
                        let _ = semantic.abort_semantic_for_arm(arm.clone(), source).await;
                        item_failures.push(LiveItemFailureRecord {
                            sequence,
                            arm: arm.clone(),
                            source_id: Some(source.logical_id.clone()),
                            task_id: None,
                            stage: "prepare_a80".into(),
                            code: safe_code(&code),
                        });
                        persist_item_failures(root, &item_failures)?;
                        continue;
                    }
                };
                let batches = match prepared
                    .get("batches")
                    .and_then(serde_json::Value::as_array)
                {
                    Some(batches) if !batches.is_empty() => batches.clone(),
                    _ => {
                        let _ = semantic.abort_semantic_for_arm(arm.clone(), source).await;
                        item_failures.push(LiveItemFailureRecord {
                            sequence,
                            arm: arm.clone(),
                            source_id: Some(source.logical_id.clone()),
                            task_id: None,
                            stage: "prepare_a80".into(),
                            code: "semantic_observation_batches_invalid".into(),
                        });
                        persist_item_failures(root, &item_failures)?;
                        continue;
                    }
                };
                let mut source_failed = false;
                'batches: for batch in &batches {
                    if batch.get("batch_state").and_then(serde_json::Value::as_str)
                        == Some("validated")
                    {
                        continue;
                    }
                    let Some(batch_index) = batch
                        .get("batch_index")
                        .and_then(serde_json::Value::as_u64)
                        .and_then(|index| u32::try_from(index).ok())
                    else {
                        let _ = semantic.abort_semantic_for_arm(arm.clone(), source).await;
                        item_failures.push(LiveItemFailureRecord {
                            sequence,
                            arm: arm.clone(),
                            source_id: Some(source.logical_id.clone()),
                            task_id: None,
                            stage: "prepare_a80".into(),
                            code: "semantic_observation_batches_invalid".into(),
                        });
                        persist_item_failures(root, &item_failures)?;
                        source_failed = true;
                        break 'batches;
                    };
                    let Some(input) = batch.get("input").cloned() else {
                        let _ = semantic.abort_semantic_for_arm(arm.clone(), source).await;
                        item_failures.push(LiveItemFailureRecord {
                            sequence,
                            arm: arm.clone(),
                            source_id: Some(source.logical_id.clone()),
                            task_id: None,
                            stage: "prepare_a80".into(),
                            code: "semantic_observation_batches_invalid".into(),
                        });
                        persist_item_failures(root, &item_failures)?;
                        source_failed = true;
                        break 'batches;
                    };
                    let batch_state = batch
                        .get("batch_state")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default();
                    let batch_already_failed = batch_state == "failed";
                    let has_prior_batch_attempt = resume_attempts.iter().any(|attempt| {
                        attempt.stage == "observation"
                            && attempt.arm == comparison_arm_wire_name(&arm)
                            && attempt.source_id.as_deref() == Some(&source.logical_id)
                            && attempt.batch_index == Some(batch_index)
                    });
                    if ((batch_state == "ready" || batch_state.is_empty())
                        && has_prior_batch_attempt)
                        || matches!(batch_state, "dispatching" | "uncertain")
                    {
                        if batch_state == "dispatching" {
                            let _ = semantic
                                .reserve_a80_batch_for_arm(arm.clone(), source, batch_index)
                                .await;
                        }
                        if !has_prior_batch_attempt && batch_state != "ready" {
                            let uncertain_sequence = sequence.saturating_add(1);
                            persist_provider_attempt(
                                root,
                                uncertain_sequence,
                                "observation",
                                &arm,
                                Some(&source.logical_id),
                                None,
                                Some(batch_index),
                                "uncertain",
                            )?;
                            sequence = uncertain_sequence;
                            provider_requests = provider_requests.saturating_add(1);
                            let _ = request_counter.fetch_update(
                                Ordering::SeqCst,
                                Ordering::SeqCst,
                                |current| current.checked_add(1),
                            );
                        }
                        let _ = semantic.abort_semantic_for_arm(arm.clone(), source).await;
                        item_failures.push(LiveItemFailureRecord {
                            sequence,
                            arm: arm.clone(),
                            source_id: Some(source.logical_id.clone()),
                            task_id: None,
                            stage: "observation_resume".into(),
                            code: "provider_attempt_outcome_uncertain".into(),
                        });
                        persist_item_failures(root, &item_failures)?;
                        source_failed = true;
                        break 'batches;
                    }
                    if verify_live_source(source, verifier).await.is_err()
                        || semantic
                            .verify_arm_task_sources(
                                arm.clone(),
                                std::slice::from_ref(source),
                                "before_a80_batch",
                            )
                            .await
                            .is_err()
                    {
                        let _ = semantic.abort_semantic_for_arm(arm.clone(), source).await;
                        first_error = Some(LiveFailureRecord {
                            sequence,
                            stage: "a80_batch_source_fence".into(),
                            code: "source_mismatch".into(),
                        });
                        break 'run;
                    }
                    let attempt_sequence = sequence.saturating_add(1);
                    if consume_live_request_budget(&request_counter, request_budget).is_err() {
                        let _ = semantic.abort_semantic_for_arm(arm.clone(), source).await;
                        first_error = Some(LiveFailureRecord {
                            sequence,
                            stage: "a80_request_budget".into(),
                            code: "request_budget_exhausted".into(),
                        });
                        break 'run;
                    }
                    provider_requests = provider_requests.saturating_add(1);
                    persist_provider_attempt(
                        root,
                        attempt_sequence,
                        "observation",
                        &arm,
                        Some(&source.logical_id),
                        None,
                        Some(batch_index),
                        "reserved",
                    )?;
                    let reservation = if batch_already_failed {
                        semantic
                            .reserve_a80_regen_for_arm(arm.clone(), source, batch_index)
                            .await
                    } else {
                        semantic
                            .reserve_a80_batch_for_arm(arm.clone(), source, batch_index)
                            .await
                    };
                    match reservation {
                        Ok(state) if state == "dispatching" => {
                            mark_provider_attempt_started(root, attempt_sequence)?;
                        }
                        Ok(_) => {
                            sequence = attempt_sequence;
                            let _ = semantic.abort_semantic_for_arm(arm.clone(), source).await;
                            item_failures.push(LiveItemFailureRecord {
                                sequence,
                                arm: arm.clone(),
                                source_id: Some(source.logical_id.clone()),
                                task_id: None,
                                stage: "reserve_a80_batch".into(),
                                code: "semantic_batch_resume_requires_recovery".into(),
                            });
                            persist_item_failures(root, &item_failures)?;
                            source_failed = true;
                            break 'batches;
                        }
                        Err(code) => {
                            sequence = attempt_sequence;
                            let _ = semantic.abort_semantic_for_arm(arm.clone(), source).await;
                            item_failures.push(LiveItemFailureRecord {
                                sequence,
                                arm: arm.clone(),
                                source_id: Some(source.logical_id.clone()),
                                task_id: None,
                                stage: "reserve_a80_batch".into(),
                                code: safe_code(&code),
                            });
                            persist_item_failures(root, &item_failures)?;
                            source_failed = true;
                            break 'batches;
                        }
                    }
                    let mut normalized_counts = None;
                    for generation_attempt in 0..2 {
                        if generation_attempt == 1 && batch_already_failed {
                            break;
                        }
                        let result = call_live_provider(
                            provider,
                            config.provider_runtime_snapshot.as_ref(),
                            &config.run_config,
                            request_budget,
                            true,
                            &request_counter,
                            &mut sequence,
                            &mut provider_requests,
                            "observation",
                            arm.clone(),
                            Some(source.logical_id.clone()),
                            None,
                            input.clone(),
                        )
                        .await;
                        let result = match result {
                            Ok(result) => result,
                            Err(error) => {
                                record_arm_usage(
                                    &mut usage_by_arm,
                                    &arm,
                                    None,
                                    None,
                                    error.attempts,
                                );
                                persist_schema_diagnostic(
                                    root,
                                    sequence,
                                    "observation",
                                    &arm,
                                    Some(&source.logical_id),
                                    None,
                                    &error,
                                )?;
                                if verify_live_source(source, verifier).await.is_err()
                                    || semantic
                                        .verify_arm_task_sources(
                                            arm.clone(),
                                            std::slice::from_ref(source),
                                            "after_a80_provider_error",
                                        )
                                        .await
                                        .is_err()
                                {
                                    first_error = Some(LiveFailureRecord {
                                        sequence,
                                        stage: "a80_provider_error_source_fence".into(),
                                        code: "source_mismatch".into(),
                                    });
                                    break 'run;
                                }
                                if is_a80_global_provider_failure(&error.code) {
                                    let _ =
                                        semantic.abort_semantic_for_arm(arm.clone(), source).await;
                                    first_error = Some(LiveFailureRecord {
                                        sequence,
                                        stage: "provider_global_fence".into(),
                                        code: safe_code(&error.code),
                                    });
                                    break 'run;
                                }
                                let recoverable = !batch_already_failed
                                    && generation_attempt == 0
                                    && matches!(
                                        error.code.as_str(),
                                        "provider_schema_invalid"
                                            | "provider_structured_json_invalid"
                                    );
                                if recoverable {
                                    let retry_sequence = sequence.saturating_add(1);
                                    if consume_live_request_budget(&request_counter, request_budget)
                                        .is_err()
                                    {
                                        let _ = semantic
                                            .abort_semantic_for_arm(arm.clone(), source)
                                            .await;
                                        first_error = Some(LiveFailureRecord {
                                            sequence,
                                            stage: "a80_request_budget".into(),
                                            code: "request_budget_exhausted".into(),
                                        });
                                        break 'run;
                                    }
                                    provider_requests = provider_requests.saturating_add(1);
                                    persist_provider_attempt(
                                        root,
                                        retry_sequence,
                                        "observation",
                                        &arm,
                                        Some(&source.logical_id),
                                        None,
                                        Some(batch_index),
                                        "reserved",
                                    )?;
                                    let marked_failed = semantic
                                        .fail_a80_batch_for_arm(
                                            arm.clone(),
                                            source,
                                            batch_index,
                                            &safe_code(&error.code),
                                        )
                                        .await
                                        .is_ok();
                                    let retry_reserved = marked_failed
                                        && semantic
                                            .reserve_a80_regen_for_arm(
                                                arm.clone(),
                                                source,
                                                batch_index,
                                            )
                                            .await
                                            .is_ok();
                                    if retry_reserved {
                                        mark_provider_attempt_started(root, retry_sequence)?;
                                        continue;
                                    }
                                    sequence = retry_sequence;
                                }
                                let _ = semantic.abort_semantic_for_arm(arm.clone(), source).await;
                                item_failures.push(LiveItemFailureRecord {
                                    sequence,
                                    arm: arm.clone(),
                                    source_id: Some(source.logical_id.clone()),
                                    task_id: None,
                                    stage: "observation".into(),
                                    code: safe_code(&error.code),
                                });
                                persist_item_failures(root, &item_failures)?;
                                source_failed = true;
                                break;
                            }
                        };
                        record_arm_usage(
                            &mut usage_by_arm,
                            &arm,
                            result.output.usage.as_ref(),
                            result.output.cost_minor,
                            result.attempts,
                        );
                        observations.push(LiveArtifactRecord {
                            sequence,
                            stage: "observation_batch".into(),
                            arm: arm.clone(),
                            source_id: Some(source.logical_id.clone()),
                            task_id: None,
                            pack_hash: None,
                            output: project_provider_output("observation", &result.output.output),
                            usage: project_usage(result.output.usage.as_ref()),
                        });
                        match semantic
                            .accept_a80_batch_for_arm(
                                arm.clone(),
                                source,
                                batch_index,
                                &result.output.output,
                            )
                            .await
                        {
                            Ok(counts) => {
                                normalized_counts = Some(counts);
                                break;
                            }
                            Err(code) => {
                                if verify_live_source(source, verifier).await.is_err()
                                    || semantic
                                        .verify_arm_task_sources(
                                            arm.clone(),
                                            std::slice::from_ref(source),
                                            "after_a80_validation_error",
                                        )
                                        .await
                                        .is_err()
                                {
                                    first_error = Some(LiveFailureRecord {
                                        sequence,
                                        stage: "a80_validation_error_source_fence".into(),
                                        code: "source_mismatch".into(),
                                    });
                                    break 'run;
                                }
                                let recoverable_core = code.starts_with("semantic_flat_");
                                if !batch_already_failed
                                    && generation_attempt == 0
                                    && recoverable_core
                                {
                                    let retry_sequence = sequence.saturating_add(1);
                                    if consume_live_request_budget(&request_counter, request_budget)
                                        .is_err()
                                    {
                                        let _ = semantic
                                            .abort_semantic_for_arm(arm.clone(), source)
                                            .await;
                                        first_error = Some(LiveFailureRecord {
                                            sequence,
                                            stage: "a80_request_budget".into(),
                                            code: "request_budget_exhausted".into(),
                                        });
                                        break 'run;
                                    }
                                    provider_requests = provider_requests.saturating_add(1);
                                    persist_provider_attempt(
                                        root,
                                        retry_sequence,
                                        "observation",
                                        &arm,
                                        Some(&source.logical_id),
                                        None,
                                        Some(batch_index),
                                        "reserved",
                                    )?;
                                    let marked_failed = semantic
                                        .fail_a80_batch_for_arm(
                                            arm.clone(),
                                            source,
                                            batch_index,
                                            &safe_code(&code),
                                        )
                                        .await
                                        .is_ok();
                                    let retry_reserved = marked_failed
                                        && semantic
                                            .reserve_a80_regen_for_arm(
                                                arm.clone(),
                                                source,
                                                batch_index,
                                            )
                                            .await
                                            .is_ok();
                                    if retry_reserved {
                                        mark_provider_attempt_started(root, retry_sequence)?;
                                        continue;
                                    }
                                    sequence = retry_sequence;
                                }
                                let _ = semantic.abort_semantic_for_arm(arm.clone(), source).await;
                                item_failures.push(LiveItemFailureRecord {
                                    sequence,
                                    arm: arm.clone(),
                                    source_id: Some(source.logical_id.clone()),
                                    task_id: None,
                                    stage: "accept_observation_batch".into(),
                                    code: safe_code(&code),
                                });
                                persist_item_failures(root, &item_failures)?;
                                source_failed = true;
                                break;
                            }
                        }
                    }
                    if source_failed {
                        break 'batches;
                    }
                    let Some(counts) = normalized_counts else {
                        let _ = semantic.abort_semantic_for_arm(arm.clone(), source).await;
                        item_failures.push(LiveItemFailureRecord {
                            sequence,
                            arm: arm.clone(),
                            source_id: Some(source.logical_id.clone()),
                            task_id: None,
                            stage: "observation_regeneration".into(),
                            code: "semantic_regen_unavailable".into(),
                        });
                        persist_item_failures(root, &item_failures)?;
                        source_failed = true;
                        break 'batches;
                    };
                    observations.push(LiveArtifactRecord {
                        sequence,
                        stage: "observation_batch_normalization".into(),
                        arm: arm.clone(),
                        source_id: Some(source.logical_id.clone()),
                        task_id: None,
                        pack_hash: None,
                        output: counts,
                        usage: None,
                    });
                    persist_live_progress(
                        root,
                        &observations,
                        &relations,
                        &cards,
                        &packs,
                        &answers,
                        &usage_by_arm,
                        provider_requests,
                        completed_tasks,
                    )?;
                    if let Some(expected) = &config.provider_runtime_snapshot
                        && provider
                            .verify_frozen_configuration(expected)
                            .await
                            .is_err()
                    {
                        first_error = Some(LiveFailureRecord {
                            sequence,
                            stage: "provider_config_after_a80_batch".into(),
                            code: "provider_runtime_configuration_drift".into(),
                        });
                        break 'run;
                    }
                }
                if source_failed {
                    continue;
                }
                if let Err(code) = semantic.finalize_a80_for_arm(arm.clone(), source).await {
                    first_error = Some(LiveFailureRecord {
                        sequence,
                        stage: "finalize_a80".into(),
                        code: safe_code(&code),
                    });
                    break 'run;
                }
                if semantic
                    .verify_arm_task_sources(
                        arm.clone(),
                        std::slice::from_ref(source),
                        "after_a80_publish",
                    )
                    .await
                    .is_err()
                    || verify_live_source(source, verifier).await.is_err()
                {
                    first_error = Some(LiveFailureRecord {
                        sequence,
                        stage: "a80_publish_source_fence".into(),
                        code: "source_mismatch".into(),
                    });
                    break 'run;
                }
                persist_live_progress(
                    root,
                    &observations,
                    &relations,
                    &cards,
                    &packs,
                    &answers,
                    &usage_by_arm,
                    provider_requests,
                    completed_tasks,
                )?;
                let current_card = match semantic
                    .current_card_projection_for_arm(arm.clone(), source)
                    .await
                {
                    Ok(value) => value,
                    Err(code) => {
                        first_error = Some(LiveFailureRecord {
                            sequence,
                            stage: "card_read".into(),
                            code: safe_code(&code),
                        });
                        break 'run;
                    }
                };
                cards.push(LiveArtifactRecord {
                    sequence,
                    stage: "card".into(),
                    arm: arm.clone(),
                    source_id: Some(source.logical_id.clone()),
                    task_id: None,
                    pack_hash: None,
                    output: card_artifact_projection(&current_card, source),
                    usage: None,
                });
                persist_live_progress(
                    root,
                    &observations,
                    &relations,
                    &cards,
                    &packs,
                    &answers,
                    &usage_by_arm,
                    provider_requests,
                    completed_tasks,
                )?;
                continue;
            }
            let new_protocol = config.semantic_protocol == "m1-two-stage-v2";
            let prepared = match if new_protocol {
                semantic.prepare_for_arm(arm.clone(), source).await
            } else {
                semantic.prepare_legacy_for_arm(arm.clone(), source).await
            } {
                Ok(value) => value,
                Err(code) => {
                    let _ = semantic.abort_semantic_for_arm(arm.clone(), source).await;
                    first_error = Some(LiveFailureRecord {
                        sequence,
                        stage: "prepare".into(),
                        code: safe_code(&code),
                    });
                    break 'run;
                }
            };
            if verify_live_source(source, verifier).await.is_err() {
                let _ = semantic.abort_semantic_for_arm(arm.clone(), source).await;
                first_error = Some(LiveFailureRecord {
                    sequence,
                    stage: "source_fence_after_prepare".into(),
                    code: "source_mismatch".into(),
                });
                break 'run;
            }
            if semantic
                .verify_arm_task_sources(arm.clone(), std::slice::from_ref(source), "after_prepare")
                .await
                .is_err()
            {
                let _ = semantic.abort_semantic_for_arm(arm.clone(), source).await;
                first_error = Some(LiveFailureRecord {
                    sequence,
                    stage: "arm_source_fence_after_prepare".into(),
                    code: "source_mismatch".into(),
                });
                break 'run;
            }
            persist_provider_attempt(
                root,
                sequence.saturating_add(1),
                "observation",
                &arm,
                Some(&source.logical_id),
                None,
                None,
                "started",
            )?;
            let result = match call_live_provider(
                provider,
                config.provider_runtime_snapshot.as_ref(),
                &config.run_config,
                request_budget,
                false,
                &request_counter,
                &mut sequence,
                &mut provider_requests,
                "observation",
                arm.clone(),
                Some(source.logical_id.clone()),
                None,
                prepared.clone(),
            )
            .await
            {
                Ok(result) => result,
                Err(error) => {
                    // A Provider/schema failure occurs after prepare has created
                    // a running extraction. End that exact prepared operation
                    // before recording the original first error.
                    let _ = semantic.abort_semantic_for_arm(arm.clone(), source).await;
                    record_arm_usage(&mut usage_by_arm, &arm, None, None, error.attempts);
                    persist_schema_diagnostic(
                        root,
                        sequence,
                        "observation",
                        &arm,
                        Some(&source.logical_id),
                        None,
                        &error,
                    )?;
                    persist_live_progress(
                        root,
                        &observations,
                        &relations,
                        &cards,
                        &packs,
                        &answers,
                        &usage_by_arm,
                        provider_requests,
                        completed_tasks,
                    )?;
                    first_error = Some(LiveFailureRecord {
                        sequence,
                        stage: "observation".into(),
                        code: safe_code(&error.code),
                    });
                    break 'run;
                }
            };
            let usage = result.output.usage.clone();
            record_arm_usage(
                &mut usage_by_arm,
                &arm,
                usage.as_ref(),
                result.output.cost_minor,
                result.attempts,
            );
            observations.push(LiveArtifactRecord {
                sequence,
                stage: "observation".into(),
                arm: arm.clone(),
                source_id: Some(source.logical_id.clone()),
                task_id: None,
                pack_hash: None,
                output: project_provider_output("observation", &result.output.output),
                usage: project_usage(usage.as_ref()),
            });
            persist_live_progress(
                root,
                &observations,
                &relations,
                &cards,
                &packs,
                &answers,
                &usage_by_arm,
                provider_requests,
                completed_tasks,
            )?;
            if let Some(expected) = &config.provider_runtime_snapshot
                && provider
                    .verify_frozen_configuration(expected)
                    .await
                    .is_err()
            {
                first_error = Some(LiveFailureRecord {
                    sequence,
                    stage: "provider_config_after_observation".into(),
                    code: "provider_runtime_configuration_drift".into(),
                });
                break 'run;
            }
            if semantic
                .verify_arm_task_sources(
                    arm.clone(),
                    std::slice::from_ref(source),
                    "after_provider",
                )
                .await
                .is_err()
            {
                first_error = Some(LiveFailureRecord {
                    sequence,
                    stage: "arm_source_fence_after_provider".into(),
                    code: "source_mismatch".into(),
                });
                break 'run;
            }
            if verify_live_source(source, verifier).await.is_err() {
                first_error = Some(LiveFailureRecord {
                    sequence,
                    stage: "source_fence_after_provider".into(),
                    code: "source_mismatch".into(),
                });
                break 'run;
            }
            let two_stage_protocol = new_protocol;
            if !two_stage_protocol
                && let Err(code) = semantic
                    .submit_observation_for_arm(arm.clone(), source, &result.output.output)
                    .await
            {
                first_error = Some(LiveFailureRecord {
                    sequence,
                    stage: "submit_observation".into(),
                    code: safe_code(&code),
                });
                break 'run;
            }
            let composition_input = if two_stage_protocol {
                let accepted = semantic
                    .accept_observation_for_arm(arm.clone(), source, &result.output.output)
                    .await;
                let _overlap_removed = semantic
                    .take_observation_context_overlap_removed_for_arm(arm.clone(), source)
                    .await;
                match accepted {
                    Ok(value) => value,
                    Err(code) => {
                        let _ = semantic.abort_semantic_for_arm(arm.clone(), source).await;
                        first_error = Some(LiveFailureRecord {
                            sequence,
                            stage: "accept_observation".into(),
                            code: safe_code(&code),
                        });
                        break 'run;
                    }
                }
            } else {
                serde_json::json!({"status":"legacy_completed"})
            };
            if composition_input
                .get("status")
                .and_then(serde_json::Value::as_str)
                == Some("composition_required")
            {
                persist_provider_attempt(
                    root,
                    sequence.saturating_add(1),
                    "composition",
                    &arm,
                    Some(&source.logical_id),
                    None,
                    None,
                    "started",
                )?;
                let composition = match call_live_provider(
                    provider,
                    config.provider_runtime_snapshot.as_ref(),
                    &config.run_config,
                    request_budget,
                    false,
                    &request_counter,
                    &mut sequence,
                    &mut provider_requests,
                    "composition",
                    arm.clone(),
                    Some(source.logical_id.clone()),
                    None,
                    composition_input.clone(),
                )
                .await
                {
                    Ok(result) => result,
                    Err(error) => {
                        let _ = semantic.abort_semantic_for_arm(arm.clone(), source).await;
                        record_arm_usage(&mut usage_by_arm, &arm, None, None, error.attempts);
                        persist_schema_diagnostic(
                            root,
                            sequence,
                            "composition",
                            &arm,
                            Some(&source.logical_id),
                            None,
                            &error,
                        )?;
                        first_error = Some(LiveFailureRecord {
                            sequence,
                            stage: "composition".into(),
                            code: safe_code(&error.code),
                        });
                        break 'run;
                    }
                };
                record_arm_usage(
                    &mut usage_by_arm,
                    &arm,
                    composition.output.usage.as_ref(),
                    composition.output.cost_minor,
                    composition.attempts,
                );
                observations.push(LiveArtifactRecord {
                    sequence,
                    stage: "composition".into(),
                    arm: arm.clone(),
                    source_id: Some(source.logical_id.clone()),
                    task_id: None,
                    pack_hash: None,
                    output: project_provider_output("composition", &composition.output.output),
                    usage: project_usage(composition.output.usage.as_ref()),
                });
                if let Err(code) = semantic
                    .submit_composition_for_arm(arm.clone(), source, &composition.output.output)
                    .await
                {
                    let _ = semantic.abort_semantic_for_arm(arm.clone(), source).await;
                    first_error = Some(LiveFailureRecord {
                        sequence,
                        stage: "submit_composition".into(),
                        code: safe_code(&code),
                    });
                    break 'run;
                }
            }
            if semantic
                .verify_arm_task_sources(arm.clone(), std::slice::from_ref(source), "after_submit")
                .await
                .is_err()
            {
                first_error = Some(LiveFailureRecord {
                    sequence,
                    stage: "arm_source_fence_after_submit".into(),
                    code: "source_mismatch".into(),
                });
                break 'run;
            }
            if verify_live_source(source, verifier).await.is_err() {
                first_error = Some(LiveFailureRecord {
                    sequence,
                    stage: "source_fence_after_submit".into(),
                    code: "source_mismatch".into(),
                });
                break 'run;
            }
            persist_live_progress(
                root,
                &observations,
                &relations,
                &cards,
                &packs,
                &answers,
                &usage_by_arm,
                provider_requests,
                completed_tasks,
            )?;
            persist_incremental_checkpoint(
                root,
                &manifest_hash,
                &run_config_hash,
                &live_config_hash,
                "observation",
                provider_requests,
                completed_tasks,
                first_error.clone(),
            )?;
            let current_card = match semantic
                .current_card_projection_for_arm(arm.clone(), source)
                .await
            {
                Ok(value) => value,
                Err(code) => {
                    first_error = Some(LiveFailureRecord {
                        sequence,
                        stage: "card_read".into(),
                        code: safe_code(&code),
                    });
                    break 'run;
                }
            };
            if verify_live_source(source, verifier).await.is_err() {
                first_error = Some(LiveFailureRecord {
                    sequence,
                    stage: "source_fence_after_card".into(),
                    code: "source_mismatch".into(),
                });
                break 'run;
            }
            cards.push(LiveArtifactRecord {
                sequence,
                stage: "card".into(),
                arm: arm.clone(),
                source_id: Some(source.logical_id.clone()),
                task_id: None,
                pack_hash: None,
                output: card_artifact_projection(&current_card, source),
                usage: None,
            });
            persist_live_progress(
                root,
                &observations,
                &relations,
                &cards,
                &packs,
                &answers,
                &usage_by_arm,
                provider_requests,
                completed_tasks,
            )?;
        }
    }
    if first_error.is_none() {
        'tasks: for task in &config.manifest.tasks {
            if !task_ids.contains(&task.id) {
                continue;
            }
            if task_answers_are_terminal(&task.id, &config.run_config, &answers) {
                continue;
            }
            let source_paths = task
                .source_ids
                .iter()
                .filter(|source_id| {
                    config.run_config.comparisons[2]
                        .source_ids
                        .contains(source_id)
                })
                .filter_map(|source_id| {
                    config
                        .manifest
                        .sources
                        .iter()
                        .find(|source| &source.logical_id == source_id)
                        .map(|source| source.path.clone())
                })
                .collect::<Vec<_>>();
            let c_config = &config.run_config.comparisons[2];
            if c_config.task_ids.contains(&task.id)
                && verify_task_sources(task, c_config, &config.manifest, verifier)
                    .await
                    .is_err()
            {
                first_error = Some(LiveFailureRecord {
                    sequence,
                    stage: "task_source_fence".into(),
                    code: "source_mismatch".into(),
                });
                break 'tasks;
            }
            if !c_config.task_ids.contains(&task.id) {
                continue;
            }
            if source_paths.is_empty() {
                first_error = Some(LiveFailureRecord {
                    sequence,
                    stage: "source_selection".into(),
                    code: "arm_task_has_no_selected_source".into(),
                });
                break;
            }
            let c_sources = task
                .source_ids
                .iter()
                .filter(|source_id| c_config.source_ids.contains(source_id))
                .filter_map(|source_id| {
                    config
                        .manifest
                        .sources
                        .iter()
                        .find(|source| &source.logical_id == source_id)
                        .cloned()
                })
                .collect::<Vec<_>>();
            let relation_artifact_exists = relations.iter().any(|record| {
                record.arm == ComparisonArm::C
                    && record.task_id.as_deref() == Some(task.id.as_str())
                    && matches!(record.stage.as_str(), "relation" | "relation_failure")
            });
            let relation_attempt_unresolved = resume_attempts.iter().any(|attempt| {
                attempt.stage == "relation"
                    && attempt.arm == "c"
                    && attempt.task_id.as_deref() == Some(task.id.as_str())
            }) && !relation_artifact_exists;
            if relation_attempt_unresolved {
                item_failures.push(LiveItemFailureRecord {
                    sequence,
                    arm: ComparisonArm::C,
                    source_id: None,
                    task_id: Some(task.id.clone()),
                    stage: "relation_resume".into(),
                    code: "provider_attempt_outcome_uncertain".into(),
                });
                persist_item_failures(root, &item_failures)?;
                relations.push(LiveArtifactRecord {
                    sequence,
                    stage: "relation_failure".into(),
                    arm: ComparisonArm::C,
                    source_id: None,
                    task_id: Some(task.id.clone()),
                    pack_hash: None,
                    output: serde_json::json!({"status":"not_replayed","code":"provider_attempt_outcome_uncertain"}),
                    usage: None,
                });
                persist_live_progress(
                    root,
                    &observations,
                    &relations,
                    &cards,
                    &packs,
                    &answers,
                    &usage_by_arm,
                    provider_requests,
                    completed_tasks,
                )?;
            }
            let mut prepared_relation_missing =
                relation_artifact_exists || relation_attempt_unresolved;
            if !relation_artifact_exists && !relation_attempt_unresolved {
                if semantic
                    .verify_arm_task_sources(ComparisonArm::C, &c_sources, "before_relation")
                    .await
                    .is_err()
                {
                    first_error = Some(LiveFailureRecord {
                        sequence,
                        stage: "arm_source_fence_before_relation".into(),
                        code: "source_mismatch".into(),
                    });
                    break 'tasks;
                }
                let prepared_relation = match semantic
                    .prepare_relation_input_for_arm(ComparisonArm::C, task, &c_sources)
                    .await
                {
                    Ok(value) => value,
                    Err(code) => {
                        if config.semantic_protocol == "m1-a80-v1" {
                            item_failures.push(LiveItemFailureRecord {
                                sequence,
                                arm: ComparisonArm::C,
                                source_id: None,
                                task_id: Some(task.id.clone()),
                                stage: "relation_candidates".into(),
                                code: safe_code(&code),
                            });
                            persist_item_failures(root, &item_failures)?;
                            relations.push(LiveArtifactRecord {
                            sequence,
                            stage: "relation_failure".into(),
                            arm: ComparisonArm::C,
                            source_id: None,
                            task_id: Some(task.id.clone()),
                            pack_hash: None,
                            output: serde_json::json!({"status":"failed","code":safe_code(&code)}),
                            usage: None,
                        });
                            persist_live_progress(
                                root,
                                &observations,
                                &relations,
                                &cards,
                                &packs,
                                &answers,
                                &usage_by_arm,
                                provider_requests,
                                completed_tasks,
                            )?;
                            None
                        } else {
                            first_error = Some(LiveFailureRecord {
                                sequence,
                                stage: "relation_candidates".into(),
                                code: safe_code(&code),
                            });
                            break 'tasks;
                        }
                    }
                };
                prepared_relation_missing = prepared_relation.is_none();
                if let Some(prepared) = prepared_relation.as_ref() {
                    let Some(dispatch) = prepared
                        .get("dispatch_required")
                        .and_then(serde_json::Value::as_bool)
                    else {
                        first_error = Some(LiveFailureRecord {
                            sequence,
                            stage: "relation_candidates".into(),
                            code: "relation_candidate_contract_invalid".into(),
                        });
                        break 'tasks;
                    };
                    let candidate_count = prepared
                        .get("candidate_count")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or_default();
                    if dispatch {
                        let relation_input = serde_json::json!({
                            "candidate_count": candidate_count,
                            "candidates": prepared.get("candidates").cloned().unwrap_or_else(|| serde_json::json!([])),
                        });
                        persist_provider_attempt(
                            root,
                            sequence.saturating_add(1),
                            "relation",
                            &ComparisonArm::C,
                            None,
                            Some(&task.id),
                            None,
                            "started",
                        )?;
                        let relation = match call_live_provider(
                            provider,
                            config.provider_runtime_snapshot.as_ref(),
                            &config.run_config,
                            request_budget,
                            false,
                            &request_counter,
                            &mut sequence,
                            &mut provider_requests,
                            "relation",
                            ComparisonArm::C,
                            None,
                            Some(task.id.clone()),
                            relation_input,
                        )
                        .await
                        {
                            Ok(result) => Some(result),
                            Err(error) => {
                                record_arm_usage(
                                    &mut usage_by_arm,
                                    &ComparisonArm::C,
                                    None,
                                    None,
                                    error.attempts,
                                );
                                persist_schema_diagnostic(
                                    root,
                                    sequence,
                                    "relation",
                                    &ComparisonArm::C,
                                    None,
                                    Some(&task.id),
                                    &error,
                                )?;
                                persist_live_progress(
                                    root,
                                    &observations,
                                    &relations,
                                    &cards,
                                    &packs,
                                    &answers,
                                    &usage_by_arm,
                                    provider_requests,
                                    completed_tasks,
                                )?;
                                if config.semantic_protocol == "m1-a80-v1"
                                    && is_a80_global_provider_failure(&error.code)
                                {
                                    first_error = Some(LiveFailureRecord {
                                        sequence,
                                        stage: "provider_global_fence".into(),
                                        code: safe_code(&error.code),
                                    });
                                    break 'tasks;
                                }
                                if config.semantic_protocol == "m1-a80-v1" {
                                    let code = safe_code(&error.code);
                                    item_failures.push(LiveItemFailureRecord {
                                        sequence,
                                        arm: ComparisonArm::C,
                                        source_id: None,
                                        task_id: Some(task.id.clone()),
                                        stage: "relation".into(),
                                        code: code.clone(),
                                    });
                                    persist_item_failures(root, &item_failures)?;
                                    relations.push(LiveArtifactRecord {
                                        sequence,
                                        stage: "relation_failure".into(),
                                        arm: ComparisonArm::C,
                                        source_id: None,
                                        task_id: Some(task.id.clone()),
                                        pack_hash: None,
                                        output: serde_json::json!({"status":"failed","code":code}),
                                        usage: None,
                                    });
                                    persist_live_progress(
                                        root,
                                        &observations,
                                        &relations,
                                        &cards,
                                        &packs,
                                        &answers,
                                        &usage_by_arm,
                                        provider_requests,
                                        completed_tasks,
                                    )?;
                                    None
                                } else {
                                    first_error = Some(LiveFailureRecord {
                                        sequence,
                                        stage: "relation".into(),
                                        code: safe_code(&error.code),
                                    });
                                    break;
                                }
                            }
                        };
                        if let Some(relation) = relation {
                            let relation_usage = relation.output.usage.clone();
                            record_arm_usage(
                                &mut usage_by_arm,
                                &ComparisonArm::C,
                                relation_usage.as_ref(),
                                relation.output.cost_minor,
                                relation.attempts,
                            );
                            relations.push(LiveArtifactRecord {
                                sequence,
                                stage: "relation".into(),
                                arm: ComparisonArm::C,
                                source_id: None,
                                task_id: Some(task.id.clone()),
                                pack_hash: None,
                                output: project_provider_output(
                                    "relation",
                                    &relation.output.output,
                                ),
                                usage: project_usage(relation_usage.as_ref()),
                            });
                            persist_live_progress(
                                root,
                                &observations,
                                &relations,
                                &cards,
                                &packs,
                                &answers,
                                &usage_by_arm,
                                provider_requests,
                                completed_tasks,
                            )?;
                            if let Some(expected) = &config.provider_runtime_snapshot
                                && provider
                                    .verify_frozen_configuration(expected)
                                    .await
                                    .is_err()
                            {
                                first_error = Some(LiveFailureRecord {
                                    sequence,
                                    stage: "provider_config_after_relation".into(),
                                    code: "provider_runtime_configuration_drift".into(),
                                });
                                break 'tasks;
                            }
                            match semantic
                                .submit_relation_proposal_for_arm(
                                    ComparisonArm::C,
                                    task,
                                    &c_sources,
                                    &relation.output.output,
                                )
                                .await
                            {
                                Ok(applied) => {
                                    if let Some(record) = relations.last_mut()
                                        && let Some(output) = record.output.as_object_mut()
                                    {
                                        output.insert("application".into(), applied);
                                    }
                                }
                                Err(code) => {
                                    if config.semantic_protocol == "m1-a80-v1" {
                                        let code = safe_code(&code);
                                        item_failures.push(LiveItemFailureRecord {
                                            sequence,
                                            arm: ComparisonArm::C,
                                            source_id: None,
                                            task_id: Some(task.id.clone()),
                                            stage: "submit_relation".into(),
                                            code: code.clone(),
                                        });
                                        persist_item_failures(root, &item_failures)?;
                                        if let Some(record) = relations.last_mut()
                                            && let Some(output) = record.output.as_object_mut()
                                        {
                                            output.insert(
                                                "application".into(),
                                                serde_json::json!({"status":"failed","code":code}),
                                            );
                                        }
                                    } else {
                                        first_error = Some(LiveFailureRecord {
                                            sequence,
                                            stage: "submit_relation".into(),
                                            code: safe_code(&code),
                                        });
                                        persist_incremental_checkpoint(
                                            root,
                                            &manifest_hash,
                                            &run_config_hash,
                                            &live_config_hash,
                                            "submit_relation",
                                            provider_requests,
                                            completed_tasks,
                                            first_error.clone(),
                                        )?;
                                        persist_live_progress(
                                            root,
                                            &observations,
                                            &relations,
                                            &cards,
                                            &packs,
                                            &answers,
                                            &usage_by_arm,
                                            provider_requests,
                                            completed_tasks,
                                        )?;
                                        break 'tasks;
                                    }
                                }
                            }
                            persist_live_progress(
                                root,
                                &observations,
                                &relations,
                                &cards,
                                &packs,
                                &answers,
                                &usage_by_arm,
                                provider_requests,
                                completed_tasks,
                            )?;
                        }
                    } else {
                        relations.push(LiveArtifactRecord {
                        sequence,
                        stage: "relation".into(),
                        arm: ComparisonArm::C,
                        source_id: None,
                        task_id: Some(task.id.clone()),
                        pack_hash: None,
                        output: serde_json::json!({
                            "status": prepared.get("status").and_then(serde_json::Value::as_str).unwrap_or("no_candidates"),
                            "candidate_count": candidate_count,
                            "expected_relation_count": task.expected_source_relations.len(),
                        }),
                        usage: None,
                    });
                        if !task.expected_source_relations.is_empty() {
                            review.push(serde_json::json!({
                                "task_id": task.id,
                                "status": "candidate_discovery_miss",
                                "expected_relation_count": task.expected_source_relations.len(),
                            }));
                        }
                        persist_live_progress(
                            root,
                            &observations,
                            &relations,
                            &cards,
                            &packs,
                            &answers,
                            &usage_by_arm,
                            provider_requests,
                            completed_tasks,
                        )?;
                    }
                }
            }
            let (pack, pack_hash, pack_reused) =
                if let Some((pack, hash)) = persisted_packs.get(&task.id) {
                    (pack.clone(), hash.clone(), true)
                } else {
                    let pack = match semantic
                        .build_pack_for_arm(
                            ComparisonArm::C,
                            task,
                            &c_sources,
                            &config.run_config.comparisons[0].budget,
                        )
                        .await
                    {
                        Ok(pack) => match project_pack_projection(&pack) {
                            Ok(pack) => pack,
                            Err(error) => {
                                if config.semantic_protocol == "m1-a80-v1" {
                                    let code = safe_code(&error.to_string());
                                    item_failures.push(LiveItemFailureRecord {
                                        sequence,
                                        arm: ComparisonArm::C,
                                        source_id: None,
                                        task_id: Some(task.id.clone()),
                                        stage: "pack_contract".into(),
                                        code: code.clone(),
                                    });
                                    pack_failures.insert(task.id.clone(), code);
                                    persist_item_failures(root, &item_failures)?;
                                    continue 'tasks;
                                }
                                first_error = Some(LiveFailureRecord {
                                    sequence,
                                    stage: "pack_contract".into(),
                                    code: safe_code(&error.to_string()),
                                });
                                break 'tasks;
                            }
                        },
                        Err(code) => {
                            if config.semantic_protocol == "m1-a80-v1" {
                                let code = safe_code(&code);
                                item_failures.push(LiveItemFailureRecord {
                                    sequence,
                                    arm: ComparisonArm::C,
                                    source_id: None,
                                    task_id: Some(task.id.clone()),
                                    stage: "pack".into(),
                                    code: code.clone(),
                                });
                                pack_failures.insert(task.id.clone(), code);
                                persist_item_failures(root, &item_failures)?;
                                continue 'tasks;
                            }
                            first_error = Some(LiveFailureRecord {
                                sequence,
                                stage: "pack".into(),
                                code: safe_code(&code),
                            });
                            break;
                        }
                    };
                    let hash = pack["pack_hash"].as_str().unwrap_or_default().to_owned();
                    (pack, hash, false)
                };
            if pack_hash.is_empty() {
                if config.semantic_protocol == "m1-a80-v1" {
                    let code = "pack_hash_missing".to_owned();
                    item_failures.push(LiveItemFailureRecord {
                        sequence,
                        arm: ComparisonArm::C,
                        source_id: None,
                        task_id: Some(task.id.clone()),
                        stage: "pack".into(),
                        code: code.clone(),
                    });
                    pack_failures.insert(task.id.clone(), code);
                    persist_item_failures(root, &item_failures)?;
                    continue 'tasks;
                }
                first_error = Some(LiveFailureRecord {
                    sequence,
                    stage: "pack".into(),
                    code: "pack_hash_missing".into(),
                });
                break 'tasks;
            }
            persisted_packs.insert(task.id.clone(), (pack.clone(), pack_hash.clone()));
            if verify_task_sources(task, c_config, &config.manifest, verifier)
                .await
                .is_err()
            {
                first_error = Some(LiveFailureRecord {
                    sequence,
                    stage: "task_source_fence_after_pack".into(),
                    code: "source_mismatch".into(),
                });
                break 'tasks;
            }
            if semantic
                .verify_arm_task_sources(ComparisonArm::C, &c_sources, "after_pack")
                .await
                .is_err()
            {
                first_error = Some(LiveFailureRecord {
                    sequence,
                    stage: "arm_source_fence_after_pack".into(),
                    code: "source_mismatch".into(),
                });
                break 'tasks;
            }
            if !pack_reused {
                packs.push(LiveArtifactRecord {
                    sequence,
                    stage: "pack".into(),
                    arm: ComparisonArm::C,
                    source_id: None,
                    task_id: Some(task.id.clone()),
                    pack_hash: Some(pack_hash.clone()),
                    output: pack.clone(),
                    usage: None,
                });
                write_live_jsonl(&root.join("packs.jsonl"), &packs)?;
            }
            if prepared_relation_missing {
                if semantic
                    .verify_arm_task_sources(ComparisonArm::C, &c_sources, "before_relation")
                    .await
                    .is_err()
                {
                    first_error = Some(LiveFailureRecord {
                        sequence,
                        stage: "arm_source_fence_before_relation".into(),
                        code: "source_mismatch".into(),
                    });
                    break 'tasks;
                }
                persist_provider_attempt(
                    root,
                    sequence.saturating_add(1),
                    "relation",
                    &ComparisonArm::C,
                    None,
                    Some(&task.id),
                    None,
                    "started",
                )?;
                let relation = match call_live_provider(
                    provider,
                    config.provider_runtime_snapshot.as_ref(),
                    &config.run_config,
                    request_budget,
                    false,
                    &request_counter,
                    &mut sequence,
                    &mut provider_requests,
                    "relation",
                    ComparisonArm::C,
                    None,
                    Some(task.id.clone()),
                    serde_json::json!({"pack": pack, "pack_hash": pack_hash}),
                )
                .await
                {
                    Ok(result) => Some(result),
                    Err(error) => {
                        record_arm_usage(
                            &mut usage_by_arm,
                            &ComparisonArm::C,
                            None,
                            None,
                            error.attempts,
                        );
                        persist_schema_diagnostic(
                            root,
                            sequence,
                            "relation",
                            &ComparisonArm::C,
                            None,
                            Some(&task.id),
                            &error,
                        )?;
                        persist_live_progress(
                            root,
                            &observations,
                            &relations,
                            &cards,
                            &packs,
                            &answers,
                            &usage_by_arm,
                            provider_requests,
                            completed_tasks,
                        )?;
                        if config.semantic_protocol == "m1-a80-v1"
                            && is_a80_global_provider_failure(&error.code)
                        {
                            first_error = Some(LiveFailureRecord {
                                sequence,
                                stage: "provider_global_fence".into(),
                                code: safe_code(&error.code),
                            });
                            break 'tasks;
                        }
                        if config.semantic_protocol == "m1-a80-v1" {
                            let code = safe_code(&error.code);
                            item_failures.push(LiveItemFailureRecord {
                                sequence,
                                arm: ComparisonArm::C,
                                source_id: None,
                                task_id: Some(task.id.clone()),
                                stage: "relation".into(),
                                code: code.clone(),
                            });
                            persist_item_failures(root, &item_failures)?;
                            relations.push(LiveArtifactRecord {
                                sequence,
                                stage: "relation_failure".into(),
                                arm: ComparisonArm::C,
                                source_id: None,
                                task_id: Some(task.id.clone()),
                                pack_hash: Some(pack_hash.clone()),
                                output: serde_json::json!({"status":"failed","code":code}),
                                usage: None,
                            });
                            persist_live_progress(
                                root,
                                &observations,
                                &relations,
                                &cards,
                                &packs,
                                &answers,
                                &usage_by_arm,
                                provider_requests,
                                completed_tasks,
                            )?;
                            None
                        } else {
                            first_error = Some(LiveFailureRecord {
                                sequence,
                                stage: "relation".into(),
                                code: safe_code(&error.code),
                            });
                            break;
                        }
                    }
                };
                if let Some(relation) = relation {
                    let relation_usage = relation.output.usage.clone();
                    record_arm_usage(
                        &mut usage_by_arm,
                        &ComparisonArm::C,
                        relation_usage.as_ref(),
                        relation.output.cost_minor,
                        relation.attempts,
                    );
                    relations.push(LiveArtifactRecord {
                        sequence,
                        stage: "relation".into(),
                        arm: ComparisonArm::C,
                        source_id: None,
                        task_id: Some(task.id.clone()),
                        pack_hash: Some(pack_hash.clone()),
                        output: project_provider_output("relation", &relation.output.output),
                        usage: project_usage(relation_usage.as_ref()),
                    });
                    persist_live_progress(
                        root,
                        &observations,
                        &relations,
                        &cards,
                        &packs,
                        &answers,
                        &usage_by_arm,
                        provider_requests,
                        completed_tasks,
                    )?;
                }
                if let Some(expected) = &config.provider_runtime_snapshot
                    && provider
                        .verify_frozen_configuration(expected)
                        .await
                        .is_err()
                {
                    first_error = Some(LiveFailureRecord {
                        sequence,
                        stage: "provider_config_after_relation".into(),
                        code: "provider_runtime_configuration_drift".into(),
                    });
                    break 'tasks;
                }
            }
            if semantic
                .verify_arm_task_sources(ComparisonArm::C, &c_sources, "after_relation")
                .await
                .is_err()
            {
                first_error = Some(LiveFailureRecord {
                    sequence,
                    stage: "arm_source_fence_after_relation".into(),
                    code: "source_mismatch".into(),
                });
                break 'tasks;
            }
            if verify_task_sources(task, c_config, &config.manifest, verifier)
                .await
                .is_err()
            {
                first_error = Some(LiveFailureRecord {
                    sequence,
                    stage: "task_source_fence_after_relation".into(),
                    code: "source_mismatch".into(),
                });
                break 'tasks;
            }
            persist_incremental_checkpoint(
                root,
                &manifest_hash,
                &run_config_hash,
                &live_config_hash,
                "relation",
                provider_requests,
                completed_tasks,
                first_error.clone(),
            )?;
            persist_live_progress(
                root,
                &observations,
                &relations,
                &cards,
                &packs,
                &answers,
                &usage_by_arm,
                provider_requests,
                completed_tasks,
            )?;
            'arms: for arm in [ComparisonArm::A, ComparisonArm::B, ComparisonArm::C] {
                let arm_index = match arm {
                    ComparisonArm::A => 0,
                    ComparisonArm::B => 1,
                    ComparisonArm::C => 2,
                };
                if !config.run_config.comparisons[arm_index]
                    .task_ids
                    .contains(&task.id)
                {
                    continue;
                }
                if answers.iter().any(|record| {
                    record.arm == arm
                        && record.task_id.as_deref() == Some(task.id.as_str())
                        && matches!(record.stage.as_str(), "answer" | "answer_failure")
                }) {
                    continue;
                }
                if resume_attempts.iter().any(|attempt| {
                    attempt.stage == "answer"
                        && attempt.arm == comparison_arm_wire_name(&arm)
                        && attempt.task_id.as_deref() == Some(task.id.as_str())
                }) {
                    item_failures.push(LiveItemFailureRecord {
                        sequence,
                        arm: arm.clone(),
                        source_id: None,
                        task_id: Some(task.id.clone()),
                        stage: "answer_resume".into(),
                        code: "provider_attempt_outcome_uncertain".into(),
                    });
                    persist_item_failures(root, &item_failures)?;
                    answers.push(LiveArtifactRecord {
                        sequence,
                        stage: "answer_failure".into(),
                        arm: arm.clone(),
                        source_id: None,
                        task_id: Some(task.id.clone()),
                        pack_hash: None,
                        output: serde_json::json!({"status":"not_replayed","code":"provider_attempt_outcome_uncertain"}),
                        usage: None,
                    });
                    persist_live_progress(
                        root,
                        &observations,
                        &relations,
                        &cards,
                        &packs,
                        &answers,
                        &usage_by_arm,
                        provider_requests,
                        completed_tasks,
                    )?;
                    continue;
                }
                if arm == ComparisonArm::C
                    && let Some(code) = pack_failures.get(&task.id)
                {
                    item_failures.push(LiveItemFailureRecord {
                        sequence,
                        arm: arm.clone(),
                        source_id: None,
                        task_id: Some(task.id.clone()),
                        stage: "answer_pack".into(),
                        code: code.clone(),
                    });
                    persist_item_failures(root, &item_failures)?;
                    answers.push(LiveArtifactRecord {
                        sequence,
                        stage: "answer_failure".into(),
                        arm: arm.clone(),
                        source_id: None,
                        task_id: Some(task.id.clone()),
                        pack_hash: None,
                        output: serde_json::json!({"status":"failed","code":code}),
                        usage: None,
                    });
                    persist_live_progress(
                        root,
                        &observations,
                        &relations,
                        &cards,
                        &packs,
                        &answers,
                        &usage_by_arm,
                        provider_requests,
                        completed_tasks,
                    )?;
                    continue;
                }
                if verify_task_sources(
                    task,
                    &config.run_config.comparisons[arm_index],
                    &config.manifest,
                    verifier,
                )
                .await
                .is_err()
                {
                    first_error = Some(LiveFailureRecord {
                        sequence,
                        stage: "task_source_fence".into(),
                        code: "source_mismatch".into(),
                    });
                    break 'tasks;
                }
                let selected_sources = task
                    .source_ids
                    .iter()
                    .filter(|source_id| {
                        config.run_config.comparisons[arm_index]
                            .source_ids
                            .contains(source_id)
                    })
                    .filter_map(|source_id| {
                        config
                            .manifest
                            .sources
                            .iter()
                            .find(|source| &source.logical_id == source_id)
                            .cloned()
                    })
                    .collect::<Vec<_>>();
                if selected_sources.is_empty() {
                    first_error = Some(LiveFailureRecord {
                        sequence,
                        stage: "source_selection".into(),
                        code: "arm_task_has_no_selected_source".into(),
                    });
                    break;
                }
                let (arm_input, pack_hash) = if arm == ComparisonArm::A {
                    if verify_task_sources(
                        task,
                        &config.run_config.comparisons[arm_index],
                        &config.manifest,
                        verifier,
                    )
                    .await
                    .is_err()
                    {
                        first_error = Some(LiveFailureRecord {
                            sequence,
                            stage: "task_source_fence_after_retrieval".into(),
                            code: "source_mismatch".into(),
                        });
                        break 'tasks;
                    }
                    let mut input = match semantic
                        .ordinary_retrieve_with_profile(
                            task,
                            &selected_sources,
                            &config.run_config.comparisons[arm_index].budget,
                            &config.run_config.comparisons[arm_index].index_profile_id,
                        )
                        .await
                    {
                        Ok(input) => input,
                        Err(code) => {
                            if config.semantic_protocol == "m1-a80-v1" {
                                append_a80_answer_failure(
                                    &mut item_failures,
                                    &mut answers,
                                    sequence,
                                    arm.clone(),
                                    &task.id,
                                    "ordinary_retrieval",
                                    &code,
                                    None,
                                );
                                persist_item_failures(root, &item_failures)?;
                                persist_live_progress(
                                    root,
                                    &observations,
                                    &relations,
                                    &cards,
                                    &packs,
                                    &answers,
                                    &usage_by_arm,
                                    provider_requests,
                                    completed_tasks,
                                )?;
                                continue 'arms;
                            }
                            first_error = Some(LiveFailureRecord {
                                sequence,
                                stage: "ordinary_retrieval".into(),
                                code: safe_code(&code),
                            });
                            break;
                        }
                    };
                    let Some(input_object) = input.as_object_mut() else {
                        if config.semantic_protocol == "m1-a80-v1" {
                            append_a80_answer_failure(
                                &mut item_failures,
                                &mut answers,
                                sequence,
                                arm.clone(),
                                &task.id,
                                "ordinary_retrieval",
                                "ordinary_retrieval_output_invalid",
                                None,
                            );
                            persist_item_failures(root, &item_failures)?;
                            persist_live_progress(
                                root,
                                &observations,
                                &relations,
                                &cards,
                                &packs,
                                &answers,
                                &usage_by_arm,
                                provider_requests,
                                completed_tasks,
                            )?;
                            continue 'arms;
                        }
                        first_error = Some(LiveFailureRecord {
                            sequence,
                            stage: "ordinary_retrieval".into(),
                            code: "ordinary_retrieval_output_invalid".into(),
                        });
                        break 'tasks;
                    };
                    input_object.insert("query".into(), serde_json::json!(task.query));
                    if verify_task_sources(
                        task,
                        &config.run_config.comparisons[arm_index],
                        &config.manifest,
                        verifier,
                    )
                    .await
                    .is_err()
                    {
                        first_error = Some(LiveFailureRecord {
                            sequence,
                            stage: "task_source_fence_after_retrieval".into(),
                            code: "source_mismatch".into(),
                        });
                        break 'tasks;
                    }
                    review.push(ordinary_retrieval_review_record(task, &input));
                    if let Some(code) = ordinary_retrieval_failure(task, &selected_sources, &input)
                    {
                        if config.semantic_protocol == "m1-a80-v1" {
                            append_a80_answer_failure(
                                &mut item_failures,
                                &mut answers,
                                sequence,
                                arm.clone(),
                                &task.id,
                                "ordinary_retrieval",
                                code,
                                None,
                            );
                            persist_item_failures(root, &item_failures)?;
                            persist_live_progress(
                                root,
                                &observations,
                                &relations,
                                &cards,
                                &packs,
                                &answers,
                                &usage_by_arm,
                                provider_requests,
                                completed_tasks,
                            )?;
                            continue 'arms;
                        }
                        first_error = Some(LiveFailureRecord {
                            sequence,
                            stage: "ordinary_retrieval".into(),
                            code: code.into(),
                        });
                        break 'tasks;
                    }
                    (input, None)
                } else {
                    let (pack, hash) = if arm == ComparisonArm::C {
                        persisted_packs
                            .get(&task.id)
                            .cloned()
                            .ok_or(EvalError::LiveConfig("C pack was not persisted"))?
                    } else {
                        let pack = match semantic
                            .build_pack_for_arm(
                                arm.clone(),
                                task,
                                &selected_sources,
                                &config.run_config.comparisons[arm_index].budget,
                            )
                            .await
                        {
                            Ok(pack) => match project_pack_projection(&pack) {
                                Ok(pack) => pack,
                                Err(error) => {
                                    if config.semantic_protocol == "m1-a80-v1" {
                                        append_a80_answer_failure(
                                            &mut item_failures,
                                            &mut answers,
                                            sequence,
                                            arm.clone(),
                                            &task.id,
                                            "pack_contract",
                                            &error.to_string(),
                                            None,
                                        );
                                        persist_item_failures(root, &item_failures)?;
                                        persist_live_progress(
                                            root,
                                            &observations,
                                            &relations,
                                            &cards,
                                            &packs,
                                            &answers,
                                            &usage_by_arm,
                                            provider_requests,
                                            completed_tasks,
                                        )?;
                                        continue 'arms;
                                    }
                                    first_error = Some(LiveFailureRecord {
                                        sequence,
                                        stage: "pack_contract".into(),
                                        code: safe_code(&error.to_string()),
                                    });
                                    break 'tasks;
                                }
                            },
                            Err(code) => {
                                if config.semantic_protocol == "m1-a80-v1" {
                                    append_a80_answer_failure(
                                        &mut item_failures,
                                        &mut answers,
                                        sequence,
                                        arm.clone(),
                                        &task.id,
                                        "pack",
                                        &code,
                                        None,
                                    );
                                    persist_item_failures(root, &item_failures)?;
                                    persist_live_progress(
                                        root,
                                        &observations,
                                        &relations,
                                        &cards,
                                        &packs,
                                        &answers,
                                        &usage_by_arm,
                                        provider_requests,
                                        completed_tasks,
                                    )?;
                                    continue 'arms;
                                }
                                first_error = Some(LiveFailureRecord {
                                    sequence,
                                    stage: "pack".into(),
                                    code: safe_code(&code),
                                });
                                break;
                            }
                        };
                        let hash = pack["pack_hash"].as_str().unwrap_or_default().to_owned();
                        if hash.is_empty() {
                            if config.semantic_protocol == "m1-a80-v1" {
                                append_a80_answer_failure(
                                    &mut item_failures,
                                    &mut answers,
                                    sequence,
                                    arm.clone(),
                                    &task.id,
                                    "pack",
                                    "pack_hash_missing",
                                    None,
                                );
                                persist_item_failures(root, &item_failures)?;
                                persist_live_progress(
                                    root,
                                    &observations,
                                    &relations,
                                    &cards,
                                    &packs,
                                    &answers,
                                    &usage_by_arm,
                                    provider_requests,
                                    completed_tasks,
                                )?;
                                continue 'arms;
                            }
                            first_error = Some(LiveFailureRecord {
                                sequence,
                                stage: "pack".into(),
                                code: "pack_hash_missing".into(),
                            });
                            break;
                        }
                        packs.push(LiveArtifactRecord {
                            sequence,
                            stage: "pack".into(),
                            arm: arm.clone(),
                            source_id: None,
                            task_id: Some(task.id.clone()),
                            pack_hash: Some(hash.clone()),
                            output: pack.clone(),
                            usage: None,
                        });
                        write_live_jsonl(&root.join("packs.jsonl"), &packs)?;
                        (pack, hash)
                    };
                    (
                        serde_json::json!({
                            "query": task.query,
                            "pack": pack,
                            "pack_hash": hash
                        }),
                        Some(hash),
                    )
                };
                if arm != ComparisonArm::A
                    && semantic
                        .verify_arm_task_sources(arm.clone(), &selected_sources, "after_pack")
                        .await
                        .is_err()
                {
                    first_error = Some(LiveFailureRecord {
                        sequence,
                        stage: "arm_source_fence_after_pack".into(),
                        code: "source_mismatch".into(),
                    });
                    break 'tasks;
                }
                if arm != ComparisonArm::A
                    && verify_task_sources(
                        task,
                        &config.run_config.comparisons[arm_index],
                        &config.manifest,
                        verifier,
                    )
                    .await
                    .is_err()
                {
                    first_error = Some(LiveFailureRecord {
                        sequence,
                        stage: "task_source_fence_after_pack".into(),
                        code: "source_mismatch".into(),
                    });
                    break 'tasks;
                }
                if arm != ComparisonArm::A
                    && semantic
                        .verify_arm_task_sources(arm.clone(), &selected_sources, "before_answer")
                        .await
                        .is_err()
                {
                    first_error = Some(LiveFailureRecord {
                        sequence,
                        stage: "arm_source_fence_before_answer".into(),
                        code: "source_mismatch".into(),
                    });
                    break 'tasks;
                }
                persist_provider_attempt(
                    root,
                    sequence.saturating_add(1),
                    "answer",
                    &arm,
                    None,
                    Some(&task.id),
                    None,
                    "started",
                )?;
                let result = match call_live_provider(
                    provider,
                    config.provider_runtime_snapshot.as_ref(),
                    &config.run_config,
                    request_budget,
                    false,
                    &request_counter,
                    &mut sequence,
                    &mut provider_requests,
                    "answer",
                    arm.clone(),
                    None,
                    Some(task.id.clone()),
                    arm_input,
                )
                .await
                {
                    Ok(result) => result,
                    Err(error) => {
                        record_arm_usage(&mut usage_by_arm, &arm, None, None, error.attempts);
                        persist_schema_diagnostic(
                            root,
                            sequence,
                            "answer",
                            &arm,
                            None,
                            Some(&task.id),
                            &error,
                        )?;
                        persist_live_progress(
                            root,
                            &observations,
                            &relations,
                            &cards,
                            &packs,
                            &answers,
                            &usage_by_arm,
                            provider_requests,
                            completed_tasks,
                        )?;
                        if config.semantic_protocol == "m1-a80-v1"
                            && is_a80_global_provider_failure(&error.code)
                        {
                            first_error = Some(LiveFailureRecord {
                                sequence,
                                stage: "provider_global_fence".into(),
                                code: safe_code(&error.code),
                            });
                            break 'tasks;
                        }
                        if config.semantic_protocol == "m1-a80-v1" {
                            item_failures.push(LiveItemFailureRecord {
                                sequence,
                                arm: arm.clone(),
                                source_id: None,
                                task_id: Some(task.id.clone()),
                                stage: "answer".into(),
                                code: safe_code(&error.code),
                            });
                            persist_item_failures(root, &item_failures)?;
                            answers.push(LiveArtifactRecord {
                                sequence,
                                stage: "answer_failure".into(),
                                arm: arm.clone(),
                                source_id: None,
                                task_id: Some(task.id.clone()),
                                pack_hash: pack_hash.clone(),
                                output: serde_json::json!({"status":"failed","code":safe_code(&error.code)}),
                                usage: None,
                            });
                            persist_live_progress(
                                root,
                                &observations,
                                &relations,
                                &cards,
                                &packs,
                                &answers,
                                &usage_by_arm,
                                provider_requests,
                                completed_tasks,
                            )?;
                            continue;
                        }
                        first_error = Some(LiveFailureRecord {
                            sequence,
                            stage: "answer".into(),
                            code: safe_code(&error.code),
                        });
                        break;
                    }
                };
                let usage = result.output.usage.clone();
                record_arm_usage(
                    &mut usage_by_arm,
                    &arm,
                    usage.as_ref(),
                    result.output.cost_minor,
                    result.attempts,
                );
                answers.push(LiveArtifactRecord {
                    sequence,
                    stage: "answer".into(),
                    arm: arm.clone(),
                    source_id: None,
                    task_id: Some(task.id.clone()),
                    pack_hash: pack_hash.clone(),
                    output: project_provider_output("answer", &result.output.output),
                    usage: project_usage(usage.as_ref()),
                });
                persist_live_progress(
                    root,
                    &observations,
                    &relations,
                    &cards,
                    &packs,
                    &answers,
                    &usage_by_arm,
                    provider_requests,
                    completed_tasks,
                )?;
                if let Some(expected) = &config.provider_runtime_snapshot
                    && provider
                        .verify_frozen_configuration(expected)
                        .await
                        .is_err()
                {
                    first_error = Some(LiveFailureRecord {
                        sequence,
                        stage: "provider_config_after_answer".into(),
                        code: "provider_runtime_configuration_drift".into(),
                    });
                    break 'tasks;
                }
                if arm != ComparisonArm::A
                    && semantic
                        .verify_arm_task_sources(arm.clone(), &selected_sources, "after_answer")
                        .await
                        .is_err()
                {
                    first_error = Some(LiveFailureRecord {
                        sequence,
                        stage: "arm_source_fence_after_answer".into(),
                        code: "source_mismatch".into(),
                    });
                    break 'tasks;
                }
                if verify_task_sources(
                    task,
                    &config.run_config.comparisons[arm_index],
                    &config.manifest,
                    verifier,
                )
                .await
                .is_err()
                {
                    first_error = Some(LiveFailureRecord {
                        sequence,
                        stage: "task_source_fence_after_answer".into(),
                        code: "source_mismatch".into(),
                    });
                    break 'tasks;
                }
                persist_incremental_checkpoint(
                    root,
                    &manifest_hash,
                    &run_config_hash,
                    &live_config_hash,
                    "answer",
                    provider_requests,
                    completed_tasks,
                    first_error.clone(),
                )?;
                persist_live_progress(
                    root,
                    &observations,
                    &relations,
                    &cards,
                    &packs,
                    &answers,
                    &usage_by_arm,
                    provider_requests,
                    completed_tasks,
                )?;
            }
            if first_error.is_some() {
                break;
            }
            completed_tasks += 1;
        }
    }
    if first_error.is_none() && !item_failures.is_empty() {
        first_error = Some(LiveFailureRecord {
            sequence,
            stage: "isolated_items".into(),
            code: "evaluation_items_failed".into(),
        });
    }
    if let Some(error) = &first_error {
        // Make the terminal failure visible before assembling the larger
        // report. The checkpoint is independently atomic/readable even if a
        // later report artifact cannot be produced.
        persist_incremental_checkpoint(
            root,
            &manifest_hash,
            &run_config_hash,
            &live_config_hash,
            &error.stage,
            provider_requests,
            completed_tasks,
            Some(error.clone()),
        )?;
        review.push(serde_json::json!({"status":"pending","failure_code":error.code}));
    } else {
        review.push(serde_json::json!({"status":"pending","reason":"manual_review_required"}));
    }
    let status = if first_error.is_some() {
        LiveRunStatus::Failed
    } else {
        LiveRunStatus::Completed
    };
    let checkpoint = LiveCheckpoint {
        schema_version: LIVE_RUN_SCHEMA,
        manifest_hash: manifest_hash.clone(),
        run_config_hash: run_config_hash.clone(),
        live_config_hash: live_config_hash.clone(),
        last_boundary: "final".into(),
        provider_requests,
        completed_tasks,
        status: status.clone(),
        first_error: first_error.clone(),
    };
    let cost_unknown = usage_by_arm.values().any(|stats| stats.cost_unknown);
    let cost_available = !cost_unknown
        && usage_by_arm
            .values()
            .all(|stats| stats.cost_minor.is_some());
    let cost_total = cost_available.then(|| {
        usage_by_arm
            .values()
            .filter_map(|stats| stats.cost_minor)
            .fold(0_u64, u64::saturating_add)
    });
    let input_tokens = usage_by_arm
        .values()
        .all(|stats| stats.input_tokens.is_some())
        .then(|| {
            usage_by_arm
                .values()
                .filter_map(|stats| stats.input_tokens)
                .fold(0_u64, u64::saturating_add)
        });
    let output_tokens = usage_by_arm
        .values()
        .all(|stats| stats.output_tokens.is_some())
        .then(|| {
            usage_by_arm
                .values()
                .filter_map(|stats| stats.output_tokens)
                .fold(0_u64, u64::saturating_add)
        });
    let token_status = if provider_requests == 0 {
        "unavailable"
    } else if input_tokens.is_some() && output_tokens.is_some() {
        "known"
    } else {
        "unknown"
    };
    let usage = serde_json::json!({
        "schema_version": LIVE_RUN_SCHEMA,
        "request_count": provider_requests,
        "input_tokens": input_tokens,
        "output_tokens": output_tokens,
        "token_status": token_status,
        "completed_tasks": completed_tasks,
        "cost_minor": cost_total,
        "cost_status": if cost_unknown {
            "unknown"
        } else if cost_available {
            "known"
        } else {
            "unavailable"
        },
        "cost_budget_minor": config.run_config.cost_budget_minor,
        "unbounded_cost_authorized": config.unbounded_cost_authorized,
        "by_arm": usage_by_arm,
        "status": if first_error.is_some() { "failed" } else { "pending_manual_review" },
    });
    let metrics = serde_json::json!({
        "focus_coverage": "pending_manual_review",
        "support_precision": "pending_manual_review",
        "condition_retention": "pending_manual_review",
        "redundancy": "pending_manual_review",
        "no_answer": "pending_manual_review",
        "task_result": "pending_manual_review",
    });
    let schema_diagnostics = root
        .join("schema-diagnostics.jsonl")
        .exists()
        .then(|| {
            fs::read_to_string(root.join("schema-diagnostics.jsonl"))
                .ok()
                .map(|text| {
                    text.lines()
                        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
                        .collect::<Vec<_>>()
                })
        })
        .flatten()
        .unwrap_or_default();
    let report_value = serde_json::json!({
        "schema_version": LIVE_RUN_SCHEMA,
        "manifest_hash": manifest_hash,
        "run_config_hash": run_config_hash,
        "status": if first_error.is_some() { "failed" } else { "completed_pending_review" },
        "quality_claim": "not_evaluated",
        "engineering_status": if first_error.is_some() { "failed" } else { "pending" },
        "semantic_status": "pending_manual_review",
        "task_status": "pending_manual_review",
        "cost_status": "reported_only",
        "by_arm": {"A": metrics.clone(), "B": metrics.clone(), "C": metrics},
        "task_results": {
            "evaluated_tasks": completed_tasks,
            "requested_tasks": task_ids.len(),
            "answers_recorded": answers.len(),
            "packs_recorded": packs.len(),
            "focus_coverage": "pending_manual_review",
            "support_precision": "pending_manual_review",
            "condition_retention": "pending_manual_review",
            "redundancy": "pending_manual_review",
            "no_answer": "pending_manual_review",
            "task_result": "pending_manual_review"
        },
        "first_error": first_error.clone(),
        "schema_diagnostics": schema_diagnostics,
        "source_count": source_ids.len(),
        "task_count": task_ids.len(),
    });
    let report = format!(
        "# Semantic-card M6 live evaluation\n\nStatus: `{}`\n\nQuality claim: `not_evaluated`\n\nThe following machine-readable summary is provisional and requires independent human review:\n\n```json\n{}\n```\n",
        if first_error.is_some() {
            "failed"
        } else {
            "completed_pending_review"
        },
        serde_json::to_string_pretty(&report_value).map_err(|_| EvalError::ArtifactIo)?
    );
    write_live_artifacts(
        root,
        &config.manifest,
        &config.run_config,
        &manifest_hash,
        &run_config_hash,
        &observations,
        &relations,
        &cards,
        &packs,
        &answers,
        &review,
        &usage,
        &report,
        &checkpoint,
        config,
        &live_config_hash,
    )?;
    Ok(LiveEvaluationResult {
        status,
        manifest_hash,
        run_config_hash,
        artifact_root: root.display().to_string(),
        provider_requests,
        completed_tasks,
        first_error_code: first_error.map(|error| error.code),
    })
}

async fn verify_live_source_snapshots<V: SourceSnapshotVerifier>(
    manifest: &EvaluationManifest,
    verifier: &V,
) -> Result<(), EvalError> {
    validate_live_manifest(manifest)?;
    for source in &manifest.sources {
        let actual = verifier.snapshot_current(source).await?;
        if !actual.authorized
            || actual.logical_id != source.logical_id
            || actual.vault_id != source.vault_id
            || actual.file_id != source.file_id
            || actual.path != source.path
            || actual.file_revision != source.file_revision
            || actual.source_revision_id != source.source_revision_id
            || actual.content_hash != source.content_hash
            || format!("{:x}", Sha256::digest(&actual.content)) != source.content_hash
            || actual.authorization_revision != source.authorization_revision
            || actual.profile_id != source.profile_id
            || actual.rules_revision != source.rules_revision
            || actual.source_generation != source.source_generation
            || actual.extraction_commit_sequence != source.extraction_commit_sequence
        {
            return Err(EvalError::SourceMismatch);
        }
    }
    Ok(())
}

fn canonical_hash<T: Serialize>(value: &T) -> Result<String, serde_json::Error> {
    fn canonicalize(value: serde_json::Value) -> serde_json::Value {
        match value {
            serde_json::Value::Object(object) => {
                let mut entries = object.into_iter().collect::<Vec<_>>();
                entries.sort_by(|left, right| left.0.cmp(&right.0));
                serde_json::Value::Object(
                    entries
                        .into_iter()
                        .map(|(key, value)| (key, canonicalize(value)))
                        .collect(),
                )
            }
            serde_json::Value::Array(values) => {
                serde_json::Value::Array(values.into_iter().map(canonicalize).collect())
            }
            scalar => scalar,
        }
    }
    let value = canonicalize(serde_json::to_value(value)?);
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(&value)?)))
}

pub fn canonical_live_evaluation_hash(config: &LiveEvaluationConfig) -> Result<String, EvalError> {
    canonical_hash(config).map_err(|_| EvalError::LiveConfig("live config hash failed"))
}

fn is_hash(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn safe_code(value: &str) -> String {
    value
        .chars()
        .filter(|character| {
            character.is_ascii_alphanumeric() || *character == '_' || *character == '-'
        })
        .take(64)
        .collect()
}

fn reject_secret_fields<T: Serialize>(value: &T) -> Result<(), EvalError> {
    fn normalize(value: &str) -> String {
        value
            .chars()
            .filter(|character| character.is_ascii_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect()
    }
    fn visit(value: &serde_json::Value) -> bool {
        match value {
            serde_json::Value::Object(map) => map.iter().any(|(key, value)| {
                let key = normalize(key);
                key == "secret"
                    || key.ends_with("secret")
                    || key.contains("password")
                    || matches!(
                        key.as_str(),
                        "token" | "accesstoken" | "refreshtoken" | "bearertoken" | "authtoken"
                    )
                    || key.contains("apikey")
                    || key == "authorization"
                    || key.contains("authorizationheader")
                    || key.contains("requestauthorization")
                    || visit(value)
            }),
            serde_json::Value::Array(values) => values.iter().any(visit),
            _ => false,
        }
    }
    if visit(&serde_json::to_value(value).map_err(|_| EvalError::Manifest("serialization failed"))?)
    {
        Err(EvalError::Manifest("secret-bearing fields are forbidden"))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a80_global_provider_failure_includes_both_budget_exhaustion_codes() {
        assert!(is_a80_global_provider_failure(
            "provider_request_budget_exhausted"
        ));
        assert!(is_a80_global_provider_failure("request_budget_exhausted"));
        assert!(!is_a80_global_provider_failure("local_fake_timeout"));
    }

    #[test]
    fn card_projection_preserves_all_cards_in_collection() {
        let projected = card_artifact_projection(
            &serde_json::json!({
                "card_count": 2,
                "cards": [
                    {"card_id":"card-1","title":"first","temporal_scope":{"status":"source_stated","value":"2025"},"assertions":[{"content":"bounded","evidence_refs":["E1"]}]},
                    {"card_id":"card-2","title":"second","assertions":[]}
                ]
            }),
            &source(),
        );
        assert_eq!(projected["card_count"], 2);
        assert_eq!(projected["cards"].as_array().unwrap().len(), 2);
        assert_eq!(projected["cards"][0]["temporal_scope"]["value"], "2025");
        assert_eq!(
            projected["cards"][0]["assertions"][0]["evidence_refs"][0],
            "E1"
        );
        assert!(projected["safe_projection_hash"].as_str().is_some());
    }

    #[test]
    fn card_projection_preserves_a_valid_empty_collection() {
        let projected =
            card_artifact_projection(&serde_json::json!({"card_count":0,"cards":[]}), &source());
        assert_eq!(projected["card_count"], 0);
        assert_eq!(projected["cards"].as_array().unwrap().len(), 0);
    }

    fn source() -> EvalSource {
        EvalSource {
            logical_id: "S01".into(),
            synthetic_placeholder: false,
            vault_id: "synthetic".into(),
            file_id: "file-1".into(),
            path: "notes/one.md".into(),
            file_revision: 1,
            content_hash: "a".repeat(64),
            logical_block_count: 0,
            source_revision_id: "source-1".into(),
            authorization_revision: 1,
            profile_id: "profile-v1".into(),
            rules_revision: 1,
            source_generation: 1,
            extraction_commit_sequence: 1,
        }
    }
    fn manifest() -> EvaluationManifest {
        EvaluationManifest {
            schema_version: MANIFEST_SCHEMA.into(),
            dataset_id: "m6-synthetic".into(),
            synthetic_only: true,
            sources: vec![source()],
            tasks: vec![EvalTask {
                id: "task-1".into(),
                split: TaskSplit::Development,
                query_refs: vec!["Q01".into()],
                query: "what is the decision?".into(),
                source_ids: vec!["S01".into()],
                source_fence: vec![],
                must_preserve: vec!["scope".into()],
                must_not_infer: vec!["status".into()],
                expected_usable_information: vec!["decision".into()],
                expected_no_answer: false,
                expected_status: "available".into(),
                expected_source_relations: vec![],
                severity: "high".into(),
            }],
        }
    }
    fn config(mode: EvalMode) -> EvaluationRunConfig {
        let budget = Budget {
            max_entries: 10,
            max_bytes: 4096,
            max_tokens: 1024,
            external_request_budget: 0,
        };
        EvaluationRunConfig {
            schema_version: RUN_SCHEMA.into(),
            mode,
            allow_live: false,
            explicit_live_authorization: false,
            source_allowlist: Vec::new(),
            cost_budget_minor: 0,
            artifact_root: None,
            comparisons: [ComparisonArm::A, ComparisonArm::B, ComparisonArm::C]
                .into_iter()
                .map(|arm| ComparisonConfig {
                    arm,
                    source_ids: vec!["S01".into()],
                    task_ids: vec!["task-1".into()],
                    budget: budget.clone(),
                    index_profile_id: "index-v1".into(),
                    prompt_id: "prompt-v1".into(),
                    schema_id: "schema-v1".into(),
                    answer_model_id: "answer-model-placeholder".into(),
                })
                .collect(),
        }
    }
    #[test]
    fn manifest_unknown_path_hash_and_secret_fail_closed() {
        let mut value = serde_json::to_value(manifest()).unwrap();
        value["unknown"] = serde_json::json!(true);
        assert!(serde_json::from_value::<EvaluationManifest>(value).is_err());
        let mut invalid = manifest();
        invalid.sources[0].content_hash = "bad".into();
        assert_eq!(
            validate_manifest(&invalid),
            Err(EvalError::Manifest("source identity or hash is invalid"))
        );
        invalid = manifest();
        invalid.sources[0].path = "../secret.md".into();
        assert_eq!(
            validate_manifest(&invalid),
            Err(EvalError::Manifest("source path is unsafe"))
        );
        assert!(reject_secret_fields(&serde_json::json!({"Api-Key":"x"})).is_err());
        assert!(reject_secret_fields(&serde_json::json!({"REFRESH.Token":"x"})).is_err());
        assert!(reject_secret_fields(&serde_json::json!({"max_tokens":100})).is_ok());
        assert!(reject_secret_fields(&serde_json::json!({"authorization_revision":4})).is_ok());
    }

    #[test]
    fn safe_template_artifact_fingerprints_full_semantics_without_persisting_content() {
        let template = ProviderStageTemplate {
            stage: "observation".into(),
            model_id: "model-v1".into(),
            prompt_id: "prompt-v1".into(),
            schema_id: "schema-v1".into(),
            index_profile_id: "profile-v1".into(),
            system: "private system prompt text".into(),
            schema_name: "private schema name".into(),
            schema: serde_json::json!({
                "type": "object",
                "description": "private schema description",
            }),
            max_output_tokens: 512,
            temperature: Some(0.1),
            timeout_seconds: 30,
        };
        let artifact = safe_provider_template_artifacts(std::slice::from_ref(&template))
            .unwrap()
            .remove(0);
        let serialized = artifact.to_string();
        assert_eq!(
            artifact["template_fingerprint"],
            canonical_hash(&template).unwrap()
        );
        assert!(!serialized.contains(&template.system));
        assert!(!serialized.contains(&template.schema_name));
        assert!(!serialized.contains("private schema description"));

        let mut changed_prompt = template.clone();
        changed_prompt.system.push_str(" with changed semantics");
        let changed_prompt_artifact = safe_provider_template_artifacts(&[changed_prompt])
            .unwrap()
            .remove(0);
        assert_ne!(
            artifact["template_fingerprint"],
            changed_prompt_artifact["template_fingerprint"]
        );

        let mut changed_schema = template;
        changed_schema
            .schema
            .as_object_mut()
            .unwrap()
            .insert("required".into(), serde_json::json!(["statement"]));
        let changed_schema_artifact = safe_provider_template_artifacts(&[changed_schema])
            .unwrap()
            .remove(0);
        assert_ne!(
            artifact["template_fingerprint"],
            changed_schema_artifact["template_fingerprint"]
        );
    }

    #[test]
    fn converts_the_existing_m0_fixture_and_preserves_all_gold_refs() {
        let manifest = convert_m0_fixture_manifest(include_str!(
            "../../memory/tests/fixtures/semantic-memory/manifest.json"
        ))
        .unwrap();
        assert_eq!(manifest.sources.len(), 12);
        assert_eq!(manifest.tasks.len(), 24);
        assert!(manifest.tasks.iter().all(
            |task| !task.query_refs.is_empty() && !task.expected_usable_information.is_empty()
        ));
        assert!(
            manifest
                .tasks
                .iter()
                .flat_map(|task| task.expected_source_relations.iter())
                .all(|relation| relation.source_ids.iter().all(|id| manifest
                    .sources
                    .iter()
                    .any(|source| &source.logical_id == id)))
        );
    }
    #[test]
    fn live_and_arm_mismatch_are_blocked_before_external_request() {
        assert_eq!(
            validate_run_config(&manifest(), &config(EvalMode::Live)),
            Err(EvalError::LiveBlocked("allow_live is false"))
        );
        let mut invalid = config(EvalMode::Mock);
        invalid.comparisons[2].budget.max_tokens = 5;
        assert_eq!(
            validate_run_config(&manifest(), &invalid),
            Err(EvalError::RunConfig(
                "A/B/C inputs or budget are inconsistent"
            ))
        );
        for field in ["prompt", "schema", "answer"] {
            let mut mismatch = config(EvalMode::Mock);
            match field {
                "prompt" => mismatch.comparisons[1].prompt_id = "changed".into(),
                "schema" => mismatch.comparisons[1].schema_id = "changed".into(),
                "answer" => mismatch.comparisons[1].answer_model_id = "changed".into(),
                _ => unreachable!(),
            }
            assert_eq!(
                validate_run_config(&manifest(), &mismatch),
                Err(EvalError::RunConfig(
                    "A/B/C inputs or budget are inconsistent"
                ))
            );
        }
        let mut live = config(EvalMode::Live);
        live.allow_live = true;
        live.explicit_live_authorization = true;
        live.cost_budget_minor = 1;
        live.artifact_root = Some("/tmp/eval".into());
        live.source_allowlist = vec!["S01".into()];
        assert_eq!(
            validate_run_config(&manifest(), &live),
            Err(EvalError::LiveBlocked(
                "source allowlist does not exactly cover manifest"
            ))
        );
        live.source_allowlist.push("file-1".into());
        live.source_allowlist.push("extra".into());
        assert_eq!(
            validate_run_config(&manifest(), &live),
            Err(EvalError::LiveBlocked(
                "source allowlist does not exactly cover manifest"
            ))
        );
    }
    struct FakeVerifier {
        drift: bool,
        fence: Option<&'static str>,
    }
    impl SourceSnapshotVerifier for FakeVerifier {
        fn snapshot(&self, source: &EvalSource) -> Result<AuthoritativeSourceSnapshot, EvalError> {
            let content = b"fixture-body".to_vec();
            let hash = format!("{:x}", Sha256::digest(&content));
            Ok(AuthoritativeSourceSnapshot {
                logical_id: source.logical_id.clone(),
                vault_id: source.vault_id.clone(),
                file_id: source.file_id.clone(),
                path: source.path.clone(),
                file_revision: source.file_revision,
                source_revision_id: source.source_revision_id.clone(),
                content_hash: if self.drift { "b".repeat(64) } else { hash },
                content,
                authorized: true,
                authorization_revision: if self.fence == Some("authorization") {
                    2
                } else {
                    source.authorization_revision
                },
                profile_id: if self.fence == Some("profile") {
                    "changed".into()
                } else {
                    source.profile_id.clone()
                },
                rules_revision: if self.fence == Some("rules") {
                    2
                } else {
                    source.rules_revision
                },
                source_generation: if self.fence == Some("generation") {
                    2
                } else {
                    source.source_generation
                },
                extraction_commit_sequence: if self.fence == Some("commit") {
                    2
                } else {
                    source.extraction_commit_sequence
                },
            })
        }
    }
    #[test]
    fn authoritative_source_snapshot_success_and_hash_drift_are_fail_closed() {
        let mut manifest = manifest();
        manifest.sources[0].content_hash = format!("{:x}", Sha256::digest(b"fixture-body"));
        assert!(
            verify_source_snapshots(
                &manifest,
                &FakeVerifier {
                    drift: false,
                    fence: None
                }
            )
            .is_ok()
        );
        assert_eq!(
            verify_source_snapshots(
                &manifest,
                &FakeVerifier {
                    drift: true,
                    fence: None
                }
            ),
            Err(EvalError::SourceMismatch)
        );
        for fence in ["authorization", "profile", "rules", "generation", "commit"] {
            assert_eq!(
                verify_source_snapshots(
                    &manifest,
                    &FakeVerifier {
                        drift: false,
                        fence: Some(fence)
                    }
                ),
                Err(EvalError::SourceMismatch)
            );
        }
    }
    #[test]
    fn three_arm_records_require_complete_matching_fingerprint() {
        let manifest = manifest();
        let config = config(EvalMode::Mock);
        let hash = validate_manifest(&manifest).unwrap();
        let records = config
            .comparisons
            .iter()
            .map(|arm| ArmRecord {
                arm: arm.arm.clone(),
                manifest_hash: hash.clone(),
                source_ids: arm.source_ids.clone(),
                task_ids: arm.task_ids.clone(),
                budget: arm.budget.clone(),
                index_profile_id: arm.index_profile_id.clone(),
                prompt_id: arm.prompt_id.clone(),
                schema_id: arm.schema_id.clone(),
                answer_model_id: arm.answer_model_id.clone(),
            })
            .collect::<Vec<_>>();
        assert!(validate_arm_records(&manifest, &config, &records).unwrap());
        let mut mismatch = records.clone();
        mismatch[2].prompt_id = "changed".into();
        assert!(!validate_arm_records(&manifest, &config, &mismatch).unwrap());
        assert!(!validate_arm_records(&manifest, &config, &records[..2]).unwrap());
    }
    #[test]
    fn replay_stops_at_first_error_and_never_claims_semantic_pass() {
        let artifact = run_mock_or_replay(
            &manifest(),
            &config(EvalMode::Replay),
            &[
                ReplayEvent::ProviderError {
                    code: "local_fake_timeout".into(),
                },
                ReplayEvent::ProviderSuccess,
            ],
        )
        .unwrap();
        assert_eq!(artifact.engineering_status, GateStatus::Fail);
        assert_eq!(artifact.semantic_status, GateStatus::Pending);
        assert_eq!(artifact.errors, vec!["provider_error:local_fake_timeout"]);
    }
    #[test]
    fn artifact_path_is_restricted_and_writes_only_redacted_model() {
        let artifact = run_mock_or_replay(&manifest(), &config(EvalMode::Mock), &[]).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("artifact.json");
        write_artifact(&path, &artifact).unwrap();
        assert!(path.is_file());
        assert_eq!(
            write_artifact(Path::new("vault/artifact.json"), &artifact),
            Err(EvalError::UnsafeArtifactPath)
        );
    }

    #[test]
    fn pack_projection_is_typed_safe_complete_and_hashes_material_changes() {
        let make_pack = |assertion: &str| {
            serde_json::json!({
                "current_context": [{
                    "id":"card-1","origin":"m1","title":"Decision","kind":"decision",
                    "scope_ref":"project","assertion_status":"source_asserted","temporal_scope":{},
                    "core_assertions":[assertion],"conditions":["approved"],"exceptions":[],
                    "ordered_steps":["rollback"],"unresolved_items":[],"optional_details":[],
                    "source_references":[{"source_id":"s","source_revision_id":"r","source_path":"notes/a.md"}],
                    "evidence_refs":["evidence-1"],"note_body":"private raw body"
                }],
                "relevant_experiences":[],"conflicts_or_checks":[],"evidence_gaps":[],
                "related_sources":[{"source_id":"s","source_revision_id":"r","source_path":"notes/a.md"}],
                "diagnostics":[],"estimated_tokens":20
            })
        };
        let first = project_pack_projection(&make_pack("preserve rollback")).unwrap();
        let second = project_pack_projection(&make_pack("different assertion")).unwrap();
        assert_ne!(first["pack_hash"], second["pack_hash"]);
        assert_eq!(first["current_context"].as_array().unwrap().len(), 1);
        assert_eq!(first["current_context"][0]["conditions"][0], "approved");
        assert_eq!(
            first["current_context"][0]["evidence_refs"][0],
            "evidence-1"
        );
        assert_eq!(first["related_sources"].as_array().unwrap().len(), 1);
        assert!(!first.to_string().contains("private raw body"));
        for key in [
            "current_context",
            "relevant_experiences",
            "conflicts_or_checks",
            "evidence_gaps",
            "related_sources",
        ] {
            assert!(first.get(key).is_some(), "missing typed pack section {key}");
        }
        assert!(project_pack_projection(&serde_json::json!({})).is_err());
    }

    fn shadow_config(temp: &tempfile::TempDir) -> (ShadowBuildConfig, ShadowCatchupState) {
        let bytes = b"shadow fixture";
        let mut manifest = manifest();
        manifest.sources[0].content_hash = format!("{:x}", Sha256::digest(bytes));
        let source_root = temp.path().join("source");
        let shadow_root = temp.path().join("shadow");
        let state = temp.path().join("state");
        let history = temp.path().join("history");
        let artifact = temp.path().join("artifact");
        std::fs::create_dir_all(source_root.join("notes")).unwrap();
        std::fs::write(source_root.join("notes/one.md"), bytes).unwrap();
        let hash = validate_manifest(&manifest).unwrap();
        (
            ShadowBuildConfig {
                manifest,
                source_root: source_root.display().to_string(),
                shadow_root: shadow_root.display().to_string(),
                shadow_state: state.display().to_string(),
                shadow_history: history.display().to_string(),
                artifact_root: artifact.display().to_string(),
                source_allowlist: vec!["S01".into(), "file-1".into()],
                outbox_cursor: 4,
                rules_revision: 2,
                shadow_vault_id: "shadow-vault".into(),
            },
            ShadowCatchupState {
                outbox_cursor: 4,
                rules_revision: 2,
                manifest_hash: hash,
            },
        )
    }

    #[test]
    fn shadow_dry_run_copies_only_allowlisted_files_and_keeps_source_unchanged() {
        let temp = tempfile::tempdir().unwrap();
        let (config, current) = shadow_config(&temp);
        let source = std::path::PathBuf::from(&config.source_root).join("notes/one.md");
        let before = std::fs::read(&source).unwrap();
        let plan = prepare_shadow_dry_run(&config, &current).unwrap();
        assert_eq!(plan.copied_logical_ids, vec!["S01"]);
        assert!(plan.source_unchanged && plan.independent_state_paths);
        assert_eq!(std::fs::read(&source).unwrap(), before);
        assert_eq!(
            std::fs::read(std::path::PathBuf::from(&config.shadow_root).join("notes/one.md"))
                .unwrap(),
            before
        );
    }

    #[test]
    fn shadow_config_rejects_overlap_manifest_drift_and_cursor_rules_drift() {
        let temp = tempfile::tempdir().unwrap();
        let (mut config, current) = shadow_config(&temp);
        config.shadow_root = config.source_root.clone();
        assert_eq!(
            prepare_shadow_dry_run(&config, &current),
            Err(EvalError::ShadowConfig(
                "shadow output overlaps source root"
            ))
        );
        let (config, mut current) = shadow_config(&temp);
        current.manifest_hash = "drift".into();
        assert_eq!(
            prepare_shadow_dry_run(&config, &current),
            Err(EvalError::ShadowConfig("source manifest drifted"))
        );
        let (mut config, mut current) = shadow_config(&temp);
        current.outbox_cursor = 5;
        current.rules_revision = 3;
        let plan = prepare_shadow_dry_run(&config, &current).unwrap();
        assert!(plan.needs_replay && plan.needs_rebuild);
        config.source_allowlist.push("outside".into());
        assert_eq!(
            prepare_shadow_dry_run(&config, &current),
            Err(EvalError::ShadowConfig(
                "source allowlist does not exactly cover manifest"
            ))
        );
    }

    #[cfg(unix)]
    #[test]
    fn shadow_dry_run_rejects_source_and_output_symlinks_and_real_overlap() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let (mut config, current) = shadow_config(&temp);
        let real_source = temp.path().join("real-source");
        std::fs::rename(&config.source_root, &real_source).unwrap();
        let source_link = temp.path().join("source-link");
        symlink(&real_source, &source_link).unwrap();
        config.source_root = source_link.display().to_string();
        assert_eq!(
            prepare_shadow_dry_run(&config, &current),
            Err(EvalError::ShadowConfig(
                "symlink path component is forbidden"
            ))
        );

        let temp = tempfile::tempdir().unwrap();
        let (config, current) = shadow_config(&temp);
        let shadow_root = std::path::PathBuf::from(&config.shadow_root);
        let outside = temp.path().join("outside");
        std::fs::create_dir_all(&shadow_root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        symlink(&outside, shadow_root.join("notes")).unwrap();
        assert_eq!(
            prepare_shadow_dry_run(&config, &current),
            Err(EvalError::ShadowConfig(
                "symlink path component is forbidden"
            ))
        );
        assert!(!outside.join("one.md").exists());

        let temp = tempfile::tempdir().unwrap();
        let (config, current) = shadow_config(&temp);
        let source_file = std::path::PathBuf::from(&config.source_root).join("notes/one.md");
        let real_file = source_file.with_extension("real");
        std::fs::rename(&source_file, &real_file).unwrap();
        symlink(&real_file, &source_file).unwrap();
        assert_eq!(
            prepare_shadow_dry_run(&config, &current),
            Err(EvalError::ShadowConfig(
                "symlink path component is forbidden"
            ))
        );

        let temp = tempfile::tempdir().unwrap();
        let (mut config, current) = shadow_config(&temp);
        let real_parent = temp.path().join("real-parent");
        std::fs::create_dir_all(&real_parent).unwrap();
        let link_parent = temp.path().join("link-parent");
        symlink(&real_parent, &link_parent).unwrap();
        config.shadow_root = link_parent.join("shadow").display().to_string();
        assert_eq!(
            prepare_shadow_dry_run(&config, &current),
            Err(EvalError::ShadowConfig(
                "symlink path component is forbidden"
            ))
        );

        let temp = tempfile::tempdir().unwrap();
        let (mut config, current) = shadow_config(&temp);
        let alias_parent = temp.path().join("alias-parent");
        std::fs::create_dir_all(&alias_parent).unwrap();
        config.shadow_root = alias_parent.join("..").join("source").display().to_string();
        assert_eq!(
            prepare_shadow_dry_run(&config, &current),
            Err(EvalError::ShadowConfig(
                "shadow output overlaps source root"
            ))
        );
    }
}
