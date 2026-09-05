//! Deterministic memory-quality regression runner.
//!
//! Retrieval cases execute the real local `MemoryService::recall` path against
//! an isolated in-memory database. Generation cases evaluate labeled fake
//! Provider output only; they prove fixture/schema accounting, not model
//! semantic quality. Real-provider evaluation is deliberately not implemented
//! by this default command because it requires explicit data/cost consent.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    env, fs,
    path::{Path, PathBuf},
    time::Instant,
};

use mcp_vault_auth::{AuthService, MasterKeyRing};
use mcp_vault_core::VaultCore;
use mcp_vault_domain::{Revision, VaultContext, VaultId, VaultSlug};
use mcp_vault_memory::{
    EXTRACTION_PIPELINE_VERSION, MemoryOrigin, MemoryService, MemoryType, RecallRequest,
    RememberInput,
};
use mcp_vault_state::{StateStore, VaultStatus};
use mcp_vault_storage_fs::StorageOptions;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

#[derive(Debug, Deserialize)]
struct Corpus {
    version: u32,
    memories: Vec<FixtureMemory>,
    retrieval_queries: Vec<RetrievalCase>,
    generation_cases: Vec<GenerationCase>,
}

#[derive(Debug, Deserialize)]
struct FixtureMemory {
    key: String,
    memory_type: String,
    content: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    entities: Vec<String>,
    #[serde(default = "default_score")]
    importance: f64,
}

#[derive(Debug, Deserialize)]
struct RetrievalCase {
    id: String,
    query: String,
    #[serde(default)]
    expected: Vec<String>,
    language: String,
    split: String,
    #[serde(default)]
    hard_negative: bool,
}

#[derive(Debug, Deserialize)]
struct GenerationCase {
    id: String,
    split: String,
    source: String,
    expected_fact_ids: Vec<String>,
    legacy_candidates: Vec<GeneratedCandidate>,
    current_candidates: Vec<GeneratedCandidate>,
}

#[derive(Clone, Debug, Deserialize)]
struct GeneratedCandidate {
    content: String,
    #[serde(default)]
    fact_ids: Vec<String>,
    supported: bool,
    #[serde(default)]
    subject_error: bool,
    #[serde(default)]
    condition_error: bool,
    #[serde(default)]
    type_error: bool,
}

#[derive(Debug, Serialize)]
struct RetrievalDetail {
    id: String,
    language: String,
    split: String,
    expected: Vec<String>,
    returned: Vec<String>,
    reciprocal_rank: f64,
    passed: bool,
    hard_negative: bool,
    latency_ms: u128,
    candidate_memory_count: u32,
    relevant_memory_count: u32,
    truncated: bool,
    degraded: Vec<String>,
}

#[derive(Debug, Serialize)]
struct GenerationDetail {
    id: String,
    split: String,
    source_bytes: usize,
    expected_facts: usize,
    returned_items: usize,
    supported_items: usize,
    covered_facts: usize,
    subject_errors: usize,
    condition_errors: usize,
    type_errors: usize,
    duplicate_items: usize,
}

fn default_score() -> f64 {
    0.5
}

fn parse_args() -> Result<(PathBuf, PathBuf), String> {
    let mut mode = None;
    let mut fixtures = None;
    let mut output = None;
    let mut args = env::args().skip(1);
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--mode" => mode = args.next(),
            "--fixtures" => fixtures = args.next().map(PathBuf::from),
            "--output" => output = args.next().map(PathBuf::from),
            "--help" | "-h" => {
                return Err(
                    "usage: quality_eval --mode deterministic --fixtures <dir> --output <file>"
                        .to_owned(),
                );
            }
            _ => return Err(format!("unknown argument: {argument}")),
        }
    }
    if mode.as_deref() != Some("deterministic") {
        return Err(
            "only --mode deterministic is safe by default; real-provider evaluation requires a separate explicitly authorized runner"
                .to_owned(),
        );
    }
    Ok((
        fixtures.ok_or_else(|| "--fixtures is required".to_owned())?,
        output.ok_or_else(|| "--output is required".to_owned())?,
    ))
}

fn memory_type(value: &str) -> Result<MemoryType, String> {
    MemoryType::try_from(value).map_err(|_| format!("unknown fixture memory type: {value}"))
}

async fn evaluate_retrieval(
    corpus: &Corpus,
) -> Result<(Vec<RetrievalDetail>, Value, Value), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let state = StateStore::connect_and_migrate("sqlite::memory:").await?;
    let context = VaultContext::new(
        VaultId::new(),
        VaultSlug::new("quality-eval")?,
        directory.path().join("vault"),
        Revision::ZERO,
    )?;
    state
        .vaults()
        .insert(&context, "Quality evaluation", VaultStatus::Active)
        .await?;
    let auth = AuthService::new(state.auth(), MasterKeyRing::from_bytes(1, &[19_u8; 32])?);
    let service = MemoryService::new(state.clone(), auth);
    let core = VaultCore::new(
        state.clone(),
        directory.path().join("history"),
        Default::default(),
        StorageOptions::default(),
        Default::default(),
    );
    let mut memory_keys = HashMap::new();
    let mut reverse_keys = HashMap::new();
    let mut metadata_checked = 0_usize;
    let mut metadata_exact = 0_usize;
    for fixture in &corpus.memories {
        let expected_type = memory_type(&fixture.memory_type)?;
        let stored = service
            .remember(
                &context,
                &core,
                RememberInput {
                    content: fixture.content.clone(),
                    memory_type: Some(expected_type),
                    importance: Some(fixture.importance),
                    confidence: None,
                    tags: fixture.tags.clone(),
                    entities: fixture.entities.clone(),
                    idempotency_key: Some(format!("quality-eval:{}", fixture.key)),
                    origin: MemoryOrigin::ExplicitAdmin,
                    ..RememberInput::default()
                },
            )
            .await?
            .memory
            .ok_or("quality fixture was not stored")?;
        metadata_checked += 1;
        if stored.memory_type == Some(expected_type)
            && stored.importance == Some(fixture.importance)
            && stored.confidence.is_none()
            && stored.tags == fixture.tags
            && stored.entities == fixture.entities
            && stored.valid_from.is_none()
            && stored.valid_to.is_none()
        {
            metadata_exact += 1;
        }
        let id = stored.id;
        memory_keys.insert(fixture.key.clone(), id);
        reverse_keys.insert(id, fixture.key.clone());
    }

    let mut details = Vec::with_capacity(corpus.retrieval_queries.len());
    for case in &corpus.retrieval_queries {
        if case
            .expected
            .iter()
            .any(|key| !memory_keys.contains_key(key))
        {
            return Err(format!("{} references an unknown expected memory", case.id).into());
        }
        let started = Instant::now();
        let result = service
            .recall(
                &context,
                RecallRequest {
                    query: case.query.clone(),
                    include_related_notes: false,
                    max_results: 5,
                    max_tokens: 1_200,
                    ..RecallRequest::default()
                },
            )
            .await?;
        let returned = result
            .memories
            .iter()
            .filter_map(|memory| reverse_keys.get(&memory.id).cloned())
            .collect::<Vec<_>>();
        let expected = case.expected.iter().cloned().collect::<HashSet<_>>();
        let rank = returned
            .iter()
            .position(|key| expected.contains(key))
            .map(|index| 1.0 / (index as f64 + 1.0))
            .unwrap_or(0.0);
        let passed = if expected.is_empty() {
            returned.is_empty()
        } else {
            rank > 0.0
        };
        details.push(RetrievalDetail {
            id: case.id.clone(),
            language: case.language.clone(),
            split: case.split.clone(),
            expected: case.expected.clone(),
            returned,
            reciprocal_rank: rank,
            passed,
            hard_negative: case.hard_negative,
            latency_ms: started.elapsed().as_millis(),
            candidate_memory_count: result.candidate_memory_count,
            relevant_memory_count: result.relevant_memory_count,
            truncated: result.truncated,
            degraded: result.degraded,
        });
    }
    let answered = details
        .iter()
        .filter(|detail| !detail.expected.is_empty())
        .collect::<Vec<_>>();
    let unanswered = details
        .iter()
        .filter(|detail| detail.expected.is_empty())
        .collect::<Vec<_>>();
    let recall_at_5 = answered.iter().filter(|detail| detail.passed).count() as f64
        / answered.len().max(1) as f64;
    let mrr_at_5 = answered
        .iter()
        .map(|detail| detail.reciprocal_rank)
        .sum::<f64>()
        / answered.len().max(1) as f64;
    let false_return_rate = unanswered
        .iter()
        .filter(|detail| !detail.returned.is_empty())
        .count() as f64
        / unanswered.len().max(1) as f64;
    let mut language_groups = BTreeMap::new();
    for language in details
        .iter()
        .map(|detail| detail.language.as_str())
        .collect::<std::collections::BTreeSet<_>>()
    {
        let group = details
            .iter()
            .filter(|detail| detail.language == language)
            .collect::<Vec<_>>();
        let group_answered = group
            .iter()
            .filter(|detail| !detail.expected.is_empty())
            .collect::<Vec<_>>();
        let group_unanswered = group
            .iter()
            .filter(|detail| detail.expected.is_empty())
            .collect::<Vec<_>>();
        language_groups.insert(
            language,
            json!({
                "queries": group.len(),
                "answered_queries": group_answered.len(),
                "unanswered_queries": group_unanswered.len(),
                "recall_at_5": group_answered.iter().filter(|detail| detail.passed).count() as f64
                    / group_answered.len().max(1) as f64,
                "mrr_at_5": group_answered.iter().map(|detail| detail.reciprocal_rank).sum::<f64>()
                    / group_answered.len().max(1) as f64,
                "no_answer_false_return_rate": group_unanswered.iter()
                    .filter(|detail| !detail.returned.is_empty()).count() as f64
                    / group_unanswered.len().max(1) as f64,
            }),
        );
    }
    let mut latencies = details
        .iter()
        .map(|detail| detail.latency_ms)
        .collect::<Vec<_>>();
    latencies.sort_unstable();
    let percentile = |percent: usize| -> u128 {
        let index = latencies.len().saturating_sub(1).saturating_mul(percent) / 100;
        latencies.get(index).copied().unwrap_or_default()
    };
    let metrics = json!({
        "queries": details.len(),
        "request_count": details.len(),
        "provider_request_count": 0,
        "answered_queries": answered.len(),
        "unanswered_queries": unanswered.len(),
        "hard_negatives": details.iter().filter(|detail| detail.hard_negative).count(),
        "recall_at_5": recall_at_5,
        "mrr_at_5": mrr_at_5,
        "no_answer_false_return_rate": false_return_rate,
        "passed": details.iter().filter(|detail| detail.passed).count(),
        "language_groups": language_groups,
        "latency_ms": {
            "p50": percentile(50),
            "p95": percentile(95),
            "max": latencies.last().copied().unwrap_or_default(),
        },
    });
    let metadata = json!({
        "records_checked": metadata_checked,
        "exact_round_trips": metadata_exact,
        "exact_round_trip_rate": metadata_exact as f64 / metadata_checked.max(1) as f64,
        "fields": ["memory_type", "importance", "confidence", "tags", "entities", "valid_from", "valid_to"],
    });
    Ok((details, metrics, metadata))
}

fn evaluate_generation(corpus: &Corpus) -> (Vec<GenerationDetail>, Value) {
    let use_current = EXTRACTION_PIPELINE_VERSION > 10;
    let mut details = Vec::with_capacity(corpus.generation_cases.len());
    for case in &corpus.generation_cases {
        let candidates = if use_current {
            &case.current_candidates
        } else {
            &case.legacy_candidates
        };
        let expected = case
            .expected_fact_ids
            .iter()
            .cloned()
            .collect::<HashSet<_>>();
        let covered = candidates
            .iter()
            .flat_map(|candidate| candidate.fact_ids.iter().cloned())
            .filter(|fact| expected.contains(fact))
            .collect::<HashSet<_>>();
        let mut unique_content = HashSet::new();
        let duplicates = candidates
            .iter()
            .filter(|candidate| !unique_content.insert(candidate.content.trim().to_lowercase()))
            .count();
        details.push(GenerationDetail {
            id: case.id.clone(),
            split: case.split.clone(),
            source_bytes: case.source.len(),
            expected_facts: expected.len(),
            returned_items: candidates.len(),
            supported_items: candidates
                .iter()
                .filter(|candidate| candidate.supported)
                .count(),
            covered_facts: covered.len(),
            subject_errors: candidates
                .iter()
                .filter(|candidate| candidate.subject_error)
                .count(),
            condition_errors: candidates
                .iter()
                .filter(|candidate| candidate.condition_error)
                .count(),
            type_errors: candidates
                .iter()
                .filter(|candidate| candidate.type_error)
                .count(),
            duplicate_items: duplicates,
        });
    }
    let returned = details
        .iter()
        .map(|detail| detail.returned_items)
        .sum::<usize>();
    let supported = details
        .iter()
        .map(|detail| detail.supported_items)
        .sum::<usize>();
    let expected = details
        .iter()
        .map(|detail| detail.expected_facts)
        .sum::<usize>();
    let covered = details
        .iter()
        .map(|detail| detail.covered_facts)
        .sum::<usize>();
    let metrics = json!({
        "cases": details.len(),
        "profile": if use_current { "current-memory-set-fake" } else { "legacy-two-phase-fake" },
        "support_precision": supported as f64 / returned.max(1) as f64,
        "critical_fact_coverage": covered as f64 / expected.max(1) as f64,
        "subject_errors": details.iter().map(|detail| detail.subject_errors).sum::<usize>(),
        "condition_errors": details.iter().map(|detail| detail.condition_errors).sum::<usize>(),
        "type_errors": details.iter().map(|detail| detail.type_errors).sum::<usize>(),
        "duplicate_items": details.iter().map(|detail| detail.duplicate_items).sum::<usize>(),
        "semantic_quality_proven": false,
        "note": "Labeled deterministic fake outputs validate accounting and regression shape only; they are not a real-model quality result."
    });
    (details, metrics)
}

fn dataset_fingerprint(path: &Path, bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    format!("sha256:{digest:x}:{}", path.display())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (fixtures, output) = parse_args().map_err(std::io::Error::other)?;
    let corpus_path = fixtures.join("corpus.json");
    let bytes = fs::read(&corpus_path)?;
    let corpus: Corpus = serde_json::from_slice(&bytes)?;
    if corpus.retrieval_queries.len() < 40
        || corpus
            .retrieval_queries
            .iter()
            .filter(|case| case.expected.is_empty())
            .count()
            < 10
        || corpus.generation_cases.len() < 15
    {
        return Err("quality corpus does not meet the minimum sample counts".into());
    }
    let (retrieval_details, retrieval_metrics, metadata_fidelity) =
        evaluate_retrieval(&corpus).await?;
    let (generation_details, generation_metrics) = evaluate_generation(&corpus);
    let retrieval_recall = retrieval_metrics["recall_at_5"]
        .as_f64()
        .unwrap_or_default();
    let false_return_rate = retrieval_metrics["no_answer_false_return_rate"]
        .as_f64()
        .unwrap_or(1.0);
    let metadata_rate = metadata_fidelity["exact_round_trip_rate"]
        .as_f64()
        .unwrap_or_default();
    let support_precision = generation_metrics["support_precision"]
        .as_f64()
        .unwrap_or_default();
    let critical_fact_coverage = generation_metrics["critical_fact_coverage"]
        .as_f64()
        .unwrap_or_default();
    // Measured from the immutable pre-change commit with this exact corpus.
    const BASELINE_GIT_HEAD: &str = "7b4913710ee9bdae7e8239eea80b78f9a6bef47e";
    const COMMON_TASK_RECALL_BASELINE: f64 = 28.0 / 30.0;
    const BASELINE_MRR_AT_5: f64 = 0.827_777_777_777_777_7;
    const BASELINE_NO_ANSWER_FALSE_RETURN_RATE: f64 = 1.0;
    let deterministic_pass = retrieval_recall >= COMMON_TASK_RECALL_BASELINE
        && false_return_rate <= 0.05
        && metadata_rate == 1.0
        && support_precision >= 0.95
        && critical_fact_coverage >= 0.90;
    let report = json!({
        "schema_version": 1,
        "mode": "deterministic",
        "corpus_version": corpus.version,
        "dataset_fingerprint": dataset_fingerprint(&corpus_path, &bytes),
        "git_head": env::var("MCP_VAULT_GIT_HEAD").ok(),
        "extraction_pipeline_version": EXTRACTION_PIPELINE_VERSION,
        "retrieval": retrieval_metrics,
        "generation": generation_metrics,
        "metadata_fidelity": metadata_fidelity,
        "acceptance": {
            "deterministic_pass": deterministic_pass,
            "common_task_recall_baseline": {
                "git_head": BASELINE_GIT_HEAD,
                "recall_at_5": COMMON_TASK_RECALL_BASELINE,
                "mrr_at_5": BASELINE_MRR_AT_5,
                "no_answer_false_return_rate": BASELINE_NO_ANSWER_FALSE_RETURN_RATE,
                "report": "target/quality/baseline.json"
            },
            "maximum_no_answer_false_return_rate": 0.05,
            "minimum_support_precision": 0.95,
            "minimum_critical_fact_coverage": 0.90,
            "real_model": {
                "status": "not_run",
                "reason": "No explicit Provider/data/cost authorization was supplied; deterministic fake output is not semantic quality evidence."
            }
        },
        "retrieval_details": retrieval_details,
        "generation_details": generation_details,
    });
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&output, serde_json::to_vec_pretty(&report)?)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    if !deterministic_pass {
        return Err(
            "deterministic memory-quality acceptance failed; inspect the written report".into(),
        );
    }
    Ok(())
}
