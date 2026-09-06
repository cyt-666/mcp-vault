//! Bounded real-Provider calibration against non-private bundled documents.
use crate::{
    MemoryError, MemoryService, markdown,
    service::{bounded_memory_embedding_text, memory_embedding_inputs_for_current_fields},
};
use mcp_vault_domain::{
    FileId, JobId, MemoryId, ModelId, Revision, VaultContext, VaultPath, WritePrecondition,
};
use mcp_vault_indexer::{
    note_evaluation_inputs, note_evaluation_query,
    relevance::{LEXICAL_RELEVANCE_PROFILE, calibrated_semantic_rank_score, lexical_relevance},
};
use mcp_vault_providers::{EmbeddingRequest, ProviderError, ProviderMode, RequestBudget};
use mcp_vault_state::{CalibrationBudget, CalibrationRun, NoteEmbeddingSourceRecord, StateStore};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
};

const CORPUS: &str = include_str!("../../../tests/fixtures/memory-quality/calibration.json");
const POLICY: &str = "recall-admission-object-rank-v5-joint";
const MAINTENANCE: &str = "retrieval.calibration.automatic";
const ACTIVE: &str = "retrieval.calibration.active";

mod diagnostics;
pub use diagnostics::diagnose_calibration;

#[derive(Clone, Deserialize)]
struct Corpus {
    version: String,
    documents: Vec<Document>,
    queries: Vec<Query>,
}
#[derive(Clone, Deserialize)]
struct Document {
    id: String,
    split: String,
    content: String,
    title: String,
    path: String,
}
#[derive(Clone, Deserialize)]
struct Query {
    id: String,
    split: String,
    query: String,
    relevant: Vec<String>,
    pure_semantic: bool,
    cross_language: bool,
}

/// Exact configured calibration channel. No private source content is included.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CalibrationProfile {
    /// memory or note.
    pub channel: String,
    /// Internal model identity.
    pub model_id: ModelId,
    /// Provider model label.
    pub external_model_id: String,
    /// Business embedding identity, unchanged by calibration policy changes.
    pub embedding_profile_hash: String,
    /// Vault, channel, input, ranking and benchmark validity identity.
    pub signature: String,
}

/// Honest holdout metrics, with counts rather than an inferred confidence.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CalibrationMetrics {
    /// Queries that have labeled answers.
    pub answered_queries: u32,
    /// Queries with no answer anywhere in the benchmark corpus.
    pub unanswered_queries: u32,
    /// Mean per-query fraction of relevant objects in Top-5.
    pub recall_at_5: f64,
    /// Mean precision of returned Top-5; an empty answer scores zero.
    pub returned_precision_at_5: f64,
    /// Mean reciprocal rank of the first correct object.
    pub mrr_at_5: f64,
    /// Answerable cross-language queries without lexical admission.
    pub pure_semantic_queries: u32,
    /// Pure semantic subset Recall@5.
    pub pure_semantic_recall_at_5: f64,
    /// Cross-language subset size.
    pub cross_language_queries: u32,
    /// Cross-language subset Recall@5.
    pub cross_language_recall_at_5: f64,
    /// Absolute count of no-answer queries returning any object.
    pub no_answer_false_returns: u32,
    /// No-answer false returns divided by no-answer queries.
    pub no_answer_false_return_rate: f64,
    /// Content-free case identifiers requiring inspection.
    pub failed_cases: Vec<String>,
}
impl CalibrationMetrics {
    fn passes(&self) -> bool {
        self.answered_queries >= 40
            && self.unanswered_queries >= 20
            && self.recall_at_5 >= 0.70
            && self.returned_precision_at_5 >= 0.80
            && self.no_answer_false_return_rate <= 0.05
            && self.pure_semantic_queries > 0
            && self.pure_semantic_recall_at_5 >= 0.70
    }
}

/// Server-produced result; never an assertion about private Vault quality.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CalibrationReport {
    /// Explicit evaluation scope.
    pub evaluation_scope: String,
    /// Execution path provenance, independent of an adapter's model name.
    pub execution: String,
    /// Exact role/model/input signature.
    pub profile: CalibrationProfile,
    /// Bundled corpus version.
    pub benchmark_version: String,
    /// Exact bundled labels and text fingerprint.
    pub benchmark_hash: String,
    /// Frozen threshold selected using calibration split only.
    pub min_cosine: f64,
    /// Calibration-split metrics.
    pub calibration: CalibrationMetrics,
    /// Independent holdout metrics.
    pub holdout: CalibrationMetrics,
    /// Lexical control on the same holdout.
    pub lexical_holdout: CalibrationMetrics,
    /// Whether this channel's quality gates passed; combined eligibility is in status.
    pub passed: bool,
    /// Unique embedding payloads in this channel.
    pub embedding_inputs: usize,
    /// Actual accounted HTTP attempts, including retries.
    pub requests: i64,
    /// Actual accounted serialized request bytes.
    pub request_bytes: i64,
    /// Package build version; deployment reports should also retain the build commit.
    #[serde(default)]
    pub implementation_version: String,
    /// Build source revision, or unknown for an archive without build metadata.
    #[serde(default)]
    pub implementation_commit: String,
    /// Whether the build checkout had changes; unknown for source archives.
    #[serde(default)]
    pub implementation_source_state: String,
    /// Wall time since admission, including restart downtime.
    #[serde(default)]
    pub elapsed_ms: i64,
    /// Completion timestamp.
    pub evaluated_at: i64,
}

/// Admin-visible channel status; does not serialize embedding checkpoints.
#[derive(Clone, Debug, Serialize)]
pub struct CalibrationStatus {
    /// Channel requested.
    pub channel: String,
    /// Effective configured signature, if available.
    pub profile: Option<CalibrationProfile>,
    /// Maintenance authorization setting (independent of active query state).
    pub automatic: bool,
    /// Whether a current server evaluation is applicable and passed.
    pub active: bool,
    /// Redacted reasons explaining unavailable preparation.
    pub blockers: Vec<String>,
    /// Persisted execution state and counters.
    pub run: Option<CalibrationRun>,
    /// Applicable server-produced report, if available.
    pub report: Option<CalibrationReport>,
    /// Union of no-answer failures when both configured channels have reports.
    pub joint_no_answer: Option<JointNoAnswer>,
}

/// A query is a false return if either channel returns an object.
#[derive(Clone, Debug, Serialize)]
pub struct JointNoAnswer {
    /// Holdout no-answer sample count.
    pub queries: usize,
    /// Distinct query identifiers, never private content.
    pub failed_cases: Vec<String>,
    /// Union failure count divided by sample count.
    pub false_return_rate: f64,
    /// The same five-percent gate applied to the combined result.
    pub passed: bool,
}

fn joint_no_answer(left: &CalibrationMetrics, right: &CalibrationMetrics) -> JointNoAnswer {
    let corpus: Corpus = serde_json::from_str(CORPUS).expect("bundled calibration corpus");
    let queries: Vec<_> = corpus
        .queries
        .iter()
        .filter(|query| query.split == "holdout" && query.relevant.is_empty())
        .collect();
    let failed_cases: Vec<_> = queries
        .iter()
        .filter(|query| {
            left.failed_cases.contains(&query.id) || right.failed_cases.contains(&query.id)
        })
        .map(|query| query.id.clone())
        .collect();
    let rate = failed_cases.len() as f64 / queries.len().max(1) as f64;
    JointNoAnswer {
        queries: queries.len(),
        failed_cases,
        false_return_rate: rate,
        passed: queries.len() >= 20 && rate <= 0.05,
    }
}

struct DurableBudget {
    state: StateStore,
    context: VaultContext,
    profile: CalibrationProfile,
    deadline_ms: i64,
}
#[async_trait::async_trait]
impl RequestBudget for DurableBudget {
    async fn reserve(&self, bytes: usize) -> Result<(), ProviderError> {
        if self
            .state
            .settings()
            .get_vault(&self.context, MAINTENANCE)
            .await?
            .is_some_and(|setting| setting.value.as_bool() == Some(false))
        {
            return Err(ProviderError::Transport {
                code: "calibration_maintenance_disabled",
                retryable: false,
            });
        }
        self.state
            .calibrations()
            .reserve_request(
                &self.context,
                &self.profile.channel,
                &self.profile.signature,
                bytes,
                self.deadline_ms,
            )
            .await
            .map_err(|error| match error {
                mcp_vault_state::StateError::InvalidInput(_) => ProviderError::Transport {
                    code: "calibration_budget_exhausted_or_stopped",
                    retryable: false,
                },
                _ => ProviderError::Transport {
                    code: "calibration_accounting_unavailable",
                    retryable: true,
                },
            })
    }
}

impl MemoryService {
    /// Describe the policies and currently applicable channel reports without
    /// changing embedding inputs or expiring valid business vectors.
    pub(crate) async fn effective_retrieval_hash(
        &self,
        context: &VaultContext,
    ) -> Result<String, MemoryError> {
        let memory = self.calibration_status(context, "memory").await?;
        let note = self.calibration_status(context, "note").await?;
        Ok(hash(
            &json!({"policy":POLICY,"lexical":LEXICAL_RELEVANCE_PROFILE,
            "memory":memory.profile,"memory_active":memory.active,
            "note":note.profile,"note_active":note.active})
            .to_string(),
        ))
    }

    /// Resolve a channel without reading private notes or requiring extraction.
    pub async fn calibration_profile(
        &self,
        context: &VaultContext,
        channel: &str,
    ) -> Result<Option<CalibrationProfile>, MemoryError> {
        if !matches!(channel, "memory" | "note") {
            return Err(MemoryError::InvalidInput("invalid calibration channel"));
        }
        let Some(binding) = self
            .state
            .providers()
            .resolve_binding(context, &format!("embedding_{channel}"))
            .await?
        else {
            return Ok(None);
        };
        let model = self
            .state
            .providers()
            .get_model(binding.model_id)
            .await?
            .ok_or(MemoryError::NotFound)?;
        let embedding_profile_hash = self.providers.embeddings().profile_hash(model.id).await?;
        let signature = hash(
            &json!({"vault":context.id(),"channel":channel,"embedding":embedding_profile_hash,
            "query_input":if channel=="memory" {"memory-query-2048-v1"}else{"note-query-v3"},
            "document_input":if channel=="memory" {"body-v3"}else{"text-v3"},"policy":POLICY,
            "lexical":LEXICAL_RELEVANCE_PROFILE,"benchmark":hash(CORPUS)})
            .to_string(),
        );
        Ok(Some(CalibrationProfile {
            channel: channel.into(),
            model_id: model.id,
            external_model_id: model.external_model_id,
            embedding_profile_hash,
            signature,
        }))
    }

    /// Read preparation status only. Opening Admin never schedules work.
    pub async fn calibration_status(
        &self,
        context: &VaultContext,
        channel: &str,
    ) -> Result<CalibrationStatus, MemoryError> {
        let automatic = self
            .state
            .settings()
            .get_vault(context, MAINTENANCE)
            .await?
            .is_none_or(|setting| setting.value.as_bool() != Some(false));
        let profile = self.calibration_profile(context, channel).await?;
        let mut status = CalibrationStatus {
            channel: channel.into(),
            profile: profile.clone(),
            automatic,
            active: false,
            blockers: Vec::new(),
            run: None,
            report: None,
            joint_no_answer: None,
        };
        let Some(profile) = profile else {
            status.blockers.push("model_binding_missing".into());
            return Ok(status);
        };
        if self.providers.provider_mode(context).await? == ProviderMode::Disabled {
            status.blockers.push("provider_disabled".into());
        }
        let model = self
            .state
            .providers()
            .get_model(profile.model_id)
            .await?
            .ok_or(MemoryError::NotFound)?;
        let provider = self
            .state
            .providers()
            .get_provider(model.provider_id)
            .await?
            .ok_or(MemoryError::NotFound)?;
        if !model.enabled || !provider.enabled {
            status.blockers.push("model_or_provider_disabled".into());
        }
        status.run = self
            .state
            .calibrations()
            .get(context, channel, &profile.signature)
            .await?;
        let active_key = format!("{ACTIVE}.{channel}.{}", profile.signature);
        if let Some(setting) = self
            .state
            .settings()
            .get_vault(context, &active_key)
            .await?
            && let Ok(report) = serde_json::from_value::<CalibrationReport>(setting.value)
            && report.profile.signature == profile.signature
            && report.passed
            && report.holdout.passes()
            && report.execution == "server_provider_adapter"
            && report.benchmark_hash == hash(CORPUS)
        {
            status.report = Some(report);
        }
        if status.report.is_none() {
            status.blockers.push(
                status
                    .run
                    .as_ref()
                    .map_or("calibration_missing", |run| run.status.as_str())
                    .to_owned(),
            );
        }
        // Check both publications on every eligibility read. This also closes
        // simultaneous publication races without recursive status resolution.
        if let Some(report) = &status.report
            && let Some(other) = self
                .calibration_profile(
                    context,
                    if channel == "memory" {
                        "note"
                    } else {
                        "memory"
                    },
                )
                .await?
            && let Some(setting) = self
                .state
                .settings()
                .get_vault(
                    context,
                    &format!("{ACTIVE}.{}.{}", other.channel, other.signature),
                )
                .await?
            && let Ok(other_report) = serde_json::from_value::<CalibrationReport>(setting.value)
            && other_report.profile.signature == other.signature
            && other_report.passed
            && other_report.holdout.passes()
            && other_report.execution == "server_provider_adapter"
            && other_report.benchmark_hash == hash(CORPUS)
        {
            let joint = joint_no_answer(&report.holdout, &other_report.holdout);
            if !joint.passed {
                status
                    .blockers
                    .push("joint_no_answer_quality_failed".into());
            }
            status.joint_no_answer = Some(joint);
        }
        status.active = status.report.is_some() && status.blockers.is_empty();
        Ok(status)
    }

    /// Explicitly retry selected configured channels. No caller-supplied metrics.
    pub async fn request_retrieval_calibration(
        &self,
        context: &VaultContext,
        channel: &str,
    ) -> Result<Option<JobId>, MemoryError> {
        if !matches!(channel, "all" | "memory" | "note") {
            return Err(MemoryError::InvalidInput("invalid calibration channel"));
        }
        if let Some(job) = self
            .state
            .jobs()
            .find_active_by_type(context, "retrieval.calibrate")
            .await?
        {
            return Ok(Some(job.id));
        }
        let mut signatures = Vec::new();
        for selected in ["memory", "note"] {
            if channel != "all" && channel != selected {
                continue;
            }
            let status = self.calibration_status(context, selected).await?;
            if !status.automatic
                || status.blockers.iter().any(|value| {
                    matches!(
                        value.as_str(),
                        "provider_disabled" | "model_or_provider_disabled"
                    )
                })
            {
                continue;
            }
            let Some(profile) = status.profile else {
                continue;
            };
            self.state
                .calibrations()
                .retry(context, selected, &profile.signature)
                .await?;
            signatures.push(profile.signature);
        }
        if signatures.is_empty() {
            return Ok(None);
        }
        let dedup = format!("retrieval-calibration:manual:{}", JobId::new());
        Ok(Some(
            self.state
                .jobs()
                .enqueue_singleton(
                    context,
                    "retrieval.calibrate",
                    &dedup,
                    &json!({"signatures":signatures,"explicit_diagnostic":true}),
                    0,
                    3,
                    now(),
                )
                .await?
                .id,
        ))
    }

    /// Configure future automatic rounds without altering quality thresholds.
    pub async fn set_calibration_budget(
        &self,
        context: &VaultContext,
        budget: CalibrationBudget,
    ) -> Result<(), MemoryError> {
        budget.validate()?;
        self.state
            .settings()
            .set_vault(
                context,
                "retrieval.calibration.budget",
                &json!(budget),
                WritePrecondition::Unconditional,
                None,
            )
            .await?;
        Ok(())
    }

    /// Cancel the durable job and stop even a queued, not-yet-started signature.
    pub async fn cancel_retrieval_calibration(
        &self,
        context: &VaultContext,
        job_id: JobId,
    ) -> Result<(), MemoryError> {
        let job = self
            .state
            .jobs()
            .get(context, job_id)
            .await?
            .ok_or(MemoryError::NotFound)?;
        if job.job_type != "retrieval.calibrate" {
            return Err(MemoryError::InvalidInput("not a calibration job"));
        }
        self.state.jobs().request_cancel(context, job_id).await?;
        for channel in ["memory", "note"] {
            if let Some(profile) = self.calibration_profile(context, channel).await?
                && job.payload["signatures"].as_array().is_some_and(|items| {
                    items
                        .iter()
                        .any(|item| item.as_str() == Some(&profile.signature))
                })
            {
                self.state
                    .calibrations()
                    .ensure(context, channel, &profile.signature)
                    .await?;
                self.state
                    .calibrations()
                    .cancel(context, channel, &profile.signature)
                    .await?;
            }
        }
        Ok(())
    }

    /// Pause automatic maintenance independently of semantic query eligibility.
    pub async fn set_calibration_maintenance(
        &self,
        context: &VaultContext,
        enabled: bool,
    ) -> Result<(), MemoryError> {
        self.state
            .settings()
            .set_vault(
                context,
                MAINTENANCE,
                &json!(enabled),
                WritePrecondition::Unconditional,
                None,
            )
            .await?;
        if enabled {
            self.ensure_retrieval_calibration(context).await?;
        }
        Ok(())
    }

    /// One atomic, Vault-singleton job handles both channels serially. Startup,
    /// periodic compensation and Admin run all use this same operation.
    pub async fn ensure_retrieval_calibration(
        &self,
        context: &VaultContext,
    ) -> Result<Option<JobId>, MemoryError> {
        // Kept as a compatible no-op for older event callers. Only explicit
        // diagnostic requests may schedule synthetic inputs (ADR-0028).
        let _ = context;
        Ok(None)
    }

    /// Run the actual configured Provider with bounded cached synthetic inputs.
    /// The worker owns cancellation and invokes this sequentially per channel.
    pub async fn execute_retrieval_calibration(
        &self,
        context: &VaultContext,
        profile: &CalibrationProfile,
    ) -> Result<CalibrationReport, MemoryError> {
        self.check_calibration_profile(context, profile).await?;
        self.state
            .calibrations()
            .prune(context, &profile.channel, &profile.signature)
            .await?;
        let configured: CalibrationBudget = self
            .state
            .settings()
            .get_vault(context, "retrieval.calibration.budget")
            .await?
            .map(|setting| serde_json::from_value(setting.value))
            .transpose()
            .map_err(|_| MemoryError::Configuration("calibration_budget_invalid"))?
            .unwrap_or_default();
        let run = self
            .state
            .calibrations()
            .ensure_with_budget(context, &profile.channel, &profile.signature, &configured)
            .await?;
        let limits: CalibrationBudget = serde_json::from_str(&run.budget_json)
            .map_err(|_| MemoryError::Configuration("calibration_budget_invalid"))?;
        if let Some(report) = run
            .report_json
            .as_deref()
            .and_then(|value| serde_json::from_str::<CalibrationReport>(value).ok())
        {
            if report.passed {
                self.publish_calibration(context, &report).await?;
            }
            return Ok(report);
        }
        if !matches!(run.status.as_str(), "pending" | "running") {
            return Err(MemoryError::Configuration("calibration_stopped"));
        }
        let corpus: Corpus = serde_json::from_str(CORPUS)
            .map_err(|_| MemoryError::Configuration("calibration_benchmark_invalid"))?;
        let prepared = prepare(&corpus, &profile.channel)?;
        if prepared.inputs.len() > limits.max_inputs as usize
            || prepared.inputs.iter().map(String::len).sum::<usize>() > limits.max_bytes as usize
        {
            return Err(MemoryError::Configuration(
                "calibration_input_budget_exhausted",
            ));
        }
        let mut cache: BTreeMap<String, Vec<f32>> = serde_json::from_str(&run.checkpoint_json)
            .map_err(|_| MemoryError::Configuration("calibration_checkpoint_invalid"))?;
        let missing = prepared
            .inputs
            .iter()
            .filter(|input| !cache.contains_key(&hash(input)))
            .cloned()
            .collect::<Vec<_>>();
        let budget = Arc::new(DurableBudget {
            state: self.state.clone(),
            context: context.clone(),
            profile: profile.clone(),
            deadline_ms: i64::from(limits.timeout_seconds) * 1000,
        });
        for batch in missing.chunks(limits.batch_size as usize) {
            self.check_calibration_profile(context, profile).await?;
            let result = self
                .providers
                .embed_with_budget(
                    context,
                    profile.model_id,
                    &EmbeddingRequest {
                        model: profile.external_model_id.clone(),
                        inputs: batch.to_vec(),
                    },
                    budget.clone(),
                )
                .await?;
            if result.vectors.len() != batch.len() {
                return Err(MemoryError::Configuration(
                    "calibration_embedding_count_invalid",
                ));
            }
            for (input, vector) in batch.iter().zip(result.vectors) {
                validate_vector(&vector)?;
                cache.insert(hash(input), vector);
            }
            self.state
                .calibrations()
                .checkpoint(context, &profile.channel, &profile.signature, &json!(cache))
                .await?;
        }
        let scored = score_queries(context, &corpus, &prepared, &cache, &profile.channel).await?;
        let mut best = None;
        for floor in threshold_candidates(&scored) {
            let metrics = evaluate(&scored, "calibration", Some(floor), &profile.channel);
            if metrics.passes()
                && best
                    .as_ref()
                    .is_none_or(|(_, old): &(f64, CalibrationMetrics)| {
                        metrics.recall_at_5 > old.recall_at_5
                            || (metrics.recall_at_5 == old.recall_at_5
                                && metrics.returned_precision_at_5 > old.returned_precision_at_5)
                    })
            {
                best = Some((floor, metrics));
            }
        }
        let (min_cosine, calibration) = best.unwrap_or_else(|| {
            (
                0.999_999,
                evaluate(&scored, "calibration", Some(0.999_999), &profile.channel),
            )
        });
        let holdout = evaluate(&scored, "holdout", Some(min_cosine), &profile.channel);
        let lexical_holdout = evaluate(&scored, "holdout", None, &profile.channel);
        let run = self
            .state
            .calibrations()
            .get(context, &profile.channel, &profile.signature)
            .await?
            .ok_or(MemoryError::Conflict)?;
        let report = CalibrationReport {
            evaluation_scope: "builtin_benchmark".into(),
            execution: "server_provider_adapter".into(),
            profile: profile.clone(),
            benchmark_version: corpus.version,
            benchmark_hash: hash(CORPUS),
            min_cosine,
            passed: calibration.passes()
                && holdout.passes()
                && holdout.recall_at_5 >= lexical_holdout.recall_at_5,
            calibration,
            holdout,
            lexical_holdout,
            embedding_inputs: prepared.inputs.len(),
            requests: run.requests,
            request_bytes: run.request_bytes,
            implementation_version: env!("CARGO_PKG_VERSION").into(),
            implementation_commit: env!("MCP_VAULT_BUILD_COMMIT").into(),
            implementation_source_state: env!("MCP_VAULT_BUILD_SOURCE_STATE").into(),
            elapsed_ms: now().saturating_sub(run.started_at),
            evaluated_at: now(),
        };
        self.check_calibration_profile(context, profile).await?;
        self.state
            .calibrations()
            .finish(
                context,
                &profile.channel,
                &profile.signature,
                if report.passed {
                    "passed"
                } else {
                    "quality_failed"
                },
                &json!(report),
            )
            .await?;
        if report.passed {
            self.publish_calibration(context, &report).await?;
        }
        Ok(report)
    }

    async fn check_calibration_profile(
        &self,
        context: &VaultContext,
        profile: &CalibrationProfile,
    ) -> Result<(), MemoryError> {
        let status = self.calibration_status(context, &profile.channel).await?;
        if !status.automatic {
            return Err(MemoryError::Configuration(
                "calibration_maintenance_disabled",
            ));
        }
        if status
            .profile
            .as_ref()
            .is_none_or(|current| current.signature != profile.signature)
        {
            return Err(MemoryError::Conflict);
        }
        if status.blockers.iter().any(|value| {
            matches!(
                value.as_str(),
                "provider_disabled" | "model_or_provider_disabled"
            )
        }) {
            return Err(MemoryError::Configuration("calibration_provider_disabled"));
        }
        Ok(())
    }
    async fn publish_calibration(
        &self,
        context: &VaultContext,
        report: &CalibrationReport,
    ) -> Result<(), MemoryError> {
        self.check_calibration_profile(context, &report.profile)
            .await?;
        let run = self
            .state
            .calibrations()
            .get(context, &report.profile.channel, &report.profile.signature)
            .await?
            .ok_or(MemoryError::Conflict)?;
        if run.status != "passed"
            || run
                .report_json
                .as_deref()
                .and_then(|value| serde_json::from_str::<serde_json::Value>(value).ok())
                != Some(json!(report))
        {
            return Err(MemoryError::Conflict);
        }
        self.state
            .calibrations()
            .publish(
                context,
                &report.profile.channel,
                &report.profile.signature,
                &json!(report),
            )
            .await?;
        Ok(())
    }
}

struct Prepared {
    inputs: Vec<String>,
    documents: HashMap<String, Vec<String>>,
    queries: HashMap<String, String>,
}
fn prepare(corpus: &Corpus, channel: &str) -> Result<Prepared, MemoryError> {
    let mut result = Prepared {
        inputs: Vec::new(),
        documents: HashMap::new(),
        queries: HashMap::new(),
    };
    for doc in &corpus.documents {
        let inputs = if channel == "memory" {
            memory_embedding_inputs_for_current_fields(
                MemoryId::new(),
                "synthetic",
                &markdown::normalize_content(&doc.content),
            )
            .into_iter()
            .map(|input| input.text)
            .collect()
        } else {
            note_evaluation_inputs(&NoteEmbeddingSourceRecord {
                file_id: FileId::new(),
                path: VaultPath::parse(&doc.path)
                    .map_err(|_| MemoryError::Configuration("calibration_path_invalid"))?,
                revision: Revision::new(1),
                title: Some(doc.title.clone()),
                headings: vec![doc.title.clone()],
                plain_text: format!("{}\n\n{}", doc.title, doc.content),
                analyzed_content_hash: hash(&doc.content),
            })
        };
        result.documents.insert(
            doc.id.clone(),
            inputs.iter().map(|value: &String| hash(value)).collect(),
        );
        result.inputs.extend(inputs);
    }
    for query in &corpus.queries {
        let input = if channel == "memory" {
            bounded_memory_embedding_text(&query.query).to_owned()
        } else {
            note_evaluation_query(&query.query)
        };
        result.queries.insert(query.id.clone(), hash(&input));
        result.inputs.push(input);
    }
    result.inputs.sort();
    result.inputs.dedup();
    Ok(result)
}
struct ScoredQuery<'a> {
    query: &'a Query,
    lexical_ranks: HashMap<String, usize>,
    candidates: Vec<(String, f64, f64)>,
}

fn threshold_candidates(scored: &[ScoredQuery<'_>]) -> Vec<f64> {
    let mut boundaries = vec![0.0, 1.0];
    for query in scored
        .iter()
        .filter(|query| query.query.split == "calibration")
    {
        for (_, cosine, _) in &query.candidates {
            if (0.0..1.0).contains(cosine) {
                boundaries.push(*cosine);
            }
        }
    }
    boundaries.sort_by(f64::total_cmp);
    boundaries.dedup();
    boundaries
        .windows(2)
        .map(|pair| (pair[0] + pair[1]) / 2.0)
        .collect()
}
async fn score_queries<'a>(
    context: &VaultContext,
    corpus: &'a Corpus,
    prepared: &Prepared,
    cache: &BTreeMap<String, Vec<f32>>,
    channel: &str,
) -> Result<Vec<ScoredQuery<'a>>, MemoryError> {
    let mut indexes = HashMap::new();
    for split in ["calibration", "holdout"] {
        let docs = corpus
            .documents
            .iter()
            .filter(|doc| doc.split == split)
            .map(|doc| mcp_vault_state::CalibrationDocument {
                id: doc.id.clone(),
                path: doc.path.clone(),
                title: doc.title.clone(),
                content: doc.content.clone(),
                normalized_content: markdown::normalize_content(&doc.content),
            })
            .collect::<Vec<_>>();
        indexes.insert(
            split,
            mcp_vault_state::CalibrationLexicalIndex::new(context, channel, &docs).await?,
        );
    }
    let mut result = Vec::new();
    for query in &corpus.queries {
        let fts = if channel == "memory" {
            crate::service::quote_fts_query(&query.query)?
        } else {
            mcp_vault_indexer::quote_fts_query_any(&query.query)?
        };
        let lexical_ranks = indexes[query.split.as_str()]
            .candidates(context, &fts)
            .await?
            .into_iter()
            .enumerate()
            .map(|(rank, (id, _))| (id, rank))
            .collect::<HashMap<_, _>>();
        let vector = cache
            .get(&prepared.queries[&query.id])
            .ok_or(MemoryError::Configuration(
                "calibration_checkpoint_incomplete",
            ))?;
        let mut candidates = Vec::new();
        for doc in corpus
            .documents
            .iter()
            .filter(|doc| doc.split == query.split)
        {
            let mut best = -1.0_f64;
            for key in &prepared.documents[&doc.id] {
                best = best.max(cosine(
                    vector,
                    cache.get(key).ok_or(MemoryError::Configuration(
                        "calibration_checkpoint_incomplete",
                    ))?,
                )?);
            }
            let evidence = lexical_relevance(&query.query, &doc.content, &[], &[]);
            candidates.push((
                doc.id.clone(),
                best,
                if evidence.admitted {
                    evidence.coverage
                } else {
                    0.0
                },
            ));
        }
        candidates.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        result.push(ScoredQuery {
            query,
            candidates,
            lexical_ranks,
        });
    }
    Ok(result)
}
fn ranked_candidates<'a>(
    entry: &'a ScoredQuery<'_>,
    floor: Option<f64>,
    channel: &str,
) -> Vec<(&'a String, f64)> {
    let mut ranked = entry
        .candidates
        .iter()
        .enumerate()
        .filter_map(|(rank, (id, similarity, lexical))| {
            let semantic = floor
                .and_then(|floor| calibrated_semantic_rank_score(*similarity as f32, rank, floor));
            let lexical_rank = entry.lexical_ranks.get(id).copied();
            if (*lexical == 0.0 || lexical_rank.is_none()) && semantic.is_none() {
                return None;
            }
            let lexical_score = if *lexical > 0.0 && lexical_rank.is_some() {
                let lexical_rank = lexical_rank.unwrap_or(0);
                if channel == "memory" {
                    {
                        let (coverage, rank) =
                            mcp_vault_indexer::relevance::memory_lexical_contributions(
                                *lexical,
                                lexical_rank,
                            );
                        coverage + rank
                    }
                } else {
                    mcp_vault_indexer::relevance::note_lexical_contribution(lexical_rank)
                }
            } else {
                0.0
            };
            Some((id, lexical_score + semantic.unwrap_or(0.0)))
        })
        .collect::<Vec<_>>();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    ranked.truncate(5);
    ranked
}

fn evaluate(
    queries: &[ScoredQuery<'_>],
    split: &str,
    floor: Option<f64>,
    channel: &str,
) -> CalibrationMetrics {
    let mut metrics = CalibrationMetrics::default();
    for entry in queries.iter().filter(|entry| entry.query.split == split) {
        let ranked = ranked_candidates(entry, floor, channel);
        let query = entry.query;
        if query.relevant.is_empty() {
            metrics.unanswered_queries += 1;
            if !ranked.is_empty() {
                metrics.no_answer_false_returns += 1;
                metrics.failed_cases.push(query.id.clone());
            }
            continue;
        }
        metrics.answered_queries += 1;
        let relevant = ranked
            .iter()
            .filter(|(id, _)| query.relevant.contains(id))
            .count();
        let recall = relevant as f64 / query.relevant.len() as f64;
        metrics.recall_at_5 += recall;
        metrics.returned_precision_at_5 += if ranked.is_empty() {
            0.0
        } else {
            relevant as f64 / ranked.len() as f64
        };
        if let Some(rank) = ranked
            .iter()
            .position(|(id, _)| query.relevant.contains(id))
        {
            metrics.mrr_at_5 += 1.0 / (rank as f64 + 1.0);
        }
        if query.pure_semantic {
            metrics.pure_semantic_queries += 1;
            metrics.pure_semantic_recall_at_5 += recall;
        }
        if query.cross_language {
            metrics.cross_language_queries += 1;
            metrics.cross_language_recall_at_5 += recall;
        }
        if recall < 1.0 || relevant != ranked.len() {
            metrics.failed_cases.push(query.id.clone());
        }
    }
    if metrics.answered_queries > 0 {
        for value in [
            &mut metrics.recall_at_5,
            &mut metrics.returned_precision_at_5,
            &mut metrics.mrr_at_5,
        ] {
            *value /= f64::from(metrics.answered_queries);
        }
    }
    if metrics.pure_semantic_queries > 0 {
        metrics.pure_semantic_recall_at_5 /= f64::from(metrics.pure_semantic_queries);
    }
    if metrics.cross_language_queries > 0 {
        metrics.cross_language_recall_at_5 /= f64::from(metrics.cross_language_queries);
    }
    if metrics.unanswered_queries > 0 {
        metrics.no_answer_false_return_rate =
            f64::from(metrics.no_answer_false_returns) / f64::from(metrics.unanswered_queries);
    }
    metrics
}
fn validate_vector(vector: &[f32]) -> Result<(), MemoryError> {
    if vector.is_empty()
        || vector.iter().any(|x| !x.is_finite())
        || vector.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>() <= f64::EPSILON
    {
        return Err(MemoryError::Configuration("calibration_embedding_invalid"));
    }
    Ok(())
}
fn cosine(a: &[f32], b: &[f32]) -> Result<f64, MemoryError> {
    validate_vector(a)?;
    validate_vector(b)?;
    if a.len() != b.len() {
        return Err(MemoryError::Configuration(
            "calibration_embedding_dimension_mismatch",
        ));
    }
    Ok(f64::from(mcp_vault_providers::exact_cosine_similarity(
        a, b,
    )?))
}
fn hash(value: &str) -> String {
    format!("sha256:{:x}", Sha256::digest(value.as_bytes()))
}
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |v| v.as_millis() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn joint_no_answer_counts_union_and_rejects_disjoint_channel_errors() {
        let corpus: Corpus = serde_json::from_str(CORPUS).unwrap();
        let ids: Vec<_> = corpus
            .queries
            .iter()
            .filter(|q| q.split == "holdout" && q.relevant.is_empty())
            .map(|q| q.id.clone())
            .collect();
        let left = CalibrationMetrics {
            failed_cases: vec![ids[0].clone()],
            ..Default::default()
        };
        let mut right = left.clone();
        let same = joint_no_answer(&left, &right);
        assert!(same.passed);
        assert_eq!(same.false_return_rate, 0.05);
        right.failed_cases = vec![ids[1].clone()];
        let disjoint = joint_no_answer(&left, &right);
        assert!(!disjoint.passed);
        assert_eq!(disjoint.failed_cases.len(), 2);
        assert_eq!(disjoint.false_return_rate, 0.10);
    }
    #[test]
    fn builtin_benchmark_has_disjoint_sources_and_true_semantic_subsets() {
        let corpus: Corpus = serde_json::from_str(CORPUS).unwrap();
        assert_eq!(corpus.queries.len(), 120);
        for split in ["calibration", "holdout"] {
            let queries = corpus
                .queries
                .iter()
                .filter(|query| query.split == split)
                .collect::<Vec<_>>();
            assert_eq!(queries.len(), 60);
            assert_eq!(
                queries
                    .iter()
                    .filter(|query| query.relevant.is_empty())
                    .count(),
                20
            );
            for query in queries {
                for id in &query.relevant {
                    let doc = corpus.documents.iter().find(|doc| doc.id == *id).unwrap();
                    assert_eq!(doc.split, split);
                    if query.pure_semantic {
                        assert!(
                            !lexical_relevance(&query.query, &doc.content, &[], &[]).admitted,
                            "{} is not pure semantic",
                            query.id
                        );
                    }
                }
            }
        }
    }
    #[test]
    fn calibration_rejects_zero_nan_mismatched_embeddings() {
        assert!(validate_vector(&[0.0, 0.0]).is_err());
        assert!(validate_vector(&[f32::NAN, 1.0]).is_err());
        assert!(validate_vector(&[f32::INFINITY]).is_err());
        assert!(cosine(&[1.0, 0.0], &[1.0]).is_err());
    }
    #[test]
    fn empty_answer_does_not_pass_quality_and_recall_is_not_hit_rate() {
        let query = Query {
            id: "multiple".into(),
            split: "holdout".into(),
            query: "query".into(),
            relevant: vec!["a".into(), "b".into()],
            pure_semantic: true,
            cross_language: true,
        };
        let scored = vec![ScoredQuery {
            query: &query,
            lexical_ranks: HashMap::new(),
            candidates: vec![("a".into(), 0.9, 0.0), ("b".into(), 0.2, 0.0)],
        }];
        let partial = evaluate(&scored, "holdout", Some(0.8), "memory");
        assert_eq!(partial.recall_at_5, 0.5);
        assert_eq!(partial.returned_precision_at_5, 1.0);
        let empty = evaluate(&scored, "holdout", None, "memory");
        assert_eq!(empty.recall_at_5, 0.0);
        assert_eq!(empty.returned_precision_at_5, 0.0);
        assert!(!empty.passes());
    }
}
