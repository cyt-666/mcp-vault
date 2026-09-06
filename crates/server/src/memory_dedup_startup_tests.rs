//! Golden old-schema startup through the production admission and durable worker.
#[path = "../../state/tests/support/pre_dedup_snapshot.rs"]
mod snapshot;
use crate::workers::{Cancellation, WorkerConfig, WorkerSupervisor, memory_dedup_job_handler};
use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
};
use mcp_vault_auth::{AuthService, MasterKeyRing};
use mcp_vault_core::VaultCore;
use mcp_vault_domain::{Actor, Revision, SourcePlane, VaultContext, VaultId, VaultPath, VaultSlug};
use mcp_vault_memory::{ExtractionPolicy, MemoryService, RememberInput};
use mcp_vault_providers::{
    ModelCapabilities, ModelInput, ModelSettings, ProviderInput, ProviderKind, ProviderMode,
    ProviderService, ProviderSettings,
};
use mcp_vault_state::{StateStore, VaultStatus};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::io::AsyncReadExt;

const SCALER: &str = "StandardScaler 对每个特征减去训练集均值，再除以训练集标准差。";
const ENGLISH: &str = "StandardScaler subtracts the training mean and divides by the training standard deviation for each feature.";
const LOSS: &str = "Loss: forward pass, calculate loss, backpropagate, update parameters; formula `w = w - lr * grad`.";
const SUMMARY: &str = "Loss: forward pass, calculate loss, backpropagate, update parameters.";
const SENTENCE: &str = "Use batch size 32. Use batch size 32.";
const SINGLE: &str = "Use batch size 32.";
const KV: &str = "KV Cache reuses attention keys and values during generation.";
const NOT_MEMORY: &str = "KV Cache is not durable long-term memory.";
#[derive(Default)]
struct Calls {
    offline: AtomicBool,
    extraction: AtomicUsize,
    judgment: AtomicUsize,
    embeddings: AtomicUsize,
    embedding_inputs: tokio::sync::Mutex<Vec<String>>,
}
fn equivalent(a: &str, b: &str) -> bool {
    a == b
        || [SCALER, ENGLISH].contains(&a) && [SCALER, ENGLISH].contains(&b)
        || [SENTENCE, SINGLE].contains(&a) && [SENTENCE, SINGLE].contains(&b)
}
async fn fake(State(calls): State<Arc<Calls>>, Json(body): Json<Value>) -> Response {
    if calls.offline.load(Ordering::SeqCst) {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error":"synthetic provider outage"})),
        )
            .into_response();
    }
    let system = body["messages"][0]["content"].as_str().unwrap();
    let user = body["messages"][1]["content"].as_str().unwrap();
    let output = if system.starts_with("Compare two untrusted") {
        calls.judgment.fetch_add(1, Ordering::SeqCst);
        let input: Value = serde_json::from_str(user).unwrap();
        let a = input["left"]["content"].as_str().unwrap();
        let b = input["right"]["content"].as_str().unwrap();
        let relation = if equivalent(a, b) {
            "equivalent"
        } else if a == LOSS && b == SUMMARY {
            "left_covers_right"
        } else if b == LOSS && a == SUMMARY {
            "right_covers_left"
        } else {
            "different"
        };
        json!({"left":0,"right":1,"relation":relation})
    } else if system.starts_with("Remove only repeated") {
        calls.judgment.fetch_add(1, Ordering::SeqCst);
        let input: Value = serde_json::from_str(user).unwrap();
        let content = input["content"].as_str().unwrap();
        json!({"content":if content == SENTENCE { SINGLE } else {content}})
    } else {
        calls.extraction.fetch_add(1, Ordering::SeqCst);
        let contents: Vec<&str> = if user.contains("fixture-scaler-a") {
            vec![SCALER]
        } else if user.contains("fixture-scaler-b") {
            vec![ENGLISH]
        } else if user.contains("fixture-loss") {
            vec![LOSS, SUMMARY]
        } else if user.contains("fixture-sentence") {
            vec![SENTENCE]
        } else {
            vec![KV, NOT_MEMORY]
        };
        json!({"memories":contents.into_iter().map(|content|json!({"content":content,"kind":"fact"})).collect::<Vec<_>>()})
    };
    Json(json!({"choices":[{"message":{"content":output.to_string()}}]})).into_response()
}
async fn fake_embeddings(State(calls): State<Arc<Calls>>, Json(body): Json<Value>) -> Json<Value> {
    calls.embeddings.fetch_add(1, Ordering::SeqCst);
    calls.embedding_inputs.lock().await.extend(
        body["input"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned()),
    );
    Json(
        json!({"data":body["input"].as_array().unwrap().iter().enumerate().map(|(index,input)| {
        let text=input.as_str().unwrap();
        let coordinate=if text.contains("StandardScaler") {0} else if text.contains("Loss") {1} else if text.contains("batch size") {2} else if text.contains("not durable") {3} else {4};
        let mut vector=vec![0.0_f32;8];vector[coordinate]=1.0;
        json!({"index":index,"embedding":vector})
    }).collect::<Vec<_>>()}),
    )
}
fn service(state: &StateStore) -> MemoryService {
    MemoryService::new(
        state.clone(),
        AuthService::new(
            state.auth(),
            MasterKeyRing::from_bytes(1, &[43; 32]).unwrap(),
        ),
    )
}

#[tokio::test]
async fn upgrade_existing_current_source_sets_auto_dedups_without_user_actions() {
    run_upgrade(false).await;
}

#[tokio::test]
async fn upgraded_worker_resumes_after_provider_recovers_without_user_actions() {
    run_upgrade(true).await;
}

async fn run_upgrade(simulated_outage: bool) {
    let dir = tempfile::tempdir().unwrap();
    let database = format!("sqlite://{}", dir.path().join("seed.sqlite").display());
    let state = StateStore::connect_and_migrate(&database).await.unwrap();
    let context = VaultContext::new(
        VaultId::new(),
        VaultSlug::new("dedup-upgrade").unwrap(),
        dir.path().join("vault"),
        Revision::ZERO,
    )
    .unwrap();
    state
        .vaults()
        .insert(&context, "Synthetic upgrade", VaultStatus::Active)
        .await
        .unwrap();
    let history = dir.path().join("history");
    let core = VaultCore::new(
        state.clone(),
        history.clone(),
        Default::default(),
        Default::default(),
        Default::default(),
    );
    let memory = service(&state);
    let calls = Arc::new(Calls::default());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let router = Router::new()
        .route("/v1/chat/completions", post(fake))
        .route("/v1/embeddings", post(fake_embeddings))
        .with_state(calls.clone());
    let provider_task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let providers = ProviderService::new(
        state.clone(),
        AuthService::new(
            state.auth(),
            MasterKeyRing::from_bytes(1, &[43; 32]).unwrap(),
        ),
    );
    providers
        .set_provider_mode(&context, ProviderMode::LocalOnly, None)
        .await
        .unwrap();
    let provider = providers
        .create_provider(ProviderInput {
            name: "Synthetic upgrade mechanism".into(),
            kind: ProviderKind::OpenAiCompatible,
            base_url: format!("http://{address}/v1/").parse().unwrap(),
            settings: ProviderSettings::default(),
            enabled: true,
            secret: None,
        })
        .await
        .unwrap();
    let model = providers
        .register_model(ModelInput {
            provider_id: provider.id,
            external_model_id: "fixture".into(),
            capabilities: ModelCapabilities {
                embeddings: true,
                dimension: Some(8),
                structured_output: true,
                max_output_tokens: Some(8192),
                ..Default::default()
            },
            settings: ModelSettings::default(),
            enabled: true,
        })
        .await
        .unwrap();
    providers
        .bind_model(
            Some(&context),
            "memory_extraction",
            model.id,
            json!({}),
            None,
        )
        .await
        .unwrap();
    memory
        .set_extraction_policy(
            &context,
            ExtractionPolicy {
                enabled: true,
                ..Default::default()
            },
            None,
            None,
        )
        .await
        .unwrap();
    let mut originals = Vec::new();
    for marker in ["scaler-a", "scaler-b", "loss", "sentence", "protected"] {
        let path = VaultPath::parse(&format!("notes/{marker}.md")).unwrap();
        let bytes = format!("# fixture-{marker}");
        let file = core
            .create_bytes(
                &context,
                &path,
                bytes.as_bytes(),
                Actor::system(),
                SourcePlane::System,
                None,
            )
            .await
            .unwrap()
            .file;
        memory.extract_note(&context, &core, &path).await.unwrap();
        originals.push((path, bytes, file.id));
    }
    // A real v2.1 canonical source set with an already-published duplicate and pause.
    let set = state
        .current_memory()
        .get_note_set_by_source(&context, originals[0].2)
        .await
        .unwrap()
        .unwrap();
    let mut read = core
        .read_managed(&context, &set.canonical_path)
        .await
        .unwrap();
    let mut bytes = Vec::new();
    read.reader.read_to_end(&mut bytes).await.unwrap();
    let text = String::from_utf8(bytes).unwrap();
    let (header, body) = text.split_once("```json\n").unwrap();
    let (body, trailer) = body.split_once("\n```").unwrap();
    let mut items: Vec<Value> = serde_json::from_str(body).unwrap();
    let mut duplicate = items[0].clone();
    duplicate["id"] = json!(mcp_vault_domain::MemoryId::new());
    duplicate["ordinal"] = json!(1);
    items.push(duplicate);
    let bytes = format!(
        "{}```json\n{}\n```{trailer}",
        header.replace("extraction_paused: false", "extraction_paused: true"),
        serde_json::to_string_pretty(&items).unwrap()
    );
    core.replace_managed_bytes(
        &context,
        &set.canonical_path,
        set.canonical_revision,
        bytes.as_bytes(),
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    memory.rebuild(&context, &core).await.unwrap();
    let explicit = memory
        .remember(
            &context,
            &core,
            RememberInput {
                content: SCALER.into(),
                confidence: Some(0.7),
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .memory
        .unwrap();
    // Reproduce the old v2.1 writer's null-valued empty YAML lists.
    let explicit_path = explicit.canonical_path.as_ref().unwrap();
    let mut read = core.read_managed(&context, explicit_path).await.unwrap();
    let mut bytes = Vec::new();
    read.reader.read_to_end(&mut bytes).await.unwrap();
    let old_explicit = String::from_utf8(bytes)
        .unwrap()
        .replace("tags: []\n", "tags:\n")
        .replace("entities: []\n", "entities:\n");
    core.replace_managed_bytes(
        &context,
        explicit_path,
        read.file.current_revision,
        old_explicit.as_bytes(),
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        memory.rebuild(&context, &core).await.unwrap().quarantined,
        0
    );
    providers
        .bind_model(
            Some(&context),
            "embedding_memory",
            model.id,
            json!({}),
            None,
        )
        .await
        .unwrap();
    let raw = state
        .current_memory()
        .contributions(&context, None, 200)
        .await
        .unwrap();
    let sources = raw
        .iter()
        .map(|bundle| mcp_vault_providers::EmbeddingSourceRef {
            object_type: "memory".into(),
            object_id: bundle.memory.id.to_string(),
            chunk_key: "body-v3:0000".into(),
            content_hash: bundle.memory.content_hash.clone(),
        })
        .collect::<Vec<_>>();
    memory
        .reembed_sources(&context, model.id, &sources)
        .await
        .unwrap();
    let old_vectors = state
        .providers()
        .list_embeddings(&context, model.id, "memory", 200, 0)
        .await
        .unwrap();
    assert!(!old_vectors.is_empty());
    let old_embedding_inputs = calls.embedding_inputs.lock().await.len();
    let old_path = dir.path().join("old.sqlite");
    state.snapshot_to(&old_path).await.unwrap();
    let old_database = format!("sqlite://{}", old_path.display());
    snapshot::strip_new_dedup_schema(&old_database).await;
    let extraction_calls = calls.extraction.load(Ordering::SeqCst);
    calls.offline.store(simulated_outage, Ordering::SeqCst);
    // From here on only application startup and normal reads: no generation,
    // model binding, canonical regeneration, migration API, or user action.
    let upgraded = StateStore::connect_and_migrate(&old_database)
        .await
        .unwrap();
    let memory = service(&upgraded);
    for restart in 0..2 {
        let before = calls.judgment.load(Ordering::SeqCst);
        let supervisor = WorkerSupervisor::new(
            upgraded.clone(),
            Arc::new(|_| Box::pin(async { Ok(()) })),
            WorkerConfig {
                poll_interval: Duration::from_millis(100),
                ..Default::default()
            },
        )
        .unwrap();
        supervisor
            .register_job_handler(
                "memory.deduplicate",
                memory_dedup_job_handler(
                    upgraded.clone(),
                    history.clone(),
                    Default::default(),
                    memory.clone(),
                ),
            )
            .unwrap();
        supervisor
            .register_job_handler(
                "vault.reconcile",
                Arc::new(|_, _| Box::pin(async { crate::workers::JobOutcome::Complete })),
            )
            .unwrap();
        let upgraded_providers = ProviderService::new(
            upgraded.clone(),
            AuthService::new(
                upgraded.auth(),
                MasterKeyRing::from_bytes(1, &[43; 32]).unwrap(),
            ),
        );
        supervisor
            .register_job_handler(
                "embedding.rebuild",
                crate::workers::embedding_job_handler(
                    upgraded.clone(),
                    mcp_vault_indexer::IndexService::with_provider_service(
                        upgraded.clone(),
                        upgraded_providers,
                    ),
                    memory.clone(),
                ),
            )
            .unwrap();
        crate::admit_memory_maintenance(&upgraded, &memory)
            .await
            .unwrap();
        let admitted_job = upgraded
            .jobs()
            .list(&context, None, Some("memory.deduplicate"), 100, 0)
            .await
            .unwrap()
            .into_iter()
            .find(|job| job.status != mcp_vault_state::JobStatus::Completed)
            .expect("startup must admit a maintenance job")
            .id;
        let stop = Cancellation::default();
        let runner = tokio::spawn({
            let worker = supervisor.clone();
            let stop = stop.clone();
            async move { worker.run(stop).await }
        });
        supervisor.wait_until_running().await;
        let tick_stop = Cancellation::default();
        let tick = tokio::spawn(crate::run_reconciliation_loop(
            upgraded.clone(),
            memory.clone(),
            Duration::from_secs(1),
            mcp_vault_domain::MaintenanceGate::new(),
            tick_stop.clone(),
        ));
        tokio::time::timeout(Duration::from_secs(150), async {
            let mut previous_status = String::new();
            loop {
                let status = upgraded
                    .current_memory()
                    .formal_status(&context)
                    .await
                    .unwrap();
                if previous_status != status.status {
                    eprintln!(
                        "synthetic upgrade restart={restart} outage={simulated_outage}: {status:?}"
                    );
                    previous_status = status.status.clone();
                }
                assert!(
                    !matches!(
                        status.status.as_str(),
                        "memory_markdown_invalid" | "memory_equivalence_output_invalid"
                    ),
                    "invalid maintenance output: {status:?}"
                );
                if status.retry_at > 0 && calls.offline.swap(false, Ordering::SeqCst) {
                    assert!(
                        status.adopted,
                        "local adoption must complete while provider is unavailable"
                    );
                    assert_eq!(
                        memory.get(&context, explicit.id).await.unwrap().confidence,
                        Some(0.7)
                    );
                }
                if status.status == "covered_candidates"
                    && status.pending_pairs == 0
                    && (restart == 0 || calls.judgment.load(Ordering::SeqCst) == before)
                    && upgraded
                        .jobs()
                        .get(&context, admitted_job)
                        .await
                        .unwrap()
                        .unwrap()
                        .status
                        == mcp_vault_state::JobStatus::Completed
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(30)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("maintenance failed; see persisted formal status"));
        tokio::time::sleep(Duration::from_millis(200)).await;
        tick_stop.cancel();
        tick.await.unwrap();
        stop.cancel();
        runner.await.unwrap();
        let completed_job = upgraded
            .jobs()
            .get(&context, admitted_job)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(completed_job.priority, 100);
        let progress = completed_job
            .progress
            .expect("maintenance must report progress before releasing its lease");
        assert_eq!(progress["phase"], "memory_dedup");
        assert_eq!(progress["pending_pairs"], 0);
        assert_eq!(progress["adopted"], true);
        assert!(progress["slices_completed"].as_u64().unwrap() > 0);
        if !simulated_outage && restart == 0 {
            assert!(progress["slices_completed"].as_u64().unwrap() > 1);
            assert_eq!(
                completed_job.attempts, 1,
                "ordinary slices must keep their worker lease"
            );
        }
        if restart == 1 {
            assert_eq!(
                calls.judgment.load(Ordering::SeqCst),
                before,
                "restart repeated successful judgments"
            );
        }
    }
    assert_eq!(calls.extraction.load(Ordering::SeqCst), extraction_calls);
    for input in calls
        .embedding_inputs
        .lock()
        .await
        .iter()
        .skip(old_embedding_inputs)
    {
        assert!(
            input.contains(&SINGLE.to_lowercase()) && !input.contains(&SENTENCE.to_lowercase()),
            "unchanged old input was reembedded: {input}"
        );
    }
    let current_vectors = upgraded
        .providers()
        .list_embeddings(&context, model.id, "memory", 200, 0)
        .await
        .unwrap();
    assert!(current_vectors.iter().any(|v| {
        old_vectors
            .iter()
            .any(|old| old.id == v.id && old.input_hash == v.input_hash)
    }));
    assert!(
        upgraded
            .jobs()
            .list(&context, None, Some("memory.extract"), 100, 0)
            .await
            .unwrap()
            .is_empty()
    );
    let all = memory
        .list(&context, vec![], None, None, None, 100, 0)
        .await
        .unwrap();
    assert_eq!(all.len(), 6, "{all:#?}");
    eprintln!(
        "synthetic upgrade outage={simulated_outage}: visible=9->6, old_vectors={}, reused_vectors={}, new_embedding_inputs={}, judgment_requests={}",
        old_vectors.len(),
        current_vectors
            .iter()
            .filter(|v| old_vectors
                .iter()
                .any(|old| old.id == v.id && old.input_hash == v.input_hash))
            .count(),
        calls.embedding_inputs.lock().await.len() - old_embedding_inputs,
        calls.judgment.load(Ordering::SeqCst)
    );
    let scaler = all
        .iter()
        .find(|m| m.id != explicit.id && [SCALER, ENGLISH].contains(&m.content.as_str()))
        .unwrap();
    assert_eq!(scaler.sources.len(), 2);
    assert!(all.iter().any(|m| m.content == LOSS));
    assert!(
        !all.iter()
            .any(|m| m.content == SUMMARY || m.content == SENTENCE)
    );
    assert!(all.iter().any(|m| m.content == SINGLE));
    assert!(all.iter().any(|m| m.content == KV));
    assert!(all.iter().any(|m| m.content == NOT_MEMORY));
    assert_eq!(
        memory.get(&context, explicit.id).await.unwrap().confidence,
        Some(0.7)
    );
    let upgraded_core = VaultCore::new(
        upgraded.clone(),
        history,
        Default::default(),
        Default::default(),
        Default::default(),
    );
    for (path, bytes, _) in &originals {
        let mut read = upgraded_core.read(&context, path).await.unwrap();
        let mut actual = Vec::new();
        read.reader.read_to_end(&mut actual).await.unwrap();
        assert_eq!(actual, bytes.as_bytes());
    }
    assert!(
        upgraded
            .current_memory()
            .get_note_set_by_source(&context, originals[0].2)
            .await
            .unwrap()
            .unwrap()
            .extraction_paused
    );
    let judgments = calls.judgment.load(Ordering::SeqCst);
    snapshot::assert_public_fts_only(&old_database, &context.id().to_string(), all.len() as i64)
        .await;
    snapshot::clear_derived_memory_projections(&old_database).await;
    let rebuild = memory.rebuild(&context, &upgraded_core).await.unwrap();
    assert_eq!(rebuild.quarantined, 0, "cold rebuild {rebuild:?}");
    snapshot::assert_public_fts_only(&old_database, &context.id().to_string(), all.len() as i64)
        .await;
    let rebuilt = memory
        .list(&context, vec![], None, None, None, 100, 0)
        .await
        .unwrap();
    assert_eq!(
        all.iter()
            .map(|m| (m.id, m.content.clone(), m.revision, m.sources.len()))
            .collect::<Vec<_>>(),
        rebuilt
            .iter()
            .map(|m| (m.id, m.content.clone(), m.revision, m.sources.len()))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        judgments,
        calls.judgment.load(Ordering::SeqCst),
        "cold Markdown rebuild called a model"
    );
    provider_task.abort();
}
